//! ProTracker's native tick-zero and per-tick effect processor.

#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::{ChannelId, DirtyBits, Frame, I1F15, InstrumentId, Note, Step, TempoModel, U0F16, VoiceParams};
use starplayer_engine::{EndOfSongPolicy, Jump, OrderEntry, PatternData, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TraceChannelState, TrackerProcessor};
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{EffectNames, LoopMode, Module, OrderEntry as ModelOrderEntry};
use starplayer_rt::Arc;

use crate::pattern::{CELL_BYTES, ModCell};
use crate::tables::{FINETUNE_REFERENCE_RATES, PROTRACKER_PERIODS, extended_period};

const NO_SAMPLE: u8 = 0;
const NO_NOTE: u8 = u8::MAX;
const DEFAULT_PERIOD: u32 = 428;
const PAULA_PAL_CLOCK_HZ: u64 = 3_546_895;
const VIBRATO_TABLE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253,
    255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24,
];

/// Format-native state for one ProTracker channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModChannel {
    pub channel_number: u8,
    pub sample_number: u8,
    pub pending_dirty: DirtyBits,
    pub sample_offset: u32,
    pub current_volume: u8,
    pub actual_volume: u8,
    pub current_note: u8,
    pub current_period: u32,
    pub target_note: u8,
    pub target_period: u32,
    pub actual_period: u32,
    pub command: u8,
    pub command_data: u8,
    pub portamento_memory: u8,
    pub vibrato_memory: u8,
    pub tremolo_memory: u8,
    pub offset_memory: u8,
    /// Offset added to ProTracker's retained sample pointer after this row's note has
    /// been started. This models the PT 1/2 double-offset quirk without mutating PCM.
    offset_post_trigger: u32,
    pub vibrato_phase: u8,
    pub tremolo_phase: u8,
    pub vibrato_waveform: u8,
    pub tremolo_waveform: u8,
    pub finetune: u8,
    pub pan: I1F15,
    pub glissando_enabled: bool,
    pub pattern_loop_start: u16,
    pub pattern_loop_count: u8,
    pub delayed_note: bool,
    pub offset_past_end: bool,
    /// Last EFx value. EF is parsed but deliberately has no PCM mutation; see
    /// `plans/reference/format-notes-mod.md`.
    pub invert_loop_speed: u8,
}

impl ModChannel {
    fn new(channel_number: u8, pan: I1F15) -> ModChannel {
        ModChannel {
            channel_number,
            sample_number: NO_SAMPLE,
            pending_dirty: DirtyBits::PAN,
            sample_offset: 0,
            current_volume: 0,
            actual_volume: 0,
            current_note: NO_NOTE,
            current_period: DEFAULT_PERIOD,
            target_note: NO_NOTE,
            target_period: 0,
            actual_period: DEFAULT_PERIOD,
            command: 0,
            command_data: 0,
            portamento_memory: 0,
            vibrato_memory: 0,
            tremolo_memory: 0,
            offset_memory: 0,
            offset_post_trigger: 0,
            vibrato_phase: 0,
            tremolo_phase: 0,
            vibrato_waveform: 0,
            tremolo_waveform: 0,
            finetune: 0,
            pan,
            glissando_enabled: false,
            pattern_loop_start: 0,
            pattern_loop_count: 0,
            delayed_note: false,
            offset_past_end: false,
            invert_loop_speed: 0,
        }
    }
}

/// Owned native MOD pattern access for the generic sequencer.
#[derive(Clone, Debug)]
pub struct ModPatternData(pub Arc<Module>);

impl PatternData for ModPatternData {
    fn order_count(&self) -> u16 { self.0.orders().len().min(u16::MAX as usize) as u16 }

    fn order(&self, order: u16) -> Option<OrderEntry> {
        match self.0.order_entry(order as usize)? {
            ModelOrderEntry::Pattern(pattern) => Some(OrderEntry::Pattern(pattern.0)),
            ModelOrderEntry::Marker => Some(OrderEntry::Skip),
            ModelOrderEntry::End => Some(OrderEntry::End),
        }
    }

    fn channel_count(&self) -> u8 { self.0.header().channel_count }
    fn rows_in_pattern(&self, pattern: u16) -> Option<u16> { self.0.pattern(starplayer_model::PatternId(pattern)).map(|index| index.rows()) }

    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]> {
        let pattern_id = starplayer_model::PatternId(pattern);
        let index = self.0.pattern(pattern_id)?;
        if row >= index.rows() { return None; }
        let row_length = index.channels() as usize * CELL_BYTES;
        let start = row as usize * row_length;
        self.0.pattern_bytes(pattern_id)?.get(start..start + row_length)
    }
}

/// Stateful ProTracker replay semantics. No S3M command or period representation enters
/// this type.
pub struct ModProcessor {
    module: Arc<Module>,
    channels: Box<[ModChannel]>,
    sample_rate_hz: u32,
    amiga_limits: bool,
    last_pattern: Option<u16>,
    row_pattern_break: bool,
    waveform_random_state: u32,
}

impl ModProcessor {
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> ModProcessor {
        let channels = (0..module.header().channel_count).map(|channel| {
            let pan = module.header().channel_pan(channel).unwrap_or(I1F15::ZERO);
            ModChannel::new(channel, pan)
        }).collect::<Vec<_>>().into_boxed_slice();
        ModProcessor {
            amiga_limits: module.header().flags.amiga_limits,
            module,
            channels,
            sample_rate_hz,
            last_pattern: None,
            row_pattern_break: false,
            // Unlike libxmp's wall-clock seed, a fixed seed preserves StarPlayer's
            // byte-identical replay invariant while still implementing waveform 3.
            waveform_random_state: 0x6D2B_79F5,
        }
    }

    pub fn channels(&self) -> &[ModChannel] { &self.channels }
    pub fn channel(&self, channel: u8) -> Option<&ModChannel> { self.channels.get(channel as usize) }

    fn reset_row(&mut self) {
        for state in self.channels.iter_mut() {
            if state.actual_period != state.current_period {
                state.actual_period = state.current_period;
                state.pending_dirty.insert(DirtyBits::PITCH);
            }
            if state.actual_volume != state.current_volume {
                state.actual_volume = state.current_volume;
                state.pending_dirty.insert(DirtyBits::VOLUME);
            }
            state.command = 0;
            state.command_data = 0;
            state.delayed_note = false;
            state.offset_past_end = false;
            state.offset_post_trigger = 0;
        }
    }

    fn latch_cell(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: ModCell) -> bool {
        let channel_id = ChannelId(channel_index as u16);
        let effect_name = if cell.effect == 0 && cell.param == 0 { "" } else { EffectNames::MOD.name(cell.effect, cell.param).unwrap_or("") };
        context.report_effect(channel_id, cell.effect, cell.param, effect_name);
        if cell.instrument != 0 && cell.instrument <= 31 {
            let instrument_id = InstrumentId((cell.instrument - 1) as u16);
            if let Some(sample_id) = self.module.instrument(instrument_id).and_then(|instrument| instrument.sample)
                && let Some(sample) = self.module.sample(sample_id)
            {
                let state = &mut self.channels[channel_index];
                state.sample_number = cell.instrument;
                state.finetune = finetune_from_rate(sample.reference_rate_hz());
                state.sample_offset = 0;
                let volume = ((sample.default_volume().to_bits() as u32 * 64 + 32_767) / 65_535) as u8;
                state.current_volume = volume;
                state.actual_volume = volume;
                state.pending_dirty.insert(DirtyBits::VOLUME);
            }
        }

        // ProTracker setPeriod scans the zero-finetune row to identify the packed
        // period's note slot, then reads that slot from the newly latched instrument's
        // finetuned row. Keep those two operations visibly ordered: a raw 428 with
        // finetune +7 is still C, but plays the +7 table's period 407.
        let note = cell.linear_note();
        if note.is_some() || cell.instrument != 0 { context.report_note(channel_id, note.map(Note::new), (cell.instrument != 0).then_some(cell.instrument)); }

        // E5 changes the finetune used to look up a note on the same row.
        if cell.effect == 0xE && cell.param >> 4 == 5 { self.channels[channel_index].finetune = cell.param & 15; }

        let Some(note) = note else { return false };
        let period = extended_period(self.channels[channel_index].finetune, note);
        if matches!(cell.effect, 3 | 5) {
            let state = &mut self.channels[channel_index];
            state.target_note = note;
            state.target_period = period;
            return true;
        }

        let delayed = cell.effect == 0xE && cell.param >> 4 == 0xD && cell.param & 15 != 0;
        let state = &mut self.channels[channel_index];
        state.current_note = note;
        state.current_period = period;
        state.actual_period = period;
        state.delayed_note = delayed;
        if !delayed { state.pending_dirty.insert(DirtyBits::SAMPLE | DirtyBits::PITCH); }
        if state.vibrato_waveform & 4 == 0 { state.vibrato_phase = 0; }
        if state.tremolo_waveform & 4 == 0 { state.tremolo_phase = 0; }
        self.refresh_offset_past_end(channel_index);
        true
    }

    fn static_effect(&mut self, channel_index: usize, cell: ModCell, note_present: bool, row: u16, outcome: &mut TickOutcome) {
        self.channels[channel_index].command = cell.effect;
        self.channels[channel_index].command_data = cell.param;
        match cell.effect {
            0 if cell.param == 0 => self.clear_command(channel_index),
            0 | 1 | 2 | 5 | 0xA => {}
            3 => {
                if cell.param != 0 { self.channels[channel_index].portamento_memory = cell.param; }
            }
            4 => self.remember_vibrato(channel_index, cell.param),
            6 => self.remember_vibrato(channel_index, 0),
            7 => self.remember_tremolo(channel_index, cell.param),
            8 => {
                self.channels[channel_index].pan = pan_byte(cell.param);
                self.channels[channel_index].pending_dirty.insert(DirtyBits::PAN);
                self.clear_command(channel_index);
            }
            9 => self.sample_offset(channel_index, note_present, cell.param),
            0xB => {
                // ProTracker's later-channel Bxx resets a Dxx row already seen.
                outcome.jump = Some(Jump::to_order(cell.param as u16));
                self.row_pattern_break = false;
                self.clear_command(channel_index);
            }
            0xC => {
                self.set_volume(channel_index, cell.param.min(64));
                self.clear_command(channel_index);
            }
            0xD => {
                // The packed byte is BCD in ProTracker; invalid results wrap to row zero.
                let decoded = (cell.param >> 4) as u16 * 10 + (cell.param & 15) as u16;
                let break_row = if decoded > 63 { 0 } else { decoded };
                let order = outcome.jump.filter(|jump| !jump.within_pattern).and_then(|jump| jump.order);
                outcome.jump = Some(order.map(|order| Jump::to_order_row(order, break_row)).unwrap_or_else(|| Jump::break_to_row(break_row)));
                self.row_pattern_break = true;
                self.clear_command(channel_index);
            }
            0xE => self.static_extended(channel_index, cell.param, note_present, row, outcome),
            0xF if cell.param == 0 => {
                outcome.stop = true;
                self.clear_command(channel_index);
            }
            0xF if cell.param < 32 => {
                outcome.speed = cell.param;
                self.clear_command(channel_index);
            }
            0xF => {
                outcome.tempo_bpm = cell.param as u16;
                self.channels[channel_index].pending_dirty.insert(DirtyBits::TEMPO);
                self.clear_command(channel_index);
            }
            _ => self.clear_command(channel_index),
        }
    }

    fn static_extended(&mut self, channel_index: usize, parameter: u8, note_present: bool, row: u16, outcome: &mut TickOutcome) {
        let subcommand = parameter >> 4;
        let value = parameter & 15;
        match subcommand {
            0 => self.clear_command(channel_index), // Amiga low-pass filter: intentionally a no-op.
            1 => self.slide_period(channel_index, false, value as u32),
            2 => self.slide_period(channel_index, true, value as u32),
            3 => { self.channels[channel_index].glissando_enabled = value != 0; self.clear_command(channel_index); }
            4 => { self.channels[channel_index].vibrato_waveform = value & 7; self.clear_command(channel_index); }
            5 => { self.channels[channel_index].finetune = value; self.clear_command(channel_index); }
            6 => {
                if value == 0 {
                    self.channels[channel_index].pattern_loop_start = row;
                } else if self.channels[channel_index].pattern_loop_count == 0 {
                    self.channels[channel_index].pattern_loop_count = value;
                    outcome.jump = Some(Jump::within_pattern_to_row(self.channels[channel_index].pattern_loop_start));
                } else {
                    self.channels[channel_index].pattern_loop_count -= 1;
                    if self.channels[channel_index].pattern_loop_count != 0 {
                        outcome.jump = Some(Jump::within_pattern_to_row(self.channels[channel_index].pattern_loop_start));
                    }
                }
                self.clear_command(channel_index);
            }
            7 => { self.channels[channel_index].tremolo_waveform = value & 7; self.clear_command(channel_index); }
            8 => {
                self.channels[channel_index].pan = pan_byte(value << 4);
                self.channels[channel_index].pending_dirty.insert(DirtyBits::PAN);
                self.clear_command(channel_index);
            }
            9 if value == 0 => self.clear_command(channel_index),
            9 => {
                // E9x without a note retriggers on tick zero; a note on this row already
                // performed that restart and must not be triggered twice.
                if !note_present { self.retrigger(channel_index); }
            }
            0xA => self.slide_volume(channel_index, value, 0),
            0xB => self.slide_volume(channel_index, 0, value),
            0xC if value == 0 => { self.cut_note(channel_index); self.clear_command(channel_index); }
            0xC => {}
            0xD if value == 0 => { self.trigger_delayed(channel_index); self.clear_command(channel_index); }
            0xD => {}
            0xE => { outcome.pattern_delay = value; }
            0xF => {
                // EF mutates bytes in the shared sample allocation in PT. Module PCM is
                // immutable and shared with the RT mixer, so doing that here would need
                // either a lock or allocation in render. Record the parsed value and
                // leave audio unchanged; the exclusion is explicit in the format notes.
                self.channels[channel_index].invert_loop_speed = value;
                self.clear_command(channel_index);
            }
            _ => {}
        }
    }

    fn minor_effect(&mut self, channel_index: usize, local_tick: u8, repeat_zero: bool) {
        let command = self.channels[channel_index].command;
        let parameter = self.channels[channel_index].command_data;
        match command {
            0 if parameter != 0 => self.arpeggio(channel_index, local_tick),
            1 => self.slide_period(channel_index, false, parameter as u32),
            2 => self.slide_period(channel_index, true, parameter as u32),
            3 => self.tone_portamento(channel_index),
            4 => self.vibrato(channel_index),
            5 => { self.tone_portamento(channel_index); self.volume_slide(channel_index, parameter); }
            6 => { self.vibrato(channel_index); self.volume_slide(channel_index, parameter); }
            7 => self.tremolo(channel_index),
            0xA => self.volume_slide(channel_index, parameter),
            0xE => self.minor_extended(channel_index, parameter, local_tick, repeat_zero),
            _ => {}
        }
    }

    fn minor_extended(&mut self, channel_index: usize, parameter: u8, local_tick: u8, repeat_zero: bool) {
        let subcommand = parameter >> 4;
        let value = parameter & 15;
        match subcommand {
            1 if repeat_zero => self.slide_period(channel_index, false, value as u32),
            2 if repeat_zero => self.slide_period(channel_index, true, value as u32),
            9 if value != 0 && local_tick.is_multiple_of(value) => self.retrigger(channel_index),
            0xA if repeat_zero => self.slide_volume(channel_index, value, 0),
            0xB if repeat_zero => self.slide_volume(channel_index, 0, value),
            0xC if local_tick == value => self.cut_note(channel_index),
            0xD if local_tick == value => self.trigger_delayed(channel_index),
            _ => {}
        }
    }

    fn remember_vibrato(&mut self, channel_index: usize, parameter: u8) {
        let old = self.channels[channel_index].vibrato_memory;
        let speed = if parameter & 0xF0 == 0 { old & 0xF0 } else { parameter & 0xF0 };
        let depth = if parameter & 15 == 0 { old & 15 } else { parameter & 15 };
        self.channels[channel_index].vibrato_memory = speed | depth;
    }

    fn remember_tremolo(&mut self, channel_index: usize, parameter: u8) {
        let old = self.channels[channel_index].tremolo_memory;
        let speed = if parameter & 0xF0 == 0 { old & 0xF0 } else { parameter & 0xF0 };
        let depth = if parameter & 15 == 0 { old & 15 } else { parameter & 15 };
        self.channels[channel_index].tremolo_memory = speed | depth;
    }

    fn sample_offset(&mut self, channel_index: usize, note_present: bool, parameter: u8) {
        if parameter != 0 { self.channels[channel_index].offset_memory = parameter; }
        if !note_present { return; }
        let offset = (self.channels[channel_index].offset_memory as u32) << 8;
        self.channels[channel_index].sample_offset = offset;
        // PT 1/2 starts this note at the requested offset, then advances the retained
        // sample pointer by the same amount once more. A later note without a new
        // instrument consequently starts at twice the original offset.
        self.channels[channel_index].offset_post_trigger = offset;
        self.refresh_offset_past_end(channel_index);
    }

    fn refresh_offset_past_end(&mut self, channel_index: usize) {
        let offset = self.channels[channel_index].sample_offset;
        self.channels[channel_index].offset_past_end = self.sample_index(channel_index).is_some_and(|sample| offset >= sample.length_frames());
    }

    fn finish_row_effects(&mut self, channel_index: usize) {
        let added = self.channels[channel_index].offset_post_trigger;
        if added == 0 { return; }
        self.channels[channel_index].sample_offset = self.channels[channel_index].sample_offset.saturating_add(added);
        self.channels[channel_index].offset_post_trigger = 0;
        self.refresh_offset_past_end(channel_index);
    }

    fn finish_row_outcome(&self, current_order: u16, outcome: &mut TickOutcome) {
        if outcome.pattern_delay == 0 || !self.row_pattern_break { return; }
        let Some(jump) = outcome.jump.as_mut() else { return };
        if jump.within_pattern { return; }

        // PT advances once more after the delayed break target has been selected. MOD
        // patterns are always 64 rows, so skipping row 63 means entering the following
        // order at row zero.
        let target_row = jump.row.unwrap_or(0);
        if target_row < 63 {
            jump.row = Some(target_row + 1);
        } else {
            jump.order = Some(jump.order.unwrap_or_else(|| current_order.saturating_add(1)).saturating_add(1));
            jump.row = Some(0);
        }
    }

    fn arpeggio(&mut self, channel_index: usize, tick: u8) {
        let state = &mut self.channels[channel_index];
        if state.current_note == NO_NOTE { return; }
        let semitones = match tick % 3 { 1 => state.command_data >> 4, 2 => state.command_data & 15, _ => 0 };
        state.actual_period = if self.amiga_limits && (36..=71).contains(&state.current_note) {
            let table_index = (state.current_note - 36) as usize + semitones as usize;
            let wrapped = table_index % 37;
            if wrapped == 36 { 0 } else { PROTRACKER_PERIODS[(state.finetune & 15) as usize][wrapped] as u32 }
        } else {
            extended_period(state.finetune, state.current_note.saturating_add(semitones).min(95))
        };
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn slide_period(&mut self, channel_index: usize, down: bool, amount: u32) {
        let state = &mut self.channels[channel_index];
        state.current_period = if down { state.current_period.saturating_add(amount) } else { state.current_period.saturating_sub(amount).max(1) };
        state.current_period = clamp_period(self.amiga_limits, state.finetune, state.current_period);
        state.actual_period = state.current_period;
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn tone_portamento(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        if state.target_period == 0 { return; }
        let amount = state.portamento_memory as u32;
        if state.current_period < state.target_period {
            state.current_period = state.current_period.saturating_add(amount).min(state.target_period);
        } else if state.current_period > state.target_period {
            state.current_period = state.current_period.saturating_sub(amount).max(state.target_period);
        }
        if state.current_period == state.target_period {
            state.target_period = 0;
            state.target_note = NO_NOTE;
        }
        state.current_period = clamp_period(self.amiga_limits, state.finetune, state.current_period);
        state.actual_period = if state.glissando_enabled { glissando_period(state.finetune, state.current_period) } else { state.current_period };
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn vibrato(&mut self, channel_index: usize) {
        let (selector, phase, depth) = {
            let state = &self.channels[channel_index];
            (state.vibrato_waveform, state.vibrato_phase, state.vibrato_memory & 15)
        };
        let waveform = self.waveform_value(selector, phase);
        let delta = (waveform as i32 * depth as i32) >> 7;
        let state = &mut self.channels[channel_index];
        state.actual_period = add_signed(state.current_period, delta);
        state.actual_period = clamp_period(self.amiga_limits, state.finetune, state.actual_period);
        state.vibrato_phase = state.vibrato_phase.wrapping_add((state.vibrato_memory >> 4) << 2);
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn tremolo(&mut self, channel_index: usize) {
        let (selector, phase, depth) = {
            let state = &self.channels[channel_index];
            (state.tremolo_waveform, state.tremolo_phase, state.tremolo_memory & 15)
        };
        let waveform = self.waveform_value(selector, phase);
        let delta = (waveform as i32 * depth as i32) >> 6;
        let state = &mut self.channels[channel_index];
        state.actual_volume = (state.current_volume as i32 + delta).clamp(0, 64) as u8;
        state.tremolo_phase = state.tremolo_phase.wrapping_add((state.tremolo_memory >> 4) << 2);
        state.pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn volume_slide(&mut self, channel_index: usize, parameter: u8) {
        if parameter & 0xF0 != 0 { self.slide_volume(channel_index, parameter >> 4, 0); }
        else { self.slide_volume(channel_index, 0, parameter & 15); }
    }

    fn slide_volume(&mut self, channel_index: usize, up: u8, down: u8) {
        let state = &mut self.channels[channel_index];
        state.current_volume = if up != 0 { state.current_volume.saturating_add(up).min(64) } else { state.current_volume.saturating_sub(down) };
        state.actual_volume = state.current_volume;
        state.pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn set_volume(&mut self, channel_index: usize, volume: u8) {
        self.channels[channel_index].current_volume = volume;
        self.channels[channel_index].actual_volume = volume;
        self.channels[channel_index].pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn retrigger(&mut self, channel_index: usize) {
        if self.channels[channel_index].sample_number == NO_SAMPLE || self.channels[channel_index].current_note == NO_NOTE { return; }
        self.channels[channel_index].sample_offset = 0;
        self.channels[channel_index].offset_past_end = false;
        self.channels[channel_index].pending_dirty.insert(DirtyBits::SAMPLE);
    }

    fn trigger_delayed(&mut self, channel_index: usize) {
        if !self.channels[channel_index].delayed_note && self.channels[channel_index].current_note == NO_NOTE { return; }
        self.channels[channel_index].delayed_note = false;
        if self.channels[channel_index].sample_number != NO_SAMPLE { self.channels[channel_index].pending_dirty.insert(DirtyBits::SAMPLE | DirtyBits::PITCH); }
    }

    fn cut_note(&mut self, channel_index: usize) { self.set_volume(channel_index, 0); }

    fn waveform_value(&mut self, selector: u8, phase: u8) -> i16 {
        if selector & 3 == 3 {
            let mut random = self.waveform_random_state;
            if random == 0 { random = 1; }
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            self.waveform_random_state = random;
            return ((random >> 23) as i16 & 511) - 256;
        }
        waveform_value(selector, phase)
    }

    fn sample_index(&self, channel_index: usize) -> Option<&starplayer_model::SampleIndex> {
        let number = self.channels[channel_index].sample_number;
        if number == NO_SAMPLE { return None; }
        self.module.instrument(InstrumentId((number - 1) as u16)).and_then(|instrument| instrument.sample).and_then(|sample| self.module.sample(sample))
    }

    fn flush_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let dirty = self.channels[channel_index].pending_dirty;
        if dirty.is_empty() { return; }
        let channel_id = ChannelId(channel_index as u16);
        if dirty.contains(DirtyBits::SAMPLE) {
            let Some(sample) = self.sample_index(channel_index) else {
                context.stop_channel(channel_id);
                self.channels[channel_index].pending_dirty = DirtyBits::empty();
                return;
            };
            let region = if self.channels[channel_index].offset_past_end {
                // PT leaves the sample pointer at its start and forces a one-word length.
                SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames().min(2))
            } else {
                sample_region(sample)
            };
            let state = &self.channels[channel_index];
            let tag = VoiceTag {
                channel: channel_index as u8,
                instrument: state.sample_number,
                sample: state.sample_number,
                note: if state.current_note == NO_NOTE { 0 } else { state.current_note },
            };
            let offset = if state.offset_past_end { 0 } else { state.sample_offset };
            context.trigger_channel(channel_id, tag, region, self.voice_params(channel_index, dirty), offset);
        } else if let Some(voice) = context.channels.foreground(channel_id) {
            let state = &self.channels[channel_index];
            if dirty.contains(DirtyBits::PITCH) { context.write_voice_param(voice, starplayer_core::VoiceParam::Step(step_from_period(state.actual_period, self.sample_rate_hz))); }
            if dirty.contains(DirtyBits::VOLUME) { context.write_voice_param(voice, starplayer_core::VoiceParam::Volume(unit_from_ratio(state.actual_volume as u32, 64))); }
            if dirty.contains(DirtyBits::PAN) { context.write_voice_param(voice, starplayer_core::VoiceParam::Pan(state.pan)); }
            if dirty.contains(DirtyBits::TEMPO) { context.mark_voice_dirty(voice, DirtyBits::TEMPO); }
        }
        self.channels[channel_index].pending_dirty = DirtyBits::empty();
        if dirty.contains(DirtyBits::SAMPLE) { self.finish_row_effects(channel_index); }
    }

    fn voice_params(&self, channel_index: usize, dirty: DirtyBits) -> VoiceParams {
        let state = &self.channels[channel_index];
        VoiceParams {
            step: step_from_period(state.actual_period, self.sample_rate_hz),
            volume: unit_from_ratio(state.actual_volume as u32, 64),
            pan: state.pan,
            dirty,
            ..VoiceParams::SILENT
        }
    }

    fn report_trace_channels(&self, context: &mut TickContext<'_>) {
        for (channel_index, state) in self.channels.iter().enumerate() {
            let sample = if state.sample_number == NO_SAMPLE { 0 } else { state.sample_number as u16 };
            context.report_trace_channel(ChannelId(channel_index as u16), TraceChannelState {
                note: (state.current_note != NO_NOTE).then_some(state.current_note),
                instrument: sample,
                sample,
                volume: state.actual_volume as u16,
                period: state.actual_period,
                pan: pan_trace(state.pan),
            });
        }
    }

    fn clear_command(&mut self, channel_index: usize) {
        self.channels[channel_index].command = 0;
        self.channels[channel_index].command_data = 0;
    }
}

impl TrackerProcessor for ModProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        context.report_global_volume(U0F16::MAX);
        if self.last_pattern.is_some_and(|pattern| pattern != row.pattern) {
            for state in self.channels.iter_mut() { state.pattern_loop_start = 0; }
        }
        self.last_pattern = Some(row.pattern);
        self.row_pattern_break = false;
        self.reset_row();
        let mut outcome = context.outcome();
        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        for channel_index in 0..channel_count {
            let start = channel_index * CELL_BYTES;
            let Some(cell) = ModCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])) else { continue };
            let note_present = self.latch_cell(context, channel_index, cell);
            self.static_effect(channel_index, cell, note_present, row.row, &mut outcome);
            let state = &mut self.channels[channel_index];
            state.actual_period = clamp_period(self.amiga_limits, state.finetune, state.actual_period);
        }
        self.finish_row_outcome(row.order, &mut outcome);
        for channel_index in 0..self.channels.len() {
            self.flush_channel(context, channel_index);
        }
        self.report_trace_channels(context);
        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        context.report_global_volume(U0F16::MAX);
        let outcome = context.outcome();
        let local_tick = if context.row_clock.speed == 0 { 0 } else { (context.row_clock.tick_in_row % context.row_clock.speed as u16) as u8 };
        let repeat_zero = context.row_clock.is_first_tick_of_repeat();
        for channel_index in 0..self.channels.len() {
            self.minor_effect(channel_index, local_tick, repeat_zero);
            self.flush_channel(context, channel_index);
        }
        self.report_trace_channels(context);
        outcome
    }
}

/// Build a public MOD sequencer using ProTracker timing and native period semantics.
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, ModProcessor, ModPatternData> {
    let settings = SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Loop,
    };
    PatternSequencer::new(tempo_model, ModPatternData(Arc::clone(&module)), ModProcessor::new(module, sample_rate_hz), settings)
}

fn clamp_period(amiga_limits: bool, finetune: u8, period: u32) -> u32 {
    if !amiga_limits { return period.max(1); }
    let table = &PROTRACKER_PERIODS[(finetune & 15) as usize];
    period.clamp(table[35] as u32, table[0] as u32)
}

fn glissando_period(finetune: u8, period: u32) -> u32 {
    for &candidate in &PROTRACKER_PERIODS[(finetune & 15) as usize] {
        if candidate as u32 <= period { return candidate as u32; }
    }
    PROTRACKER_PERIODS[(finetune & 15) as usize][35] as u32
}

fn waveform_value(selector: u8, phase: u8) -> i16 {
    let index = ((phase >> 2) & 31) as usize;
    match selector & 3 {
        0 if phase < 128 => VIBRATO_TABLE[index] as i16,
        0 => -(VIBRATO_TABLE[index] as i16),
        1 if phase < 128 => ((index as u8) << 3) as i16,
        1 => -(255i16.saturating_sub(((index as u8) << 3) as i16)),
        2 if phase < 128 => 255,
        2 => -255,
        _ => 0,
    }
}

fn add_signed(value: u32, delta: i32) -> u32 {
    if delta < 0 { value.saturating_sub(delta.unsigned_abs()).max(1) } else { value.saturating_add(delta as u32) }
}

fn step_from_period(period: u32, sample_rate_hz: u32) -> Step {
    if period == 0 { Step::ZERO } else { Step::from_ratio(PAULA_PAL_CLOCK_HZ, period as u64 * sample_rate_hz.max(1) as u64) }
}

fn sample_region(sample: &starplayer_model::SampleIndex) -> SampleRegion {
    match sample.loop_mode() {
        LoopMode::Forward => LoopSpan::new(sample.loop_start(), sample.loop_end()).map(|span| SampleRegion::looping(sample.pcm_offset(), span)).unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::PingPong => LoopSpan::ping_pong(sample.loop_start(), sample.loop_end()).map(|span| SampleRegion::looping(sample.pcm_offset(), span)).unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::None => SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames()),
    }
}

fn finetune_from_rate(rate: u32) -> u8 {
    FINETUNE_REFERENCE_RATES.iter().position(|candidate| *candidate == rate).unwrap_or(0) as u8
}

fn pan_byte(value: u8) -> I1F15 { bipolar_from_ratio(value as i32 * 2 - 255, 255) }
fn pan_trace(pan: I1F15) -> u16 {
    let scaled = ((pan.to_bits() as i32 + 32_768) * 255 + 32_767) / 65_535;
    scaled.clamp(0, 255) as u16
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use super::*;
    use starplayer_core::RowClock;
    use starplayer_engine::{ChannelTable, SongPosition};
    use starplayer_mixer::VoicePool;
    use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};

    fn processor_with_sample(amiga_limits: bool, reference_rate_hz: u32, sample_frames: usize) -> ModProcessor {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![0; 64 * CELL_BYTES], 64, 1).expect("pattern");
        let sample_spec = SampleSpec::one_shot("sample").with_reference_rate(reference_rate_hz);
        let sample = builder.add_sample(&vec![0; sample_frames], sample_spec).expect("sample");
        builder.add_instrument(InstrumentDef::from_sample("sample", sample, U0F16::MAX)).expect("instrument");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::Mod, 1);
        header.flags.amiga_limits = amiga_limits;
        builder.set_header(header);
        ModProcessor::new(Arc::new(builder.build().expect("module")), 44_100)
    }

    fn process_row(processor: &mut ModProcessor, cell: ModCell) -> TickOutcome {
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        let bytes = cell.to_bytes();
        processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes })
    }

    fn process_tick(processor: &mut ModProcessor, absolute_tick: u16, pattern_delay: u8) -> TickOutcome {
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut clock = RowClock::new(6);
        clock.pattern_delay = pattern_delay;
        clock.tick_in_row = absolute_tick;
        clock.repeat_index = (absolute_tick / 6) as u8;
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, clock, SongPosition::default(), 125);
        processor.tick(&mut context)
    }

    #[test]
    fn raw_period_slot_is_rebuilt_from_the_latched_finetune_row() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[7], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let state = processor.channel(0).expect("channel");
        assert_eq!(state.current_note, 48, "428 is C-4 on the repository's C-0 note axis");
        assert_eq!(state.current_period, 407, "the sounding period comes from finetune +7");
    }

    #[test]
    fn fine_effects_run_again_at_pattern_delay_repeat_tick_zero() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 0xE, param: 0x11 });
        assert_eq!(processor.channel(0).map(|state| state.current_period), Some(427));
        let _ = process_tick(&mut processor, 6, 1);
        assert_eq!(processor.channel(0).map(|state| state.current_period), Some(426));

        let _ = process_row(&mut processor, ModCell { effect: 0xE, param: 0xB1, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.current_volume), Some(63));
        let _ = process_tick(&mut processor, 6, 1);
        assert_eq!(processor.channel(0).map(|state| state.current_volume), Some(62));
    }

    #[test]
    fn random_waveform_is_deterministic_but_not_the_square_alias() {
        let mut first = processor_with_sample(false, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut second = processor_with_sample(false, FINETUNE_REFERENCE_RATES[0], 1024);
        for processor in [&mut first, &mut second] {
            let state = &mut processor.channels[0];
            state.current_period = 428;
            state.vibrato_waveform = 3;
            state.vibrato_memory = 0x4F;
            processor.vibrato(0);
        }
        assert_eq!(first.channels[0].actual_period, second.channels[0].actual_period);
        assert_eq!(first.waveform_random_state, second.waveform_random_state);
        assert_ne!(first.channels[0].actual_period, 428 + ((255 * 15) >> 7), "selector 3 is not square");
    }

    #[test]
    fn ef_is_an_explicit_pcm_immutable_no_op() {
        let mut processor = processor_with_sample(false, FINETUNE_REFERENCE_RATES[0], 1024);
        let before = processor.module.pcm().to_vec();
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 0xE, param: 0xFF });
        for tick in 1..6 { let _ = process_tick(&mut processor, tick, 0); }
        assert_eq!(processor.channel(0).map(|state| state.invert_loop_speed), Some(15));
        assert_eq!(processor.module.pcm(), before.as_slice());
    }

    #[test]
    fn pattern_break_is_bcd_and_out_of_range_wraps_to_zero() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xD, param: 0x31, ..ModCell::EMPTY }).jump, Some(Jump::break_to_row(31)));
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xD, param: 0x64, ..ModCell::EMPTY }).jump, Some(Jump::break_to_row(0)));
    }

    #[test]
    fn a_pattern_delay_skips_the_break_target_row() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut outcome = process_row(&mut processor, ModCell { effect: 0xD, param: 0x31, ..ModCell::EMPTY });
        outcome.pattern_delay = 1;
        processor.finish_row_outcome(4, &mut outcome);
        assert_eq!(outcome.jump, Some(Jump::break_to_row(32)));

        let mut outcome = process_row(&mut processor, ModCell { effect: 0xD, param: 0x63, ..ModCell::EMPTY });
        outcome.pattern_delay = 1;
        processor.finish_row_outcome(4, &mut outcome);
        assert_eq!(outcome.jump, Some(Jump::to_order_row(6, 0)));
    }

    #[test]
    fn sample_offset_memory_past_end_and_pt_double_add_are_native_mod_semantics() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 1 });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "PT retains a second addition after starting at 0x100");
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 0, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "a note without an instrument reuses PT's retained pointer");
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 0 });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "900 recalls the last nonzero offset after a new instrument resets the pointer");

        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 8 });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(4096));
        assert_eq!(processor.channel(0).map(|state| state.offset_past_end), Some(true));
    }

    #[test]
    fn speed_and_tempo_split_at_thirty_two_and_f00_stops() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 31, ..ModCell::EMPTY }).speed, 31);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 32, ..ModCell::EMPTY }).tempo_bpm, 32);
        assert!(process_row(&mut processor, ModCell { effect: 0xF, param: 0, ..ModCell::EMPTY }).stop);
    }

    #[test]
    fn amiga_clamp_depends_on_the_selected_finetune_row() {
        assert_eq!(clamp_period(true, 7, 1), 108);
        assert_eq!(clamp_period(true, 8, 5000), 907);
        assert_eq!(clamp_period(false, 8, 5000), 5000);
    }

    #[test]
    fn protracker_sine_has_a_255_peak_and_128_depth_divisor() {
        assert_eq!(waveform_value(0, 64), 255);
        assert_eq!(waveform_value(0, 192), -255);
        assert_eq!((255u32 * 15) >> 7, 29);
    }

    #[test]
    fn arpeggio_crosses_the_zero_sentinel_then_wraps() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let state = &mut processor.channels[0];
        state.current_note = 71;
        state.current_period = 113;
        state.command_data = 0x12;
        processor.arpeggio(0, 1);
        assert_eq!(processor.channels[0].actual_period, 0);
        processor.arpeggio(0, 2);
        assert_eq!(processor.channels[0].actual_period, 856);
        assert_eq!(step_from_period(0, 44_100), Step::ZERO);
    }

    #[test]
    fn a_plain_note_preserves_an_unfinished_portamento_target() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        processor.channels[0].target_period = 320;
        processor.channels[0].target_note = 41;
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        assert_eq!(processor.channels[0].target_period, 320);
        let _ = process_row(&mut processor, ModCell { effect: 3, param: 4, ..ModCell::EMPTY });
        let _ = process_tick(&mut processor, 1, 0);
        assert_eq!(processor.channels[0].current_period, 424);
    }

    #[test]
    fn reaching_a_portamento_target_clears_it() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let state = &mut processor.channels[0];
        state.current_period = 430;
        state.target_period = 428;
        state.target_note = 36;
        state.portamento_memory = 4;
        processor.tone_portamento(0);
        assert_eq!(processor.channels[0].current_period, 428);
        assert_eq!(processor.channels[0].target_period, 0);
    }
}

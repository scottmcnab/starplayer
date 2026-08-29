//! Scream Tracker 3's tick-zero and per-tick effect processor.

// Every channel index originates from a loop bounded by `channels.len()` and every table
// index is masked to its declared domain. Keeping that invariant at the dispatch boundary
// makes the individual handlers readable as the assembly they port.
#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec;

use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::tables::{PERIOD_TABLE, ST3_FREQUENCY_NUMERATOR, ST3_PERIOD_SCALE, waveform_sample};
use starplayer_core::{ChannelId, DirtyBits, Frame, InstrumentId, Note, Step, TempoModel, U0F16, VoiceParams};
use starplayer_engine::{EndOfSongPolicy, Jump, OrderEntry, PatternData, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TrackerProcessor};
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{EffectNames, LoopMode, Module, OrderEntry as ModelOrderEntry};
use starplayer_rt::Arc;

use crate::header::pan_nibble_to_bipolar;
use crate::pattern::{CELL_BYTES, COMMAND_NONE, INSTRUMENT_NONE, NOTE_CUT, NOTE_NONE, S3mCell, VOLUME_NONE};

const NO_SAMPLE: u8 = 255;
const DEFAULT_PERIOD: u32 = 1712;
const DEFAULT_REFERENCE_RATE_HZ: u32 = 8363;
const FINE_TUNE_TABLE: [u32; 16] = [7895, 7941, 7985, 8046, 8107, 8169, 8232, 8280, 8363, 8413, 8463, 8529, 8581, 8651, 8723, 8757];
const RETRIGGER_TABLE: [i8; 16] = [0, -1, -2, -4, -8, -16, 0, 0, 0, 1, 2, 4, 8, 16, 0, 0];

/// The original `ChannelData`, with Rust names followed by the assembly field mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3mChannel {
    pub channel_number: u8,              // `_ChannelNumber`
    pub sample_number: u8,               // `_SampleNum`
    pub pending_dirty: DirtyBits,         // `_ChannelFlag`
    pub sample_offset: u32,               // `_SampleOffset` (bytes there, frames here)
    pub current_volume: u8,               // `_CurrentVol`
    pub actual_volume: u8,                // `_ActualVol`
    pub current_note: u8,                 // `_CurrentNote`
    pub current_period: u32,              // `_CurrentPeriod`
    pub target_note: u8,                  // `_TargetNote`
    pub target_period: u32,               // `_TargetPeriod`
    pub actual_period: u32,               // `_ActualPeriod`
    pub command: u8,                      // `_CommandValue`
    pub command_data: u8,                 // `_DataValue`
    pub portamento_memory: u8,            // `_PortaValue`
    pub volume_slide_memory: u8,          // `_VolSlideValue` (shared D/E/F)
    pub vibrato_memory: u8,               // `_VibValue` (shared H/R/U)
    pub vibrato_phase: u8,                // `_VibCount` (shared H/R/U)
    pub vibrato_waveform: u8,             // `_VibTable`
    pub tremolo_waveform: u8,             // `_TremTable`
    pub retrigger_memory: u8,             // `_RetrigValue`
    pub reference_rate_hz: u32,           // `_C4SPD`
    pub sample_volume: u8,                // `_SampleVolume`
    pub pan_position: u8,                 // `_PanPosition`
    pub special_value: u8,                // `_SpecialValue`
    pub tremor_count: u8,                 // `_TremorCount`
    pub tremor_on: bool,                  // `_TremorFlag`
    pub arpeggio_count: u8,               // `_ArpCount`
    pub arpeggio_memory: u8,              // `_ArpValue`
    pub offset_memory: u8,                // `_OffsetValue`
    pub glissando_enabled: bool,           // `_GlissFlag`
}

impl S3mChannel {
    fn new(channel_number: u8, pan_position: u8) -> S3mChannel {
        S3mChannel {
            channel_number,
            sample_number: NO_SAMPLE,
            pending_dirty: DirtyBits::PAN,
            sample_offset: 0,
            current_volume: 0,
            actual_volume: 0,
            current_note: 0,
            current_period: DEFAULT_PERIOD,
            target_note: 0,
            target_period: DEFAULT_PERIOD,
            actual_period: DEFAULT_PERIOD,
            command: 0,
            command_data: 0,
            portamento_memory: 0,
            volume_slide_memory: 0,
            vibrato_memory: 0,
            vibrato_phase: 0,
            vibrato_waveform: 0,
            tremolo_waveform: 0,
            retrigger_memory: 0,
            reference_rate_hz: DEFAULT_REFERENCE_RATE_HZ,
            sample_volume: 0,
            pan_position,
            special_value: 0,
            tremor_count: 0,
            tremor_on: false,
            arpeggio_count: 0,
            arpeggio_memory: 0,
            offset_memory: 0,
            glissando_enabled: false,
        }
    }
}

/// Owned pattern access for an S3M module. A wrapper is required by Rust's orphan rule.
#[derive(Clone, Debug)]
pub struct S3mPatternData(pub Arc<Module>);

impl PatternData for S3mPatternData {
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
        let pattern_index = self.0.pattern(pattern_id)?;
        if row >= pattern_index.rows() { return None; }
        let row_length = pattern_index.channels() as usize * CELL_BYTES;
        let start = row as usize * row_length;
        self.0.pattern_bytes(pattern_id)?.get(start..start + row_length)
    }
}

/// The stateful ST3 effect processor.
pub struct S3mProcessor {
    module: Arc<Module>,
    channels: Box<[S3mChannel]>,
    sample_rate_hz: u32,
    global_volume: u8,
    amiga_limits: bool,
    pattern_loop_start: u16,
    pattern_loop_count: u8,
    last_pattern: Option<u16>,
}

impl S3mProcessor {
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> S3mProcessor {
        let header = module.header();
        let global_volume = ((header.global_volume.to_bits() as u32 * 64 + 32767) / 65535) as u8;
        let mut states = VecBuilder::new();
        for index in 0..header.channel_count {
            let pan = header.default_pan.get(index as usize).copied().map(pan_to_nibble).unwrap_or(7);
            states.push(S3mChannel::new(index, pan));
        }
        S3mProcessor {
            amiga_limits: header.flags.amiga_limits,
            module,
            channels: states.finish(),
            sample_rate_hz,
            global_volume,
            pattern_loop_start: 0,
            pattern_loop_count: 0,
            last_pattern: None,
        }
    }

    pub fn channels(&self) -> &[S3mChannel] { &self.channels }
    pub fn channel(&self, channel: u8) -> Option<&S3mChannel> { self.channels.get(channel as usize) }

    fn reset_row(&mut self) {
        for state in self.channels.iter_mut() {
            if state.current_period != state.actual_period {
                state.actual_period = state.current_period;
                state.pending_dirty.insert(DirtyBits::PITCH);
            }
            if state.command != 17 { state.special_value = 0; }
            state.command = 0;
        }
    }

    fn latch_cell(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: S3mCell) {
        let channel_id = ChannelId(channel_index as u16);
        context.report_effect(channel_id, cell.command, cell.info, EffectNames::S3M.name(cell.command, cell.info).unwrap_or(""));
        if cell.note != NOTE_NONE || cell.instrument != INSTRUMENT_NONE {
            context.report_note(channel_id, cell.linear_semitone().map(Note::new), (cell.instrument != 0).then_some(cell.instrument));
        }

        if cell.instrument != INSTRUMENT_NONE {
            let instrument_id = InstrumentId((cell.instrument - 1) as u16);
            if let Some(sample_id) = self.module.instrument(instrument_id).and_then(|instrument| instrument.sample)
                && let Some(sample) = self.module.sample(sample_id)
            {
                // PtrToSample succeeded: only now does the assembly assign _SampleNum.
                self.channels[channel_index].sample_number = cell.instrument;
                // D7: SampleIndex preserves and this reads the full 32-bit C2SPD.
                self.channels[channel_index].reference_rate_hz = sample.reference_rate_hz();
                let volume = ((sample.default_volume().to_bits() as u32 * 64 + 32767) / 65535) as u8;
                self.channels[channel_index].sample_volume = volume;
                self.channels[channel_index].current_volume = volume;
                self.channels[channel_index].actual_volume = volume;
                self.channels[channel_index].pending_dirty.insert(DirtyBits::VOLUME);
            }
        }

        match cell.note {
            NOTE_NONE => {}
            NOTE_CUT => self.cut_note(channel_index),
            note => {
                let sounding = context.channels.is_sounding(channel_id, context.voices);
                let portamento = sounding && matches!(cell.command, 7 | 12) && self.channels[channel_index].sample_number != NO_SAMPLE;
                let period = period_from_note(note, self.channels[channel_index].reference_rate_hz);
                if portamento {
                    self.channels[channel_index].target_note = note;
                    self.channels[channel_index].target_period = period;
                } else {
                    let state = &mut self.channels[channel_index];
                    state.current_note = note;
                    state.target_note = note;
                    state.current_period = period;
                    state.target_period = period;
                    state.actual_period = period;
                    state.sample_offset = 0;
                    state.pending_dirty.insert(DirtyBits::SAMPLE);
                }
                self.channels[channel_index].pending_dirty.insert(DirtyBits::PITCH);
            }
        }

        if cell.volume != VOLUME_NONE {
            let volume = cell.volume.min(64);
            let state = &mut self.channels[channel_index];
            state.current_volume = volume;
            state.actual_volume = volume;
            state.pending_dirty.insert(DirtyBits::VOLUME);
        }
    }

    fn cut_note(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.current_note = 0;
        state.target_note = 0;
        state.current_period = DEFAULT_PERIOD;
        state.target_period = DEFAULT_PERIOD;
        state.actual_period = DEFAULT_PERIOD;
        state.sample_number = NO_SAMPLE;
        state.sample_offset = 0;
        state.pending_dirty.insert(DirtyBits::SAMPLE);
    }

    fn static_effect(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: S3mCell, outcome: &mut TickOutcome) {
        match cell.command {
            // Analysis §4, S_FX_A (2776). D6: canonical ST3 ignores A00.
            1 if cell.info != 0 => { outcome.speed = cell.info; self.clear_minor(channel_index); }
            // Analysis §4, S_FX_B (2782). Preserve Cxx's row when both share a row.
            2 => {
                let break_row = outcome.jump.filter(|jump| !jump.within_pattern).and_then(|jump| jump.row);
                outcome.jump = Some(match break_row { Some(row) => Jump::to_order_row(cell.info as u16, row), None => Jump::to_order(cell.info as u16) });
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_C (2792). The packed byte is decimal, not hexadecimal.
            3 => {
                let row = (cell.info >> 4) as u16 * 10 + (cell.info & 15) as u16;
                let jump_order = outcome.jump.filter(|jump| !jump.within_pattern).and_then(|jump| jump.order);
                outcome.jump = Some(match jump_order { Some(order) => Jump::to_order_row(order, row), None => Jump::break_to_row(row) });
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_D (2805).
            4 => self.static_volume_slide(channel_index, cell.info),
            // Analysis §4, S_FX_E/F (2850/2879).
            5 | 6 => self.static_pitch_slide(channel_index, cell.command, cell.info),
            // Analysis §4, S_FX_G (2908).
            7 => {
                let value = if cell.info == 0 { self.channels[channel_index].portamento_memory } else { cell.info };
                self.channels[channel_index].portamento_memory = value;
                self.set_minor(channel_index, 7, value);
            }
            // Analysis §4, S_FX_H (2920); R and U reuse this handler.
            8 | 18 | 21 => self.static_vibrato(channel_index, cell.command, cell.info),
            // Analysis §4, S_FX_I (2941), including tick zero. D3 reads this channel's own volume.
            9 => { self.set_minor(channel_index, 9, if cell.info == 0 { self.channels[channel_index].command_data } else { cell.info }); self.minor_tremor(channel_index); }
            // Analysis §4, S_FX_J (2956), including tick zero.
            10 => { self.channels[channel_index].arpeggio_count = 1; let value = if cell.info == 0 { self.channels[channel_index].arpeggio_memory } else { cell.info }; self.channels[channel_index].arpeggio_memory = value; self.set_minor(channel_index, 10, value); self.minor_arpeggio(channel_index); }
            // Analysis §4, S_FX_K/L (2973/2987).
            11 => { self.static_volume_slide(channel_index, cell.info); self.static_vibrato(channel_index, 8, 0); self.channels[channel_index].command = 11; self.channels[channel_index].command_data = cell.info; }
            12 => {
                self.static_volume_slide(channel_index, cell.info);
                let portamento = self.channels[channel_index].portamento_memory;
                self.set_minor(channel_index, 7, portamento);
                self.channels[channel_index].command = 12;
                self.channels[channel_index].command_data = cell.info;
            }
            // Analysis §4, S_FX_O (3001), gated on a note being present.
            15 => { let value = if cell.info == 0 { self.channels[channel_index].offset_memory } else { cell.info }; self.channels[channel_index].offset_memory = value; self.channels[channel_index].sample_offset = (value as u32) << 8; if self.channels[channel_index].current_note != 0 { self.channels[channel_index].pending_dirty.insert(DirtyBits::SAMPLE); } self.clear_minor(channel_index); }
            // Analysis §4, S_FX_Q (3015).
            17 => self.static_retrigger(channel_index, cell.info),
            // Analysis §4, S_FX_S (3038).
            19 => self.static_special(channel_index, cell.info, context.position.row, outcome),
            // Analysis §4, S_FX_T (3130).
            20 => {
                outcome.tempo_bpm = core::cmp::max(cell.info as u16, 32);
                self.channels[channel_index].pending_dirty.insert(DirtyBits::TEMPO);
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_V (3140).
            22 => {
                self.global_volume = cell.info.min(64);
                context.report_global_volume(unit_from_ratio(self.global_volume as u32, 64));
                for state in self.channels.iter_mut() { state.pending_dirty.insert(DirtyBits::VOLUME); }
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_X (3154).
            24 => { let mut pan = cell.info >> 3; if pan >= 16 { pan -= 1; } self.channels[channel_index].pan_position = pan & 15; self.channels[channel_index].pending_dirty.insert(DirtyBits::PAN); self.clear_minor(channel_index); }
            COMMAND_NONE => self.clear_minor(channel_index),
            // M/N/P/W/Y/Z and malformed commands map to S_FX_0.
            _ => self.clear_minor(channel_index),
        }
    }

    fn static_volume_slide(&mut self, channel_index: usize, parameter: u8) {
        let value = if parameter == 0 { self.channels[channel_index].volume_slide_memory } else { parameter };
        if value > 0xF0 {
            self.slide_volume(channel_index, 0, value & 15);
            self.channels[channel_index].volume_slide_memory = value;
            self.clear_minor(channel_index);
        } else if value & 15 == 15 && value >> 4 != 0 {
            self.slide_volume(channel_index, value >> 4, 0);
            self.channels[channel_index].volume_slide_memory = value;
            self.clear_minor(channel_index);
        } else {
            self.channels[channel_index].volume_slide_memory = value;
            self.set_minor(channel_index, 4, value);
        }
    }

    fn static_pitch_slide(&mut self, channel_index: usize, command: u8, parameter: u8) {
        let value = if parameter == 0 { self.channels[channel_index].volume_slide_memory } else { parameter };
        self.channels[channel_index].volume_slide_memory = value;
        if value <= 0xDF {
            self.set_minor(channel_index, command, value);
        } else {
            let multiplier = if value <= 0xEF { 1 } else { 4 };
            self.slide_period(channel_index, command == 5, (value & 15) as u32 * multiplier);
            self.clear_minor(channel_index);
        }
    }

    fn static_vibrato(&mut self, channel_index: usize, command: u8, parameter: u8) {
        let old = self.channels[channel_index].vibrato_memory;
        let value = if parameter == 0 { old } else if parameter <= 15 { old & 0xF0 | parameter } else { parameter };
        self.channels[channel_index].vibrato_memory = value;
        self.set_minor(channel_index, command, value);
        if self.channels[channel_index].pending_dirty.contains(DirtyBits::SAMPLE) { self.channels[channel_index].vibrato_phase = 0; }
    }

    fn static_retrigger(&mut self, channel_index: usize, parameter: u8) {
        let value = if parameter == 0 { self.channels[channel_index].retrigger_memory } else { parameter };
        self.channels[channel_index].retrigger_memory = value;
        if self.channels[channel_index].special_value == 0 {
            self.channels[channel_index].special_value = value & 15;
        } else {
            self.minor_retrigger(channel_index);
        }
        self.set_minor(channel_index, 17, value);
    }

    fn static_special(&mut self, channel_index: usize, parameter: u8, row: u16, outcome: &mut TickOutcome) {
        let subcommand = parameter >> 4;
        let value = parameter & 15;
        match subcommand {
            // Analysis §4, S_FX_S S1x (3047): glissando control.
            1 => { self.channels[channel_index].glissando_enabled = value != 0; self.clear_minor(channel_index); }
            // Analysis §4, S_FX_S S2x (3057): monotonic ST3 finetune table.
            2 => { self.channels[channel_index].reference_rate_hz = FINE_TUNE_TABLE[value as usize]; self.clear_minor(channel_index); }
            // Analysis §4, S_FX_S S3x/S4x (3066/3080): waveform and shared phase reset.
            3 | 4 => {
                let mut selector = value;
                if selector >= 3 { selector = selector.wrapping_sub(4); self.channels[channel_index].vibrato_phase = 0; }
                if subcommand == 3 { self.channels[channel_index].vibrato_waveform = selector & 3; } else { self.channels[channel_index].tremolo_waveform = selector & 3; }
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_S S8x (3094): direct pan nibble.
            8 => { self.channels[channel_index].pan_position = value; self.channels[channel_index].pending_dirty.insert(DirtyBits::PAN); self.clear_minor(channel_index); }
            // Analysis §4, S_FX_S SBx (3101): inner pattern loop.
            11 => {
                if value == 0 {
                    self.pattern_loop_start = row;
                } else if self.pattern_loop_count < value {
                    self.pattern_loop_count = self.pattern_loop_count.saturating_add(1);
                    outcome.jump = Some(Jump::within_pattern_to_row(self.pattern_loop_start));
                } else {
                    self.pattern_loop_count = 0;
                }
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_S SCx (3126) and M_FX_S (3479): delayed note cut.
            12 => { self.channels[channel_index].special_value = value; self.set_minor(channel_index, 19, parameter); }
            // Analysis §4, S_FX_S SDx (3135) and M_FX_S (3499): saved dirty byte.
            13 => {
                self.channels[channel_index].command_data = parameter;
                self.channels[channel_index].special_value = self.channels[channel_index].pending_dirty.bits();
                self.channels[channel_index].pending_dirty = DirtyBits::empty();
                self.channels[channel_index].command = 19;
            }
            // Analysis §4, S_FX_S SEx (3146): whole row repeats without a refetch.
            14 => { outcome.pattern_delay = value; self.clear_minor(channel_index); }
            // Analysis §4, S_FX_S SFx (3152): unsupported funk repeat.
            _ => self.clear_minor(channel_index),
        }
    }

    fn minor_effect(&mut self, channel_index: usize) {
        match self.channels[channel_index].command {
            // Analysis §4, M_FX_D/E/F/G/H/I/J/K/L/Q/R/S/U (3213–3538).
            4 => self.minor_volume_slide(channel_index),
            5 => { let value = self.channels[channel_index].command_data as u32 * 4; self.slide_period(channel_index, true, value); }
            6 => { let value = self.channels[channel_index].command_data as u32 * 4; self.slide_period(channel_index, false, value); }
            7 => self.minor_portamento(channel_index),
            8 => self.minor_vibrato(channel_index, true),
            9 => self.minor_tremor(channel_index),
            10 => self.minor_arpeggio(channel_index),
            11 => { self.minor_volume_slide(channel_index); self.minor_vibrato(channel_index, true); }
            12 => { self.minor_volume_slide(channel_index); self.minor_portamento(channel_index); }
            17 => self.minor_retrigger(channel_index),
            18 => self.minor_tremolo(channel_index),
            19 => self.minor_special(channel_index),
            21 => self.minor_vibrato(channel_index, false),
            _ => {}
        }
    }

    fn minor_volume_slide(&mut self, channel_index: usize) {
        let value = self.channels[channel_index].volume_slide_memory;
        if value & 0xF0 != 0 { self.slide_volume(channel_index, value >> 4, 0); } else { self.slide_volume(channel_index, 0, value & 15); }
    }

    fn slide_volume(&mut self, channel_index: usize, up: u8, down: u8) {
        let volume = if up != 0 { self.channels[channel_index].actual_volume.saturating_add(up).min(64) } else { self.channels[channel_index].actual_volume.saturating_sub(down) };
        self.channels[channel_index].current_volume = volume;
        self.channels[channel_index].actual_volume = volume;
        self.channels[channel_index].pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn slide_period(&mut self, channel_index: usize, down: bool, amount: u32) {
        let period = if down { self.channels[channel_index].current_period.saturating_add(amount) } else { self.channels[channel_index].current_period.saturating_sub(amount).max(1) };
        self.channels[channel_index].current_period = period;
        self.channels[channel_index].actual_period = period;
        self.channels[channel_index].pending_dirty.insert(DirtyBits::PITCH);
    }

    fn minor_portamento(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        let step = state.portamento_memory as u32 * 4;
        state.current_period = if state.current_period < state.target_period {
            state.current_period.saturating_add(step).min(state.target_period)
        } else {
            state.current_period.saturating_sub(step).max(state.target_period)
        };
        state.actual_period = state.current_period;
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn minor_vibrato(&mut self, channel_index: usize, coarse: bool) {
        let state = &mut self.channels[channel_index];
        let sample = waveform_sample(state.vibrato_waveform, state.vibrato_phase) as i32;
        let scaled = if coarse { sample << 2 } else { sample };
        let delta = (scaled * (state.vibrato_memory & 15) as i32) >> 7;
        state.actual_period = add_signed(state.current_period, delta).max(1);
        state.pending_dirty.insert(DirtyBits::PITCH);
        state.vibrato_phase = state.vibrato_phase.wrapping_add(state.vibrato_memory >> 4) & 63;
    }

    fn minor_tremolo(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        let sample = waveform_sample(state.tremolo_waveform, state.vibrato_phase) as i32;
        let delta = (sample * (state.vibrato_memory & 15) as i32) >> 7;
        state.actual_volume = (state.current_volume as i32 + delta).clamp(0, 64) as u8;
        state.pending_dirty.insert(DirtyBits::VOLUME);
        // D9: canonical modulo-64 phase, not M_FX_R's one-past-the-table `jbe` defect.
        state.vibrato_phase = state.vibrato_phase.wrapping_add(state.vibrato_memory >> 4) & 63;
    }

    fn minor_tremor(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        if state.tremor_count != 0 { state.tremor_count -= 1; return; }
        if state.tremor_on {
            state.tremor_on = false;
            state.tremor_count = state.command_data & 15;
            state.actual_volume = 0;
        } else {
            state.tremor_on = true;
            state.tremor_count = state.command_data >> 4;
            // D3: read this channel's current_volume, not the undefined EDI register.
            state.actual_volume = state.current_volume;
        }
        state.pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn minor_arpeggio(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.arpeggio_count = state.arpeggio_count.saturating_sub(1);
        let semitones = if state.arpeggio_count == 0 { state.arpeggio_count = 3; 0 } else if state.arpeggio_count == 2 { state.arpeggio_memory >> 4 } else { state.arpeggio_memory & 15 };
        // D2: full octave carry and a valid-note clamp, never an out-of-table read.
        let base = (state.current_note >> 4) as u16 * 12 + (state.current_note & 15) as u16;
        let linear = (base + semitones as u16).min(15 * 12 + 11);
        let note = ((linear / 12) as u8) << 4 | (linear % 12) as u8;
        state.actual_period = period_from_note(note, state.reference_rate_hz);
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn minor_retrigger(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.special_value = state.special_value.wrapping_sub(1);
        if state.special_value != 0 || state.current_note == 0 { return; }
        state.special_value = state.retrigger_memory & 15;
        state.sample_offset = 0;
        state.pending_dirty.insert(DirtyBits::SAMPLE);
        let operation = state.retrigger_memory >> 4;
        let old = state.actual_volume as i16;
        let volume = match operation {
            6 => old * 2 / 3,
            7 => old / 2,
            14 => old * 3 / 2,
            15 => old * 2,
            _ => old + RETRIGGER_TABLE[operation as usize] as i16,
        }.clamp(0, 64) as u8;
        state.current_volume = volume;
        state.actual_volume = volume;
        state.pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn minor_special(&mut self, channel_index: usize) {
        let subcommand = self.channels[channel_index].command_data >> 4;
        if subcommand == 12 {
            self.channels[channel_index].special_value = self.channels[channel_index].special_value.wrapping_sub(1);
            if self.channels[channel_index].special_value == 0 { self.cut_note(channel_index); self.channels[channel_index].command = 0; }
        } else if subcommand == 13 {
            let countdown = (self.channels[channel_index].command_data & 15).wrapping_sub(1);
            if countdown == 0 {
                self.channels[channel_index].pending_dirty = DirtyBits::from_bits_retain(self.channels[channel_index].special_value);
                self.channels[channel_index].command = 0;
            } else {
                self.channels[channel_index].command_data = 0xD0 | countdown;
            }
        }
    }

    fn clip_pitch(&mut self, channel_index: usize) {
        if !self.channels[channel_index].pending_dirty.contains(DirtyBits::PITCH) { return; }
        let state = &mut self.channels[channel_index];
        if state.glissando_enabled {
            // Analysis §4, ClipPitch (2634–2688): convert back to the unscaled Amiga
            // domain, double until it meets Period_Table, then choose the nearer of the
            // two surrounding entries. The assembly chooses the higher pitch on a tie.
            let mut amiga_period = (state.actual_period as u64).saturating_mul(state.reference_rate_hz as u64) / ST3_PERIOD_SCALE as u64;
            let mut octave = 0u32;
            let mut selected = PERIOD_TABLE.last().copied().unwrap_or(907) as u32;
            loop {
                let mut previous = None;
                let mut found = false;
                for &table_period in &PERIOD_TABLE {
                    let table_period = table_period as u64;
                    if amiga_period >= table_period {
                        selected = match previous {
                            Some(previous_period) if previous_period - amiga_period < amiga_period - table_period => previous_period as u32,
                            _ => table_period as u32,
                        };
                        found = true;
                        break;
                    }
                    previous = Some(table_period);
                }
                if found || octave >= 31 { break; }
                amiga_period = amiga_period.saturating_mul(2);
                octave += 1;
            }
            let scaled = (selected as u64).saturating_mul(ST3_PERIOD_SCALE as u64) >> octave;
            state.actual_period = (scaled / state.reference_rate_hz.max(1) as u64).max(1).min(u32::MAX as u64) as u32;
        }
        if self.amiga_limits { state.actual_period = state.actual_period.clamp(452, 3424); }
    }

    fn flush_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let channel_id = ChannelId(channel_index as u16);
        let dirty = self.channels[channel_index].pending_dirty;
        if dirty.is_empty() { return; }

        if dirty.contains(DirtyBits::SAMPLE) {
            if self.channels[channel_index].sample_number == NO_SAMPLE {
                context.channels.stop(channel_id, context.voices);
                self.channels[channel_index].pending_dirty = DirtyBits::empty();
                return;
            }
            let instrument_number = self.channels[channel_index].sample_number;
            let instrument_id = InstrumentId((instrument_number - 1) as u16);
            if let Some(sample_id) = self.module.instrument(instrument_id).and_then(|instrument| instrument.sample)
                && let Some(sample) = self.module.sample(sample_id)
            {
                let region = sample_region(sample);
                let params = self.voice_params(channel_index, dirty);
                let tag = VoiceTag { channel: channel_index as u8, instrument: instrument_number, sample: sample_id.0 as u8, note: linear_note(self.channels[channel_index].current_note) };
                context.channels.trigger(channel_id, context.voices, tag, region, params, self.channels[channel_index].sample_offset);
            }
        } else if let Some(voice_id) = context.channels.foreground(channel_id)
            && let Some(voice) = context.voices.get_mut(voice_id)
        {
            let state = &self.channels[channel_index];
            if dirty.contains(DirtyBits::PITCH) { voice.params.set_step(step_from_period(state.actual_period, self.sample_rate_hz)); }
            if dirty.contains(DirtyBits::VOLUME) { voice.params.set_volume(scaled_volume(state.actual_volume, self.global_volume)); }
            if dirty.contains(DirtyBits::PAN) { voice.params.set_pan(pan_nibble_to_bipolar(state.pan_position)); }
            if dirty.contains(DirtyBits::TEMPO) { voice.params.dirty.insert(DirtyBits::TEMPO); }
        }
        self.channels[channel_index].pending_dirty = DirtyBits::empty();
    }

    fn voice_params(&self, channel_index: usize, dirty: DirtyBits) -> VoiceParams {
        let state = &self.channels[channel_index];
        VoiceParams {
            step: step_from_period(state.actual_period, self.sample_rate_hz),
            volume: scaled_volume(state.actual_volume, self.global_volume),
            pan: pan_nibble_to_bipolar(state.pan_position),
            dirty,
            ..VoiceParams::SILENT
        }
    }

    fn set_minor(&mut self, channel_index: usize, command: u8, data: u8) { self.channels[channel_index].command = command; self.channels[channel_index].command_data = data; }
    fn clear_minor(&mut self, channel_index: usize) { self.set_minor(channel_index, 0, 0); }
}

impl TrackerProcessor for S3mProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        let mut outcome = context.outcome();
        if self.last_pattern.is_some_and(|pattern| pattern != row.pattern) { self.pattern_loop_start = 0; }
        self.last_pattern = Some(row.pattern);
        self.reset_row();
        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        for channel_index in 0..channel_count {
            let start = channel_index * CELL_BYTES;
            let Some(cell) = S3mCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])) else { continue };
            self.latch_cell(context, channel_index, cell);
            self.static_effect(context, channel_index, cell, &mut outcome);
            self.clip_pitch(channel_index);
        }
        for channel_index in 0..self.channels.len() { self.flush_channel(context, channel_index); }
        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        let outcome = context.outcome();
        for channel_index in 0..self.channels.len() {
            self.minor_effect(channel_index);
            self.clip_pitch(channel_index);
            self.flush_channel(context, channel_index);
        }
        outcome
    }
}

/// Build the public S3M sequencer with header speed, tempo, global volume and pan state.
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, S3mProcessor, S3mPatternData> {
    let settings = SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Loop,
    };
    PatternSequencer::new(tempo_model, S3mPatternData(Arc::clone(&module)), S3mProcessor::new(module, sample_rate_hz), settings)
}

fn period_from_note(note: u8, reference_rate_hz: u32) -> u32 {
    let linear = (note >> 4) as u16 * 12 + (note & 15) as u16;
    let octave = core::cmp::min(linear / 12, 15) as u32;
    let semitone = (linear % 12) as usize;
    let scaled = (PERIOD_TABLE[semitone] as u64 * ST3_PERIOD_SCALE as u64) >> octave;
    (scaled / reference_rate_hz.max(1) as u64).max(1) as u32
}

fn step_from_period(period: u32, sample_rate_hz: u32) -> Step {
    Step::from_ratio((ST3_FREQUENCY_NUMERATOR / period.max(1)) as u64, sample_rate_hz as u64)
}

fn scaled_volume(channel_volume: u8, global_volume: u8) -> U0F16 {
    unit_from_ratio(channel_volume.min(64) as u32 * global_volume.min(64) as u32, 64 * 64)
}

fn sample_region(sample: &starplayer_model::SampleIndex) -> SampleRegion {
    match sample.loop_mode() {
        LoopMode::Forward => LoopSpan::new(sample.loop_start(), sample.loop_end()).map(|span| SampleRegion::looping(sample.pcm_offset(), span)).unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::PingPong => LoopSpan::ping_pong(sample.loop_start(), sample.loop_end()).map(|span| SampleRegion::looping(sample.pcm_offset(), span)).unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::None => SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames()),
    }
}

fn linear_note(note: u8) -> u8 { (note >> 4).saturating_mul(12).saturating_add(note & 15) }
fn add_signed(value: u32, delta: i32) -> u32 { if delta < 0 { value.saturating_sub(delta.unsigned_abs()) } else { value.saturating_add(delta as u32) } }
fn pan_to_nibble(pan: starplayer_core::I1F15) -> u8 { (((pan.to_bits() as i32 + 32767) * 15 + 32767) / 65534).clamp(0, 15) as u8 }

struct VecBuilder<T>(alloc::vec::Vec<T>);
impl<T> VecBuilder<T> {
    fn new() -> VecBuilder<T> { VecBuilder(vec![]) }
    fn push(&mut self, value: T) { self.0.push(value); }
    fn finish(self) -> Box<[T]> { self.0.into_boxed_slice() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ROWS;
    use starplayer_core::{Frame, RowClock};
    use starplayer_engine::{ChannelTable, SongPosition};
    use starplayer_mixer::VoicePool;
    use starplayer_model::{ModuleBuilder, ModuleFormat, ModuleHeader};

    fn processor() -> S3mProcessor {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![255u8; ROWS as usize * CELL_BYTES], ROWS, 1).expect("one fixed-stride pattern");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        S3mProcessor::new(Arc::new(builder.build().expect("valid module")), 44_100)
    }

    fn with_context(test: impl FnOnce(&mut S3mProcessor, &mut TickContext<'_>)) {
        let mut processor = processor();
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        test(&mut processor, &mut context);
    }

    #[test]
    fn dxy_classification_order_and_normal_high_nibble_priority_match_st3() {
        let mut processor = processor();
        processor.channels[0].current_volume = 10;
        processor.channels[0].actual_volume = 10;
        processor.static_volume_slide(0, 0xF2);
        assert_eq!(processor.channels[0].actual_volume, 8, "DFx is instant fine-down");
        processor.channels[0].current_volume = 10;
        processor.channels[0].actual_volume = 10;
        processor.static_volume_slide(0, 0x2F);
        assert_eq!(processor.channels[0].actual_volume, 12, "DxF is instant fine-up");
        processor.channels[0].current_volume = 10;
        processor.channels[0].actual_volume = 10;
        processor.static_volume_slide(0, 0xF0);
        processor.minor_volume_slide(0);
        assert_eq!(processor.channels[0].actual_volume, 25, "DF0 falls through and the high nibble wins");
        processor.channels[0].current_volume = 10;
        processor.channels[0].actual_volume = 10;
        processor.static_volume_slide(0, 0x0F);
        processor.minor_volume_slide(0);
        assert_eq!(processor.channels[0].actual_volume, 0, "D0F falls through and clamps the normal slide at zero");
        processor.channels[0].current_volume = 62;
        processor.channels[0].actual_volume = 62;
        processor.static_volume_slide(0, 0x20);
        processor.minor_volume_slide(0);
        assert_eq!(processor.channels[0].actual_volume, 64, "a normal upward slide clamps at 64");
    }

    #[test]
    fn d_e_and_f_really_share_one_parameter_memory() {
        let mut processor = processor();
        processor.channels[0].current_volume = 32;
        processor.channels[0].actual_volume = 32;
        processor.static_volume_slide(0, 0x05);
        processor.channels[0].current_period = 100;
        processor.static_pitch_slide(0, 5, 0);
        processor.minor_effect(0);
        assert_eq!(processor.channels[0].volume_slide_memory, 5);
        assert_eq!(processor.channels[0].actual_period, 120, "E00 uses the 5 remembered by D05 and multiplies it by four");
    }

    #[test]
    fn c10_is_decimal_row_ten_and_a00_is_ignored() {
        with_context(|processor, context| {
            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 3, info: 0x10, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(outcome.jump, Some(Jump::break_to_row(10)));
            processor.static_effect(context, 0, S3mCell { command: 1, info: 0, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(outcome.speed, 6, "D6: A00 does not stall the clock");
        });
    }

    #[test]
    fn wide_arpeggios_carry_fully_and_never_leave_the_period_table() {
        let mut processor = processor();
        processor.channels[0].current_note = 0x4B;
        processor.channels[0].reference_rate_hz = 8363;
        processor.channels[0].arpeggio_count = 3;
        processor.channels[0].arpeggio_memory = 0xD0;
        processor.minor_arpeggio(0);
        assert!(processor.channels[0].actual_period > 0, "D2: nibble 13 produces a valid carried note period");
    }

    #[test]
    fn tremolo_phase_wraps_at_sixty_four_instead_of_indexing_past_the_table() {
        let mut processor = processor();
        processor.channels[0].vibrato_phase = 63;
        processor.channels[0].vibrato_memory = 0x11;
        processor.minor_tremolo(0);
        assert_eq!(processor.channels[0].vibrato_phase, 0, "D9: M_FX_R's exact-64 one-past-end defect is not reproduced");
    }

    #[test]
    fn vibrato_tremolo_and_fine_vibrato_share_parameter_and_phase_state() {
        with_context(|processor, context| {
            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 8, info: 0x41, ..S3mCell::EMPTY }, &mut outcome);
            for _ in 0..4 { processor.minor_vibrato(0, true); }
            assert_eq!(processor.channels[0].vibrato_phase, 16);
            processor.static_effect(context, 0, S3mCell { command: 19, info: 0x40, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.channels[0].vibrato_phase, 16, "S40 does not reset: the assembly resets only selectors 3..7");
            processor.static_effect(context, 0, S3mCell { command: 19, info: 0x43, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.channels[0].vibrato_phase, 0, "S43 resets the phase shared with vibrato");
            processor.static_effect(context, 0, S3mCell { command: 18, info: 0x01, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.channels[0].vibrato_memory, 0x41, "R01 keeps H41's speed nibble");
            processor.minor_tremolo(0);
            assert_eq!(processor.channels[0].vibrato_phase, 4, "tremolo continues from the reset shared phase");
            processor.static_effect(context, 0, S3mCell { command: 21, info: 0, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.channels[0].vibrato_memory, 0x41, "U00 recalls the same shared parameter");
        });
    }

    #[test]
    fn retrigger_counter_survives_rows_and_all_multiplicative_volume_cases_match_the_table() {
        let mut processor = processor();
        processor.channels[0].command = 17;
        processor.channels[0].special_value = 2;
        processor.reset_row();
        assert_eq!(processor.channels[0].special_value, 2, "Q phase survives the row-start reset");
        processor.channels[0].command = 8;
        processor.reset_row();
        assert_eq!(processor.channels[0].special_value, 0, "non-Q rows clear the shared special counter");

        for (operation, expected) in [(6, 40), (7, 30), (14, 64), (15, 64)] {
            processor.channels[0].current_note = 0x40;
            processor.channels[0].current_volume = 60;
            processor.channels[0].actual_volume = 60;
            processor.channels[0].retrigger_memory = operation << 4 | 1;
            processor.channels[0].special_value = 1;
            processor.minor_retrigger(0);
            assert_eq!(processor.channels[0].actual_volume, expected, "Q operation {operation:X} has its ST3 multiplicative result");
        }
    }

    #[test]
    fn canonical_pulse_wave_has_valid_values_at_phases_sixty_two_and_sixty_three() {
        assert_eq!(waveform_sample(2, 62), 255, "D1 fills the original table's first missing entry");
        assert_eq!(waveform_sample(2, 63), 255, "D1 fills the original table's second missing entry");
    }

    #[test]
    fn every_special_subcommand_updates_the_state_named_by_the_effect_table() {
        with_context(|processor, context| {
            let mut outcome = context.outcome();
            processor.static_special(0, 0x11, 7, &mut outcome);
            assert!(processor.channels[0].glissando_enabled);
            processor.static_special(0, 0x2F, 7, &mut outcome);
            assert_eq!(processor.channels[0].reference_rate_hz, 8757);
            processor.static_special(0, 0x37, 7, &mut outcome);
            assert_eq!(processor.channels[0].vibrato_waveform, 3, "S37 selects random after the subtract-four mapping");
            processor.static_special(0, 0x47, 7, &mut outcome);
            assert_eq!(processor.channels[0].tremolo_waveform, 3);
            processor.static_special(0, 0x8E, 7, &mut outcome);
            assert_eq!(processor.channels[0].pan_position, 14);
            processor.static_special(0, 0xB0, 7, &mut outcome);
            assert_eq!(processor.pattern_loop_start, 7);
            processor.static_special(0, 0xC2, 7, &mut outcome);
            assert_eq!(processor.channels[0].special_value, 2);
            processor.channels[0].pending_dirty = DirtyBits::VOLUME | DirtyBits::PAN;
            processor.static_special(0, 0xD3, 7, &mut outcome);
            assert_eq!(processor.channels[0].special_value, (DirtyBits::VOLUME | DirtyBits::PAN).bits(), "SDx saves the complete dirty byte");
            assert!(processor.channels[0].pending_dirty.is_empty());
            processor.static_special(0, 0xE2, 7, &mut outcome);
            assert_eq!(outcome.pattern_delay, 2);
            processor.static_special(0, 0xFF, 7, &mut outcome);
            assert_eq!(processor.channels[0].command, 0, "SFx is ignored and leaves no minor effect");
        });
    }

    #[test]
    fn pattern_loop_and_external_jumps_obey_channel_order_without_merging_destinations() {
        with_context(|processor, context| {
            processor.pattern_loop_start = 4;
            let mut outcome = context.outcome();
            processor.static_special(0, 0xB1, 7, &mut outcome);
            assert_eq!(outcome.jump, Some(Jump::within_pattern_to_row(4)));
            processor.static_effect(context, 0, S3mCell { command: 2, info: 3, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(outcome.jump, Some(Jump::to_order(3)), "a later Bxx replaces SBx instead of inheriting its row");

            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 3, info: 0x12, ..S3mCell::EMPTY }, &mut outcome);
            processor.pattern_loop_count = 0;
            processor.static_special(0, 0xB1, 7, &mut outcome);
            assert_eq!(outcome.jump, Some(Jump::within_pattern_to_row(4)), "a later SBx replaces Cxx with its in-pattern destination");
        });
    }
}

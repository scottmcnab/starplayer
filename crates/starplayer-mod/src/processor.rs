//! ProTracker's native tick-zero and per-tick effect processor.

#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::quirks::{BreakParameter, ModTiming, PaulaClock, QuirkSelection, QuirkSet};
use starplayer_core::{ChannelId, DirtyBits, Frame, I1F15, InstrumentId, Note, Step, TempoModel, TempoModelId, U0F16, VoiceParams, Xorshift32};
use starplayer_engine::{EndOfSongPolicy, OrderEntry, PatternData, PatternFlowState, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TrackerProcessor};
#[cfg(feature = "trace")]
use starplayer_engine::TraceChannelState;
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{EffectNames, LoopMode, Module, OrderEntry as ModelOrderEntry};
use starplayer_rt::Arc;

use crate::pattern::{CELL_BYTES, ModCell};
use crate::tables::{FINETUNE_REFERENCE_RATES, PROTRACKER_PERIODS, extended_period};

const NO_SAMPLE: u8 = 0;
const NO_NOTE: u8 = u8::MAX;
const DEFAULT_PERIOD: u32 = 428;
const PAULA_MINIMUM_PERIOD: u32 = 113;
/// Fixed seed for waveform selector 3; see accuracy policy D11.
const WAVEFORM_RANDOM_SEED: u32 = 0x6D2B_79F5;
const VIBRATO_TABLE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253,
    255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24,
];

/// A note entering the shared MOD-effect vocabulary.
///
/// MOD supplies a packed period because ProTracker deliberately searches that value in
/// its selected finetune table. Formats such as MTM already store a linear note and must
/// not manufacture a MOD period merely to use the common effect machinery.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EffectNote {
    #[default]
    None,
    ModPeriod(u16),
    Linear(u8),
}

/// Decoded input to the ProTracker-compatible effect core.
///
/// This is an execution vocabulary, not a serialized pattern representation. MOD and
/// MTM keep their own native cells and decode them independently before entering here.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EffectCell {
    pub note: EffectNote,
    pub instrument: u8,
    pub effect: u8,
    pub param: u8,
}

impl From<ModCell> for EffectCell {
    fn from(cell: ModCell) -> EffectCell {
        EffectCell {
            note: if cell.period == 0 { EffectNote::None } else { EffectNote::ModPeriod(cell.period) },
            instrument: cell.instrument,
            effect: cell.effect,
            param: cell.param,
        }
    }
}

/// Format-level differences around the otherwise shared ProTracker command set.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EffectSemantics {
    #[default]
    ProTracker,
    /// MultiTracker's Dxx parameter is hexadecimal and its Fxx command is applied
    /// immediately. `reset_counterpart` selects native MultiTracker timing, where a
    /// speed change resets BPM to 125 and a BPM change resets speed to 6; false selects
    /// the widespread Dual Module Player interpretation detected by the MTM loader.
    MultiTracker { reset_counterpart: bool },
}

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
    /// Whether this pattern row contains a packed period. Unlike `current_note`, this
    /// is row-local and remains valid across EEx repeats without leaking into a new row.
    pub row_note_present: bool,
    /// This row's note is controlled by EDx and may trigger once per delayed repeat.
    pub delayed_note: bool,
    pub offset_past_end: bool,
    /// Last EFx value. EF is parsed but deliberately has no PCM mutation; see
    /// `plans/reference/format-notes-mod.md`.
    pub invert_loop_speed: u8,
    /// This row named an instrument that must not restart the voice: ProTracker's queued
    /// sample swap (accuracy policy D12). `Some(NO_SAMPLE)` is PT's null sample, which
    /// stops the voice at the same boundary instead of replacing it.
    queued_swap: Option<u8>,
    /// Whether the channel still owns a voice: one was started and nothing has cut it
    /// since, even if its sample has since run out. This is libxmp's mapped-versus-
    /// unmapped distinction, and it decides where a revived queued sample starts.
    voice_owned: bool,
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
            row_note_present: false,
            delayed_note: false,
            offset_past_end: false,
            invert_loop_speed: 0,
            queued_swap: None,
            voice_owned: false,
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
    waveform_random_state: Xorshift32,
    pending_tempo: Option<(u16, usize)>,
    semantics: EffectSemantics,
    /// The replay behaviour this module was loaded with. Resolved once, at construction,
    /// and never written again — see [`starplayer_core::quirks`].
    quirks: QuirkSet,
    /// `E60`/`E6x`, `Bxx` and `Dxx` bookkeeping under the module's
    /// [`ModLoopDialect`](starplayer_core::quirks::ModLoopDialect).
    flow: PatternFlowState,
}

impl ModProcessor {
    /// A ProTracker processor whose quirks come from the dialect the loader detected.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> ModProcessor {
        ModProcessor::with_semantics(module, sample_rate_hz, EffectSemantics::ProTracker)
    }

    /// Construct the shared effect core for another format that uses the MOD command
    /// vocabulary. The caller remains responsible for a native pattern decoder and a
    /// native [`TrackerProcessor`] wrapper.
    pub fn with_semantics(module: Arc<Module>, sample_rate_hz: u32, semantics: EffectSemantics) -> ModProcessor {
        // Naming the command semantics names the dialect too: MultiTracker is a dialect
        // of the MOD command vocabulary, so its quirks come with it rather than depending
        // on a header field the caller may not have filled in.
        let quirks = match semantics {
            EffectSemantics::ProTracker => QuirkSelection::FromDialect,
            EffectSemantics::MultiTracker { .. } => QuirkSelection::Override(QuirkSet::multitracker()),
        };
        ModProcessor::with_semantics_and_quirks(module, sample_rate_hz, semantics, quirks)
    }

    /// The full constructor: native command semantics plus an explicit [`QuirkSelection`].
    pub fn with_semantics_and_quirks(module: Arc<Module>, sample_rate_hz: u32, semantics: EffectSemantics, quirks: QuirkSelection) -> ModProcessor {
        let quirks = quirks.resolve(module.header().dialect);
        let channels = (0..module.header().channel_count).map(|channel| {
            let pan = module.header().channel_pan(channel).unwrap_or(I1F15::ZERO);
            ModChannel::new(channel, pan)
        }).collect::<Vec<_>>().into_boxed_slice();
        ModProcessor {
            amiga_limits: module.header().flags.amiga_limits,
            flow: PatternFlowState::new(quirks.mod_pattern_loop.flow(), channels.len()),
            module,
            channels,
            sample_rate_hz,
            last_pattern: None,
            row_pattern_break: false,
            // Unlike libxmp's wall-clock seed, a fixed seed preserves StarPlayer's
            // byte-identical replay invariant while still implementing waveform 3.
            waveform_random_state: Xorshift32::new(WAVEFORM_RANDOM_SEED),
            pending_tempo: None,
            semantics,
            quirks,
        }
    }

    /// The replay behaviour in force. Fixed for the lifetime of the loaded module.
    pub const fn quirks(&self) -> QuirkSet { self.quirks }

    /// The pattern-loop and break/jump bookkeeping, for inspection by a host or a test.
    pub const fn pattern_flow(&self) -> &PatternFlowState { &self.flow }

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
            state.row_note_present = false;
            state.delayed_note = false;
            state.offset_post_trigger = 0;
            state.queued_swap = None;
        }
    }

    fn latch_cell(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: EffectCell) -> bool {
        let channel_id = ChannelId(channel_index as u16);
        let effect_name = if cell.effect == 0 && cell.param == 0 { "" } else { EffectNames::MOD.name(cell.effect, cell.param).unwrap_or("") };
        context.report_effect(channel_id, cell.effect, cell.param, effect_name);
        // Native MOD cells can encode only 1..=31, while MTM's six-bit field reaches
        // 63. The decoded effect boundary validates against the loaded instrument table
        // instead of imposing MOD's serialized field width on every caller.
        // D12: the queued boundary swap and ProTracker's null-sample reading of an empty
        // instrument slot are one behaviour, selected by one field.
        let protracker = self.quirks.protracker_sample_swap_at_boundary;
        if cell.instrument != 0 {
            let instrument_id = InstrumentId((cell.instrument - 1) as u16);
            let resolved = self.module.instrument(instrument_id).and_then(|instrument| instrument.sample)
                .and_then(|sample_id| self.module.sample(sample_id));
            // ProTracker reads an instrument number whose sample holds no data as its
            // null sample: volume, finetune and the reported number all stay, and the
            // sounding voice stops when it next reaches its loop point.
            match resolved.filter(|sample| !protracker || sample.length_frames() > 0) {
                Some(sample) => {
                    let volume = ((sample.default_volume().to_bits() as u32 * 64 + 32_767) / 65_535) as u8;
                    let finetune = finetune_from_rate(sample.reference_rate_hz());
                    let state = &mut self.channels[channel_index];
                    state.sample_number = cell.instrument;
                    state.finetune = finetune;
                    state.sample_offset = 0;
                    state.offset_past_end = false;
                    state.current_volume = volume;
                    state.actual_volume = volume;
                    state.pending_dirty.insert(DirtyBits::VOLUME);
                    if protracker { state.queued_swap = Some(cell.instrument); }
                }
                None if protracker => self.channels[channel_index].queued_swap = Some(NO_SAMPLE),
                None => {}
            }
        }

        // ProTracker setPeriod scans the zero-finetune row to identify the packed
        // period's note slot, then reads that slot from the newly latched instrument's
        // finetuned row. Keep those two operations visibly ordered: a raw 428 with
        // finetune +7 is still C, but plays the +7 table's period 407.
        let note = match cell.note {
            EffectNote::None => None,
            EffectNote::ModPeriod(period) => crate::tables::note_from_period(period),
            EffectNote::Linear(note) => Some(note),
        };
        self.channels[channel_index].row_note_present = note.is_some();
        if note.is_some() || cell.instrument != 0 { context.report_note(channel_id, note.map(Note::new), (cell.instrument != 0).then_some(cell.instrument)); }

        // E5 changes the finetune used to look up a note on the same row.
        if cell.effect == 0xE && cell.param >> 4 == 5 { self.channels[channel_index].finetune = cell.param & 15; }

        let Some(note) = note else { return false };
        if matches!(cell.effect, 3 | 5) {
            let state = &mut self.channels[channel_index];
            let (target_note, target_period) = match cell.note {
                EffectNote::ModPeriod(period) => tone_portamento_target(state.finetune, period),
                EffectNote::Linear(note) => (note, extended_period(state.finetune, note)),
                EffectNote::None => (NO_NOTE, 0),
            };
            state.target_note = target_note;
            state.target_period = target_period;
            return true;
        }

        let period = extended_period(self.channels[channel_index].finetune, note);
        let delayed = cell.effect == 0xE && cell.param >> 4 == 0xD;
        let state = &mut self.channels[channel_index];
        // A note that is not a tone portamento starts the new sample outright, so there
        // is nothing left to queue.
        state.queued_swap = None;
        state.current_note = note;
        state.current_period = period;
        state.actual_period = period;
        state.delayed_note = delayed;
        if !delayed { state.pending_dirty.insert(DirtyBits::SAMPLE | DirtyBits::PITCH); }
        if !delayed {
            if state.vibrato_waveform & 4 == 0 { state.vibrato_phase = 0; }
            if state.tremolo_waveform & 4 == 0 { state.tremolo_phase = 0; }
        }
        true
    }

    fn static_effect(&mut self, channel_index: usize, cell: EffectCell, note_present: bool, row: u16, outcome: &mut TickOutcome) {
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
                // ProTracker's later-channel Bxx resets a Dxx row already seen; the
                // dialect's `PatternFlow` decides whether it does and whether a loop jump
                // on the row blocks it.
                if self.flow.pattern_jump(cell.param as u16) { self.row_pattern_break = false; }
                self.clear_command(channel_index);
            }
            0xC => {
                self.set_volume(channel_index, cell.param.min(64));
                self.clear_command(channel_index);
            }
            0xD => {
                let decoded = match self.quirks.mod_break_parameter {
                    // The packed byte is BCD in ProTracker; invalid results wrap to row zero.
                    BreakParameter::BinaryCodedDecimal => (cell.param >> 4) as u16 * 10 + (cell.param & 15) as u16,
                    // MultiTracker stores the row as an ordinary hexadecimal byte.
                    BreakParameter::Hexadecimal => cell.param as u16,
                };
                let break_row = if decoded > 63 { 0 } else { decoded };
                if self.flow.pattern_break(break_row) { self.row_pattern_break = true; }
                self.clear_command(channel_index);
            }
            0xE => self.static_extended(channel_index, cell.param, row, outcome),
            // MultiTracker has no "F00 stops the song" rule; libxmp's fx_s3m_speed
            // ignores a zero parameter outright.
            0xF if cell.param == 0 && self.quirks.mod_f00_stops_song => {
                outcome.stop = true;
                self.clear_command(channel_index);
            }
            0xF if cell.param == 0 => self.clear_command(channel_index),
            // A VBlank-timed MOD has no CIA timer to latch a BPM into, so every non-zero
            // `Fxx` is ticks per row whatever its value — libxmp's `QUIRK_NOBPM`
            // (`src/effects.c:463-472`). The tempo and the deferred latch are both left
            // alone. `EffectSemantics::MultiTracker` never reaches here with VBlank set:
            // MTM is always CIA and its quirks say so.
            0xF if cell.param < 32 || self.quirks.mod_timing == ModTiming::VBlank => {
                outcome.speed = cell.param;
                if matches!(self.semantics, EffectSemantics::MultiTracker { reset_counterpart: true }) {
                    outcome.tempo_bpm = 125;
                }
                self.clear_command(channel_index);
            }
            0xF => {
                match self.semantics {
                    EffectSemantics::ProTracker => {
                        // PT writes the CIA latch now, but the timer adopts it at the next
                        // interrupt. Defer the outcome until that next tracker event.
                        self.pending_tempo = Some((cell.param as u16, channel_index));
                    }
                    EffectSemantics::MultiTracker { reset_counterpart } => {
                        outcome.tempo_bpm = cell.param as u16;
                        if reset_counterpart { outcome.speed = 6; }
                    }
                }
                self.clear_command(channel_index);
            }
            _ => self.clear_command(channel_index),
        }
    }

    fn static_extended(&mut self, channel_index: usize, parameter: u8, row: u16, outcome: &mut TickOutcome) {
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
                self.flow.pattern_loop(channel_index, row, value);
                self.clear_command(channel_index);
            }
            7 => { self.channels[channel_index].tremolo_waveform = value & 7; self.clear_command(channel_index); }
            8 => {
                // MultiTracker's own pan domain is the header's 0..15 nibble, and the
                // MTM loader maps it that way; ProTracker's shared E8x is libxmp's
                // `fxp << 4` on the 0..255 domain.
                self.channels[channel_index].pan = match self.semantics {
                    EffectSemantics::ProTracker => pan_byte(value << 4),
                    EffectSemantics::MultiTracker { .. } => multitracker_pan_nibble(value),
                };
                self.channels[channel_index].pending_dirty.insert(DirtyBits::PAN);
                self.clear_command(channel_index);
            }
            9 if value == 0 => self.clear_command(channel_index),
            9 => {
                // E9x without a note retriggers on tick zero; a note on this row already
                // performed that restart and must not be triggered twice.
                if !self.channels[channel_index].row_note_present { self.retrigger(channel_index); }
            }
            0xA => self.slide_volume(channel_index, value, 0),
            0xB => self.slide_volume(channel_index, 0, value),
            0xC if value == 0 => { self.cut_note(channel_index); self.clear_command(channel_index); }
            0xC => {}
            // Keep ED0 latched so pattern-delay repeat tick zero runs noteDelay again.
            0xD if value == 0 => self.trigger_delayed(channel_index),
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
            9 if value != 0 && local_tick.is_multiple_of(value)
                && (local_tick != 0 || !self.channels[channel_index].row_note_present) => self.retrigger(channel_index),
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
        let offset = (self.channels[channel_index].offset_memory as u32) << 8;
        match self.semantics {
            EffectSemantics::ProTracker => {
                // PT 1/2 starts this note at the requested offset, then advances the
                // retained sample pointer by the same amount once more.
                if self.advance_sample_pointer(channel_index, offset) && note_present {
                    self.channels[channel_index].offset_post_trigger = offset;
                }
            }
            EffectSemantics::MultiTracker { .. } if note_present => {
                // MTM's offset is an absolute start position, including when it lies
                // beyond the addressable sample. Keep the full sample region and raw
                // position: the mixer then ends an out-of-range one-shot or wraps a
                // looping sample through its bounded position normalisation. PT's
                // retained-pointer/one-word fallback is deliberately not used here.
                let state = &mut self.channels[channel_index];
                state.sample_offset = offset;
                state.offset_past_end = false;
            }
            EffectSemantics::MultiTracker { .. } => {}
        }
    }

    fn advance_sample_pointer(&mut self, channel_index: usize, amount: u32) -> bool {
        let Some(sample_length) = self.sample_index(channel_index).map(|sample| sample.length_frames()) else { return false };
        let state = &mut self.channels[channel_index];
        let remaining = if state.offset_past_end { 1 } else { sample_length.saturating_sub(state.sample_offset) };
        if amount < remaining {
            state.sample_offset = state.sample_offset.saturating_add(amount);
            true
        } else {
            // sampleOffset sets n_length to one word but deliberately leaves n_start at
            // the last successfully advanced pointer.
            state.offset_past_end = true;
            false
        }
    }

    fn finish_row_effects(&mut self, channel_index: usize) {
        let added = self.channels[channel_index].offset_post_trigger;
        if added == 0 { return; }
        self.channels[channel_index].offset_post_trigger = 0;
        let _ = self.advance_sample_pointer(channel_index, added);
    }

    fn finish_row_outcome(&self, current_order: u16, outcome: &mut TickOutcome) {
        if !matches!(self.semantics, EffectSemantics::ProTracker) { return; }
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
        let (current_note, current_period, finetune, parameter) = {
            let state = &self.channels[channel_index];
            (state.current_note, state.current_period, state.finetune, state.command_data)
        };
        if current_note == NO_NOTE { return; }
        let phase = tick % 3;
        let semitones = match phase { 1 => parameter >> 4, 2 => parameter & 15, _ => 0 };
        let actual_period = if phase == 0 {
            current_period
        } else if self.amiga_limits {
            let base = period_table_slot(finetune, current_period);
            protracker_arpeggio_period(finetune, base + semitones as usize)
        } else {
            let base_note = (0..=95u8).find(|note| current_period >= extended_period(finetune, *note)).unwrap_or(95);
            extended_period(finetune, base_note.saturating_add(semitones).min(95))
        };
        let state = &mut self.channels[channel_index];
        state.actual_period = actual_period;
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
        let delta = lfo_delta(waveform, depth, 7);
        let state = &mut self.channels[channel_index];
        // mt_Vibrato3 writes n_period +/- delta straight to Paula. The Amiga table range
        // belongs to the slides and to tone portamento, not to the LFO output.
        state.actual_period = add_signed(state.current_period, delta);
        state.vibrato_phase = state.vibrato_phase.wrapping_add((state.vibrato_memory >> 4) << 2);
        state.pending_dirty.insert(DirtyBits::PITCH);
    }

    fn tremolo(&mut self, channel_index: usize) {
        let (selector, phase, depth, ramp_half_phase) = {
            let state = &self.channels[channel_index];
            // D20: PT's `mt_Tremolo2` picks which half of the ramp to read by testing
            // `n_vibratopos` rather than `n_tremolopos`. Off under `canonical()`, where
            // the ramp reads its own phase, which is also what libxmp and OpenMPT do.
            let ramp_half_phase = match self.quirks.protracker_tremolo_ramp_from_vibrato_phase {
                true => state.vibrato_phase,
                false => state.tremolo_phase,
            };
            (state.tremolo_waveform, state.tremolo_phase, state.tremolo_memory & 15, ramp_half_phase)
        };
        let waveform = self.tremolo_waveform_value(selector, phase, ramp_half_phase);
        let delta = lfo_delta(waveform, depth, 6);
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
        self.channels[channel_index].pending_dirty.insert(DirtyBits::SAMPLE);
    }

    fn trigger_delayed(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        if !state.delayed_note || !state.row_note_present { return; }
        if state.sample_number != NO_SAMPLE { state.pending_dirty.insert(DirtyBits::SAMPLE | DirtyBits::PITCH); }
    }

    fn apply_pending_tempo(&mut self, outcome: &mut TickOutcome) {
        let Some((tempo_bpm, channel_index)) = self.pending_tempo.take() else { return };
        outcome.tempo_bpm = tempo_bpm;
        if let Some(channel) = self.channels.get_mut(channel_index) { channel.pending_dirty.insert(DirtyBits::TEMPO); }
    }

    fn cut_note(&mut self, channel_index: usize) { self.set_volume(channel_index, 0); }

    fn waveform_value(&mut self, selector: u8, phase: u8) -> i16 {
        if selector & 3 == 3 {
            let random = self.waveform_random_state.next_u32();
            return ((random >> 23) as i16 & 511) - 256;
        }
        waveform_value(selector, phase)
    }

    /// [`Self::waveform_value`], but with the ramp's half chosen by a possibly different
    /// phase — the one thing PT's tremolo does differently (accuracy policy D20).
    fn tremolo_waveform_value(&mut self, selector: u8, phase: u8, ramp_half_phase: u8) -> i16 {
        if selector & 3 != 1 || phase == ramp_half_phase { return self.waveform_value(selector, phase); }
        ramp_waveform_value(phase, ramp_half_phase)
    }

    fn sample_index(&self, channel_index: usize) -> Option<&starplayer_model::SampleIndex> {
        let number = self.channels[channel_index].sample_number;
        if number == NO_SAMPLE { return None; }
        self.module.instrument(InstrumentId((number - 1) as u16)).and_then(|instrument| instrument.sample).and_then(|sample| self.module.sample(sample))
    }

    /// ProTracker's queued sample swap: an instrument number without a note, or with a
    /// note and a tone portamento, replaces the sounding sample only when that sample
    /// reaches its loop point or its end (accuracy policy D12).
    fn apply_queued_swap(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let Some(queued) = self.channels[channel_index].queued_swap.take() else { return };
        // PT needs a note to have sounded on this channel before a lone instrument number
        // means anything at all.
        if self.channels[channel_index].current_note == NO_NOTE { return; }
        let channel_id = ChannelId(channel_index as u16);
        if queued == NO_SAMPLE {
            context.queue_channel_region(channel_id, SampleRegion::default());
            return;
        }
        let region = {
            let Some(sample) = self.sample_index(channel_index) else { return };
            sample_region(sample)
        };
        if context.queue_channel_region(channel_id, region) { return; }
        // A voice that merely ran out still owns its channel; one that was cut left it
        // unbound. PT tells the two apart the same way libxmp's `virt_queuepatch` does,
        // and it decides where the revived sample starts: Paula reloads a still-owned
        // channel from its loop registers, while an unbound one is an ordinary fresh
        // note from the sample's first frame.
        let still_owned = self.channels[channel_index].voice_owned;
        let state = &mut self.channels[channel_index];
        state.sample_offset = if still_owned { region.loop_span().map(|span| span.start()).unwrap_or(0) } else { 0 };
        state.offset_past_end = false;
        state.pending_dirty.insert(DirtyBits::SAMPLE | DirtyBits::PITCH);
    }

    fn flush_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        self.apply_queued_swap(context, channel_index);
        let dirty = self.channels[channel_index].pending_dirty;
        if dirty.is_empty() { return; }
        let channel_id = ChannelId(channel_index as u16);
        if dirty.contains(DirtyBits::SAMPLE) {
            let Some(sample) = self.sample_index(channel_index) else {
                context.stop_channel(channel_id);
                self.channels[channel_index].voice_owned = false;
                self.channels[channel_index].pending_dirty = DirtyBits::empty();
                return;
            };
            let state = &self.channels[channel_index];
            let region = if state.offset_past_end {
                // Keep PT's retained n_start while restricting playback after it to one
                // word. The region ends two frames after the logical retained offset.
                let end = state.sample_offset.saturating_add(2).min(sample.length_frames());
                SampleRegion::one_shot(sample.pcm_offset(), end)
            } else {
                sample_region(sample)
            };
            let tag = VoiceTag {
                channel: channel_index as u8,
                instrument: state.sample_number,
                sample: state.sample_number,
                note: if state.current_note == NO_NOTE { 0 } else { state.current_note },
            };
            let offset_frames = state.sample_offset;
            context.trigger_channel(channel_id, tag, region, self.voice_params(channel_index, dirty), offset_frames);
            self.channels[channel_index].voice_owned = true;
        } else if let Some(voice) = context.channels.foreground(channel_id) {
            let state = &self.channels[channel_index];
            if dirty.contains(DirtyBits::PITCH) { context.write_voice_param(voice, starplayer_core::VoiceParam::Step(step_from_period(state.actual_period, self.sample_rate_hz, self.amiga_limits, self.quirks.mod_paula_clock))); }
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
            step: step_from_period(state.actual_period, self.sample_rate_hz, self.amiga_limits, self.quirks.mod_paula_clock),
            volume: unit_from_ratio(state.actual_volume as u32, 64),
            pan: state.pan,
            dirty,
            ..VoiceParams::SILENT
        }
    }

    /// Feed the diagnostic per-tick trace. Gated for the same reason as the S3M
    /// processor's: the loop runs once per channel per tracker tick inside `render()`.
    #[cfg(feature = "trace")]
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

    /// Execute one row supplied as decoded effect cells.
    ///
    /// MTM uses this boundary after decoding its native three-byte cells; no MOD bytes
    /// or MOD period fields are created by that path.
    pub fn row_effects<I>(&mut self, context: &mut TickContext<'_>, order: u16, pattern: u16, row: u16, cells: I) -> TickOutcome
    where
        I: IntoIterator<Item = EffectCell>,
    {
        context.report_global_volume(U0F16::MAX);
        // PT's n_pattpos is per channel and survives a pattern change; only E60 writes
        // it, so a pattern whose first E6x has no preceding E60 loops back to the
        // previous pattern's mark.
        self.last_pattern = Some(pattern);
        self.row_pattern_break = false;
        self.flow.begin_row();
        self.reset_row();
        let mut outcome = context.outcome();
        self.apply_pending_tempo(&mut outcome);
        for (channel_index, cell) in cells.into_iter().take(self.channels.len()).enumerate() {
            let note_present = self.latch_cell(context, channel_index, cell);
            self.static_effect(channel_index, cell, note_present, row, &mut outcome);
            let state = &mut self.channels[channel_index];
            state.actual_period = clamp_period(self.amiga_limits, state.finetune, state.actual_period);
        }
        outcome.jump = self.flow.jump();
        self.finish_row_outcome(order, &mut outcome);
        for channel_index in 0..self.channels.len() {
            self.flush_channel(context, channel_index);
        }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }
}

impl TrackerProcessor for ModProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        let cells = row.bytes.chunks_exact(CELL_BYTES)
            .map(|bytes| EffectCell::from(ModCell::from_bytes(bytes).unwrap_or(ModCell::EMPTY)));
        self.row_effects(context, row.order, row.pattern, row.row, cells)
    }

    /// Drop every piece of state a seek must not carry across: the deferred CIA tempo,
    /// the LFO random stream, every effect memory and every pattern-loop counter.
    fn reset(&mut self) {
        for channel_index in 0..self.channels.len() {
            let pan = self.module.header().channel_pan(channel_index as u8).unwrap_or(I1F15::ZERO);
            self.channels[channel_index] = ModChannel::new(channel_index as u8, pan);
        }
        self.last_pattern = None;
        self.row_pattern_break = false;
        self.flow.reset();
        self.waveform_random_state = Xorshift32::new(WAVEFORM_RANDOM_SEED);
        self.pending_tempo = None;
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        context.report_global_volume(U0F16::MAX);
        let mut outcome = context.outcome();
        self.apply_pending_tempo(&mut outcome);
        let local_tick = if context.row_clock.speed == 0 { 0 } else { (context.row_clock.tick_in_row % context.row_clock.speed as u16) as u8 };
        let repeat_zero = context.row_clock.is_first_tick_of_repeat();
        for channel_index in 0..self.channels.len() {
            self.minor_effect(channel_index, local_tick, repeat_zero);
            self.flush_channel(context, channel_index);
        }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }
}

/// Build a public MOD sequencer using ProTracker timing and native period semantics.
///
/// The quirks come from the [`FormatDialect`](starplayer_core::quirks::FormatDialect) the
/// loader stored in the module header; the tempo model is the caller's.
/// [`sequencer_with_quirks`] is the form that takes both from one [`QuirkSelection`].
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, ModProcessor, ModPatternData> {
    let settings = sequencer_settings(&module, sample_rate_hz);
    PatternSequencer::new(tempo_model, ModPatternData(Arc::clone(&module)), ModProcessor::new(module, sample_rate_hz), settings)
}

/// Build a public MOD sequencer under an explicit [`QuirkSelection`].
///
/// The selection is resolved once, against the loader-detected dialect, and supplies both
/// the effect processor's quirks and the tempo model.
pub fn sequencer_with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> PatternSequencer<TempoModelId, ModProcessor, ModPatternData> {
    let resolved = quirks.resolve(module.header().dialect);
    let settings = sequencer_settings(&module, sample_rate_hz);
    let processor = ModProcessor::with_semantics_and_quirks(Arc::clone(&module), sample_rate_hz, EffectSemantics::ProTracker, QuirkSelection::Override(resolved));
    PatternSequencer::new(resolved.tempo_model, ModPatternData(module), processor, settings)
}

fn sequencer_settings(module: &Module, sample_rate_hz: u32) -> SequencerSettings {
    SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Loop,
    }
}

const PROTRACKER_ARPEGGIO_OVERFLOW: [u16; 15] = [
    774, 1800, 2314, 3087, 4113, 4627, 5400, 6426, 6940, 7713, 8739, 9253, 24625, 12851, 13365,
];

fn period_table_slot(finetune: u8, period: u32) -> usize {
    PROTRACKER_PERIODS[(finetune & 15) as usize].iter()
        .position(|candidate| period >= *candidate as u32).unwrap_or(36)
}

/// Read PT's physically flat 16 x 37 table, including its zero sentinels and the
/// documented 15-word overflow padding following finetune -1.
fn protracker_arpeggio_period(finetune: u8, slot: usize) -> u32 {
    let absolute = (finetune & 15) as usize * 37 + slot;
    if absolute < 16 * 37 {
        let row = absolute / 37;
        let column = absolute % 37;
        if column == 36 { 0 } else { PROTRACKER_PERIODS[row][column] as u32 }
    } else {
        PROTRACKER_ARPEGGIO_OVERFLOW.get(absolute - 16 * 37).copied().unwrap_or(0) as u32
    }
}

fn tone_portamento_target(finetune: u8, packed_period: u16) -> (u8, u32) {
    let mut slot = period_table_slot(finetune, packed_period as u32);
    // PT compensates its signed finetune rows after scanning the selected row.
    if finetune & 8 != 0 && slot > 0 { slot -= 1; }
    let note = if slot < 36 { 36 + slot as u8 } else { NO_NOTE };
    (note, protracker_arpeggio_period(finetune, slot))
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

/// PT's ramp waveform with its magnitude read from `phase` and its half chosen by
/// `half_phase`. The two are the same value everywhere except under accuracy policy D20.
fn ramp_waveform_value(phase: u8, half_phase: u8) -> i16 {
    let index = (phase >> 2) & 31;
    match half_phase < 128 {
        true => (index << 3) as i16,
        false => -(255i16.saturating_sub((index << 3) as i16)),
    }
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

/// ProTracker scales an LFO by `mulu` on the unsigned magnitude and `lsr`, then adds or
/// subtracts by the sign of the phase counter. An arithmetic shift of the signed product
/// would round toward minus infinity and make the negative half one unit deeper.
fn lfo_delta(waveform: i16, depth: u8, shift: u32) -> i32 {
    let magnitude = ((waveform.unsigned_abs() as u32 * depth as u32) >> shift) as i32;
    if waveform < 0 { -magnitude } else { magnitude }
}

/// Paula's own range limit, as modelled by pt2-clone's `paulaSetPeriod`: a written zero
/// means 65536 — a near-silent ~54 Hz crawl rather than a frozen DC hold — and anything
/// below 113 is clamped up to 113.
///
/// The zero rule is unconditional: no format wants a frozen DC hold. The 113 floor is the
/// Amiga's, so it follows the loader's Amiga-limits flag: a four-channel ProTracker module
/// gets it, while extended-range MODs and MultiTracker — whose top octaves sit below 113
/// by design and which libxmp plays unclamped — do not.
fn step_from_period(period: u32, sample_rate_hz: u32, paula_floor: bool, clock: PaulaClock) -> Step {
    let hardware_period = match period {
        0 => 65_536,
        period if paula_floor => period.max(PAULA_MINIMUM_PERIOD),
        period => period,
    };
    Step::from_ratio(clock.hz(), hardware_period as u64 * sample_rate_hz.max(1) as u64)
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
/// MultiTracker's native pan grid, identical to the MTM loader's header mapping.
fn multitracker_pan_nibble(value: u8) -> I1F15 { bipolar_from_ratio(value.min(15) as i32 * 2 - 15, 15) }

// The trace projection of a pan value; the pan round-trip test uses it without `trace`.
#[cfg(any(feature = "trace", test))]
fn pan_trace(pan: I1F15) -> u16 {
    let scaled = ((pan.to_bits() as i32 + 32_768) * 255 + 32_767) / 65_535;
    scaled.clamp(0, 255) as u16
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use super::*;
    use starplayer_core::{ExactFixedPoint, RowClock, VoiceId};
    use starplayer_engine::{ChannelTable, ControlClock, EngineContext, EventSource, Jump, SongPosition};
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

    /// The same module as `processor_with_sample`, played under an explicit quirk set.
    fn processor_with_quirks(quirks: QuirkSet) -> ModProcessor {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![0; 64 * CELL_BYTES], 64, 1).expect("pattern");
        let sample = builder.add_sample(&vec![0; 1024], SampleSpec::one_shot("sample")).expect("sample");
        builder.add_instrument(InstrumentDef::from_sample("sample", sample, U0F16::MAX)).expect("instrument");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 1));
        let module = Arc::new(builder.build().expect("module"));
        ModProcessor::with_semantics_and_quirks(module, 44_100, EffectSemantics::ProTracker, QuirkSelection::Override(quirks))
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

    struct ProcessorHarness {
        voices: VoicePool,
        channels: ChannelTable,
    }

    impl ProcessorHarness {
        fn new() -> ProcessorHarness { ProcessorHarness { voices: VoicePool::new(2), channels: ChannelTable::new(1) } }

        fn row(&mut self, processor: &mut ModProcessor, cell: ModCell) -> TickOutcome {
            let mut context = TickContext::new(Frame::ZERO, &mut self.voices, &mut self.channels, RowClock::new(6), SongPosition::default(), 125);
            let bytes = cell.to_bytes();
            processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes })
        }

        fn tick(&mut self, processor: &mut ModProcessor, absolute_tick: u16, pattern_delay: u8) -> TickOutcome {
            let mut clock = RowClock::new(6);
            clock.pattern_delay = pattern_delay;
            clock.tick_in_row = absolute_tick;
            clock.repeat_index = (absolute_tick / 6) as u8;
            let mut context = TickContext::new(Frame::ZERO, &mut self.voices, &mut self.channels, clock, SongPosition::default(), 125);
            processor.tick(&mut context)
        }

        fn foreground(&self) -> Option<VoiceId> { self.channels.foreground(ChannelId(0)) }

        fn position(&self) -> Option<u64> { self.foreground().and_then(|voice| self.voices.get(voice)).map(|voice| voice.position()) }

        fn region_length(&self) -> Option<u32> { self.foreground().and_then(|voice| self.voices.get(voice)).map(|voice| voice.region().length_frames()) }

        fn remaining_frames(&self) -> Option<u32> {
            self.foreground().and_then(|voice| self.voices.get(voice))
                .map(|voice| voice.region().length_frames().saturating_sub((voice.position() >> 32) as u32))
        }

        fn pending_region(&self) -> Option<starplayer_mixer::SampleRegion> {
            self.foreground().and_then(|voice| self.voices.get(voice)).and_then(|voice| voice.pending_region())
        }

        fn region(&self) -> Option<starplayer_mixer::SampleRegion> {
            self.foreground().and_then(|voice| self.voices.get(voice)).map(|voice| voice.region())
        }

        /// Render `frames` output frames of the channel's voice, so a boundary the
        /// processor only queued actually arrives.
        fn render(&mut self, module: &Module, frames: usize) {
            let mut output = vec![starplayer_mixer::FixedFrame::default(); frames];
            self.voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(module.pcm(), &mut output);
        }
    }

    /// Three MOD instruments: two 64-frame forward loops and one empty slot, which is
    /// what ProTracker reads as its null sample.
    fn swap_module() -> Arc<Module> {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![0; 64 * CELL_BYTES], 64, 1).expect("pattern");
        for base in [1_000i16, 5_000] {
            let pcm: Vec<i16> = (0..64).map(|index| base + index as i16).collect();
            let sample = builder.add_sample(&pcm, SampleSpec::one_shot("looping").with_forward_loop(0, 64)).expect("sample");
            builder.add_instrument(InstrumentDef::from_sample("looping", sample, U0F16::MAX)).expect("instrument");
        }
        let empty = builder.add_sample(&[], SampleSpec::one_shot("empty")).expect("empty sample");
        builder.add_instrument(InstrumentDef::from_sample("empty", empty, U0F16::MAX)).expect("empty instrument");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 1));
        Arc::new(builder.build().expect("module"))
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
    fn a_note_with_9xx_keeps_pt_s_double_offset_pointer_rule() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 1 });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "PT retains a second addition after starting at 0x100");
    }

    #[test]
    fn no_note_9xx_applies_parameter_memory_to_the_retained_pointer() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = process_row(&mut processor, ModCell { effect: 9, param: 1, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(256));
        let _ = process_row(&mut processor, ModCell { effect: 9, param: 0, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "900 reapplies the last nonzero offset without a note");
    }

    #[test]
    fn a_new_instrument_resets_the_pointer_before_900_recall() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 1 });
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 0, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "a note without an instrument reuses PT's retained pointer");
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 0 });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(512), "900 recalls the last nonzero offset after a new instrument resets the pointer");
    }

    #[test]
    fn an_initial_past_end_offset_keeps_the_sample_base_as_a_one_word_region() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 128);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = harness.row(&mut processor, ModCell { effect: 9, param: 1, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(0));
        assert_eq!(processor.channel(0).map(|state| state.offset_past_end), Some(true));
        let _ = harness.row(&mut processor, ModCell { effect: 0xE, param: 0x92, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_offset), Some(0), "the failed first advance retains the sample base");
        assert_eq!(processor.channel(0).map(|state| state.offset_past_end), Some(true));
        assert_eq!(harness.position(), Some(0));
        assert_eq!(harness.region_length(), Some(2));
    }

    #[test]
    fn a_later_past_end_offset_keeps_the_last_successful_pointer_as_one_word() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = harness.row(&mut processor, ModCell { effect: 9, param: 1, ..ModCell::EMPTY });
        assert_eq!(processor.channels[0].sample_offset, 256);
        assert!(!processor.channels[0].offset_past_end);

        let _ = harness.row(&mut processor, ModCell { effect: 9, param: 3, ..ModCell::EMPTY });
        assert_eq!(processor.channels[0].sample_offset, 256, "0x300 equals the 768-frame remainder, so PT leaves n_start unchanged");
        assert!(processor.channels[0].offset_past_end);

        let _ = harness.row(&mut processor, ModCell { effect: 0xE, param: 0x92, ..ModCell::EMPTY });
        assert_eq!(harness.position(), Some(256u64 << 32));
        assert_eq!(harness.remaining_frames(), Some(2));
        assert_eq!(processor.channels[0].sample_offset, 256, "E9 retains the failed-offset pointer");
    }

    #[test]
    fn speed_and_tempo_split_at_thirty_two_and_f00_stops() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 31, ..ModCell::EMPTY }).speed, 31);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 32, ..ModCell::EMPTY }).tempo_bpm, 125);
        assert_eq!(process_tick(&mut processor, 1, 0).tempo_bpm, 32);
        assert!(process_row(&mut processor, ModCell { effect: 0xF, param: 0, ..ModCell::EMPTY }).stop);
    }

    /// C10 deliverable 2: under `ModTiming::VBlank` there is no CIA timer, so `F20` and
    /// `F30` are 32- and 48-tick rows and the tempo is never touched.
    #[test]
    fn under_vblank_every_non_zero_fxx_is_a_speed_and_no_tempo_is_latched() {
        let mut processor = processor_with_quirks(QuirkSet { mod_timing: ModTiming::VBlank, ..QuirkSet::canonical() });
        let fermata = process_row(&mut processor, ModCell { effect: 0xF, param: 0x20, ..ModCell::EMPTY });
        assert_eq!(fermata.speed, 32, "F20 is a 32-tick row on the vertical blank");
        assert_eq!(fermata.tempo_bpm, 125, "no BPM is set");
        assert!(processor.pending_tempo.is_none(), "the CIA latch is never armed under VBlank");
        assert_eq!(process_tick(&mut processor, 1, 0).tempo_bpm, 125, "and nothing arrives a tick later either");

        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 0x30, ..ModCell::EMPTY }).speed, 48);
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 0x7D, ..ModCell::EMPTY }).speed, 125, "even F7D is a speed");
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 0x04, ..ModCell::EMPTY }).speed, 4, "a low Fxx is unchanged");
        assert!(process_row(&mut processor, ModCell { effect: 0xF, param: 0, ..ModCell::EMPTY }).stop, "F00 keeps its stop-marker rule");
    }

    /// And the CIA profile the same module would otherwise get is untouched.
    #[test]
    fn under_cia_a_high_fxx_is_still_a_deferred_tempo() {
        let mut processor = processor_with_quirks(QuirkSet::canonical());
        assert_eq!(process_row(&mut processor, ModCell { effect: 0xF, param: 0x20, ..ModCell::EMPTY }).tempo_bpm, 125);
        assert_eq!(process_tick(&mut processor, 1, 0).tempo_bpm, 32, "the CIA latch is adopted at the next tracker event");
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
        assert_eq!(processor.channels[0].actual_period, 850, "the flat PT table wraps into finetune +1 after its zero sentinel");
    }

    #[test]
    fn arpeggio_searches_the_period_reached_by_a_plain_slide() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let state = &mut processor.channels[0];
        state.current_note = 48;
        state.current_period = 428;
        processor.slide_period(0, false, 1);
        processor.channels[0].command_data = 0x01;
        processor.arpeggio(0, 1);
        assert_eq!(processor.channels[0].actual_period, 404, "a zero offset on arpeggio phase one still performs PT's table search");
        processor.channels[0].command_data = 0x10;
        processor.arpeggio(0, 1);
        assert_eq!(processor.channels[0].current_period, 427);
        assert_eq!(processor.channels[0].actual_period, 381, "427 scans to C# before the one-semitone arpeggio offset");
    }

    #[test]
    fn arpeggio_searches_a_completed_tone_portamento_period() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let state = &mut processor.channels[0];
        state.current_note = 48;
        state.current_period = 430;
        state.target_period = 404;
        state.target_note = 49;
        state.portamento_memory = 32;
        state.command_data = 0x10;
        processor.tone_portamento(0);
        assert_eq!(processor.channels[0].target_period, 0);
        processor.arpeggio(0, 1);
        assert_eq!(processor.channels[0].actual_period, 381, "the finished C# target, not stale C note state, is the base");
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

    #[test]
    fn tone_portamento_scans_the_selected_positive_finetune_row() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[7], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = process_row(&mut processor, ModCell { period: 420, effect: 3, param: 4, ..ModCell::EMPTY });
        assert_eq!(processor.channels[0].target_period, 407, "selected +7 scan differs from rebuilding zero-row slot 13 as 384");
    }

    #[test]
    fn tone_portamento_applies_pt_s_negative_finetune_slot_adjustment() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[15], 1024);
        let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = process_row(&mut processor, ModCell { period: 420, effect: 3, param: 4, ..ModCell::EMPTY });
        assert_eq!(processor.channels[0].target_period, 431, "-1 scans slot 13 then applies PT's one-slot negative-finetune correction");
    }

    #[test]
    fn ed0_triggers_only_a_note_from_its_own_row_and_repeats_with_ee() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, effect: 0xE, param: 0xD0 });
        let first = harness.foreground().expect("ED0 note triggers at row tick zero");
        let _ = harness.tick(&mut processor, 6, 1);
        let repeated = harness.foreground().expect("ED0 note repeats at delayed tick zero");
        assert_ne!(first, repeated);

        let _ = harness.row(&mut processor, ModCell { effect: 0xE, param: 0xD0, ..ModCell::EMPTY });
        let no_note = harness.foreground();
        let _ = harness.tick(&mut processor, 6, 1);
        assert_eq!(harness.foreground(), no_note, "a no-note ED0 does not reuse historical current_note");
    }

    #[test]
    fn positive_ed_waits_for_its_tick_and_retriggers_on_pattern_delay_repeats() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, effect: 0xE, param: 0xD2 });
        assert_eq!(harness.foreground(), None, "ED2 does not trigger while the row is latched");
        let _ = harness.tick(&mut processor, 1, 0);
        assert_eq!(harness.foreground(), None);
        let _ = harness.tick(&mut processor, 2, 0);
        let first = harness.foreground().expect("ED2 triggers on tick two");
        let _ = harness.tick(&mut processor, 8, 1);
        assert_ne!(harness.foreground(), Some(first), "ED2 triggers again at tick two of the EEx repeat");
    }

    #[test]
    fn no_note_positive_ed_does_not_retrigger_a_historical_note() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let original = harness.foreground();
        let _ = harness.row(&mut processor, ModCell { effect: 0xE, param: 0xD2, ..ModCell::EMPTY });
        let _ = harness.tick(&mut processor, 2, 0);
        let _ = harness.tick(&mut processor, 8, 1);
        assert_eq!(harness.foreground(), original);
    }

    #[test]
    fn e9_note_rows_skip_tick_zero_while_no_note_rows_retrigger_retained_offsets() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, effect: 9, param: 1 });
        assert_eq!(processor.channels[0].sample_offset, 512);

        let _ = harness.row(&mut processor, ModCell { period: 404, effect: 0xE, param: 0x92, ..ModCell::EMPTY });
        let note_tick_zero = harness.foreground().expect("the note itself triggers once");
        assert_eq!(harness.position(), Some(512u64 << 32));
        let _ = harness.tick(&mut processor, 6, 1);
        assert_eq!(harness.foreground(), Some(note_tick_zero), "row-delay tick zero does not double-trigger a note row");

        let _ = harness.row(&mut processor, ModCell { effect: 0xE, param: 0x92, ..ModCell::EMPTY });
        let no_note_tick_zero = harness.foreground().expect("no-note E92 retriggers at tick zero");
        assert_ne!(no_note_tick_zero, note_tick_zero);
        assert_eq!(harness.position(), Some(512u64 << 32));
        let _ = harness.tick(&mut processor, 2, 0);
        let interval_retrigger = harness.foreground().expect("E92 retriggers on tick two");
        assert_ne!(interval_retrigger, no_note_tick_zero);
        assert_eq!(harness.position(), Some(512u64 << 32));
        let _ = harness.tick(&mut processor, 6, 1);
        assert_ne!(harness.foreground(), Some(interval_retrigger), "no-note E92 retriggers at repeated tick zero");
        assert_eq!(processor.channels[0].sample_offset, 512);
    }

    #[test]
    fn delayed_notes_preserve_all_lfo_waveform_phases_at_the_trigger_tick() {
        for selector in 0..=7 {
            let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
            processor.channels[0].vibrato_waveform = selector;
            processor.channels[0].tremolo_waveform = selector;
            processor.channels[0].vibrato_phase = 76;
            processor.channels[0].tremolo_phase = 92;
            let _ = process_row(&mut processor, ModCell { period: 428, instrument: 1, effect: 0xE, param: 0xD2 });
            assert_eq!((processor.channels[0].vibrato_phase, processor.channels[0].tremolo_phase), (76, 92), "selector {selector} is unchanged while ED2 waits");
            let _ = process_tick(&mut processor, 1, 0);
            assert_eq!((processor.channels[0].vibrato_phase, processor.channels[0].tremolo_phase), (76, 92));
            let _ = process_tick(&mut processor, 2, 0);
            assert_eq!((processor.channels[0].vibrato_phase, processor.channels[0].tremolo_phase), (76, 92), "PT noteDelay/doRetrg preserves selector {selector}");
        }
    }

    #[test]
    fn bpm_change_uses_the_old_tick_zero_interval_then_the_new_cia_interval() {
        let mut pattern = vec![0; 64 * CELL_BYTES];
        pattern[..CELL_BYTES].copy_from_slice(&ModCell { effect: 0xF, param: 250, ..ModCell::EMPTY }.to_bytes());
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&pattern, 64, 1).expect("pattern");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 1));
        let module = Arc::new(builder.build().expect("module"));
        let mut sequencer = sequencer_for(module, 44_100, ExactFixedPoint);
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut control = ControlClock::new(44_100, Frame::ZERO);
        let mut frames = Vec::new();
        for _ in 0..3 {
            let frame = sequencer.next_event_frame().expect("tick");
            frames.push(frame);
            let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
            sequencer.dispatch(frame, &mut context);
        }
        assert_eq!(frames, vec![Frame(0), Frame(882), Frame(1323)]);
        assert_eq!(sequencer.tempo_bpm(), 250);
    }

    // ── C3b: ProTracker fidelity repairs ───────────────────────────────────────────

    #[test]
    fn an_lfo_rounds_its_magnitude_rather_than_toward_minus_infinity() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let state = &mut processor.channels[0];
        state.current_period = 428;
        state.vibrato_phase = 132;
        state.vibrato_memory = 0x0F;
        processor.vibrato(0);
        assert_eq!(processor.channels[0].actual_period, 426, "PT multiplies the unsigned magnitude and subtracts; an arithmetic shift would give 425");
        assert_eq!(lfo_delta(-24, 15, 7), -2);
        assert_eq!(lfo_delta(24, 15, 7), 2, "both halves of the waveform are the same depth");
        assert_eq!(lfo_delta(-24, 15, 6), -5);
        assert_eq!(lfo_delta(24, 15, 6), 5);
    }

    #[test]
    fn vibrato_writes_past_the_amiga_limit_because_pt_writes_it_straight_to_paula() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let _ = process_row(&mut processor, ModCell { period: 856, instrument: 1, effect: 4, param: 0xFF });
        let deepest = (1..6).map(|tick| {
            let _ = process_tick(&mut processor, tick, 0);
            processor.channels[0].actual_period
        }).max();
        assert_eq!(deepest, Some(885), "4FF on C-1 keeps its whole downward excursion under Amiga limits");
        assert_eq!(processor.channels[0].current_period, 856, "the unmodulated period is untouched");
    }

    #[test]
    fn the_paula_period_floor_and_the_period_zero_rule_apply_at_step_derivation() {
        let pal = PaulaClock::Pal;
        assert_eq!(step_from_period(1, 44_100, true, pal), step_from_period(113, 44_100, true, pal), "Paula clamps anything below 113 up to 113");
        assert_eq!(step_from_period(112, 44_100, true, pal), step_from_period(113, 44_100, true, pal));
        assert_eq!(step_from_period(0, 44_100, true, pal), Step::from_ratio(PaulaClock::Pal.hz(), 65_536 * 44_100), "a written zero means 65536, not a DC hold");
        assert_ne!(step_from_period(0, 44_100, true, pal), Step::ZERO);
        assert_eq!(step_from_period(428, 44_100, true, pal), Step::from_ratio(PaulaClock::Pal.hz(), 428 * 44_100), "an ordinary period is unchanged");
        assert_eq!(step_from_period(56, 44_100, false, pal), Step::from_ratio(PaulaClock::Pal.hz(), 56 * 44_100), "without Amiga limits (MultiTracker, extended-range MODs) the top octaves stay below 113");
        assert_eq!(step_from_period(0, 44_100, false, pal), step_from_period(0, 44_100, true, pal), "the zero rule does not depend on the flag");
    }

    /// D14: the Paula clock is a `QuirkSet` field, and nothing else about the derivation
    /// changes with it. No corpus case selects NTSC, so this is the only thing that sees
    /// the field.
    #[test]
    fn the_paula_clock_quirk_selects_the_ntsc_rate_and_nothing_else() {
        assert_eq!(step_from_period(428, 44_100, true, PaulaClock::Pal), Step::from_ratio(3_546_895, 428 * 44_100));
        assert_eq!(step_from_period(428, 44_100, true, PaulaClock::Ntsc), Step::from_ratio(3_579_545, 428 * 44_100));
        assert!(step_from_period(428, 44_100, true, PaulaClock::Ntsc) > step_from_period(428, 44_100, true, PaulaClock::Pal), "NTSC plays a MOD about 0.36 % sharp");
        assert_eq!(QuirkSet::canonical().mod_paula_clock, PaulaClock::Pal, "the canonical profile is PAL");
    }

    #[test]
    fn a_pattern_loop_mark_survives_a_pattern_change() {
        let mut processor = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut row_at = |processor: &mut ModProcessor, pattern: u16, row: u16, cell: ModCell| {
            let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
            processor.row_effects(&mut context, 0, pattern, row, [EffectCell::from(cell)])
        };
        assert_eq!(row_at(&mut processor, 0, 8, ModCell { effect: 0xE, param: 0x60, ..ModCell::EMPTY }).jump, None);
        assert_eq!(processor.pattern_flow().start(0), 8);
        let jump = row_at(&mut processor, 1, 4, ModCell { effect: 0xE, param: 0x61, ..ModCell::EMPTY }).jump;
        assert_eq!(jump, Some(Jump::within_pattern_to_row(8)), "PT's per-channel n_pattpos persists across a pattern change");
        assert_eq!(processor.pattern_flow().start(0), 8, "only E60 writes the mark");
    }

    #[test]
    fn f00_stops_under_protracker_and_is_ignored_under_multitracker() {
        let mut protracker = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        assert!(process_row(&mut protracker, ModCell { effect: 0xF, param: 0, ..ModCell::EMPTY }).stop);

        assert!(protracker.quirks().mod_f00_stops_song, "the canonical MOD profile stops on F00");

        let mut multitracker = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        multitracker.semantics = EffectSemantics::MultiTracker { reset_counterpart: true };
        multitracker.quirks = QuirkSet::multitracker();
        let outcome = process_row(&mut multitracker, ModCell { effect: 0xF, param: 0, ..ModCell::EMPTY });
        assert!(!outcome.stop, "libxmp's fx_s3m_speed ignores F00 outright");
        assert_eq!((outcome.speed, outcome.tempo_bpm), (6, 125), "and it changes neither counterpart");
    }

    /// The `Dxx` parameter encoding is a `QuirkSet` field, not a branch on the format:
    /// ProTracker reads `D16` as row 16 and MultiTracker as row 22.
    #[test]
    fn the_break_parameter_encoding_field_selects_bcd_or_hexadecimal() {
        let mut protracker = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        assert_eq!(protracker.quirks().mod_break_parameter, BreakParameter::BinaryCodedDecimal);
        assert_eq!(process_row(&mut protracker, ModCell { effect: 0xD, param: 0x16, ..ModCell::EMPTY }).jump, Some(Jump::break_to_row(16)));
        assert_eq!(process_row(&mut protracker, ModCell { effect: 0xD, param: 0x64, ..ModCell::EMPTY }).jump, Some(Jump::break_to_row(0)), "a BCD value past row 63 wraps to row zero");

        let mut multitracker = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        multitracker.quirks = QuirkSet::multitracker();
        assert_eq!(process_row(&mut multitracker, ModCell { effect: 0xD, param: 0x16, ..ModCell::EMPTY }).jump, Some(Jump::break_to_row(0x16)));
    }

    /// D20: with the quirk off the tremolo ramp reads its own phase; with it on it takes
    /// the half from the vibrato phase, which is PT's `mt_Tremolo2` bug. No corpus oracle
    /// models it, so this test is the only thing that can see the field.
    #[test]
    fn the_tremolo_ramp_quirk_reads_the_half_from_the_vibrato_phase() {
        let mut canonical = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        let mut buggy = processor_with_sample(true, FINETUNE_REFERENCE_RATES[0], 1024);
        buggy.quirks = QuirkSet { protracker_tremolo_ramp_from_vibrato_phase: true, ..QuirkSet::canonical() };
        for processor in [&mut canonical, &mut buggy] {
            processor.channels[0].current_volume = 32;
            processor.channels[0].actual_volume = 32;
            processor.channels[0].tremolo_waveform = 1;
            processor.channels[0].tremolo_memory = 0x0F;
            // The two phases sit in opposite halves, which is the only case that differs.
            processor.channels[0].tremolo_phase = 0;
            processor.channels[0].vibrato_phase = 160;
        }
        canonical.tremolo(0);
        buggy.tremolo(0);
        assert_eq!(canonical.channels[0].actual_volume, 32, "the ramp's own phase 0 is the bottom of its rising half");
        assert_eq!(buggy.channels[0].actual_volume, 0, "PT reads the falling half because n_vibratopos is negative");
        assert!(!QuirkSet::canonical().protracker_tremolo_ramp_from_vibrato_phase, "the bug is off by default");
    }

    /// The 8xx byte and libxmp's `fxp << 4` E8x nibble both survive the trip through
    /// `pan_byte` and the trace projection unchanged, so the conformance comparison sees
    /// exactly the byte the command carried.
    #[test]
    fn protracker_pan_bytes_round_trip_through_the_trace_projection() {
        for byte in 0..=255u8 {
            assert_eq!(pan_trace(pan_byte(byte)), byte as u16, "8xx {byte:#04x} did not round-trip");
        }
        assert_eq!(pan_trace(pan_byte(0xF << 4)), 240, "E8F is libxmp's fxp << 4, not hard right");
        assert_eq!(pan_trace(multitracker_pan_nibble(15)), 255, "MultiTracker's own E8x grid reaches hard right");
        assert_eq!(pan_trace(multitracker_pan_nibble(8)), 136);
    }

    #[test]
    fn a_seeked_render_is_byte_identical_to_a_fresh_render_of_the_same_order() {
        let module = two_order_module();
        let fresh = render_from_order(&module, 0, 1, 16_384);
        let after_a_dirty_prefix = render_from_order(&module, 7, 1, 16_384);
        assert_eq!(after_a_dirty_prefix, fresh, "a pending CIA tempo or an effect memory survived the seek");
        assert!(fresh.iter().any(|frame| frame.left != 0), "the comparison rendered actual audio");
    }

    fn two_order_module() -> Arc<Module> {
        // Order 0 leaves a vibrato memory on row 0 and a deferred CIA tempo on row 1, so
        // a warm-up of exactly seven ticks stops with both still live.
        let mut first = vec![0; 64 * CELL_BYTES];
        first[..CELL_BYTES].copy_from_slice(&ModCell { period: 428, instrument: 1, effect: 4, param: 0x8F }.to_bytes());
        let second_row = CELL_BYTES;
        first[second_row..second_row + CELL_BYTES].copy_from_slice(&ModCell { effect: 0xF, param: 0x64, ..ModCell::EMPTY }.to_bytes());
        // Order 1 recalls the vibrato memory on row 0 and retriggers on row 1, so both a
        // depth difference and a row-length difference would be audible.
        let mut second = vec![0; 64 * CELL_BYTES];
        second[..CELL_BYTES].copy_from_slice(&ModCell { period: 428, instrument: 1, effect: 4, param: 0 }.to_bytes());
        second[second_row..second_row + CELL_BYTES].copy_from_slice(&ModCell { period: 428, instrument: 1, ..ModCell::EMPTY }.to_bytes());

        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&first, 64, 1).expect("first pattern");
        builder.add_pattern(&second, 64, 1).expect("second pattern");
        let pcm: Vec<i16> = (0..1_024).map(|index| (index as i16).wrapping_mul(31)).collect();
        let sample = builder.add_sample(&pcm, SampleSpec::one_shot("sample")).expect("sample");
        builder.add_instrument(InstrumentDef::from_sample("sample", sample, U0F16::MAX)).expect("instrument");
        builder.set_orders(&[0, 1, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 1));
        Arc::new(builder.build().expect("module"))
    }

    /// Dispatch `warm_up_ticks` ticks, seek to `order`, then render `frames` output frames
    /// from a mixer state that is fresh either way — so the only thing that can differ is
    /// what the processor carried across the seek.
    fn render_from_order(module: &Arc<Module>, warm_up_ticks: usize, order: u16, frames: usize) -> Vec<starplayer_mixer::FixedFrame> {
        let mut sequencer = sequencer_for(Arc::clone(module), 44_100, ExactFixedPoint);
        {
            let mut voices = VoicePool::new(4);
            let mut channels = ChannelTable::new(1);
            let mut control = ControlClock::new(44_100, Frame::ZERO);
            for _ in 0..warm_up_ticks {
                let Some(frame) = sequencer.next_event_frame() else { break };
                let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
                sequencer.dispatch(frame, &mut context);
            }
        }
        assert!(sequencer.seek_order(order), "the seek target resolves");
        sequencer.restart_clock_at(Frame::ZERO);

        let mut voices = VoicePool::new(4);
        let mut channels = ChannelTable::new(1);
        let mut control = ControlClock::new(44_100, Frame::ZERO);
        let mut output = vec![starplayer_mixer::FixedFrame::default(); frames];
        let mut produced = 0;
        while produced < frames {
            let next = sequencer.next_event_frame().map(|frame| frame.0 as usize).unwrap_or(frames).min(frames);
            if next > produced {
                voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(module.pcm(), &mut output[produced..next]);
                produced = next;
                continue;
            }
            if sequencer.next_event_frame().is_none() { break; }
            let frame = Frame(produced as u64);
            let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
            sequencer.dispatch(frame, &mut context);
        }
        output
    }

    // ── C3b / accuracy policy D12: the queued sample swap ──────────────────────────

    #[test]
    fn a_lone_instrument_queues_the_replacement_instead_of_restarting_the_voice() {
        let module = swap_module();
        let mut processor = ModProcessor::new(Arc::clone(&module), 44_100);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let voice = harness.foreground().expect("the note started a voice");
        let first_region = harness.region().expect("a sounding region");

        let _ = harness.row(&mut processor, ModCell { instrument: 2, ..ModCell::EMPTY });
        assert_eq!(harness.foreground(), Some(voice), "a lone instrument never restarts the voice");
        assert_eq!(harness.region(), Some(first_region), "and does not change the sounding sample yet");
        assert!(harness.pending_region().is_some(), "it queues the replacement for the loop point");
        assert_eq!(processor.channel(0).map(|state| state.sample_number), Some(2), "volume, finetune and the reported number apply at once");

        harness.render(&module, 512);
        assert_eq!(harness.foreground(), Some(voice), "the swap keeps the same voice");
        assert_ne!(harness.region(), Some(first_region), "the loop point adopted the queued sample");
        assert_eq!(harness.pending_region(), None);
    }

    #[test]
    fn a_tone_portamento_naming_another_sample_queues_it_too() {
        let module = swap_module();
        let mut processor = ModProcessor::new(Arc::clone(&module), 44_100);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let voice = harness.foreground().expect("the note started a voice");
        let _ = harness.row(&mut processor, ModCell { period: 404, instrument: 2, effect: 3, param: 4 });
        assert_eq!(harness.foreground(), Some(voice), "3xy never retriggers");
        assert!(harness.pending_region().is_some(), "the named sample waits for the boundary");
    }

    #[test]
    fn an_empty_instrument_slot_is_pt_s_null_sample() {
        let module = swap_module();
        let mut processor = ModProcessor::new(Arc::clone(&module), 44_100);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = harness.row(&mut processor, ModCell { instrument: 3, ..ModCell::EMPTY });
        assert_eq!(processor.channel(0).map(|state| state.sample_number), Some(1), "an empty slot leaves the channel's sample, volume and finetune alone");
        assert_eq!(harness.pending_region().map(|region| region.length_frames()), Some(0), "it queues the null sample");
        harness.render(&module, 512);
        assert_eq!(harness.foreground().and_then(|voice| harness.voices.get(voice)), None, "which stops the voice at its loop point");
    }

    #[test]
    fn a_lone_instrument_restarts_a_channel_whose_sample_has_already_stopped() {
        let module = swap_module();
        let mut processor = ModProcessor::new(Arc::clone(&module), 44_100);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = harness.row(&mut processor, ModCell { instrument: 3, ..ModCell::EMPTY });
        harness.render(&module, 512);
        assert!(harness.foreground().and_then(|voice| harness.voices.get(voice)).is_none(), "the null sample stopped it");

        let _ = harness.row(&mut processor, ModCell { instrument: 2, ..ModCell::EMPTY });
        assert!(harness.foreground().and_then(|voice| harness.voices.get(voice)).is_some(), "there is no boundary left to wait for, so PT starts it now");
        assert_eq!(processor.channel(0).map(|state| state.sample_number), Some(2));
    }

    /// Research point 2: MultiTracker is not a ProTracker dialect here. libxmp gates the
    /// queued swap on `QUIRK_PROTRACK`, which its MTM loader does not set, so an MTM
    /// instrument column applies volume and finetune and leaves the sounding sample
    /// alone — no queue, and no restart either.
    #[test]
    fn multitracker_does_not_queue_a_sample_swap() {
        let module = swap_module();
        let mut processor = ModProcessor::with_semantics(Arc::clone(&module), 44_100, EffectSemantics::MultiTracker { reset_counterpart: true });
        assert!(!processor.quirks().protracker_sample_swap_at_boundary, "D12 is a QuirkSet field, and MultiTracker's is off");
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let region = harness.region().expect("a sounding region");
        let _ = harness.row(&mut processor, ModCell { instrument: 2, ..ModCell::EMPTY });
        assert_eq!(harness.pending_region(), None, "MTM has no queued swap");
        assert_eq!(harness.region(), Some(region), "and does not restart the voice either");
        assert_eq!(processor.channel(0).map(|state| state.sample_number), Some(2));

        // The same module and the same ProTracker command semantics, with the field on:
        // the field is what decides, not the format.
        let mut protracker = ModProcessor::with_semantics_and_quirks(module, 44_100, EffectSemantics::MultiTracker { reset_counterpart: true }, QuirkSelection::Override(QuirkSet::canonical()));
        assert!(protracker.quirks().protracker_sample_swap_at_boundary);
        let mut harness = ProcessorHarness::new();
        let _ = harness.row(&mut protracker, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
        let _ = harness.row(&mut protracker, ModCell { instrument: 2, ..ModCell::EMPTY });
        assert!(harness.pending_region().is_some(), "with the field on the replacement is queued for the loop point");
    }

    #[test]
    fn a_queued_swap_allocates_nothing_and_survives_every_block_size() {
        let module = swap_module();
        let render = |chunk: usize| {
            let mut processor = ModProcessor::new(Arc::clone(&module), 44_100);
            let mut harness = ProcessorHarness::new();
            let _ = harness.row(&mut processor, ModCell { period: 428, instrument: 1, ..ModCell::EMPTY });
            let _ = harness.row(&mut processor, ModCell { instrument: 2, ..ModCell::EMPTY });
            let voice = harness.foreground().expect("a sounding voice");
            let mut output = vec![starplayer_mixer::FixedFrame::default(); 1_024];
            let mut written = 0;
            while written < output.len() {
                let end = (written + chunk).min(output.len());
                if let (Some(window), Some(voice)) = (output.get_mut(written..end), harness.voices.get_mut(voice)) {
                    let _ = starplayer_mixer::accumulate_voice::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(voice, module.pcm(), window);
                }
                written = end;
            }
            output
        };
        let whole = render(1_024);
        for chunk in [1usize, 3, 64, 128, 4_096, 8_191] {
            assert_eq!(render(chunk), whole, "block size {chunk} moved the queued swap");
        }
    }
}

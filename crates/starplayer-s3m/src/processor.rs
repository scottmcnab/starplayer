//! Scream Tracker 3's tick-zero and per-tick effect processor.

// Every channel index originates from a loop bounded by `channels.len()` and every table
// index is masked to its declared domain. Keeping that invariant at the dispatch boundary
// makes the individual handlers readable as the assembly they port.
#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec;

use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::tables::{PERIOD_TABLE, ST3_FREQUENCY_NUMERATOR, ST3_PERIOD_SCALE, waveform_sample};
use starplayer_core::quirks::{QuirkSelection, QuirkSet};
use starplayer_core::{ChannelId, DirtyBits, Frame, InstrumentId, Note, Step, TempoModel, TempoModelId, U0F16, VoiceParams};
use starplayer_engine::{EndOfSongPolicy, OrderEntry, PatternData, PatternFlowState, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TrackerProcessor};
#[cfg(any(feature = "trace", test))]
use starplayer_engine::TraceChannelState;
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{EffectNames, LoopMode, Module, OrderEntry as ModelOrderEntry};
use starplayer_rt::Arc;

use crate::header::pan_nibble_to_bipolar;
use crate::pattern::{CELL_BYTES, COMMAND_NONE, INSTRUMENT_NONE, NOTE_CUT, NOTE_NONE, S3mCell, VOLUME_NONE};

const NO_SAMPLE: u8 = 255;
const DEFAULT_PERIOD: u32 = 1712;
/// D26: Scream Tracker 3 clamps the *output* period to at least 64 in its own period x 4
/// domain, and stops the channel outright once a slide reaches zero (OpenMPT
/// `PeriodLimit.s3m`). The unclamped period keeps sliding underneath the clamp.
const MIN_OUTPUT_PERIOD: u32 = 64;
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
    pub parameter_memory: u8,             // `_VolSlideValue`, D25: ST3's one shared memory
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
    /// D29: set by `^^^`/`SCx`, which silence the channel without stopping the voice.
    /// `Qxy` refuses to retrigger a cut channel (OpenMPT `RetrigAfterNoteCut.s3m`).
    pub note_cut: bool,
    /// D26: a slide reached period zero; the voice stops at the next flush.
    pub stop_voice: bool,
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
            parameter_memory: 0,
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
            note_cut: false,
            stop_voice: false,
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
    /// The replay behaviour this module was loaded with. Resolved once, at construction,
    /// and never written again — see [`starplayer_core::quirks`].
    quirks: QuirkSet,
    /// `SB0`/`SBx`, `Bxx` and `Cxx` bookkeeping under the module's
    /// [`S3mLoopDialect`](starplayer_core::quirks::S3mLoopDialect). D30 is the ST3.21
    /// baseline it defaults to.
    flow: PatternFlowState,
    last_order: Option<(u16, u16)>,
}

impl S3mProcessor {
    /// An ST3 processor whose quirks come from the dialect the loader detected.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> S3mProcessor {
        S3mProcessor::with_quirks(module, sample_rate_hz, QuirkSelection::FromDialect)
    }

    /// An ST3 processor with an explicit [`QuirkSelection`]: `FromDialect` takes the
    /// loader's answer, `Override` takes the host's.
    pub fn with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> S3mProcessor {
        let header = module.header();
        let quirks = quirks.resolve(header.dialect);
        let global_volume = ((header.global_volume.to_bits() as u32 * 64 + 32767) / 65535) as u8;
        let mut states = VecBuilder::new();
        for index in 0..header.channel_count {
            let pan = header.default_pan.get(index as usize).copied().map(pan_to_nibble).unwrap_or(crate::header::PAN_CENTRE);
            states.push(S3mChannel::new(index, pan));
        }
        let header_channel_count = header.channel_count as usize;
        S3mProcessor {
            amiga_limits: header.flags.amiga_limits,
            module,
            channels: states.finish(),
            sample_rate_hz,
            global_volume,
            quirks,
            flow: PatternFlowState::new(quirks.s3m_pattern_loop.flow(), header_channel_count),
            last_order: None,
        }
    }

    /// The replay behaviour in force. Fixed for the lifetime of the loaded module.
    pub const fn quirks(&self) -> QuirkSet { self.quirks }

    /// The pattern-loop and break/jump bookkeeping, for inspection by a host or a test.
    pub const fn pattern_flow(&self) -> &PatternFlowState { &self.flow }

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

        // D28: `Gxx`/`Lxx` keeps the sounding sample and its C2SPD; only the named
        // instrument's default volume is adopted (OpenMPT `PortaSmpChange.s3m`). The
        // original assigns `_SampleNum` before it ever looks at the command.
        let sounding = context.channels.is_sounding(channel_id, context.voices);
        let tone_portamento = matches!(cell.command, 7 | 12) && sounding && self.channels[channel_index].sample_number != NO_SAMPLE;

        if cell.instrument != INSTRUMENT_NONE {
            let instrument_id = InstrumentId((cell.instrument - 1) as u16);
            if let Some(sample_id) = self.module.instrument(instrument_id).and_then(|instrument| instrument.sample)
                && let Some(sample) = self.module.sample(sample_id)
            {
                if !tone_portamento {
                    // PtrToSample succeeded: only now does the assembly assign _SampleNum.
                    self.channels[channel_index].sample_number = cell.instrument;
                    // D7: SampleIndex preserves and this reads the full 32-bit C2SPD.
                    self.channels[channel_index].reference_rate_hz = sample.reference_rate_hz();
                }
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
                let period = period_from_note(note, self.channels[channel_index].reference_rate_hz);
                if tone_portamento {
                    self.channels[channel_index].target_note = note;
                    self.channels[channel_index].target_period = period;
                    self.channels[channel_index].note_cut = false;
                } else {
                    let state = &mut self.channels[channel_index];
                    state.note_cut = false;
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

    /// `^^^` and `SCx`.
    ///
    /// D29: Scream Tracker 3 only takes the channel volume to zero; the voice keeps
    /// running, which is why `Qxy` cannot revive it and why a cut voice still advances
    /// through its loop (libxmp `s3m_sample_porta.s3m`). The original's `@@cutnote`
    /// clears `_SampleNum` and stops the voice instead.
    fn cut_note(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.current_volume = 0;
        state.actual_volume = 0;
        state.note_cut = true;
        state.pending_dirty.insert(DirtyBits::VOLUME);
    }

    /// The unconditional channel stop: a slide that reaches period zero (D26).
    ///
    /// The instrument stays latched, so a later note with an empty instrument column
    /// still sounds (OpenMPT `PeriodLimit.s3m` plays `B-7` with no instrument after the
    /// slide has stopped the channel).
    fn stop_note(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.stop_voice = true;
        state.pending_dirty.remove(DirtyBits::PITCH);
    }

    fn static_effect(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: S3mCell, outcome: &mut TickOutcome) {
        // D25: every command with a non-zero parameter feeds the one shared memory, the
        // unimplemented ones included (OpenMPT `NOP.s3m`).
        self.write_parameter_memory(channel_index, cell.info);
        match cell.command {
            // Analysis §4, S_FX_A (2776). D6: canonical ST3 ignores A00.
            1 if cell.info != 0 => { outcome.speed = cell.info; self.clear_minor(channel_index); }
            // Analysis §4, S_FX_B (2782). Preserve Cxx's row when both share a row.
            2 => {
                self.flow.pattern_jump(cell.info as u16);
                self.clear_minor(channel_index);
            }
            // Analysis §4, S_FX_C (2792). The packed byte is decimal, not hexadecimal.
            3 => {
                self.flow.pattern_break((cell.info >> 4) as u16 * 10 + (cell.info & 15) as u16);
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
            9 => { let value = self.recall_parameter(channel_index, cell.info); self.set_minor(channel_index, 9, value); self.minor_tremor(channel_index); }
            // Analysis §4, S_FX_J (2956), including tick zero.
            10 => { self.channels[channel_index].arpeggio_count = 1; let value = self.recall_parameter(channel_index, cell.info); self.channels[channel_index].arpeggio_memory = value; self.set_minor(channel_index, 10, value); self.minor_arpeggio(channel_index); }
            // Analysis §4, S_FX_K/L (2973/2987).
            11 => {
                let value = self.recall_parameter(channel_index, cell.info);
                self.static_volume_slide(channel_index, cell.info);
                self.static_vibrato(channel_index, 8, 0);
                self.set_minor(channel_index, 11, value);
            }
            12 => {
                let value = self.recall_parameter(channel_index, cell.info);
                self.static_volume_slide(channel_index, cell.info);
                self.set_minor(channel_index, 12, value);
            }
            // Analysis §4, S_FX_O (3001), gated on a note being present.
            15 => { let value = if cell.info == 0 { self.channels[channel_index].offset_memory } else { cell.info }; self.channels[channel_index].offset_memory = value; self.channels[channel_index].sample_offset = (value as u32) << 8; if self.channels[channel_index].current_note != 0 { self.channels[channel_index].pending_dirty.insert(DirtyBits::SAMPLE); } self.clear_minor(channel_index); }
            // Analysis §4, S_FX_Q (3015).
            17 => self.static_retrigger(channel_index, cell.info),
            // Analysis §4, S_FX_S (3038).
            19 => { let value = self.recall_parameter(channel_index, cell.info); self.static_special(channel_index, value, context.position.row, outcome); }
            // Analysis §4, S_FX_T (3130).
            20 => {
                let value = self.recall_parameter(channel_index, cell.info);
                outcome.tempo_bpm = core::cmp::max(value as u16, 32);
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
        let value = self.recall_parameter(channel_index, parameter);
        if value > 0xF0 {
            self.slide_volume(channel_index, 0, value & 15);
            self.clear_minor(channel_index);
        } else if value & 15 == 15 && value >> 4 != 0 {
            self.slide_volume(channel_index, value >> 4, 0);
            self.clear_minor(channel_index);
        } else {
            self.set_minor(channel_index, 4, value);
        }
    }

    fn static_pitch_slide(&mut self, channel_index: usize, command: u8, parameter: u8) {
        let value = self.recall_parameter(channel_index, parameter);
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
        let value = self.recall_parameter(channel_index, parameter);
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
            //
            // D30: ST3 counts **down** from the parameter the first `SBx` stored, so a row
            // carrying several different `SBx` parameters consumes one iteration per
            // channel instead of restarting the count; when the loop ends the target
            // advances past the `SBx` row; and a loop jump wins over `Bxx`/`Cxx` on the
            // same row whichever channel they sit on. The original increments a counter
            // towards the parameter, never advances the target, and lets a later channel's
            // `Bxx` replace the loop jump.
            11 => {
                self.flow.pattern_loop(channel_index, row, value);
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
            // Analysis §4, S_FX_S SEx (3146): the whole row repeats.
            //
            // D32: only the **first** non-zero `SEx` on a row counts (OpenMPT
            // `PatternDelays.s3m`); the original lets the rightmost channel's value win
            // and lets `SE0` clear a delay an earlier channel already asked for.
            14 => { if value != 0 && outcome.pattern_delay == 0 { outcome.pattern_delay = value; } self.clear_minor(channel_index); }
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
        // D27: when both nibbles are set ST3 slides *down*; `M_FX_D` tests the high nibble
        // first and slides up instead (OpenMPT `ParamMemory.s3m` needs `D82` to be `D02`).
        let value = self.channels[channel_index].command_data;
        if value & 15 != 0 { self.slide_volume(channel_index, 0, value & 15); } else { self.slide_volume(channel_index, value >> 4, 0); }
    }

    fn slide_volume(&mut self, channel_index: usize, up: u8, down: u8) {
        let volume = if up != 0 { self.channels[channel_index].actual_volume.saturating_add(up).min(64) } else { self.channels[channel_index].actual_volume.saturating_sub(down) };
        self.channels[channel_index].current_volume = volume;
        self.channels[channel_index].actual_volume = volume;
        self.channels[channel_index].pending_dirty.insert(DirtyBits::VOLUME);
    }

    fn slide_period(&mut self, channel_index: usize, down: bool, amount: u32) {
        // D26: an upward slide is allowed to reach zero; `clip_pitch` stops the channel
        // there rather than pinning the period at one.
        let period = if down { self.channels[channel_index].current_period.saturating_add(amount) } else { self.channels[channel_index].current_period.saturating_sub(amount) };
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

    /// D35: the tremolo depth is `table * depth / 64`, the same scale ProTracker, libxmp
    /// and OpenMPT use. `M_FX_R` copies `M_FX_H`'s `sar eax,7` but leaves out its
    /// `sal edx,2` — the line is commented out in the original — which halves it. The
    /// division truncates towards zero rather than flooring, so a negative half-step
    /// rounds the same way the reference implementations round it.
    fn minor_tremolo(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        let sample = waveform_sample(state.tremolo_waveform, state.vibrato_phase) as i32;
        let delta = sample * (state.vibrato_memory & 15) as i32 / 64;
        state.actual_volume = (state.current_volume as i32 + delta).clamp(0, 64) as u8;
        state.pending_dirty.insert(DirtyBits::VOLUME);
        // D9: canonical modulo-64 phase, not M_FX_R's one-past-the-table `jbe` defect.
        state.vibrato_phase = state.vibrato_phase.wrapping_add(state.vibrato_memory >> 4) & 63;
    }

    /// D34: the on phase lasts exactly `x` ticks and the off phase exactly `y`, counting
    /// the tick the effect starts on. `M_FX_I` reloads the counter and *then* spends a
    /// whole tick on it, so each phase runs one tick long. A zero nibble counts as one.
    fn minor_tremor(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        if state.tremor_count == 0 {
            if state.tremor_on {
                state.tremor_on = false;
                state.tremor_count = (state.command_data & 15).max(1);
            } else {
                state.tremor_on = true;
                state.tremor_count = (state.command_data >> 4).max(1);
            }
        }
        state.tremor_count -= 1;
        // D3: read this channel's current_volume, not the undefined EDI register.
        state.actual_volume = if state.tremor_on { state.current_volume } else { 0 };
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
        if state.special_value != 0 || state.current_note == 0 || state.note_cut { return; }
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
        if self.channels[channel_index].current_period == 0 {
            // D26: a slide that reaches period zero stops the channel outright.
            self.stop_note(channel_index);
            return;
        }
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
        if self.amiga_limits {
            // D31: ST3 clamps the channel period itself, so a slide that runs into the
            // limit resumes from it. `ClipPitch` clamps only `_ActualPeriod`, which lets
            // `_CurrentPeriod` keep running away underneath.
            state.current_period = state.current_period.clamp(452, 3424);
            state.actual_period = state.actual_period.clamp(452, 3424);
        }
        // D26: ST3's lower output-period limit. The unclamped `current_period` keeps
        // sliding underneath it, so a later downward slide resumes from the true value.
        state.actual_period = state.actual_period.max(MIN_OUTPUT_PERIOD);
    }

    fn flush_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let channel_id = ChannelId(channel_index as u16);
        if self.channels[channel_index].stop_voice {
            self.channels[channel_index].stop_voice = false;
            self.channels[channel_index].pending_dirty = DirtyBits::empty();
            context.stop_channel(channel_id);
            return;
        }
        let dirty = self.channels[channel_index].pending_dirty;
        if dirty.is_empty() { return; }

        if dirty.contains(DirtyBits::SAMPLE) {
            if self.channels[channel_index].sample_number == NO_SAMPLE {
                context.stop_channel(channel_id);
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
                let sample_number = sample_id.0.saturating_add(1).min(u8::MAX as u16) as u8;
                let tag = VoiceTag { channel: channel_index as u8, instrument: instrument_number, sample: sample_number, note: linear_note(self.channels[channel_index].current_note) };
                context.trigger_channel(channel_id, tag, region, params, self.channels[channel_index].sample_offset);
            }
        } else if let Some(voice_id) = context.channels.foreground(channel_id)
        {
            let state = &self.channels[channel_index];
            if dirty.contains(DirtyBits::PITCH) { context.write_voice_param(voice_id, starplayer_core::VoiceParam::Step(step_from_period(state.actual_period, self.sample_rate_hz))); }
            if dirty.contains(DirtyBits::VOLUME) { context.write_voice_param(voice_id, starplayer_core::VoiceParam::Volume(scaled_volume(state.actual_volume, self.global_volume))); }
            if dirty.contains(DirtyBits::PAN) { context.write_voice_param(voice_id, starplayer_core::VoiceParam::Pan(pan_nibble_to_bipolar(state.pan_position))); }
            if dirty.contains(DirtyBits::TEMPO) { context.mark_voice_dirty(voice_id, DirtyBits::TEMPO); }
        }
        self.channels[channel_index].pending_dirty = DirtyBits::empty();
    }

    /// Feed the diagnostic per-tick trace.
    ///
    /// Gated, not merely a no-op call: the loop walks every channel and resolves each
    /// one's model sample through an instrument lookup, and that work would otherwise run
    /// inside `render()` on every tracker tick of a shipping build.
    #[cfg(feature = "trace")]
    fn report_trace_channels(&self, context: &mut TickContext<'_>) {
        for (channel_index, state) in self.channels.iter().enumerate() {
            context.report_trace_channel(ChannelId(channel_index as u16), self.trace_channel_state(state));
        }
    }

    #[cfg(any(feature = "trace", test))]
    fn trace_channel_state(&self, state: &S3mChannel) -> TraceChannelState {
        let note = (state.current_note != 0).then_some(linear_note(state.current_note));
        let instrument = if state.sample_number == NO_SAMPLE { 0 } else { state.sample_number as u16 };
        let sample = instrument
            .checked_sub(1)
            .and_then(|instrument| self.module.instrument(InstrumentId(instrument)))
            .and_then(|instrument| instrument.sample)
            .map(|sample| sample.0.saturating_add(1))
            .unwrap_or(0);
        TraceChannelState {
            note,
            instrument,
            sample,
            volume: state.actual_volume as u16,
            period: state.actual_period,
            pan: state.pan_position as u16 * 17,
        }
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

    /// D25: ST3 keeps **one** parameter memory per channel. A non-zero parameter on any
    /// command writes it; a zero parameter on `D`, `E`, `F`, `I`, `J`, `K`, `L`, `Q`, `S`
    /// and `T` reads it back, so `H82` on one row makes the next row's `D00` behave as
    /// `D02` (OpenMPT `ParamMemory.s3m`). `G` and the `H`/`R`/`U` family keep their own
    /// read memories and only write this one.
    fn recall_parameter(&mut self, channel_index: usize, parameter: u8) -> u8 {
        if parameter == 0 { return self.channels[channel_index].parameter_memory; }
        self.channels[channel_index].parameter_memory = parameter;
        parameter
    }

    fn write_parameter_memory(&mut self, channel_index: usize, value: u8) {
        if value != 0 { self.channels[channel_index].parameter_memory = value; }
    }
}

impl TrackerProcessor for S3mProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, 64));
        let mut outcome = context.outcome();
        // D30: every position change resets the one global loop target and counter
        // (OpenMPT `LoopReset.s3m`). ModPlug 1.16 and Imago Orpheus differ, so the rule
        // lives in the dialect's `PatternFlow` rather than here.
        if self.last_order.is_some_and(|position| position != (row.order, row.pattern)) {
            self.flow.position_changed();
        }
        self.last_order = Some((row.order, row.pattern));
        self.flow.begin_row();
        self.reset_row();
        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        for channel_index in 0..channel_count {
            let start = channel_index * CELL_BYTES;
            let Some(cell) = S3mCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])) else { continue };
            self.latch_cell(context, channel_index, cell);
            self.static_effect(context, channel_index, cell, &mut outcome);
            self.clip_pitch(channel_index);
        }
        outcome.jump = self.flow.jump();
        for channel_index in 0..self.channels.len() { self.flush_channel(context, channel_index); }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }

    /// Restore the state a fresh processor would have. Nothing here allocates: the
    /// channel array keeps its box and each entry is overwritten in place.
    fn reset(&mut self) {
        let header = self.module.header();
        self.global_volume = ((header.global_volume.to_bits() as u32 * 64 + 32767) / 65535) as u8;
        for channel_index in 0..self.channels.len() {
            let pan = header.default_pan.get(channel_index).copied().map(pan_to_nibble).unwrap_or(crate::header::PAN_CENTRE);
            self.channels[channel_index] = S3mChannel::new(channel_index as u8, pan);
        }
        self.flow.reset();
        self.last_order = None;
    }

    /// D33: every `SEx` repeat of a row is another first tick in ST3, so the row's
    /// tick-zero effects run again — without re-latching its notes, which keep sounding
    /// from the first pass (OpenMPT `PatternDelaysRetrig.s3m`). `__UpdateTracker`
    /// decrements `_MRowDelay` and skips the row entirely instead.
    fn row_repeat(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, 64));
        let mut outcome = context.outcome();
        self.flow.begin_row();
        self.reset_row();
        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        for channel_index in 0..channel_count {
            let start = channel_index * CELL_BYTES;
            let Some(cell) = S3mCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])) else { continue };
            self.static_effect(context, channel_index, cell, &mut outcome);
            self.clip_pitch(channel_index);
        }
        outcome.jump = self.flow.jump();
        for channel_index in 0..self.channels.len() { self.flush_channel(context, channel_index); }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, 64));
        let outcome = context.outcome();
        for channel_index in 0..self.channels.len() {
            self.minor_effect(channel_index);
            self.clip_pitch(channel_index);
            self.flush_channel(context, channel_index);
        }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }
}

/// Build the public S3M sequencer with header speed, tempo, global volume and pan state.
///
/// The quirks come from the [`FormatDialect`](starplayer_core::quirks::FormatDialect) the
/// loader stored in the module header; the tempo model is the caller's.
/// [`sequencer_with_quirks`] is the form that takes both from one [`QuirkSelection`].
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, S3mProcessor, S3mPatternData> {
    let settings = sequencer_settings(&module, sample_rate_hz);
    PatternSequencer::new(tempo_model, S3mPatternData(Arc::clone(&module)), S3mProcessor::new(module, sample_rate_hz), settings)
}

/// Build the public S3M sequencer under an explicit [`QuirkSelection`].
///
/// The selection is resolved once, against the loader-detected dialect, and supplies both
/// the effect processor's quirks and the tempo model — so a host that asks for
/// `QuirkSet::starplayer_classic()` gets the original's truncating tick length as well as
/// its effect behaviour, from one argument.
pub fn sequencer_with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> PatternSequencer<TempoModelId, S3mProcessor, S3mPatternData> {
    let resolved = quirks.resolve(module.header().dialect);
    let settings = sequencer_settings(&module, sample_rate_hz);
    let processor = S3mProcessor::with_quirks(Arc::clone(&module), sample_rate_hz, QuirkSelection::Override(resolved));
    PatternSequencer::new(resolved.tempo_model, S3mPatternData(module), processor, settings)
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
    use starplayer_core::quirks::S3mLoopDialect;
    use starplayer_core::{Frame, RowClock};
    use starplayer_engine::{ChannelTable, Jump, SongPosition};
    use starplayer_mixer::VoicePool;
    use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};

    fn processor() -> S3mProcessor { S3mProcessor::new(one_channel_module(), 44_100) }

    fn one_channel_module() -> Arc<Module> {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![255u8; ROWS as usize * CELL_BYTES], ROWS, 1).expect("one fixed-stride pattern");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        Arc::new(builder.build().expect("valid module"))
    }

    fn with_context(test: impl FnOnce(&mut S3mProcessor, &mut TickContext<'_>)) {
        let mut processor = processor();
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        test(&mut processor, &mut context);
    }

    #[test]
    fn trace_resolves_sample_number_independently_of_instrument_slot() {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[0], SampleSpec::one_shot("pcm")).expect("one PCM sample");
        builder.add_instrument(InstrumentDef::default()).expect("empty instrument slot zero");
        builder.add_instrument(InstrumentDef::from_sample("pcm", sample, U0F16::MAX)).expect("PCM instrument slot one");
        builder.add_pattern(&vec![255u8; ROWS as usize * CELL_BYTES], ROWS, 1).expect("one fixed-stride pattern");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let mut processor = S3mProcessor::new(Arc::new(builder.build().expect("valid module")), 44_100);
        processor.channels[0].sample_number = 2;

        let state = processor.trace_channel_state(&processor.channels[0]);
        assert_eq!(state.instrument, 2, "trace keeps the one-based S3M instrument slot");
        assert_eq!(state.sample, 1, "trace reports the mapped model sample in the one-based domain");

        processor.channels[0].sample_number = 1;
        let empty_state = processor.trace_channel_state(&processor.channels[0]);
        assert_eq!(empty_state.sample, 0, "an empty instrument has no trace sample");

        processor.channels[0].sample_number = 2;
        processor.channels[0].current_note = 0x40;
        processor.channels[0].pending_dirty = DirtyBits::SAMPLE;
        let mut voices = VoicePool::new(1);
        let mut channels = ChannelTable::new(1);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        processor.flush_channel(&mut context, 0);
        let voice = context.channels.foreground(ChannelId(0)).and_then(|voice| context.voices.get(voice)).expect("triggered PCM voice");
        assert_eq!(voice.tag.sample, 1, "fallback voice metadata uses the same one-based sample number");
    }

    #[test]
    fn dxy_classification_order_and_normal_down_nibble_priority_match_st3() {
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
        processor.channels[0].current_volume = 32;
        processor.channels[0].actual_volume = 32;
        processor.static_volume_slide(0, 0x82);
        processor.minor_volume_slide(0);
        assert_eq!(processor.channels[0].actual_volume, 30, "D27: with both nibbles set the down nibble wins, so D82 is D02");
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
        assert_eq!(processor.channels[0].parameter_memory, 5);
        assert_eq!(processor.channels[0].actual_period, 120, "E00 uses the 5 remembered by D05 and multiplies it by four");
    }

    #[test]
    fn c10_is_decimal_row_ten_and_a00_is_ignored() {
        with_context(|processor, context| {
            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 3, info: 0x10, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::break_to_row(10)));
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
            assert_eq!(processor.pattern_flow().start(0), 7);
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
    fn a_loop_jump_cancels_a_break_or_jump_on_the_same_row_in_either_channel_order() {
        with_context(|processor, context| {
            // D30: `SBx` first, then `Bxx` on a later channel — the jump is blocked.
            let mut outcome = context.outcome();
            processor.static_special(0, 0xB0, 4, &mut outcome);
            processor.static_special(0, 0xB2, 7, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::within_pattern_to_row(4)));
            processor.static_effect(context, 0, S3mCell { command: 2, info: 3, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::within_pattern_to_row(4)), "D30: a loop jump blocks a Bxx that follows it on the row");

            // `Cxx` first, then `SBx` — the break is cancelled.
            processor.flow.begin_row();
            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 3, info: 0x12, ..S3mCell::EMPTY }, &mut outcome);
            processor.static_special(0, 0xB1, 7, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::within_pattern_to_row(4)), "D30: a loop jump cancels a Cxx already seen on the row");

            // The iteration that ends the loop lets the break through again.
            processor.flow.begin_row();
            let mut outcome = context.outcome();
            processor.static_special(0, 0xB1, 7, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), None, "the final SBx iteration produces no jump");
            assert_eq!(processor.pattern_flow().start(0), 8, "D30: the loop target advances past the SBx row when the loop ends");
            processor.static_effect(context, 0, S3mCell { command: 2, info: 3, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::to_order_row(3, 0)), "a Bxx on the terminating row is honoured");
        });
    }

    #[test]
    fn one_row_of_several_sbx_parameters_counts_down_once_per_channel() {
        with_context(|processor, context| {
            // D30: ST3 stores the first parameter and every later `SBx` on the row spends
            // one iteration of it, rather than each one restarting its own count.
            let mut outcome = context.outcome();
            processor.static_special(0, 0xB4, 13, &mut outcome);
            assert_eq!(processor.pattern_flow().count(0), 4, "the first SBx stores its own parameter");
            for expected in [3, 2, 1] {
                processor.static_special(0, 0xB1, 13, &mut outcome);
                assert_eq!(processor.pattern_flow().count(0), expected, "each further SBx on the row spends one iteration");
            }
            assert_eq!(processor.pattern_flow().jump(), Some(Jump::within_pattern_to_row(0)));
            let _ = context;
        });
    }

    /// The dialect fields are per module: the same processor code plays a ModPlug 1.16 or
    /// Imago Orpheus S3M by a different set of flow rules, chosen once at construction.
    #[test]
    fn the_s3m_loop_dialect_changes_the_flow_rules_the_processor_runs() {
        for (dialect, expected) in [
            (S3mLoopDialect::ScreamTracker321, true),
            (S3mLoopDialect::ScreamTracker301, false),
            (S3mLoopDialect::ModPlug116, true),
            (S3mLoopDialect::ImagoOrpheus, false),
        ] {
            let quirks = QuirkSet { s3m_pattern_loop: dialect, ..QuirkSet::canonical() };
            let module = one_channel_module();
            let processor = S3mProcessor::with_quirks(module, 44_100, QuirkSelection::Override(quirks));
            assert_eq!(processor.quirks().s3m_pattern_loop, dialect);
            assert_eq!(processor.pattern_flow().flow().delay_jump, expected, "{dialect:?} blocks a later Bxx");
        }
    }

    #[test]
    fn every_effect_family_recalls_the_one_shared_parameter_memory() {
        // D25: `H82` seeds the shared memory, and each following zero-parameter command
        // behaves as if it had been written with `82` (OpenMPT `ParamMemory.s3m`).
        for (command, info, expected) in [
            (4u8, 0u8, 0x82u8),  // Dxy volume slide
            (5, 0, 0x82),        // Exx portamento down
            (6, 0, 0x82),        // Fxx portamento up
            (9, 0, 0x82),        // Ixy tremor
            (10, 0, 0x82),       // Jxy arpeggio
            (17, 0, 0x82),       // Qxy retrigger
            (19, 0, 0x82),       // Sxy special
        ] {
            with_context(|processor, context| {
                let mut outcome = context.outcome();
                processor.static_effect(context, 0, S3mCell { command: 8, info: 0x82, ..S3mCell::EMPTY }, &mut outcome);
                assert_eq!(processor.channels[0].parameter_memory, 0x82, "H82 writes the shared memory");
                processor.static_effect(context, 0, S3mCell { command, info, ..S3mCell::EMPTY }, &mut outcome);
                assert_eq!(processor.channels[0].parameter_memory, expected, "command {command} with a zero parameter leaves the shared memory alone");
                let used = match command {
                    // D and Q store the effective parameter in the minor slot.
                    4 | 9 | 10 | 17 => processor.channels[0].command_data,
                    5 | 6 => processor.channels[0].command_data,
                    _ => processor.channels[0].parameter_memory,
                };
                assert_eq!(used, expected, "command {command} runs with the recalled parameter");
            });
        }
    }

    #[test]
    fn an_unimplemented_command_with_a_parameter_still_feeds_the_shared_memory() {
        // D25: OpenMPT `NOP.s3m` — a no-op effect cell contributes its parameter.
        with_context(|processor, context| {
            let mut outcome = context.outcome();
            processor.static_effect(context, 0, S3mCell { command: 26, info: 0x37, ..S3mCell::EMPTY }, &mut outcome);
            processor.static_effect(context, 0, S3mCell { command: 10, info: 0, ..S3mCell::EMPTY }, &mut outcome);
            assert_eq!(processor.channels[0].arpeggio_memory, 0x37, "J00 arpeggiates with the no-op command's parameter");
        });
    }

    #[test]
    fn the_output_period_clamps_at_sixty_four_and_period_zero_stops_the_channel() {
        // D26: OpenMPT `PeriodLimit.s3m`.
        let mut processor = processor();
        processor.channels[0].current_period = 65;
        processor.channels[0].actual_period = 65;
        processor.channels[0].pending_dirty = DirtyBits::PITCH;
        processor.clip_pitch(0);
        assert_eq!(processor.channels[0].actual_period, 65, "a period just above the limit is untouched");
        assert!(!processor.channels[0].stop_voice);

        processor.channels[0].current_period = 48;
        processor.channels[0].actual_period = 48;
        processor.channels[0].pending_dirty = DirtyBits::PITCH;
        processor.clip_pitch(0);
        assert_eq!(processor.channels[0].actual_period, 64, "a period below the limit is clamped to 64");
        assert_eq!(processor.channels[0].current_period, 48, "the channel period keeps its true value underneath the clamp");
        assert!(!processor.channels[0].stop_voice, "clamping does not stop the channel");

        processor.channels[0].command = 6;
        processor.channels[0].command_data = 12;
        processor.minor_effect(0);
        assert_eq!(processor.channels[0].current_period, 0, "F0C takes the remaining 48 units to exactly zero");
        processor.clip_pitch(0);
        assert!(processor.channels[0].stop_voice, "D26: period zero stops the channel");
        assert_eq!(processor.channels[0].sample_number, NO_SAMPLE, "the synthetic channel had no instrument to keep");
    }

    #[test]
    fn amiga_limits_clamp_the_channel_period_and_the_step_it_produces() {
        // D31: OpenMPT `AmigaLimits.s3m` — 113 * 4 and 856 * 4, applied to the state.
        let mut processor = processor();
        processor.amiga_limits = true;
        for (start, slide_down, expected) in [(453u32, false, 452u32), (3420, true, 3424)] {
            processor.channels[0].current_period = start;
            processor.channels[0].actual_period = start;
            processor.channels[0].command = if slide_down { 5 } else { 6 };
            processor.channels[0].command_data = 16;
            processor.minor_effect(0);
            processor.clip_pitch(0);
            assert_eq!(processor.channels[0].actual_period, expected, "the output period stops at the Amiga bound");
            assert_eq!(processor.channels[0].current_period, expected, "D31: the channel period stops there too, so the slide back out starts from the bound");
        }
        assert_eq!(step_from_period(452, 44_100), Step::from_ratio((ST3_FREQUENCY_NUMERATOR / 452) as u64, 44_100));
        assert_eq!(step_from_period(3424, 44_100), Step::from_ratio((ST3_FREQUENCY_NUMERATOR / 3424) as u64, 44_100));
    }

    #[test]
    fn tone_portamento_with_a_new_instrument_keeps_the_sample_and_takes_the_volume() {
        // D28: OpenMPT `PortaSmpChange.s3m`.
        let mut builder = ModuleBuilder::new();
        let loud = builder.add_sample(&[0, 0, 0, 0], SampleSpec::one_shot("loud")).expect("first sample");
        let mut quiet_spec = SampleSpec::one_shot("quiet");
        quiet_spec.default_volume = U0F16::from_bits(16384);
        let quiet = builder.add_sample(&[0, 0, 0, 0], quiet_spec).expect("second sample");
        builder.add_instrument(InstrumentDef::from_sample("loud", loud, U0F16::MAX)).expect("instrument one");
        builder.add_instrument(InstrumentDef::from_sample("quiet", quiet, U0F16::MAX)).expect("instrument two");
        builder.add_pattern(&vec![255u8; ROWS as usize * CELL_BYTES], ROWS, 1).expect("one fixed-stride pattern");
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let mut processor = S3mProcessor::new(Arc::new(builder.build().expect("valid module")), 44_100);

        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        processor.latch_cell(&mut context, 0, S3mCell { note: 0x40, instrument: 1, volume: VOLUME_NONE, command: 0, info: 0 });
        processor.flush_channel(&mut context, 0);
        assert_eq!(processor.channels[0].sample_number, 1);
        let sounding = context.channels.foreground(ChannelId(0)).expect("the first note sounds");

        processor.latch_cell(&mut context, 0, S3mCell { note: 0x44, instrument: 2, volume: VOLUME_NONE, command: 7, info: 2 });
        assert_eq!(processor.channels[0].sample_number, 1, "D28: the sounding instrument is kept");
        assert_eq!(processor.channels[0].current_volume, 16, "D28: the new instrument's default volume is adopted");
        assert_eq!(processor.channels[0].reference_rate_hz, DEFAULT_REFERENCE_RATE_HZ, "the sounding sample's C2SPD is kept");
        processor.flush_channel(&mut context, 0);
        assert_eq!(context.channels.foreground(ChannelId(0)), Some(sounding), "the voice is not retriggered");
    }

    #[test]
    fn a_note_cut_silences_the_channel_without_stopping_its_voice() {
        // D29: libxmp `s3m_sample_porta.s3m` keeps the cut voice looping at volume zero.
        let mut processor = processor();
        processor.channels[0].sample_number = 1;
        processor.channels[0].current_note = 0x40;
        processor.channels[0].current_volume = 64;
        processor.channels[0].actual_volume = 64;
        processor.channels[0].current_period = 1712;
        processor.cut_note(0);
        assert_eq!(processor.channels[0].actual_volume, 0, "the cut takes the volume to zero");
        assert_eq!(processor.channels[0].sample_number, 1, "D29: the sample keeps playing");
        assert_eq!(processor.channels[0].current_period, 1712, "the pitch is untouched");
        assert!(!processor.channels[0].pending_dirty.contains(DirtyBits::SAMPLE), "no retrigger and no stop");

        processor.channels[0].retrigger_memory = 0x01;
        processor.channels[0].special_value = 1;
        processor.minor_retrigger(0);
        assert_eq!(processor.channels[0].actual_volume, 0, "D29: Qxy does not revive a cut channel");
    }

    #[test]
    fn the_tremor_phases_last_exactly_x_and_y_ticks() {
        // D34: OpenMPT `ParamMemory.s3m` row 23 — I82 is eight ticks on and two off,
        // counting the tick the effect starts on.
        let mut processor = processor();
        processor.channels[0].current_volume = 64;
        processor.channels[0].command_data = 0x82;
        let mut volumes = alloc::vec::Vec::new();
        for _ in 0..12 {
            processor.minor_tremor(0);
            volumes.push(processor.channels[0].actual_volume);
        }
        assert_eq!(volumes, alloc::vec![64, 64, 64, 64, 64, 64, 64, 64, 0, 0, 64, 64], "eight ticks loud, two silent, then loud again");
    }

    #[test]
    fn the_tremolo_depth_is_the_protracker_scale_and_not_half_of_it() {
        // D35: `table * depth / 64`, truncating towards zero like every reference does.
        let mut processor = processor();
        processor.channels[0].current_volume = 64;
        processor.channels[0].vibrato_memory = 0x82;
        processor.channels[0].vibrato_phase = 40;
        processor.minor_tremolo(0);
        assert_eq!(processor.channels[0].actual_volume, 59, "phase 40 of the sine is -180, so depth 2 removes five");
        processor.channels[0].current_volume = 32;
        processor.channels[0].vibrato_phase = 16;
        processor.minor_tremolo(0);
        assert_eq!(processor.channels[0].actual_volume, 39, "phase 16 is +255, so depth 2 adds seven");
    }

    #[test]
    fn the_default_pan_nibble_of_a_centred_channel_is_eight() {
        // D24: ST3's own centre, not the original's 7.
        let processor = processor();
        assert_eq!(processor.channels[0].pan_position, crate::header::PAN_CENTRE);
        assert_eq!(crate::header::PAN_CENTRE, 8);
        assert_eq!(processor.trace_channel_state(&processor.channels[0]).pan, 136);
    }
}

//! FastTracker 2's tick-zero and per-tick effect processor, its instrument articulation,
//! and the volume column.
//!
//! # The reference
//!
//! Not the original DOS StarPlayer — it never supported XM. The specification here is
//! **FastTracker 2 itself**, read through `8bitbubsy/ft2-clone`'s `src/ft2_replayer.c`,
//! which is a cycle-faithful C port of FT2's replayer. Every function below names the
//! `ft2_replayer.c` routine it ports, and every `channel_t` field it mirrors is named in
//! the doc comment of the [`XmChannel`] field that holds it, exactly as `S3mChannel` names
//! the original assembly's `ChannelData` fields.
//!
//! Where libxmp (the conformance oracle) and FT2 disagree, FT2 wins and the difference is
//! recorded in `plans/product/03-accuracy-policy.md`.
//!
//! # Per-voice state is per-channel state
//!
//! Architecture §5.3 as amended by task E3: there is no engine envelope runner and no
//! `Instrument` trait. XM has exactly one voice per channel and never detaches one, so the
//! envelope positions, the fadeout level, the key-off flag and the auto-vibrato phase all
//! live in [`XmChannel`] beside the effect memories, and [`XmProcessor::tick`] advances
//! them itself — `updateVolPanAutoVib` in `ft2_replayer.c`.
//!
//! Envelope output is written **straight into the voice's
//! [`VoiceParams`]**, not through [`TickContext::write_voice_param`], so the diagnostic
//! trace's per-tick dirty flags stay effect-driven rather than reporting a write on every
//! single tick of every envelope.
//!
//! # FT2's tick counter
//!
//! FT2 counts `song.tick` **down** from `speed` to 1 and calls the tick with
//! `song.tick == speed` the first tick of the row. Every effect that keys off a tick index
//! — `EDx` note delay, `ECx` note cut, `E9x` retrigger, `Kxx` — spells it
//! `song.speed - song.tick`, which is the tick index counting up **within the current
//! repeat**, not [`RowClock::tick_in_row`], which is absolute across pattern-delay
//! repeats. [`XmProcessor::tick`] recovers FT2's counter as
//! `tick_in_row % speed`, so `EDx` fires once per `EEx` repeat, exactly as FT2 does
//! (`DelayCombination.xm`).

// Every channel index originates from a loop bounded by `channels.len()` and every table
// index is masked or bounds-checked at the point it is formed. Keeping that invariant at
// the dispatch boundary lets the individual handlers read as the C they port.
#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::quirks::{PatternFlow, QuirkSelection, QuirkSet};
use starplayer_core::tables::linear_frequency_q24;
use starplayer_core::{ChannelId, DirtyBits, Frame, I1F15, InstrumentId, Note, SampleId, Step, TempoModel, TempoModelId, U0F16, VoiceParams};
use starplayer_engine::{EndOfSongPolicy, PatternFlowState, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TrackerProcessor};
#[cfg(any(feature = "trace", test))]
use starplayer_engine::TraceChannelState;
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{AutoVibratoWaveform, EffectNames, Envelope, EnvelopePoint, InstrumentDef, LoopMode, Module};
use starplayer_rt::Arc;

use crate::data::XmPatternData;
use crate::header::XmFormatExtra;
use crate::pattern::{CELL_BYTES, NOTE_KEY_OFF, XmCell};
use crate::tables::{amiga_period, linear_period, ARPEGGIO_TICK_TABLE, AUTO_VIBRATO_SINE_TABLE, PERIOD_TABLE_LEN, VIBRATO_TABLE};

/// Highest instrument number FastTracker 2 will latch. A larger number is read as "no
/// instrument at all", which is `getNewNote`'s `inst = 0`.
const MAX_INSTRUMENT: u8 = 128;

/// FastTracker 2's fadeout domain. The XM specification says 0..65536; the replayer uses
/// half of it (`triggerInstrument`: "final fadeout range is in fact 0..32768").
const FADEOUT_MAX: u16 = 32_768;

/// The envelope value domain FT2 interpolates in: the point's `y` shifted left by eight.
const ENVELOPE_UNITY: i32 = 64 * 256;

/// The reference pitch of C-4, and the numerator of the Amiga-mode frequency.
const REFERENCE_RATE_HZ: u64 = 8363;

/// FastTracker 2's own Amiga period for C-4. `hz = 8363 * 1712 / period` in Amiga mode.
const AMIGA_C4_PERIOD: u64 = 1712;

/// `8363 << 16`: [`linear_frequency_q24`] answers `2^(units/768 - 14)` in Q8.24 where FT2
/// wants `8363 * 2^((4608 - period)/768)`, a factor of `2^8` apart, and the Q32.32 step
/// wants a further `2^32 / 2^24 == 2^8`. Multiplying the table entry by this and dividing
/// by the output rate is therefore the whole conversion, with no rounding step of its own.
const LINEAR_FREQUENCY_NUMERATOR: u64 = REFERENCE_RATE_HZ << 16;

/// `12 * 192 * 4`, the constant FT2's `period2Ft2Delta` subtracts a linear period from.
const LINEAR_INVERSE_BASE: u32 = 12 * 192 * 4;

/// The period FT2's downward slides clamp at — and it really is a signed comparison, so a
/// period that reaches 32000 is pinned at 31999 rather than wrapping.
const MAX_PERIOD: u16 = 32_000;

/// One channel's replay state.
///
/// The Rust name comes first and the `ft2_replayer.c` `channel_t` field it mirrors follows
/// it, the way [`S3mChannel`](starplayer_s3m::S3mChannel) names the original assembly's
/// `ChannelData` fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmChannel {
    // ── what is playing ─────────────────────────────────────────────────────────────
    /// `noteNum` — the pattern's own note byte, 1..96, of the last note triggered.
    pub note_number: u8,
    /// `instrNum` — the one-based instrument number last latched; 0 is none.
    pub instrument_number: u8,
    /// `instrPtr` — the instrument that number resolves to, or `None` for FT2's empty
    /// placeholder instrument (volume 0, pan 128, no envelopes, no fadeout).
    pub instrument: Option<InstrumentId>,
    /// `smpPtr` — the sample the note→sample map chose, if the instrument had one.
    pub sample: Option<SampleId>,
    /// `relativeNote` — the sounding sample's transpose, latched at note-on and **kept**
    /// across an instrument column with no note (`kFT2PortaIgnoreInstr`).
    pub relative_note: i8,
    /// `finetune` — likewise, and overwritten by `E5x` only next to a real note.
    pub finetune: i8,

    // ── pitch ───────────────────────────────────────────────────────────────────────
    /// `realPeriod` — the period the slides own.
    pub real_period: u16,
    /// `outPeriod` — `realPeriod` plus this tick's vibrato or arpeggio.
    pub out_period: u16,
    /// `finalPeriod` — `outPeriod` plus auto-vibrato; what the mixer plays.
    pub final_period: u16,
    /// `portamentoTargetPeriod`.
    pub portamento_target_period: u16,
    /// `portamentoSpeed` — already multiplied by four, as FT2 stores it.
    pub portamento_speed: u16,
    /// `portamentoDirection` — 0 arrived, 1 upwards in period, 2 downwards.
    pub portamento_direction: u8,
    /// `semitonePortaMode` — `E3x` glissando.
    pub glissando: bool,

    // ── volume and pan ──────────────────────────────────────────────────────────────
    /// `oldVol` — the sounding sample's default volume, what `resetVolumes` restores.
    pub sample_volume: u8,
    /// `oldPan` — the sounding sample's default pan.
    pub sample_pan: u8,
    /// `realVol` — the channel volume the slides own, 0..64.
    pub real_volume: u8,
    /// `outVol` — `realVol` plus this tick's tremolo or tremor.
    pub out_volume: u8,
    /// `outPan` — the channel pan, 0..255.
    pub out_pan: u8,
    /// `finalPan` — `outPan` plus the panning envelope.
    pub final_pan: u8,

    // ── the row's columns ───────────────────────────────────────────────────────────
    /// `efx` — the effect column, 0..=0x21.
    pub effect: u8,
    /// `efxData` — its parameter.
    pub effect_data: u8,
    /// `volColumnVol` — the volume column byte, in FastTracker 2's own encoding.
    pub volume_column: u8,
    /// `copyOfInstrAndNote` — what `EDx` replays when its tick comes round.
    pub delayed_instrument_and_note: u16,

    // ── the LFOs ────────────────────────────────────────────────────────────────────
    /// `vibratoPos`.
    pub vibrato_position: u8,
    /// `tremoloPos`.
    pub tremolo_position: u8,
    /// `vibratoSpeed`, already multiplied by four as `4xy` stores it.
    pub vibrato_speed: u8,
    /// `vibratoDepth`.
    pub vibrato_depth: u8,
    /// `tremoloSpeed`, likewise.
    pub tremolo_speed: u8,
    /// `tremoloDepth`.
    pub tremolo_depth: u8,
    /// `vibTremCtrl` — `E4x` in the low nibble, `E7x` in the high one.
    pub waveform_control: u8,

    // ── counters ────────────────────────────────────────────────────────────────────
    /// `tremorParam` and `tremorPos` — the high bit of the position is the on/off phase.
    pub tremor_parameter: u8,
    /// `tremorPos`.
    pub tremor_position: u8,
    /// `noteRetrigSpeed`, `noteRetrigCounter`, `noteRetrigVol` — `Rxy`'s three.
    pub retrigger_speed: u8,
    /// `noteRetrigCounter`.
    pub retrigger_counter: u8,
    /// `noteRetrigVol`.
    pub retrigger_volume: u8,

    // ── effect memories, one per family as FT2 keeps them ───────────────────────────
    /// `volSlideSpeed` — `Axy`, shared with `5xy` and `6xy`.
    pub volume_slide_memory: u8,
    /// `fVolSlideUpSpeed` — `EAx`.
    pub fine_volume_up_memory: u8,
    /// `fVolSlideDownSpeed` — `EBx`.
    pub fine_volume_down_memory: u8,
    /// `pitchSlideUpSpeed` — `1xx`.
    pub pitch_up_memory: u8,
    /// `pitchSlideDownSpeed` — `2xx`.
    pub pitch_down_memory: u8,
    /// `fPitchSlideUpSpeed` — `E1x`.
    pub fine_pitch_up_memory: u8,
    /// `fPitchSlideDownSpeed` — `E2x`.
    pub fine_pitch_down_memory: u8,
    /// `efPitchSlideUpSpeed` — `X1x`.
    pub extra_fine_pitch_up_memory: u8,
    /// `efPitchSlideDownSpeed` — `X2x`.
    pub extra_fine_pitch_down_memory: u8,
    /// `globVolSlideSpeed` — `Hxy`.
    pub global_volume_slide_memory: u8,
    /// `panningSlideSpeed` — `Pxy`.
    pub pan_slide_memory: u8,
    /// `sampleOffset` — `9xx`'s memory, in 256-frame units.
    pub offset_memory: u8,
    /// `smpStartPos` — the frame the next trigger starts at.
    pub sample_start_frame: u32,

    // ── articulation ────────────────────────────────────────────────────────────────
    /// `keyOff`.
    pub key_off: bool,
    /// `fadeoutVol`, 0..[`FADEOUT_MAX`].
    pub fadeout_volume: u16,
    /// `fadeoutSpeed` — the instrument's `volFade`, latched at note-on.
    pub fadeout_speed: u16,
    /// `volEnvTick` — the envelope's own clock, which starts at `65535` so that the first
    /// increment lands on zero.
    pub volume_envelope_tick: u16,
    /// `volEnvPos`.
    pub volume_envelope_point: u8,
    /// `volEnvValue` — the interpolated value, in the point's `y` shifted left by eight.
    pub volume_envelope_value: i16,
    /// `volEnvDelta` — the per-tick increment in the same domain.
    pub volume_envelope_delta: i16,
    /// `panEnvTick`.
    pub panning_envelope_tick: u16,
    /// `panEnvPos`.
    pub panning_envelope_point: u8,
    /// `panEnvValue`.
    pub panning_envelope_value: i16,
    /// `panEnvDelta`.
    pub panning_envelope_delta: i16,
    /// `autoVibPos`.
    pub auto_vibrato_position: u8,
    /// `autoVibAmp` — the swept depth, in the instrument's depth shifted left by eight.
    pub auto_vibrato_amplitude: u16,
    /// `autoVibSweep` — the per-tick sweep increment; zero once the sweep is over.
    pub auto_vibrato_sweep: u16,

    // ── what the flush has to do ────────────────────────────────────────────────────
    /// `CS_TRIGGER_VOICE` — start the sample from [`XmChannel::sample_start_frame`].
    pub trigger_voice: bool,
    /// Stop the voice outright. FastTracker 2 has exactly one of these: a `9xx` past the
    /// end of the sample (`kFT2ST3OffsetOutOfRange`).
    pub stop_voice: bool,
    /// `fFinalVol`, in the 0..65536 integer domain this crate keeps it in rather than
    /// FT2's `float`. Recomputed every tick by `updateVolPanAutoVib`.
    pub final_volume: u32,
    /// The instrument number of the sample **actually sounding**, which is not
    /// [`XmChannel::instrument_number`]: FastTracker 2 latches any number from 1 to 128
    /// into `instrNum` and falls back to its empty placeholder instrument when the number
    /// names nothing, so a row carrying an invalid instrument moves `instrNum` without
    /// changing a note. libxmp's `vi->ins` — the oracle's instrument column — is written
    /// only where a sample was found, which is exactly where this is.
    pub sounding_instrument: u8,
    /// The note the sounding voice was started on, zero-based from C-0 with the sample's
    /// relative note folded in. Not [`XmChannel::note_number`]: FastTracker 2 latches the
    /// pattern's note byte and *then* rejects a transposed note outside C-0..B-9, leaving
    /// the previous note sounding while the channel's sample and instrument have already
    /// moved on (`NoteLimit.xm`, `ft2_note_range.xm`).
    pub sounding_note: u8,
}

impl XmChannel {
    fn new() -> XmChannel {
        XmChannel {
            note_number: 0,
            instrument_number: 0,
            instrument: None,
            sample: None,
            relative_note: 0,
            finetune: 0,
            real_period: 0,
            out_period: 0,
            final_period: 0,
            portamento_target_period: 0,
            portamento_speed: 0,
            portamento_direction: 0,
            glissando: false,
            sample_volume: 0,
            sample_pan: 128,
            real_volume: 0,
            out_volume: 0,
            out_pan: 128,
            final_pan: 128,
            effect: 0,
            effect_data: 0,
            volume_column: 0,
            delayed_instrument_and_note: 0,
            vibrato_position: 0,
            tremolo_position: 0,
            vibrato_speed: 0,
            vibrato_depth: 0,
            tremolo_speed: 0,
            tremolo_depth: 0,
            waveform_control: 0,
            tremor_parameter: 0,
            tremor_position: 0,
            retrigger_speed: 0,
            retrigger_counter: 0,
            retrigger_volume: 0,
            volume_slide_memory: 0,
            fine_volume_up_memory: 0,
            fine_volume_down_memory: 0,
            pitch_up_memory: 0,
            pitch_down_memory: 0,
            fine_pitch_up_memory: 0,
            fine_pitch_down_memory: 0,
            extra_fine_pitch_up_memory: 0,
            extra_fine_pitch_down_memory: 0,
            global_volume_slide_memory: 0,
            pan_slide_memory: 0,
            offset_memory: 0,
            sample_start_frame: 0,
            key_off: false,
            fadeout_volume: FADEOUT_MAX,
            fadeout_speed: 0,
            volume_envelope_tick: 0,
            volume_envelope_point: 0,
            volume_envelope_value: 0,
            volume_envelope_delta: 0,
            panning_envelope_tick: 0,
            panning_envelope_point: 0,
            panning_envelope_value: 0,
            panning_envelope_delta: 0,
            auto_vibrato_position: 0,
            auto_vibrato_amplitude: 0,
            auto_vibrato_sweep: 0,
            trigger_voice: false,
            stop_voice: false,
            final_volume: 0,
            sounding_instrument: 0,
            sounding_note: 0,
        }
    }
}

/// FastTracker 2's pattern-loop rules, in the shared [`PatternFlowState`]'s vocabulary.
///
/// `patternLoop` keeps a target row and a counter **per channel** and neither is cleared by
/// a position change, which is libxmp's `FLOW_MODE_GENERIC`. The one addition is
/// `shared_break`: FT2 spells the loop jump, the position jump and the pattern break all as
/// writes to the same `song.pBreakPos`, so an `E6x` on a channel to the *right* of a `Bxx`
/// or `Dxx` overwrites the row that jump chose with its own loop target, while the jump
/// itself survives in `song.posJumpFlag`. That is `kFT2PatternLoopWithJumps`.
const fn fast_tracker_flow() -> PatternFlow {
    PatternFlow { shared_break: true, ..PatternFlow::generic() }
}

/// The stateful FastTracker 2 effect processor.
pub struct XmProcessor {
    module: Arc<Module>,
    channels: Box<[XmChannel]>,
    sample_rate_hz: u32,
    /// `song.globalVolume`, 0..64.
    global_volume: u8,
    /// Whether the module's `flags` bit 0 asked for linear frequencies.
    linear_periods: bool,
    /// The replay behaviour this module was loaded with. Resolved once, at construction.
    quirks: QuirkSet,
    flow: PatternFlowState,
}

impl XmProcessor {
    /// A FastTracker 2 processor whose quirks come from the dialect the loader detected.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> XmProcessor {
        XmProcessor::with_quirks(module, sample_rate_hz, QuirkSelection::FromDialect)
    }

    /// A FastTracker 2 processor with an explicit [`QuirkSelection`].
    pub fn with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> XmProcessor {
        let header = module.header();
        let quirks = quirks.resolve(header.dialect);
        let channel_count = header.channel_count as usize;
        let mut channels = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            channels.push(XmChannel::new());
        }
        XmProcessor {
            linear_periods: header.flags.linear_slides,
            global_volume: 64,
            module,
            channels: channels.into_boxed_slice(),
            sample_rate_hz,
            quirks,
            flow: PatternFlowState::new(fast_tracker_flow(), channel_count),
        }
    }

    /// The replay behaviour in force. Fixed for the lifetime of the loaded module.
    pub const fn quirks(&self) -> QuirkSet { self.quirks }

    /// The pattern-loop and break/jump bookkeeping, for inspection by a host or a test.
    pub const fn pattern_flow(&self) -> &PatternFlowState { &self.flow }

    /// Every channel's replay state.
    pub fn channels(&self) -> &[XmChannel] { &self.channels }

    /// One channel's replay state, or `None` if the module has no such channel.
    pub fn channel(&self, channel: u8) -> Option<&XmChannel> { self.channels.get(channel as usize) }

    /// The song's global volume, 0..64.
    pub const fn global_volume(&self) -> u8 { self.global_volume }

    // ── instrument and sample lookup ────────────────────────────────────────────────

    /// The instrument a channel is playing, or `None` for FT2's empty placeholder.
    fn instrument_of(&self, channel_index: usize) -> Option<&InstrumentDef> {
        self.channels[channel_index].instrument.and_then(|id| self.module.instrument(id))
    }

    /// The volume envelope in force, or `None` when the instrument has none or is the
    /// placeholder.
    fn volume_envelope(&self, channel_index: usize) -> Option<&Envelope> {
        self.instrument_of(channel_index).and_then(|instrument| instrument.volume_envelope.as_ref())
    }

    // ── the tick-zero row parse: `getNewNote` ───────────────────────────────────────

    /// `getNewNote` (`ft2_replayer.c` 1349).
    fn get_new_note(&mut self, context: &mut TickContext<'_>, channel_index: usize, cell: XmCell, outcome: &mut TickOutcome, row: u16) {
        let channel_id = ChannelId(channel_index as u16);
        context.report_effect(channel_id, cell.effect, cell.parameter, EffectNames::XM.name(cell.effect, cell.parameter).unwrap_or(""));
        if cell.note != 0 || cell.instrument != 0 {
            context.report_note(channel_id, cell.linear_semitone().map(Note::new), (cell.instrument != 0).then_some(cell.instrument));
        }

        self.channels[channel_index].volume_column = cell.volume;

        // An arpeggio or a vibrato that ended on the previous row puts the period back.
        let previous_effect = self.channels[channel_index].effect;
        let previous_data = self.channels[channel_index].effect_data;
        if previous_effect == 0 {
            if previous_data > 0 {
                self.channels[channel_index].out_period = self.channels[channel_index].real_period;
            }
        } else if matches!(previous_effect, 4 | 6) && !matches!(cell.effect, 4 | 6) {
            self.channels[channel_index].out_period = self.channels[channel_index].real_period;
        }

        self.channels[channel_index].effect = cell.effect;
        self.channels[channel_index].effect_data = cell.parameter;
        self.channels[channel_index].delayed_instrument_and_note = ((cell.instrument as u16) << 8) | cell.note as u16;

        // FT2 keeps the instrument *number* even when the number is out of range, but then
        // treats the row as instrument-less for every "is there an instrument here?" test
        // below — which is the `inst` local, not `ch->instrNum`.
        let mut instrument_column = cell.instrument;
        if instrument_column > 0 {
            if instrument_column <= MAX_INSTRUMENT {
                self.channels[channel_index].instrument_number = instrument_column;
            } else {
                instrument_column = 0;
            }
        }

        // A note delay defers everything, the effect column included, to its own tick.
        if cell.effect == 0x0E && (0xD1..=0xDF).contains(&cell.parameter) {
            return;
        }

        // `E90` is the one effect that reaches the note trigger without going through the
        // portamento / key-off / no-note early exits: it is a retrigger.
        if cell.effect != 0x0E || cell.parameter != 0x90 {
            if self.channels[channel_index].volume_column & 0xF0 == 0xF0 {
                let parameter = self.channels[channel_index].volume_column & 0x0F;
                if parameter > 0 {
                    self.channels[channel_index].portamento_speed = (parameter as u16) << 6;
                }
                self.prepare_portamento(channel_index, cell, instrument_column);
                self.handle_effects_tick_zero(context, channel_index, outcome, row);
                return;
            }
            if matches!(cell.effect, 3 | 5) {
                if cell.effect != 5 && cell.parameter != 0 {
                    self.channels[channel_index].portamento_speed = cell.parameter as u16 * 4;
                }
                self.prepare_portamento(channel_index, cell, instrument_column);
                self.handle_effects_tick_zero(context, channel_index, outcome, row);
                return;
            }
            if cell.effect == 0x14 && cell.parameter == 0 {
                self.key_off(channel_index);
                if instrument_column > 0 {
                    self.reset_volumes(channel_index);
                }
                self.handle_effects_tick_zero(context, channel_index, outcome, row);
                return;
            }
            if cell.note == 0 {
                if instrument_column > 0 {
                    self.reset_volumes(channel_index);
                    self.trigger_instrument(channel_index);
                }
                self.handle_effects_tick_zero(context, channel_index, outcome, row);
                return;
            }
        }

        if cell.note == NOTE_KEY_OFF {
            self.key_off(channel_index);
        } else {
            self.trigger_note(channel_index, cell.note, cell.effect, cell.parameter);
        }

        if instrument_column > 0 {
            self.reset_volumes(channel_index);
            if cell.note != NOTE_KEY_OFF {
                self.trigger_instrument(channel_index);
            }
        }

        self.handle_effects_tick_zero(context, channel_index, outcome, row);
    }

    /// `preparePortamento` (`ft2_replayer.c` 1315).
    ///
    /// The portamento target is computed from the channel's **existing** relative note and
    /// finetune, never the new instrument's — `kFT2PortaIgnoreInstr`, which is what
    /// `porta-offset.xm` and `Porta-Pickup.xm` pin.
    fn prepare_portamento(&mut self, channel_index: usize, cell: XmCell, instrument_column: u8) {
        if cell.note > 0 {
            if cell.note == NOTE_KEY_OFF {
                self.key_off(channel_index);
            } else {
                let state = &self.channels[channel_index];
                let note = (cell.note as i16 - 1 + state.relative_note as i16) * 16 + (state.finetune >> 3) as i16 + 16;
                if (0..PERIOD_TABLE_LEN as i16).contains(&note) {
                    let target = self.period_at(note as usize);
                    let state = &mut self.channels[channel_index];
                    state.portamento_target_period = target;
                    state.portamento_direction = match target.cmp(&state.real_period) {
                        core::cmp::Ordering::Equal => 0,
                        core::cmp::Ordering::Greater => 1,
                        core::cmp::Ordering::Less => 2,
                    };
                }
            }
        }

        if instrument_column > 0 {
            self.reset_volumes(channel_index);
            if cell.note != NOTE_KEY_OFF {
                self.trigger_instrument(channel_index);
            }
        }
    }

    /// `triggerNote` (`ft2_replayer.c` 539).
    fn trigger_note(&mut self, channel_index: usize, note: u8, effect: u8, parameter: u8) {
        if note == NOTE_KEY_OFF {
            self.key_off(channel_index);
            return;
        }
        // `Rxy` and `EDx` reach here with no note of their own and replay the last one.
        let note = match note {
            0 => match self.channels[channel_index].note_number {
                0 => return,
                remembered => remembered,
            },
            note => note,
        };
        self.channels[channel_index].note_number = note;

        let instrument_number = self.channels[channel_index].instrument_number;
        let instrument_id = instrument_number
            .checked_sub(1)
            .map(|index| InstrumentId(index as u16))
            .filter(|id| self.module.instrument(*id).is_some());
        self.channels[channel_index].instrument = instrument_id;

        // A note past B-7 cannot index the 96-entry map. FT2 has no check here; ft2-clone
        // adds one and so does this, because a fuzzed pattern reaches it.
        let mapped_note = core::cmp::min(note, 96);
        let sample_id = instrument_id
            .and_then(|id| self.module.instrument(id))
            .and_then(|instrument| instrument.note_sample_map.get(mapped_note as usize - 1).copied())
            .and_then(|global| global.checked_sub(1))
            .map(SampleId);
        self.channels[channel_index].sample = sample_id;

        let sample = sample_id.and_then(|id| self.module.sample(id));
        self.channels[channel_index].relative_note = sample.map_or(0, |sample| sample.relative_note());

        // FT2's `note += relativeNote` is `uint8_t` arithmetic, and the range test that
        // follows it is unsigned — so a transpose that takes the note below C-0 wraps into
        // the high end of the byte and is rejected there. `NoteLimit.xm` pins exactly this.
        let transposed = note.wrapping_add(self.channels[channel_index].relative_note as u8);
        if transposed >= 120 {
            return;
        }

        self.channels[channel_index].sample_volume = sample.map_or(0, |sample| volume_to_byte(sample.default_volume()));
        self.channels[channel_index].sample_pan = sample.map_or(128, |sample| pan_to_byte(sample.default_pan()));

        self.channels[channel_index].finetune = match effect == 0x0E && parameter & 0xF0 == 0x50 {
            true => (((parameter & 0x0F) as i16) * 16 - 128) as i8,
            false => sample.map_or(0, |sample| sample.finetune()),
        };

        if transposed != 0 {
            let index = (transposed as usize - 1) * 16 + ((self.channels[channel_index].finetune >> 3) as isize + 16) as usize;
            let period = self.period_at(index);
            self.channels[channel_index].real_period = period;
            self.channels[channel_index].out_period = period;
            // B-(-1) — a transpose that lands exactly one semitone below C-0 — updates the
            // key, the instrument and the sample but leaves the note alone, which is
            // libxmp's `FT2_NOTE_BN1` case and FT2's own `if (note != 0)` guard.
            self.channels[channel_index].sounding_note = transposed - 1;
        }

        self.channels[channel_index].trigger_voice = true;

        if effect == 9 {
            if parameter > 0 {
                self.channels[channel_index].offset_memory = self.channels[channel_index].effect_data;
            }
            self.channels[channel_index].sample_start_frame = (self.channels[channel_index].offset_memory as u32) << 8;
        } else {
            self.channels[channel_index].sample_start_frame = 0;
        }
    }

    /// `triggerInstrument` (`ft2_replayer.c` 348): reset the envelopes, the fadeout, the
    /// auto-vibrato and the LFO phases a fresh note starts with.
    fn trigger_instrument(&mut self, channel_index: usize) {
        if self.channels[channel_index].waveform_control & 0x04 == 0 {
            self.channels[channel_index].vibrato_position = 0;
        }
        if self.channels[channel_index].waveform_control & 0x40 == 0 {
            self.channels[channel_index].tremolo_position = 0;
        }
        self.channels[channel_index].retrigger_counter = 0;
        self.channels[channel_index].tremor_position = 0;
        self.channels[channel_index].key_off = false;

        let Some(instrument) = self.instrument_of(channel_index) else { return };
        let volume_enabled = instrument.volume_envelope.is_some();
        let panning_enabled = instrument.panning_envelope.is_some();
        let fadeout = instrument.fadeout;
        let auto_vibrato = self.channels[channel_index].sample
            .and_then(|id| self.module.sample(id))
            .map(|sample| sample.auto_vibrato())
            .unwrap_or_default();

        let state = &mut self.channels[channel_index];
        if volume_enabled {
            state.volume_envelope_tick = u16::MAX;
            state.volume_envelope_point = 0;
        }
        if panning_enabled {
            state.panning_envelope_tick = u16::MAX;
            state.panning_envelope_point = 0;
        }
        state.fadeout_speed = fadeout;
        state.fadeout_volume = FADEOUT_MAX;

        if auto_vibrato.depth > 0 {
            state.auto_vibrato_position = 0;
            if auto_vibrato.sweep > 0 {
                state.auto_vibrato_amplitude = 0;
                state.auto_vibrato_sweep = ((auto_vibrato.depth as u16) << 8) / auto_vibrato.sweep as u16;
            } else {
                state.auto_vibrato_amplitude = (auto_vibrato.depth as u16) << 8;
                state.auto_vibrato_sweep = 0;
            }
        }
    }

    /// `resetVolumes` (`ft2_replayer.c` 339): an instrument column restores the sample's
    /// own volume and pan whether or not it retriggers anything.
    fn reset_volumes(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        state.real_volume = state.sample_volume;
        state.out_volume = state.sample_volume;
        state.out_pan = state.sample_pan;
    }

    /// `keyOff` (`ft2_replayer.c` 411).
    ///
    /// The sample keeps running: a key-off only releases the envelope, and with **no**
    /// volume envelope it silences the channel instead. FT2's own logic bug — the panning
    /// branch tests the *disabled* case — is reproduced by omission: the value it clamps is
    /// only ever read while the panning envelope is enabled, so the branch has no
    /// observable effect and there is nothing here to write.
    fn key_off(&mut self, channel_index: usize) {
        self.channels[channel_index].key_off = true;
        let sustain_tick = self.volume_envelope(channel_index).map(|envelope| {
            let point = self.channels[channel_index].volume_envelope_point as usize;
            envelope.points.get(point).map_or(0, |point| point.tick)
        });
        let state = &mut self.channels[channel_index];
        match sustain_tick {
            Some(tick) => {
                if state.volume_envelope_tick >= tick {
                    state.volume_envelope_tick = tick.wrapping_sub(1);
                }
            }
            None => {
                state.real_volume = 0;
                state.out_volume = 0;
            }
        }
    }

    // ── tick-zero effects ───────────────────────────────────────────────────────────

    /// `handleEffects_TickZero` (`ft2_replayer.c` 1293), volume column first.
    fn handle_effects_tick_zero(&mut self, context: &mut TickContext<'_>, channel_index: usize, outcome: &mut TickOutcome, row: u16) {
        // FT2 passes a *copy* of the volume column through the tick-zero volume handlers
        // and then tests that copy for `Rxy` — so a volume column of `$10` (set volume 0)
        // leaves the copy at zero and makes `Rxy` retrigger on tick zero.
        let mut volume_column_copy = self.channels[channel_index].volume_column;
        match volume_column_copy >> 4 {
            0x1..=0x5 => {
                // `v_SetVolume`
                volume_column_copy = volume_column_copy.wrapping_sub(16).min(64);
                let state = &mut self.channels[channel_index];
                state.real_volume = volume_column_copy;
                state.out_volume = volume_column_copy;
            }
            0x8 => {
                // `v_FineVolSlideDown`
                let state = &mut self.channels[channel_index];
                volume_column_copy = (0u8.wrapping_sub(state.volume_column & 0x0F)).wrapping_add(state.real_volume);
                if (volume_column_copy as i8) < 0 {
                    volume_column_copy = 0;
                }
                state.real_volume = volume_column_copy;
                state.out_volume = volume_column_copy;
            }
            0x9 => {
                // `v_FineVolSlideUp`
                let state = &mut self.channels[channel_index];
                volume_column_copy = (state.volume_column & 0x0F).wrapping_add(state.real_volume);
                if volume_column_copy > 64 {
                    volume_column_copy = 64;
                }
                state.real_volume = volume_column_copy;
                state.out_volume = volume_column_copy;
            }
            0xA => {
                // `v_SetVibSpeed`
                volume_column_copy = (self.channels[channel_index].volume_column & 0x0F) * 4;
                if volume_column_copy != 0 {
                    self.channels[channel_index].vibrato_speed = volume_column_copy;
                }
            }
            0xC => {
                // `v_SetPan`
                volume_column_copy <<= 4;
                self.channels[channel_index].out_pan = volume_column_copy;
            }
            _ => {}
        }

        let effect = self.channels[channel_index].effect;
        let parameter = self.channels[channel_index].effect_data;
        if effect == 0 && parameter == 0 {
            return;
        }

        match effect {
            // `8xx` set panning.
            8 => self.channels[channel_index].out_pan = parameter,
            // `Cxx` set volume.
            12 => {
                let volume = parameter.min(64);
                let state = &mut self.channels[channel_index];
                state.real_volume = volume;
                state.out_volume = volume;
            }
            // `Rxy` multi retrigger.
            27 => self.multi_note_retrigger(channel_index, parameter, volume_column_copy),
            // `Xxy` extra-fine portamento.
            33 => self.extra_fine_pitch_slide(channel_index, parameter),
            _ => {}
        }

        self.handle_more_effects_tick_zero(context, channel_index, outcome, row);
    }

    /// `handleMoreEffects_TickZero` (`ft2_replayer.c` 1025) — `Bxx`, `Dxx`, `Exy`, `Fxx`,
    /// `Gxx` and `Lxx`, the effects FT2 runs even on a muted channel.
    fn handle_more_effects_tick_zero(&mut self, context: &mut TickContext<'_>, channel_index: usize, outcome: &mut TickOutcome, row: u16) {
        let parameter = self.channels[channel_index].effect_data;
        match self.channels[channel_index].effect {
            // `Bxx` position jump. FT2 stores `param - 1` and lets `getNextPos` increment
            // it, so the destination order is the parameter itself.
            11 => {
                self.flow.pattern_jump(parameter as u16);
            }
            // `Dxx` pattern break. The parameter is decimal, and one past 63 breaks to row 0.
            13 => {
                let target = (parameter >> 4) as u16 * 10 + (parameter & 0x0F) as u16;
                self.flow.pattern_break(if target <= 63 { target } else { 0 });
            }
            // `Exy` — the tick-zero half of the extended commands.
            14 => self.extended_effect_tick_zero(channel_index, parameter, outcome, row),
            // `Fxx` — below 32 a speed, otherwise a tempo.
            15 => {
                if parameter >= 32 {
                    outcome.tempo_bpm = parameter as u16;
                } else {
                    outcome.speed = parameter;
                }
            }
            // `Gxx` global volume.
            16 => {
                self.global_volume = parameter.min(64);
                context.report_global_volume(starplayer_core::fixed::unit_from_ratio(self.global_volume as u32, 64));
            }
            // `Lxx` set envelope position.
            21 => self.set_envelope_position(channel_index, parameter),
            _ => {}
        }
    }

    /// `E_Effects_TickZero` (`ft2_replayer.c` 753).
    fn extended_effect_tick_zero(&mut self, channel_index: usize, parameter: u8, outcome: &mut TickOutcome, row: u16) {
        let sub_effect = parameter >> 4;
        let value = parameter & 0x0F;
        match sub_effect {
            // `E1x` fine portamento up.
            1 => {
                let value = match value {
                    0 => self.channels[channel_index].fine_pitch_up_memory,
                    value => value,
                };
                self.channels[channel_index].fine_pitch_up_memory = value;
                let state = &mut self.channels[channel_index];
                state.real_period = state.real_period.wrapping_sub(value as u16 * 4);
                if (state.real_period as i16) < 1 {
                    state.real_period = 1;
                }
                state.out_period = state.real_period;
            }
            // `E2x` fine portamento down.
            2 => {
                let value = match value {
                    0 => self.channels[channel_index].fine_pitch_down_memory,
                    value => value,
                };
                self.channels[channel_index].fine_pitch_down_memory = value;
                let state = &mut self.channels[channel_index];
                state.real_period = state.real_period.wrapping_add(value as u16 * 4);
                if (state.real_period as i16) >= MAX_PERIOD as i16 {
                    state.real_period = MAX_PERIOD - 1;
                }
                state.out_period = state.real_period;
            }
            // `E3x` glissando control.
            3 => self.channels[channel_index].glissando = value != 0,
            // `E4x` vibrato waveform.
            4 => {
                let state = &mut self.channels[channel_index];
                state.waveform_control = (state.waveform_control & 0xF0) | value;
            }
            // `E6x` pattern loop.
            6 => self.flow.pattern_loop(channel_index, row, value),
            // `E7x` tremolo waveform.
            7 => {
                let state = &mut self.channels[channel_index];
                state.waveform_control = (value << 4) | (state.waveform_control & 0x0F);
            }
            // `EAx` fine volume slide up.
            0xA => {
                let value = match value {
                    0 => self.channels[channel_index].fine_volume_up_memory,
                    value => value,
                };
                self.channels[channel_index].fine_volume_up_memory = value;
                let state = &mut self.channels[channel_index];
                state.real_volume = state.real_volume.wrapping_add(value);
                if state.real_volume > 64 {
                    state.real_volume = 64;
                }
                state.out_volume = state.real_volume;
            }
            // `EBx` fine volume slide down.
            0xB => {
                let value = match value {
                    0 => self.channels[channel_index].fine_volume_down_memory,
                    value => value,
                };
                self.channels[channel_index].fine_volume_down_memory = value;
                let state = &mut self.channels[channel_index];
                state.real_volume = state.real_volume.wrapping_sub(value);
                if (state.real_volume as i8) < 0 {
                    state.real_volume = 0;
                }
                state.out_volume = state.real_volume;
            }
            // `EC0` — and only `EC0`; every other `ECx` waits for its tick.
            0xC => {
                if value == 0 {
                    let state = &mut self.channels[channel_index];
                    state.real_volume = 0;
                    state.out_volume = 0;
                }
            }
            // `EEx` pattern delay. FT2 stores `param + 1` repeats of the row and lets the
            // **rightmost** channel of the row win, `EE0` included.
            0xE => outcome.pattern_delay = value,
            _ => {}
        }
    }

    /// `setEnvelopePos` (`ft2_replayer.c` 831) — `Lxx`.
    ///
    /// The panning half is gated on the **volume** envelope's sustain flag, which is FT2's
    /// own logic bug and OpenMPT's `kFT2SetPanEnvPos` (`SetEnvPos.xm`).
    fn set_envelope_position(&mut self, channel_index: usize, parameter: u8) {
        // `&self.module` and `&mut self.channels[..]` are borrows of **different fields**,
        // which is what lets the envelope be read in place rather than cloned. Cloning it
        // would allocate its point list, inside `render()` — architecture §8.
        let module = &self.module;
        let state = &mut self.channels[channel_index];
        let instrument = state.instrument.and_then(|id| module.instrument(id));
        if let Some(envelope) = instrument.and_then(|instrument| instrument.volume_envelope.as_ref()) {
            let (point, delta, value) = seek_envelope(envelope, parameter);
            state.volume_envelope_tick = (parameter as u16).wrapping_sub(1);
            state.volume_envelope_point = point;
            state.volume_envelope_delta = delta;
            state.volume_envelope_value = value;
        }
        let volume_has_sustain = instrument
            .and_then(|instrument| instrument.volume_envelope.as_ref())
            .is_some_and(|envelope| envelope.sustain.is_some());
        if volume_has_sustain
            && let Some(envelope) = instrument.and_then(|instrument| instrument.panning_envelope.as_ref())
        {
            let (point, delta, value) = seek_envelope(envelope, parameter);
            state.panning_envelope_tick = (parameter as u16).wrapping_sub(1);
            state.panning_envelope_point = point;
            state.panning_envelope_delta = delta;
            state.panning_envelope_value = value;
        }
    }

    // ── per-tick effects ────────────────────────────────────────────────────────────

    /// `handleEffects_TickNonZero` (`ft2_replayer.c` 2274). `tick` is FT2's
    /// `song.speed - song.tick`: the tick index within the current pattern-delay repeat.
    fn handle_effects_tick_non_zero(&mut self, context: &mut TickContext<'_>, channel_index: usize, tick: u16, speed: u8) {
        match self.channels[channel_index].volume_column >> 4 {
            // `v_VolSlideDown`
            0x6 => {
                let state = &mut self.channels[channel_index];
                let mut volume = (0u8.wrapping_sub(state.volume_column & 0x0F)).wrapping_add(state.real_volume);
                if (volume as i8) < 0 {
                    volume = 0;
                }
                state.real_volume = volume;
                state.out_volume = volume;
            }
            // `v_VolSlideUp`
            0x7 => {
                let state = &mut self.channels[channel_index];
                let mut volume = (state.volume_column & 0x0F).wrapping_add(state.real_volume);
                if volume > 64 {
                    volume = 64;
                }
                state.real_volume = volume;
                state.out_volume = volume;
            }
            // `v_Vibrato`
            0xB => {
                let depth = self.channels[channel_index].volume_column & 0x0F;
                if depth > 0 {
                    self.channels[channel_index].vibrato_depth = depth;
                }
                self.do_vibrato(channel_index);
            }
            // `v_PanSlideLeft` — including FT2's bug that a slide of zero sets pan to zero.
            0xD => {
                let state = &mut self.channels[channel_index];
                let pan = state.out_pan as u16 + (0u8.wrapping_sub(state.volume_column & 0x0F)) as u16;
                state.out_pan = if pan < 256 { 0 } else { pan as u8 };
            }
            // `v_PanSlideRight`
            0xE => {
                let state = &mut self.channels[channel_index];
                let pan = state.out_pan as u16 + (state.volume_column & 0x0F) as u16;
                state.out_pan = if pan > 255 { 255 } else { pan as u8 };
            }
            // `v_Portamento`
            0xF => self.portamento(channel_index),
            _ => {}
        }

        let effect = self.channels[channel_index].effect;
        let parameter = self.channels[channel_index].effect_data;
        if (effect == 0 && parameter == 0) || effect > 35 {
            return;
        }

        match effect {
            0 => self.arpeggio(channel_index, parameter, speed as u16 - tick),
            1 => self.pitch_slide_up(channel_index, parameter),
            2 => self.pitch_slide_down(channel_index, parameter),
            3 => self.portamento(channel_index),
            4 => self.vibrato(channel_index, parameter),
            5 => {
                self.portamento(channel_index);
                self.volume_slide(channel_index, parameter);
            }
            6 => {
                self.do_vibrato(channel_index);
                self.volume_slide(channel_index, parameter);
            }
            7 => self.tremolo(channel_index, parameter),
            0xA => self.volume_slide(channel_index, parameter),
            0xE => self.extended_effect_tick_non_zero(channel_index, parameter, tick),
            // `Hxy` global volume slide.
            17 => self.global_volume_slide(context, channel_index, parameter),
            // `Kxx` key off at a tick.
            20 => {
                if tick == (parameter & 31) as u16 {
                    self.key_off(channel_index);
                }
            }
            // `Pxy` pan slide.
            25 => self.pan_slide(channel_index, parameter),
            // `Rxy` multi retrigger.
            27 => self.do_multi_note_retrigger(channel_index),
            // `Txy` tremor.
            29 => self.tremor(channel_index, parameter),
            _ => {}
        }
    }

    /// `E_Effects_TickNonZero` (`ft2_replayer.c` 2229) — `E9x`, `ECx` and `EDx`.
    fn extended_effect_tick_non_zero(&mut self, channel_index: usize, parameter: u8, tick: u16) {
        let value = (parameter & 0x0F) as u16;
        match parameter >> 4 {
            // `E9x` retrigger. `E90` was already handled on tick zero.
            9 => {
                if value != 0 && tick.is_multiple_of(value) {
                    self.trigger_note(channel_index, 0, 0, 0);
                    self.trigger_instrument(channel_index);
                }
            }
            // `ECx` note cut.
            0xC => {
                if tick == value {
                    let state = &mut self.channels[channel_index];
                    state.real_volume = 0;
                    state.out_volume = 0;
                }
            }
            // `EDx` note delay.
            0xD if tick == value => {
                {
                    let latched = self.channels[channel_index].delayed_instrument_and_note;
                    self.trigger_note(channel_index, (latched & 0xFF) as u8, 0, 0);
                    if latched >> 8 > 0 {
                        self.reset_volumes(channel_index);
                    }
                    self.trigger_instrument(channel_index);

                    let volume_column = self.channels[channel_index].volume_column;
                    if (0x10..=0x50).contains(&volume_column) {
                        let volume = volume_column - 16;
                        let state = &mut self.channels[channel_index];
                        state.out_volume = volume;
                        state.real_volume = volume;
                    } else if (0xC0..=0xCF).contains(&volume_column) {
                        self.channels[channel_index].out_pan = (volume_column & 0x0F) << 4;
                    }
                }
            }
            _ => {}
        }
    }

    /// `doVibrato` (`ft2_replayer.c` 1836).
    fn do_vibrato(&mut self, channel_index: usize) {
        let state = &mut self.channels[channel_index];
        let index = ((state.vibrato_position >> 2) & 0x1F) as usize;
        let mut sample = match state.waveform_control & 3 {
            0 => VIBRATO_TABLE[index],
            1 => {
                let ramp = (index as u8) << 3;
                match (state.vibrato_position as i8) < 0 {
                    true => !ramp,
                    false => ramp,
                }
            }
            _ => 255,
        };
        sample = ((sample as u16 * state.vibrato_depth as u16) >> 5) as u8;
        state.out_period = match (state.vibrato_position as i8) < 0 {
            true => state.real_period.wrapping_sub(sample as u16),
            false => state.real_period.wrapping_add(sample as u16),
        };
        state.vibrato_position = state.vibrato_position.wrapping_add(state.vibrato_speed);
    }

    /// `vibrato` (`ft2_replayer.c` 1955) — the `4xy` entry point, which latches its nibbles
    /// before running the LFO.
    fn vibrato(&mut self, channel_index: usize, parameter: u8) {
        if parameter > 0 {
            let depth = parameter & 0x0F;
            if depth > 0 {
                self.channels[channel_index].vibrato_depth = depth;
            }
            let speed = (parameter & 0xF0) >> 2;
            if speed > 0 {
                self.channels[channel_index].vibrato_speed = speed;
            }
        }
        self.do_vibrato(channel_index);
    }

    /// `tremolo` (`ft2_replayer.c` 1987), FT2's ramp-waveform bug included: the sign of the
    /// ramp comes from the **vibrato** position, not the tremolo's.
    fn tremolo(&mut self, channel_index: usize, parameter: u8) {
        if parameter > 0 {
            let depth = parameter & 0x0F;
            if depth > 0 {
                self.channels[channel_index].tremolo_depth = depth;
            }
            let speed = (parameter & 0xF0) >> 2;
            if speed > 0 {
                self.channels[channel_index].tremolo_speed = speed;
            }
        }
        let state = &mut self.channels[channel_index];
        let index = ((state.tremolo_position >> 2) & 0x1F) as usize;
        let mut sample = match (state.waveform_control >> 4) & 3 {
            0 => VIBRATO_TABLE[index],
            1 => {
                let ramp = (index as u8) << 3;
                match (state.vibrato_position as i8) < 0 {
                    true => !ramp,
                    false => ramp,
                }
            }
            _ => 255,
        };
        sample = ((sample as u16 * state.tremolo_depth as u16) >> 6) as u8;
        let volume = match (state.tremolo_position as i8) < 0 {
            true => (state.real_volume as i16 - sample as i16).max(0),
            false => (state.real_volume as i16 + sample as i16).min(64),
        };
        state.out_volume = volume as u8;
        state.tremolo_position = state.tremolo_position.wrapping_add(state.tremolo_speed);
    }

    /// `arpeggio` (`ft2_replayer.c` 1869). `tick` is FT2's own down-counting `song.tick`.
    fn arpeggio(&mut self, channel_index: usize, parameter: u8, ft2_tick: u16) {
        let selector = ARPEGGIO_TICK_TABLE[(ft2_tick & 31) as usize];
        if selector == 0 {
            self.channels[channel_index].out_period = self.channels[channel_index].real_period;
            return;
        }
        let offset = match selector {
            1 => parameter >> 4,
            _ => parameter & 0x0F,
        };
        let period = self.channels[channel_index].real_period;
        self.channels[channel_index].out_period = self.period_to_note_period(channel_index, period, offset);
    }

    /// `pitchSlideUp` (`ft2_replayer.c` 1891).
    fn pitch_slide_up(&mut self, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].pitch_up_memory,
            value => value,
        };
        self.channels[channel_index].pitch_up_memory = value;
        let state = &mut self.channels[channel_index];
        state.real_period = state.real_period.wrapping_sub(value as u16 * 4);
        if (state.real_period as i16) < 1 {
            state.real_period = 1;
        }
        state.out_period = state.real_period;
    }

    /// `pitchSlideDown` (`ft2_replayer.c` 1906).
    fn pitch_slide_down(&mut self, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].pitch_down_memory,
            value => value,
        };
        self.channels[channel_index].pitch_down_memory = value;
        let state = &mut self.channels[channel_index];
        state.real_period = state.real_period.wrapping_add(value as u16 * 4);
        if (state.real_period as i16) >= MAX_PERIOD as i16 {
            state.real_period = MAX_PERIOD - 1;
        }
        state.out_period = state.real_period;
    }

    /// `extraFinePitchSlide` (`ft2_replayer.c` 1182) — `X1x` and `X2x`, each with its own
    /// memory, and everything past `X2` ignored.
    fn extra_fine_pitch_slide(&mut self, channel_index: usize, parameter: u8) {
        let value = parameter & 0x0F;
        match parameter >> 4 {
            1 => {
                let value = match value {
                    0 => self.channels[channel_index].extra_fine_pitch_up_memory,
                    value => value,
                };
                self.channels[channel_index].extra_fine_pitch_up_memory = value;
                let state = &mut self.channels[channel_index];
                let mut period = state.real_period.wrapping_sub(value as u16);
                if (period as i16) < 1 {
                    period = 1;
                }
                state.real_period = period;
                state.out_period = period;
            }
            2 => {
                let value = match value {
                    0 => self.channels[channel_index].extra_fine_pitch_down_memory,
                    value => value,
                };
                self.channels[channel_index].extra_fine_pitch_down_memory = value;
                let state = &mut self.channels[channel_index];
                let mut period = state.real_period.wrapping_add(value as u16);
                if (period as i16) >= MAX_PERIOD as i16 {
                    period = MAX_PERIOD - 1;
                }
                state.real_period = period;
                state.out_period = period;
            }
            _ => {}
        }
    }

    /// `portamento` (`ft2_replayer.c` 1921).
    fn portamento(&mut self, channel_index: usize) {
        if self.channels[channel_index].portamento_direction == 0 {
            return;
        }
        let state = &mut self.channels[channel_index];
        if state.portamento_direction > 1 {
            state.real_period = state.real_period.wrapping_sub(state.portamento_speed);
            if (state.real_period as i16) <= (state.portamento_target_period as i16) {
                state.portamento_direction = 1;
                state.real_period = state.portamento_target_period;
            }
        } else {
            state.real_period = state.real_period.wrapping_add(state.portamento_speed);
            if state.real_period >= state.portamento_target_period {
                state.portamento_direction = 1;
                state.real_period = state.portamento_target_period;
            }
        }

        if self.channels[channel_index].glissando {
            let period = self.channels[channel_index].real_period;
            self.channels[channel_index].out_period = self.period_to_note_period(channel_index, period, 0);
        } else {
            self.channels[channel_index].out_period = self.channels[channel_index].real_period;
        }
    }

    /// `volSlide` (`ft2_replayer.c` 2041): the **down** nibble wins when both are set.
    fn volume_slide(&mut self, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].volume_slide_memory,
            value => value,
        };
        self.channels[channel_index].volume_slide_memory = value;
        let state = &mut self.channels[channel_index];
        let mut volume = state.real_volume;
        if value & 0xF0 == 0 {
            volume = volume.wrapping_sub(value);
            if (volume as i8) < 0 {
                volume = 0;
            }
        } else {
            volume = volume.wrapping_add(value >> 4);
            if volume > 64 {
                volume = 64;
            }
        }
        state.real_volume = volume;
        state.out_volume = volume;
    }

    /// `globalVolSlide` (`ft2_replayer.c` 2068).
    fn global_volume_slide(&mut self, context: &mut TickContext<'_>, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].global_volume_slide_memory,
            value => value,
        };
        self.channels[channel_index].global_volume_slide_memory = value;
        let mut volume = self.global_volume;
        if value & 0xF0 == 0 {
            volume = volume.wrapping_sub(value);
            if (volume as i8) < 0 {
                volume = 0;
            }
        } else {
            volume = volume.wrapping_add(value >> 4);
            if volume > 64 {
                volume = 64;
            }
        }
        self.global_volume = volume;
        context.report_global_volume(starplayer_core::fixed::unit_from_ratio(self.global_volume as u32, 64));
    }

    /// `panningSlide` (`ft2_replayer.c` 2106).
    fn pan_slide(&mut self, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].pan_slide_memory,
            value => value,
        };
        self.channels[channel_index].pan_slide_memory = value;
        let state = &mut self.channels[channel_index];
        let mut pan = state.out_pan as i16;
        if value & 0xF0 == 0 {
            pan -= value as i16;
            if pan < 0 {
                pan = 0;
            }
        } else {
            pan += (value >> 4) as i16;
            if pan > 255 {
                pan = 255;
            }
        }
        state.out_pan = pan as u8;
    }

    /// `tremor` (`ft2_replayer.c` 2133): the phase length is the nibble **plus one**, and
    /// the counter is not touched on tick zero.
    fn tremor(&mut self, channel_index: usize, parameter: u8) {
        let value = match parameter {
            0 => self.channels[channel_index].tremor_parameter,
            value => value,
        };
        self.channels[channel_index].tremor_parameter = value;
        let state = &mut self.channels[channel_index];
        let mut sign = state.tremor_position & 0x80;
        let mut data = state.tremor_position & 0x7F;
        data = data.wrapping_sub(1);
        if (data as i8) < 0 {
            if sign == 0x80 {
                sign = 0x00;
                data = value & 0x0F;
            } else {
                sign = 0x80;
                data = value >> 4;
            }
        }
        state.tremor_position = sign | data;
        state.out_volume = match sign == 0x80 {
            true => state.real_volume,
            false => 0,
        };
    }

    /// `multiNoteRetrig` (`ft2_replayer.c` 1273) — `Rxy`'s tick-zero half.
    fn multi_note_retrigger(&mut self, channel_index: usize, parameter: u8, volume_column_copy: u8) {
        let speed = match parameter & 0x0F {
            0 => self.channels[channel_index].retrigger_speed,
            speed => speed,
        };
        self.channels[channel_index].retrigger_speed = speed;
        let volume = match parameter >> 4 {
            0 => self.channels[channel_index].retrigger_volume,
            volume => volume,
        };
        self.channels[channel_index].retrigger_volume = volume;
        if volume_column_copy == 0 {
            self.do_multi_note_retrigger(channel_index);
        }
    }

    /// `doMultiNoteRetrig` (`ft2_replayer.c` 1222).
    fn do_multi_note_retrigger(&mut self, channel_index: usize) {
        let count = self.channels[channel_index].retrigger_counter + 1;
        if count < self.channels[channel_index].retrigger_speed {
            self.channels[channel_index].retrigger_counter = count;
            return;
        }
        self.channels[channel_index].retrigger_counter = 0;

        let state = &mut self.channels[channel_index];
        let old = state.real_volume as i16;
        let volume = match state.retrigger_volume {
            0x1 => old - 1,
            0x2 => old - 2,
            0x3 => old - 4,
            0x4 => old - 8,
            0x5 => old - 16,
            0x6 => (old >> 1) + (old >> 3) + (old >> 4),
            0x7 => old >> 1,
            0x9 => old + 1,
            0xA => old + 2,
            0xB => old + 4,
            0xC => old + 8,
            0xD => old + 16,
            0xE => (old >> 1) + old,
            0xF => old + old,
            _ => old,
        }.clamp(0, 64) as u8;
        state.real_volume = volume;
        state.out_volume = volume;

        let volume_column = state.volume_column;
        if (0x10..=0x50).contains(&volume_column) {
            state.out_volume = volume_column - 0x10;
            state.real_volume = state.out_volume;
        } else if (0xC0..=0xCF).contains(&volume_column) {
            state.out_pan = (volume_column & 0x0F) << 4;
        }

        self.trigger_note(channel_index, 0, 0, 0);
    }

    // ── articulation: `updateVolPanAutoVib` ─────────────────────────────────────────

    /// `updateVolPanAutoVib` (`ft2_replayer.c` 1457): one tick of the fadeout, both
    /// envelopes and the auto-vibrato, for one channel. Run for **every** channel on
    /// **every** tick, after that channel's effects.
    ///
    /// Leaves the final voice volume in [`XmChannel::final_volume`], in the 0..65536 domain
    /// this crate keeps FT2's `float fFinalVol` in.
    fn update_volume_pan_auto_vibrato(&mut self, channel_index: usize) {
        // `&self.module` and `&mut self.channels[..]` borrow different fields, so both
        // envelopes are read **in place**. Cloning one would allocate its point list on
        // every tick of every channel, inside `render()` — architecture §8.
        let global_volume = self.global_volume.min(64) as u64;
        let module = &self.module;
        let state = &mut self.channels[channel_index];
        let instrument = state.instrument.and_then(|id| module.instrument(id));

        // *** FADEOUT ON KEY OFF ***
        if state.key_off {
            if state.fadeout_speed > state.fadeout_volume {
                state.fadeout_volume = 0;
                state.fadeout_speed = 0;
            } else {
                state.fadeout_volume -= state.fadeout_speed;
            }
        }

        // *** VOLUME ENVELOPE ***
        let key_off = state.key_off;
        let envelope_value = match instrument.and_then(|instrument| instrument.volume_envelope.as_ref()) {
            Some(envelope) => {
                let mut cursor = EnvelopeCursor {
                    tick: state.volume_envelope_tick,
                    point: state.volume_envelope_point,
                    value: state.volume_envelope_value,
                    delta: state.volume_envelope_delta,
                };
                let value = advance_envelope(&mut cursor, envelope, key_off);
                state.volume_envelope_tick = cursor.tick;
                state.volume_envelope_point = cursor.point;
                state.volume_envelope_value = cursor.value;
                state.volume_envelope_delta = cursor.delta;
                value
            }
            None => ENVELOPE_UNITY,
        };

        // *** PANNING ENVELOPE ***
        let panning_value = instrument.and_then(|instrument| instrument.panning_envelope.as_ref()).map(|envelope| {
            let mut cursor = EnvelopeCursor {
                tick: state.panning_envelope_tick,
                point: state.panning_envelope_point,
                value: state.panning_envelope_value,
                delta: state.panning_envelope_delta,
            };
            let value = advance_envelope(&mut cursor, envelope, key_off);
            state.panning_envelope_tick = cursor.tick;
            state.panning_envelope_point = cursor.point;
            state.panning_envelope_value = cursor.value;
            state.panning_envelope_delta = cursor.delta;
            value
        });

        state.final_pan = match panning_value {
            Some(value) => {
                let mut multiplier = state.out_pan as i32 - 128;
                if multiplier >= 0 {
                    multiplier = -multiplier;
                }
                multiplier = (multiplier + 128) << 3;
                let centred = value - 32 * 256;
                let addend = ((centred * multiplier) >> 16) as i8;
                state.out_pan.wrapping_add(addend as u8)
            }
            None => state.out_pan,
        };

        // *** AUTO VIBRATO ***
        let auto_vibrato = state.sample
            .and_then(|id| module.sample(id))
            .map(|sample| sample.auto_vibrato())
            .unwrap_or_default();
        if auto_vibrato.depth > 0 {
            let amplitude = match state.auto_vibrato_sweep > 0 {
                true => {
                    let mut amplitude = state.auto_vibrato_sweep;
                    if !state.key_off {
                        amplitude = amplitude.wrapping_add(state.auto_vibrato_amplitude);
                        if amplitude >> 8 > auto_vibrato.depth as u16 {
                            amplitude = (auto_vibrato.depth as u16) << 8;
                            state.auto_vibrato_sweep = 0;
                        }
                        state.auto_vibrato_amplitude = amplitude;
                    }
                    amplitude
                }
                false => state.auto_vibrato_amplitude,
            };
            state.auto_vibrato_position = state.auto_vibrato_position.wrapping_add(auto_vibrato.rate);
            let sample = match auto_vibrato.waveform {
                AutoVibratoWaveform::Square => match state.auto_vibrato_position > 127 {
                    true => 64i16,
                    false => -64,
                },
                // FT2's `vibType` 2 is ramp **down** in its own numbering and the loader
                // maps it to [`AutoVibratoWaveform::RampDown`]; the arithmetic below is
                // FT2's own, sign and all.
                AutoVibratoWaveform::RampDown => (((state.auto_vibrato_position >> 1) as i16 + 64) & 127) - 64,
                AutoVibratoWaveform::RampUp => ((-((state.auto_vibrato_position >> 1) as i16) + 64) & 127) - 64,
                // FastTracker 2 has only four waveforms and masks anything else down to a
                // sine, which is where the loader already sends an unknown `vibType`; IT's
                // random waveform can only reach here through a hand-built module.
                AutoVibratoWaveform::Sine | AutoVibratoWaveform::Random => AUTO_VIBRATO_SINE_TABLE[state.auto_vibrato_position as usize] as i16,
            };
            let delta = ((sample as i32 * amplitude as i32) >> 14) as i16;
            let period = state.out_period.wrapping_add(delta as u16);
            state.final_period = if period >= MAX_PERIOD { 0 } else { period };
        } else {
            state.final_period = state.out_period;
        }

        // The final volume, in FT2's own product: global x channel x fadeout x envelope,
        // whose full scale is `64 * 64 * 32768 * 16384 == 2^41`. Shifting to a 0..65536
        // range is a shift by 25, and the trace's 0..64 domain is a further rounded 10.
        let product = global_volume
            * state.out_volume.min(64) as u64
            * state.fadeout_volume as u64
            * envelope_value.clamp(0, ENVELOPE_UNITY) as u64;
        state.final_volume = (product >> 25) as u32;
    }

    // ── pitch conversion ────────────────────────────────────────────────────────────

    /// The period at a note/finetune index, in whichever domain the module asked for.
    fn period_at(&self, index: usize) -> u16 {
        match self.linear_periods {
            true => linear_period(index),
            false => amiga_period(index),
        }
    }

    /// `period2NotePeriod` (`ft2_replayer.c` 1802): round a period to the nearest note in
    /// the table, then step `offset` semitones up from it.
    ///
    /// FT2's binary search runs over `8 * 12 * 16` rather than `10 * 12 * 16` and its final
    /// clamp is one short of the table, so notes above B-7 misbehave. Both are reproduced.
    fn period_to_note_period(&self, channel_index: usize, period: u16, offset: u8) -> u16 {
        let finetune = (self.channels[channel_index].finetune >> 3) as i32 + 16;
        let mut high = 8 * 12 * 16i32;
        let mut low = 0i32;
        for _ in 0..8 {
            let probe = (((low + high) >> 1) & !15) + finetune;
            let lookup = core::cmp::max(probe - 8, 0) as usize;
            if period >= self.period_at(lookup) {
                high = (probe - finetune) & !15;
            } else {
                low = (probe - finetune) & !15;
            }
        }
        let mut probe = low + finetune + ((offset as i32) << 4);
        if probe >= (8 * 12 * 16 + 15) - 1 {
            probe = (8 * 12 * 16 + 16) - 1;
        }
        self.period_at(core::cmp::max(probe, 0) as usize)
    }

    /// The mixer step for a FastTracker 2 period.
    ///
    /// Linear mode is `8363 * 2^((4608 - period) / 768)`, read out of
    /// [`linear_frequency_q24`] exactly as FT2's `period2Ft2Delta` reads its own `logTab` —
    /// table only, no transcendental function anywhere on this path (design goal 5). Amiga
    /// mode is `8363 * 1712 / period`, the format's own constant.
    fn step_for_period(&self, period: u16) -> Step {
        if period == 0 {
            // FT2: "in FT2, a period of 0 results in 0Hz".
            return Step::ZERO;
        }
        let rate = self.sample_rate_hz.max(1) as u64;
        match self.linear_periods {
            true => {
                // `period2Ft2Delta`'s own arithmetic: the inverse period, masked to sixteen
                // bits so FT2's period overflow quirk survives, read out of the shared
                // Q8.24 table. `linear_frequency_q24` answers `2^(units/768 - 14)`, which
                // is `2^((4608 - period)/768)` scaled by `2^-8`, so the numerator carries
                // the reference rate shifted up by sixteen and the quotient is already
                // Q32.32.
                let inverse = LINEAR_INVERSE_BASE.wrapping_sub(period as u32) & 0xFFFF;
                let frequency_q24 = linear_frequency_q24(inverse) as u64;
                Step::from_bits(LINEAR_FREQUENCY_NUMERATOR.saturating_mul(frequency_q24) / rate)
            }
            false => Step::from_ratio(REFERENCE_RATE_HZ * AMIGA_C4_PERIOD, period as u64 * rate),
        }
    }

    // ── the voice ───────────────────────────────────────────────────────────────────

    /// Turn one channel's state into voice writes.
    fn flush_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let channel_id = ChannelId(channel_index as u16);
        if core::mem::take(&mut self.channels[channel_index].stop_voice) {
            self.channels[channel_index].trigger_voice = false;
            context.stop_channel(channel_id);
            return;
        }

        if core::mem::take(&mut self.channels[channel_index].trigger_voice) {
            let playable = self.channels[channel_index].sample
                .and_then(|id| self.module.sample(id))
                .filter(|sample| sample.length_frames() > 0);
            let Some(sample) = playable else {
                // A note whose instrument, sub-instrument or sample is unusable **cuts**
                // the channel rather than leaving the previous note sounding: FastTracker 2
                // triggers a voice whose sample pointer is null, and libxmp spells the same
                // outcome as "playing with an active invalid sample cuts the channel"
                // (`read_event.c` 612-620, `ft2_invalid_ins_defaults.xm`).
                self.channels[channel_index].sample = None;
                self.channels[channel_index].sounding_instrument = 0;
                context.stop_channel(channel_id);
                return;
            };
            {
                let sample_id = self.channels[channel_index].sample.unwrap_or(starplayer_core::SampleId(0));
                let offset = self.channels[channel_index].sample_start_frame;
                // `kFT2ST3OffsetOutOfRange`: a `9xx` past the end of the sample stops the
                // channel outright, and the note is not picked up by a later portamento.
                if offset >= sample.length_frames() {
                    self.channels[channel_index].note_number = 0;
                    self.channels[channel_index].sounding_instrument = 0;
                    context.stop_channel(channel_id);
                    return;
                }
                let state = &self.channels[channel_index];
                let params = VoiceParams {
                    step: self.step_for_period(state.final_period),
                    volume: volume_from_bits(state.final_volume),
                    pan: pan_from_byte(state.final_pan),
                    dirty: DirtyBits::SAMPLE | DirtyBits::PITCH | DirtyBits::VOLUME | DirtyBits::PAN,
                    ..VoiceParams::SILENT
                };
                let tag = VoiceTag {
                    channel: channel_index as u8,
                    instrument: state.instrument_number,
                    sample: sample_id.0.saturating_add(1),
                    note: state.sounding_note,
                };
                context.trigger_channel(channel_id, tag, sample_region(sample), params, offset);
                self.channels[channel_index].sounding_instrument = self.channels[channel_index].instrument_number;
                return;
            }
        }

        // Every tick, FT2 recomputes the final period, volume and pan and hands all three
        // to its mixer. Written straight into `params` rather than through
        // `write_voice_param` so the trace's dirty flags stay effect-driven (task E3).
        let Some(voice) = context.channels.foreground(channel_id) else { return };
        let Some(voice) = context.voices.get_mut(voice) else { return };
        let state = &self.channels[channel_index];
        let step = self.step_for_period(state.final_period);
        if voice.params.step != step {
            voice.params.set_step(step);
        }
        let volume = volume_from_bits(state.final_volume);
        if voice.params.volume != volume {
            voice.params.set_volume(volume);
        }
        let pan = pan_from_byte(state.final_pan);
        if voice.params.pan != pan {
            voice.params.set_pan(pan);
        }
    }

    /// Feed the diagnostic per-tick trace.
    #[cfg(feature = "trace")]
    fn report_trace_channels(&self, context: &mut TickContext<'_>) {
        for channel_index in 0..self.channels.len() {
            context.report_trace_channel(ChannelId(channel_index as u16), self.trace_channel_state(channel_index));
        }
    }

    #[cfg(any(feature = "trace", test))]
    fn trace_channel_state(&self, channel_index: usize) -> TraceChannelState {
        let state = &self.channels[channel_index];
        TraceChannelState {
            note: (state.note_number != 0).then_some(state.sounding_note),
            instrument: state.sounding_instrument as u16,
            sample: state.sample.map_or(0, |id| id.0.saturating_add(1)),
            // The oracle reports a 0..1024 volume and the trace a 0..64 one, so the
            // rounding has to happen here rather than in the comparison: reporting the
            // truncated value would disagree by one wherever the low four bits are at least
            // eight.
            volume: (((state.final_volume >> 6) + 8) / 16) as u16,
            period: state.final_period as u32,
            pan: state.final_pan as u16,
        }
    }
}

/// One envelope's live position, so both envelopes share `advance_envelope`.
struct EnvelopeCursor {
    tick: u16,
    point: u8,
    value: i16,
    delta: i16,
}

/// The body of `updateVolPanAutoVib`'s envelope handling, shared by both envelopes exactly
/// as FT2 duplicates it.
///
/// Returns the envelope's value in the point-`y`-shifted-left-by-eight domain.
fn advance_envelope(cursor: &mut EnvelopeCursor, envelope: &Envelope, key_off: bool) -> i32 {
    let length = envelope.points.len();
    let point_at = |index: usize| -> EnvelopePoint {
        envelope.points.get(index).copied().unwrap_or(EnvelopePoint { tick: 0, value: 0 })
    };
    let mut interpolated = false;
    let mut position = cursor.point as usize;
    let mut value = 0i32;

    cursor.tick = cursor.tick.wrapping_add(1);

    if cursor.tick == point_at(position).tick {
        cursor.value = (point_at(position).value as i8 as i16) << 8;

        position += 1;
        if let Some(span) = envelope.loop_span {
            position -= 1;
            if position == span.end as usize {
                let sustained = envelope.sustain.is_some_and(|sustain| position == sustain.start as usize);
                if !sustained || !key_off {
                    position = span.start as usize;
                    cursor.tick = point_at(position).tick;
                    cursor.value = (point_at(position).value as i8 as i16) << 8;
                }
            }
            position += 1;
        }

        if position < length {
            let mut interpolate = true;
            if let Some(sustain) = envelope.sustain
                && !key_off
                && position.checked_sub(1) == Some(sustain.start as usize)
            {
                position -= 1;
                cursor.delta = 0;
                interpolate = false;
            }
            if interpolate {
                cursor.point = position as u8;
                let previous = point_at(position - 1);
                let next = point_at(position);
                let span = next.tick as i32 - previous.tick as i32;
                if span > 0 {
                    let difference = (next.value as i8 as i32 - previous.value as i8 as i32) as i8 as i32;
                    cursor.delta = ((difference << 8) / span) as i16;
                    value = cursor.value as i32;
                    interpolated = true;
                } else {
                    cursor.delta = 0;
                }
            }
        } else {
            cursor.delta = 0;
        }
    }

    if !interpolated {
        cursor.value = cursor.value.wrapping_add(cursor.delta);
        value = cursor.value as i32;
        // FT2 tests the high byte of the 16-bit value as unsigned, so a value that ran
        // past 64 is pinned at 64 and one that ran below zero is pinned at zero.
        let high = ((value >> 8) & 0xFF) as u8;
        if high > 64 {
            value = if high <= 160 { ENVELOPE_UNITY } else { 0 };
            cursor.delta = 0;
        }
    }

    value
}

/// `setEnvelopePos`'s search for the point a tick lands in.
///
/// Returns the point index, the per-tick delta and the interpolated value, in the same
/// domain [`advance_envelope`] uses.
fn seek_envelope(envelope: &Envelope, parameter: u8) -> (u8, i16, i16) {
    let length = envelope.points.len();
    let point_at = |index: usize| -> EnvelopePoint {
        envelope.points.get(index).copied().unwrap_or(EnvelopePoint { tick: 0, value: 0 })
    };
    let mut point = 0i32;
    let mut update = true;
    let mut tick = parameter as i32;
    let mut delta = 0i16;
    let mut value = 0i16;

    if length > 1 {
        point += 1;
        for _ in 0..length - 1 {
            if tick < point_at(point as usize).tick as i32 {
                point -= 1;
                tick -= point_at(point as usize).tick as i32;
                if tick == 0 {
                    update = false;
                    break;
                }
                let first = point_at(point as usize);
                let second = point_at(point as usize + 1);
                let span = second.tick as i32 - first.tick as i32;
                if span <= 0 {
                    update = true;
                    break;
                }
                let difference = (second.value as i8 as i32 - first.value as i8 as i32) as i8 as i32;
                delta = ((difference << 8) / span) as i16;
                value = (((first.value as i8 as i32) << 8) + delta as i32 * (tick - 1)) as i16;
                point += 1;
                update = false;
                break;
            }
            point += 1;
        }
        if update {
            point -= 1;
        }
    }

    if update {
        delta = 0;
        value = (point_at(core::cmp::max(point, 0) as usize).value as i8 as i16) << 8;
    }
    if point >= length as i32 {
        point = core::cmp::max(length as i32 - 1, 0);
    }
    (core::cmp::max(point, 0) as u8, delta, value)
}

fn volume_to_byte(volume: U0F16) -> u8 { ((volume.to_bits() as u32 * 64 + 32_767) / 65_535) as u8 }

/// The loader stores an XM sample's pan byte as `bipolar_from_ratio(pan - 128, 128)`.
/// That scale is `I1F15::MAX`, not `2^15`, and it rounds to nearest, so the inverse has to
/// round to nearest against the same 32767 — dividing by 256 loses both ends of the byte
/// range, which is exactly where a hard-panned XM sample sits.
fn pan_to_byte(pan: Option<I1F15>) -> u8 {
    match pan {
        Some(pan) => {
            let bits = pan.to_bits() as i32;
            let half = if bits < 0 { -(I1F15_SCALE / 2) } else { I1F15_SCALE / 2 };
            ((bits * 128 + half) / I1F15_SCALE + 128).clamp(0, 255) as u8
        }
        None => 128,
    }
}

/// The full-scale numerator `bipolar_from_ratio` converts against.
const I1F15_SCALE: i32 = 32_767;

fn pan_from_byte(pan: u8) -> I1F15 { starplayer_core::fixed::bipolar_from_ratio(pan as i32 - 128, 128) }

fn volume_from_bits(volume_bits: u32) -> U0F16 { U0F16::from_bits(volume_bits.min(65_535) as u16) }

fn sample_region(sample: &starplayer_model::SampleIndex) -> SampleRegion {
    match sample.loop_mode() {
        LoopMode::Forward => LoopSpan::new(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::PingPong => LoopSpan::ping_pong(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::None => SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames()),
    }
}

impl TrackerProcessor for XmProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        context.report_global_volume(starplayer_core::fixed::unit_from_ratio(self.global_volume as u32, 64));
        let mut outcome = context.outcome();
        self.flow.begin_row();

        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        for channel_index in 0..self.channels.len() {
            if channel_index < channel_count {
                let start = channel_index * CELL_BYTES;
                let cell = XmCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])).unwrap_or(XmCell::EMPTY);
                self.get_new_note(context, channel_index, cell, &mut outcome, row.row);
            }
            self.update_volume_pan_auto_vibrato(channel_index);
            self.flush_channel(context, channel_index);
        }
        outcome.jump = self.flow.jump();
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        context.report_global_volume(starplayer_core::fixed::unit_from_ratio(self.global_volume as u32, 64));
        let outcome = context.outcome();
        let speed = context.row_clock.speed;
        // FT2's counter restarts with each pattern-delay repeat, where the engine's
        // `tick_in_row` is absolute across them.
        let tick = match speed {
            0 => 0,
            speed => context.row_clock.tick_in_row % speed as u16,
        };

        for channel_index in 0..self.channels.len() {
            self.handle_effects_tick_non_zero(context, channel_index, tick, speed);
            self.update_volume_pan_auto_vibrato(channel_index);
            self.flush_channel(context, channel_index);
        }
        #[cfg(feature = "trace")]
        self.report_trace_channels(context);
        outcome
    }

    /// Restore the state a fresh processor would have. Nothing here allocates: the channel
    /// array keeps its box and each entry is overwritten in place.
    fn reset(&mut self) {
        self.global_volume = 64;
        for channel in self.channels.iter_mut() {
            *channel = XmChannel::new();
        }
        self.flow.reset();
    }
}

/// How many voices an XM module of `channel_count` channels wants in the pool.
///
/// One per pattern channel: FastTracker 2 has no New Note Actions and never detaches a
/// voice, so a channel sounds exactly one note at a time.
pub const fn recommended_voice_capacity(channel_count: usize) -> usize { channel_count }

/// Build the public XM sequencer with the header's speed, tempo and restart position.
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, XmProcessor, XmPatternData> {
    let settings = sequencer_settings(&module, sample_rate_hz);
    PatternSequencer::new(tempo_model, XmPatternData(Arc::clone(&module)), XmProcessor::new(module, sample_rate_hz), settings)
}

/// Build the public XM sequencer under an explicit [`QuirkSelection`].
pub fn sequencer_with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> PatternSequencer<TempoModelId, XmProcessor, XmPatternData> {
    let resolved = quirks.resolve(module.header().dialect);
    let settings = sequencer_settings(&module, sample_rate_hz);
    let processor = XmProcessor::with_quirks(Arc::clone(&module), sample_rate_hz, QuirkSelection::Override(resolved));
    PatternSequencer::new(resolved.tempo_model, XmPatternData(module), processor, settings)
}

fn sequencer_settings(module: &Module, sample_rate_hz: u32) -> SequencerSettings {
    SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: XmFormatExtra::from_header(module.header()).restart_position,
        end_of_song: EndOfSongPolicy::Loop,
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;
    use starplayer_core::{Frame, RowClock};
    use starplayer_engine::{ChannelTable, Jump, SongPosition};
    use starplayer_mixer::VoicePool;
    use starplayer_model::{
        AutoVibrato, AutoVibratoWaveform, EnvelopeSpan, InstrumentDef, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader, NOTE_MAP_LENGTH, SampleSpec,
    };

    const ROWS: u16 = 4;
    const CHANNELS: u8 = 2;

    /// A two-channel linear-frequency XM with one instrument, one 64-frame looping sample
    /// on every note, and a three-point volume envelope that sustains on its middle point.
    fn module(volume_envelope: Option<Envelope>, panning_envelope: Option<Envelope>, fadeout: u16, auto_vibrato: AutoVibrato) -> Arc<Module> {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(
            &[0i16; 64],
            SampleSpec { auto_vibrato, ..SampleSpec::one_shot("pcm").with_forward_loop(0, 64) },
        ).expect("one PCM sample");
        let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
        for entry in note_sample_map.iter_mut() { *entry = sample.0 + 1; }
        builder.add_instrument(InstrumentDef {
            note_sample_map,
            volume_envelope,
            panning_envelope,
            fadeout,
            ..InstrumentDef::default()
        }).expect("one instrument");
        builder.add_pattern(&vec![0u8; ROWS as usize * CHANNELS as usize * CELL_BYTES], ROWS, CHANNELS).expect("one pattern");
        builder.set_orders(&[0]);
        builder.set_header(ModuleHeader {
            flags: ModuleFlags { linear_slides: true, ..ModuleFlags::default() },
            ..ModuleHeader::new(ModuleFormat::Xm, CHANNELS)
        });
        Arc::new(builder.build().expect("every invariant holds"))
    }

    fn plain_module() -> Arc<Module> { module(None, None, 0, AutoVibrato::default()) }

    fn processor() -> XmProcessor { XmProcessor::new(plain_module(), 44_100) }

    /// Drive one row of cells through a processor and give the test the context back.
    struct Harness {
        processor: XmProcessor,
        voices: VoicePool,
        channels: ChannelTable,
        row_clock: RowClock,
        row: u16,
    }

    impl Harness {
        fn new(processor: XmProcessor) -> Harness {
            Harness {
                processor,
                voices: VoicePool::new(CHANNELS as usize),
                channels: ChannelTable::new(CHANNELS as usize),
                row_clock: RowClock::new(6),
                row: 0,
            }
        }

        fn row(&mut self, cells: &[XmCell]) -> TickOutcome {
            let mut bytes = Vec::new();
            for cell in cells { bytes.extend_from_slice(&cell.to_bytes()); }
            self.row_clock.start_row(self.row_clock.speed);
            let mut context = TickContext::new(Frame::ZERO, &mut self.voices, &mut self.channels, self.row_clock, SongPosition::default(), 125);
            let outcome = self.processor.row(&mut context, RowRef { order: 0, pattern: 0, row: self.row, bytes: &bytes });
            self.row_clock.set_speed(outcome.speed);
            self.row_clock.set_pattern_delay(outcome.pattern_delay);
            outcome
        }

        fn tick(&mut self) -> TickOutcome {
            self.row_clock.advance();
            let mut context = TickContext::new(Frame::ZERO, &mut self.voices, &mut self.channels, self.row_clock, SongPosition::default(), 125);
            self.processor.tick(&mut context)
        }

        fn channel(&self, index: usize) -> &XmChannel { &self.processor.channels[index] }
    }

    fn note(note: u8, instrument: u8) -> XmCell { XmCell { note, instrument, ..XmCell::EMPTY } }
    fn effect(effect: u8, parameter: u8) -> XmCell { XmCell { effect, parameter, ..XmCell::EMPTY } }

    #[test]
    fn a_note_takes_its_period_from_fast_tracker_twos_own_linear_table() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_period, 4608, "C-4 with no finetune is FT2's 4608");
        assert_eq!(harness.channel(0).final_period, 4608);
        assert_eq!(harness.channel(0).sounding_note, 48, "the trace note is zero-based from C-0");

        let _ = harness.row(&[note(1, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_period, 7680, "C-0 is the table's lowest playable entry");
        let _ = harness.row(&[note(96, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_period, 4608 - 47 * 64, "B-7 is 47 semitones above C-4");
    }

    #[test]
    fn the_linear_step_is_the_shared_table_read_the_way_fast_tracker_two_reads_it() {
        let processor = processor();
        // C-4 must play at exactly the reference rate.
        let step = processor.step_for_period(4608);
        assert_eq!(step, Step::from_ratio(8_363, 44_100), "C-4 is 8363 Hz");
        // One octave down doubles the period and halves the step.
        let octave_down = processor.step_for_period(4608 + 768);
        assert!(octave_down.to_bits().abs_diff(step.to_bits() / 2) <= 2, "an octave is a factor of two");
        assert_eq!(processor.step_for_period(0), Step::ZERO, "FT2: a period of zero is zero hertz");
        // D43: a period past 9216 underflows FT2's `(9216 - period) & 0xFFFF` into silence.
        assert_eq!(processor.step_for_period(9_217).to_bits(), 0, "FT2's period wraparound is silence");
    }

    #[test]
    fn amiga_mode_reads_the_transcribed_table_and_the_formats_own_numerator() {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[0i16; 8], SampleSpec::one_shot("pcm")).expect("one sample");
        let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
        for entry in note_sample_map.iter_mut() { *entry = sample.0 + 1; }
        builder.add_instrument(InstrumentDef { note_sample_map, ..InstrumentDef::default() }).expect("one instrument");
        builder.add_pattern(&vec![0u8; ROWS as usize * CHANNELS as usize * CELL_BYTES], ROWS, CHANNELS).expect("one pattern");
        builder.set_orders(&[0]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Xm, CHANNELS));
        let module = Arc::new(builder.build().expect("valid module"));
        let mut harness = Harness::new(XmProcessor::new(module, 44_100));

        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_period, 1712, "FT2's Amiga C-4 is 1712, four times ProTracker's");
        assert_eq!(harness.processor.step_for_period(1712), Step::from_ratio(8_363 * 1712, 1712 * 44_100));
    }

    #[test]
    fn an_instrument_column_with_no_note_resets_the_volume_without_retriggering() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let voice = harness.channels.foreground(ChannelId(0)).expect("the note started a voice");

        let _ = harness.row(&[XmCell { volume: 0x20, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 0x10, "the volume column set volume 16");
        let _ = harness.row(&[XmCell { instrument: 1, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 64, "an instrument column restores the sample's own volume");
        assert_eq!(harness.channels.foreground(ChannelId(0)), Some(voice), "and does not retrigger the voice");
    }

    #[test]
    fn an_out_of_range_transposed_note_keeps_the_note_that_is_sounding() {
        let mut builder = ModuleBuilder::new();
        let low = builder.add_sample(&[0i16; 8], SampleSpec::one_shot("low")).expect("one sample");
        let high = builder.add_sample(&[0i16; 8], SampleSpec { relative_note: 96, ..SampleSpec::one_shot("high") }).expect("a transposed sample");
        let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
        for entry in note_sample_map.iter_mut() { *entry = low.0 + 1; }
        note_sample_map[60] = high.0 + 1;
        builder.add_instrument(InstrumentDef { note_sample_map, ..InstrumentDef::default() }).expect("one instrument");
        builder.add_pattern(&vec![0u8; ROWS as usize * CHANNELS as usize * CELL_BYTES], ROWS, CHANNELS).expect("one pattern");
        builder.set_orders(&[0]);
        builder.set_header(ModuleHeader { flags: ModuleFlags { linear_slides: true, ..ModuleFlags::default() }, ..ModuleHeader::new(ModuleFormat::Xm, CHANNELS) });
        let mut harness = Harness::new(XmProcessor::new(Arc::new(builder.build().expect("valid module")), 44_100));

        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).sounding_note, 48);
        // Note 61 maps to the sample transposed 96 semitones up: 61 + 96 = 157, past B-9.
        let _ = harness.row(&[note(61, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).sample, Some(high), "the sample and instrument still move");
        assert_eq!(harness.channel(0).sounding_note, 48, "but the note the voice plays does not");
    }

    #[test]
    fn a_note_with_no_usable_sample_cuts_the_channel() {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[0i16; 8], SampleSpec::one_shot("pcm")).expect("one sample");
        let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
        note_sample_map[48] = sample.0 + 1;
        builder.add_instrument(InstrumentDef { note_sample_map, ..InstrumentDef::default() }).expect("one instrument");
        builder.add_pattern(&vec![0u8; ROWS as usize * CHANNELS as usize * CELL_BYTES], ROWS, CHANNELS).expect("one pattern");
        builder.set_orders(&[0]);
        builder.set_header(ModuleHeader { flags: ModuleFlags { linear_slides: true, ..ModuleFlags::default() }, ..ModuleHeader::new(ModuleFormat::Xm, CHANNELS) });
        let mut harness = Harness::new(XmProcessor::new(Arc::new(builder.build().expect("valid module")), 44_100));

        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert!(harness.channels.is_sounding(ChannelId(0), &harness.voices), "note 49 has a sample");
        let _ = harness.row(&[note(50, 1), XmCell::EMPTY]);
        assert!(!harness.channels.is_sounding(ChannelId(0), &harness.voices), "note 50 maps to nothing, and FT2 cuts");
        assert_eq!(harness.channel(0).sample_volume, 0, "the placeholder instrument's volume is zero");
        assert_eq!(harness.channel(0).sample_pan, 128, "and its pan is centred");
    }

    #[test]
    fn a_key_off_without_a_volume_envelope_silences_the_channel_and_starts_the_fadeout() {
        let mut harness = Harness::new(XmProcessor::new(module(None, None, 512, AutoVibrato::default()), 44_100));
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).fadeout_volume, FADEOUT_MAX);

        let _ = harness.row(&[note(NOTE_KEY_OFF, 0), XmCell::EMPTY]);
        assert!(harness.channel(0).key_off);
        assert_eq!(harness.channel(0).real_volume, 0, "no volume envelope means the key-off cuts the volume");
        assert_eq!(harness.channel(0).fadeout_volume, FADEOUT_MAX - 512, "and the fadeout starts on the same tick");
        assert!(harness.channels.is_sounding(ChannelId(0), &harness.voices), "a key-off never stops the sample");
    }

    #[test]
    fn a_key_off_with_a_volume_envelope_releases_the_sustain_instead_of_cutting() {
        let envelope = Envelope {
            points: vec![EnvelopePoint { tick: 0, value: 64 }, EnvelopePoint { tick: 8, value: 0 }].into_boxed_slice(),
            sustain: Some(EnvelopeSpan { start: 0, end: 0 }),
            loop_span: None,
            carry: false,
        };
        let mut harness = Harness::new(XmProcessor::new(module(Some(envelope), None, 256, AutoVibrato::default()), 44_100));
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        for _ in 0..5 { let _ = harness.tick(); }
        assert_eq!(harness.channel(0).final_volume, 65_536, "the envelope holds at its sustain point");

        let _ = harness.row(&[note(NOTE_KEY_OFF, 0), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 64, "the channel volume itself is untouched");
        assert!(harness.channel(0).final_volume < 65_536, "but the envelope has left sustain and the fadeout has begun");
    }

    #[test]
    fn every_pitch_effect_family_keeps_its_own_memory() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let _ = harness.row(&[effect(1, 0x10), XmCell::EMPTY]);
        let _ = harness.tick();
        let _ = harness.row(&[effect(2, 0x20), XmCell::EMPTY]);
        let _ = harness.tick();
        let _ = harness.row(&[effect(0x0E, 0x13), XmCell::EMPTY]);
        let _ = harness.row(&[effect(0x0E, 0x24), XmCell::EMPTY]);
        let _ = harness.row(&[effect(0x21, 0x15), XmCell::EMPTY]);
        let _ = harness.row(&[effect(0x21, 0x26), XmCell::EMPTY]);

        let state = harness.channel(0);
        assert_eq!(state.pitch_up_memory, 0x10);
        assert_eq!(state.pitch_down_memory, 0x20);
        assert_eq!(state.fine_pitch_up_memory, 3);
        assert_eq!(state.fine_pitch_down_memory, 4);
        assert_eq!(state.extra_fine_pitch_up_memory, 5);
        assert_eq!(state.extra_fine_pitch_down_memory, 6);
    }

    #[test]
    fn the_volume_column_covers_every_one_of_its_own_effects() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);

        let _ = harness.row(&[XmCell { volume: 0x30, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 32, "$10..$50 sets a volume");
        let _ = harness.row(&[XmCell { volume: 0x84, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 28, "$8x is an instant fine slide down");
        let _ = harness.row(&[XmCell { volume: 0x92, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 30, "$9x is an instant fine slide up");
        let _ = harness.row(&[XmCell { volume: 0x63, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).real_volume, 30, "$6x does nothing on tick zero");
        let _ = harness.tick();
        assert_eq!(harness.channel(0).real_volume, 27, "and slides down on every later tick");
        let _ = harness.row(&[XmCell { volume: 0x72, ..XmCell::EMPTY }, XmCell::EMPTY]);
        let _ = harness.tick();
        assert_eq!(harness.channel(0).real_volume, 29, "$7x slides up");
        let _ = harness.row(&[XmCell { volume: 0xC8, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).out_pan, 0x80, "$Cx sets the pan nibble into the high half of the byte");
        let _ = harness.row(&[XmCell { volume: 0xA4, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).vibrato_speed, 16, "$Ax sets the vibrato speed, times four");
    }

    #[test]
    fn a_volume_column_pan_slide_left_of_zero_sets_the_pan_to_zero() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let _ = harness.row(&[XmCell { volume: 0xD0, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).out_pan, 128, "nothing happens on tick zero");
        let _ = harness.tick();
        assert_eq!(harness.channel(0).out_pan, 0, "FT2's own bug: a pan slide left of zero jumps to zero");
    }

    #[test]
    fn the_arpeggio_tick_counter_runs_off_fast_tracker_twos_short_table() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        // `047` at speed 6: FT2 indexes `arpeggioTab[speed - tick_in_repeat]`, so the row
        // plays base, +7, +4, base, +7, +4 rather than the base/+4/+7 a `tick % 3` gives.
        let _ = harness.row(&[effect(0, 0x47), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).out_period, 4608, "tick zero is always the base note");
        let mut periods = Vec::new();
        for _ in 0..5 { let _ = harness.tick(); periods.push(harness.channel(0).out_period); }
        assert_eq!(periods, vec![4608 - 7 * 64, 4608 - 4 * 64, 4608, 4608 - 7 * 64, 4608 - 4 * 64]);
    }

    #[test]
    fn a_speed_below_thirty_two_is_a_speed_and_anything_else_is_a_tempo() {
        let mut harness = Harness::new(processor());
        let outcome = harness.row(&[effect(0x0F, 4), XmCell::EMPTY]);
        assert_eq!(outcome.speed, 4);
        assert_eq!(outcome.tempo_bpm, 125, "and the tempo is untouched");
        let outcome = harness.row(&[effect(0x0F, 200), XmCell::EMPTY]);
        assert_eq!(outcome.tempo_bpm, 200);
        assert_eq!(outcome.speed, 4, "and the speed is untouched");
    }

    #[test]
    fn a_pattern_delay_takes_the_rightmost_channels_value_even_when_it_is_zero() {
        let mut harness = Harness::new(processor());
        let outcome = harness.row(&[effect(0x0E, 0xE3), effect(0x0E, 0xE0)]);
        assert_eq!(outcome.pattern_delay, 0, "FT2 lets the rightmost EEx win, EE0 included");
        let outcome = harness.row(&[effect(0x0E, 0xE0), effect(0x0E, 0xE3)]);
        assert_eq!(outcome.pattern_delay, 3);
    }

    #[test]
    fn a_position_jump_or_break_on_the_same_row_as_a_pattern_loop_wins_over_it() {
        let mut harness = Harness::new(processor());
        harness.row = 4;
        let _ = harness.row(&[effect(0x0E, 0x60), XmCell::EMPTY]);
        harness.row = 8;
        // The loop first, then the break: FT2 writes `pBreakPos` twice and the break wins.
        let outcome = harness.row(&[effect(0x0E, 0x62), effect(0x0D, 0x12)]);
        assert_eq!(outcome.jump, Some(Jump::break_to_row(12)), "Dxx is decimal, so $12 is row 12");

        // The break first, then the loop: FT2's `patternLoop` overwrites the destination
        // row while the jump flag survives — `shared_break`. A fresh processor, because the
        // loop above has already spent its counter.
        // The loop target is per channel, so the `E60` goes on the same channel as the
        // `E6x` that reads it.
        let mut harness = Harness::new(processor());
        harness.row = 4;
        let _ = harness.row(&[XmCell::EMPTY, effect(0x0E, 0x60)]);
        harness.row = 8;
        let outcome = harness.row(&[effect(0x0D, 0x12), effect(0x0E, 0x62)]);
        assert_eq!(outcome.jump, Some(Jump::break_to_row(4)), "the loop target replaces the break's row");
    }

    #[test]
    fn a_bare_pattern_loop_jumps_inside_its_own_pattern_and_counts_down() {
        let mut harness = Harness::new(processor());
        harness.row = 2;
        let _ = harness.row(&[effect(0x0E, 0x60), XmCell::EMPTY]);
        harness.row = 6;
        for expected in [true, true, false] {
            let outcome = harness.row(&[effect(0x0E, 0x62), XmCell::EMPTY]);
            assert_eq!(outcome.jump == Some(Jump::within_pattern_to_row(2)), expected);
        }
    }

    #[test]
    fn a_note_delay_fires_once_per_pattern_delay_repeat() {
        let mut harness = Harness::new(processor());
        harness.row_clock.start_row(4);
        let _ = harness.row(&[XmCell { note: 49, instrument: 1, volume: 0, effect: 0x0E, parameter: 0xD2 }, XmCell::EMPTY]);
        assert!(!harness.channels.is_sounding(ChannelId(0), &harness.voices), "the row's tick zero starts nothing");
        let _ = harness.tick();
        assert!(!harness.channels.is_sounding(ChannelId(0), &harness.voices));
        let _ = harness.tick();
        assert!(harness.channels.is_sounding(ChannelId(0), &harness.voices), "tick two is the delay's own tick");
    }

    #[test]
    fn a_tone_portamento_keeps_the_channels_own_finetune_and_relative_note() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let _ = harness.row(&[XmCell { note: 50, effect: 3, parameter: 0x08, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).portamento_target_period, 4608 - 64, "C#4 is one semitone above C-4");
        assert_eq!(harness.channel(0).portamento_speed, 8 * 4, "FT2 stores the rate already multiplied by four");
        assert_eq!(harness.channel(0).real_period, 4608, "and nothing moves on tick zero");
        let _ = harness.tick();
        assert_eq!(harness.channel(0).real_period, 4608 - 32);
        let _ = harness.tick();
        assert_eq!(harness.channel(0).real_period, 4608 - 64, "the slide stops at the target");
        assert_eq!(harness.channel(0).portamento_direction, 1);
    }

    #[test]
    fn the_volume_column_portamento_ignores_the_effect_columns_parameter() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let _ = harness.row(&[XmCell { note: 50, instrument: 0, volume: 0xF2, effect: 3, parameter: 0x0F }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).portamento_speed, 2 * 16 * 4, "`M2` is 2 << 4, times four");
    }

    #[test]
    fn a_tremor_phase_lasts_the_nibble_plus_one_ticks() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let _ = harness.row(&[effect(0x1D, 0x12), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).out_volume, 64, "the counter is untouched on tick zero");
        let mut volumes = Vec::new();
        for _ in 0..6 { let _ = harness.tick(); volumes.push(harness.channel(0).out_volume); }
        assert_eq!(volumes, vec![64, 64, 0, 0, 0, 64], "T12 is two ticks on and three off");
    }

    #[test]
    fn global_volume_scales_every_channel_and_slides_with_its_own_memory() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.processor.global_volume(), 64);
        let _ = harness.row(&[effect(0x10, 32), XmCell::EMPTY]);
        assert_eq!(harness.processor.global_volume(), 32);
        assert_eq!(harness.channel(0).final_volume, 32_768, "half of full scale");
        let _ = harness.row(&[effect(0x11, 0x10), XmCell::EMPTY]);
        let _ = harness.tick();
        assert_eq!(harness.processor.global_volume(), 33);
        let _ = harness.row(&[effect(0x11, 0), XmCell::EMPTY]);
        let _ = harness.tick();
        assert_eq!(harness.processor.global_volume(), 34, "Hxy with no parameter recalls its memory");
    }

    #[test]
    fn an_auto_vibrato_bends_the_pitch_up_on_the_very_first_tick() {
        // D46: FT2 advances the phase *before* reading a sine table whose first entries are
        // negative, so the period falls immediately.
        let vibrato = AutoVibrato { waveform: AutoVibratoWaveform::Sine, sweep: 0, depth: 15, rate: 32 };
        let mut harness = Harness::new(XmProcessor::new(module(None, None, 0, vibrato), 44_100));
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert!(harness.channel(0).final_period < 4608, "the first tick already bends the pitch up");
        assert_eq!(harness.channel(0).out_period, 4608, "and the period the slides own is untouched");
    }

    #[test]
    fn an_auto_vibrato_sweep_ramps_the_depth_in_rather_than_starting_at_it() {
        let vibrato = AutoVibrato { waveform: AutoVibratoWaveform::Sine, sweep: 64, depth: 15, rate: 32 };
        let mut harness = Harness::new(XmProcessor::new(module(None, None, 0, vibrato), 44_100));
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        assert_eq!(harness.channel(0).auto_vibrato_sweep, (15 << 8) / 64);
        let first = 4608 - harness.channel(0).final_period;
        let _ = harness.tick();
        let _ = harness.tick();
        let _ = harness.tick();
        assert!(4608 - harness.channel(0).final_period > first || harness.channel(0).auto_vibrato_amplitude > 0, "the swept amplitude grows");
    }

    #[test]
    fn a_reset_restores_a_fresh_processor_without_allocating() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), effect(0x10, 8)]);
        assert_ne!(harness.channel(0).real_period, 0);
        assert_eq!(harness.processor.global_volume(), 8);

        harness.processor.reset();
        assert_eq!(harness.processor.global_volume(), 64);
        for channel in harness.processor.channels() {
            assert_eq!(*channel, XmChannel::new());
        }
        assert_eq!(harness.processor.pattern_flow().jump(), None);
    }

    #[test]
    fn the_trace_reports_the_sounding_voice_rather_than_the_latched_columns() {
        let mut harness = Harness::new(processor());
        let _ = harness.row(&[note(49, 1), XmCell::EMPTY]);
        let state = harness.processor.trace_channel_state(0);
        assert_eq!(state.note, Some(48));
        assert_eq!(state.instrument, 1);
        assert_eq!(state.sample, 1);
        assert_eq!(state.volume, 64, "the 0..64 volume is rounded from the 0..1024 domain, not truncated");
        assert_eq!(state.period, 4608);
        assert_eq!(state.pan, 128);

        // An instrument number FT2 keeps but cannot resolve moves `instrument_number` and
        // nothing else the trace reports.
        let _ = harness.row(&[XmCell { instrument: 120, ..XmCell::EMPTY }, XmCell::EMPTY]);
        assert_eq!(harness.channel(0).instrument_number, 120);
        assert_eq!(harness.processor.trace_channel_state(0).instrument, 1, "the sounding instrument has not changed");
    }

    #[test]
    fn the_recommended_voice_capacity_is_one_per_channel() {
        assert_eq!(recommended_voice_capacity(8), 8);
        assert_eq!(processor().recommended_voice_capacity(32), 32, "XM never detaches a voice");
    }

    #[test]
    fn the_sequencer_takes_its_restart_order_from_the_files_own_restart_position() {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&vec![0u8; ROWS as usize * CHANNELS as usize * CELL_BYTES], ROWS, CHANNELS).expect("one pattern");
        builder.set_orders(&[0, 0, 0]);
        builder.set_header(ModuleHeader {
            format_extra: XmFormatExtra { restart_position: 2, flags: 1 }.encode(),
            ..ModuleHeader::new(ModuleFormat::Xm, CHANNELS)
        });
        let module = Arc::new(builder.build().expect("valid module"));
        let settings = sequencer_settings(&module, 44_100);
        assert_eq!(settings.restart_order, 2);
        assert_eq!(settings.end_of_song, EndOfSongPolicy::Loop);
    }
}

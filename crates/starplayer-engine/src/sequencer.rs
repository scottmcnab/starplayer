//! [`PatternSequencer`] — order list → pattern → row → tick, and nothing about effects.
//!
//! This is the tracker half of the engine's timing: it decides *when* each tick happens
//! and *where in the song* it is, and it hands the tick to a format-supplied
//! [`TrackerProcessor`] that decides what the tick means. The split is deliberate. Timing
//! is shared by MOD, S3M, MTM, XM and IT; effect semantics are exactly what must **not**
//! be shared (design goal 7 — the original converted MOD and MTM to S3M before the player
//! saw them, and that is why its MOD playback was inaccurate).
//!
//! # Rule 1, in the type system
//!
//! > Never cache the next-event frame across a dispatch. `Txx`, `Axx`, `SEx`, `SDx`, `Bxx`
//! > and `Cxx` all change *when the next tick is* from inside the tick just processed.
//! > — architecture §3.1 rule 1
//!
//! Three things make that hard to get wrong here rather than merely documented:
//!
//! 1. **While a tick is being processed, there is no next frame.** The sequencer's state
//!    machine moves to `Processing` on entry to [`EventSource::dispatch`], and
//!    [`EventSource::next_event_frame`] answers `None` in that state. A boundary computed
//!    before the tick finished cannot be observed, because it does not exist yet.
//! 2. **The boundary is a function of the [`TickOutcome`]**, which only the processor can
//!    produce and which the sequencer must consume before it can leave `Processing`. It is
//!    `#[must_use]`, it carries the tempo and speed *in effect at the end of the tick*, and
//!    it is the only argument to the one call site of
//!    [`FrameClock::advance_tick`](starplayer_core::FrameClock::advance_tick).
//! 3. **[`FrameClock`] itself has no setter** (task A2): the only way to learn where the
//!    next tick lands is to commit the clock to it, so a stale copy cannot be kept.
//!
//! # What is *not* here
//!
//! Effect interpretation, per-channel effect memories, period arithmetic — all of that is
//! the format crate's ([`TrackerProcessor`], implemented for S3M by task B4). Pattern
//! *decoding* is not here either: the sequencer hands the processor the row's packed bytes
//! in the format's own encoding and never looks inside them.

use starplayer_core::{ChannelId, DirtyBits, Frame, FrameClock, Note, RowAdvance, RowClock, TempoModel, U0F16, VoiceId, VoiceParam, VoiceParams};
use starplayer_mixer::{SampleRegion, VoicePool, VoiceTag};

use crate::channel::ChannelTable;
use crate::source::{EngineContext, EventSource};

// ── the module's pattern data, abstractly ───────────────────────────────────────────

/// One entry in the order list.
///
/// S3M spells the last two as the bytes 254 and 255; MOD has neither and MTM has neither,
/// so the loader maps its own convention onto this and the sequencer never learns which
/// format it is playing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderEntry {
    /// Play this pattern.
    Pattern(u16),
    /// A marker to step over (S3M's `0xFE`). Not an end, not a pattern.
    Skip,
    /// End of song (S3M's `0xFF`). What happens next is the
    /// [`EndOfSongPolicy`](EndOfSongPolicy).
    End,
}

/// Read-only access to a module's order list and packed pattern bytes.
///
/// # Why the sequencer never sees a cell
///
/// The unit of access is a **row's raw bytes, in the format's own encoding**, not a decoded
/// cell and not a per-channel slice. Two reasons, and both are load-bearing:
///
/// * S3M pattern rows are variable-length packed data with a channel-present bitmask, so
///   "give me channel 7 of row 12" is not a random access — it is a scan of the row, which
///   is exactly what `FindPatternPos` (`STARPLAY/S3MLIB.ASM` ~2690) does. Making the
///   sequencer ask per channel would either force every format to unpack into a fixed
///   array first, or push the scan into an inner loop.
/// * A shared decoded cell type is the mistake this project exists not to repeat. MOD's
///   effect column, S3M's, MTM's, XM's and IT's do not mean the same things, and lowering
///   them into one shape is how the original lost MOD accuracy.
///
/// So the format crate implements this trait over whatever its loader put in the module
/// blob, and its [`TrackerProcessor`] is the only code that knows what the bytes mean.
pub trait PatternData {
    /// How many entries the order list has.
    fn order_count(&self) -> u16;

    /// The order-list entry at `order`, or `None` if the index is past the end.
    fn order(&self, order: u16) -> Option<OrderEntry>;

    /// How many channels the module's patterns have.
    fn channel_count(&self) -> u8;

    /// How many rows `pattern` has, or `None` if there is no such pattern.
    fn rows_in_pattern(&self, pattern: u16) -> Option<u16>;

    /// The packed bytes of one row, or `None` if the pattern or row does not exist.
    ///
    /// A `None` here ends the song rather than panicking: a fuzzed module reaches this
    /// function, and `render()` may not panic.
    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]>;
}

// ── where we are ────────────────────────────────────────────────────────────────────

/// Where the sequencer is in the song.
///
/// The original keeps a second, snapshotted copy of exactly these fields (`_MActualRow` /
/// `_MActualPos` / `_MActualPatt`) "so the UI sees the row that is currently sounding
/// rather than the one being parsed". That snapshot is B6's job; this is the live cursor.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SongPosition {
    /// Index into the order list.
    pub order: u16,
    /// The pattern that order entry names.
    pub pattern: u16,
    /// Row within that pattern.
    pub row: u16,
}

/// One row of one pattern, as handed to a [`TrackerProcessor`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RowRef<'data> {
    /// Index into the order list.
    pub order: u16,
    /// The pattern this row belongs to.
    pub pattern: u16,
    /// Row within the pattern.
    pub row: u16,
    /// The row's packed bytes, in the format's own encoding.
    pub bytes: &'data [u8],
}

/// Where the song goes when the current row is over.
///
/// A jump is *requested* during a tick and *applied* when the row's whole tick budget —
/// including every pattern-delay repeat — has been spent. That is what makes `Cxx` on a
/// row with `SE2` break after the repeats rather than during them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Jump {
    /// Order-list index to continue at. `None` means "the next order".
    pub order: Option<u16>,
    /// Row to continue at. `None` means row 0.
    pub row: Option<u16>,
    /// Stay inside the current pattern instead of moving through the order list — `SBx`
    /// pattern loop. When set, `order` is ignored.
    pub within_pattern: bool,
}

impl Jump {
    /// `Bxx` — continue at an order-list index, from its first row.
    pub const fn to_order(order: u16) -> Jump { Jump { order: Some(order), row: None, within_pattern: false } }

    /// `Cxx` — break to a row of the *next* pattern in the order list.
    pub const fn break_to_row(row: u16) -> Jump { Jump { order: None, row: Some(row), within_pattern: false } }

    /// `Bxx` and `Cxx` on the same row: a named order *and* a named row.
    pub const fn to_order_row(order: u16, row: u16) -> Jump {
        Jump { order: Some(order), row: Some(row), within_pattern: false }
    }

    /// `SBx` — go back to a row of the pattern already playing.
    pub const fn within_pattern_to_row(row: u16) -> Jump {
        Jump { order: None, row: Some(row), within_pattern: true }
    }
}

// ── what a tick leaves in effect ────────────────────────────────────────────────────

/// What one tick of a format's effect processor left in effect.
///
/// **This is what makes rule 1 structural.** The sequencer cannot compute the next tick
/// boundary until it has one of these, because the boundary is
/// `frames_per_tick(tempo_bpm, speed)` from the values it carries — after `Txx` and `Axx`
/// have been applied, not before.
///
/// Build it with [`TickContext::outcome`] and change what the tick changed; constructing
/// one from scratch on a tick that did not set a tempo would silently reset the song's.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[must_use = "the sequencer computes the next tick boundary from this; dropping it loses a Txx or Axx"]
pub struct TickOutcome {
    /// Tempo in effect at the end of the tick. `Txx` writes it.
    pub tempo_bpm: u16,
    /// Ticks per row in effect at the end of the tick. `Axx` writes it.
    ///
    /// Applied to the row clock immediately, so a mid-row change lengthens or shortens the
    /// row in progress — see [`RowClock::set_speed`] for the wrap rule.
    pub speed: u8,
    /// Extra whole repeats of this row (`SEx` / `EEx`).
    ///
    /// Only read on the **first tick of a row**, because that is when the original loads
    /// `_MRowDelay`; reporting it on a later tick of the same row has no effect, and a
    /// processor therefore cannot accidentally cancel a delay it set on tick 0.
    pub pattern_delay: u8,
    /// Where to go when this row's budget is spent. The last jump requested during the row
    /// wins, which is what `Bxx` followed by `Cxx` on a later channel of the same row
    /// means.
    pub jump: Option<Jump>,
    /// End the song now. `SFF`-style stop markers and an unresolvable pattern set it.
    pub stop: bool,
}

/// What a [`TrackerProcessor`] is given for one tick.
///
/// Everything a tick may write to, and nothing else — in particular not the output buffer:
/// a processor says *what happens*, never *what it sounds like*.
///
/// # Telling the UI what happened
///
/// The `report_*` methods are the modern spelling of `ChannelData._CMDVal` / `_CMDData`,
/// the fields the original marked "for host program" (architecture §9,
/// `plans/reference/original-star-ui.md` §2.3). They are **no-ops unless the `telemetry`
/// feature is on**, and the feature is deliberately invisible in their signatures: a
/// format's effect processor calls them unconditionally and never grows a `cfg`.
pub struct TickContext<'engine> {
    /// The absolute output frame this tick lands on.
    pub frame: Frame,
    /// The global voice pool.
    pub voices: &'engine mut VoicePool,
    /// Channel-to-voice bindings. [`ChannelTable::trigger`] and [`ChannelTable::stop`] are
    /// how a note starts and ends.
    pub channels: &'engine mut ChannelTable,
    /// The row's tick budget and **absolute** tick index across pattern-delay repeats.
    /// `Qxy` retrigger, `Ixy` tremor, `SDx` note delay and `SCx` note cut all key off this.
    pub row_clock: RowClock,
    /// Where in the song this tick is.
    pub position: SongPosition,
    /// The tempo in effect as the tick begins.
    pub tempo_bpm: u16,
    /// Where the tick describes itself for the UI. Reached through the `report_*` methods
    /// rather than directly, so that a format crate never mentions the feature.
    #[cfg(feature = "telemetry")]
    telemetry: Option<&'engine mut starplayer_telemetry::TelemetryPublisher>,
    /// Per-tick diagnostic recorder. Reached only through the reporting/write helpers.
    #[cfg(feature = "trace")]
    trace: Option<&'engine mut crate::trace::TraceRecorder>,
}

/// Format-native state which cannot be reconstructed from [`VoiceParams`].
///
/// The type is always available so format processors call one stable API; the reporting
/// method and the value disappear after inlining when `trace` is disabled.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceChannelState {
    /// Linear note with C-0 as zero.
    pub note: Option<u8>,
    /// One-based instrument number, or zero.
    pub instrument: u16,
    /// One-based sample number, or zero.
    pub sample: u16,
    /// Format channel volume normalized to 0..64.
    pub volume: u16,
    /// Format-native integer period.
    pub period: u32,
    /// Pan normalized to 0..255.
    pub pan: u16,
}

impl<'engine> TickContext<'engine> {
    /// A tick context with no telemetry attached.
    ///
    /// The sequencer builds its own; this exists so a format crate's tests can drive a
    /// processor without one, under either feature set.
    pub fn new(
        frame: Frame,
        voices: &'engine mut VoicePool,
        channels: &'engine mut ChannelTable,
        row_clock: RowClock,
        position: SongPosition,
        tempo_bpm: u16,
    ) -> TickContext<'engine> {
        TickContext {
            frame,
            voices,
            channels,
            row_clock,
            position,
            tempo_bpm,
            #[cfg(feature = "telemetry")]
            telemetry: None,
            #[cfg(feature = "trace")]
            trace: None,
        }
    }

    /// The outcome of a tick that changed nothing about the timing.
    ///
    /// **Start every tick from this.** It carries forward the tempo, speed and pattern
    /// delay already in effect, so a processor only writes what its effects actually
    /// changed.
    pub const fn outcome(&self) -> TickOutcome {
        TickOutcome {
            tempo_bpm: self.tempo_bpm,
            speed: self.row_clock.speed,
            pattern_delay: self.row_clock.pattern_delay,
            jump: None,
            stop: false,
        }
    }

    /// Report the row's effect column for `channel` — the raw command code and parameter
    /// plus the English name from the format's own
    /// [`EffectNames`](starplayer_model::EffectNames) table.
    ///
    /// A no-op unless the `telemetry` feature is on. The name is resolved by the format
    /// crate because the same letter means different things in different formats and
    /// `starplayer-telemetry` may not depend on `starplayer-model`.
    pub fn report_effect(&mut self, channel: ChannelId, code: u8, param: u8, name: &'static str) {
        #[cfg(feature = "telemetry")]
        if let Some(telemetry) = self.telemetry.as_deref_mut() {
            telemetry.report_effect(channel, starplayer_telemetry::EffectDisplay { code, param, name });
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = (channel, code, param, name);
    }

    /// Report the row's note and instrument columns for `channel`, overriding what the
    /// sounding voice says.
    ///
    /// Only needed where the two differ — a note parsed but delayed by `SDx`, or a
    /// portamento target the voice has not reached. A no-op unless the `telemetry` feature
    /// is on. `None` leaves the previous value alone.
    pub fn report_note(&mut self, channel: ChannelId, note: Option<Note>, instrument: Option<u8>) {
        #[cfg(feature = "telemetry")]
        if let Some(telemetry) = self.telemetry.as_deref_mut() {
            telemetry.report_note(channel, note, instrument);
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = (channel, note, instrument);
    }

    /// Report the module's global volume (`Vxx`). A no-op unless the `telemetry` feature
    /// is on.
    pub fn report_global_volume(&mut self, global_volume: U0F16) {
        #[cfg(feature = "telemetry")]
        if let Some(telemetry) = self.telemetry.as_deref_mut() {
            telemetry.set_global_volume(global_volume);
        }
        #[cfg(not(feature = "telemetry"))]
        let _ = global_volume;
        #[cfg(feature = "trace")]
        if let Some(trace) = self.trace.as_deref_mut() {
            let scaled = ((global_volume.to_bits() as u32 * 64 + 32_767) / 65_535) as u8;
            trace.report_global_volume(scaled);
        }
    }

    /// Report format-native channel state for the stable per-tick trace.
    ///
    /// This is a no-op without `trace`, so format crates do not grow feature-dependent
    /// call sites.
    #[inline]
    pub fn report_trace_channel(&mut self, channel: ChannelId, state: TraceChannelState) {
        #[cfg(feature = "trace")]
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.report_channel(channel, state);
        }
        #[cfg(not(feature = "trace"))]
        let _ = (channel, state);
    }

    /// Start or retrigger a channel and trace the initial parameter writes.
    #[inline]
    pub fn trigger_channel(
        &mut self,
        channel: ChannelId,
        tag: VoiceTag,
        region: SampleRegion,
        params: VoiceParams,
        offset_frames: u32,
    ) -> Option<VoiceId> {
        let voice = self.channels.trigger(channel, self.voices, tag, region, params, offset_frames)?;
        #[cfg(feature = "trace")]
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.record_voice_flags(self.voices, voice, params.dirty);
        }
        Some(voice)
    }

    /// Stop and unbind a channel, preserving the stop write in the tick trace even though
    /// the foreground handle is deliberately cleared immediately.
    #[inline]
    pub fn stop_channel(&mut self, channel: ChannelId) -> bool {
        #[cfg(feature = "trace")]
        if self.channels.is_sounding(channel, self.voices)
            && let Some(trace) = self.trace.as_deref_mut()
        {
            trace.record_channel_flags(channel, DirtyBits::STOP);
        }
        self.channels.stop(channel, self.voices)
    }

    /// Apply one absolute parameter write and feed the trace hook.
    #[inline]
    pub fn write_voice_param(&mut self, voice: VoiceId, param: VoiceParam) {
        #[cfg(feature = "trace")]
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.record_param_write(self.voices, voice, param);
        }
        if let Some(state) = self.voices.get_mut(voice) {
            param.apply(&mut state.params);
        }
    }

    /// Set a non-value dirty bit such as tempo and feed the same trace hook.
    #[inline]
    pub fn mark_voice_dirty(&mut self, voice: VoiceId, flags: DirtyBits) {
        #[cfg(feature = "trace")]
        if let Some(trace) = self.trace.as_deref_mut() {
            trace.record_voice_flags(self.voices, voice, flags);
        }
        if let Some(state) = self.voices.get_mut(voice) {
            state.params.dirty.insert(flags);
        }
    }
}

/// A format's effect processor: everything the sequencer does not know.
///
/// Two entry points, matching the original's two jump tables — `StaticJumpTable` on tick 0
/// (`S_FX_*`) and `MinorJumpTable` on ticks 1..n−1 (`M_FX_*`) — because that split is how
/// the semantics are actually organised, not an implementation detail.
///
/// # Not a `dyn` call per note
///
/// The sequencer is generic over the processor, so the calls monomorphise. They happen at
/// tick rate — around 50 Hz — so this is taste rather than necessity; what is *not*
/// negotiable is that nothing below here is a trait object, because a `dyn` call per
/// channel per tick would be.
///
/// # Why this is a trait before its second implementation
///
/// It is not a general abstraction being invented ahead of need — it is the seam between
/// timing and semantics, and it has five implementations queued behind it (S3M in M1, MOD
/// and MTM in M2, XM in M5, IT in M6). The trait that architecture §10.1 defers is
/// `Instrument`, which is a different seam and arrives at M4.
pub trait TrackerProcessor {
    /// The first tick of a row: parse `row`'s packed bytes, latch notes and instruments,
    /// and run the tick-0 effects.
    ///
    /// Called **once per row**, not once per pattern-delay repeat — the original does not
    /// re-fetch notes on a repeat either.
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome;

    /// Every other tick of the row, including the first tick of a pattern-delay repeat.
    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome;
}

// ── the sequencer ───────────────────────────────────────────────────────────────────

/// What happens when the order list runs out.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EndOfSongPolicy {
    /// Go back to the restart order and keep playing, raising
    /// [`PatternSequencer::song_looped`].
    #[default]
    Loop,
    /// Stop. The sequencer reports no further events and the voices ring out.
    Stop,
}

/// Everything about a sequencer that is not the module or the processor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SequencerSettings {
    /// Output rate the tick clock converts against.
    pub sample_rate_hz: u32,
    /// Absolute frame of the very first tick.
    pub first_tick_frame: Frame,
    /// Ticks per row before any `Axx`. S3M's `_initialspd`, conventionally 6.
    pub initial_speed: u8,
    /// Tempo before any `Txx`. S3M's `_initialBPM`, conventionally 125.
    pub initial_tempo_bpm: u16,
    /// Order-list index the song restarts from when it loops.
    pub restart_order: u16,
    /// What the end of the order list means.
    pub end_of_song: EndOfSongPolicy,
}

impl Default for SequencerSettings {
    fn default() -> SequencerSettings {
        SequencerSettings {
            sample_rate_hz: 44_100,
            first_tick_frame: Frame::ZERO,
            initial_speed: 6,
            initial_tempo_bpm: 125,
            restart_order: 0,
            end_of_song: EndOfSongPolicy::Loop,
        }
    }
}

/// Whether a tick boundary currently exists.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SequencerState {
    /// A tick is due, and [`PatternSequencer::next_event_frame`] says when.
    Ready,
    /// A tick is being processed. **There is no next boundary yet** — that is the point.
    Processing,
    /// The song is over, or the module is unplayable. No further events.
    Stopped,
}

/// The tracker timing spine: order list → pattern → row → tick.
pub struct PatternSequencer<Tempo: TempoModel, Processor: TrackerProcessor, Data: PatternData> {
    clock: FrameClock<Tempo>,
    processor: Processor,
    data: Data,
    row_clock: RowClock,
    position: SongPosition,
    tempo_bpm: u16,
    state: SequencerState,
    end_of_song: EndOfSongPolicy,
    restart_order: u16,
    pending_jump: Option<Jump>,
    song_looped: bool,
    /// The `_MActual*` snapshot: the position latched at the top of the row that is
    /// currently *sounding*. See [`PatternSequencer::sounding_position`].
    sounding_position: SongPosition,
    /// The `_MActualTick` half of the same snapshot: the tick within that row.
    sounding_tick: u16,
}

impl<Tempo: TempoModel, Processor: TrackerProcessor, Data: PatternData> PatternSequencer<Tempo, Processor, Data> {
    /// A sequencer positioned at the first playable order of `data`.
    ///
    /// If the order list has no playable entry at all the sequencer starts stopped, which
    /// is the only safe answer for a module that a fuzzer produced.
    pub fn new(tempo_model: Tempo, data: Data, processor: Processor, settings: SequencerSettings) -> PatternSequencer<Tempo, Processor, Data> {
        let mut sequencer = PatternSequencer {
            clock: FrameClock::new(tempo_model, settings.sample_rate_hz, settings.first_tick_frame),
            processor,
            data,
            row_clock: RowClock::new(settings.initial_speed),
            position: SongPosition::default(),
            tempo_bpm: settings.initial_tempo_bpm,
            state: SequencerState::Ready,
            end_of_song: settings.end_of_song,
            restart_order: settings.restart_order,
            pending_jump: None,
            song_looped: false,
            sounding_position: SongPosition::default(),
            sounding_tick: 0,
        };
        if !sequencer.move_to_order(settings.restart_order, 0) {
            sequencer.state = SequencerState::Stopped;
        }
        sequencer.song_looped = false;
        sequencer
    }

    /// Where in the song the *next* tick will be.
    ///
    /// This is the live cursor — the original's `_MCurrentRow` / `_MCurrentPos` /
    /// `_MCurrentPatt`. Between the last tick of a row and the first tick of the next it
    /// has already moved on, so **a display must not render this**; render
    /// [`PatternSequencer::sounding_position`] instead.
    pub const fn position(&self) -> SongPosition { self.position }

    /// Where in the song the row that is currently **sounding** is.
    ///
    /// The original's `_MActualRow` / `_MActualPos` / `_MActualPatt`, latched at the top of
    /// each row precisely so the display shows the row being heard rather than the one
    /// being parsed (`plans/reference/original-star-ui.md` §2.1,
    /// `original-s3mlib-analysis.md` §2). It is the difference between a display that looks
    /// right and one that runs a row ahead, and it is what B6's telemetry publishes.
    pub const fn sounding_position(&self) -> SongPosition { self.sounding_position }

    /// The tick within the sounding row — `_MActualTick`, counting **up** from 0 and
    /// absolute across pattern-delay repeats, like [`RowClock::tick_in_row`].
    pub const fn sounding_tick(&self) -> u16 { self.sounding_tick }

    /// The current row's tick budget and absolute tick index.
    pub const fn row_clock(&self) -> RowClock { self.row_clock }

    /// The tempo in effect.
    pub const fn tempo_bpm(&self) -> u16 { self.tempo_bpm }

    /// Whether the song has ended.
    pub const fn is_stopped(&self) -> bool { matches!(self.state, SequencerState::Stopped) }

    /// Whether the order list has wrapped at least once since this flag was last taken.
    ///
    /// The original fires its loop callback only when the new position is order 0
    /// (`PM_SetLoopCode`, `SB_IRQ_Handler`); this is the same signal, sticky so a host
    /// polling at UI rate cannot miss it.
    pub const fn song_looped(&self) -> bool { self.song_looped }

    /// Read and clear [`PatternSequencer::song_looped`].
    pub fn take_song_looped(&mut self) -> bool { core::mem::take(&mut self.song_looped) }

    /// The format's effect processor.
    pub const fn processor(&self) -> &Processor { &self.processor }

    /// The format's effect processor, mutably. Off the audio thread.
    pub const fn processor_mut(&mut self) -> &mut Processor { &mut self.processor }

    /// The module's pattern data.
    pub const fn data(&self) -> &Data { &self.data }

    /// Stop the song. No further ticks; sounding voices ring out.
    pub fn stop(&mut self) { self.state = SequencerState::Stopped; }

    /// Jump to an order-list index, from its first row, and resume if stopped.
    ///
    /// Returns whether the index resolved to a playable pattern. A seek that does not
    /// resolve **leaves the cursor where it was** and does not apply the end-of-song
    /// policy: a host asking for order 99 of a twelve-order module has made a mistake, and
    /// silently looping the song back to the start would hide it.
    pub fn seek_order(&mut self, order: u16) -> bool {
        let Some((order, pattern)) = self.resolve_order(order) else { return false };
        self.set_position(order, pattern, 0);
        self.pending_jump = None;
        self.state = SequencerState::Ready;
        true
    }

    /// Jump to a row of the pattern already playing, and resume if stopped.
    pub fn seek_row(&mut self, row: u16) {
        self.position.row = self.clamped_row(self.position.pattern, row);
        let speed = self.row_clock.speed;
        self.row_clock.start_row(speed);
        self.pending_jump = None;
        self.state = SequencerState::Ready;
    }

    /// Restart the tick clock at `frame`, keeping the song position.
    ///
    /// A seek on the engine's monotonic timeline, not a rewind: the caller supplies a frame
    /// the engine has not reached yet.
    pub fn restart_clock_at(&mut self, frame: Frame) { self.clock.reset(frame); }

    /// Run one tick and return what it left in effect.
    ///
    /// The only caller is [`EventSource::dispatch`]; it is separate so that the boundary
    /// computation in [`PatternSequencer::commit`] cannot be interleaved with it.
    fn run_tick(&mut self, frame: Frame, context: &mut EngineContext<'_>) -> TickOutcome {
        // The `_MActual*` latch. The live cursor does not move within a row, so latching
        // here rather than reading `self.position` matters only between the end of one
        // row's last tick and the start of the next row's first — which is exactly the
        // window a UI polls in, and exactly where an unlatched display runs a row ahead.
        if self.row_clock.is_first_tick_of_row() {
            self.sounding_position = self.position;
            #[cfg(feature = "telemetry")]
            if let Some(telemetry) = context.telemetry.as_deref_mut() {
                // `_CMDVal` / `_CMDData` are re-read per row, so a row whose channel has no
                // effect shows none rather than the previous row's.
                telemetry.clear_effects();
            }
        }
        self.sounding_tick = self.row_clock.tick_in_row;

        #[cfg(feature = "trace")]
        if let Some(trace) = context.trace.as_deref_mut() {
            trace.begin_tick(frame, self.sounding_position, self.sounding_tick, self.data.channel_count());
        }

        let mut tick = TickContext {
            frame,
            voices: &mut *context.voices,
            channels: &mut *context.channels,
            row_clock: self.row_clock,
            position: self.position,
            tempo_bpm: self.tempo_bpm,
            #[cfg(feature = "telemetry")]
            telemetry: context.telemetry.as_deref_mut(),
            #[cfg(feature = "trace")]
            trace: context.trace.as_deref_mut(),
        };

        if !self.row_clock.is_first_tick_of_row() {
            return self.processor.tick(&mut tick);
        }

        // The first tick of a row — and *only* the first, not the first of each
        // pattern-delay repeat. `__UpdateTracker` does not re-fetch notes on a repeat
        // either (`plans/reference/original-s3mlib-analysis.md` §3).
        match self.data.row_bytes(self.position.pattern, self.position.row) {
            Some(bytes) => {
                let row = RowRef { order: self.position.order, pattern: self.position.pattern, row: self.position.row, bytes };
                self.processor.row(&mut tick, row)
            }
            None => {
                // The order list points at a row that is not there. A corrupt or fuzzed
                // module reaches here; ending the song is the only answer that neither
                // panics nor spins.
                let mut outcome = tick.outcome();
                outcome.stop = true;
                outcome
            }
        }
    }

    /// Consume a tick's outcome and compute where the next tick lands.
    ///
    /// **The one call site of `advance_tick` in the whole sequencer.** Everything that can
    /// move the boundary — `Txx`, `Axx`, `SEx`, `Bxx`, `Cxx` — has already been applied by
    /// the time it runs, because all of it arrives inside `outcome`.
    fn commit(&mut self, outcome: TickOutcome) {
        self.tempo_bpm = outcome.tempo_bpm;
        self.row_clock.set_speed(outcome.speed);
        if self.row_clock.is_first_tick_of_row() {
            self.row_clock.set_pattern_delay(outcome.pattern_delay);
        }
        if outcome.jump.is_some() {
            self.pending_jump = outcome.jump;
        }
        if outcome.stop {
            self.state = SequencerState::Stopped;
            return;
        }

        self.clock.advance_tick(self.tempo_bpm, self.row_clock.speed);

        if self.row_clock.advance() == RowAdvance::NextRow {
            let jump = self.pending_jump.take();
            if !self.begin_next_row(jump) {
                self.state = SequencerState::Stopped;
                return;
            }
        }
        self.state = SequencerState::Ready;
    }

    /// Move to the row that follows the one just finished. Returns `false` if the song is
    /// over and the policy says stop.
    fn begin_next_row(&mut self, jump: Option<Jump>) -> bool {
        let speed = self.row_clock.speed;
        match jump {
            // `SBx` pattern loop: back to a row of the pattern already playing.
            Some(jump) if jump.within_pattern => {
                self.position.row = self.clamped_row(self.position.pattern, jump.row.unwrap_or(0));
                self.row_clock.start_row(speed);
                true
            }
            // `Bxx`, `Cxx`, or both: a named order and/or a named row. `Cxx` alone means
            // "the next order", which is why `order` is an `Option` rather than defaulted
            // by the caller.
            Some(jump) => {
                let order = jump.order.unwrap_or_else(|| self.position.order.saturating_add(1));
                self.move_to_order(order, jump.row.unwrap_or(0))
            }
            // The ordinary case: the next row, or the next order if the pattern is over.
            None => {
                let rows = self.data.rows_in_pattern(self.position.pattern).unwrap_or(0);
                let next_row = self.position.row.saturating_add(1);
                if next_row < rows {
                    self.position.row = next_row;
                    self.row_clock.start_row(speed);
                    true
                } else {
                    self.move_to_order(self.position.order.saturating_add(1), 0)
                }
            }
        }
    }

    /// Position the sequencer at `order`, applying the end-of-song policy if the order list
    /// runs out. Returns `false` only if the song is over and must stop.
    fn move_to_order(&mut self, order: u16, row: u16) -> bool {
        if let Some((order, pattern)) = self.resolve_order(order) {
            self.set_position(order, pattern, row);
            return true;
        }
        match self.end_of_song {
            EndOfSongPolicy::Stop => false,
            EndOfSongPolicy::Loop => {
                self.song_looped = true;
                // One retry, never a loop: if the restart point is itself unplayable the
                // song stops rather than spinning inside the audio callback.
                match self.resolve_order(self.restart_order) {
                    Some((order, pattern)) => {
                        self.set_position(order, pattern, 0);
                        true
                    }
                    None => false,
                }
            }
        }
    }

    /// The first playable order at or after `start`, stepping over `Skip` markers.
    ///
    /// Bounded by the order count, so a list of nothing but markers terminates.
    fn resolve_order(&self, start: u16) -> Option<(u16, u16)> {
        let count = self.data.order_count();
        let mut order = start;
        for _ in 0..count {
            if order >= count {
                return None;
            }
            match self.data.order(order)? {
                OrderEntry::Pattern(pattern) => return Some((order, pattern)),
                OrderEntry::Skip => order = order.saturating_add(1),
                OrderEntry::End => return None,
            }
        }
        None
    }

    fn set_position(&mut self, order: u16, pattern: u16, row: u16) {
        self.position = SongPosition { order, pattern, row: self.clamped_row(pattern, row) };
        let speed = self.row_clock.speed;
        self.row_clock.start_row(speed);
    }

    /// A row index that exists in `pattern`. A `Cxx` past the end of the next pattern lands
    /// on row 0, which is what ST3 does.
    fn clamped_row(&self, pattern: u16, row: u16) -> u16 {
        let rows = self.data.rows_in_pattern(pattern).unwrap_or(1).max(1);
        if row >= rows { 0 } else { row }
    }

    /// Publish one coherent snapshot — **once per tick**, at the end of dispatch.
    ///
    /// Everything in it is from this tick: the `_MActual*` position latched at the top of
    /// the sounding row, the speed and tempo the tick left in effect, and the channel and
    /// voice state as it stands before anything is mixed. Any effect the processor reported
    /// during the tick is already in the working snapshot.
    ///
    /// Allocation-free: the publisher owns its snapshot inline and the ring was allocated
    /// when the engine was built.
    #[cfg(feature = "telemetry")]
    fn publish_telemetry(&mut self, context: &mut EngineContext<'_>) {
        let Some(telemetry) = context.telemetry.as_deref_mut() else { return };
        let sounding = self.sounding_position;
        telemetry.set_position(sounding.order, sounding.pattern, sounding.row, self.sounding_tick);
        telemetry.set_timing(self.row_clock.speed, self.tempo_bpm);
        crate::telemetry::capture_channels(telemetry, context.channels, context.voices);
        // The table has however many lanes the host sized it with; the UI wants the
        // *song's* channels, which only the pattern data knows.
        telemetry.set_channel_count(self.data.channel_count().min(context.channels.len().min(u8::MAX as usize) as u8));
        telemetry.publish();
    }
}

impl<Tempo: TempoModel, Processor: TrackerProcessor, Data: PatternData> EventSource for PatternSequencer<Tempo, Processor, Data> {
    fn next_event_frame(&self) -> Option<Frame> {
        match self.state {
            SequencerState::Ready => Some(self.clock.pending_tick_frame()),
            // No boundary exists while a tick is in flight, and none exists after the song
            // has ended. Rule 1 is not a convention here — it is unrepresentable.
            SequencerState::Processing | SequencerState::Stopped => None,
        }
    }

    fn advance_to(&mut self, _frame: Frame) {
        // Nothing to do: every position this type holds is an absolute frame on the
        // engine's clock, so there is no elapsed-time bookkeeping to keep in step.
    }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        if !matches!(self.state, SequencerState::Ready) {
            return;
        }
        self.state = SequencerState::Processing;

        // A tracker tick *is* the control tick (architecture §5.4).
        context.control.tick_from_tracker(frame);

        let outcome = self.run_tick(frame, context);
        self.commit(outcome);

        #[cfg(feature = "trace")]
        if let Some(trace) = context.trace.as_deref_mut() {
            trace.finish_tick(self.row_clock.speed, self.tempo_bpm, context.channels, context.voices);
        }

        #[cfg(feature = "telemetry")]
        self.publish_telemetry(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use starplayer_core::{ExactFixedPoint, Step};
    use starplayer_mixer::SampleRegion;

    use crate::control::ControlClock;
    use crate::demo::{
        DEMO_BREAK_ROW, DEMO_ORDER_JUMP, DEMO_PATTERN_DELAY, DEMO_SET_SPEED, DEMO_SET_TEMPO, DEMO_STOP, DemoCell,
        DemoPatternData, DemoProcessor,
    };

    /// 44100 Hz at 125 BPM is exactly 882 frames per tick, so every frame in these tests is
    /// a round number and a wrong one is obvious.
    const FRAMES_PER_TICK_AT_125: u64 = 882;
    /// …and 250 BPM is exactly half that.
    const FRAMES_PER_TICK_AT_250: u64 = 441;

    type TestSequencer = PatternSequencer<ExactFixedPoint, DemoProcessor, DemoPatternData>;

    /// The engine's side of a dispatch, without an engine.
    struct Harness {
        voices: VoicePool,
        channels: ChannelTable,
        control: ControlClock,
    }

    impl Harness {
        fn new() -> Harness {
            Harness { voices: VoicePool::new(8), channels: ChannelTable::new(4), control: ControlClock::new(44_100, Frame::ZERO) }
        }

        /// Run the next tick and report the frame it landed on.
        fn tick(&mut self, sequencer: &mut TestSequencer) -> Option<Frame> {
            let frame = sequencer.next_event_frame()?;
            let mut context = EngineContext::new(frame, &mut self.voices, &mut self.channels, &mut self.control);
            sequencer.dispatch(frame, &mut context);
            Some(frame)
        }

        /// Run `count` ticks and report the frame each landed on.
        fn ticks(&mut self, sequencer: &mut TestSequencer, count: usize) -> Vec<Frame> {
            (0..count).filter_map(|_| self.tick(sequencer)).collect()
        }
    }

    fn sequencer(data: DemoPatternData, settings: SequencerSettings) -> TestSequencer {
        PatternSequencer::new(ExactFixedPoint, data, DemoProcessor::new(SampleRegion::default(), Step::ONE), settings)
    }

    #[test]
    fn ticks_land_on_exact_frames_and_rows_are_fetched_once_each() {
        let data = DemoPatternData::new(1, 4, 1);
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        let frames = harness.ticks(&mut sequencer, 12);
        let expected: Vec<Frame> = (0..12).map(|tick| Frame(tick * FRAMES_PER_TICK_AT_125)).collect();
        assert_eq!(frames, expected, "speed 6 at 125 BPM: every tick 882 frames apart");

        assert_eq!(sequencer.processor().rows_played(), 2, "two rows in twelve ticks at speed 6");
        assert_eq!(sequencer.processor().ticks_played(), 10, "the other ten ticks are not row ticks");
        assert_eq!(sequencer.position(), SongPosition { order: 0, pattern: 0, row: 2 });
    }

    /// The verification case: a tempo change on tick N moves tick **N+1**, not N+2.
    #[test]
    fn a_tempo_change_moves_the_very_next_tick() {
        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 1, 0, DemoCell::command(DEMO_SET_TEMPO, 250));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        let frames = harness.ticks(&mut sequencer, 9);
        let row_one_tick_zero = 6 * FRAMES_PER_TICK_AT_125;
        assert_eq!(frames.get(6), Some(&Frame(row_one_tick_zero)), "row 1 starts on tick 6, still at the old tempo");
        assert_eq!(
            frames.get(7),
            Some(&Frame(row_one_tick_zero + FRAMES_PER_TICK_AT_250)),
            "the tick after the Txx is one 250 BPM tick later, not one 125 BPM tick later"
        );
        assert_eq!(frames.get(8), Some(&Frame(row_one_tick_zero + 2 * FRAMES_PER_TICK_AT_250)));
        assert_eq!(sequencer.tempo_bpm(), 250);
    }

    #[test]
    fn a_speed_change_lengthens_the_row_it_appears_on() {
        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_SET_SPEED, 3));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 3);
        assert_eq!(sequencer.position().row, 1, "Axx applies to the row it is on: three ticks, not six");
        assert_eq!(sequencer.row_clock().speed, 3);
    }

    #[test]
    fn a_pattern_delay_repeats_the_row_without_re_fetching_its_notes() {
        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_PATTERN_DELAY, 2));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 1);
        assert_eq!(sequencer.row_clock().total_ticks(), 18, "speed 6, two extra repeats");

        harness.ticks(&mut sequencer, 17);
        assert_eq!(sequencer.processor().rows_played(), 1, "the row was fetched once for all three repeats");
        assert_eq!(sequencer.processor().ticks_played(), 17);
        assert_eq!(sequencer.position().row, 1, "and only then does the song move on");
    }

    #[test]
    fn a_pattern_break_moves_to_the_named_row_of_the_next_pattern() {
        let mut data = DemoPatternData::new(2, 8, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_BREAK_ROW, 5));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 6);
        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 5 }, "Cxx breaks at the end of its row");
    }

    #[test]
    fn a_break_past_the_end_of_the_next_pattern_lands_on_row_zero() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_BREAK_ROW, 200));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 6);
        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 0 });
    }

    #[test]
    fn a_position_jump_moves_through_the_order_list() {
        let mut data = DemoPatternData::new(3, 4, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_ORDER_JUMP, 2));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 6);
        assert_eq!(sequencer.position(), SongPosition { order: 2, pattern: 2, row: 0 });
    }

    #[test]
    fn skip_markers_are_stepped_over_and_an_end_marker_ends_the_song() {
        let data = DemoPatternData::new(2, 2, 1).with_orders(vec![
            OrderEntry::Skip,
            OrderEntry::Pattern(1),
            OrderEntry::End,
            OrderEntry::Pattern(0),
        ]);
        let settings = SequencerSettings { end_of_song: EndOfSongPolicy::Stop, ..SequencerSettings::default() };
        let mut sequencer = sequencer(data, settings);
        let mut harness = Harness::new();

        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 0 }, "order 0 is a marker, not a pattern");

        harness.ticks(&mut sequencer, 12);
        assert!(sequencer.is_stopped(), "the End marker at order 2 ends the song; the pattern behind it is unreachable");
        assert_eq!(sequencer.next_event_frame(), None, "a stopped sequencer reports no boundary at all");
        assert!(!sequencer.song_looped());
    }

    #[test]
    fn the_song_loops_and_raises_the_flag() {
        let data = DemoPatternData::new(1, 2, 1);
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 12);
        assert!(!sequencer.is_stopped());
        assert_eq!(sequencer.position(), SongPosition { order: 0, pattern: 0, row: 0 }, "back to the start");
        assert!(sequencer.song_looped());
        assert!(sequencer.take_song_looped());
        assert!(!sequencer.song_looped(), "taking the flag clears it");
    }

    #[test]
    fn a_stop_command_ends_the_song_wherever_the_policy_stands() {
        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 1, 0, DemoCell::command(DEMO_STOP, 0));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 7);
        assert!(sequencer.is_stopped());
        assert_eq!(harness.tick(&mut sequencer), None, "and nothing runs after it");
    }

    #[test]
    fn an_empty_order_list_starts_stopped_rather_than_spinning() {
        let data = DemoPatternData::new(0, 0, 1).with_orders(Vec::new());
        let mut sequencer = sequencer(data, SequencerSettings::default());
        assert!(sequencer.is_stopped());
        assert_eq!(sequencer.next_event_frame(), None);
        assert!(!sequencer.song_looped(), "starting on an unplayable module is not a loop");
        let mut harness = Harness::new();
        assert_eq!(harness.tick(&mut sequencer), None);
    }

    #[test]
    fn an_order_list_of_nothing_but_markers_terminates() {
        let data = DemoPatternData::new(1, 2, 1).with_orders(vec![OrderEntry::Skip; 8]);
        let sequencer = sequencer(data, SequencerSettings::default());
        assert!(sequencer.is_stopped(), "the resolver is bounded by the order count");
    }

    #[test]
    fn seeking_moves_the_cursor_and_restarts_a_stopped_song() {
        let mut data = DemoPatternData::new(2, 8, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_STOP, 0));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 1);
        assert!(sequencer.is_stopped());

        assert!(sequencer.seek_order(1));
        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 0 });
        assert!(!sequencer.is_stopped());

        sequencer.seek_row(4);
        assert_eq!(sequencer.position().row, 4);
        assert_eq!(sequencer.row_clock().tick_in_row, 0, "a seek starts the row from its first tick");
        assert!(!sequencer.seek_order(99), "seeking past the order list fails rather than looping the song");
        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 4 }, "and leaves the cursor alone");
    }

    #[test]
    fn the_sequencer_drives_the_control_clock() {
        let data = DemoPatternData::new(1, 4, 1);
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();
        assert_eq!(harness.control.driver(), crate::control::ControlDriver::Synthesised);

        harness.ticks(&mut sequencer, 3);
        assert_eq!(harness.control.driver(), crate::control::ControlDriver::Tracker, "a tracker tick is the control tick");
        assert_eq!(harness.control.ticks(), 3);
        assert_eq!(harness.control.next_synthesised_frame(), None, "so the engine adds none of its own");
    }

    #[test]
    fn notes_reach_the_voice_pool_through_the_channel_table() {
        let mut data = DemoPatternData::new(1, 4, 2);
        data.set(0, 0, 0, DemoCell::note(48));
        data.set(0, 0, 1, DemoCell::note(60));
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        harness.ticks(&mut sequencer, 1);
        assert_eq!(sequencer.processor().notes_triggered(), 2);
        assert_eq!(harness.voices.voices_active(), 2);
        assert!(harness.channels.foreground(starplayer_core::ChannelId(0)).is_some());
        assert!(harness.channels.foreground(starplayer_core::ChannelId(1)).is_some());
    }

    #[test]
    fn a_row_that_is_not_there_ends_the_song_instead_of_panicking() {
        // An order list pointing at a pattern the data does not contain: what a fuzzed
        // module looks like.
        let data = DemoPatternData::new(1, 4, 1).with_orders(vec![OrderEntry::Pattern(9)]);
        let mut sequencer = sequencer(data, SequencerSettings::default());
        let mut harness = Harness::new();

        assert_eq!(harness.tick(&mut sequencer), Some(Frame::ZERO));
        assert!(sequencer.is_stopped());
    }
}

//! The render loop: [`RENDER_QUANTUM`], the [`Engine`], the zero-advance guard, and the
//! control plane it drains at the top of every quantum.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::{ChannelId, Command, Frame, U0F16};
use starplayer_dsp::Interpolate;
use starplayer_mixer::{Limiter, MasterSettings, MixPath, OutputFormat, VoicePool};
use starplayer_rt::{Consumer, GarbageChannel, garbage_channel};

use crate::channel::ChannelTable;
use crate::command::{DEFAULT_COMMAND_CAPACITY, DEFAULT_GARBAGE_CAPACITY, EngineHandle, MAX_COMMANDS_PER_QUANTUM, PcmSource};
use crate::control::ControlClock;
use crate::ring::OutputRing;
use crate::source::{EngineContext, EventSource, SourceMux, SourceSlot};

/// The engine's internal render granularity, in frames.
///
/// 128 frames is exactly an AudioWorklet quantum, so the browser — the first target —
/// costs nothing to adapt. Voice accumulation splits *within* a quantum at event
/// boundaries; the DSP graph and the master bus only ever see whole quanta
/// (architecture §1.4).
///
/// Whether embedded targets want a compile-time override is architecture open question
/// Q2, to be settled at M8. Until then this is a constant and there is deliberately no
/// knob.
pub const RENDER_QUANTUM: usize = 128;

/// Consecutive dispatches at the same frame before the engine forces the clock forward
/// (architecture §3.1 rule 2).
///
/// S3M speed 0, `A00`, MOD `E60` self-loops, pattern-break-to-self and SMF zero-delta
/// meta events can all make a source return the same frame forever and spin the render
/// loop **inside the audio callback**, permanently. Every tracker has shipped this bug.
pub const MAX_ZERO_ADVANCE: u32 = 64;

/// Events dispatched within one render quantum before the engine stops asking.
pub const MAX_EVENTS_PER_BLOCK: u32 = 4096;

/// The output rate an [`Engine::new`] assumes, in Hz.
///
/// It matters only to the synthesised control clock; the tick clock's rate belongs to the
/// sequencer, and the mixer works in [`Step`](starplayer_core::Step) rather than Hz.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 44_100;

/// What went wrong that the host should know about but the audio thread must not panic
/// over.
///
/// Sticky: set by `render()`, cleared by whoever reads them. This is the seed of the
/// telemetry warning surface that M2 grows; it is here now because architecture §3.1
/// rule 2 requires the guard to *set a flag*, and a guard whose breach nothing can
/// observe is not a guard.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EngineWarnings {
    /// A source returned the same frame more than [`MAX_ZERO_ADVANCE`] times in a row, or
    /// still reported an event as due after being dispatched. The engine forced the clock
    /// forward by one frame and carried on.
    pub zero_advance_forced: bool,
    /// A source produced more than [`MAX_EVENTS_PER_BLOCK`] events within one render
    /// quantum. The remainder were left for the next quantum.
    pub event_limit_reached: bool,
    /// A retired module handle could not be returned over the garbage channel and was
    /// dropped on the audio thread, which may have called `free()` inside the callback.
    ///
    /// This means the control side has stopped calling
    /// [`EngineHandle::collect_all_garbage`], or was never claimed at all. It is a host
    /// bug, not a module bug.
    pub retired_module_dropped: bool,
    /// A command arrived that this milestone does not act on yet — `SeekOrder`,
    /// `SeekRow`, `SetInterpolator` or `SetTempoModel`. Flagged rather than ignored
    /// silently, so a host is not left wondering why nothing happened.
    pub unsupported_command: bool,
}

impl EngineWarnings {
    /// Whether anything has been flagged.
    pub const fn any(self) -> bool {
        self.zero_advance_forced || self.event_limit_reached || self.retired_module_dropped || self.unsupported_command
    }

    /// Take the flags and reset them.
    pub fn take(&mut self) -> EngineWarnings { core::mem::take(self) }
}

/// Everything about an engine that is fixed when it is created.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EngineSettings {
    /// Voices in the global pool.
    pub voice_capacity: usize,
    /// Control lanes. 32 covers S3M; IT's 64 is [`ChannelTable::MAX_CHANNELS`].
    pub channel_count: usize,
    /// Output rate, for the synthesised control clock.
    pub sample_rate_hz: u32,
    /// How many [`EventSource`]s may be merged at once.
    pub source_capacity: usize,
    /// Depth of the command ring.
    pub command_capacity: usize,
    /// Depth of the garbage channel.
    pub garbage_capacity: usize,
}

impl Default for EngineSettings {
    fn default() -> EngineSettings {
        EngineSettings {
            voice_capacity: 64,
            channel_count: 32,
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            source_capacity: SourceMux::DEFAULT_CAPACITY,
            command_capacity: DEFAULT_COMMAND_CAPACITY,
            garbage_capacity: DEFAULT_GARBAGE_CAPACITY,
        }
    }
}

/// The render loop.
///
/// # The four type parameters
///
/// The first three are monomorphised, so the inner loop contains no `dyn` call and no
/// branch on configuration:
///
/// * `Path` — [`FloatPath`](starplayer_mixer::FloatPath) or
///   [`FixedPath`](starplayer_mixer::FixedPath): how voices accumulate.
/// * `Interp` — [`Linear`](starplayer_dsp::Linear) or
///   [`Nearest`](starplayer_dsp::Nearest): the resampling kernel.
/// * `Out` — the host's sample format, tied to `Path` by its accumulator type.
/// * `Module` — the module handle the control plane swaps in, normally
///   `Arc<Module>`. It defaults to `()` — "no module, play whatever `set_pcm` left" — so
///   that a test or a spike need not name one.
///
/// Selecting the first three at run time from
/// [`Command`](starplayer_core::Command) is a `match` over a handful of instantiations at
/// the point the stream is opened; it is not something `render()` branches on.
///
/// # Two clocks
///
/// [`Engine::frame`] is the **output** clock: it counts frames handed to the host and never
/// stops. [`Engine::source_frame`] is the **musical** clock that event sources report
/// against, and it stops while the engine is paused. They are equal for an engine that has
/// never been paused, which is every engine in the determinism tests.
///
/// # Real-time safety
///
/// Every allocation happens in [`Engine::with_settings`] and in the off-thread setters.
/// `render()` allocates nothing, locks nothing, and cannot panic: a source that misbehaves
/// trips the zero-advance guard, a flooded command ring is drained in bounded batches, and
/// sample data that does not resolve makes a voice end. The first of those is *proved*
/// rather than asserted by inspection: see the allocator hook described on
/// [`Engine::render`], which M2-C7 landed.
pub struct Engine<Path, Interp, Out, Module = ()>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Module: PcmSource,
{
    voices: VoicePool,
    channels: ChannelTable,
    control: ControlClock,
    /// The PCM blob used when no module is loaded.
    ///
    /// A fixture hook, kept from M0 so a test can play a sample without building a whole
    /// `Module`. Once B1's `Module` exists, a real host only ever goes through
    /// [`Command::LoadModule`](starplayer_core::Command::LoadModule).
    pcm: Vec<i16>,
    /// The module the control plane last swapped in. Retiring one sends it down the
    /// garbage channel; it is never dropped here.
    module: Option<Module>,
    /// One whole quantum of accumulated frames. Allocated once.
    accumulator: Box<[Path::Accumulator]>,
    /// Where muted channels' voices render, so their state advances exactly as if they
    /// were audible. Never read.
    muted_scratch: Box<[Path::Accumulator]>,
    ring: OutputRing<Out::Sample>,
    sources: SourceMux,
    commands: Consumer<Command<Module>>,
    garbage: GarbageChannel<Module>,
    /// The control half, held until [`Engine::take_control`] claims it.
    control_handle: Option<EngineHandle<Module>>,
    frame: Frame,
    source_frame: Frame,
    playing: bool,
    master_volume: U0F16,
    limiter: Limiter,
    warnings: EngineWarnings,
    /// The audio thread's half of the telemetry channel (architecture §9). Written once
    /// per tracker tick from inside the sequencer's dispatch.
    #[cfg(feature = "telemetry")]
    telemetry: starplayer_telemetry::TelemetryPublisher,
    /// The UI's half, held until [`Engine::telemetry_reader`] claims it.
    #[cfg(feature = "telemetry")]
    telemetry_reader: Option<starplayer_telemetry::TelemetryReader>,
    /// The audio thread's half of the per-channel scope taps (architecture §9(b)).
    /// Sampled once per render segment, published once per quantum.
    #[cfg(feature = "telemetry")]
    scope_taps: crate::scope::ScopeTaps,
    /// The UI's halves, held until [`Engine::scope_readers`] claims them.
    #[cfg(feature = "telemetry")]
    scope_readers: Option<alloc::boxed::Box<[starplayer_rt::TapReader]>>,
    /// Diagnostic per-tick trace. The field does not exist in shipping builds.
    #[cfg(feature = "trace")]
    trace: crate::trace::TraceRecorder,
    interpolator: core::marker::PhantomData<Interp>,
}

impl<Path, Interp, Out, Module> Engine<Path, Interp, Out, Module>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Module: PcmSource,
{
    /// An engine with `voice_capacity` voices and everything else at its default.
    pub fn new(voice_capacity: usize) -> Engine<Path, Interp, Out, Module> {
        Engine::with_settings(EngineSettings { voice_capacity, ..EngineSettings::default() })
    }

    /// An engine built to `settings`.
    ///
    /// This is where **every** allocation the engine performs happens: the voice pool, the
    /// channel table, the quantum accumulator, the output ring, the source slots, the
    /// command ring and the garbage channel.
    pub fn with_settings(settings: EngineSettings) -> Engine<Path, Interp, Out, Module> {
        let (command_producer, command_consumer) = starplayer_rt::channel(settings.command_capacity);
        let (garbage, collector) = garbage_channel(settings.garbage_capacity);
        #[cfg(feature = "telemetry")]
        let (telemetry, telemetry_reader) = starplayer_telemetry::telemetry_channel();
        // One scope ring per control lane, allocated here with everything else. A ring is
        // ~2 KB, so even IT's 64 channels cost 128 KB once, never in `render()`.
        #[cfg(feature = "telemetry")]
        let (scope_taps, scope_readers) = crate::scope::ScopeTaps::new(settings.channel_count);

        Engine {
            voices: VoicePool::new(settings.voice_capacity),
            channels: ChannelTable::new(settings.channel_count),
            control: ControlClock::new(settings.sample_rate_hz, Frame::ZERO),
            pcm: Vec::new(),
            module: None,
            accumulator: vec![Path::Accumulator::default(); RENDER_QUANTUM].into_boxed_slice(),
            muted_scratch: vec![Path::Accumulator::default(); RENDER_QUANTUM].into_boxed_slice(),
            ring: OutputRing::new(RENDER_QUANTUM * Out::CHANNELS),
            sources: SourceMux::new(settings.source_capacity),
            commands: command_consumer,
            garbage,
            control_handle: Some(EngineHandle::new(command_producer, collector)),
            frame: Frame::ZERO,
            source_frame: Frame::ZERO,
            playing: true,
            master_volume: U0F16::MAX,
            limiter: Limiter::SoftKnee,
            warnings: EngineWarnings::default(),
            #[cfg(feature = "telemetry")]
            telemetry,
            #[cfg(feature = "telemetry")]
            telemetry_reader: Some(telemetry_reader),
            #[cfg(feature = "telemetry")]
            scope_taps,
            #[cfg(feature = "telemetry")]
            scope_readers: Some(scope_readers),
            #[cfg(feature = "trace")]
            trace: crate::trace::TraceRecorder::default(),
            interpolator: core::marker::PhantomData,
        }
    }

    /// Claim the telemetry reader, once (architecture §9).
    ///
    /// Held by the engine until taken, for the same reason as
    /// [`Engine::take_control`]: it keeps [`Engine::new`] infallible, and a host that never
    /// draws anything never has to think about it. A host that *does* must take it
    /// **before** the engine goes to the audio thread — the reader is the only way back
    /// out, and there is exactly one of it.
    ///
    /// The snapshot it hands out is published once per tracker tick from the sequencer's
    /// dispatch; see [`crate::telemetry`].
    #[cfg(feature = "telemetry")]
    pub fn telemetry_reader(&mut self) -> Option<starplayer_telemetry::TelemetryReader> {
        self.telemetry_reader.take()
    }

    /// Claim the per-channel scope readers, once (architecture §9(b)).
    ///
    /// One [`TapReader`](starplayer_rt::TapReader) per channel, in channel order, held by
    /// the engine until taken for the same reason [`Engine::telemetry_reader`] is: a host
    /// that draws nothing never has to think about them, and a host that does must take
    /// them **before** the engine goes to the audio thread.
    ///
    /// Each ring carries [`TAP_RING_BUCKETS`](starplayer_rt::TAP_RING_BUCKETS) buckets of
    /// [`TAP_BUCKET_FRAMES`](starplayer_rt::TAP_BUCKET_FRAMES) frames each, newest last.
    /// The values are voice state sampled per segment, not the mix — see [`crate::scope`].
    #[cfg(feature = "telemetry")]
    pub fn scope_readers(&mut self) -> Option<alloc::boxed::Box<[starplayer_rt::TapReader]>> {
        self.scope_readers.take()
    }

    /// Ticks captured so far in a diagnostic trace build.
    #[cfg(feature = "trace")]
    pub fn trace(&self) -> &crate::trace::Trace { self.trace.trace() }

    /// Take the captured trace and leave the engine recording into an empty one.
    #[cfg(feature = "trace")]
    pub fn take_trace(&mut self) -> crate::trace::Trace { self.trace.take() }

    /// Discard every captured tick without changing playback state.
    #[cfg(feature = "trace")]
    pub fn clear_trace(&mut self) { self.trace.clear(); }

    /// Claim the control-side handle, once.
    ///
    /// It is held by the engine until taken so that [`Engine::new`] stays infallible and a
    /// host that never wants a control plane never has to think about one — but a host that
    /// does must take it **before** the engine goes to the audio thread, because it is the
    /// only way to load a module and the only place retired modules are dropped.
    pub fn take_control(&mut self) -> Option<EngineHandle<Module>> { self.control_handle.take() }

    /// Install the module's PCM blob directly, bypassing the control plane. Off the audio
    /// thread, and a fixture hook — see the field's documentation.
    pub fn set_pcm(&mut self, pcm: Vec<i16>) { self.pcm = pcm; }

    /// The module currently loaded, if any.
    pub const fn module(&self) -> Option<&Module> { self.module.as_ref() }

    /// Install one event source, replacing everything already there. Off the audio thread.
    pub fn set_source(&mut self, source: Box<dyn EventSource>) {
        self.sources.clear();
        self.control.resume_synthesis(self.source_frame);
        // The mux was just cleared, so a slot is free unless its capacity is zero.
        let _ = self.sources.insert(source);
    }

    /// Install `source` in place of the one already there and hand the retired one **back**
    /// rather than dropping it.
    ///
    /// [`Engine::set_source`] drops what it replaces, which is a `free()`: fine from a
    /// worklet message task, forbidden inside an audio callback (architecture §8). A native
    /// host has no choice about where it swaps — once the stream is open, the callback is
    /// the only place a `&mut Engine` exists — so it swaps here and sends the retired source
    /// down a garbage channel, exactly as a retired module handle already goes.
    ///
    /// Only the lowest-slot source is handed back. A host that has merged others alongside
    /// it keeps them, in their slots, and takes them out itself with
    /// [`Engine::remove_source`].
    pub fn replace_source(&mut self, source: Box<dyn EventSource>) -> Option<Box<dyn EventSource>> {
        let retired = self.sources.take_first();
        self.control.resume_synthesis(self.source_frame);
        // The take above freed the lowest occupied slot, so this insert lands in that slot
        // and can only fail for a mux built with no capacity at all.
        let _ = self.sources.insert(source);
        retired
    }

    /// Add a source alongside whatever is already installed, or hand it back if the mux is
    /// full. Off the audio thread.
    pub fn add_source(&mut self, source: Box<dyn EventSource>) -> Result<SourceSlot, Box<dyn EventSource>> {
        self.sources.insert(source)
    }

    /// Take a source back out. Off the audio thread.
    pub fn remove_source(&mut self, slot: SourceSlot) -> Option<Box<dyn EventSource>> { self.sources.remove(slot) }

    /// The merged source set.
    pub const fn sources(&self) -> &SourceMux { &self.sources }

    /// The voice pool, for whoever is binding channels to voices.
    pub const fn voices(&self) -> &VoicePool { &self.voices }

    /// The voice pool, mutably.
    pub const fn voices_mut(&mut self) -> &mut VoicePool { &mut self.voices }

    /// The channel-to-voice bindings.
    pub const fn channels(&self) -> &ChannelTable { &self.channels }

    /// The channel-to-voice bindings, mutably.
    pub const fn channels_mut(&mut self) -> &mut ChannelTable { &mut self.channels }

    /// The clock envelopes advance on (architecture §5.4).
    pub const fn control_clock(&self) -> ControlClock { self.control }

    /// The output clock: how many frames have been rendered since construction.
    pub const fn frame(&self) -> Frame { self.frame }

    /// The musical clock event sources report against. Equal to [`Engine::frame`] unless
    /// the engine has been paused.
    pub const fn source_frame(&self) -> Frame { self.source_frame }

    /// Whether the musical clock is running.
    pub const fn is_playing(&self) -> bool { self.playing }

    /// The master volume the control plane last set. Applied by the master bus before
    /// the limiter, so turning the master down turns the limiting down with it.
    pub const fn master_volume(&self) -> U0F16 { self.master_volume }

    /// How the master bus bounds the signal on the way out. [`Limiter::SoftKnee`] unless
    /// the host asks for the transparent path.
    pub const fn limiter(&self) -> Limiter { self.limiter }

    /// Choose the master bus limiter. Takes effect from the next whole quantum.
    pub fn set_limiter(&mut self, limiter: Limiter) { self.limiter = limiter; }

    /// Sticky warnings raised by the render loop.
    pub const fn warnings(&self) -> EngineWarnings { self.warnings }

    /// Read and clear the warnings.
    pub fn take_warnings(&mut self) -> EngineWarnings { self.warnings.take() }

    /// Fill `host_output` with interleaved samples in `Out`'s format.
    ///
    /// Any length is accepted, including one that is not a whole number of frames or
    /// quanta: the output ring carries the remainder of a quantum between calls, so
    /// output is identical whatever block sizes the host uses. That is the invariant
    /// `tests/block_size_determinism.rs` exists to hold in place.
    pub fn render(&mut self, host_output: &mut [Out::Sample]) {
        // The no-allocation property of this body is **enforced**, not asserted by
        // inspection: `crates/starplayer-offline/tests/render_allocation.rs` (M2-C7)
        // installs a global allocator that records every allocation and deallocation made
        // while an "inside render()" thread-local is set, and drives every module in the
        // corpus through this call with it armed. `cargo xtask ci --job rt-safety` is the
        // CI gate. There is no wrapper here, deliberately: the hook is test-only, so a
        // shipping build carries neither a flag nor a branch for it.
        let mut written = 0usize;
        while written < host_output.len() {
            if self.ring.is_empty() {
                self.render_quantum();
            }
            let Some(tail) = host_output.get_mut(written..) else { break };
            let count = self.ring.drain_into(tail);
            if count == 0 {
                // Unreachable: a freshly refilled ring always has samples. Bailing rather
                // than spinning, because a spin here is an audio dropout that never ends.
                break;
            }
            written = written.saturating_add(count);
        }
    }

    /// Render exactly one [`RENDER_QUANTUM`], splitting voice accumulation at event
    /// boundaries inside it, and leave the converted result in the ring.
    fn render_quantum(&mut self) {
        // Architecture §1.2 drains the control plane at the top of the render pass. Doing
        // it once per quantum rather than once per host call is what keeps a command from
        // landing on a different frame depending on the host's buffer size.
        self.drain_commands();

        for accumulated in self.accumulator.iter_mut() {
            *accumulated = Path::Accumulator::default();
        }

        // The scope taps sum into this quantum's buckets, so they start at zero. Done
        // before the segment loop for the same reason the accumulator is: a quantum is
        // the unit a bucket belongs to (architecture §9(b)).
        #[cfg(feature = "telemetry")]
        self.scope_taps.begin_quantum();

        let mut offset = 0usize;
        let mut events_this_quantum = 0u32;

        while offset < RENDER_QUANTUM {
            self.dispatch_due_events(&mut events_this_quantum);

            // Recomputed after every dispatch, never cached across one: a tempo or jump
            // effect changes when the next tick is from inside the tick just processed
            // (architecture §3.1 rule 1).
            let gap = self.frames_until_next_event();
            let remaining = RENDER_QUANTUM.saturating_sub(offset);
            let mut span = gap.min(remaining as u64) as usize;
            if span == 0 {
                // The source still claims an event is due at a frame we have already
                // dispatched. Force the clock forward by one frame and keep rendering;
                // never panic — a panic in an AudioWorklet kills audio for the page
                // permanently (architecture §3.1 rule 2).
                self.warnings.zero_advance_forced = true;
                span = 1;
            }

            let end = offset.saturating_add(span);
            // A stopped transport freezes the entire musical state, including sample
            // cursors. Advancing voices under silence would make Play resume halfway
            // through a one-shot even though the source clock had not moved.
            if self.playing {
                let pcm: &[i16] = match &self.module {
                    Some(module) => module.pcm(),
                    None => &self.pcm,
                };
                // Before the accumulation, not after: the tap reads the positions this
                // segment is about to render from. It never touches the accumulator, so
                // the mixer's output is unchanged — which is what
                // `cargo xtask goldens --check` proves.
                #[cfg(feature = "telemetry")]
                self.scope_taps.sample_segment(&self.voices, pcm, offset, span);
                if let Some(window) = self.accumulator.get_mut(offset..end) {
                    let scratch = self.muted_scratch.get_mut(offset..end).unwrap_or(&mut []);
                    let channels = &self.channels;
                    let is_muted = |channel: u8| channels.get(ChannelId(channel as u16)).is_some_and(|lane| lane.muted);
                    self.voices.accumulate_masked::<Path, Interp>(pcm, window, scratch, is_muted);
                }
            }

            self.frame = self.frame.saturating_add(span as u64);
            if self.playing {
                self.source_frame = self.source_frame.saturating_add(span as u64);
                self.sources.advance_to(self.source_frame);
            }
            offset = end;
        }

        // Publish the quantum's 32 buckets per channel. One `Relaxed` store per channel;
        // a reader racing it sees a torn window, which is invisible on a scope.
        #[cfg(feature = "telemetry")]
        self.scope_taps.end_quantum();

        // A voice that ended during the quantum leaves a stale handle behind; clearing it
        // here is what makes `ChannelTable::is_sounding` — the original's `_ActiveFlag`,
        // which the tone-portamento decision reads — agree with the pool.
        self.channels.release_finished(&self.voices);

        // ── DSP, on whole quanta only ───────────────────────────────────────────────
        // The per-channel insert hook is a no-op until the DSP graph lands (M7), but the
        // call site exists and takes a whole quantum so that there is no ragged-segment
        // shape for it to slip into. The master bus is real from M1-B5: master volume,
        // then the limiter.
        Self::process_channel_inserts(&mut self.accumulator);
        let master_settings = MasterSettings { volume: self.master_volume, limiter: self.limiter };
        Self::process_master_bus(&mut self.accumulator, master_settings);

        let accumulator = &self.accumulator;
        self.ring.refill_with(|destination| Out::convert(accumulator, destination));
    }

    /// Apply up to [`MAX_COMMANDS_PER_QUANTUM`] queued commands.
    fn drain_commands(&mut self) {
        for _ in 0..MAX_COMMANDS_PER_QUANTUM {
            let Some(command) = self.commands.pop() else { break };
            self.apply_command(command);
        }
    }

    fn apply_command(&mut self, command: Command<Module>) {
        match command {
            // The garbage-channel case, and the reason the channel exists: dropping the
            // last `Arc<Module>` here would call `free()` inside the audio callback.
            Command::LoadModule(module) => {
                // Every live voice indexes the PCM of the module that triggered it, so it
                // must go before the new PCM becomes visible; otherwise it reads the new
                // module's samples at the old offsets. The bindings go with it; the
                // channels' mute flags are the host's and stay.
                self.voices.release_all();
                self.channels.forget_all();
                if let Some(retired) = self.module.replace(module)
                    && let Err(orphan) = self.garbage.retire(retired)
                {
                    // Last resort. The control side has stopped collecting, so there is
                    // nowhere else for this to go and the alternative — growing a queue —
                    // is an allocation on the audio thread.
                    self.warnings.retired_module_dropped = true;
                    drop(orphan);
                }
            }
            Command::Play => self.playing = true,
            Command::Stop => self.playing = false,
            Command::SetMasterVolume(volume) => self.master_volume = volume,
            Command::MuteChannel { channel, muted } => {
                if let Some(lane) = self.channels.get_mut(channel) {
                    lane.muted = muted;
                }
            }
            // Seeking needs the concrete sequencer, which is behind `dyn EventSource`
            // here; a host that owns one calls `PatternSequencer::seek_order` directly.
            // Switching path or interpolator at run time is a re-instantiation of the
            // engine's type parameters, not a field write. Both arrive with transport
            // control in B6; flagged rather than ignored so the gap is visible.
            Command::SeekOrder(_)
            | Command::SeekRow(_)
            | Command::SeekFrame(_)
            | Command::SetAtEnd(_)
            | Command::SetInterpolator(_)
            | Command::SetTempoModel(_) => {
                self.warnings.unsupported_command = true;
            }
        }
    }

    /// Frames that may be rendered before something is due. `u64::MAX` when nothing is.
    fn frames_until_next_event(&self) -> u64 {
        if !self.playing {
            return u64::MAX;
        }
        let source_gap = match self.sources.next_event_frame() {
            Some(next) => self.source_frame.frames_until(next),
            None => u64::MAX,
        };
        let control_gap = match self.control.next_synthesised_frame() {
            Some(next) => self.source_frame.frames_until(next),
            None => u64::MAX,
        };
        source_gap.min(control_gap)
    }

    /// Dispatch everything due at the current source frame, guarded against a source that
    /// never advances.
    ///
    /// Sources go first and the synthesised control tick second, because a tracker
    /// sequencer's dispatch *takes the control clock over* — after it has run there is no
    /// synthesised tick left to take.
    fn dispatch_due_events(&mut self, events_this_quantum: &mut u32) {
        if !self.playing {
            return;
        }
        let mut zero_advance = 0u32;
        loop {
            let source_due = self.sources.next_event_frame().is_some_and(|next| next <= self.source_frame);
            let control_due = self.control.next_synthesised_frame().is_some_and(|next| next <= self.source_frame);
            if !source_due && !control_due {
                break;
            }

            if source_due {
                // Mirrored before the dispatch rather than after, because the dispatch is
                // what publishes. A flag raised *by* this tick therefore appears in the
                // next tick's snapshot; the flags are sticky, so nothing is lost.
                #[cfg(feature = "telemetry")]
                self.telemetry.set_warnings(self.warnings.into());

                let mut context = EngineContext::new(self.source_frame, &mut self.voices, &mut self.channels, &mut self.control);
                #[cfg(feature = "telemetry")]
                context.set_telemetry(&mut self.telemetry);
                #[cfg(feature = "trace")]
                context.set_trace(&mut self.trace);
                self.sources.dispatch(self.source_frame, &mut context);
            }
            // Re-read rather than reusing `control_due`: the dispatch above may have taken
            // the clock over, in which case there is no synthesised tick to take.
            if self.control.next_synthesised_frame().is_some_and(|next| next <= self.source_frame) {
                self.control.tick_synthesised();
            }

            zero_advance = zero_advance.saturating_add(1);
            *events_this_quantum = events_this_quantum.saturating_add(1);

            if zero_advance > MAX_ZERO_ADVANCE {
                self.warnings.zero_advance_forced = true;
                break;
            }
            if *events_this_quantum > MAX_EVENTS_PER_BLOCK {
                self.warnings.event_limit_reached = true;
                break;
            }
        }
    }

    /// Per-channel insert chains. A no-op until the DSP graph lands; the signature is the
    /// point.
    fn process_channel_inserts(quantum: &mut [Path::Accumulator]) {
        debug_assert_eq!(quantum.len(), RENDER_QUANTUM, "DSP only ever sees whole quanta");
        let _ = quantum;
    }

    /// The master bus: master volume, then the limiter, on a whole quantum (M1-B5).
    fn process_master_bus(quantum: &mut [Path::Accumulator], settings: MasterSettings) {
        debug_assert_eq!(quantum.len(), RENDER_QUANTUM, "DSP only ever sees whole quanta");
        Path::master(quantum, settings);
    }
}

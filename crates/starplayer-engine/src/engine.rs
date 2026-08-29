//! The render loop: [`RENDER_QUANTUM`], the [`Engine`], and the zero-advance guard.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::Frame;
use starplayer_dsp::Interpolate;
use starplayer_mixer::{MixPath, OutputFormat, VoicePool};

use crate::ring::OutputRing;
use crate::source::{EngineContext, EventSource, SilentSource};

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
}

impl EngineWarnings {
    /// Whether anything has been flagged.
    pub const fn any(self) -> bool { self.zero_advance_forced || self.event_limit_reached }

    /// Take the flags and reset them.
    pub fn take(&mut self) -> EngineWarnings { core::mem::take(self) }
}

/// The render loop.
///
/// # The three type parameters
///
/// All three are monomorphised, so the inner loop contains no `dyn` call and no branch on
/// configuration:
///
/// * `Path` — [`FloatPath`](starplayer_mixer::FloatPath) or
///   [`FixedPath`](starplayer_mixer::FixedPath): how voices accumulate.
/// * `Interp` — [`Linear`](starplayer_dsp::Linear) or
///   [`Nearest`](starplayer_dsp::Nearest): the resampling kernel.
/// * `Out` — the host's sample format, tied to `Path` by its accumulator type.
///
/// Selecting them at run time from [`Command`](starplayer_core::Command) is a `match` at
/// the top of `render()` over a handful of instantiations, and lands with the command
/// queue in M1.
///
/// # Real-time safety
///
/// Every allocation happens in [`Engine::new`] and in the off-thread setters. `render()`
/// allocates nothing, locks nothing, and cannot panic: a source that misbehaves trips the
/// zero-advance guard, and sample data that does not resolve makes a voice end. See the
/// `TODO` at the top of [`Engine::render`] for the CI hook that will prove the first of
/// those rather than asserting it by inspection.
pub struct Engine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    voices: VoicePool,
    /// The module's PCM blob.
    ///
    /// **M1-task-B1 replaces this with the `Arc<Module>` the control plane swaps in**, at
    /// which point retired modules go back over the garbage channel to be dropped off the
    /// audio thread (architecture §8). A plain `Vec` is enough for M0 because nothing
    /// swaps it while audio is running.
    pcm: Vec<i16>,
    /// One whole quantum of accumulated frames. Allocated once.
    accumulator: Box<[Path::Accumulator]>,
    ring: OutputRing<Out::Sample>,
    source: Box<dyn EventSource>,
    frame: Frame,
    warnings: EngineWarnings,
    interpolator: core::marker::PhantomData<Interp>,
}

impl<Path, Interp, Out> Engine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    /// An engine with `voice_capacity` voices, no module loaded and nothing to play.
    ///
    /// This is where **every** allocation the engine performs happens: the voice pool,
    /// the quantum accumulator and the output ring.
    pub fn new(voice_capacity: usize) -> Engine<Path, Interp, Out> {
        Engine {
            voices: VoicePool::new(voice_capacity),
            pcm: Vec::new(),
            accumulator: vec![Path::Accumulator::default(); RENDER_QUANTUM].into_boxed_slice(),
            ring: OutputRing::new(RENDER_QUANTUM * Out::CHANNELS),
            source: Box::new(SilentSource),
            frame: Frame::ZERO,
            warnings: EngineWarnings::default(),
            interpolator: core::marker::PhantomData,
        }
    }

    /// Install the module's PCM blob. Off the audio thread; M1 makes this a command.
    pub fn set_pcm(&mut self, pcm: Vec<i16>) { self.pcm = pcm; }

    /// Install the event source. Off the audio thread; M4 replaces the single source with
    /// a mux.
    pub fn set_source(&mut self, source: Box<dyn EventSource>) { self.source = source; }

    /// The voice pool, for whoever is binding channels to voices.
    pub fn voices(&self) -> &VoicePool { &self.voices }

    /// The voice pool, mutably.
    pub fn voices_mut(&mut self) -> &mut VoicePool { &mut self.voices }

    /// The engine clock: how many frames have been rendered since construction.
    pub fn frame(&self) -> Frame { self.frame }

    /// Sticky warnings raised by the render loop.
    pub fn warnings(&self) -> EngineWarnings { self.warnings }

    /// Read and clear the warnings.
    pub fn take_warnings(&mut self) -> EngineWarnings { self.warnings.take() }

    /// Fill `host_output` with interleaved samples in `Out`'s format.
    ///
    /// Any length is accepted, including one that is not a whole number of frames or
    /// quanta: the output ring carries the remainder of a quantum between calls, so
    /// output is identical whatever block sizes the host uses. That is the invariant
    /// `tests/block_size_determinism.rs` exists to hold in place.
    pub fn render(&mut self, host_output: &mut [Out::Sample]) {
        // TODO(M2-task-C7): wrap this body in `assert_no_alloc` once the allocator hook
        // lands. Until then the no-allocation property is held by inspection: everything
        // below indexes into buffers allocated in `Engine::new`, and nothing on the path
        // constructs a collection.
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
        for accumulated in self.accumulator.iter_mut() {
            *accumulated = Path::Accumulator::default();
        }

        let mut offset = 0usize;
        let mut events_this_quantum = 0u32;

        while offset < RENDER_QUANTUM {
            self.dispatch_due_events(&mut events_this_quantum);

            // Recomputed after every dispatch, never cached across one: a tempo or jump
            // effect changes when the next tick is from inside the tick just processed
            // (architecture §3.1 rule 1).
            let gap = match self.source.next_event_frame() {
                Some(next) => self.frame.frames_until(next),
                None => u64::MAX,
            };
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
            if let Some(window) = self.accumulator.get_mut(offset..end) {
                self.voices.accumulate::<Path, Interp>(&self.pcm, window);
            }

            self.frame = self.frame.saturating_add(span as u64);
            self.source.advance_to(self.frame);
            offset = end;
        }

        // ── DSP, on whole quanta only ───────────────────────────────────────────────
        // Both hooks are no-ops in M0 (task A3 is explicit that no real DSP lands here),
        // but the call sites exist and take a whole quantum so that the day a per-channel
        // insert or the master bus arrives, there is no ragged-segment shape for it to
        // slip into.
        Self::process_channel_inserts(&mut self.accumulator);
        Self::process_master_bus(&mut self.accumulator);

        let accumulator = &self.accumulator;
        self.ring.refill_with(|destination| Out::convert(accumulator, destination));
    }

    /// Dispatch everything due at the current frame, guarded against a source that never
    /// advances.
    fn dispatch_due_events(&mut self, events_this_quantum: &mut u32) {
        let mut zero_advance = 0u32;
        while self.source.next_event_frame().is_some_and(|next| next <= self.frame) {
            let mut context = EngineContext { frame: self.frame, voices: &mut self.voices };
            self.source.dispatch(self.frame, &mut context);

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

    /// The master bus. A no-op until the DSP graph lands.
    fn process_master_bus(quantum: &mut [Path::Accumulator]) {
        debug_assert_eq!(quantum.len(), RENDER_QUANTUM, "DSP only ever sees whole quanta");
        let _ = quantum;
    }
}

//! [`EventSource`] — how anything that has something to say at a particular frame says it
//! (architecture §3).
//!
//! # Pull, with absolute frames
//!
//! A source is asked *when* its next event is, advanced to a frame, and told to dispatch.
//! Absolute frames, not deltas: one engine-owned `u64` clock (twelve million years at
//! 48 kHz) instead of every source keeping its own elapsed-time bookkeeping and
//! eventually desyncing from the others.
//!
//! # Why this trait exists before its second implementation
//!
//! `EventSource` is one of the two exceptions to "no trait until its second real
//! implementation exists" (architecture §10.1), because it has several from the start:
//! the pattern sequencer, an external queue for live MIDI and plugin-host events, an SMF
//! sequencer, and the control clock that advances envelopes. M0 lands two of the trivial
//! ones — [`SilentSource`] and [`ScriptedSource`] — so the shape is exercised rather than
//! merely declared.
//!
//! # What M4 adds
//!
//! Several sources at once, merged with the §3.1.3 deterministic tie-break
//! `(frame, source_slot, sequence_within_source)`. M0 drives exactly one source, so
//! ordering has nothing to decide.

use alloc::vec::Vec;

use starplayer_core::{Frame, VoiceId, VoiceParam};
use starplayer_mixer::VoicePool;

/// What [`EventSource::dispatch`] is given access to.
///
/// A **concrete** type, not `&mut dyn EventSink`: only `EventSource` itself needs to be
/// `dyn` (sources are heterogeneous), and dyn dispatch at tick rate — roughly 50 Hz — is
/// free, while a dyn call per parameter write would not be.
///
/// M1 widens this to the channel table, the tempo clock and the telemetry publisher. It
/// deliberately does **not** widen to the output buffer: a source says *what* happens,
/// never *what it sounds like*.
pub struct EngineContext<'engine> {
    /// The frame being dispatched. Equal to the engine clock.
    pub frame: Frame,
    /// The global voice pool.
    pub voices: &'engine mut VoicePool,
}

/// Something that produces events at absolute frames.
pub trait EventSource {
    /// Absolute frame of the next event, or `None` if idle.
    ///
    /// Must never return a frame earlier than the engine clock. The engine tolerates one
    /// that does — it treats it as due immediately — because `render()` may not panic,
    /// but it is a bug in the source.
    ///
    /// **Never cached across a dispatch** by the engine (architecture §3.1 rule 1):
    /// `Txx`, `Axx`, `SEx`, `SDx`, `Bxx` and `Cxx` all change when the next tick is from
    /// inside the tick just processed.
    fn next_event_frame(&self) -> Option<Frame>;

    /// Advance internal state to `frame` without producing events.
    ///
    /// `frame` is never past [`EventSource::next_event_frame`].
    fn advance_to(&mut self, frame: Frame);

    /// Produce everything due exactly at `frame`.
    ///
    /// Only called when [`EventSource::next_event_frame`] reports this frame as due.
    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>);
}

/// A source with nothing to say. What a fresh [`Engine`](crate::Engine) starts with, so
/// that "no source yet" needs no `Option` in the render loop.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SilentSource;

impl EventSource for SilentSource {
    fn next_event_frame(&self) -> Option<Frame> { None }
    fn advance_to(&mut self, _frame: Frame) {}
    fn dispatch(&mut self, _frame: Frame, _context: &mut EngineContext<'_>) {}
}

/// One scripted parameter write.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ScriptedAction {
    /// When it happens.
    pub frame: Frame,
    /// Which voice it addresses. A stale handle is ignored, not an error.
    pub voice: VoiceId,
    /// What it sets.
    pub param: VoiceParam,
}

impl ScriptedAction {
    /// A parameter write at `frame`.
    pub const fn new(frame: Frame, voice: VoiceId, param: VoiceParam) -> ScriptedAction {
        ScriptedAction { frame, voice, param }
    }
}

/// A fixed list of parameter writes at known frames.
///
/// The stand-in for the pattern sequencer until M1 builds one, and the thing that lets
/// the block-size determinism test assert that an event lands on the frame it was
/// scheduled for rather than on the nearest buffer boundary. It exists in the library
/// rather than in the test because every later format crate will want the same fixture.
///
/// Actions are dispatched in list order within a frame, which is all the tie-breaking a
/// single source needs; [`ScriptedSource::new`] sorts by frame so an unsorted list is not
/// a trap.
#[derive(Clone, Debug, Default)]
pub struct ScriptedSource {
    actions: Vec<ScriptedAction>,
    next: usize,
}

impl ScriptedSource {
    /// A source that plays `actions`, in frame order.
    pub fn new(actions: Vec<ScriptedAction>) -> ScriptedSource {
        let mut actions = actions;
        actions.sort_by_key(|action| action.frame);
        ScriptedSource { actions, next: 0 }
    }

    /// How many actions have not been dispatched yet.
    pub fn remaining(&self) -> usize { self.actions.len().saturating_sub(self.next) }
}

impl EventSource for ScriptedSource {
    fn next_event_frame(&self) -> Option<Frame> { self.actions.get(self.next).map(|action| action.frame) }

    fn advance_to(&mut self, _frame: Frame) {}

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        while let Some(action) = self.actions.get(self.next) {
            if action.frame > frame {
                break;
            }
            if let Some(voice) = context.voices.get_mut(action.voice) {
                action.param.apply(&mut voice.params);
            }
            self.next += 1;
        }
    }
}

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
//! # Several sources at once
//!
//! [`SourceMux`] merges any number of sources with the §3.1.3 deterministic tie-break
//! `(frame, source_slot, sequence_within_source)`. It is here from M1 rather than M4
//! (task B3, research point 2) because it is a hundred lines and because the tie-break
//! rule is only testable once two real sources can collide — "play a module and jam MIDI
//! over it" is the use case, but *proving the ordering is stable* is the reason it is
//! worth having early.

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::{Frame, VoiceId, VoiceParam};
use starplayer_mixer::VoicePool;

use crate::channel::ChannelTable;
use crate::control::ControlClock;

/// What [`EventSource::dispatch`] is given access to.
///
/// A **concrete** type, not `&mut dyn EventSink`: only `EventSource` itself needs to be
/// `dyn` (sources are heterogeneous), and dyn dispatch at tick rate — roughly 50 Hz — is
/// free, while a dyn call per parameter write would not be.
///
/// It deliberately does **not** widen to the output buffer: a source says *what* happens,
/// never *what it sounds like*. The telemetry publisher joined it in B6, and is the one
/// exception: it is write-only, and a source may only *describe* itself through it.
///
/// # Build one with [`EngineContext::new`]
///
/// The telemetry field only exists under `feature = "telemetry"`, so a struct literal
/// would compile under one feature set and not the other. [`EngineContext::new`] compiles
/// under both and is the only supported way to make one.
pub struct EngineContext<'engine> {
    /// The frame being dispatched. Equal to the engine's source clock.
    pub frame: Frame,
    /// The global voice pool.
    pub voices: &'engine mut VoicePool,
    /// The channel-to-voice bindings.
    pub channels: &'engine mut ChannelTable,
    /// The clock envelopes advance on. A tracker sequencer takes it over by calling
    /// [`ControlClock::tick_from_tracker`] (architecture §5.4).
    pub control: &'engine mut ControlClock,
    /// Where a source describes itself for the UI, when the host asked for telemetry
    /// (architecture §9). `None` means nobody is watching and every report is a no-op.
    #[cfg(feature = "telemetry")]
    pub telemetry: Option<&'engine mut starplayer_telemetry::TelemetryPublisher>,
    /// Per-tick diagnostic recorder. Absent, including as a field, when tracing is off.
    #[cfg(feature = "trace")]
    pub(crate) trace: Option<&'engine mut crate::trace::TraceRecorder>,
}

impl<'engine> EngineContext<'engine> {
    /// A context with no telemetry attached.
    pub fn new(
        frame: Frame,
        voices: &'engine mut VoicePool,
        channels: &'engine mut ChannelTable,
        control: &'engine mut ControlClock,
    ) -> EngineContext<'engine> {
        EngineContext {
            frame,
            voices,
            channels,
            control,
            #[cfg(feature = "telemetry")]
            telemetry: None,
            #[cfg(feature = "trace")]
            trace: None,
        }
    }

    /// Attach a telemetry publisher, so everything dispatched through this context
    /// describes itself for the UI.
    #[cfg(feature = "telemetry")]
    pub fn set_telemetry(&mut self, telemetry: &'engine mut starplayer_telemetry::TelemetryPublisher) {
        self.telemetry = Some(telemetry);
    }

    /// Attach the engine-owned diagnostic recorder.
    #[cfg(feature = "trace")]
    pub(crate) fn set_trace(&mut self, trace: &'engine mut crate::trace::TraceRecorder) {
        self.trace = Some(trace);
    }

    /// Apply one absolute parameter write and feed the trace hook in diagnostic builds.
    /// A stale voice remains a no-op.
    #[inline]
    pub fn write_voice_param(&mut self, voice: VoiceId, param: VoiceParam) {
        #[cfg(feature = "trace")]
        let channel = self.voices.get(voice).map(|state| state.tag.channel);
        let Some(state) = self.voices.get_mut(voice) else { return };
        param.apply(&mut state.params);
        #[cfg(feature = "trace")]
        if let (Some(trace), Some(channel)) = (self.trace.as_deref_mut(), channel) {
            let flag = match param {
                VoiceParam::Step(_) | VoiceParam::Filter(_) => starplayer_core::DirtyBits::PITCH,
                VoiceParam::Volume(_) => starplayer_core::DirtyBits::VOLUME,
                VoiceParam::Pan(_) => starplayer_core::DirtyBits::PAN,
            };
            trace.record_channel_flags(starplayer_core::ChannelId(channel as u16), flag);
        }
    }

    /// Reborrow, so a source can hand a shorter-lived context to something it owns.
    pub fn reborrow(&mut self) -> EngineContext<'_> {
        EngineContext {
            frame: self.frame,
            voices: self.voices,
            channels: self.channels,
            control: self.control,
            #[cfg(feature = "telemetry")]
            telemetry: self.telemetry.as_deref_mut(),
            #[cfg(feature = "trace")]
            trace: self.trace.as_deref_mut(),
        }
    }
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
            context.write_voice_param(action.voice, action.param);
            self.next += 1;
        }
    }
}

// ── several sources at once ─────────────────────────────────────────────────────────

/// A stable handle to one source inside a [`SourceMux`].
///
/// **Generational, not a `Vec` position** (architecture §3.1 rule 3). A `Vec` index shifts
/// when an earlier element is removed, which would silently reorder every later source's
/// tie-break priority the moment a MIDI input was disconnected. A generational slot keeps
/// its number for the life of the mux and reports a stale handle as gone.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceSlot {
    index: u16,
    generation: u16,
}

impl SourceSlot {
    /// Slot number. This is the tie-break key: lower slots dispatch first.
    pub const fn index(self) -> u16 { self.index }

    /// Generation counter, bumped every time the slot is reused.
    pub const fn generation(self) -> u16 { self.generation }
}

struct MuxSlot {
    source: Option<Box<dyn EventSource>>,
    generation: u16,
}

impl MuxSlot {
    const fn empty() -> MuxSlot { MuxSlot { source: None, generation: 0 } }
}

/// Several [`EventSource`]s, merged, with a dispatch order that never depends on anything
/// but the data.
///
/// # Rule 3: deterministic tie-breaking
///
/// Two sources with an event at the same frame must dispatch in a stable order, or offline
/// rendering stops matching real time and neither matches across hosts. The order is
/// `(frame, source_slot, sequence_within_source)`:
///
/// * **frame** — the engine only ever asks for one frame at a time, so this is settled
///   before the mux is involved;
/// * **source_slot** — [`SourceMux::dispatch`] walks slots in ascending index, always,
///   whatever order they were inserted or removed in;
/// * **sequence_within_source** — each source dispatches everything it has due at that
///   frame in its own internal order, which is its business and is deterministic by the
///   same argument.
///
/// # Fixed capacity
///
/// The slot array is allocated once. Inserting a source is a control-plane operation and
/// returns `None` when the mux is full rather than growing, because growing would be an
/// allocation on a structure the audio thread is reading.
pub struct SourceMux {
    slots: Box<[MuxSlot]>,
}

impl SourceMux {
    /// How many sources one engine can merge by default: a pattern sequencer, an SMF
    /// sequencer, live MIDI in, a keyboard, a plugin-host queue, and room to spare.
    pub const DEFAULT_CAPACITY: usize = 8;

    /// A mux with room for `capacity` sources, allocated once.
    pub fn new(capacity: usize) -> SourceMux {
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, MuxSlot::empty);
        SourceMux { slots: slots.into_boxed_slice() }
    }

    /// How many sources the mux can hold.
    pub fn capacity(&self) -> usize { self.slots.len() }

    /// How many sources are installed.
    pub fn len(&self) -> usize { self.slots.iter().filter(|slot| slot.source.is_some()).count() }

    /// Whether no sources are installed.
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Install `source` in the lowest free slot, or hand it back if the mux is full.
    ///
    /// The lowest free slot, rather than the next one along, so that a mux built the same
    /// way twice has the same layout twice — which is what makes the tie-break reproducible
    /// across runs.
    pub fn insert(&mut self, source: Box<dyn EventSource>) -> Result<SourceSlot, Box<dyn EventSource>> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.source.is_none() {
                slot.source = Some(source);
                return Ok(SourceSlot { index: index as u16, generation: slot.generation });
            }
        }
        Err(source)
    }

    /// Take a source back out. Returns `None` for a handle whose slot has been reused.
    pub fn remove(&mut self, slot: SourceSlot) -> Option<Box<dyn EventSource>> {
        let entry = self.slots.get_mut(slot.index as usize)?;
        if entry.generation != slot.generation {
            return None;
        }
        let source = entry.source.take()?;
        entry.generation = entry.generation.wrapping_add(1);
        Some(source)
    }

    /// Whether `slot` still refers to a live source.
    pub fn contains(&self, slot: SourceSlot) -> bool {
        self.slots.get(slot.index as usize).is_some_and(|entry| entry.generation == slot.generation && entry.source.is_some())
    }

    /// Remove every source, invalidating every handle.
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.source.take().is_some() {
                slot.generation = slot.generation.wrapping_add(1);
            }
        }
    }
}

impl EventSource for SourceMux {
    fn next_event_frame(&self) -> Option<Frame> {
        self.slots.iter().filter_map(|slot| slot.source.as_ref()?.next_event_frame()).min()
    }

    fn advance_to(&mut self, frame: Frame) {
        for slot in self.slots.iter_mut() {
            if let Some(source) = slot.source.as_mut() {
                source.advance_to(frame);
            }
        }
    }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        // Ascending slot index, unconditionally: this loop *is* the tie-break rule.
        for slot in self.slots.iter_mut() {
            let Some(source) = slot.source.as_mut() else { continue };
            if source.next_event_frame().is_some_and(|next| next <= frame) {
                source.dispatch(frame, context);
            }
        }
    }
}

impl core::fmt::Debug for SourceMux {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("SourceMux").field("len", &self.len()).field("capacity", &self.capacity()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlClock;
    use alloc::rc::Rc;
    use alloc::vec;
    use core::cell::RefCell;

    /// Records the order in which sources were dispatched, so the tie-break can be
    /// observed rather than inferred. `Rc` rather than a borrow because
    /// `Box<dyn EventSource>` is `'static`.
    type DispatchLog = Rc<RefCell<Vec<u8>>>;

    struct MarkingSource {
        frames: Vec<Frame>,
        next: usize,
        mark: u8,
        log: DispatchLog,
    }

    impl MarkingSource {
        fn boxed(mark: u8, frames: Vec<Frame>, log: &DispatchLog) -> Box<dyn EventSource> {
            Box::new(MarkingSource { frames, next: 0, mark, log: Rc::clone(log) })
        }
    }

    impl EventSource for MarkingSource {
        fn next_event_frame(&self) -> Option<Frame> { self.frames.get(self.next).copied() }
        fn advance_to(&mut self, _frame: Frame) {}
        fn dispatch(&mut self, frame: Frame, _context: &mut EngineContext<'_>) {
            while self.frames.get(self.next).is_some_and(|due| *due <= frame) {
                self.log.borrow_mut().push(self.mark);
                self.next = self.next.saturating_add(1);
            }
        }
    }

    /// Drive the mux by hand, the way the engine's render loop does, up to `until`.
    fn drive(mux: &mut SourceMux, until: Frame) {
        let mut voices = VoicePool::new(1);
        let mut channels = ChannelTable::new(1);
        let mut control = ControlClock::new(44_100, Frame::ZERO);
        for _ in 0..1_000 {
            let Some(frame) = mux.next_event_frame() else { break };
            if frame > until {
                break;
            }
            let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
            mux.dispatch(frame, &mut context);
            mux.advance_to(frame);
        }
    }

    #[test]
    fn the_next_frame_is_the_earliest_across_every_source() {
        let log: DispatchLog = Rc::new(RefCell::new(Vec::new()));
        let mut mux = SourceMux::new(4);
        assert!(mux.is_empty());
        assert_eq!(mux.next_event_frame(), None, "an empty mux is idle");

        assert!(mux.insert(MarkingSource::boxed(b'a', vec![Frame(90)], &log)).is_ok());
        assert!(mux.insert(MarkingSource::boxed(b'b', vec![Frame(30)], &log)).is_ok());
        assert_eq!(mux.next_event_frame(), Some(Frame(30)));
        assert_eq!(mux.len(), 2);
    }

    #[test]
    fn sources_at_the_same_frame_dispatch_in_slot_order_not_insertion_order() {
        let log: DispatchLog = Rc::new(RefCell::new(Vec::new()));
        let mut mux = SourceMux::new(4);
        let first = mux.insert(MarkingSource::boxed(b'0', vec![Frame(10)], &log)).ok();
        assert!(mux.insert(MarkingSource::boxed(b'1', vec![Frame(10), Frame(20)], &log)).is_ok());

        drive(&mut mux, Frame(10));
        assert_eq!(*log.borrow(), vec![b'0', b'1'], "ascending slot index");

        let slot = first.expect("slot 0");
        assert!(mux.contains(slot));
        assert!(mux.remove(slot).is_some());
        assert!(!mux.contains(slot), "the handle went stale with the slot's generation");
        assert!(mux.remove(slot).is_none(), "removing twice is a no-op, not a corruption");

        log.borrow_mut().clear();
        assert!(mux.insert(MarkingSource::boxed(b'2', vec![Frame(20)], &log)).is_ok());
        drive(&mut mux, Frame(100));
        assert_eq!(*log.borrow(), vec![b'2', b'1'], "the replacement took slot 0 and therefore dispatches first");
    }

    #[test]
    fn a_full_mux_hands_the_source_back() {
        let log: DispatchLog = Rc::new(RefCell::new(Vec::new()));
        let mut mux = SourceMux::new(1);
        assert!(mux.insert(MarkingSource::boxed(b'a', Vec::new(), &log)).is_ok());
        assert!(mux.insert(MarkingSource::boxed(b'b', Vec::new(), &log)).is_err());
        assert_eq!(mux.len(), 1);
        assert_eq!(mux.capacity(), 1);

        mux.clear();
        assert!(mux.is_empty());
        assert!(mux.insert(Box::new(SilentSource)).is_ok(), "clearing frees the slots");
    }

    #[test]
    fn a_silent_source_is_idle_and_a_scripted_one_reports_its_first_action() {
        assert_eq!(SilentSource.next_event_frame(), None);
        let actions = vec![
            ScriptedAction::new(Frame(90), VoiceId::new(0, 0), VoiceParam::Volume(starplayer_core::U0F16::MAX)),
            ScriptedAction::new(Frame(30), VoiceId::new(0, 0), VoiceParam::Volume(starplayer_core::U0F16::ZERO)),
        ];
        let source = ScriptedSource::new(actions);
        assert_eq!(source.next_event_frame(), Some(Frame(30)), "the constructor sorts by frame");
        assert_eq!(source.remaining(), 2);
    }
}

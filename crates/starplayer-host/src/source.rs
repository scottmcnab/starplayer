//! The playback source every host installs, and the mailbox it reads seeks from.
//!
//! Lifted out of the browser host (task D4, research point 1), because none of it was ever
//! browser-specific: it is the answer to a question every host asks. `Engine` stores
//! `Box<dyn EventSource>` on purpose (architecture §1.2, §3), so its typed command handler
//! cannot downcast to a `PatternSequencer` and cannot seek. [`SeekableModuleSource`] is the
//! safe bridge — the command stays a typed `Command`, but order, row and frame seeks become
//! one fixed-size mailbox write that the wrapper consumes at an event boundary.
//!
//! # Why the mailbox is atomic
//!
//! The browser host used `Rc<Cell<SeekRequest>>`, which is exactly right for a worklet: one
//! realm, one thread, no synchronisation to pay for. A native host writes the mailbox from
//! the control thread and reads it inside the audio callback, so the same slot has to cross
//! a thread boundary — and [`EventSource`] is `Send` precisely because the engine travels
//! to the audio thread with its sources. Three relaxed atomics with one release/acquire
//! edge on the discriminant do the job with no lock and no allocation.

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::boxed::Box;
use std::sync::Arc;

use starplayer::core::quirks::QuirkSelection;
use starplayer::core::{AtEnd, Error, Frame};
use starplayer::engine::{EngineContext, EventSource, ScanLimits};
use starplayer::model::Module;
use starplayer::rt::Arc as RtArc;
use starplayer::{NativeSequencer, ScannedSong};

/// Where a seek wants to land.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SeekKind {
    /// Nothing is pending.
    #[default]
    None,
    /// An index into the order list.
    Order(u16),
    /// A row of the pattern already playing.
    Row(u16),
    /// An elapsed position in the song, in frames from the start of the current pass.
    Frame(u64),
}

/// A seek, and the frame the source should apply it at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeekRequest {
    /// What to seek to.
    pub kind: SeekKind,
    /// The engine's source frame when the request was made; the wrapper brings the
    /// sequencer's clock back to exactly here so the musical timeline stays monotonic.
    pub frame: Frame,
}

const SEEK_NONE: u8 = 0;
const SEEK_ORDER: u8 = 1;
const SEEK_ROW: u8 = 2;
const SEEK_FRAME: u8 = 3;

/// A single-slot, lock-free mailbox for one pending seek.
///
/// Latest-wins: a second seek before the first has been consumed replaces it, which is what
/// a person dragging a progress slider means. Writes are ordered so the discriminant is
/// published last and read first, so a reader never sees a kind whose argument has not
/// arrived.
#[derive(Debug, Default)]
pub struct SeekMailbox {
    kind: AtomicU8,
    argument: AtomicU64,
    frame: AtomicU64,
}

impl SeekMailbox {
    /// An empty mailbox.
    pub fn new() -> Arc<SeekMailbox> { Arc::new(SeekMailbox::default()) }

    /// Ask for `request`, replacing anything not yet consumed.
    pub fn request(&self, request: SeekRequest) {
        let (kind, argument) = match request.kind {
            SeekKind::None => (SEEK_NONE, 0),
            SeekKind::Order(order) => (SEEK_ORDER, order as u64),
            SeekKind::Row(row) => (SEEK_ROW, row as u64),
            SeekKind::Frame(song_frame) => (SEEK_FRAME, song_frame),
        };
        self.argument.store(argument, Ordering::Relaxed);
        self.frame.store(request.frame.0, Ordering::Relaxed);
        // Published last, so the acquire load below cannot see this kind without its
        // argument.
        self.kind.store(kind, Ordering::Release);
    }

    /// What is pending, without consuming it.
    pub fn peek(&self) -> SeekRequest {
        let kind = self.kind.load(Ordering::Acquire);
        if kind == SEEK_NONE {
            return SeekRequest::default();
        }
        let argument = self.argument.load(Ordering::Relaxed);
        let frame = Frame(self.frame.load(Ordering::Relaxed));
        let kind = match kind {
            SEEK_ORDER => SeekKind::Order(argument as u16),
            SEEK_ROW => SeekKind::Row(argument as u16),
            SEEK_FRAME => SeekKind::Frame(argument),
            _ => SeekKind::None,
        };
        SeekRequest { kind, frame }
    }

    /// Forget whatever is pending.
    pub fn clear(&self) { self.kind.store(SEEK_NONE, Ordering::Release); }
}

/// The repeat setting, shared between the caller and the live source.
///
/// Read at each event boundary rather than written straight into the sequencer, so a change
/// made while the audio thread is between quanta lands cleanly.
#[derive(Debug)]
pub struct AtEndSlot(AtomicU8);

const AT_END_FADE_OUT: u8 = 0;
const AT_END_CONTINUE: u8 = 1;
const AT_END_STOP: u8 = 2;

impl AtEndSlot {
    /// A slot holding `at_end`.
    pub fn new(at_end: AtEnd) -> Arc<AtEndSlot> { Arc::new(AtEndSlot(AtomicU8::new(AtEndSlot::encode(at_end)))) }

    const fn encode(at_end: AtEnd) -> u8 {
        match at_end {
            AtEnd::FadeOut => AT_END_FADE_OUT,
            AtEnd::Continue => AT_END_CONTINUE,
            AtEnd::Stop => AT_END_STOP,
        }
    }

    /// What the host last asked for.
    pub fn get(&self) -> AtEnd {
        match self.0.load(Ordering::Relaxed) {
            AT_END_CONTINUE => AtEnd::Continue,
            AT_END_STOP => AtEnd::Stop,
            _ => AtEnd::FadeOut,
        }
    }

    /// Choose what happens at the detected loop point.
    pub fn set(&self, at_end: AtEnd) { self.0.store(AtEndSlot::encode(at_end), Ordering::Relaxed); }
}

/// A format-native tracker source with a host-owned seek mailbox.
///
/// The wrapper consumes a pending seek at an event boundary and restarts the sequencer's
/// clock on the engine's monotonic source timeline. Nothing here allocates, so it is safe
/// inside `render()`.
pub struct SeekableModuleSource {
    sequencer: NativeSequencer,
    request: Arc<SeekMailbox>,
    at_end: Arc<AtEndSlot>,
    applied_at_end: AtEnd,
}

impl SeekableModuleSource {
    /// Wrap `sequencer` around a mailbox and a repeat slot the host keeps a handle to.
    pub fn new(sequencer: NativeSequencer, request: Arc<SeekMailbox>, at_end: Arc<AtEndSlot>) -> SeekableModuleSource {
        let applied_at_end = at_end.get();
        SeekableModuleSource { sequencer, request, at_end, applied_at_end }
    }
}

impl EventSource for SeekableModuleSource {
    fn next_event_frame(&self) -> Option<Frame> {
        let request = self.request.peek();
        match request.kind {
            SeekKind::None => self.sequencer.next_event_frame(),
            _ => Some(match self.sequencer.next_event_frame() {
                Some(next) => core::cmp::min(next, request.frame),
                None => request.frame,
            }),
        }
    }

    fn advance_to(&mut self, frame: Frame) { self.sequencer.advance_to(frame); }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        let at_end = self.at_end.get();
        if at_end != self.applied_at_end {
            self.sequencer.set_at_end(at_end);
            self.applied_at_end = at_end;
        }
        let request = self.request.peek();
        if !matches!(request.kind, SeekKind::None) && frame >= request.frame {
            self.request.clear();
            // An order the module does not have, or a frame past the end of the scan, is a
            // caller-side mistake and not something the audio realm can report: the
            // sequencer stays where it was, which is the only answer that keeps playing.
            match request.kind {
                SeekKind::Order(order) => { let _ = self.sequencer.seek_order_at(order, frame); }
                SeekKind::Row(row) => self.sequencer.seek_row(row),
                SeekKind::Frame(song_frame) => { let _ = self.sequencer.seek_frame(song_frame, frame); }
                SeekKind::None => {}
            }
            self.sequencer.restart_clock_at(frame);
        }
        if self.sequencer.next_event_frame().is_some_and(|next| next <= frame) {
            self.sequencer.dispatch(frame, context);
        }
    }
}

/// The two slots a host writes transport requests through.
///
/// They belong to the **host**, not to the module: a seek mailbox is a property of the
/// thing being steered, and a repeat setting outlives whatever is loaded. Making them
/// per-module was the browser host's shape and it does not survive the crossing to a
/// native one — the audio thread would be the last owner of the outgoing pair on every
/// load, and dropping an `Arc` there is a `free()` inside the callback.
#[derive(Clone, Debug)]
pub struct SourceHandles {
    /// Where seeks are written. Latest wins.
    pub seek: Arc<SeekMailbox>,
    /// Where the repeat setting is written.
    pub at_end: Arc<AtEndSlot>,
}

impl SourceHandles {
    /// A fresh pair, with the repeat setting at `at_end`.
    pub fn new(at_end: AtEnd) -> SourceHandles { SourceHandles { seek: SeekMailbox::new(), at_end: AtEndSlot::new(at_end) } }
}

/// Everything module activation hands back: the source the engine plays, and the scan it
/// plays against.
pub struct BuiltSource {
    /// Hand this to [`Engine::replace_source`](starplayer::engine::Engine::replace_source).
    pub source: Box<dyn EventSource>,
    /// The scan the timeline and the quirks came from.
    pub scanned: Arc<ScannedSong>,
}

/// Scan a module on a throwaway sequencer, so the playback one never has to be rewound.
///
/// Off the audio thread: it allocates the scan's sequencer, voice pool and timeline.
pub fn scan_module(module: &RtArc<Module>, sample_rate_hz: u32) -> Result<ScannedSong, Error> {
    starplayer::scan_song(module, sample_rate_hz, ScanLimits::for_rate(sample_rate_hz))
}

/// Build the playback source, scanning the song first unless the caller already has the
/// scan.
///
/// A caller that already holds one passes it: a mixer-mode rebuild changes nothing the scan
/// depends on — the timeline is a function of the output rate and the module's dialect, and
/// of nothing the mixer chooses (architecture §4.1).
///
/// The scan decides the quirks — for a MOD it is the scan, not the header, that settles CIA
/// against VBlank — and the playback sequencer is built from exactly the set the timeline
/// was measured under. Never `QuirkSelection::FromDialect`.
///
/// Off the audio thread.
pub fn build_source(
    module: RtArc<Module>,
    sample_rate_hz: u32,
    start: SeekKind,
    frame: Frame,
    handles: &SourceHandles,
    cached_scan: Option<Arc<ScannedSong>>,
) -> Result<BuiltSource, Error> {
    let scanned = match cached_scan {
        Some(scanned) => scanned,
        None => Arc::new(scan_module(&module, sample_rate_hz)?),
    };
    let at_end = handles.at_end.get();
    let quirks = QuirkSelection::Override(scanned.quirks);
    let mut sequencer = NativeSequencer::new(module, sample_rate_hz, quirks)?;
    sequencer.set_timeline(scanned.timeline.clone());
    sequencer.set_at_end(at_end);
    match start {
        SeekKind::Frame(song_frame) => { let _ = sequencer.seek_frame(song_frame, frame); }
        SeekKind::Order(order) => { let _ = sequencer.seek_order_at(order, frame); }
        SeekKind::Row(row) => sequencer.seek_row(row),
        SeekKind::None => { let _ = sequencer.seek_order_at(0, frame); }
    }
    sequencer.restart_clock_at(frame);
    let source = SeekableModuleSource::new(sequencer, Arc::clone(&handles.seek), Arc::clone(&handles.at_end));
    Ok(BuiltSource { source: Box::new(source), scanned })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mailbox_round_trips_every_kind_and_clears() {
        let mailbox = SeekMailbox::new();
        assert_eq!(mailbox.peek(), SeekRequest::default());

        for kind in [SeekKind::Order(7), SeekKind::Row(31), SeekKind::Frame(u32::MAX as u64 + 9)] {
            mailbox.request(SeekRequest { kind, frame: Frame(1_234) });
            assert_eq!(mailbox.peek(), SeekRequest { kind, frame: Frame(1_234) });
        }
        mailbox.clear();
        assert_eq!(mailbox.peek().kind, SeekKind::None);
    }

    #[test]
    fn the_latest_seek_replaces_one_that_has_not_been_consumed() {
        let mailbox = SeekMailbox::new();
        mailbox.request(SeekRequest { kind: SeekKind::Order(1), frame: Frame(10) });
        mailbox.request(SeekRequest { kind: SeekKind::Frame(500), frame: Frame(20) });
        assert_eq!(mailbox.peek(), SeekRequest { kind: SeekKind::Frame(500), frame: Frame(20) }, "a slider drag is not a queue");
    }

    #[test]
    fn the_at_end_slot_round_trips_every_setting() {
        let slot = AtEndSlot::new(AtEnd::Continue);
        assert_eq!(slot.get(), AtEnd::Continue);
        for at_end in [AtEnd::FadeOut, AtEnd::Stop, AtEnd::Continue] {
            slot.set(at_end);
            assert_eq!(slot.get(), at_end);
        }
    }
}

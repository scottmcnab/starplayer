//! Song length, loop detection and the elapsed-time clock a media player's progress
//! slider is drawn from.
//!
//! A tracked module does not declare how long it is. Order lists run off their end and
//! wrap, `Bxx` jumps backwards mid-song, `SBx`/`E6x` pattern loops revisit rows, and
//! `Txx`/`Axx` change how long a tick lasts while the song is playing. "How long is this
//! module?" therefore has no answer that can be read out of the file — it has to be
//! *played*.
//!
//! # Prior art
//!
//! * OpenMPT's `CSoundFile::GetLength` walks the order list running a subset of the real
//!   player, with a `RowVisitor` set of visited `(order, row)` pairs; the song ends at the
//!   first row it reaches twice. Rows reached while an `E6x` pattern loop is running are
//!   exempt, and a complexity counter bails out of deliberately pathological loops.
//!   libopenmpt then exposes `set_repeat_count` and `play.at_end = fadeout | continue |
//!   stop` on top of it.
//! * libxmp's `scan_module` does the same with `scan_cnt[ord][row]` counters and an
//!   `inside_loop` flag, and stores a per-order `time`/`speed`/`bpm` so that seeking to an
//!   order restores the timing that order was reached with.
//!
//! # What is different here
//!
//! The scan runs the **real** [`PatternSequencer`] with the **real** format processor and
//! no mixing, so the timeline is correct by construction under every quirk, dialect and
//! tempo model — there is no second, simplified player to drift out of step with the
//! first. And the *live* sequencer carries the *same* [`LoopDetector`], so "the song has
//! been heard once through" fires on exactly the frame the scan predicted.
//!
//! # What is allocated where
//!
//! [`LoopDetector::new`] allocates its bitset once, when the sequencer is built.
//! [`SongTimeline`]'s vectors are filled by [`scan_timeline`], off the audio thread.
//! Nothing in this module allocates from `dispatch`: [`LoopDetector::visit`] and
//! [`LoopDetector::reset_marking_before`] are bit operations over buffers that already
//! exist.

use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::{AtEnd, Frame, TempoModel};
use starplayer_mixer::VoicePool;

use crate::channel::ChannelTable;
use crate::control::ControlClock;
use crate::sequencer::{OrderEntry, PatternData, PatternSequencer, SongPosition, TrackerProcessor};
use crate::source::{EngineContext, EventSource};

/// Rows of any one pattern the loop detector can tell apart.
///
/// Every format in scope tops out well below this (ProTracker 64, ST3 up to 255), so the
/// cap only ever bites on fuzzed data. A row past it is treated as unvisited and never
/// marked, which can only ever *delay* the detected loop point, never invent one.
pub const MAX_ROWS_PER_ORDER: u16 = 256;

/// Pattern-loop arrivals in a row before the detector gives up.
///
/// `SBx` and `E6x` bodies are exempt from the repeat check — that is the whole reason
/// `inside_loop` exists — so a construction that loops for ever inside one pattern would
/// otherwise scan until the tick limit. ProTracker's shared loop counter makes those
/// reachable in real modules, not only in fuzzed ones.
pub const MAX_PATTERN_LOOP_ARRIVALS: u32 = 4_096;

/// How the sequencer arrived at the row it is about to play.
///
/// The loop detector needs this because "I have played this row before" only means the
/// song has looped when the row was reached by *moving through the song*. A row revisited
/// by an `SBx` pattern loop is the loop doing its job.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum RowArrival {
    /// The song was just started, or seeked. Nothing before this row is continuous with it.
    #[default]
    Start,
    /// The next row of the same pattern.
    Sequential,
    /// The first row of the next order, because the pattern ran out.
    NextOrder,
    /// A `Bxx` / `Cxx` / `Dxx` jump through the order list.
    Jump,
    /// An `SBx` / `E6x` pattern loop, back to a row of the pattern already playing.
    PatternLoop,
    /// The order list ran out and [`EndOfSongPolicy::Loop`](crate::EndOfSongPolicy)
    /// restarted it.
    ///
    /// **This is the end of the song, not a loop point.** Running off the end of the order
    /// list is not something the music asks for — it is what is left when the music stops
    /// asking for anything — so it ends the pass whatever row the restart order names and
    /// whether or not that row has been played (task D2).
    Wrapped,
}

/// What [`LoopDetector::visit`] made of one row.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Visit {
    /// A row the song has not played before. The scan records a [`RowMark`] for it.
    New,
    /// A row already played, but not one that ends the song — inside a pattern loop, or
    /// after the detector has already fired.
    Repeat,
    /// A row already played, reached by moving through the song: **the loop point**.
    ///
    /// Only a `Bxx` / `Cxx` / `Dxx` jump, a pattern loop, or the ordinary advance after one
    /// of those can produce this. The order list running out is [`Visit::Wrapped`] instead.
    Looped,
    /// The order list ran out: **the end of the song**, reported before the map is
    /// consulted at all, because a wrap ends the pass whatever row it lands on.
    Wrapped,
    /// The detector gave up: too many pattern-loop arrivals in a row.
    Budget,
}

/// One row of one pattern, and when the song first reached it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowMark {
    /// Index into the order list.
    pub order: u16,
    /// The pattern that order entry names.
    pub pattern: u16,
    /// Row within that pattern.
    pub row: u16,
    /// Song-relative frame of the row's first tick — the scan starts at [`Frame::ZERO`].
    pub frame: u64,
    /// Ticks per row as the row began, **before** the row's own `Axx` ran.
    pub speed: u8,
    /// Tempo as the row began, **before** the row's own `Txx` ran.
    pub tempo_bpm: u16,
}

/// Why the scan stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EndReason {
    /// The song reached, **through its own flow**, a row it had already played: it repeats
    /// from `target` for ever.
    ///
    /// A `Bxx` / `Cxx` / `Dxx` jump, a pattern loop, or the ordinary advance after one of
    /// those. The order list simply running out is [`EndReason::Ended`], not this.
    Looped {
        /// The row the song goes back to — the start of the repeating section.
        target: SongPosition,
    },
    /// The order list ran out: the song is over (task D2).
    ///
    /// Whatever the restart order names, and whether or not that row has been played. This
    /// covers a `Cxx` / `Dxx` break on the last order, a `Bxx` to an order at or past the
    /// end of the list, and an S3M order list reaching its `0xFF` terminator under
    /// [`EndOfSongPolicy::Loop`](crate::EndOfSongPolicy). There is no repeating section:
    /// a host that wants the song again restarts it from the top.
    Ended,
    /// The song stopped playing of its own accord: the order list ran out under
    /// [`EndOfSongPolicy::Stop`](crate::EndOfSongPolicy), or a stop marker fired.
    ///
    /// The same kind of end as [`EndReason::Ended`]; they differ only in which end-of-song
    /// policy the sequencer was built with.
    Stopped,
    /// The scan hit one of its [`ScanLimits`], or the pattern-loop budget. The length is a
    /// lower bound rather than the answer.
    Budget,
}

/// The scanned shape of one song: every row it plays, when, and how it ends.
///
/// Plain data with no behaviour beyond lookups, built once off the audio thread and then
/// read (never written) from the sequencer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SongTimeline {
    sample_rate_hz: u32,
    marks: Vec<RowMark>,
    order_marks: Vec<Option<u32>>,
    end_frame: u64,
    end: EndReason,
}

impl SongTimeline {
    /// An empty timeline for a module with nothing to play.
    pub fn empty(sample_rate_hz: u32) -> SongTimeline {
        SongTimeline { sample_rate_hz, marks: Vec::new(), order_marks: Vec::new(), end_frame: 0, end: EndReason::Stopped }
    }

    /// The output rate the frames in this timeline are counted at.
    pub const fn sample_rate_hz(&self) -> u32 { self.sample_rate_hz }

    /// Every row the song reaches, in the order it reaches them — ascending by frame.
    pub fn marks(&self) -> &[RowMark] { &self.marks }

    /// The length of one pass, in frames: the frame the song stops being new on — the
    /// first repeated row for a [`EndReason::Looped`] song, the restart row for an
    /// [`EndReason::Ended`] one, and the tick after the last for an
    /// [`EndReason::Stopped`] one.
    pub const fn end_frame(&self) -> u64 { self.end_frame }

    /// How the song ends.
    pub const fn end(&self) -> EndReason { self.end }

    /// Song-relative frame of `(order, row)`, if the song ever reaches it.
    ///
    /// A linear scan rather than a binary search — the marks are ordered by frame, not by
    /// position. It is reached from the audio thread once per detected loop point, which
    /// is a few thousand comparisons every few minutes; it allocates nothing and cannot
    /// panic, which are the properties that matter there.
    pub fn frame_at(&self, order: u16, row: u16) -> Option<u64> {
        self.mark_at(order, row).map(|mark| mark.frame)
    }

    /// The mark for `(order, row)`, if the song ever reaches it. A linear scan, for the
    /// reason [`SongTimeline::frame_at`] gives.
    pub fn mark_at(&self, order: u16, row: u16) -> Option<&RowMark> {
        self.marks.iter().find(|mark| mark.order == order && mark.row == row)
    }

    /// The row sounding at `frame` — the last mark whose frame is at or before it.
    ///
    /// Binary search: the marks are ascending by frame because they are recorded in play
    /// order.
    pub fn mark_at_frame(&self, frame: u64) -> Option<&RowMark> {
        let after = self.marks.partition_point(|mark| mark.frame <= frame);
        self.marks.get(after.checked_sub(1)?)
    }

    /// The first row the song plays of `order`, if it plays that order at all.
    pub fn order_mark(&self, order: u16) -> Option<&RowMark> {
        let index = (*self.order_marks.get(order as usize)?)?;
        self.marks.get(index as usize)
    }

    /// How long the repeating section is, for a song that repeats.
    ///
    /// `None` for a song that ends: there is nothing to repeat.
    pub fn loop_length_frames(&self) -> Option<u64> {
        match self.end {
            EndReason::Looped { target } => {
                let start = self.frame_at(target.order, target.row).unwrap_or(0);
                Some(self.end_frame.saturating_sub(start))
            }
            // The scan never found the loop point, so the best available answer is "as
            // long as we watched for".
            EndReason::Budget => Some(self.end_frame),
            EndReason::Ended | EndReason::Stopped => None,
        }
    }

    /// One pass in seconds — a host convenience for a `m:ss` display, never the RT path.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate_hz == 0 { return 0.0; }
        self.end_frame as f64 / self.sample_rate_hz as f64
    }
}

/// A bitset of the `(order, row)` pairs the song has played, and the state that says
/// which of them count.
///
/// Allocated once, when the sequencer is built. [`LoopDetector::visit`] is called from
/// `dispatch` and does bit arithmetic only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopDetector {
    /// First bit index of each order's rows.
    order_offsets: Vec<u32>,
    /// Rows mapped for each order, capped at [`MAX_ROWS_PER_ORDER`].
    order_rows: Vec<u16>,
    words: Vec<u64>,
    inside_loop: bool,
    loop_arrivals: u32,
    armed: bool,
}

impl LoopDetector {
    /// A detector sized for `data`'s order list. The only allocation this type ever does.
    pub fn new<Data: PatternData>(data: &Data) -> LoopDetector {
        let order_count = data.order_count() as usize;
        let mut order_offsets = Vec::with_capacity(order_count);
        let mut order_rows = Vec::with_capacity(order_count);
        let mut total_bits = 0u32;
        for order in 0..data.order_count() {
            let rows = match data.order(order) {
                Some(OrderEntry::Pattern(pattern)) => data.rows_in_pattern(pattern).unwrap_or(0).min(MAX_ROWS_PER_ORDER),
                _ => 0,
            };
            order_offsets.push(total_bits);
            order_rows.push(rows);
            total_bits = total_bits.saturating_add(rows as u32);
        }
        let words = vec![0u64; (total_bits as usize).div_ceil(64)];
        LoopDetector { order_offsets, order_rows, words, inside_loop: false, loop_arrivals: 0, armed: true }
    }

    /// Whether the detector is still watching. It disarms once it has fired.
    pub const fn is_armed(&self) -> bool { self.armed }

    /// Whether the last arrival put the song inside a pattern loop.
    pub const fn is_inside_loop(&self) -> bool { self.inside_loop }

    /// Consult the detector at the **first tick of a row, before the tick runs**.
    ///
    /// Once per row, not once per pattern-delay repeat: a repeated row is the same row.
    pub fn visit(&mut self, position: SongPosition, arrival: RowArrival) -> Visit {
        match arrival {
            RowArrival::PatternLoop => {
                self.inside_loop = true;
                self.loop_arrivals = self.loop_arrivals.saturating_add(1);
            }
            // Falling through to the next row neither enters nor leaves a pattern loop.
            RowArrival::Sequential => {}
            RowArrival::Start | RowArrival::NextOrder | RowArrival::Jump | RowArrival::Wrapped => {
                self.inside_loop = false;
                self.loop_arrivals = 0;
            }
        }

        // Marking still happens after the detector has fired, so that a host which keeps
        // playing sees a coherent map; it just no longer reports anything.
        let already_played = self.mark(position);
        if !self.armed {
            return Visit::Repeat;
        }
        // The order list ran out. That ends the pass whatever row the restart lands on, so
        // it is answered before the map is consulted (task D2). It is also answered before
        // the budget, which cannot fire here anyway: a `Wrapped` arrival has just cleared
        // `loop_arrivals`.
        if matches!(arrival, RowArrival::Wrapped) {
            self.armed = false;
            return Visit::Wrapped;
        }
        if self.loop_arrivals > MAX_PATTERN_LOOP_ARRIVALS {
            self.armed = false;
            return Visit::Budget;
        }
        if already_played && !self.inside_loop {
            self.armed = false;
            return Visit::Looped;
        }
        if already_played { Visit::Repeat } else { Visit::New }
    }

    /// Mark one row as played, without asking what that means.
    ///
    /// The sequencer calls this on the loop target after wrapping under
    /// [`AtEnd::Continue`](starplayer_core::AtEnd): the row is playing *now*, and if the
    /// re-marking left it clear the next pass would sail through the loop point instead of
    /// firing on it.
    pub fn mark_visited(&mut self, position: SongPosition) { self.mark(position); }

    /// Forget every row and re-arm.
    pub fn reset(&mut self) {
        for word in self.words.iter_mut() {
            *word = 0;
        }
        self.inside_loop = false;
        self.loop_arrivals = 0;
        self.armed = true;
    }

    /// Reset, then mark every row the song had already played before `frame`.
    ///
    /// This is what makes the loop point after a seek the canonical one: seeking into the
    /// middle of a song leaves the rows before the seek target marked, so the detector
    /// still fires where the scan said it would rather than wherever the seek happened to
    /// land. Bit operations over vectors that already exist — no allocation.
    pub fn reset_marking_before(&mut self, timeline: &SongTimeline, frame: u64) {
        self.reset();
        for mark in timeline.marks() {
            if mark.frame >= frame {
                break;
            }
            self.mark(SongPosition { order: mark.order, pattern: mark.pattern, row: mark.row });
        }
    }

    /// Mark one position and report whether it was already marked. A position outside the
    /// map is never marked and always reads as unplayed.
    fn mark(&mut self, position: SongPosition) -> bool {
        let Some(index) = self.bit_index(position.order, position.row) else { return false };
        let Some(word) = self.words.get_mut(index / 64) else { return false };
        let bit = 1u64 << (index % 64);
        let already_set = *word & bit != 0;
        *word |= bit;
        already_set
    }

    fn bit_index(&self, order: u16, row: u16) -> Option<usize> {
        let rows = *self.order_rows.get(order as usize)?;
        if row >= rows {
            return None;
        }
        let offset = *self.order_offsets.get(order as usize)?;
        Some(offset as usize + row as usize)
    }
}

/// Where a scan gives up on a module that will not end.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScanLimits {
    /// Frames of song to play before reporting [`EndReason::Budget`].
    pub max_frames: u64,
    /// Tracker ticks to run before reporting [`EndReason::Budget`].
    pub max_ticks: u32,
}

impl ScanLimits {
    /// One hour of audio at `sample_rate_hz`, and a million ticks — around five and a half
    /// hours at the conventional 50 Hz, so the frame limit is the one that normally bites.
    pub const fn for_rate(sample_rate_hz: u32) -> ScanLimits {
        ScanLimits { max_frames: sample_rate_hz as u64 * 3_600, max_ticks: 1_000_000 }
    }
}

impl Default for ScanLimits {
    /// One hour at 44.1 kHz. Prefer [`ScanLimits::for_rate`], which knows the real rate.
    fn default() -> ScanLimits { ScanLimits::for_rate(44_100) }
}

/// Play `sequencer` through once, with no mixing, and record where every row landed.
///
/// The sequencer must be **freshly built**: its first tick due at [`Frame::ZERO`], no
/// timeline installed, and nothing dispatched through it yet. It is left stopped at the
/// loop point and must not be played afterwards — build a second sequencer over the same
/// `Arc<Module>` for playback, so that nothing depends on
/// [`TrackerProcessor::reset`](crate::TrackerProcessor::reset) restoring every last bit of
/// processor state.
///
/// Off the audio thread: it allocates a throwaway voice pool and channel table, and the
/// timeline's vectors.
pub fn scan_timeline<Tempo, Processor, Data>(sequencer: &mut PatternSequencer<Tempo, Processor, Data>, limits: ScanLimits) -> SongTimeline
where
    Tempo: TempoModel,
    Processor: TrackerProcessor,
    Data: PatternData,
{
    let sample_rate_hz = sequencer.sample_rate_hz();
    let channel_count = (sequencer.data().channel_count() as usize).max(1);
    let order_count = sequencer.data().order_count() as usize;
    // The processor, not the channel count, says how wide the pool has to be: a format
    // whose channel can sound several voices at once — XM, IT — would otherwise scan
    // through a pool it keeps filling, and measure a song shape the player never plays.
    // MOD, S3M and MTM keep the default, which is the channel count, so their scans are
    // unchanged.
    let voice_capacity = sequencer.processor().recommended_voice_capacity(channel_count).max(1);
    let mut voices = VoicePool::new(voice_capacity);
    let mut channels = ChannelTable::new(channel_count);
    let mut control = ControlClock::new(sample_rate_hz, Frame::ZERO);

    // The scan is the one place that wants the sequencer to stop dead at the loop point:
    // that is the frame it is trying to measure.
    sequencer.set_at_end(AtEnd::Stop);

    let mut marks: Vec<RowMark> = Vec::new();
    let mut order_marks: Vec<Option<u32>> = vec![None; order_count];
    // Assigned on every path out of the loop below, which only ever leaves through a
    // `break`; declaring them uninitialised is what makes the compiler check that.
    let end_frame: u64;
    let end: EndReason;
    let mut ticks = 0u32;
    let mut last_tick_frame = 0u64;

    loop {
        let Some(frame) = sequencer.next_event_frame() else {
            // The song ended of its own accord. Its length runs to where the tick after
            // the last one would have been, which is one tick at the timing that tick left
            // in effect.
            end = EndReason::Stopped;
            end_frame = if ticks == 0 { 0 } else { last_tick_frame.saturating_add(tick_length_frames(sequencer)) };
            break;
        };
        if frame.0 >= limits.max_frames {
            end = EndReason::Budget;
            end_frame = frame.0;
            break;
        }
        if ticks >= limits.max_ticks {
            end = EndReason::Budget;
            end_frame = frame.0;
            break;
        }

        sequencer.advance_to(frame);
        let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
        sequencer.dispatch(frame, &mut context);
        ticks = ticks.saturating_add(1);
        last_tick_frame = frame.0;

        let Some(visit) = sequencer.last_row_visit() else { continue };
        match visit.visit {
            Visit::New => {
                if let Some(slot) = order_marks.get_mut(visit.mark.order as usize)
                    && slot.is_none()
                {
                    *slot = Some(marks.len() as u32);
                }
                marks.push(visit.mark);
            }
            Visit::Looped => {
                end = EndReason::Looped {
                    target: SongPosition { order: visit.mark.order, pattern: visit.mark.pattern, row: visit.mark.row },
                };
                end_frame = visit.mark.frame;
                break;
            }
            // The order list ran out. The pass is as long as it would have been had the
            // wrap been a loop point: the frame the restart row's first tick lands on.
            Visit::Wrapped => {
                end = EndReason::Ended;
                end_frame = visit.mark.frame;
                break;
            }
            Visit::Budget => {
                end = EndReason::Budget;
                end_frame = visit.mark.frame;
                break;
            }
            Visit::Repeat => {}
        }
    }

    SongTimeline { sample_rate_hz, marks, order_marks, end_frame, end }
}

/// Whole frames of one tick at the timing the sequencer currently holds.
///
/// Used only to close off a song that stopped, where there is no next tick to read the
/// boundary from. Q32.32 in, whole frames out; the dropped fraction is at most one frame
/// on a song that has already ended.
fn tick_length_frames<Tempo, Processor, Data>(sequencer: &PatternSequencer<Tempo, Processor, Data>) -> u64
where
    Tempo: TempoModel,
    Processor: TrackerProcessor,
    Data: PatternData,
{
    let length = sequencer.tempo_model().frames_per_tick(sequencer.sample_rate_hz(), sequencer.tempo_bpm(), sequencer.row_clock().speed);
    length >> 32
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use starplayer_core::{ExactFixedPoint, Step};
    use starplayer_mixer::SampleRegion;

    use crate::demo::{
        DEMO_BREAK_ROW, DEMO_ORDER_JUMP, DEMO_SET_SPEED, DEMO_SET_TEMPO, DEMO_STOP, DemoCell, DemoPatternData,
        DemoProcessor,
    };
    use crate::sequencer::{EndOfSongPolicy, SequencerSettings};

    /// 44100 Hz at 125 BPM is exactly 882 frames per tick.
    const FRAMES_PER_TICK: u64 = 882;

    type TestSequencer = PatternSequencer<ExactFixedPoint, DemoProcessor, DemoPatternData>;

    fn sequencer(data: DemoPatternData, settings: SequencerSettings) -> TestSequencer {
        PatternSequencer::new(ExactFixedPoint, data, DemoProcessor::new(SampleRegion::default(), Step::ONE), settings)
    }

    fn scan(data: DemoPatternData) -> SongTimeline {
        scan_with(data, SequencerSettings::default())
    }

    fn scan_with(data: DemoPatternData, settings: SequencerSettings) -> SongTimeline {
        let mut built = sequencer(data, settings);
        scan_timeline(&mut built, ScanLimits::for_rate(44_100))
    }

    /// The engine's side of a dispatch, without an engine — the same shape the sequencer's
    /// own tests use.
    struct Harness {
        voices: VoicePool,
        channels: ChannelTable,
        control: ControlClock,
    }

    impl Harness {
        fn new() -> Harness {
            Harness { voices: VoicePool::new(4), channels: ChannelTable::new(4), control: ControlClock::new(44_100, Frame::ZERO) }
        }

        fn tick(&mut self, sequencer: &mut TestSequencer) -> Option<Frame> {
            let frame = sequencer.next_event_frame()?;
            sequencer.advance_to(frame);
            let mut context = EngineContext::new(frame, &mut self.voices, &mut self.channels, &mut self.control);
            sequencer.dispatch(frame, &mut context);
            Some(frame)
        }

        /// Play until the sequencer reports no further boundary, and report the frame of
        /// the last dispatch. `None` for a song that never stops within the bound, so a
        /// bug cannot hang the test run and every caller's `expect` says which case it is.
        fn play_to_the_end(&mut self, sequencer: &mut TestSequencer) -> Option<Frame> {
            let mut last = None;
            for _ in 0..100_000 {
                match self.tick(sequencer) {
                    Some(frame) => last = Some(frame),
                    None => return last,
                }
            }
            None
        }
    }

    /// A playback sequencer over the same data as the scan, with the scanned timeline
    /// installed — what a host builds after scanning.
    fn playback(data: DemoPatternData, at_end: AtEnd) -> (TestSequencer, SongTimeline) {
        let timeline = scan(data.clone());
        let mut built = sequencer(data, SequencerSettings::default());
        built.set_timeline(timeline.clone());
        built.set_at_end(at_end);
        (built, timeline)
    }

    #[test]
    fn at_end_stop_stops_the_live_sequencer_on_the_scanned_end_frame() {
        let (mut built, timeline) = playback(DemoPatternData::new(2, 2, 1), AtEnd::Stop);
        let mut harness = Harness::new();

        let last = harness.play_to_the_end(&mut built).expect("the song plays");
        assert_eq!(last.0, timeline.end_frame(), "the live detector fires on exactly the scanned frame");
        assert!(built.is_stopped());
        assert!(built.end_reached());
        assert_eq!(built.song_frame(last), timeline.end_frame(), "elapsed reaches the total and stops there");
    }

    /// Task D2, the owner's case: Repeat off on a song whose order list runs out stops it
    /// on the end frame instead of playing into a second pass under a fade.
    #[test]
    fn at_end_fade_out_stops_a_song_that_runs_out_of_order_list() {
        let (mut built, timeline) = playback(DemoPatternData::new(2, 2, 1), AtEnd::FadeOut);
        assert_eq!(timeline.end(), EndReason::Ended);
        let mut harness = Harness::new();

        let last = harness.play_to_the_end(&mut built).expect("the song plays");
        assert_eq!(last.0, timeline.end_frame(), "it stops on the scanned end frame, not one pass later");
        assert!(built.is_stopped(), "there is no second pass to fade into");
        assert!(built.end_reached());
    }

    /// …and Repeat off on a song that really loops still plays on, so the host can fade it.
    #[test]
    fn at_end_fade_out_plays_on_past_a_real_loop_point() {
        let mut data = DemoPatternData::new(2, 2, 1);
        data.set(1, 1, 0, DemoCell::command(DEMO_ORDER_JUMP, 0));
        let (mut built, timeline) = playback(data, AtEnd::FadeOut);
        assert!(matches!(timeline.end(), EndReason::Looped { .. }), "the B00 makes this a loop");
        let mut harness = Harness::new();

        let mut last = Frame::ZERO;
        while last.0 < timeline.end_frame() {
            last = harness.tick(&mut built).expect("a fading song keeps playing");
        }
        assert!(built.end_reached(), "the loop point is reported");
        assert!(!built.is_stopped(), "and the song plays into the second pass for the host to fade");
        assert_eq!(harness.tick(&mut built).map(|frame| frame.0), Some(timeline.end_frame() + FRAMES_PER_TICK));
    }

    #[test]
    fn at_end_continue_rebases_the_song_clock_onto_the_loop_point() {
        let mut data = DemoPatternData::new(3, 2, 1);
        // Order 2 jumps back to order 1, so the loop point is not the top of the song.
        data.set(2, 1, 0, DemoCell::command(DEMO_ORDER_JUMP, 1));
        let (mut built, timeline) = playback(data, AtEnd::Continue);
        let mut harness = Harness::new();

        assert!(matches!(timeline.end(), EndReason::Looped { .. }), "this song loops");
        let EndReason::Looped { target } = timeline.end() else { return };
        assert_eq!(target, SongPosition { order: 1, pattern: 1, row: 0 });
        let target_frame = timeline.frame_at(target.order, target.row).expect("the target was played");

        // Play up to and including the tick the loop point lands on.
        let mut last = Frame::ZERO;
        while last.0 < timeline.end_frame() {
            last = harness.tick(&mut built).expect("a song set to continue never stops");
        }
        assert_eq!(last.0, timeline.end_frame());
        assert!(built.end_reached(), "the wrap is reported on the tick it happens");
        assert_eq!(built.song_frame(last), target_frame, "elapsed drops back to the loop point rather than to zero");

        // And it keeps looping: the same point comes round again one loop length later.
        let loop_length = timeline.loop_length_frames().expect("a looping song has a loop length");
        while last.0 < timeline.end_frame() + loop_length {
            last = harness.tick(&mut built).expect("still playing");
        }
        assert_eq!(last.0, timeline.end_frame() + loop_length);
        assert!(built.end_reached(), "the second pass ends on the same row as the first");
        assert_eq!(built.song_frame(last), target_frame);
    }

    /// D2 research point 1: a restart order that points into the middle of the song. Under
    /// `Continue` the second pass starts there with the elapsed clock rebased onto that
    /// order's scanned frame, and the *third* pass ends the same way — the wrap must be
    /// `Visit::Wrapped` again, not a spurious `Looped` off the re-marked map.
    ///
    /// The scan and the playback sequencer are given different restart orders on purpose:
    /// [`PatternSequencer::new`] starts a song at its restart order, so a song that starts
    /// at the top and restarts at order 2 cannot be spelled any other way until a MOD's
    /// Noisetracker restart byte is read (out of scope in D1).
    #[test]
    fn a_restart_order_in_the_middle_of_the_song_wraps_onto_its_own_frame_every_pass() {
        let data = DemoPatternData::new(4, 2, 1);
        let timeline = scan(data.clone());
        assert_eq!(timeline.end(), EndReason::Ended);
        let row_length = 6 * FRAMES_PER_TICK;
        assert_eq!(timeline.end_frame(), 8 * row_length);

        let settings = SequencerSettings { restart_order: 2, ..SequencerSettings::default() };
        let mut built = sequencer(data, settings);
        built.set_timeline(timeline.clone());
        built.set_at_end(AtEnd::Continue);
        assert!(built.seek_order_at(0, Frame::ZERO), "the song itself starts at the top");

        let restart_frame = timeline.frame_at(2, 0).expect("the scan reached order 2");
        assert_eq!(restart_frame, 4 * row_length);

        let mut harness = Harness::new();
        let mut last = Frame::ZERO;
        // Three ends: the order list runs out at 8 rows, then every 4 rows after it.
        for (pass, wrap_at) in [8 * row_length, 12 * row_length, 16 * row_length].into_iter().enumerate() {
            while last.0 < wrap_at {
                last = harness.tick(&mut built).expect("a song set to continue never stops");
            }
            assert_eq!(last.0, wrap_at, "pass {pass} ends where the order list runs out");
            assert!(built.end_reached(), "pass {pass} reported its end");
            assert_eq!(built.position(), SongPosition { order: 2, pattern: 2, row: 0 }, "pass {pass} restarted at the restart order");
            assert_eq!(built.song_frame(last), restart_frame, "pass {pass} rebased elapsed onto order 2's scanned frame");
        }
    }

    /// D2 research point 2: a wrap onto a row the scan never reached. It is still the end
    /// of the song, and `wrap_at_loop_point`'s `None` arm — restart the elapsed clock at
    /// zero and forget every mark — is what a row with no scanned position deserves.
    #[test]
    fn a_wrap_onto_an_unplayed_row_still_ends_the_song_and_restarts_its_clock() {
        let mut data = DemoPatternData::new(4, 2, 1);
        // Order 0 jumps straight to order 2, so order 1 is never played and never scanned.
        data.set(0, 0, 0, DemoCell::command(DEMO_ORDER_JUMP, 2));
        let timeline = scan(data.clone());
        assert_eq!(timeline.end(), EndReason::Ended);
        assert_eq!(timeline.frame_at(1, 0), None, "the scan never reaches order 1");

        let settings = SequencerSettings { restart_order: 1, ..SequencerSettings::default() };
        let mut built = sequencer(data, settings);
        built.set_timeline(timeline.clone());
        built.set_at_end(AtEnd::Continue);
        assert!(built.seek_order_at(0, Frame::ZERO), "the song itself starts at the top");

        let row_length = 6 * FRAMES_PER_TICK;
        let mut harness = Harness::new();
        let mut last = Frame::ZERO;
        while last.0 < 5 * row_length {
            last = harness.tick(&mut built).expect("a song set to continue never stops");
        }
        assert_eq!(last.0, timeline.end_frame(), "one row of order 0, then orders 2 and 3");
        assert!(built.end_reached());
        assert!(!built.is_stopped());
        assert_eq!(built.position(), SongPosition { order: 1, pattern: 1, row: 0 }, "the wrap lands on the unplayed restart order");
        assert_eq!(built.song_frame(last), 0, "a row with no scanned position starts the elapsed clock afresh");

        // The detector was reset rather than re-marked, so the next wrap fires too.
        while last.0 < 5 * row_length + 6 * row_length {
            last = harness.tick(&mut built).expect("still playing");
        }
        assert!(built.end_reached(), "orders 1, 2 and 3, then the order list runs out again");
        assert_eq!(built.song_frame(last), 0);
    }

    #[test]
    fn seek_frame_restores_the_scanned_row_timing_and_keeps_the_loop_point_canonical() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(0, 2, 0, DemoCell::command(DEMO_SET_SPEED, 3));
        data.set(0, 3, 0, DemoCell::command(DEMO_SET_TEMPO, 250));
        let (mut built, timeline) = playback(data, AtEnd::Stop);

        let mark = *timeline.marks().get(5).expect("the song has more than six rows");
        assert_eq!((mark.order, mark.row), (1, 1));
        assert_ne!((mark.speed, mark.tempo_bpm), (6, 125), "the seek target is reached at changed timing");

        let now = Frame(4_000_000);
        assert_eq!(built.seek_frame(mark.frame, now), Some(mark));
        built.restart_clock_at(now);

        assert_eq!(built.position(), SongPosition { order: mark.order, pattern: mark.pattern, row: mark.row });
        assert_eq!(built.row_clock().speed, mark.speed, "the row starts at the speed the scan saw");
        assert_eq!(built.tempo_bpm(), mark.tempo_bpm);
        assert_eq!(built.song_frame(now), mark.frame, "elapsed is continuous across the seek");
        assert!(!built.end_reached());

        // Playing on from there must stop at the same place a full play-through would.
        let mut harness = Harness::new();
        let last = harness.play_to_the_end(&mut built).expect("the song plays on");
        assert_eq!(built.song_frame(last), timeline.end_frame(), "the loop point after a seek is the canonical one");
        assert_eq!(last.0, now.0 + (timeline.end_frame() - mark.frame));
    }

    #[test]
    fn seeking_to_a_frame_needs_a_timeline() {
        let mut built = sequencer(DemoPatternData::new(2, 4, 1), SequencerSettings::default());
        assert_eq!(built.seek_frame(0, Frame(1_000)), None, "there is nothing to resolve a frame against");
    }

    #[test]
    fn seeking_to_an_order_the_scan_never_reached_starts_its_clock_at_now() {
        let mut data = DemoPatternData::new(3, 2, 1);
        // Order 0 jumps straight past order 1, so order 1 is never played.
        data.set(0, 1, 0, DemoCell::command(DEMO_ORDER_JUMP, 2));
        let (mut built, timeline) = playback(data, AtEnd::Stop);
        assert_eq!(timeline.frame_at(1, 0), None, "the scan never reaches order 1");

        let now = Frame(2_000_000);
        assert!(built.seek_order_at(1, now));
        assert_eq!(built.song_frame(now), 0, "a hidden order has no elapsed position of its own");
    }

    #[test]
    fn a_song_that_ends_restarts_under_continue_and_stops_without_a_timeline() {
        let mut data = DemoPatternData::new(2, 2, 1);
        data.set(1, 1, 0, DemoCell::command(DEMO_STOP, 0));
        let timeline = scan(data.clone());
        assert_eq!(timeline.end(), EndReason::Stopped);

        // No timeline: the end of the song means what it always has.
        let mut bare = sequencer(data.clone(), SequencerSettings::default());
        let mut harness = Harness::new();
        harness.play_to_the_end(&mut bare).expect("it plays");
        assert!(bare.is_stopped(), "at_end is inert until a host installs a timeline");

        // With one, `Continue` restarts from the top and the elapsed clock goes with it.
        let mut built = sequencer(data, SequencerSettings::default());
        built.set_timeline(timeline.clone());
        built.set_at_end(AtEnd::Continue);
        let mut last = Frame::ZERO;
        while last.0 < timeline.end_frame() {
            last = harness.tick(&mut built).expect("a restarted song never stops");
        }
        assert!(!built.is_stopped());
        assert_eq!(built.position(), SongPosition { order: 0, pattern: 0, row: 0 }, "back at the top");
        assert_eq!(built.song_frame(last), 0, "and the elapsed clock restarted with it");
    }

    #[test]
    fn a_pattern_delay_consults_the_detector_once_for_the_whole_row() {
        use crate::demo::DEMO_PATTERN_DELAY;
        let mut data = DemoPatternData::new(1, 2, 1);
        data.set(0, 0, 0, DemoCell::command(DEMO_PATTERN_DELAY, 2));
        let timeline = scan(data.clone());

        let mut built = sequencer(data, SequencerSettings::default());
        built.set_timeline(timeline.clone());
        built.set_at_end(AtEnd::Stop);
        let mut harness = Harness::new();
        let mut visits = 0;
        for _ in 0..40 {
            if harness.tick(&mut built).is_none() {
                break;
            }
            if built.last_row_visit().is_some() {
                visits += 1;
            }
        }
        assert_eq!(visits, 3, "row 0 with SE2, row 1, then row 0 again as the loop point");
        assert_eq!(timeline.marks().len(), 2, "the three repeats of row 0 are one row");
        assert_eq!(timeline.end_frame(), (18 + 6) * FRAMES_PER_TICK);
    }

    /// Task D2: a song whose order list simply runs out *ends* there. Nothing in it asked
    /// to be heard again, so it is not a loop however the end-of-song policy restarts it.
    #[test]
    fn a_straight_order_list_ends_when_it_runs_out() {
        let timeline = scan(DemoPatternData::new(3, 4, 1));
        assert_eq!(timeline.end(), EndReason::Ended);
        assert_eq!(timeline.end_frame(), 3 * 4 * 6 * FRAMES_PER_TICK, "three patterns of four rows at speed 6");
        assert_eq!(timeline.marks().len(), 12);
        assert_eq!(timeline.loop_length_frames(), None, "a song that ends has no repeating section");
        assert_eq!(timeline.frame_at(1, 0), Some(4 * 6 * FRAMES_PER_TICK));
        assert_eq!(timeline.order_mark(2).map(|mark| mark.frame), Some(8 * 6 * FRAMES_PER_TICK));
        assert_eq!(timeline.sample_rate_hz(), 44_100);
    }

    /// …and the same order list with a `B00` on its last row *does* loop, at exactly the
    /// same frame: the difference between the two ends is the module asking for it.
    #[test]
    fn a_position_jump_on_the_last_row_loops_where_a_run_out_would_have_ended() {
        let run_out = scan(DemoPatternData::new(3, 4, 1));
        let mut data = DemoPatternData::new(3, 4, 1);
        data.set(2, 3, 0, DemoCell::command(DEMO_ORDER_JUMP, 0));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Looped { target: SongPosition { order: 0, pattern: 0, row: 0 } });
        assert_eq!(timeline.end_frame(), run_out.end_frame(), "the same frame, read two ways");
        assert_eq!(timeline.loop_length_frames(), Some(timeline.end_frame()), "the whole song is the loop");
    }

    /// A `Cxx`/`Dxx` break on the last order has nowhere to break to: the order list runs
    /// out and the song ends.
    #[test]
    fn a_pattern_break_on_the_last_order_ends_the_song() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(1, 1, 0, DemoCell::command(DEMO_BREAK_ROW, 2));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Ended);
        assert_eq!(timeline.end_frame(), 6 * 6 * FRAMES_PER_TICK, "four rows of order 0, two of order 1");
        assert_eq!(timeline.loop_length_frames(), None);
    }

    /// A `Bxx` past the end of the order list is a restart, not a jump — ProTracker's
    /// `mt_PosJump` clamps one the same way — so it ends the song rather than looping it.
    #[test]
    fn a_position_jump_past_the_end_of_the_order_list_ends_the_song() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(0, 1, 0, DemoCell::command(DEMO_ORDER_JUMP, 9));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Ended);
        assert_eq!(timeline.end_frame(), 2 * 6 * FRAMES_PER_TICK, "two rows, then the jump runs off the list");
        assert_eq!(timeline.marks().len(), 2);
    }

    /// An S3M order list reaching its `0xFF` terminator under the wrapping policy every
    /// format builder uses: the same end.
    #[test]
    fn an_end_marker_ends_the_song_even_under_the_wrapping_policy() {
        let data = DemoPatternData::new(2, 4, 1).with_orders(vec![OrderEntry::Pattern(0), OrderEntry::End, OrderEntry::Pattern(1)]);
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Ended);
        assert_eq!(timeline.end_frame(), 4 * 6 * FRAMES_PER_TICK, "the one reachable pattern, once");
        assert_eq!(timeline.loop_length_frames(), None);
    }

    #[test]
    fn every_mark_carries_the_speed_and_tempo_the_row_began_with() {
        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 1, 0, DemoCell::command(DEMO_SET_SPEED, 3));
        data.set(0, 2, 0, DemoCell::command(DEMO_SET_TEMPO, 250));
        let timeline = scan(data);

        let speeds: Vec<(u8, u16)> = timeline.marks().iter().map(|mark| (mark.speed, mark.tempo_bpm)).collect();
        assert_eq!(speeds, vec![(6, 125), (6, 125), (3, 125), (3, 250)], "a mark records the timing before its own row ran");
    }

    #[test]
    fn a_backward_jump_makes_its_target_the_loop_point() {
        let mut data = DemoPatternData::new(3, 4, 1);
        // Order 2 jumps back to order 1: rows of order 0 play once, orders 1 and 2 repeat.
        data.set(2, 3, 0, DemoCell::command(DEMO_ORDER_JUMP, 1));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Looped { target: SongPosition { order: 1, pattern: 1, row: 0 } });
        assert_eq!(timeline.end_frame(), 12 * 6 * FRAMES_PER_TICK);
        assert_eq!(timeline.loop_length_frames(), Some(8 * 6 * FRAMES_PER_TICK), "the intro is not part of the loop");
    }

    #[test]
    fn a_jump_to_its_own_order_ends_the_song_there() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(1, 0, 0, DemoCell::command(DEMO_ORDER_JUMP, 1));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Looped { target: SongPosition { order: 1, pattern: 1, row: 0 } });
        assert_eq!(timeline.end_frame(), 5 * 6 * FRAMES_PER_TICK, "four rows of order 0, then row 0 of order 1 once");
        assert_eq!(timeline.loop_length_frames(), Some(6 * FRAMES_PER_TICK));
    }

    #[test]
    fn a_pattern_loop_body_is_counted_every_time_without_ending_the_song() {
        // The demo format has no SBx, so drive the detector directly with the arrivals a
        // three-times E6x loop produces: rows 0..3, then rows 1..3 three more times.
        let data = DemoPatternData::new(1, 4, 1);
        let mut detector = LoopDetector::new(&data);
        let at = |row| SongPosition { order: 0, pattern: 0, row };

        assert_eq!(detector.visit(at(0), RowArrival::Start), Visit::New);
        for row in 1..4 {
            assert_eq!(detector.visit(at(row), RowArrival::Sequential), Visit::New);
        }
        for repeat in 0..3 {
            assert_eq!(detector.visit(at(1), RowArrival::PatternLoop), Visit::Repeat, "repeat {repeat} is inside the loop");
            assert!(detector.is_inside_loop());
            for row in 2..4 {
                assert_eq!(detector.visit(at(row), RowArrival::Sequential), Visit::Repeat, "and so is the rest of its body");
            }
        }
        assert!(detector.is_armed(), "an E6x body never ends a song");

        // Leaving the loop and moving on is checked normally again.
        assert_eq!(detector.visit(SongPosition { order: 1, pattern: 1, row: 0 }, RowArrival::NextOrder), Visit::New);
        assert!(!detector.is_inside_loop());
        assert_eq!(detector.visit(at(0), RowArrival::Wrapped), Visit::Wrapped, "the order list running out is the end of the song");
        assert!(!detector.is_armed());
        assert_eq!(detector.visit(at(1), RowArrival::Sequential), Visit::Repeat, "and it only fires once");
    }

    #[test]
    fn an_endless_pattern_loop_runs_out_of_budget() {
        let data = DemoPatternData::new(1, 4, 1);
        let mut detector = LoopDetector::new(&data);
        let at = |row| SongPosition { order: 0, pattern: 0, row };
        assert_eq!(detector.visit(at(0), RowArrival::Start), Visit::New);

        let mut visits = 0u32;
        loop {
            let visit = detector.visit(at(1), RowArrival::PatternLoop);
            visits += 1;
            if visit == Visit::Budget {
                break;
            }
            assert!(visits < MAX_PATTERN_LOOP_ARRIVALS + 8, "the budget must bite");
        }
        assert_eq!(visits, MAX_PATTERN_LOOP_ARRIVALS + 1);
        assert!(!detector.is_armed());
    }

    /// D2 research point 3: a wrap can never be reported as a budget end. A `Wrapped`
    /// arrival clears the pattern-loop counter on its way in, so the two conditions cannot
    /// both hold on the same visit however close the budget was to biting.
    #[test]
    fn a_wrap_on_the_brink_of_the_pattern_loop_budget_is_still_a_wrap() {
        let data = DemoPatternData::new(2, 4, 1);
        let mut detector = LoopDetector::new(&data);
        let at = |row| SongPosition { order: 0, pattern: 0, row };

        assert_eq!(detector.visit(at(0), RowArrival::Start), Visit::New);
        for arrival in 0..MAX_PATTERN_LOOP_ARRIVALS {
            assert_ne!(detector.visit(at(1), RowArrival::PatternLoop), Visit::Budget, "arrival {arrival} is still inside the budget");
        }
        assert!(detector.is_armed(), "the budget has not bitten yet");
        assert_eq!(detector.visit(at(0), RowArrival::Wrapped), Visit::Wrapped, "and the wrap answers first");
        assert!(!detector.is_armed());
    }

    #[test]
    fn a_stop_command_ends_the_song_rather_than_looping_it() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(0, 2, 0, DemoCell::command(DEMO_STOP, 0));
        let timeline = scan(data);

        assert_eq!(timeline.end(), EndReason::Stopped);
        assert_eq!(timeline.loop_length_frames(), None, "a song that ends has nothing to repeat");
        // Two whole rows, then the first tick of the stopping row, then one tick's worth
        // to close the row off.
        assert_eq!(timeline.end_frame(), (2 * 6 + 1) * FRAMES_PER_TICK);
        assert_eq!(timeline.marks().len(), 3);
    }

    #[test]
    fn a_module_with_no_playable_order_stops_at_frame_zero() {
        let data = DemoPatternData::new(0, 0, 1).with_orders(Vec::new());
        let timeline = scan(data);
        assert_eq!(timeline.end(), EndReason::Stopped);
        assert_eq!(timeline.end_frame(), 0);
        assert!(timeline.marks().is_empty());
        assert_eq!(timeline.mark_at_frame(0), None);
        assert_eq!(timeline.duration_seconds(), 0.0);
        assert_eq!(timeline, SongTimeline::empty(44_100), "which is exactly what an empty timeline is");
    }

    #[test]
    fn an_order_list_that_runs_out_under_the_stop_policy_ends_the_song() {
        let settings = SequencerSettings { end_of_song: EndOfSongPolicy::Stop, ..SequencerSettings::default() };
        let timeline = scan_with(DemoPatternData::new(2, 2, 1), settings);
        assert_eq!(timeline.end(), EndReason::Stopped);
        assert_eq!(timeline.end_frame(), 4 * 6 * FRAMES_PER_TICK);
    }

    #[test]
    fn the_frame_limit_reports_a_budget_end() {
        let limits = ScanLimits { max_frames: 10 * FRAMES_PER_TICK, max_ticks: 1_000_000 };
        let mut built = sequencer(DemoPatternData::new(8, 64, 1), SequencerSettings::default());
        let timeline = scan_timeline(&mut built, limits);
        assert_eq!(timeline.end(), EndReason::Budget);
        assert_eq!(timeline.loop_length_frames(), Some(timeline.end_frame()));
    }

    #[test]
    fn the_tick_limit_reports_a_budget_end() {
        let limits = ScanLimits { max_frames: u64::MAX, max_ticks: 10 };
        let mut built = sequencer(DemoPatternData::new(8, 64, 1), SequencerSettings::default());
        let timeline = scan_timeline(&mut built, limits);
        assert_eq!(timeline.end(), EndReason::Budget);
        assert_eq!(timeline.end_frame(), 10 * FRAMES_PER_TICK);
    }

    #[test]
    fn a_frame_lookup_finds_the_row_that_is_sounding() {
        let timeline = scan(DemoPatternData::new(2, 4, 1));
        let row_length = 6 * FRAMES_PER_TICK;
        assert_eq!(timeline.mark_at_frame(0).map(|mark| mark.row), Some(0));
        assert_eq!(timeline.mark_at_frame(row_length - 1).map(|mark| mark.row), Some(0));
        assert_eq!(timeline.mark_at_frame(row_length).map(|mark| mark.row), Some(1));
        assert_eq!(timeline.mark_at_frame(4 * row_length).map(|mark| (mark.order, mark.row)), Some((1, 0)));
        assert_eq!(timeline.mark_at_frame(u64::MAX).map(|mark| (mark.order, mark.row)), Some((1, 3)), "past the end is the last row");
    }

    #[test]
    fn resetting_before_a_frame_marks_only_what_came_first() {
        let mut data = DemoPatternData::new(2, 4, 1);
        data.set(1, 3, 0, DemoCell::command(DEMO_BREAK_ROW, 0));
        let timeline = scan(data.clone());
        let mut detector = LoopDetector::new(&data);
        let row_length = 6 * FRAMES_PER_TICK;

        detector.reset_marking_before(&timeline, 2 * row_length);
        assert_eq!(detector.visit(SongPosition { order: 0, pattern: 0, row: 0 }, RowArrival::Start), Visit::Looped, "row 0 was marked");
        detector.reset_marking_before(&timeline, 2 * row_length);
        assert_eq!(detector.visit(SongPosition { order: 0, pattern: 0, row: 2 }, RowArrival::Start), Visit::New, "row 2 was not");
    }

    #[test]
    fn a_row_outside_the_map_never_ends_the_song() {
        let data = DemoPatternData::new(1, 4, 1);
        let mut detector = LoopDetector::new(&data);
        let outside = SongPosition { order: 9, pattern: 9, row: 0 };
        assert_eq!(detector.visit(outside, RowArrival::Start), Visit::New);
        assert_eq!(detector.visit(outside, RowArrival::NextOrder), Visit::New, "an unmapped position is always unvisited");
        assert!(detector.is_armed());
    }
}

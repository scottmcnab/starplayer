//! Exercise real allocator failures during control-side playback preparation.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::TryReserveError;

use starplayer_core::{ExactFixedPoint, Step};
use starplayer_engine::demo::{DemoPatternData, DemoProcessor};
use starplayer_engine::{
    ChannelTable, LoopDetector, PatternSequencer, RowMark, ScanLimits, SequencerSettings, SilentSource,
    TimelineBufferError, scan_timeline, try_box_source, try_scan_timeline, try_scan_timeline_in,
};
use starplayer_mixer::{SampleRegion, VoicePool};

thread_local! {
    static REMAINING: Cell<Option<usize>> = const { Cell::new(None) };
}

struct FailingAllocator;
fn refuse() -> bool {
    REMAINING.try_with(|remaining| match remaining.get() {
        None => false,
        Some(0) => true,
        Some(count) => { remaining.set(Some(count - 1)); false }
    }).unwrap_or(false)
}

// SAFETY: successful operations are forwarded unchanged to System. A refusal returns
// null as GlobalAlloc permits; the destructor-free thread-local hook never allocates.
unsafe impl GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse() { core::ptr::null_mut() } else { unsafe { System.alloc(layout) } }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) { unsafe { System.dealloc(pointer, layout) } }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refuse() { core::ptr::null_mut() } else { unsafe { System.realloc(pointer, layout, size) } }
    }
}
#[global_allocator]
static ALLOCATOR: FailingAllocator = FailingAllocator;

fn failing_after<T>(count: usize, operation: impl FnOnce() -> Result<T, TryReserveError>) -> Result<T, TryReserveError> {
    REMAINING.with(|remaining| remaining.set(Some(count)));
    let result = operation();
    REMAINING.with(|remaining| remaining.set(None));
    result
}

type Sequencer = PatternSequencer<ExactFixedPoint, DemoProcessor, DemoPatternData>;
fn sequencer() -> Sequencer {
    PatternSequencer::new(ExactFixedPoint, DemoPatternData::new(3, 64, 4), DemoProcessor::new(SampleRegion::default(), Step::ONE), SequencerSettings::default())
}

#[test]
fn state_and_dynamic_source_allocation_fail_cleanly() {
    assert!(failing_after(0, || ChannelTable::try_new(4)).is_err());
    assert!(failing_after(0, || VoicePool::try_new(4)).is_err());
    // Use a non-zero-sized source: a ZST correctly needs no allocation.
    let source = sequencer();
    assert!(failing_after(0, || try_box_source(source)).is_err());
}

#[test]
fn detector_handles_failure_at_each_of_its_three_allocations() {
    let data = DemoPatternData::new(3, 64, 4);
    for allocation in 0..3 {
        assert!(failing_after(allocation, || LoopDetector::try_new(&data)).is_err());
    }
    assert!(failing_after(3, || LoopDetector::try_new(&data)).is_ok());
}

#[test]
fn fallible_scan_matches_normal_timing_and_survives_every_allocation_failure() {
    let expected = scan_timeline(&mut sequencer(), ScanLimits::for_rate(44_100));
    let mut completed = false;
    for allocation in 0..32 {
        let mut source = sequencer();
        match failing_after(allocation, || try_scan_timeline(&mut source, ScanLimits::for_rate(44_100))) {
            Err(_) => {}
            Ok(actual) => { assert_eq!(actual, expected); completed = true; break; }
        }
    }
    assert!(completed, "all real allocation sites must be exercised before success");
    for allocation in 0..2 {
        assert!(failing_after(allocation, || expected.try_clone()).is_err());
    }
    assert_eq!(failing_after(2, || expected.try_clone()).unwrap(), expected);
}

#[test]
fn borrowed_scan_matches_owned_storage_and_clones_shallowly_without_allocating() {
    let expected = scan_timeline(&mut sequencer(), ScanLimits::for_rate(44_100));
    let marks = Box::leak(vec![RowMark::default(); expected.marks().len()].into_boxed_slice());
    let order_marks = Box::leak(vec![None; 3].into_boxed_slice());
    let borrowed = try_scan_timeline_in(&mut sequencer(), ScanLimits::for_rate(44_100), marks, order_marks).unwrap();

    assert_eq!(borrowed, expected, "borrowed and owned tables expose identical timelines");
    let borrowed_marks = borrowed.marks().as_ptr();
    let cloned = failing_after(0, || Ok::<_, TryReserveError>(borrowed.clone())).unwrap();
    assert_eq!(cloned, expected);
    assert_eq!(cloned.marks().as_ptr(), borrowed_marks, "a borrowed clone keeps the same row table");
    let fallible_clone = failing_after(0, || borrowed.try_clone()).unwrap();
    assert_eq!(fallible_clone.marks().as_ptr(), borrowed_marks, "try_clone is shallow too");
}

#[test]
fn borrowed_scan_reports_the_exact_missing_capacity_for_each_table() {
    let expected = scan_timeline(&mut sequencer(), ScanLimits::for_rate(44_100));
    let required_marks = expected.marks().len();

    let marks = Box::leak(vec![RowMark::default(); required_marks].into_boxed_slice());
    let short_order_marks = Box::leak(vec![None; 2].into_boxed_slice());
    match try_scan_timeline_in(&mut sequencer(), ScanLimits::for_rate(44_100), marks, short_order_marks) {
        Err(TimelineBufferError::TooSmall { required, available }) => assert_eq!((required, available), (3, 2)),
        other => panic!("expected an order-table capacity error, got {other:?}"),
    }

    let short_marks = Box::leak(vec![RowMark::default(); required_marks - 1].into_boxed_slice());
    let order_marks = Box::leak(vec![None; 3].into_boxed_slice());
    match try_scan_timeline_in(&mut sequencer(), ScanLimits::for_rate(44_100), short_marks, order_marks) {
        Err(TimelineBufferError::TooSmall { required, available }) => {
            assert_eq!((required, available), (required_marks, required_marks - 1));
        }
        other => panic!("expected a row-table capacity error, got {other:?}"),
    }
}

#[test]
fn a_boxed_zero_sized_source_remains_usable() {
    let source = failing_after(0, || try_box_source(SilentSource)).unwrap();
    assert_eq!(source.next_event_frame(), None);
}

#[test]
fn summary_scan_matches_timeline_with_only_voice_and_channel_allocations() {
    let expected = scan_timeline(&mut sequencer(), ScanLimits::for_rate(44_100));
    let mut source = sequencer();
    let actual = failing_after(2, || starplayer_engine::try_scan_timeline_end(&mut source, ScanLimits::for_rate(44_100))).unwrap();
    assert_eq!(actual, (expected.end(), expected.end_frame()));
}

#[cfg(feature = "telemetry")]
#[test]
fn a_host_can_omit_scopes_while_retaining_scalar_telemetry() {
    use starplayer_engine::{Engine, EngineSettings};
    use starplayer_dsp::Linear;
    use starplayer_mixer::{FixedPath, StereoI16};
    let settings = EngineSettings { channel_count: 8, voice_capacity: 8, scope_taps: false, telemetry_depth: 1, ..EngineSettings::default() };
    let mut engine = Engine::<FixedPath, Linear, StereoI16>::with_settings(settings);
    assert!(engine.scope_readers().unwrap().is_empty());
    assert!(engine.telemetry_reader().is_some());
    let mut output = [1i16; 256];
    engine.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0));
}

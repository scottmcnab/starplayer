//! Resource failures must remain recoverable before a replacement reaches the renderer.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use starplayer::core::Error;
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{RowMark, ScanLimits};
use starplayer::rt::Arc;

thread_local! { static REMAINING: Cell<Option<usize>> = const { Cell::new(None) }; }
struct FailingAllocator;
fn refuse() -> bool {
    REMAINING.with(|remaining| match remaining.get() {
        None => false,
        Some(0) => true,
        Some(count) => { remaining.set(Some(count - 1)); false }
    })
}
unsafe impl GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse() { std::ptr::null_mut() } else { unsafe { System.alloc(layout) } }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) { unsafe { System.dealloc(pointer, layout) } }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refuse() { std::ptr::null_mut() } else { unsafe { System.realloc(pointer, layout, size) } }
    }
}
#[global_allocator]
static ALLOCATOR: FailingAllocator = FailingAllocator;
fn failing_after<T>(count: usize, operation: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    REMAINING.with(|remaining| remaining.set(Some(count)));
    let result = operation();
    REMAINING.with(|remaining| remaining.set(None));
    result
}
fn fixtures() -> Vec<&'static [u8]> {
    vec![
        #[cfg(feature = "s3m")]
        include_bytes!("../../../fuzz/seeds/s3m/minimal.s3m"),
        #[cfg(feature = "mod")]
        include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod"),
        #[cfg(feature = "mtm")]
        include_bytes!("../../../fuzz/seeds/mtm/synthetic-golden.mtm"),
        #[cfg(feature = "xm")]
        include_bytes!("../../../fuzz/seeds/xm/pingpong-16bit.xm"),
        #[cfg(feature = "it")]
        include_bytes!("../../../fuzz/seeds/it/compressed-16bit.it"),
    ]
}

#[test]
fn fallible_and_caller_storage_scans_match_the_legacy_scan() {
    for bytes in fixtures() {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        let limits = ScanLimits::for_rate(8000);
        let expected = starplayer::scan_song(&module, 8000, limits).unwrap();
        let actual = starplayer::try_scan_song(&module, 8000, limits).unwrap();
        assert_eq!(actual, expected);
        let marks = Box::leak(vec![RowMark::default(); expected.timeline.marks().len()].into_boxed_slice());
        let order_marks = Box::leak(vec![None; module.orders().len()].into_boxed_slice());
        let borrowed = starplayer::try_scan_song_in(&module, 8000, limits, marks, order_marks).unwrap();
        assert_eq!(borrowed, expected);
    }
}

#[test]
fn every_preparation_allocation_failure_returns_without_leaking_module_owners() {
    for bytes in fixtures() {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        let mut succeeded = false;
        for allocation in 0..32 {
            let result = failing_after(allocation, || starplayer::NativeSequencer::try_new(Arc::clone(&module), 8000, QuirkSelection::FromDialect));
            succeeded = result.is_ok();
            if let Err(error) = &result { assert!(matches!(error, Error::Resource(_)), "{error:?}"); }
            drop(result);
            assert_eq!(Arc::strong_count(&module), 1);
            if succeeded { break; }
        }
        assert!(succeeded, "exercise all constructor allocations before success");
        succeeded = false;
        for allocation in 0..64 {
            let result = failing_after(allocation, || starplayer::try_scan_song(&module, 8000, ScanLimits::for_rate(8000)));
            succeeded = result.is_ok();
            if let Err(error) = &result { assert!(matches!(error, Error::Resource(_)), "{error:?}"); }
            drop(result);
            assert_eq!(Arc::strong_count(&module), 1);
            if succeeded { break; }
        }
        assert!(succeeded, "exercise all scan allocations before success");
    }
}

#[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn prepared_capacity_and_every_optional_allocation_are_checked_before_playback() {
    for bytes in fixtures() {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        let mut succeeded = false;
        for allocation in 0..32 {
            let result = failing_after(allocation, || starplayer::NativeSequencer::new_with_articulation(Arc::clone(&module), 8000, QuirkSelection::FromDialect, 339));
            if let Err(error) = &result { assert!(matches!(error, Error::Resource(_)), "{error:?}"); }
            if let Ok(sequencer) = &result {
                match sequencer {
                    #[cfg(feature = "xm")]
                    starplayer::NativeSequencer::Xm(source) => assert_eq!(source.processor().articulation_capacity(), 339),
                    #[cfg(feature = "it")]
                    starplayer::NativeSequencer::It(source) => assert_eq!(source.processor().articulation_capacity(), 339),
                    _ => {}
                }
                succeeded = true;
            }
            drop(result);
            assert_eq!(Arc::strong_count(&module), 1);
            if succeeded { break; }
        }
        assert!(succeeded, "every optional preparation allocation is recoverable");
        let legacy = starplayer::NativeSequencer::new(Arc::clone(&module), 8000, QuirkSelection::FromDialect).unwrap();
        match legacy {
            #[cfg(feature = "xm")]
            starplayer::NativeSequencer::Xm(source) => assert_eq!(source.processor().articulation_capacity(), 0),
            #[cfg(feature = "it")]
            starplayer::NativeSequencer::It(source) => assert_eq!(source.processor().articulation_capacity(), 272),
            _ => {}
        }
    }
}

#[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn prepared_articulation_preserves_canonical_pcm_at_every_host_block_size() {
    use starplayer::dsp::Linear;
    use starplayer::engine::{Engine, EngineSettings};
    use starplayer::mixer::{FixedPath, StereoI16};
    fn render(module: &Arc<starplayer::model::Module>, prepared: bool, block: usize) -> Vec<i16> {
        let settings = if prepared {
            EngineSettings { sample_rate_hz: 8000, ..EngineSettings::interactive_web(Default::default(), module.instruments().len()).unwrap() }
        } else { EngineSettings { sample_rate_hz: 8000, channel_count: 64, voice_capacity: 272, ..EngineSettings::default() } };
        let mut engine: Engine<FixedPath, Linear, StereoI16, Arc<starplayer::model::Module>> = Engine::with_settings(settings);
        let mut control = engine.take_control().unwrap();
        control.load_module(Arc::clone(module)).ok().unwrap();
        let source = if prepared {
            starplayer::NativeSequencer::new_with_articulation(Arc::clone(module), 8000, QuirkSelection::FromDialect, 339).unwrap()
        } else { starplayer::NativeSequencer::new(Arc::clone(module), 8000, QuirkSelection::FromDialect).unwrap() };
        engine.set_source(Box::new(source));
        if prepared {
            control.send(starplayer::core::Command::Interactive(starplayer::core::InteractiveCommand { identity: Default::default(), action: starplayer::core::InteractiveAction::Play })).ok().unwrap();
        }
        let mut output = vec![0; 32_000];
        for chunk in output.chunks_mut(block * 2) { engine.render(chunk); }
        output
    }
    for bytes in fixtures() {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        let canonical = render(&module, false, 128);
        for block in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(render(&module, true, block), canonical, "prepared {:?} at block {block}", module.header().format);
        }
    }
}

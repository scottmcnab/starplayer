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

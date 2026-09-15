use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use starplayer_core::Error;
use starplayer_model::{DecodeBudget, ImageDecodeError, ImageDecodeStatus};
use starplayer_xm::ImageDecoder;

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

// SAFETY: successful operations are forwarded unchanged to System. Refused allocation
// returns null as GlobalAlloc requires, and the thread-local hook itself does not allocate.
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

fn failing_after<T>(count: usize, operation: impl FnOnce() -> T) -> T {
    REMAINING.with(|remaining| remaining.set(Some(count)));
    let result = operation();
    REMAINING.with(|remaining| remaining.set(None));
    result
}

fn complete(decoder: &mut ImageDecoder<'_>) -> Result<usize, ImageDecodeError> {
    loop {
        match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 1024 })? {
            ImageDecodeStatus::Pending => {}
            ImageDecodeStatus::Complete { image_length } => return Ok(image_length),
        }
    }
}

#[test]
fn every_metadata_allocation_is_fallible() {
    let source = include_bytes!("../../../fuzz/seeds/xm/pingpong-16bit.xm");
    let expected_length = starplayer_xm::load(source).expect("seed loads").to_image().len();
    let mut completed = false;
    for allocation in 0..128 {
        let mut destination = vec![0u8; expected_length];
        let mut workspace = [0u8; 4096];
        let mut decoder = ImageDecoder::new(source, &mut destination, &mut workspace);
        match failing_after(allocation, || complete(&mut decoder)) {
            Err(ImageDecodeError::Module(Error::Resource(_))) => {}
            Ok(length) => {
                assert_eq!(length, expected_length);
                completed = true;
                break;
            }
            other => panic!("allocation {allocation} returned {other:?}"),
        }
    }
    assert!(completed, "the test passed every real metadata allocation before its bound");
}

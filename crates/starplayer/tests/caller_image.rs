//! Caller-buffer conversion must never allocate a sample-sized internal temporary.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use starplayer::{DecodeBudget, ImageDecodeStatus, ModuleImageDecoder};
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}
struct MeasuringAllocator;
fn record(size: usize) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        LARGEST.with(|largest| largest.set(largest.get().max(size)));
    }
}
unsafe impl GlobalAlloc for MeasuringAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 { record(layout.size()); unsafe { System.alloc(layout) } }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) { unsafe { System.dealloc(pointer, layout) } }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 { record(size); unsafe { System.realloc(pointer, layout, size) } }
}
#[global_allocator]
static ALLOCATOR: MeasuringAllocator = MeasuringAllocator;

fn compare(source: &[u8], pcm_budget: usize) -> usize {
    let module = starplayer::load(source).unwrap();
    let expected = module.to_image();
    let pcm_start = expected.len() - module.image_pcm_bytes();
    let mut output = vec![0x55u8; expected.len()];
    let mut workspace = vec![0u8; 256 * 1024];
    LARGEST.with(|largest| largest.set(0));
    TRACK.with(|track| track.set(true));
    let mut decoder = ModuleImageDecoder::new(source, &mut output, &mut workspace).unwrap();
    let mut steps = 0;
    loop {
        let progress = decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: pcm_budget }).unwrap();
        steps += 1;
        if let ImageDecodeStatus::Complete { image_length } = progress {
            assert_eq!(image_length, expected.len());
            break;
        }
        assert!(steps < 2_000_000, "decoder must make bounded progress");
    }
    drop(decoder);
    TRACK.with(|track| track.set(false));
    assert_eq!(output, expected);
    // With one frame per step, writing the PCM requires at least its full frame count.
    if pcm_budget == 1 { assert!(steps >= (expected.len() - pcm_start) / 2); }
    LARGEST.with(Cell::get)
}

#[cfg(feature = "s3m")]
#[test]
fn pcm_growth_does_not_grow_internal_allocations() {
    let seed = include_bytes!("../../../fuzz/seeds/s3m/minimal.s3m");
    fn with_frames(seed: &[u8], frames: usize) -> Vec<u8> {
        let mut source = seed.to_vec();
        let order_count = u16::from_le_bytes([source[32], source[33]]) as usize;
        let pointer = 96 + order_count;
        let instrument = u16::from_le_bytes([source[pointer], source[pointer + 1]]) as usize * 16;
        let sample_offset = (source.len() + 15) & !15;
        source.resize(sample_offset + frames, 0x81);
        let paragraph = sample_offset / 16;
        source[instrument + 13] = (paragraph >> 16) as u8;
        source[instrument + 14] = paragraph as u8;
        source[instrument + 15] = (paragraph >> 8) as u8;
        source[instrument + 16..instrument + 20].copy_from_slice(&(frames as u32).to_le_bytes());
        source[instrument + 31] = 0;
        source
    }
    let small = with_frames(seed, 32);
    let large = with_frames(seed, 300_000);
    let small_allocation = compare(&small, 1);
    let large_allocation = compare(&large, 1024);
    assert_eq!(large_allocation, small_allocation, "PCM storage belongs solely to caller buffers");
    assert!(large_allocation < 4096, "large sample must not cause a large internal allocation");
}

#[test]
fn native_fixtures_match_with_one_pcm_frame_per_step() {
    let sources: Vec<&[u8]> = vec![
        #[cfg(feature = "mod")]
        include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod"),
        #[cfg(feature = "s3m")]
        include_bytes!("../../../fuzz/seeds/s3m/minimal.s3m"),
        #[cfg(feature = "mtm")]
        include_bytes!("../../../fuzz/seeds/mtm/synthetic-golden.mtm"),
        #[cfg(feature = "xm")]
        include_bytes!("../../../fuzz/seeds/xm/pingpong-16bit.xm"),
        #[cfg(feature = "it")]
        include_bytes!("../../../fuzz/seeds/it/compressed-16bit.it"),
    ];
    for source in sources { compare(source, 1); }
}

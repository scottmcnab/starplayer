//! Borrowed image adoption must report every metadata allocation failure.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use starplayer_core::{Error, I1F15, U0F16};
use starplayer_model::{
    image_workspace_i16, Envelope, EnvelopePoint, ImageDecodeError, InstrumentDef, Module,
    ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec,
};

thread_local! { static REMAINING: Cell<Option<usize>> = const { Cell::new(None) }; }

struct FailingAllocator;

fn refuse() -> bool {
    REMAINING.with(|remaining| match remaining.get() {
        None => false,
        Some(0) => true,
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
    })
}

unsafe impl GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse() { std::ptr::null_mut() } else { unsafe { System.alloc(layout) } }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

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

fn image_with_every_allocating_metadata_kind() -> (Module, &'static [u8]) {
    let mut builder = ModuleBuilder::new();
    let sample = builder
        .add_sample(&[10, 20, 30, 40], SampleSpec::one_shot("sample name").with_forward_loop(1, 4))
        .expect("sample");
    builder.add_pattern(&[1, 2, 3, 4], 1, 1).expect("pattern");
    builder.set_orders(&[0]);
    let mut instrument = InstrumentDef::from_sample("instrument name", sample, U0F16::MAX);
    instrument.volume_envelope = Some(Envelope {
        points: vec![EnvelopePoint { tick: 0, value: 0 }, EnvelopePoint { tick: 8, value: 64 }].into_boxed_slice(),
        sustain: None,
        loop_span: None,
        carry: false,
    });
    builder.add_instrument(instrument).expect("instrument");
    let mut header = ModuleHeader::new(ModuleFormat::Xm, 2);
    header.title = String::from("image title").into_boxed_str();
    header.default_pan = vec![I1F15::MIN, I1F15::MAX].into_boxed_slice();
    header.default_channel_volume = vec![U0F16::MAX, U0F16::from_bits(32_768)].into_boxed_slice();
    header.format_data = vec![1, 2, 3, 4].into_boxed_slice();
    builder.set_header(header);
    let module = builder.build().expect("module");
    let image = module.to_image();
    let mut aligned = vec![0u32; image.len().div_ceil(4)];
    let bytes = bytemuck::cast_slice_mut(&mut aligned);
    bytes.get_mut(..image.len()).expect("aligned storage").copy_from_slice(&image);
    let image_length = image.len();
    let leaked = Vec::leak(aligned);
    let bytes: &'static [u8] = bytemuck::cast_slice(leaked);
    (module, bytes.get(..image_length).expect("image extent"))
}

#[test]
fn every_try_from_image_allocation_failure_is_recoverable() {
    let (expected, image) = image_with_every_allocating_metadata_kind();
    for allocation in 0..64 {
        let result = failing_after(allocation, || Module::try_from_image(image));
        match result {
            Ok(module) => {
                assert_eq!(module, expected);
                return;
            }
            Err(error) => assert!(matches!(error, Error::Resource(_)), "allocation {allocation}: {error:?}"),
        }
    }
    panic!("try_from_image did not succeed after every metadata allocation was exercised");
}

#[test]
fn i16_workspace_skips_an_odd_address_and_reports_its_exact_capacity() {
    let mut storage = [0u8; 8];
    let aligned_start = (core::mem::align_of::<i16>() - (storage.as_ptr() as usize) % core::mem::align_of::<i16>())
        % core::mem::align_of::<i16>();
    {
        let aligned = storage.get_mut(aligned_start..aligned_start + 4).expect("aligned extent");
        assert_eq!(image_workspace_i16(aligned, 2).map(|frames| frames.len()), Ok(2));
    }
    {
        let odd = storage.get_mut(aligned_start + 1..aligned_start + 6).expect("odd extent");
        let frames = image_workspace_i16(odd, 2).expect("one alignment byte and two frames fit");
        assert_eq!(frames.len(), 2);
        assert_eq!((frames.as_ptr() as usize) % core::mem::align_of::<i16>(), 0);
    }
    {
        let odd_short = storage.get_mut(aligned_start + 1..aligned_start + 5).expect("short odd extent");
        assert_eq!(
            image_workspace_i16(odd_short, 2),
            Err(ImageDecodeError::WorkspaceTooSmall { required: 5, available: 4 }),
        );
    }
}

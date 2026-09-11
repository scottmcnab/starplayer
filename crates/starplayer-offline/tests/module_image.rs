//! M8-I2's proof: a module played out of a flash-resident **module image** is the same
//! module, down to the byte the golden contract hashes.
//!
//! Three claims, one per test:
//!
//! 1. every golden fixture survives `to_image` → `from_image` field for field, and the
//!    module that comes back **borrows** its PCM rather than owning it;
//! 2. the borrowed module's canonical render hashes to the committed golden — that is,
//!    `goldens/` is byte-identical through an image, which is what M8-I3 renders on the
//!    device and compares;
//! 3. `Module::enhanced` over a borrowed module still rebuilds an equal, owned module
//!    (M10-K5a's identity-rebuild contract, across the storage split).
//!
//! `Module::from_image` wants a `&'static [u8]`, which on a device is flash and here is a
//! deliberate leak — a handful of images for the life of the test process.

use starplayer::model::{EnhancedPcm, Module, SampleEnhancer, SamplePcm};
use starplayer_offline::{
    GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_INTERPOLATOR, GoldenFixture, canonical_sha256_of_loaded, golden_fixtures,
    golden_filename_for_interpolator, sha256_hex,
};

/// Leak `bytes` as a `'static` slice that starts on a 4-byte boundary, standing in for the
/// `#[repr(C, align(4))]` wrapper a firmware puts around `include_bytes!`.
///
/// The `Vec<u32>` is the point: a leaked `Vec<u8>` promises alignment 1, so a test built on
/// one would pass or fail on the allocator's mood rather than on the code.
fn leak_aligned(bytes: &[u8]) -> &'static [u8] {
    let words: Vec<u32> = vec![0; bytes.len().div_ceil(4)];
    let leaked: &'static mut [u8] = bytemuck::cast_slice_mut(Vec::leak(words));
    let head = &mut leaked[..bytes.len()];
    head.copy_from_slice(bytes);
    head
}

/// The fixture as its loader builds it, and the same module borrowed out of an image.
fn loaded_and_borrowed(fixture: &GoldenFixture) -> (Module, Module) {
    let loaded = starplayer::load(&fixture.bytes).expect("the fixture loads");
    let image = leak_aligned(&loaded.to_image());
    let borrowed = Module::from_image(image).expect("the image reads back");
    (loaded, borrowed)
}

#[test]
fn every_golden_fixture_round_trips_through_a_module_image() {
    for fixture in golden_fixtures() {
        let name = format!("{}/{}", fixture.format, fixture.stem);
        let (loaded, borrowed) = loaded_and_borrowed(&fixture);

        assert_eq!(borrowed, loaded, "{name}: an image must round-trip a module field for field");
        assert!(borrowed.pcm_storage().is_borrowed(), "{name}: the PCM must be borrowed from the image, not copied");
        assert!(borrowed.blob_storage().is_borrowed(), "{name}: the pattern blob must be borrowed from the image, not copied");
    }
}

#[test]
fn the_goldens_are_byte_identical_through_a_module_image() {
    for fixture in golden_fixtures() {
        let name = format!("{}/{}", fixture.format, fixture.stem);
        let (loaded, borrowed) = loaded_and_borrowed(&fixture);

        let from_file = canonical_sha256_of_loaded(loaded, GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_INTERPOLATOR).expect("the fixture renders");
        let from_image = canonical_sha256_of_loaded(borrowed, GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_INTERPOLATOR).expect("the borrowed module renders");
        assert_eq!(from_image, from_file, "{name}: a borrowed module must render exactly what the loaded one does");

        let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../goldens")
            .join(fixture.format.directory())
            .join(golden_filename_for_interpolator(fixture.stem, GOLDEN_INTERPOLATOR));
        let committed = std::fs::read_to_string(&golden_path).expect("the committed golden is readable");
        assert_eq!(sha256_hex(from_image), committed.trim(), "{name}: the committed golden must hold through an image");
    }
}

/// An enhancer that changes nothing — M10-K5a's identity case.
struct Identity;

impl SampleEnhancer for Identity {
    fn name(&self) -> String { String::from("identity") }
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm { EnhancedPcm::unchanged(sample) }
}

#[test]
fn an_identity_rebuild_of_a_borrowed_module_is_an_equal_owned_module() {
    for fixture in golden_fixtures() {
        let name = format!("{}/{}", fixture.format, fixture.stem);
        let (loaded, borrowed) = loaded_and_borrowed(&fixture);
        let rebuilt = borrowed.enhanced(&Identity).expect("an identity rebuild");

        assert_eq!(rebuilt, loaded, "{name}: an identity rebuild must be indistinguishable, borrowed or not");
        assert!(!rebuilt.pcm_storage().is_borrowed(), "{name}: a rebuild goes through the builder, which always owns");
    }
}

#[test]
fn a_misaligned_image_is_refused_rather_than_copied() {
    let fixture = golden_fixtures().into_iter().next().expect("there is at least one fixture");
    let loaded = starplayer::load(&fixture.bytes).expect("the fixture loads");
    let bytes = loaded.to_image();

    let mut shifted = vec![0u8];
    shifted.extend_from_slice(&bytes);
    let misaligned = &leak_aligned(&shifted)[1..];

    assert!(Module::from_image(misaligned).is_err(), "from_image must refuse a misaligned image rather than spend the RAM");
    let recovered = Module::from_image_or_copy(misaligned).expect("the documented fallback reads it");
    assert_eq!(recovered, loaded);
    assert!(!recovered.pcm_storage().is_borrowed(), "the fallback copies exactly the part that could not be borrowed");
    assert!(recovered.blob_storage().is_borrowed(), "bytes have no alignment to lose");
}

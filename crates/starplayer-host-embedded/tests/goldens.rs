//! The bench digest against the committed hashes.
//!
//! M8's exit criterion is that the **device's own** SHA-256 of the canonical ten-second
//! render of every golden fixture equals the hash under `goldens/`. This file proves the
//! claim on the host first, so that a mismatch on the A1S or the C5 is known to be about
//! the silicon and not about [`bench::render_digest`] disagreeing with
//! `starplayer_offline::canonical_sha256_with` on what the canonical render even is.

use std::fs;
use std::path::{Path, PathBuf};

use starplayer::dsp::{Cubic, Linear, Sinc};
use starplayer::rt::Arc;
use starplayer_host_embedded::bench;
use starplayer_offline::{
    GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_RENDER_FRAMES, GOLDEN_SAMPLE_RATE_HZ, GoldenFormat, Interpolator, fixtures,
    golden_filename_for_interpolator, sha256_hex,
};

/// Every fixture in the canonical contract, exactly as `starplayer-goldens` lists them.
fn corpus() -> Vec<(GoldenFormat, &'static str, Vec<u8>)> {
    vec![
        (GoldenFormat::Mod, "synthetic", fixtures::synthetic_mod()),
        (GoldenFormat::Mtm, "synthetic", fixtures::synthetic_mtm()),
        (GoldenFormat::Xm, "synthetic", fixtures::synthetic_xm()),
        (GoldenFormat::It, "synthetic", fixtures::synthetic_it()),
        (GoldenFormat::S3m, "petri", include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M").to_vec()),
        (GoldenFormat::S3m, "reflex", include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M").to_vec()),
    ]
}

fn goldens_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens")
}

fn committed_hash(format: GoldenFormat, stem: &str, kernel: Interpolator) -> String {
    let path = goldens_directory().join(format.directory()).join(golden_filename_for_interpolator(stem, kernel));
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("no committed golden at {}: {error}", path.display())).trim().to_string()
}

fn module(bytes: &[u8]) -> Arc<starplayer::model::Module> {
    Arc::new(starplayer::load(bytes).expect("the fixture loads"))
}

#[test]
fn the_bench_digest_equals_every_committed_golden() {
    for (format, stem, bytes) in corpus() {
        let digest = bench::render_digest::<Linear>(&module(&bytes), GOLDEN_SAMPLE_RATE_HZ, GOLDEN_RENDER_FRAMES, GOLDEN_HOST_BLOCK_FRAMES)
            .unwrap_or_else(|error| panic!("{format} {stem} did not render: {error}"));
        assert_eq!(sha256_hex(digest), committed_hash(format, stem, Interpolator::Linear), "{format} {stem}");
    }
}

/// The two cross-target kernel pins M7-H5 added on `reflex`. They exist to catch a kernel
/// that is not bit-identical across architectures, which is exactly the question M8 asks of
/// Xtensa and RISC-V, so the bench has to be able to produce them too.
#[test]
fn the_bench_digest_equals_the_cubic_and_sinc_pins_as_well() {
    let reflex = module(include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M"));
    let cubic = bench::render_digest::<Cubic>(&reflex, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_RENDER_FRAMES, GOLDEN_HOST_BLOCK_FRAMES).expect("cubic renders");
    assert_eq!(sha256_hex(cubic), committed_hash(GoldenFormat::S3m, "reflex", Interpolator::Cubic));
    let sinc = bench::render_digest::<Sinc>(&reflex, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_RENDER_FRAMES, GOLDEN_HOST_BLOCK_FRAMES).expect("sinc renders");
    assert_eq!(sha256_hex(sinc), committed_hash(GoldenFormat::S3m, "reflex", Interpolator::Sinc));
}

/// The digest is the *audio*'s fingerprint, not the block loop's: hashing incrementally
/// block by block has to give the same answer at any block size, or the device's figure
/// would depend on its DMA buffer.
#[test]
fn the_bench_digest_does_not_depend_on_the_block_size_it_was_taken_at() {
    let reflex = module(include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M"));
    let reference = committed_hash(GoldenFormat::S3m, "reflex", Interpolator::Linear);
    for block_frames in [1, 3, 64, 128, 4_096, 8_191] {
        let digest = bench::render_digest::<Linear>(&reflex, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_RENDER_FRAMES, block_frames).expect("reflex renders");
        assert_eq!(sha256_hex(digest), reference, "block size {block_frames}");
    }
}

/// The stereo arm of the bench — what a firmware times to get cycles per frame — renders
/// the frames it was asked for, through the caller's own scratch.
#[test]
fn the_cycle_count_render_fills_the_callers_scratch_and_reports_what_it_produced() {
    let reflex = module(include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M"));
    let mut scratch = vec![0i16; 512 * starplayer_host_embedded::OUTPUT_CHANNELS];
    let frames = GOLDEN_SAMPLE_RATE_HZ as usize;
    assert_eq!(bench::render_frames::<Linear>(&reflex, GOLDEN_SAMPLE_RATE_HZ, frames, &mut scratch), Ok(frames));
    assert!(scratch.iter().any(|sample| *sample != 0), "the last block carried audio");
}

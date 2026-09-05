//! The scalar-equivalence gate for the mixer's kernels (M7-H6 deliverable 3).
//!
//! The companion of `crates/starplayer-dsp/tests/simd_equivalence.rs`, and the same rule:
//! for every kernel in `starplayer_mixer::simd`, run the `scalar_*` body — compiled into
//! every build — and the body the `simd` feature selects, over the same pseudo-random
//! input, and compare **bit patterns**.
//!
//! The bus summation is compared on both mix paths, because it is vectorised on both. The
//! master volume is compared on the float path, where it is vectorised, and asserted to be
//! the scalar body on the fixed path, where it is not.

#![cfg(feature = "simd")]

use starplayer_core::Xorshift32;
use starplayer_mixer::path::{MixPath, Stereo};
use starplayer_mixer::simd::{scalar_add_block_f32, scalar_add_block_i32, scalar_master_volume_f32, scalar_master_volume_fixed, wide_add_block_f32, wide_add_block_i32, wide_master_volume_f32};
use starplayer_mixer::{FixedPath, FloatPath, Limiter, MasterSettings};

/// Blocks compared. Each is a whole `RENDER_QUANTUM` plus the odd lengths either side of
/// it, because a two-frame-at-a-time kernel's tail is exactly where an off-by-one lives.
const BLOCKS: usize = 1_000;

const LENGTHS: [usize; 6] = [0, 1, 2, 3, 127, 128];

fn next_float(stream: &mut Xorshift32) -> f32 {
    let bits = stream.next_u32();
    match bits & 0xF {
        0 => 0.0,
        1 => -0.0,
        _ => ((bits >> 8) as i32 - (1 << 23)) as f32 * (1.0 / 4096.0),
    }
}

fn next_fixed(stream: &mut Xorshift32) -> i32 {
    let bits = stream.next_u32();
    match bits & 0xF {
        // Saturation is the whole point of the fixed body, so it has to be reached.
        0 => i32::MAX,
        1 => i32::MIN,
        _ => bits as i32,
    }
}

#[test]
fn the_float_bus_summation_agrees_bit_for_bit() {
    let mut stream = Xorshift32::new(0x0B05_0000);
    for block in 0..BLOCKS {
        let length = LENGTHS[block % LENGTHS.len()];
        let source: Vec<Stereo<f32>> = (0..length).map(|_| Stereo::new(next_float(&mut stream), next_float(&mut stream))).collect();
        let start: Vec<Stereo<f32>> = (0..length).map(|_| Stereo::new(next_float(&mut stream), next_float(&mut stream))).collect();

        let mut scalar = start.clone();
        let mut vector = start.clone();
        scalar_add_block_f32(&mut scalar, &source);
        wide_add_block_f32(&mut vector, &source);
        for (index, (expected, got)) in scalar.iter().zip(vector.iter()).enumerate() {
            assert_eq!((expected.left.to_bits(), expected.right.to_bits()), (got.left.to_bits(), got.right.to_bits()), "block {block} length {length} frame {index}");
        }

        // And what `MixPath` actually dispatches to is one of those two.
        let mut dispatched = start.clone();
        FloatPath::add_block(&mut dispatched, &source);
        assert_eq!(dispatched.iter().map(|frame| (frame.left.to_bits(), frame.right.to_bits())).collect::<Vec<_>>(), scalar.iter().map(|frame| (frame.left.to_bits(), frame.right.to_bits())).collect::<Vec<_>>());
    }
}

#[test]
fn the_fixed_bus_summation_agrees_bit_for_bit_including_its_saturation() {
    let mut stream = Xorshift32::new(0x0B05_F1ED);
    for block in 0..BLOCKS {
        let length = LENGTHS[block % LENGTHS.len()];
        let source: Vec<Stereo<i32>> = (0..length).map(|_| Stereo::new(next_fixed(&mut stream), next_fixed(&mut stream))).collect();
        let start: Vec<Stereo<i32>> = (0..length).map(|_| Stereo::new(next_fixed(&mut stream), next_fixed(&mut stream))).collect();

        let mut scalar = start.clone();
        let mut vector = start.clone();
        let mut dispatched = start.clone();
        scalar_add_block_i32(&mut scalar, &source);
        wide_add_block_i32(&mut vector, &source);
        FixedPath::add_block(&mut dispatched, &source);
        assert_eq!(scalar, vector, "block {block} length {length}");
        assert_eq!(scalar, dispatched, "block {block} length {length}");
    }
}

#[test]
fn the_float_master_volume_agrees_bit_for_bit() {
    let mut stream = Xorshift32::new(0xA57E_2000);
    for block in 0..BLOCKS {
        let length = LENGTHS[block % LENGTHS.len()];
        let volume = (stream.next_u32() >> 16) as f32 * (1.0 / 65_535.0);
        let start: Vec<Stereo<f32>> = (0..length).map(|_| Stereo::new(next_float(&mut stream), next_float(&mut stream))).collect();

        let mut scalar = start.clone();
        let mut vector = start.clone();
        scalar_master_volume_f32(&mut scalar, volume);
        wide_master_volume_f32(&mut vector, volume);
        for (index, (expected, got)) in scalar.iter().zip(vector.iter()).enumerate() {
            assert_eq!((expected.left.to_bits(), expected.right.to_bits()), (got.left.to_bits(), got.right.to_bits()), "block {block} length {length} frame {index}");
        }
    }
}

/// The fixed master volume has no vector body — see `starplayer_dsp::simd`'s module
/// documentation — and the whole master bus is compared here anyway, so that the day one
/// appears it is compared too.
#[test]
fn the_fixed_master_bus_is_the_two_passes_the_single_frame_body_describes() {
    let mut stream = Xorshift32::new(0xA57E_F1ED);
    for limiter in [Limiter::Clamp, Limiter::SoftKnee] {
        for _ in 0..BLOCKS {
            let settings = MasterSettings { volume: starplayer_core::U0F16::from_bits((stream.next_u32() >> 16) as u16), limiter };
            let start: Vec<Stereo<i32>> = (0..128).map(|_| Stereo::new(next_fixed(&mut stream), next_fixed(&mut stream))).collect();

            let mut whole = start.clone();
            FixedPath::master(&mut whole, settings);
            let one_at_a_time: Vec<Stereo<i32>> = start.iter().map(|frame| starplayer_mixer::master::process_fixed(*frame, settings)).collect();
            assert_eq!(whole, one_at_a_time, "splitting the volume from the limiter moved a sample");

            let mut volume_only = start.clone();
            scalar_master_volume_fixed(&mut volume_only, settings.volume.to_bits() as i64);
            assert!(volume_only.iter().zip(start.iter()).all(|(scaled, original)| scaled.left.unsigned_abs() <= original.left.unsigned_abs()), "a Q0.16 volume may not raise a magnitude");
        }
    }
}

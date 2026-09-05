//! The scalar-equivalence gate (M7-H6 deliverable 3).
//!
//! Architecture §7.1: SIMD is "an optimisation *inside* the monomorphised loop, never a
//! semantic change". This file is what makes that testable rather than asserted. For every
//! kernel in `starplayer_dsp::simd` it runs the `scalar_*` body — which is compiled into
//! every build — and the body the `simd` feature actually selects, over the same pseudo
//! random inputs, and compares **bit patterns**. Not magnitudes, not a tolerance: the
//! fixed path is the cross-target golden reference and the float path's block-size
//! determinism is a byte comparison, so anything short of bit-identical is a regression.
//!
//! The file only exists when the feature is on: with it off the two bodies are literally
//! the same function and there is nothing to compare.
//!
//! The fixed path is exercised too, and is expected to be trivially equal — `i32` never
//! overrides the dispatch seam, for the reason `starplayer_dsp::simd`'s module
//! documentation gives. The assertion is what will notice the day that stops being true.

#![cfg(feature = "simd")]

use starplayer_core::Xorshift32;
use starplayer_dsp::biquad::BiquadCoefficients;
use starplayer_dsp::simd::{COMB_LANES, TAP_LANES, scalar_biquad_stereo_step, scalar_comb_bank_step, scalar_interpolate_taps, scalar_sinc_dot_f32, wide_sinc_dot_f32};
use starplayer_dsp::{DspSample, Stereo};

/// Blocks of random input every kernel is compared over.
const BLOCKS: usize = 1_000;

/// Frames in one such block — a whole `DSP_BLOCK_FRAMES`, so a recursive kernel is
/// compared after a realistic amount of state has accumulated rather than from silence.
const FRAMES: usize = 128;

/// One pseudo-random sample on the mixer's raw `i16` scale, with the awkward values
/// deliberately over-represented: exact zero, negative zero, and full scale, which are
/// where a lane-wise selection and a scalar branch are most likely to disagree.
fn next_sample(stream: &mut Xorshift32) -> f32 {
    let bits = stream.next_u32();
    match bits & 0x1F {
        0 => 0.0,
        1 => -0.0,
        2 => 32_767.0,
        3 => -32_768.0,
        _ => ((bits >> 8) as i32 - (1 << 23)) as f32 * (1.0 / 256.0),
    }
}

/// A Q8.24 coefficient somewhere in `[0, 1)`, which is where every coefficient this crate
/// cooks lives, plus the two ends.
fn next_coefficient_q24(stream: &mut Xorshift32) -> i32 {
    let bits = stream.next_u32();
    match bits & 0xF {
        0 => 0,
        1 => 1 << 24,
        _ => (bits >> 8) as i32,
    }
}

fn assert_bits_equal(scalar: f32, vector: f32, what: &str) {
    assert_eq!(scalar.to_bits(), vector.to_bits(), "{what}: scalar {scalar:?} ({:#010x}) vs simd {vector:?} ({:#010x})", scalar.to_bits(), vector.to_bits());
}

#[test]
fn the_comb_bank_kernel_agrees_bit_for_bit_on_the_float_path() {
    let mut stream = Xorshift32::new(0x051D_C0DE);
    for block in 0..BLOCKS {
        let feedback_q24 = next_coefficient_q24(&mut stream);
        let damping_q24 = next_coefficient_q24(&mut stream);
        let damping_complement_q24 = (1 << 24) - damping_q24;
        let mut scalar_stores = [0.0f32; COMB_LANES];
        let mut vector_stores = [0.0f32; COMB_LANES];

        for frame in 0..FRAMES {
            let input = next_sample(&mut stream);
            let mut delayed = [0.0f32; COMB_LANES];
            for slot in delayed.iter_mut() {
                *slot = next_sample(&mut stream);
            }

            let scalar = scalar_comb_bank_step(input, delayed, &mut scalar_stores, feedback_q24, damping_q24, damping_complement_q24, (i16::MAX as i32) << 6);
            let vector = <f32 as DspSample>::comb_bank_step(input, delayed, &mut vector_stores, feedback_q24, damping_q24, damping_complement_q24, (i16::MAX as i32) << 6);

            for lane in 0..COMB_LANES {
                assert_bits_equal(scalar[lane], vector[lane], &format!("block {block} frame {frame} comb {lane} output"));
                assert_bits_equal(scalar_stores[lane], vector_stores[lane], &format!("block {block} frame {frame} comb {lane} store"));
            }
        }
    }
}

#[test]
fn the_comb_bank_kernel_is_the_scalar_body_on_the_fixed_path() {
    let mut stream = Xorshift32::new(0x00C0_FFEE);
    for _ in 0..BLOCKS {
        let feedback_q24 = next_coefficient_q24(&mut stream);
        let damping_q24 = next_coefficient_q24(&mut stream);
        let damping_complement_q24 = (1 << 24) - damping_q24;
        let mut scalar_stores = [0i32; COMB_LANES];
        let mut dispatched_stores = [0i32; COMB_LANES];

        for _ in 0..FRAMES {
            let input = next_sample(&mut stream) as i32;
            let mut delayed = [0i32; COMB_LANES];
            for slot in delayed.iter_mut() {
                *slot = next_sample(&mut stream) as i32;
            }
            let scalar = scalar_comb_bank_step(input, delayed, &mut scalar_stores, feedback_q24, damping_q24, damping_complement_q24, (i16::MAX as i32) << 6);
            let dispatched = <i32 as DspSample>::comb_bank_step(input, delayed, &mut dispatched_stores, feedback_q24, damping_q24, damping_complement_q24, (i16::MAX as i32) << 6);
            assert_eq!(scalar, dispatched, "the fixed path must not have acquired a vector body without a test");
            assert_eq!(scalar_stores, dispatched_stores);
        }
    }
}

#[test]
fn the_stereo_biquad_kernel_agrees_bit_for_bit_on_the_float_path() {
    let mut stream = Xorshift32::new(0x0B19_0AD5);
    for block in 0..BLOCKS {
        // A biquad's coefficients live in `[-2, 2)` of Q8.24 rather than `[0, 1)`.
        let coefficients = BiquadCoefficients {
            b0: next_coefficient_q24(&mut stream) - (1 << 24),
            b1: next_coefficient_q24(&mut stream) - (1 << 24),
            b2: next_coefficient_q24(&mut stream) - (1 << 24),
            a1: next_coefficient_q24(&mut stream) - (1 << 24),
            a2: next_coefficient_q24(&mut stream) - (1 << 24),
        };
        let mut scalar_left = [0.0f32; 2];
        let mut scalar_right = [0.0f32; 2];
        let mut vector_left = [0.0f32; 2];
        let mut vector_right = [0.0f32; 2];

        for frame in 0..FRAMES {
            let input = Stereo::new(next_sample(&mut stream), next_sample(&mut stream));
            let scalar = scalar_biquad_stereo_step(&coefficients, input, &mut scalar_left, &mut scalar_right);
            let vector = <f32 as DspSample>::biquad_stereo_step(&coefficients, input, &mut vector_left, &mut vector_right);
            assert_bits_equal(scalar.left, vector.left, &format!("block {block} frame {frame} left"));
            assert_bits_equal(scalar.right, vector.right, &format!("block {block} frame {frame} right"));
            for index in 0..2 {
                assert_bits_equal(scalar_left[index], vector_left[index], &format!("block {block} frame {frame} left state {index}"));
                assert_bits_equal(scalar_right[index], vector_right[index], &format!("block {block} frame {frame} right state {index}"));
            }
        }
    }
}

#[test]
fn the_stereo_biquad_kernel_is_the_scalar_body_on_the_fixed_path() {
    let mut stream = Xorshift32::new(0xFEED_FACE);
    let coefficients = BiquadCoefficients { b0: 1 << 23, b1: -(1 << 22), b2: 1 << 20, a1: -(1 << 21), a2: 1 << 19 };
    let mut scalar_left = [0i32; 2];
    let mut scalar_right = [0i32; 2];
    let mut dispatched_left = [0i32; 2];
    let mut dispatched_right = [0i32; 2];
    for _ in 0..BLOCKS * FRAMES {
        let input = Stereo::new(next_sample(&mut stream) as i32, next_sample(&mut stream) as i32);
        let scalar = scalar_biquad_stereo_step(&coefficients, input, &mut scalar_left, &mut scalar_right);
        let dispatched = <i32 as DspSample>::biquad_stereo_step(&coefficients, input, &mut dispatched_left, &mut dispatched_right);
        assert_eq!(scalar, dispatched);
        assert_eq!((scalar_left, scalar_right), (dispatched_left, dispatched_right));
    }
}

#[test]
fn the_fractional_tap_kernel_agrees_bit_for_bit_on_the_float_path() {
    let mut stream = Xorshift32::new(0x07A9_5EED);
    for block in 0..BLOCKS * FRAMES / 16 {
        let mut current = [0.0f32; TAP_LANES];
        let mut next = [0.0f32; TAP_LANES];
        let mut fraction = [0i32; TAP_LANES];
        for lane in 0..TAP_LANES {
            current[lane] = next_sample(&mut stream);
            next[lane] = next_sample(&mut stream);
            // A Q0.16 fraction, with zero — the scalar body's early return — over-weighted.
            fraction[lane] = if stream.next_u32() & 3 == 0 { 0 } else { (stream.next_u32() & 0xFFFF) as i32 };
        }
        let scalar = scalar_interpolate_taps(current, next, fraction);
        let vector = <f32 as DspSample>::interpolate_taps(current, next, fraction);
        for lane in 0..TAP_LANES {
            assert_bits_equal(scalar[lane], vector[lane], &format!("block {block} tap {lane}"));
        }
    }
}

#[test]
fn the_fractional_tap_kernel_is_the_scalar_body_on_the_fixed_path() {
    let mut stream = Xorshift32::new(0x1234_5678);
    for _ in 0..BLOCKS * FRAMES / 16 {
        let mut current = [0i32; TAP_LANES];
        let mut next = [0i32; TAP_LANES];
        let mut fraction = [0i32; TAP_LANES];
        for lane in 0..TAP_LANES {
            current[lane] = next_sample(&mut stream) as i32;
            next[lane] = next_sample(&mut stream) as i32;
            fraction[lane] = if stream.next_u32() & 3 == 0 { 0 } else { (stream.next_u32() & 0xFFFF) as i32 };
        }
        assert_eq!(scalar_interpolate_taps(current, next, fraction), <i32 as DspSample>::interpolate_taps(current, next, fraction));
    }
}

#[test]
fn the_sinc_dot_product_agrees_bit_for_bit() {
    let mut stream = Xorshift32::new(0x9E37_79B9);
    for block in 0..BLOCKS * FRAMES / 8 {
        let mut taps = [0i16; 8];
        let mut coefficients = [0i16; 8];
        for lane in 0..8 {
            taps[lane] = (stream.next_u32() >> 16) as i16;
            coefficients[lane] = (stream.next_u32() >> 16) as i16;
        }
        assert_bits_equal(scalar_sinc_dot_f32(&taps, &coefficients), wide_sinc_dot_f32(&taps, &coefficients), &format!("block {block}"));
    }
}

/// Research point 2, answered with evidence rather than with the documentation.
///
/// SSE2 has no `pmulld`, so `wide` emulates a 32-bit lane multiply with two `pmuludq`
/// passes and a shuffle. This asserts the emulation is exact — the same low 32 bits
/// `i32::wrapping_mul` produces — and that `widening_mul` is the same full product `i64`
/// multiplication gives, which is what a future fixed-path kernel would have to rely on.
#[test]
fn the_vector_integer_multiply_is_exact() {
    use wide::i32x4;

    let mut stream = Xorshift32::new(0x000B_5EED);
    for _ in 0..10_000 {
        let mut left = [0i32; 4];
        let mut right = [0i32; 4];
        for lane in 0..4 {
            left[lane] = stream.next_u32() as i32;
            right[lane] = stream.next_u32() as i32;
        }
        let product = (i32x4::new(left) * i32x4::new(right)).to_array();
        let widened = i32x4::new(left).widening_mul(i32x4::new(right)).to_array();
        for lane in 0..4 {
            assert_eq!(product[lane], left[lane].wrapping_mul(right[lane]), "lane multiply is not the scalar wrapping product");
            assert_eq!(widened[lane], left[lane] as i64 * right[lane] as i64, "widening multiply is not the full product");
        }
    }
}

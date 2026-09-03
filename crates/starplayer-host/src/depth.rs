//! Output depth and dither, applied *after* the engine's output ring.
//!
//! Architecture §7.1: depth and dither are not engine arms. Making them arms would turn
//! eight instantiations into forty; making them a post-stage gives every arm all five
//! depths, costs one pass over the block, and leaves the fixed path's native `i16`
//! untouched at [`OutputDepth::I16`] — so the canonical bit-exact path stays bit-exact.
//!
//! Lifted out of the browser host (task D4, research point 1): both hosts quantise the same
//! way from the same seed, or the same module at the same settings sounds different in the
//! browser and on the desktop.

use starplayer::engine::{MixerMode, OutputDepth};
use starplayer::mixer::{Dither, HostSample, I24};

/// `"STAR"`. The dither stream is seeded, never random: an offline render has to be
/// reproducible and the goldens have to be able to fingerprint a dithered one.
pub const DITHER_SEED: u32 = 0x5354_4152;

/// The dither generator `mode` asks for.
pub fn dither_for(mode: MixerMode) -> Dither {
    if mode.dither { Dither::seeded(DITHER_SEED) } else { Dither::OFF }
}

/// Quantise one float-path sample to `depth`, and report the `i16` when that is the depth.
///
/// The first value is the sample scaled back to `±1.0` — what a float host writes out, now
/// carrying only the resolution `depth` allows. The second is the exact integer code, for a
/// host whose device really does take `i16`.
pub fn quantize_float_sample(value: f32, depth: OutputDepth, dither: &mut Dither) -> (f32, Option<i16>) {
    match depth {
        OutputDepth::F32 => (<f32 as HostSample>::from_unit_f32(value, dither), None),
        OutputDepth::I32 => (<i32 as HostSample>::from_unit_f32(value, dither) as f32 * (1.0 / 2_147_483_520.0), None),
        OutputDepth::I24 => (<I24 as HostSample>::from_unit_f32(value, dither).0 as f32 * (1.0 / 8_388_607.0), None),
        OutputDepth::I16 => {
            let quantized = <i16 as HostSample>::from_unit_f32(value, dither);
            (quantized as f32 * (1.0 / 32_767.0), Some(quantized))
        }
        OutputDepth::I8 => (<i8 as HostSample>::from_unit_f32(value, dither) as f32 * (1.0 / 127.0), None),
    }
}

/// Quantise one fixed-path sample to `depth`. The `i16` arm is a pass-through by
/// construction: the accumulator is already on that scale.
pub fn quantize_fixed_sample(value: i32, depth: OutputDepth, dither: &mut Dither) -> (f32, Option<i16>) {
    match depth {
        OutputDepth::F32 => (<f32 as HostSample>::from_i16_scale(value, dither), None),
        OutputDepth::I32 => (<i32 as HostSample>::from_i16_scale(value, dither) as f32 * (1.0 / 2_147_483_520.0), None),
        OutputDepth::I24 => (<I24 as HostSample>::from_i16_scale(value, dither).0 as f32 * (1.0 / 8_388_607.0), None),
        OutputDepth::I16 => {
            let quantized = <i16 as HostSample>::from_i16_scale(value, dither);
            (quantized as f32 * (1.0 / 32_767.0), Some(quantized))
        }
        OutputDepth::I8 => (<i8 as HostSample>::from_i16_scale(value, dither) as f32 * (1.0 / 127.0), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_fixed_i16_arm_is_a_pass_through() {
        let mut dither = Dither::OFF;
        for value in [-32_767, -1, 0, 1, 12_345, 32_767] {
            assert_eq!(quantize_fixed_sample(value, OutputDepth::I16, &mut dither).1, Some(value as i16));
        }
    }

    #[test]
    fn eight_bit_output_cannot_take_more_than_256_values() {
        let mut dither = Dither::OFF;
        let values: BTreeSet<u32> = (0..4_096)
            .map(|step| quantize_float_sample(step as f32 / 2_048.0 - 1.0, OutputDepth::I8, &mut dither).0.to_bits())
            .collect();
        assert!(values.len() <= 256, "8-bit output produced {} distinct values", values.len());
        assert!(values.len() > 200, "and it really did use the depth: {}", values.len());
    }

    #[test]
    fn the_seeded_dither_stream_repeats_exactly_and_changes_the_output() {
        let mode = MixerMode { dither: true, ..MixerMode::DEFAULT };
        let run = |mut dither: Dither| {
            (0..64).map(|step| quantize_float_sample(step as f32 / 128.0, OutputDepth::I8, &mut dither).0.to_bits()).collect::<Vec<u32>>()
        };
        assert_eq!(run(dither_for(mode)), run(dither_for(mode)));
        assert_ne!(run(dither_for(mode)), run(Dither::OFF));
    }
}

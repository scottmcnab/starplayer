//! Arithmetic the enhancers may use on the far side of a determinism contract: a square
//! root and a power, built from `+ − × ÷` alone.
//!
//! # Why not `f64::sqrt` and `f64::powf`
//!
//! Neither exists in `core` — both are `std`-only inherent methods that reach libm — so a
//! `no_std` crate cannot call them at all, and even where they exist their last bits are
//! not guaranteed to agree between the libm a native build links and the one a wasm build
//! carries. An enhanced module is hashed as a regression contract
//! (`crates/starplayer-enhance/tests/determinism.rs`), so a single differing bit anywhere
//! in a sample's rebuild is a broken contract.
//!
//! IEEE-754 `+ − × ÷` are exactly specified and correctly rounded on every target this
//! engine builds for, so a fixed number of Newton steps from a fixed starting point is
//! bit-identical everywhere. That is the whole trick: the routines below are not
//! *faster* than libm and are not meant to be — they are **reproducible**, which libm is
//! not.
//!
//! The committed polyphase and window tables are the other half of the same rule: anything
//! that can be computed once at build time is, and only what genuinely depends on the
//! sample's own data is computed at run time, here.

/// Newton steps [`square_root`] takes.
///
/// The seed below lands within about 3 % relative error, and a Newton step squares the
/// error: 3e-2, 5e-4, 1e-7, 6e-15, then the iteration is at the last bit and stays there.
/// Eight steps is four more than convergence needs, which costs nothing at load time and
/// removes any question of the count mattering.
const SQUARE_ROOT_STEPS: usize = 8;

/// Bits of binary expansion [`power_hundredths`] takes for the fractional part of its
/// exponent.
///
/// A hundredth is not a terminating binary fraction, so the expansion is cut off rather
/// than finishing, and the cut-off is the routine's whole error: the exponent is short by
/// less than `2^-64`, which moves the result by less than one `f64` step for any base a
/// gain can take. Thirty-two bits was visibly not enough — it left `0.01^0.01` a part in
/// 10^9 high. The loop leaves early whenever the expansion does terminate.
const POWER_FRACTION_BITS: usize = 64;

/// The positive square root of `value`, by Newton's method — `+ − × ÷` only.
///
/// Returns `0.0` for zero, for a negative input and for anything not finite, so a caller
/// working in the mean-square domain never has to guard its own arithmetic.
pub fn square_root(value: f64) -> f64 {
    if !(value > 0.0) || value == f64::INFINITY {
        return 0.0;
    }
    // Halve the exponent by halving the whole bit pattern and re-biasing. For a positive
    // normal `f64` this lands within a few percent of the root; for a subnormal it lands
    // somewhere sane, and the Newton steps do the rest.
    let mut estimate = f64::from_bits((value.to_bits() >> 1) + (1023u64 << 51));
    if !(estimate > 0.0) {
        estimate = value;
    }
    for _ in 0..SQUARE_ROOT_STEPS {
        estimate = 0.5 * (estimate + value / estimate);
    }
    estimate
}

/// `base` raised to `exponent_hundredths / 100`, for a `base` in `0.0 ..= 1.0`.
///
/// The whole part is repeated multiplication; the fractional part is the binary expansion
/// of the exponent, multiplying in successive square roots of the base — `base^(1/2)`,
/// `base^(1/4)` and so on — for each set bit. Every step is a multiplication or a
/// [`square_root`], so the result is the same on every target.
///
/// A `base` of zero answers `0.0` for any non-zero exponent and `1.0` for a zero one, as
/// the ordinary power does; a `base` above one is clamped, because both callers are
/// working with gains that must never boost.
pub fn power_hundredths(base: f64, exponent_hundredths: u32) -> f64 {
    if exponent_hundredths == 0 {
        return 1.0;
    }
    let base = if base > 1.0 { 1.0 } else { base };
    if !(base > 0.0) {
        return 0.0;
    }

    let mut result = 1.0f64;
    for _ in 0..exponent_hundredths / 100 {
        result *= base;
    }

    // The remaining hundredths, as a binary fraction: at step `k` the running root is
    // `base^(1/2^k)`, and the bit is set when doubling the remainder crosses one.
    let mut remainder = u64::from(exponent_hundredths % 100);
    let mut root = base;
    for _ in 0..POWER_FRACTION_BITS {
        if remainder == 0 {
            break;
        }
        root = square_root(root);
        remainder *= 2;
        if remainder >= 100 {
            result *= root;
            remainder -= 100;
        }
    }
    result
}

/// Round half away from zero, then saturate to `i16`.
///
/// `as i64` truncates towards zero and saturates on a non-finite input, so adding half a
/// unit in the value's own direction first is exactly "round half away from zero" with no
/// branch on the rounding mode of the target. Every enhancer that writes a frame writes it
/// through here, so they all round the same way.
pub fn saturating_i16(value: f64) -> i16 {
    let rounded = if value >= 0.0 { (value + 0.5) as i64 } else { (value - 0.5) as i64 };
    rounded.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_square_root_matches_the_library_one_across_the_range_it_is_used_in() {
        let cases = [1.0e-12, 1.0e-6, 0.001, 0.25, 0.5, 1.0, 2.0, 12.0, 5_461.333_333_333_333, 1.0e6, 1.0e12, 1.0e18];
        for value in cases {
            let ours = square_root(value);
            let theirs = std::primitive::f64::sqrt(value);
            assert!((ours - theirs).abs() <= 4.0 * f64::EPSILON * theirs, "sqrt({value}): {ours} against {theirs}");
        }
    }

    #[test]
    fn the_square_root_of_a_degenerate_input_is_zero_rather_than_a_nan() {
        assert_eq!(square_root(0.0), 0.0);
        assert_eq!(square_root(-1.0), 0.0);
        assert_eq!(square_root(f64::NAN), 0.0);
        assert_eq!(square_root(f64::INFINITY), 0.0);
    }

    #[test]
    fn the_power_matches_the_library_one_for_every_hundredth_a_knob_can_ask_for() {
        for base_step in 1..=100u32 {
            let base = base_step as f64 / 100.0;
            for exponent in [1u32, 25, 30, 50, 99, 100, 150, 200, 275, 300] {
                let ours = power_hundredths(base, exponent);
                let theirs = std::primitive::f64::powf(base, exponent as f64 / 100.0);
                assert!((ours - theirs).abs() < 1.0e-12, "{base}^{exponent}/100: {ours} against {theirs}");
            }
        }
    }

    #[test]
    fn the_power_agrees_with_repeated_multiplication_on_whole_exponents() {
        assert_eq!(power_hundredths(0.5, 0), 1.0);
        assert_eq!(power_hundredths(0.5, 100), 0.5);
        assert_eq!(power_hundredths(0.5, 200), 0.25);
        assert_eq!(power_hundredths(0.5, 300), 0.125);
        assert_eq!(power_hundredths(0.0, 100), 0.0);
        assert_eq!(power_hundredths(0.0, 0), 1.0);
        assert_eq!(power_hundredths(2.0, 200), 1.0, "a base above one is clamped, because both callers are gains");
    }

    /// The reason this module exists: the same inputs give the same bits every time, so a
    /// rebuilt module's hash is a contract rather than a coincidence.
    #[test]
    fn the_same_inputs_give_the_same_bits_twice() {
        for step in 1..=64u32 {
            let base = step as f64 / 64.0;
            assert_eq!(square_root(base).to_bits(), square_root(base).to_bits());
            assert_eq!(power_hundredths(base, 137).to_bits(), power_hundredths(base, 137).to_bits());
        }
    }

    #[test]
    fn rounding_is_half_away_from_zero_and_saturates() {
        assert_eq!(saturating_i16(0.5), 1);
        assert_eq!(saturating_i16(-0.5), -1);
        assert_eq!(saturating_i16(1.4), 1);
        assert_eq!(saturating_i16(-1.4), -1);
        assert_eq!(saturating_i16(1.0e9), i16::MAX);
        assert_eq!(saturating_i16(-1.0e9), i16::MIN);
    }
}

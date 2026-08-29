//! Fixed-point arithmetic: the unit scalars `U0F16` / `I1F15`, the Q32.32 resample
//! increment [`Step`], and the Q32.32 general value [`Q32_32`] used by the tick
//! accumulator.
//!
//! # Why this split (task A2 research point 1)
//!
//! Two representations are needed and they want different treatments, so this module
//! deliberately does both things rather than picking one:
//!
//! * **Unit scalars come from the `fixed` crate, re-exported unchanged.** `U0F16` and
//!   `I1F15` are literally `fixed`'s own type names — the architecture (§2.1, §2.3) was
//!   written against them — and the operations they need are the ones that are easy to
//!   get subtly wrong by hand: saturating multiply of two fractions, rounding
//!   conversions, and lossless widening. `fixed` has those, exhaustively tested
//!   upstream, and is `no_std` unless `std` is opted into. Wrapping it in newtypes here
//!   would mean re-exporting most of its API by hand for no gain, so it is re-exported
//!   directly.
//!
//!   The cost, recorded here because it is not free: `fixed` 1.31 depends unconditionally
//!   on `half`, which drags in `zerocopy` (and therefore a `syn` proc-macro build), plus
//!   `az`, `bytemuck`, `typenum` and `cfg-if` — eight runtime crates on the root of the
//!   graph. All of them build clean for `riscv32imc-unknown-none-elf` and none enables
//!   `std`, so no design goal is violated, but if the supply-chain surface of the root
//!   crate ever becomes the binding constraint, `U0F16` and `I1F15` are the two types
//!   here small enough to replace with newtypes over `u16` / `i16`.
//!
//! * **Q32.32 is a hand-written `u64` newtype.** Architecture §2.5 pins the shape as
//!   `pub struct Step(pub u64)`, and that is the right call: the mixer inner loop is
//!   `position = position.wrapping_add(step.0)` on a raw `u64`, and the tick accumulator
//!   is `accumulator += frames_per_tick; whole = accumulator >> 32`. Those are the two
//!   hottest lines in the engine and they should read as exactly what the 80386 original
//!   did (`SB_ProcessTracks`, `S3MLIB.ASM` ~5795, splits the same 32.32 value across
//!   `_Mix_HighSpeed` / `_Mix_LowSpeed`). Nothing is hand-rolled beyond calling
//!   `u64::saturating_add` and friends; the only arithmetic written out is the
//!   `u128`-widened exact ratio, which `fixed` does not offer in this shape anyway.
//!
//! # Rules
//!
//! Every operation here saturates. Nothing wraps silently, nothing panics, and there is
//! no floating point and no transcendental function anywhere in this module — the
//! fixed-point path has to be bit-identical on x86, ARM and WASM (architecture §7.3).

pub use fixed::types::{I1F15, U0F16};

/// Number of fractional bits in the Q32.32 types.
pub const Q32_32_FRACTIONAL_BITS: u32 = 32;

/// One whole unit in Q32.32 bits.
const Q32_32_ONE: u64 = 1 << Q32_32_FRACTIONAL_BITS;

/// Full-scale numerator for [`U0F16`]. `U0F16` spans `0 .. 65535/65536`, so unity is
/// `U0F16::MAX`, one LSB below a mathematical 1.0.
const U0F16_FULL_SCALE: u32 = u16::MAX as u32;

/// Full-scale numerator for [`I1F15`]. `I1F15` spans `-1.0 ..= 32767/32768`, so `+1.0`
/// saturates to `I1F15::MAX` while `-1.0` is exact.
const I1F15_FULL_SCALE: i32 = i16::MAX as i32;

/// Convert a tracker-native ratio (S3M volume 0..64, IT global volume 0..128, a MIDI
/// 7-bit controller 0..127, …) to a unit scalar, rounding to nearest.
///
/// `numerator` is clamped to `denominator`, and a zero `denominator` yields zero rather
/// than panicking — nothing in this crate may panic on the audio thread.
pub const fn unit_from_ratio(numerator: u32, denominator: u32) -> U0F16 {
    if denominator == 0 {
        return U0F16::ZERO;
    }
    let clamped = if numerator > denominator { denominator } else { numerator };
    let scaled = (clamped as u64) * (U0F16_FULL_SCALE as u64) + (denominator as u64) / 2;
    U0F16::from_bits((scaled / denominator as u64) as u16)
}

/// Convert a MIDI 7-bit value (velocity, controller, aftertouch) to a unit scalar.
pub const fn unit_from_midi7(value: u8) -> U0F16 {
    unit_from_ratio(value as u32, 127)
}

/// Convert a signed ratio (pan −8..=+8, a pitch-bend offset, …) to a bipolar scalar,
/// rounding to nearest.
///
/// The output range is **symmetric**, `[-I1F15::MAX, I1F15::MAX]`: full scale in either
/// direction is ±32767, and `I1F15::MIN` (an exact −1.0) is deliberately never produced.
/// A symmetric range means negating a converted value is always representable, which is
/// what pan reversal and bend inversion need, and it keeps `x * x` from being the one
/// input that saturates.
///
/// A zero `denominator` yields zero rather than panicking.
pub const fn bipolar_from_ratio(numerator: i32, denominator: i32) -> I1F15 {
    if denominator == 0 {
        return I1F15::ZERO;
    }
    let (numerator, denominator) = if denominator < 0 { (-numerator, -denominator) } else { (numerator, denominator) };
    let magnitude = if numerator < 0 { -numerator } else { numerator };
    let clamped = if magnitude > denominator { denominator } else { magnitude };
    let scaled = (clamped as i64) * (I1F15_FULL_SCALE as i64) + (denominator as i64) / 2;
    let bits = (scaled / denominator as i64) as i16;
    if numerator < 0 { I1F15::from_bits(-bits) } else { I1F15::from_bits(bits) }
}

/// Convert a 14-bit MIDI pitch-bend word (0..=16383, centre 8192) to a bipolar scalar.
pub const fn bipolar_from_midi_bend(bend: u16) -> I1F15 {
    let centred = bend as i32 - 8192;
    bipolar_from_ratio(centred, 8192)
}

/// Q32.32 sample-position increment per output frame (architecture §2.5).
///
/// The mixer wants a resample increment, not a frequency: the division belongs in format
/// code, which knows both the sample's reference rate and the output rate and runs it at
/// tick rate. A Q16.16 Hz type would also cap at 65535 Hz for no reason.
///
/// The integer part is the number of whole source frames consumed per output frame, so a
/// `Step` of [`Step::ONE`] plays a sample back at exactly its own rate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Step(pub u64);

impl Step {
    /// A silent / stalled voice.
    pub const ZERO: Step = Step(0);

    /// Playback at exactly the sample's own rate.
    pub const ONE: Step = Step(Q32_32_ONE);

    /// The largest representable increment.
    pub const MAX: Step = Step(u64::MAX);

    /// Wrap raw Q32.32 bits.
    pub const fn from_bits(bits: u64) -> Step { Step(bits) }

    /// The raw Q32.32 bits. This is what the mixer inner loop adds to a position.
    pub const fn to_bits(self) -> u64 { self.0 }

    /// Whole source frames per output frame.
    pub const fn integer_part(self) -> u32 { (self.0 >> Q32_32_FRACTIONAL_BITS) as u32 }

    /// The fractional part, as raw Q0.32 bits.
    pub const fn fractional_bits(self) -> u32 { self.0 as u32 }

    /// Exact `numerator / denominator` in Q32.32, saturating.
    ///
    /// This is the `hz / output_rate` divide the original performs with a 64-bit
    /// `div` in `SB_ProcessTracks` (`S3MLIB.ASM` ~5795). A zero `denominator` saturates
    /// to [`Step::MAX`] rather than panicking.
    pub const fn from_ratio(numerator: u64, denominator: u64) -> Step {
        Step(q32_32_bits_from_ratio(numerator, denominator))
    }

    /// Saturating addition — used by pitch slides, never by the sample position.
    pub const fn saturating_add(self, other: Step) -> Step { Step(self.0.saturating_add(other.0)) }

    /// Saturating subtraction.
    pub const fn saturating_sub(self, other: Step) -> Step { Step(self.0.saturating_sub(other.0)) }

    /// Saturating multiplication by a whole number, for arpeggio-style octave scaling.
    pub const fn saturating_mul_int(self, factor: u32) -> Step { Step(self.0.saturating_mul(factor as u64)) }

    /// Saturating division by a whole number. A zero `divisor` yields [`Step::MAX`],
    /// which is the saturating limit of the mathematical result.
    pub const fn saturating_div_int(self, divisor: u32) -> Step {
        if divisor == 0 { Step::MAX } else { Step(self.0 / divisor as u64) }
    }
}

/// A general unsigned Q32.32 value.
///
/// Same representation as [`Step`] and a deliberately separate type: the tick
/// accumulator measures *time in output frames*, not a sample increment, and mixing the
/// two up is exactly the class of bug this crate exists to make impossible.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Q32_32(pub u64);

impl Q32_32 {
    /// Zero.
    pub const ZERO: Q32_32 = Q32_32(0);

    /// One whole unit.
    pub const ONE: Q32_32 = Q32_32(Q32_32_ONE);

    /// The largest representable value.
    pub const MAX: Q32_32 = Q32_32(u64::MAX);

    /// Wrap raw Q32.32 bits.
    pub const fn from_bits(bits: u64) -> Q32_32 { Q32_32(bits) }

    /// The raw Q32.32 bits.
    pub const fn to_bits(self) -> u64 { self.0 }

    /// Wrap a whole number, saturating.
    pub const fn from_integer(value: u32) -> Q32_32 { Q32_32((value as u64) << Q32_32_FRACTIONAL_BITS) }

    /// Exact `numerator / denominator` in Q32.32, saturating. A zero `denominator`
    /// yields [`Q32_32::MAX`] rather than panicking.
    pub const fn from_ratio(numerator: u64, denominator: u64) -> Q32_32 {
        Q32_32(q32_32_bits_from_ratio(numerator, denominator))
    }

    /// The whole part.
    pub const fn integer_part(self) -> u32 { (self.0 >> Q32_32_FRACTIONAL_BITS) as u32 }

    /// The fractional part, as raw Q0.32 bits.
    pub const fn fractional_bits(self) -> u32 { self.0 as u32 }

    /// Saturating addition.
    pub const fn saturating_add(self, other: Q32_32) -> Q32_32 { Q32_32(self.0.saturating_add(other.0)) }

    /// Saturating subtraction.
    pub const fn saturating_sub(self, other: Q32_32) -> Q32_32 { Q32_32(self.0.saturating_sub(other.0)) }

    /// Saturating multiplication by a whole number.
    pub const fn saturating_mul_int(self, factor: u32) -> Q32_32 { Q32_32(self.0.saturating_mul(factor as u64)) }

    /// Remove and return the whole part, leaving only the fraction behind.
    ///
    /// This is the tick accumulator's one operation: add a Q32.32 tick length, then take
    /// the whole output frames out and carry the remainder, so `ExactFixedPoint` never
    /// drifts (architecture §1.3).
    pub const fn take_whole(&mut self) -> u32 {
        let whole = self.integer_part();
        self.0 &= Q32_32_ONE - 1;
        whole
    }
}

/// `numerator / denominator` in Q32.32 bits, exact via a `u128` intermediate and
/// saturating at `u64::MAX`. A zero `denominator` saturates rather than trapping.
const fn q32_32_bits_from_ratio(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    let scaled = (numerator as u128) << Q32_32_FRACTIONAL_BITS;
    let quotient = scaled / denominator as u128;
    if quotient > u64::MAX as u128 { u64::MAX } else { quotient as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_ratio_maps_tracker_volume_range() {
        assert_eq!(unit_from_ratio(0, 64), U0F16::ZERO);
        assert_eq!(unit_from_ratio(64, 64), U0F16::MAX);
        assert_eq!(unit_from_ratio(32, 64), U0F16::from_bits(32768));
        assert_eq!(unit_from_ratio(200, 64), U0F16::MAX, "numerator clamps to the denominator");
        assert_eq!(unit_from_ratio(1, 0), U0F16::ZERO, "a zero denominator must not panic");
    }

    #[test]
    fn unit_from_midi7_spans_the_full_range() {
        assert_eq!(unit_from_midi7(0), U0F16::ZERO);
        assert_eq!(unit_from_midi7(127), U0F16::MAX);
        for value in 0..=127u8 {
            let previous = unit_from_midi7(value.saturating_sub(1));
            assert!(unit_from_midi7(value) >= previous, "midi7 conversion must be monotonic at {value}");
        }
    }

    #[test]
    fn bipolar_ratio_is_symmetric_and_clamped() {
        assert_eq!(bipolar_from_ratio(0, 8), I1F15::ZERO);
        assert_eq!(bipolar_from_ratio(8, 8), I1F15::MAX);
        assert_eq!(bipolar_from_ratio(-8, 8), -I1F15::MAX, "the range is symmetric: -32767, not I1F15::MIN");
        assert_ne!(bipolar_from_ratio(-8, 8), I1F15::MIN);
        assert_eq!(bipolar_from_ratio(4, 8), I1F15::from_bits(16384));
        assert_eq!(bipolar_from_ratio(-4, 8), I1F15::from_bits(-16384));
        assert_eq!(bipolar_from_ratio(99, 8), I1F15::MAX, "numerator clamps to the denominator");
        assert_eq!(bipolar_from_ratio(-99, 8), -I1F15::MAX, "numerator clamps to the denominator");
        assert_eq!(bipolar_from_ratio(1, 0), I1F15::ZERO, "a zero denominator must not panic");
        assert_eq!(bipolar_from_ratio(1, -2), bipolar_from_ratio(-1, 2), "a negative denominator flips the sign");
    }

    #[test]
    fn midi_bend_centre_is_zero() {
        assert_eq!(bipolar_from_midi_bend(8192), I1F15::ZERO);
        // The 14-bit range is asymmetric about its centre: +8191 against −8192, so the
        // top of the range is one step short of full scale.
        assert_eq!(bipolar_from_midi_bend(16383), bipolar_from_ratio(8191, 8192));
        assert_eq!(bipolar_from_midi_bend(0), -I1F15::MAX);
        assert!(bipolar_from_midi_bend(0) < I1F15::ZERO);
    }

    #[test]
    fn step_from_ratio_is_exact() {
        assert_eq!(Step::from_ratio(1, 1), Step::ONE);
        assert_eq!(Step::from_ratio(1, 2), Step(1 << 31));
        // 8363 Hz sample played at 44100 Hz: 8363 / 44100 in Q32.32.
        let expected = ((8363u128 << 32) / 44100) as u64;
        assert_eq!(Step::from_ratio(8363, 44100), Step(expected));
    }

    #[test]
    fn step_saturating_ops_do_not_wrap() {
        assert_eq!(Step::MAX.saturating_add(Step::ONE), Step::MAX);
        assert_eq!(Step::ZERO.saturating_sub(Step::ONE), Step::ZERO);
        assert_eq!(Step::MAX.saturating_mul_int(2), Step::MAX);
        assert_eq!(Step::MAX.saturating_div_int(0), Step::MAX, "a zero divisor saturates rather than trapping");
        assert_eq!(Step::from_ratio(1, 0), Step::MAX, "a zero denominator saturates rather than trapping");
    }

    #[test]
    fn q32_32_saturating_ops_do_not_wrap() {
        assert_eq!(Q32_32::MAX.saturating_add(Q32_32::ONE), Q32_32::MAX);
        assert_eq!(Q32_32::ZERO.saturating_sub(Q32_32::ONE), Q32_32::ZERO);
        assert_eq!(Q32_32::MAX.saturating_mul_int(7), Q32_32::MAX);
        assert_eq!(Q32_32::from_integer(u32::MAX), Q32_32((u32::MAX as u64) << 32));
        assert_eq!(Q32_32::from_ratio(1, 0), Q32_32::MAX);
    }

    #[test]
    fn unit_scalar_saturating_ops_do_not_wrap() {
        assert_eq!(U0F16::MAX.saturating_add(U0F16::MAX), U0F16::MAX);
        assert_eq!(U0F16::ZERO.saturating_sub(U0F16::MAX), U0F16::ZERO);
        assert_eq!(I1F15::MAX.saturating_add(I1F15::MAX), I1F15::MAX);
        assert_eq!(I1F15::MIN.saturating_sub(I1F15::MAX), I1F15::MIN);
        assert_eq!(I1F15::MIN.saturating_mul(I1F15::MIN), I1F15::MAX, "(-1) * (-1) saturates to just under +1");
    }

    #[test]
    fn take_whole_carries_the_remainder() {
        let mut accumulator = Q32_32::from_bits((3 << 32) | 0x8000_0000);
        assert_eq!(accumulator.take_whole(), 3);
        assert_eq!(accumulator, Q32_32::from_bits(0x8000_0000));
        assert_eq!(accumulator.take_whole(), 0, "a fraction alone yields no whole units");
        assert_eq!(accumulator, Q32_32::from_bits(0x8000_0000));
    }
}

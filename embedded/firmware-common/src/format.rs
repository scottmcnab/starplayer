//! `core::fmt` helpers for logging paths that must not allocate.
//!
//! Three adapters, each a zero-sized-ish wrapper with a [`Display`] implementation, so a
//! board writes `info!("{}", Hex(&digest))` and no `alloc::format!` ever appears on a
//! logging path. `esp_println`'s macros take `core::fmt::Arguments`, so a `Display`
//! implementation is all any of them needs.

use core::fmt::{Display, Formatter, Result};

/// A byte string as lower-case hex, no separators.
///
/// The SHA-256 of a golden render is printed through this, and the 64 characters it
/// produces are compared character for character against `goldens/<format>/<name>.sha256`
/// — which stores exactly this spelling.
pub struct Hex<'a>(pub &'a [u8]);

impl Display for Hex<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// A byte count as `N.N KiB`, one decimal place, truncated rather than rounded.
///
/// For heap and image figures in a boot log, where the digit that matters is the
/// kilobyte and a reader who wants the exact byte count can read the raw number printed
/// beside it.
pub struct Kib(pub usize);

impl Display for Kib {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        let tenths = self.0.saturating_mul(10) / 1024;
        write!(formatter, "{}.{} KiB", tenths / 10, tenths % 10)
    }
}

/// `numerator / denominator` as a percentage with two decimal places.
///
/// The budget document's "% of one core" column: a voice that costs 1 764 000 cycles a
/// second on a 240 MHz core is `0.73%`, and two decimals is the resolution at which one
/// voice is still visible. A zero denominator prints `n/a` rather than dividing.
pub struct Percent {
    /// The part.
    pub numerator: u64,
    /// The whole.
    pub denominator: u64,
}

impl Display for Percent {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        if self.denominator == 0 {
            return write!(formatter, "n/a");
        }
        // Scaled by 10 000 rather than 100 so the two decimal places survive the integer
        // division; `u128` because a cycle count times 10 000 overflows `u64` only above
        // 1.8e15, but the same expression is used for byte counts and there is no reason
        // to leave the trap armed.
        let hundredths = (self.numerator as u128).saturating_mul(10_000) / (self.denominator as u128);
        write!(formatter, "{}.{:02}%", hundredths / 100, hundredths % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn hex_prints_lower_case_pairs_with_leading_zeroes() {
        assert_eq!(format!("{}", Hex(&[0x00, 0x0f, 0xa5, 0xff])), "000fa5ff");
    }

    #[test]
    fn an_empty_byte_string_prints_nothing() { assert_eq!(format!("{}", Hex(&[])), ""); }

    #[test]
    fn kib_truncates_to_one_decimal_place() {
        assert_eq!(format!("{}", Kib(0)), "0.0 KiB");
        assert_eq!(format!("{}", Kib(1024)), "1.0 KiB");
        assert_eq!(format!("{}", Kib(1536)), "1.5 KiB");
        // 71 576 is `PETRI.S3M`'s engine heap (I1 research point 3a).
        assert_eq!(format!("{}", Kib(71_576)), "69.8 KiB");
    }

    #[test]
    fn percent_keeps_two_decimal_places() {
        assert_eq!(format!("{}", Percent { numerator: 1, denominator: 2 }), "50.00%");
        assert_eq!(format!("{}", Percent { numerator: 1_764_000, denominator: 240_000_000 }), "0.73%");
        assert_eq!(format!("{}", Percent { numerator: 3, denominator: 0 }), "n/a");
    }
}

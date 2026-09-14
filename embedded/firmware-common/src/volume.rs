//! Fixed-point master-volume policy helpers for board control surfaces.
//!
//! A board chooses its own maximum and step. These helpers keep button and network paths on
//! the same saturating arithmetic without changing the engine's unity default.

use starplayer::core::U0F16;

/// Clamp a requested master volume to a board's maximum.
pub fn cap_master_volume(requested: U0F16, maximum: U0F16) -> U0F16 {
    U0F16::from_bits(requested.to_bits().min(maximum.to_bits()))
}

/// Raise master volume by one step without exceeding the board's maximum.
pub fn raise_master_volume(current: U0F16, step: U0F16, maximum: U0F16) -> U0F16 {
    cap_master_volume(U0F16::from_bits(current.to_bits().saturating_add(step.to_bits())), maximum)
}

/// Lower master volume by one step, first bringing an out-of-policy current value under the
/// board's maximum.
pub fn lower_master_volume(current: U0F16, step: U0F16, maximum: U0F16) -> U0F16 {
    let current = cap_master_volume(current, maximum);
    U0F16::from_bits(current.to_bits().saturating_sub(step.to_bits()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A1S_MAXIMUM: U0F16 = U0F16::from_bits(16_384);
    const A1S_STEP: U0F16 = U0F16::from_bits(1_024);

    #[test]
    fn a1s_cap_accepts_quarter_scale_and_clamps_every_louder_request() {
        assert_eq!(cap_master_volume(U0F16::from_bits(16_383), A1S_MAXIMUM).to_bits(), 16_383);
        assert_eq!(cap_master_volume(A1S_MAXIMUM, A1S_MAXIMUM), A1S_MAXIMUM);
        assert_eq!(cap_master_volume(U0F16::MAX, A1S_MAXIMUM), A1S_MAXIMUM);
    }

    #[test]
    fn a1s_step_is_exact_and_saturates_at_both_policy_limits() {
        assert_eq!(raise_master_volume(U0F16::from_bits(15_360), A1S_STEP, A1S_MAXIMUM), A1S_MAXIMUM);
        assert_eq!(raise_master_volume(A1S_MAXIMUM, A1S_STEP, A1S_MAXIMUM), A1S_MAXIMUM);
        assert_eq!(lower_master_volume(A1S_MAXIMUM, A1S_STEP, A1S_MAXIMUM).to_bits(), 15_360);
        assert_eq!(lower_master_volume(U0F16::from_bits(0), A1S_STEP, A1S_MAXIMUM).to_bits(), 0);
        assert_eq!(lower_master_volume(U0F16::MAX, A1S_STEP, A1S_MAXIMUM).to_bits(), 15_360);
    }
}

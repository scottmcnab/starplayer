//! The mixer's vector kernels, and the scalar bodies that specify them (M7-H6).
//!
//! Two operations run over a whole quantum with no state and no branching, which is
//! exactly the shape a vector unit wants: the **channel-major bus summation**
//! ([`MixPath::add_block`](crate::path::MixPath::add_block), one call per bus per quantum)
//! and the **master volume** multiply. Everything else on the master bus — the limiter's
//! table lookup and its interpolation — stays scalar; it is a gather, not arithmetic.
//!
//! The rules are `starplayer_dsp::simd`'s, restated where they bite here:
//!
//! * Each kernel exists twice, and the `scalar_*` body is compiled into **every** build.
//!   `tests/simd_equivalence.rs` runs both in one binary and compares bit patterns.
//! * The float path is vectorised throughout. On the fixed path only the bus summation is:
//!   `wide` has `i32x4::saturating_add`, which is `FixedPath::add_frame` exactly, while the
//!   master volume is a widening multiply and a rounding narrowing back to `i32`, which
//!   `wide` cannot finish inside a vector.
//! * A frame is a `Stereo`, not a bare sample, so two frames fill one four-lane vector. The
//!   loops therefore step in pairs and finish any odd last frame scalar — a quantum is 128
//!   frames and never has one, but the functions take a slice and must be right for any.

use starplayer_dsp::round_shift_nearest;

use crate::path::Stereo;

#[cfg(feature = "simd")]
use wide::{f32x4, i32x4};

/// Add one float bus into another, frame by frame — `FloatPath::add_frame` over a block.
pub fn scalar_add_block_f32(destination: &mut [Stereo<f32>], source: &[Stereo<f32>]) {
    for (mixed, mine) in destination.iter_mut().zip(source.iter()) {
        mixed.left += mine.left;
        mixed.right += mine.right;
    }
}

/// Add one fixed bus into another — `FixedPath::add_frame` over a block, saturating.
pub fn scalar_add_block_i32(destination: &mut [Stereo<i32>], source: &[Stereo<i32>]) {
    for (mixed, mine) in destination.iter_mut().zip(source.iter()) {
        mixed.left = mixed.left.saturating_add(mine.left);
        mixed.right = mixed.right.saturating_add(mine.right);
    }
}

/// [`scalar_add_block_f32`], two frames at a time.
#[cfg(feature = "simd")]
pub fn wide_add_block_f32(destination: &mut [Stereo<f32>], source: &[Stereo<f32>]) {
    let paired = destination.len().min(source.len()) & !1;
    let (Some(head), Some(mine)) = (destination.get_mut(..paired), source.get(..paired)) else { return };
    for (frames, added) in head.chunks_exact_mut(2).zip(mine.chunks_exact(2)) {
        let (Some(first), Some(second), Some(third), Some(fourth)) = (frames.first().copied(), frames.get(1).copied(), added.first().copied(), added.get(1).copied()) else {
            continue;
        };
        let sum = f32x4::new([first.left, first.right, second.left, second.right]) + f32x4::new([third.left, third.right, fourth.left, fourth.right]);
        let [left, right, next_left, next_right] = sum.to_array();
        if let Some(slot) = frames.first_mut() {
            *slot = Stereo::new(left, right);
        }
        if let Some(slot) = frames.get_mut(1) {
            *slot = Stereo::new(next_left, next_right);
        }
    }
    if let (Some(tail), Some(mine)) = (destination.get_mut(paired..), source.get(paired..)) {
        scalar_add_block_f32(tail, mine);
    }
}

/// [`scalar_add_block_i32`], two frames at a time.
///
/// `i32x4::saturating_add` is `i32::saturating_add` per lane, which is the whole of
/// `FixedPath::add_frame` — the one fixed-path operation `wide` expresses exactly.
#[cfg(feature = "simd")]
pub fn wide_add_block_i32(destination: &mut [Stereo<i32>], source: &[Stereo<i32>]) {
    let paired = destination.len().min(source.len()) & !1;
    let (Some(head), Some(mine)) = (destination.get_mut(..paired), source.get(..paired)) else { return };
    for (frames, added) in head.chunks_exact_mut(2).zip(mine.chunks_exact(2)) {
        let (Some(first), Some(second), Some(third), Some(fourth)) = (frames.first().copied(), frames.get(1).copied(), added.first().copied(), added.get(1).copied()) else {
            continue;
        };
        let sum = i32x4::new([first.left, first.right, second.left, second.right])
            .saturating_add(i32x4::new([third.left, third.right, fourth.left, fourth.right]));
        let [left, right, next_left, next_right] = sum.to_array();
        if let Some(slot) = frames.first_mut() {
            *slot = Stereo::new(left, right);
        }
        if let Some(slot) = frames.get_mut(1) {
            *slot = Stereo::new(next_left, next_right);
        }
    }
    if let (Some(tail), Some(mine)) = (destination.get_mut(paired..), source.get(paired..)) {
        scalar_add_block_i32(tail, mine);
    }
}

/// Master volume on a whole float quantum, before the limiter.
pub fn scalar_master_volume_f32(quantum: &mut [Stereo<f32>], volume: f32) {
    for frame in quantum.iter_mut() {
        frame.left *= volume;
        frame.right *= volume;
    }
}

/// [`scalar_master_volume_f32`], two frames at a time.
#[cfg(feature = "simd")]
pub fn wide_master_volume_f32(quantum: &mut [Stereo<f32>], volume: f32) {
    let paired = quantum.len() & !1;
    let gain = f32x4::splat(volume);
    if let Some(head) = quantum.get_mut(..paired) {
        for frames in head.chunks_exact_mut(2) {
            let (Some(first), Some(second)) = (frames.first().copied(), frames.get(1).copied()) else { continue };
            let scaled = f32x4::new([first.left, first.right, second.left, second.right]) * gain;
            let [left, right, next_left, next_right] = scaled.to_array();
            if let Some(slot) = frames.first_mut() {
                *slot = Stereo::new(left, right);
            }
            if let Some(slot) = frames.get_mut(1) {
                *slot = Stereo::new(next_left, next_right);
            }
        }
    }
    if let Some(tail) = quantum.get_mut(paired..) {
        scalar_master_volume_f32(tail, volume);
    }
}

/// Master volume on a whole fixed quantum, before the limiter.
///
/// Q0.16, reduced with the round-to-nearest-ties-away rule the C6 golden contract makes
/// part of every fixed-path precision reduction. The result always fits `i32` — the volume
/// is at most `65535/65536` of unity, so the scaled magnitude never exceeds the input's —
/// which is what lets the volume and the limiter be two passes over the quantum rather
/// than one fused expression.
///
/// Scalar, and staying scalar: `wide` widens (`i32x4::widening_mul`) but cannot narrow an
/// `i64x4` back, so the rounding shift would have to leave the vector through memory for
/// every frame.
pub fn scalar_master_volume_fixed(quantum: &mut [Stereo<i32>], volume: i64) {
    for frame in quantum.iter_mut() {
        frame.left = round_shift_nearest(frame.left as i64 * volume, 16) as i32;
        frame.right = round_shift_nearest(frame.right as i64 * volume, 16) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property `master_block_fixed` relies on to keep the volume and the limiter as
    /// two passes: a Q0.16 volume is below unity, so the scaled magnitude never exceeds
    /// the input's and the `i64` product always narrows back into `i32` losslessly.
    #[test]
    fn the_fixed_master_volume_never_leaves_i32() {
        let mut quantum = [Stereo::new(i32::MAX, i32::MIN), Stereo::new(0, 1)];
        scalar_master_volume_fixed(&mut quantum, 65_535);
        assert_eq!(quantum.first().copied(), Some(Stereo::new(2_147_450_879, -2_147_450_880)));
        assert_eq!(quantum.get(1).copied(), Some(Stereo::new(0, 1)));
    }

    #[test]
    fn full_master_volume_is_the_identity_to_within_its_own_rounding() {
        let mut quantum = [Stereo::new(20_000, -20_000)];
        scalar_master_volume_fixed(&mut quantum, 65_535);
        assert_eq!(quantum.first().copied(), Some(Stereo::new(20_000, -20_000)));
    }

    #[test]
    fn an_odd_length_block_still_sums_every_frame() {
        let mut destination = [Stereo::new(1i32, 2), Stereo::new(3, 4), Stereo::new(5, 6)];
        scalar_add_block_i32(&mut destination, &[Stereo::new(10, 20), Stereo::new(30, 40), Stereo::new(50, 60)]);
        assert_eq!(destination, [Stereo::new(11, 22), Stereo::new(33, 44), Stereo::new(55, 66)]);
    }
}

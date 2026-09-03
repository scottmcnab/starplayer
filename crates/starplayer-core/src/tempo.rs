//! [`TempoModel`] — how many output frames a tracker tick lasts.
//!
//! Tick length is a **policy, not a constant** (architecture §1.3). The original's
//! `SetSBTempo` truncates twice and is audibly, measurably wrong; both that behaviour and
//! the exact one are wanted, so the choice is a trait with three implementations from the
//! start. It is one of only two traits committed before a second implementation exists
//! (architecture §10.1) — it has three.

use crate::fixed::Q32_32;

/// The lowest tempo the models will divide by. `Txx` clamps to 32 BPM in ST3
/// (`STARPLAY/S3MLIB.ASM` ~3130) but a corrupt module can still reach these functions
/// with zero, and nothing in the RT path may panic — so a zero tempo is treated as 1 BPM
/// rather than dividing by zero.
const MINIMUM_DIVISOR_BPM: u32 = 1;

/// How long one tracker tick lasts, in output frames.
///
/// # Units
///
/// The return value is **Q32.32**: whole output frames in the high 32 bits, the fraction
/// in the low 32. The fraction is what stops [`ExactFixedPoint`] from drifting — the
/// caller accumulates it and takes whole frames out as they appear (see
/// [`crate::clock::FrameClock`]).
///
/// # The `speed` parameter
///
/// `speed` is accepted by every implementation and **used by none of them**. Ticks per
/// row is the row clock's concern, not the tempo model's: a tick is the same length
/// whether a row lasts six of them or one. It is in the signature because IT's tempo
/// slides (`Txx` with `x` in the high nibble) are evaluated per tick and a future model
/// may want to know where in the row it is; implementing that is M6's job.
pub trait TempoModel {
    /// Frames per tick, Q32.32.
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> u64;
}

/// The default: `sample_rate * 2.5 / bpm`, exact in Q32.32.
///
/// A tracker tick is `bpm * 2 / 5` ticks per second
/// (`plans/reference/original-s3mlib-analysis.md` §6), so a tick is `rate * 5 / (2 * bpm)`
/// frames. Computed with a `u128` intermediate so `(rate << 32) * 5` cannot overflow, and
/// saturated back into `u64`.
///
/// At 44100 Hz and 130 BPM the true value is 848.0769… frames. The Q32.32 result is
/// within one part in 2^32 of that, and because [`crate::clock::FrameClock`] carries the
/// remainder, the accumulated position never drifts from the closed form.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExactFixedPoint;

impl TempoModel for ExactFixedPoint {
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, _speed: u8) -> u64 {
        exact_frames_per_tick(sample_rate_hz, tempo_bpm)
    }
}

/// The original's tick length, double truncation and all: `(rate * 10 / bpm) >> 2`.
///
/// `SetSBTempo` (`STARPLAY/S3MLIB.ASM` ~5621) is exactly this:
///
/// ```text
/// movzx eax,[__MixingRate]   ; eax = rate
/// mov edx,10 / mul edx       ; eax = rate * 10
/// movzx ecx,[ebx+_MCurrentBPM]
/// xor edx,edx / div ecx      ; eax = rate * 10 / bpm      <- first truncation
/// shr eax,2                  ; eax = that >> 2            <- second truncation
/// mov [_SB_GapLength],eax
/// ```
///
/// At 44100 Hz and 130 BPM that yields 848 where the true value is 848.077 — about 1.3
/// seconds of drift over a four-minute song. The result has no fractional part, so
/// [`crate::clock::FrameClock`] has nothing to carry and the drift is reproduced faithfully.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct St3Truncating;

impl TempoModel for St3Truncating {
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, _speed: u8) -> u64 {
        let divisor = (tempo_bpm as u32).max(MINIMUM_DIVISOR_BPM) as u64;
        let whole_frames = (sample_rate_hz as u64 * 10 / divisor) >> 2;
        // Shifting into Q32.32 can overflow for an absurd sample rate; saturate instead.
        whole_frames.saturating_mul(1u64 << 32)
    }
}

/// Impulse Tracker's tick length: `rate * 5 / (2 * bpm)`, truncated to a **whole output
/// frame** (accuracy policy §2, task G3 research point 4).
///
/// Impulse Tracker's own driver reloads a whole-sample gap length every tick, and so does
/// every replayer measured against it — libxmp truncates the same expression to an `int`
/// (`src/mixer.c:440`, `ticksize = (int)calc`) and OpenMPT's classic path does the same.
/// The truncated remainder is **not** carried, so at a tempo whose exact tick length is
/// fractional the tick is consistently a little short.
///
/// This is not the double truncation [`St3Truncating`] reproduces: there is one division
/// and one truncation, so at 44100 Hz the two agree at every tempo whose quotient is a
/// whole number and differ by up to three frames elsewhere.
///
/// It is deliberately **not** [`ExactFixedPoint`]. The project's default tempo model is
/// drift-free because a drifting clock is a defect in a *modern* player, but the drift is
/// observable in IT's own output: a voice's sample position after N ticks is
/// `step · Σ frames_per_tick`, and the pinned corpus's `data/*.it` and `openmpt/it`
/// fixtures compare that position against libxmp frame by frame. Sixty-six of the 121 IT
/// cases diverge on `position` under [`ExactFixedPoint`] and agree under this model, which
/// is what settles it: for IT the exact clock is not a more accurate reading of the same
/// behaviour, it is a different behaviour.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ItModern;

impl TempoModel for ItModern {
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, _speed: u8) -> u64 {
        let divisor = (tempo_bpm as u32).max(MINIMUM_DIVISOR_BPM) as u64;
        let whole_frames = (sample_rate_hz as u64 * 5) / (2 * divisor);
        whole_frames.saturating_mul(1u64 << 32)
    }
}

/// `sample_rate * 5 / (2 * bpm)` in Q32.32, exact and saturating.
fn exact_frames_per_tick(sample_rate_hz: u32, tempo_bpm: u16) -> u64 {
    let divisor = (tempo_bpm as u32).max(MINIMUM_DIVISOR_BPM) as u128;
    let numerator = ((sample_rate_hz as u128) << 32) * 5;
    let quotient = numerator / (2 * divisor);
    if quotient > u64::MAX as u128 { u64::MAX } else { quotient as u64 }
}

/// Which [`TempoModel`] the engine should use, as carried by
/// [`crate::event::Command::SetTempoModel`].
///
/// The models themselves are zero-sized types selected by a generic parameter on the
/// clock, so the control plane needs a plain data tag rather than a trait object.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum TempoModelId {
    /// [`ExactFixedPoint`].
    #[default]
    ExactFixedPoint,
    /// [`St3Truncating`].
    St3Truncating,
    /// [`ItModern`].
    ItModern,
}

impl TempoModel for TempoModelId {
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> u64 {
        match self {
            TempoModelId::ExactFixedPoint => ExactFixedPoint.frames_per_tick(sample_rate_hz, tempo_bpm, speed),
            TempoModelId::St3Truncating => St3Truncating.frames_per_tick(sample_rate_hz, tempo_bpm, speed),
            TempoModelId::ItModern => ItModern.frames_per_tick(sample_rate_hz, tempo_bpm, speed),
        }
    }
}

/// Convenience: the tick length as a [`Q32_32`].
pub fn frames_per_tick_fixed<Tempo: TempoModel + ?Sized>(tempo_model: &Tempo, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> Q32_32 {
    Q32_32::from_bits(tempo_model.frames_per_tick(sample_rate_hz, tempo_bpm, speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn st3_truncating_reproduces_the_original_gap_length() {
        // SetSBTempo: (44100 * 10 / 130) >> 2 == 3392 >> 2 == 848.
        let frames = St3Truncating.frames_per_tick(44100, 130, 6);
        assert_eq!(frames >> 32, 848, "the original's double truncation yields 848 frames per tick");
        assert_eq!(frames as u32, 0, "the truncating model has no fractional part to carry");
    }

    #[test]
    fn st3_truncating_matches_the_assembly_at_other_rates() {
        for (rate, bpm) in [(44100u32, 125u16), (22050, 125), (48000, 130), (11025, 33)] {
            let expected = ((rate as u64 * 10 / bpm as u64) >> 2) << 32;
            assert_eq!(St3Truncating.frames_per_tick(rate, bpm, 6), expected, "rate {rate}, bpm {bpm}");
        }
    }

    #[test]
    fn exact_fixed_point_is_the_true_tick_length() {
        let frames = ExactFixedPoint.frames_per_tick(44100, 130, 6);
        assert_eq!(frames >> 32, 848, "44100 * 2.5 / 130 == 848.0769…");
        assert!(frames as u32 > 0, "the exact model must carry a fraction");
        // Within one Q32.32 ulp of the closed form.
        let closed_form = ((44100u128 << 32) * 5) / 260;
        assert_eq!(frames as u128, closed_form);
    }

    #[test]
    fn it_modern_is_currently_exact_fixed_point() {
        for bpm in [32u16, 125, 130, 255] {
            assert_eq!(
                ItModern.frames_per_tick(44100, bpm, 6),
                (44100u64 * 5 / (2 * bpm.max(1) as u64)) << 32,
                "bpm {bpm}: Impulse Tracker truncates its tick length to a whole output frame",
            );
        }
    }

    #[test]
    fn speed_is_ignored_by_every_model() {
        for speed in [0u8, 1, 6, 31, 255] {
            assert_eq!(ExactFixedPoint.frames_per_tick(44100, 130, speed), ExactFixedPoint.frames_per_tick(44100, 130, 6));
            assert_eq!(St3Truncating.frames_per_tick(44100, 130, speed), St3Truncating.frames_per_tick(44100, 130, 6));
            assert_eq!(ItModern.frames_per_tick(44100, 130, speed), ItModern.frames_per_tick(44100, 130, 6));
        }
    }

    #[test]
    fn a_zero_tempo_does_not_divide_by_zero() {
        assert_eq!(ExactFixedPoint.frames_per_tick(44100, 0, 6), ExactFixedPoint.frames_per_tick(44100, 1, 6));
        assert_eq!(St3Truncating.frames_per_tick(44100, 0, 6), St3Truncating.frames_per_tick(44100, 1, 6));
    }

    #[test]
    fn an_absurd_sample_rate_saturates_rather_than_overflowing() {
        assert_eq!(ExactFixedPoint.frames_per_tick(u32::MAX, 1, 6), u64::MAX);
        assert_eq!(St3Truncating.frames_per_tick(u32::MAX, 1, 6), u64::MAX);
    }

    #[test]
    fn tempo_model_id_dispatches_to_the_right_model() {
        assert_eq!(TempoModelId::default(), TempoModelId::ExactFixedPoint);
        assert_eq!(TempoModelId::St3Truncating.frames_per_tick(44100, 130, 6), St3Truncating.frames_per_tick(44100, 130, 6));
        assert_eq!(TempoModelId::ExactFixedPoint.frames_per_tick(44100, 130, 6), ExactFixedPoint.frames_per_tick(44100, 130, 6));
        assert_eq!(frames_per_tick_fixed(&TempoModelId::ItModern, 44100, 130, 6).to_bits(), ItModern.frames_per_tick(44100, 130, 6));
    }
}

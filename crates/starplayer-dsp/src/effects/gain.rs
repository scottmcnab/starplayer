//! [`GainInsert`] — a smoothed gain trim, and the [`Insert`] trait's first implementation.
//!
//! One parameter, in centi-decibels. It exists to be the reference for how an effect is
//! written: it holds a [`SmoothedParam`], reads `current()` per frame while the parameter
//! is moving, and hoists the conversion out of the loop when it is not — exactly the
//! discipline `crate::ramp` describes for the mixer's voice gains, and the reason an
//! automated gain change cannot click.
//!
//! # −60 dB is −∞
//!
//! [`GAIN_MIN_CENTI_DB`] is the bottom of the fader and resolves to a gain of exactly
//! zero, not to −60 dB. A host that wants a channel silent has somewhere to put the
//! slider, and the routing tests have a value that proves a bus really is muted.
//!
//! # Unity is bit-transparent
//!
//! `0` centi-decibels is [`Q15_UNITY`], and `DspSample::scale_q15` at unity is the
//! identity on both paths. An insert sitting at its default therefore leaves the bus
//! bit-identical, on the fixed path *and* the float one.

use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::sample::DspSample;
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};
use crate::frame::Stereo;

/// The bottom of the fader, in centi-decibels. Resolves to silence rather than to −60 dB.
pub const GAIN_MIN_CENTI_DB: i32 = -6_000;

/// The top of the fader, in centi-decibels: +12 dB.
pub const GAIN_MAX_CENTI_DB: i32 = 1_200;

/// The one parameter [`GainInsert`] has.
pub const GAIN_PARAM: ParamId = ParamId(0);

/// What a host draws for a [`GainInsert`].
pub static GAIN_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "gain",
    params: &[ParamSpec {
        name: "gain",
        unit: ParamUnit::CentiDecibels,
        min: GAIN_MIN_CENTI_DB,
        max: GAIN_MAX_CENTI_DB,
        default: 0,
    }],
};

/// A smoothed gain trim.
///
/// Not generic over the sample type: the body is the same arithmetic on both paths, so
/// `impl<Sample: DspSample> Insert<Sample> for GainInsert` covers `f32` and `i32` at once
/// and a host can build one without naming a path.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GainInsert {
    gain: SmoothedParam,
}

impl GainInsert {
    /// A gain insert at unity.
    pub const fn new() -> GainInsert { GainInsert::at(0) }

    /// A gain insert already at `centi_db`, with no ramp to run.
    pub const fn at(centi_db: i32) -> GainInsert {
        GainInsert { gain: SmoothedParam::steady(clamp_centi_db(centi_db)) }
    }

    /// Where the gain is heading, in centi-decibels.
    pub const fn centi_db(&self) -> i32 { self.gain.target() }
}

/// `GAIN_DESCRIPTOR.params[0].clamp`, spelled out so [`GainInsert::at`] can be a
/// `const fn`: reading a `static` slice is not something a `const fn` may do.
const fn clamp_centi_db(centi_db: i32) -> i32 {
    if centi_db < GAIN_MIN_CENTI_DB {
        GAIN_MIN_CENTI_DB
    } else if centi_db > GAIN_MAX_CENTI_DB {
        GAIN_MAX_CENTI_DB
    } else {
        centi_db
    }
}

impl<Sample: DspSample> Insert<Sample> for GainInsert {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        if self.gain.is_moving() {
            for frame in block.iter_mut() {
                let gain = fader_gain_q15(self.gain.advance());
                frame.left = frame.left.scale_q15(gain);
                frame.right = frame.right.scale_q15(gain);
            }
        } else {
            // Hoisted, exactly as the mixer's kernel hoists a gain that is not ramping.
            // A ramp that has just landed produces the frame this branch would have, so
            // splitting a ramp across the hoist boundary changes nothing.
            let gain = fader_gain_q15(self.gain.current());
            for frame in block.iter_mut() {
                frame.left = frame.left.scale_q15(gain);
                frame.right = frame.right.scale_q15(gain);
            }
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        if id == GAIN_PARAM {
            self.gain.set_target(clamp_centi_db(value), SMOOTH_FRAMES);
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        if id == GAIN_PARAM { Some(self.gain.target()) } else { None }
    }

    fn reset(&mut self) { self.gain.snap(); }

    fn descriptor(&self) -> &'static InsertDescriptor { &GAIN_DESCRIPTOR }
}

/// The fader's centi-decibel to Q1.15 gain curve: [`crate::tables::db_to_gain_q15`] with a
/// floor at [`GAIN_MIN_CENTI_DB`] and a ceiling at [`GAIN_MAX_CENTI_DB`].
///
/// H1 carried its own 73-entry whole-decibel table here, with the comment that it was H2's
/// `db_to_gain_q15` living in advance of H2 landing. H2 landed it, so this now calls
/// through and the duplicate table is gone. Two things had to be checked before the switch
/// and both hold:
///
/// * **Unity is still bit-exact.** `fader_gain_q15(0)` is 32768 in the table version and
///   32768 through [`crate::tables::pow2_q24`], so a gain insert at its default still
///   leaves a bus bit-identical on both paths — the property the whole insert graph's
///   "buses are always on" argument rests on. `+12 dB` is 130452 either way as well.
/// * **The bottom of the fader is still silence**, not −60 dB.
///   [`crate::tables::db_to_gain_q15`] has no notion of an −∞ fader position — its own floor
///   is −96 dB, where it returns 33 — so the early return below is what keeps
///   [`GAIN_MIN_CENTI_DB`] meaning "muted".
///
/// What did move: at a *fractional* decibel the two disagree by up to 203 in Q1.15 (0.6 %),
/// because the table version interpolated a chord under the exponential between whole
/// decibels and this one evaluates the exponential itself. Nothing pins those values — no
/// golden installs an insert — and the new numbers are the more accurate ones.
pub fn fader_gain_q15(centi_db: i32) -> i32 {
    if centi_db <= GAIN_MIN_CENTI_DB {
        return 0;
    }
    crate::tables::db_to_gain_q15(centi_db.min(GAIN_MAX_CENTI_DB))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Q15_UNITY;
    use alloc::boxed::Box;
    use alloc::vec;

    fn block<Sample: DspSample>(value: Sample) -> alloc::vec::Vec<Stereo<Sample>> {
        vec![Stereo::new(value, value); DSP_BLOCK_FRAMES]
    }

    #[test]
    fn the_bottom_of_the_fader_is_silence_not_minus_sixty_decibels() {
        assert_eq!(fader_gain_q15(GAIN_MIN_CENTI_DB), 0);
        assert_eq!(fader_gain_q15(GAIN_MIN_CENTI_DB - 1), 0);
        assert!(fader_gain_q15(GAIN_MIN_CENTI_DB + 1) > 0, "one centi-decibel up is audible again");
    }

    #[test]
    fn whole_decibels_are_the_tables_own_numbers() {
        assert_eq!(fader_gain_q15(0), Q15_UNITY, "unity has to be bit-exact, or an insert at its default is not transparent");
        assert_eq!(fader_gain_q15(-600), crate::tables::db_to_gain_q15(-600));
        assert_eq!(fader_gain_q15(1_200), 130_452);
        assert_eq!(fader_gain_q15(9_000), 130_452, "past the top of the fader is the top of the fader");
    }

    #[test]
    fn a_centi_decibel_remainder_interpolates_between_two_entries() {
        let half = fader_gain_q15(-50);
        assert!(half > fader_gain_q15(-100) && half < fader_gain_q15(0), "half a decibel down sits between its neighbours");
        // −0.5 dB is 0.94406 linear, which is 30,935 in Q1.15.
        assert!((half - 30_935).abs() <= 2, "half a decibel down came out at {half}");
    }

    #[test]
    fn a_fresh_insert_is_bit_transparent_on_both_paths() {
        let mut insert = GainInsert::new();
        let mut fixed = block(20_000i32);
        Insert::<i32>::process(&mut insert, &mut fixed);
        assert_eq!(fixed, block(20_000i32), "unity moved a fixed-path block");

        let mut insert = GainInsert::new();
        let mut float = block(0.25f32);
        Insert::<f32>::process(&mut insert, &mut float);
        assert_eq!(float, block(0.25f32), "unity moved a float-path block");
    }

    #[test]
    fn the_bottom_of_the_fader_silences_a_block_once_the_ramp_has_landed() {
        let mut insert = GainInsert::at(GAIN_MIN_CENTI_DB);
        let mut fixed = block(20_000i32);
        Insert::<i32>::process(&mut insert, &mut fixed);
        assert_eq!(fixed, block(0i32));
    }

    #[test]
    fn a_parameter_change_ramps_rather_than_jumping() {
        let mut insert = GainInsert::new();
        Insert::<i32>::set_param(&mut insert, GAIN_PARAM, GAIN_MIN_CENTI_DB);
        assert_eq!(Insert::<i32>::param(&insert, GAIN_PARAM), Some(GAIN_MIN_CENTI_DB));

        let mut first = block(20_000i32);
        Insert::<i32>::process(&mut insert, &mut first);
        assert!(first.first().expect("a block").left != 0, "the first frame has barely moved");
        assert!(first.last().expect("a block").left < 20_000, "and the block is on its way down");

        let mut second = block(20_000i32);
        Insert::<i32>::process(&mut insert, &mut second);
        assert_eq!(second.last().expect("a block"), &Stereo::new(0, 0), "two quanta is SMOOTH_FRAMES, so it has landed");
    }

    #[test]
    fn a_reset_lands_a_moving_parameter_on_its_target() {
        let mut insert = GainInsert::new();
        Insert::<i32>::set_param(&mut insert, GAIN_PARAM, GAIN_MIN_CENTI_DB);
        Insert::<i32>::reset(&mut insert);
        let mut fixed = block(20_000i32);
        Insert::<i32>::process(&mut insert, &mut fixed);
        assert_eq!(fixed, block(0i32), "a reset parameter is at its target from the first frame");
    }

    #[test]
    fn an_unknown_parameter_is_ignored_rather_than_panicking() {
        let mut insert = GainInsert::new();
        Insert::<f32>::set_param(&mut insert, ParamId(7), 500);
        assert_eq!(Insert::<f32>::param(&insert, ParamId(7)), None);
        assert_eq!(Insert::<f32>::param(&insert, GAIN_PARAM), Some(0));
    }

    #[test]
    fn the_descriptor_names_the_one_parameter() {
        let insert = GainInsert::new();
        let descriptor = Insert::<f32>::descriptor(&insert);
        assert_eq!(descriptor.name, "gain");
        assert_eq!(descriptor.params.len(), 1);
        assert_eq!(descriptor.params.first().expect("one parameter").unit, ParamUnit::CentiDecibels);
    }

    #[test]
    fn a_boxed_insert_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: Box<dyn Insert<f32>> = Box::new(GainInsert::new());
        assert_send(&boxed);
    }
}

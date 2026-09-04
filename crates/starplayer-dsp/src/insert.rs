//! [`Insert`] — one effect in a channel or master chain (M7 master-plan decisions 2–4).
//!
//! # The shape, and why it is this shape
//!
//! * **Whole blocks.** [`Insert::process`] is handed exactly [`DSP_BLOCK_FRAMES`] frames,
//!   always. Architecture §1.4 calls ragged per-block DSP the highest-probability silent
//!   failure in the design; an effect that can only ever see a whole quantum cannot have
//!   that bug. The engine asserts `RENDER_QUANTUM == DSP_BLOCK_FRAMES` at compile time.
//! * **Integer parameters in fixed units** ([`ParamUnit`]), so the fixed path never sees a
//!   float and a parameter crossing the control ring is a plain `i32`.
//! * **Generic over [`DspSample`]**, so one effect body exists on both mixing paths from
//!   the day it lands and the fixed one stays cross-target bit-identical.
//! * **`Send`**, because a host builds an effect — allocating its delay lines — on its own
//!   thread and sends the box to the audio thread over the engine's insert ring.
//!
//! Nothing here allocates. Building an effect allocates; [`Insert::set_param`] is a copy
//! and the start of a smoothing ramp, and [`Insert::reset`] clears state that is already
//! there.

use crate::frame::Stereo;
use crate::sample::DspSample;

/// Frames in one DSP block.
///
/// The engine's `RENDER_QUANTUM` must equal this; `starplayer-engine` asserts it at
/// compile time. An effect may rely on it: `block.len()` is this, every call.
pub const DSP_BLOCK_FRAMES: usize = 128;

/// Which parameter of an effect, by position in its [`InsertDescriptor::params`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId(pub u8);

/// What an effect parameter's integer value means.
///
/// Every unit is a fixed integer scale rather than a float, so the fixed path never
/// touches a float and a host's slider position survives the ring unchanged.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ParamUnit {
    /// Hundredths of a decibel. `0` is unity, `-600` is −6 dB.
    CentiDecibels,
    /// Output frames.
    Frames,
    /// Hundredths of a semitone.
    Cents,
    /// Whole percent, `0 ..= 100` unless the effect says otherwise.
    Percent,
    /// Whole milliseconds.
    Milliseconds,
    /// A ratio times 100: `400` is 4:1.
    Ratio,
    /// Off when zero, on otherwise.
    Switch,
}

/// One parameter of an effect: what it is called, what its integer means, and its range.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParamSpec {
    /// Short lower-case name, for a host's control surface.
    pub name: &'static str,
    /// What the integer means.
    pub unit: ParamUnit,
    /// Smallest accepted value. A value below it is clamped, never refused.
    pub min: i32,
    /// Largest accepted value.
    pub max: i32,
    /// What a freshly built effect holds.
    pub default: i32,
}

impl ParamSpec {
    /// `value` brought inside `[min, max]`.
    pub const fn clamp(&self, value: i32) -> i32 {
        if value < self.min {
            self.min
        } else if value > self.max {
            self.max
        } else {
            value
        }
    }
}

/// What an effect is, for a host that has to draw it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InsertDescriptor {
    /// Short lower-case name.
    pub name: &'static str,
    /// The parameters, in [`ParamId`] order.
    pub params: &'static [ParamSpec],
}

/// One effect in a channel or master insert chain.
pub trait Insert<Sample: DspSample>: Send {
    /// Process one whole block in place. `block.len() == DSP_BLOCK_FRAMES` always.
    fn process(&mut self, block: &mut [Stereo<Sample>]);

    /// Set one parameter. **Real-time safe**: a copy and the start of a smoothing ramp,
    /// never an allocation. An out-of-range value is clamped rather than refused.
    fn set_param(&mut self, id: ParamId, value: i32);

    /// The parameter's target value, or `None` if this effect has no such parameter.
    fn param(&self, id: ParamId) -> Option<i32>;

    /// Clear every delay line and envelope; parameters keep their values, landing on
    /// whatever they were ramping towards.
    fn reset(&mut self);

    /// What this effect is.
    fn descriptor(&self) -> &'static InsertDescriptor;
}

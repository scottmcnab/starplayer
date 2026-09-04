//! [`Stereo`] — the left/right pair every bus, gain and DSP block is written in.
//!
//! It lived in `starplayer-mixer` until M7-H1. Effects need it and `starplayer-dsp` sits
//! *below* the mixer in the dependency order, so the type moved down and the mixer
//! re-exports it; `starplayer_mixer::Stereo` and `starplayer_mixer::path::Stereo` both
//! still resolve to this type.

/// A left/right pair of whatever the path uses for gain or for accumulated signal.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Stereo<T> {
    /// Left channel.
    pub left: T,
    /// Right channel.
    pub right: T,
}

impl<T> Stereo<T> {
    /// A left/right pair.
    pub const fn new(left: T, right: T) -> Stereo<T> { Stereo { left, right } }
}

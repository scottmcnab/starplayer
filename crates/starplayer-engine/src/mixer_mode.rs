//! Plain-data mixer configuration shared by hosts.
//!
//! The engine's path, interpolator and output format are type parameters, so this value
//! does not mutate a live [`Engine`](crate::Engine). A host uses it to select one of a
//! finite set of engine instantiations when it opens or rebuilds an output stream.

use core::fmt::{self, Display, Formatter};

use starplayer_core::Interpolator;

/// Which accumulator implementation a host should instantiate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum MixPathKind {
    /// Floating-point accumulation, the desktop and browser default.
    #[default]
    Float,
    /// Integer accumulation, the canonical bit-exact path.
    Fixed,
}

/// Host-visible output depth after mixing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputDepth {
    /// Normalised 32-bit floating point.
    #[default]
    F32,
    /// Signed 32-bit integer.
    I32,
    /// Signed 24-bit integer, represented as 24-in-32 by the mixer.
    I24,
    /// Signed 16-bit integer.
    I16,
    /// Signed 8-bit integer.
    I8,
}

/// A host's requested mixer and output configuration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MixerMode {
    /// Float or fixed-point accumulation.
    pub path: MixPathKind,
    /// Resampling kernel. The current web host accepts nearest and linear.
    pub interpolator: Interpolator,
    /// Output depth applied after the engine's output ring.
    pub depth: OutputDepth,
    /// Whether deterministic TPDF dither is enabled for reduced-depth conversion.
    pub dither: bool,
    /// Output channels. Current hosts accept one or two.
    pub channels: u8,
}

impl Default for MixerMode {
    fn default() -> MixerMode { MixerMode::DEFAULT }
}

impl MixerMode {
    /// Float, linear, f32, undithered stereo.
    pub const DEFAULT: MixerMode = MixerMode {
        path: MixPathKind::Float,
        interpolator: Interpolator::Linear,
        depth: OutputDepth::F32,
        dither: false,
        channels: 2,
    };

    /// Stable wire layout:
    ///
    /// ```text
    /// bit 0       path          0 float, 1 fixed
    /// bits 1..=2  interpolator  0 nearest, 1 linear, 2 cubic, 3 sinc
    /// bits 3..=5  depth         0 f32, 1 i32, 2 i24, 3 i16, 4 i8
    /// bit 6       dither        0 off, 1 TPDF
    /// bit 7       reserved      must be zero
    /// bits 8..=15 channels      literal channel count (currently 1 or 2)
    /// bits 16..=31 reserved     must be zero
    /// ```
    pub const fn to_wire(self) -> u32 {
        let path = match self.path {
            MixPathKind::Float => 0,
            MixPathKind::Fixed => 1,
        };
        let interpolator = match self.interpolator {
            Interpolator::None => 0,
            Interpolator::Linear => 1,
            Interpolator::Cubic => 2,
            Interpolator::Sinc => 3,
        };
        let depth = match self.depth {
            OutputDepth::F32 => 0,
            OutputDepth::I32 => 1,
            OutputDepth::I24 => 2,
            OutputDepth::I16 => 3,
            OutputDepth::I8 => 4,
        };
        path | (interpolator << 1) | (depth << 3) | ((self.dither as u32) << 6) | ((self.channels as u32) << 8)
    }

    /// Decode a mode, rejecting unknown depth values, reserved bits and channel counts
    /// other than mono or stereo.
    pub const fn from_wire(wire: u32) -> Option<MixerMode> {
        if wire & 0xFFFF_0080 != 0 {
            return None;
        }
        let path = if wire & 1 == 0 { MixPathKind::Float } else { MixPathKind::Fixed };
        let interpolator = match (wire >> 1) & 0b11 {
            0 => Interpolator::None,
            1 => Interpolator::Linear,
            2 => Interpolator::Cubic,
            3 => Interpolator::Sinc,
            _ => return None,
        };
        let depth = match (wire >> 3) & 0b111 {
            0 => OutputDepth::F32,
            1 => OutputDepth::I32,
            2 => OutputDepth::I24,
            3 => OutputDepth::I16,
            4 => OutputDepth::I8,
            _ => return None,
        };
        let channels = ((wire >> 8) & 0xFF) as u8;
        if channels != 1 && channels != 2 {
            return None;
        }
        Some(MixerMode { path, interpolator, depth, dither: wire & (1 << 6) != 0, channels })
    }

    /// A compact label suitable for telemetry and user interfaces.
    pub fn describe(&self) -> impl Display + '_ { self }
}

impl Display for MixerMode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let path = match self.path {
            MixPathKind::Float => "float",
            MixPathKind::Fixed => "fixed",
        };
        let interpolator = match self.interpolator {
            Interpolator::None => "nearest",
            Interpolator::Linear => "linear",
            Interpolator::Cubic => "cubic",
            Interpolator::Sinc => "sinc",
        };
        let depth = match self.depth {
            OutputDepth::F32 => "32-bit float",
            OutputDepth::I32 => "32-bit int",
            OutputDepth::I24 => "24-bit",
            OutputDepth::I16 => "16-bit",
            OutputDepth::I8 => "8-bit",
        };
        let channels = match self.channels {
            1 => "mono",
            2 => "stereo",
            _ => "invalid channels",
        };
        write!(formatter, "{path} · {interpolator} · {depth}")?;
        if self.dither {
            formatter.write_str(" · TPDF")?;
        }
        write!(formatter, " · {channels}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn mixer_mode_wire_round_trips_every_field() {
        let modes = [
            MixerMode::DEFAULT,
            MixerMode { path: MixPathKind::Fixed, interpolator: Interpolator::None, depth: OutputDepth::I8, dither: true, channels: 1 },
            MixerMode { path: MixPathKind::Float, interpolator: Interpolator::Cubic, depth: OutputDepth::I24, dither: false, channels: 2 },
            MixerMode { path: MixPathKind::Fixed, interpolator: Interpolator::Sinc, depth: OutputDepth::I32, dither: true, channels: 2 },
        ];
        for mode in modes {
            assert_eq!(MixerMode::from_wire(mode.to_wire()), Some(mode));
        }
        assert_eq!(MixerMode::DEFAULT.to_wire(), 0x0000_0202, "the default encoding is part of the stable wire protocol");
        assert_eq!(MixerMode::from_wire(0), None, "zero carries an invalid channel count");
        assert_eq!(MixerMode::from_wire(MixerMode::DEFAULT.to_wire() | 0x80), None, "reserved bits are rejected");
    }

    #[test]
    fn mixer_mode_descriptions_are_compact_and_stable() {
        assert_eq!(MixerMode::DEFAULT.describe().to_string(), "float · linear · 32-bit float · stereo");
        let retro = MixerMode { path: MixPathKind::Fixed, interpolator: Interpolator::None, depth: OutputDepth::I8, dither: false, channels: 2 };
        assert_eq!(retro.describe().to_string(), "fixed · nearest · 8-bit · stereo");
        let dithered = MixerMode { dither: true, ..retro };
        assert_eq!(dithered.describe().to_string(), "fixed · nearest · 8-bit · TPDF · stereo");
    }
}

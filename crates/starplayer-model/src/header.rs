//! [`ModuleHeader`] — the format-neutral song header — plus [`ModuleFormat`] and
//! [`ModuleFlags`].

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use starplayer_core::{I1F15, U0F16};

/// Which file format a module was loaded from.
///
/// The engine needs this to pick the format's effect processor and tempo model; nothing
/// else about a module's behaviour is keyed off it. Design goal 7: each format keeps its
/// own pattern data and its own effect processor, and no format is lowered into another.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleFormat {
    /// Scream Tracker 3.
    S3m,
    /// ProTracker and its many descendants.
    Mod,
    /// MultiTracker.
    Mtm,
    /// FastTracker 2 (M5).
    Xm,
    /// Impulse Tracker (M6).
    It,
}

/// Song-wide behaviour switches that more than one format shares.
///
/// A flag earns a place here when the *engine* has to know about it. Bits that only one
/// format's effect processor cares about stay in
/// [`ModuleHeader::format_extra`](ModuleHeader::format_extra) or in the format crate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModuleFlags {
    /// Clamp periods to the Amiga hardware range — ProTracker's behaviour, and S3M's
    /// "Amiga limits" header bit.
    pub amiga_limits: bool,
    /// Pitch slides are linear in semitones rather than in Amiga periods (XM, IT).
    pub linear_slides: bool,
    /// Scream Tracker 2's volume-slide timing, where a slide also applies on tick 0.
    pub fast_volume_slides: bool,
    /// The module asks for stereo playback (S3M's stereo bit); a mono module pans every
    /// channel to the centre.
    pub stereo: bool,
}

/// Everything about a song that is not a sample, an instrument, a pattern or an order.
///
/// Deliberately format-neutral. Anything a single format's effect processor needs and
/// nothing else does goes in [`format_extra`](ModuleHeader::format_extra) — a small
/// bitfield the format crate owns and interprets — or stays inside that crate entirely.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleHeader {
    /// The song title as the file spells it, already trimmed of padding.
    pub title: Box<str>,
    /// Which format this came from.
    pub format: ModuleFormat,
    /// Channels the song plays on. At least one; the builder rejects zero.
    pub channel_count: u8,
    /// Ticks per row at the start of the song (S3M's "initial speed", ProTracker's Fxx
    /// below 32).
    pub initial_speed: u8,
    /// Beats per minute at the start of the song. `u16` because IT allows up to 255 and
    /// nothing is gained by making the field the same width as the file's.
    pub initial_tempo: u16,
    /// Song global volume, scaling every channel.
    pub global_volume: U0F16,
    /// Master/mixing volume — the original's amplification setting, S3M's master volume
    /// byte.
    pub master_volume: U0F16,
    /// Default pan position per channel, `-1` hard left to `+1` hard right.
    ///
    /// Either empty — meaning "centre every channel" — or exactly `channel_count` long.
    /// The builder rejects any other length.
    pub default_pan: Box<[I1F15]>,
    /// Behaviour switches shared across formats.
    pub flags: ModuleFlags,
    /// Format-owned header bits. The format crate that produced the module is the only
    /// thing that may interpret this; the engine passes it through untouched.
    pub format_extra: u32,
}

impl ModuleHeader {
    /// A header for `format` with `channel_count` centred channels, ProTracker's default
    /// speed 6 / tempo 125, and unity volumes.
    pub fn new(format: ModuleFormat, channel_count: u8) -> ModuleHeader {
        ModuleHeader {
            title: String::new().into_boxed_str(),
            format,
            channel_count,
            initial_speed: 6,
            initial_tempo: 125,
            global_volume: U0F16::MAX,
            master_volume: U0F16::MAX,
            default_pan: Box::default(),
            flags: ModuleFlags::default(),
            format_extra: 0,
        }
    }

    /// A `default_pan` table with every channel centred, for a loader that would rather
    /// fill one in than leave the field empty.
    pub fn centred_pan(channel_count: u8) -> Box<[I1F15]> {
        vec![I1F15::ZERO; channel_count as usize].into_boxed_slice()
    }

    /// Default pan for one channel, `None` past the end of the song's channels.
    ///
    /// An empty [`default_pan`](ModuleHeader::default_pan) means every channel is
    /// centred, so this answers [`I1F15::ZERO`] for any channel in range.
    pub fn channel_pan(&self, channel: u8) -> Option<I1F15> {
        if channel >= self.channel_count {
            return None;
        }
        match self.default_pan.is_empty() {
            true => Some(I1F15::ZERO),
            false => self.default_pan.get(channel as usize).copied(),
        }
    }
}

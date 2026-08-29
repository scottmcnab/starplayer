//! [`SampleIndex`] — where one sample lives in the module's PCM blob and how it loops —
//! and [`SampleSpec`], the description a loader hands to
//! [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample).

use alloc::boxed::Box;
use alloc::string::String;
use starplayer_core::U0F16;

/// The reference rate a sample with no rate of its own is assumed to play C-4 at: the
/// Amiga/ProTracker middle-C rate, and Scream Tracker 3's default C2SPD.
pub const DEFAULT_REFERENCE_RATE_HZ: u32 = 8363;

/// How a sample repeats once playback reaches its loop end.
///
/// `PingPong` is declared now because XM and IT need it; the mixer implements the
/// forward cases only (M1), and the bidirectional kernel and its own guard-frame
/// treatment arrive with those formats (M6).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LoopMode {
    /// Plays once, then the voice ends.
    #[default]
    None,
    /// Repeats `loop_start .. loop_end` forwards forever.
    Forward,
    /// Alternates forwards and backwards over `loop_start .. loop_end` (XM/IT, M6).
    PingPong,
}

impl LoopMode {
    /// Whether this mode loops at all.
    pub const fn is_looping(self) -> bool { !matches!(self, LoopMode::None) }
}

/// What a loader knows about a sample before its PCM has been placed in the blob.
///
/// The builder derives `pcm_offset` and the stored length itself, because those depend on
/// the guard-frame layout it writes — a loader cannot get them wrong.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SampleSpec {
    /// The sample's name as the file spells it, already trimmed of padding.
    pub name: String,
    /// How the sample repeats.
    pub loop_mode: LoopMode,
    /// First frame of the loop. Ignored when `loop_mode` is [`LoopMode::None`].
    pub loop_start: u32,
    /// One past the last frame of the loop. Ignored when `loop_mode` is
    /// [`LoopMode::None`].
    pub loop_end: u32,
    /// The sample's own volume, before channel and global volume.
    pub default_volume: U0F16,
    /// The playback rate, in Hz, at which this sample sounds its reference note (C-4).
    ///
    /// S3M's C2SPD and MOD's finetune-derived rate both land here, **at the full 32-bit
    /// width** — the original read only the low 16 bits of S3M's 32-bit field, which is
    /// deviation D7 in `plans/product/03-accuracy-policy.md`.
    pub reference_rate_hz: u32,
}

impl SampleSpec {
    /// A one-shot sample at the default reference rate and full volume.
    pub fn one_shot(name: &str) -> SampleSpec {
        SampleSpec {
            name: String::from(name),
            loop_mode: LoopMode::None,
            loop_start: 0,
            loop_end: 0,
            default_volume: U0F16::MAX,
            reference_rate_hz: DEFAULT_REFERENCE_RATE_HZ,
        }
    }

    /// The same sample with a forward loop over `start .. end`.
    pub fn with_forward_loop(self, start: u32, end: u32) -> SampleSpec {
        SampleSpec { loop_mode: LoopMode::Forward, loop_start: start, loop_end: end, ..self }
    }

    /// The same sample at a different reference rate.
    pub fn with_reference_rate(self, reference_rate_hz: u32) -> SampleSpec {
        SampleSpec { reference_rate_hz, ..self }
    }
}

/// Where one sample lives in [`Module::pcm`](crate::Module::pcm), and how it loops.
///
/// # The layout this describes
///
/// `length_frames` counts the **addressable** frames — the frames a playback position may
/// legally land on. Immediately after them the blob holds
/// [`GUARD_FRAMES`](starplayer_core::GUARD_FRAMES) more, which the interpolator may read
/// but which are never a playback position. So the sample occupies
/// `pcm_offset .. pcm_offset + length_frames + GUARD_FRAMES`, which is exactly what
/// `starplayer_mixer::sample::SampleData::resolve` expects.
///
/// For a forward-looping sample the addressable length **is** `loop_end`: the frames
/// after the loop end are never audible, so the builder discards them and fills the guard
/// with a wrapped copy of the loop instead. See
/// [`GUARD_FRAMES`](starplayer_core::GUARD_FRAMES).
///
/// # Turning this into a mixer region
///
/// `starplayer-model` deliberately does not depend on `starplayer-mixer` (the edges are
/// model → core and mixer → core, dsp), so there is no `SampleRegion` constructor here.
/// The engine builds one from `pcm_offset`, `length_frames` and the loop fields when it
/// triggers a voice.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SampleIndex {
    pcm_offset: u32,
    length_frames: u32,
    loop_start: u32,
    loop_end: u32,
    loop_mode: LoopMode,
    default_volume: U0F16,
    reference_rate_hz: u32,
    name: Box<str>,
}

impl SampleIndex {
    /// Assemble an index. Crate-private: the only supported way to obtain a
    /// `SampleIndex` is [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample),
    /// which is also what guarantees the PCM behind it is laid out as documented.
    pub(crate) fn new(pcm_offset: u32, length_frames: u32, specification: SampleSpec) -> SampleIndex {
        let SampleSpec { name, loop_mode, loop_start, loop_end, default_volume, reference_rate_hz } = specification;
        SampleIndex {
            pcm_offset,
            length_frames,
            loop_start,
            loop_end,
            loop_mode,
            default_volume,
            reference_rate_hz,
            name: name.into_boxed_str(),
        }
    }

    /// Offset of the sample's first frame within [`Module::pcm`](crate::Module::pcm).
    pub const fn pcm_offset(&self) -> u32 { self.pcm_offset }

    /// Addressable frames, excluding the guard frames.
    pub const fn length_frames(&self) -> u32 { self.length_frames }

    /// Frames this sample occupies in the blob, guard frames included.
    pub const fn stored_frames(&self) -> usize {
        self.length_frames as usize + starplayer_core::GUARD_FRAMES
    }

    /// First frame of the loop. Meaningless unless [`SampleIndex::loop_mode`] loops.
    pub const fn loop_start(&self) -> u32 { self.loop_start }

    /// One past the last frame of the loop. Meaningless unless
    /// [`SampleIndex::loop_mode`] loops.
    pub const fn loop_end(&self) -> u32 { self.loop_end }

    /// How the sample repeats.
    pub const fn loop_mode(&self) -> LoopMode { self.loop_mode }

    /// The sample's own volume.
    pub const fn default_volume(&self) -> U0F16 { self.default_volume }

    /// Playback rate in Hz for the reference note — the full 32 bits (accuracy policy
    /// D7).
    pub const fn reference_rate_hz(&self) -> u32 { self.reference_rate_hz }

    /// The sample's name as the file spelled it.
    pub fn name(&self) -> &str { &self.name }
}

//! [`SampleIndex`] — where one sample lives in the module's PCM blob and how it loops —
//! and [`SampleSpec`], the description a loader hands to
//! [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample).

use alloc::boxed::Box;
use alloc::string::String;
use starplayer_core::{I1F15, U0F16};

/// The reference rate a sample with no rate of its own is assumed to play C-4 at: the
/// Amiga/ProTracker middle-C rate, and Scream Tracker 3's default C2SPD.
pub const DEFAULT_REFERENCE_RATE_HZ: u32 = 8363;

/// How a sample repeats once playback reaches its loop end.
///
/// `PingPong` is declared for XM and IT. `starplayer_mixer::kernel` implements both
/// modes' run and boundary arithmetic already; what task E1 adds on the model side is the
/// builder-side guard-frame layout for a ping-pong loop with no sustain loop, matching
/// `starplayer_mixer::sample::append_guarded_sample` frame for frame. See
/// [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LoopMode {
    /// Plays once, then the voice ends.
    #[default]
    None,
    /// Repeats `loop_start .. loop_end` forwards forever.
    Forward,
    /// Alternates forwards and backwards over `loop_start .. loop_end` (XM/IT).
    PingPong,
}

impl LoopMode {
    /// Whether this mode loops at all.
    pub const fn is_looping(self) -> bool { !matches!(self, LoopMode::None) }
}

/// Shape of XM/IT auto-vibrato's LFO.
///
/// The two formats number these differently and neither numbering is reproduced here:
/// IT's `VibratoType` is `0` sine, `1` ramp down, `2` square, `3` random, while XM's
/// `vibType` is `0` sine, `1` square, `2` ramp down, `3` ramp up. Each loader maps its own
/// numbering onto this enum, which is why [`RampUp`](AutoVibratoWaveform::RampUp) — an XM
/// shape IT has no code for — sits at the end rather than beside
/// [`RampDown`](AutoVibratoWaveform::RampDown).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum AutoVibratoWaveform {
    #[default]
    Sine,
    RampDown,
    Square,
    Random,
    /// A rising sawtooth: XM's `vibType` 3 (M5-F1). IT has no equivalent.
    RampUp,
}

/// Auto-vibrato, in the source format's own units — like [`InstrumentDef::fadeout`](crate::InstrumentDef::fadeout).
///
/// XM stores one auto-vibrato per **instrument** and FT2 applies it to every sample the
/// instrument owns; the XM loader copies it down onto each [`SampleSpec`] it builds. IT
/// stores one per **sample** natively, so its loader passes each sample's own values
/// straight through. `sweep` in particular means different things in the two formats —
/// XM's `vibrato_sweep` is the number of ticks to reach full depth, IT's `VibratoSweep` is
/// the rate depth ramps in at — and neither is normalised here; the format's own effect
/// processor reads it in its own units.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AutoVibrato {
    /// Shape of the LFO.
    pub waveform: AutoVibratoWaveform,
    /// How quickly the depth ramps in from zero, in the source format's own units.
    pub sweep: u8,
    /// Depth of the LFO, in the source format's own units. Zero — the neutral value — is
    /// no vibrato at all.
    pub depth: u8,
    /// Speed of the LFO, in the source format's own units.
    pub rate: u8,
}

/// IT's sustain loop: a second, inner loop played while a note is held, released to the
/// sample's ordinary loop (or to no loop) on key-off.
///
/// Declared, not played — nothing in M1 through E1 reads this field. See
/// [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample) for what a sustain loop
/// does to a sample's stored-frame and guard-frame layout.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SustainLoop {
    /// Forward or ping-pong; [`LoopMode::None`] is meaningless here and rejected by
    /// [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample).
    pub mode: LoopMode,
    /// First frame of the sustain loop.
    pub start: u32,
    /// One past the last frame of the sustain loop.
    pub end: u32,
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
    /// deviation D7 in `plans/product/03-accuracy-policy.md`. IT's `C5Speed` is a real
    /// rate too and maps straight here.
    pub reference_rate_hz: u32,
    /// XM: signed semitone offset added to the note before pitch is derived. Kept raw
    /// rather than folded into `reference_rate_hz`: XM's tuning is applied in linear or
    /// Amiga mode *after* the note is known, so deriving a Hz rate at load time would be
    /// lossy in Amiga mode. The XM loader passes [`DEFAULT_REFERENCE_RATE_HZ`] here as the
    /// nominal rate. Formats without a relative note leave this `0`.
    pub relative_note: i8,
    /// XM: signed finetune in 1/128 semitone, kept raw for the same reason as
    /// `relative_note`. IT expresses tuning through `reference_rate_hz` instead and leaves
    /// this `0`.
    pub finetune: i8,
    /// The sample's own default pan, or `None` when the file does not enable one (IT's
    /// pan "enabled" bit; XM always has a sample pan, but this stays `Option` so a format
    /// with no per-sample pan at all need not invent one).
    pub default_pan: Option<I1F15>,
    /// Per-sample auto-vibrato. XM stores it per instrument and applies it to every
    /// sample; the XM loader copies it down onto each sample it builds.
    pub auto_vibrato: AutoVibrato,
    /// IT's sustain loop, played instead of the normal loop until the note is released.
    /// See [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample) for what this
    /// does to the sample's stored-frame and guard-frame layout.
    pub sustain_loop: Option<SustainLoop>,
}

impl SampleSpec {
    /// A one-shot sample at the default reference rate and full volume, with every
    /// XM/IT-only field at its neutral value.
    pub fn one_shot(name: &str) -> SampleSpec {
        SampleSpec {
            name: String::from(name),
            loop_mode: LoopMode::None,
            loop_start: 0,
            loop_end: 0,
            default_volume: U0F16::MAX,
            reference_rate_hz: DEFAULT_REFERENCE_RATE_HZ,
            relative_note: 0,
            finetune: 0,
            default_pan: None,
            auto_vibrato: AutoVibrato::default(),
            sustain_loop: None,
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
/// with a wrapped copy of the loop instead. A ping-pong loop with no sustain loop is the
/// same shape, with a reflected guard rather than a wrapped one. See
/// [`GUARD_FRAMES`](starplayer_core::GUARD_FRAMES).
///
/// # Sample sustain loops
///
/// A sample with a sustain loop stores its **whole** body — the normal loop and the
/// sustain loop may each lie anywhere inside it — with a silent guard, because neither
/// loop's end need sit at the stored length any more. One frame of `Linear`
/// interpolation reads real PCM past a loop end instead of the wrapped or reflected copy
/// in this case; that is accepted for now and left for M7's kernels to reconsider.
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
    relative_note: i8,
    finetune: i8,
    default_pan: Option<I1F15>,
    auto_vibrato: AutoVibrato,
    sustain_loop: Option<SustainLoop>,
    name: Box<str>,
}

impl SampleIndex {
    /// Assemble an index. Crate-private: the only supported way to obtain a
    /// `SampleIndex` is [`ModuleBuilder::add_sample`](crate::ModuleBuilder::add_sample),
    /// which is also what guarantees the PCM behind it is laid out as documented.
    pub(crate) fn new(pcm_offset: u32, length_frames: u32, specification: SampleSpec) -> SampleIndex {
        let SampleSpec {
            name,
            loop_mode,
            loop_start,
            loop_end,
            default_volume,
            reference_rate_hz,
            relative_note,
            finetune,
            default_pan,
            auto_vibrato,
            sustain_loop,
        } = specification;
        SampleIndex {
            pcm_offset,
            length_frames,
            loop_start,
            loop_end,
            loop_mode,
            default_volume,
            reference_rate_hz,
            relative_note,
            finetune,
            default_pan,
            auto_vibrato,
            sustain_loop,
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

    /// XM's signed semitone offset, raw. `0` for a format with no relative note.
    pub const fn relative_note(&self) -> i8 { self.relative_note }

    /// XM's signed finetune, in 1/128 semitone, raw. `0` for a format with no finetune of
    /// its own.
    pub const fn finetune(&self) -> i8 { self.finetune }

    /// The sample's own default pan, or `None` when the file does not enable one.
    pub const fn default_pan(&self) -> Option<I1F15> { self.default_pan }

    /// Per-sample auto-vibrato, in the source format's own units.
    pub const fn auto_vibrato(&self) -> AutoVibrato { self.auto_vibrato }

    /// IT's sustain loop, or `None` for a sample that has none.
    pub const fn sustain_loop(&self) -> Option<SustainLoop> { self.sustain_loop }

    /// The sample's name as the file spelled it.
    pub fn name(&self) -> &str { &self.name }
}

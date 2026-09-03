//! [`QuirkSet`], [`FormatDialect`] and [`QuirkSelection`] — replay behaviour as **data**.
//!
//! The philosophy is `plans/product/03-accuracy-policy.md`'s: a quirk is a named field of
//! a small `Copy` struct, not a branch scattered through a processor. A behaviour that
//! cannot be expressed as a field here is not a quirk — it is a format difference and
//! belongs in that format's own processor (design goal 7).
//!
//! # The three layers
//!
//! 1. [`QuirkSet`] — the flat answer. Every field names the accuracy-policy entry it
//!    implements, and [`QuirkSet::canonical`] is the default profile.
//! 2. [`FormatDialect`] — what the *file header* said wrote the module. Each loader
//!    detects one and stores it in the module header; [`FormatDialect::quirks`] maps it
//!    to the `QuirkSet` that dialect implies.
//! 3. [`QuirkSelection`] — the precedence, in the type: [`QuirkSelection::FromDialect`]
//!    takes the loader's answer, [`QuirkSelection::Override`] takes the host's. There is
//!    no third source and no global.
//!
//! # The one field the header cannot settle
//!
//! [`QuirkSet::mod_timing`] is the exception to layer 2, and the accuracy policy's §2
//! table records it as such. A MOD's four-byte tag says *which* tracker wrote it but not
//! *which interrupt* that tracker's replayer ran off, and a ProTracker-tagged file may
//! have been written under either. Only the `M&K!` and `N.T.` tags settle it from the
//! header; every other VBlank-timed MOD is recognised from its pattern cells and, when
//! those are ambiguous, from which of the two timings gives the shorter song. That
//! decision needs a sequencer, so it lives in the facade's `scan_song` rather than in a
//! loader, and it reaches a processor the same way any other host decision does — as a
//! [`QuirkSelection::Override`]. The dialect is still never revised.
//!
//! # Fixed for the lifetime of a loaded module
//!
//! A `QuirkSet` is resolved once, when a sequencer is built for a module, and stored by
//! value in the processor. Nothing exposes a setter: changing a quirk mid-playback would
//! mean a channel's effect memories were written under one rule and read under another,
//! and no tracker ever did that. Load the module again to change the profile.

use crate::tempo::TempoModelId;

/// Which video clock a MOD's Amiga periods are divided by (accuracy policy **D14**).
///
/// ProTracker on a PAL Amiga clocks Paula at 3_546_895 Hz; the NTSC machine runs at
/// 3_579_545 Hz, about 0.36 % sharp. libxmp picks NTSC for the ScreamTracker 3,
/// FastTracker, TakeTracker and Mod's Grave tracker ids or for more than four channels,
/// and PAL for everything else; StarPlayer's default is PAL for every MOD because every
/// affected corpus fixture is a four-channel `M.K.` file whose dump advances at PAL speed.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaulaClock {
    /// 3_546_895 Hz. The default.
    #[default]
    Pal,
    /// 3_579_545 Hz.
    Ntsc,
}

impl PaulaClock {
    /// The clock in hertz, as the step derivation divides it by the Amiga period.
    pub const fn hz(self) -> u64 {
        match self {
            PaulaClock::Pal => 3_546_895,
            PaulaClock::Ntsc => 3_579_545,
        }
    }
}

/// Which interrupt a MOD's tracker tick ran off (accuracy policy **§2**, *Tracker
/// dialects*).
///
/// ProTracker on an Amiga could clock its replayer from the CIA-B timer or from the
/// vertical blank. On the CIA it can be told a tempo in beats per minute, so `Fxx` splits
/// at 32; on the vertical blank there is no timer to set, so every `Fxx` is ticks per row
/// and a value of 32 or more is simply a long row. NoiseTracker and SoundTracker have no
/// CIA mode at all.
///
/// The oracle sees it as libxmp's `QUIRK_NOBPM` (`src/effects.c:463-472`), which is what
/// the `M&K!` / `N.T.` tags, the pattern evidence and the two-timing length comparison
/// in `starplayer::scan_song` select between.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModTiming {
    /// The CIA timer: `Fxx` below 32 sets ticks per row, 32 and above sets BPM.
    /// ProTracker 2's rule and the default.
    #[default]
    Cia,
    /// The vertical blank: there is no BPM, so every non-zero `Fxx` sets ticks per row.
    /// NoiseTracker, SoundTracker and ProTracker's own VBlank mode.
    VBlank,
}

/// How a MOD-family `Dxx` pattern-break parameter is read (C3 research point 3).
///
/// ProTracker stores the destination row as two decimal digits packed one per nibble, so
/// `D16` breaks to row 16 and `D1A` is out of range and wraps to row 0. MultiTracker's
/// own documentation, and libxmp's break handler for it, read the byte as plain
/// hexadecimal.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum BreakParameter {
    /// `(xx >> 4) * 10 + (xx & 15)`, wrapping to row 0 past 63. ProTracker's own reading.
    #[default]
    BinaryCodedDecimal,
    /// The byte as written. MultiTracker's reading.
    Hexadecimal,
}

/// Which tracker's `SBx` pattern-loop and break/jump interaction an S3M is played with
/// (accuracy policy §2, *Tracker dialects*).
///
/// libxmp selects the same four profiles from the `Cwt/v` field at
/// `src/loaders/s3m_load.c:390-432`; [`FormatDialect`] reproduces that predicate and this
/// enum is the behaviour it selects.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum S3mLoopDialect {
    /// Scream Tracker 3.21, the default and the subject of accuracy policy **D30**.
    #[default]
    ScreamTracker321,
    /// Scream Tracker 3.01: the first `SBx` with no preceding `SB0` targets *its own*
    /// row, and a loop jump does not block a `Bxx` / `Cxx` on the same row.
    ScreamTracker301,
    /// ModPlug Tracker 1.16 / early OpenMPT: per-channel loop target and counter, and a
    /// channel only *starts* a loop when no other channel is already looping.
    ModPlug116,
    /// Imago Orpheus: per-channel target and counter, no loop-target advancement, and a
    /// loop jump rewrites — rather than blocks — a `Cxx` already seen on the row.
    ImagoOrpheus,
}

/// Which tracker's `E6x` pattern-loop and break/jump interaction a MOD is played with
/// (accuracy policy §2, *Tracker dialects*).
///
/// libxmp selects these from the four-byte tag at offset 1080
/// (`src/loaders/mod_load.c:76-95` and `:983-994`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModLoopDialect {
    /// ProTracker and everything that copied it: per-channel target and counter, and no
    /// interaction at all between a loop jump and a `Bxx` / `Dxx` on the same row.
    #[default]
    ProTracker,
    /// Atari Octalyser (`CD61` / `CD81`): one global target and counter, `E60` ignored
    /// while a loop is running, the end of a loop cancels jumps already seen on the row,
    /// and a loop jump blocks every break and jump on its row.
    Octalyser,
    /// Digital Tracker (`FA04` / `FA06` / `FA08`): one global target and counter, only
    /// the **first** `E60` or `E6x` on a row is executed at all, and a loop jump blocks
    /// every break and jump on its row.
    DigitalTracker,
}

/// The pattern-loop and break/jump rules one dialect implies.
///
/// Derived data, not a [`QuirkSet`] field: [`S3mLoopDialect::flow`] and
/// [`ModLoopDialect::flow`] produce it, and `starplayer-engine`'s `PatternFlowState`
/// executes it. The field names and their meanings are libxmp's `FLOW_LOOP_*` bits
/// (`src/common.h:332-346`), because that is the vocabulary the oracle dumps were
/// generated with and matching it field for field is what makes the comparison honest.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PatternFlow {
    /// `FLOW_LOOP_GLOBAL_TARGET`: one loop target for the whole song rather than one per
    /// channel.
    pub global_target: bool,
    /// `FLOW_LOOP_GLOBAL_COUNT`: one loop counter for the whole song.
    pub global_count: bool,
    /// `FLOW_LOOP_END_ADVANCES`: when a loop finishes, the target moves to the `SBx` row
    /// plus one.
    pub end_advances: bool,
    /// `FLOW_LOOP_END_CANCELS`: a finishing loop cancels a loop destination already
    /// chosen on this row.
    pub end_cancels: bool,
    /// `FLOW_LOOP_PATTERN_RESET`: a position change clears the target and the counter.
    pub pattern_reset: bool,
    /// `FLOW_LOOP_INIT_SAMEROW`: an `SBx` with no target yet set targets its own row
    /// rather than row zero.
    pub init_same_row: bool,
    /// `FLOW_LOOP_FIRST_EFFECT`: only the first pattern-loop effect on a row runs.
    pub first_effect_only: bool,
    /// `FLOW_LOOP_ONE_AT_A_TIME`: a channel starts a loop only when no other channel's
    /// counter is running.
    pub one_at_a_time: bool,
    /// `FLOW_LOOP_IGNORE_TARGET`: an `E60` / `SB0` is ignored while this channel's
    /// counter is non-zero.
    pub ignore_target_while_looping: bool,
    /// `FLOW_LOOP_DELAY_BREAK`: a loop jump blocks a break *later* on the same row.
    pub delay_break: bool,
    /// `FLOW_LOOP_DELAY_JUMP`: a loop jump blocks a position jump later on the same row.
    pub delay_jump: bool,
    /// `FLOW_LOOP_UNSET_BREAK`: a loop jump cancels a break *earlier* on the same row.
    pub unset_break: bool,
    /// `FLOW_LOOP_UNSET_JUMP`: a loop jump cancels a position jump earlier on the row.
    pub unset_jump: bool,
    /// `FLOW_LOOP_SHARED_BREAK`: a loop jump overwrites the destination row of a break
    /// already seen on the row instead of cancelling it.
    pub shared_break: bool,
    /// `FLOW_JUMP_NO_ROW_SET`: a `Bxx` does **not** reset the destination row a `Cxx` /
    /// `Dxx` on the same row already chose. Set for Scream Tracker 3 and Impulse Tracker,
    /// clear for ProTracker.
    pub jump_keeps_break_row: bool,
}

impl PatternFlow {
    /// libxmp `FLOW_MODE_GENERIC`: per-channel target and counter and no interaction with
    /// breaks or jumps. ProTracker, MultiTracker and everything that copied them.
    pub const fn generic() -> PatternFlow {
        PatternFlow {
            global_target: false, global_count: false, end_advances: false, end_cancels: false,
            pattern_reset: false, init_same_row: false, first_effect_only: false, one_at_a_time: false,
            ignore_target_while_looping: false, delay_break: false, delay_jump: false, unset_break: false,
            unset_jump: false, shared_break: false, jump_keeps_break_row: false,
        }
    }

    /// `FLOW_LOOP_NO_BREAK_JUMP`: a loop jump both blocks later and cancels earlier
    /// breaks and jumps on its row.
    const fn with_no_break_jump(mut self) -> PatternFlow {
        self.delay_break = true;
        self.delay_jump = true;
        self.unset_break = true;
        self.unset_jump = true;
        self
    }

    /// `FLOW_LOOP_GLOBAL`: one target and one counter for the whole song.
    const fn with_global(mut self) -> PatternFlow {
        self.global_target = true;
        self.global_count = true;
        self
    }
}

impl S3mLoopDialect {
    /// The flow rules this dialect implies, mirroring libxmp's `FLOW_MODE_*` constants.
    pub const fn flow(self) -> PatternFlow {
        match self {
            // FLOW_MODE_ST3_321
            S3mLoopDialect::ScreamTracker321 => {
                let mut flow = PatternFlow::generic().with_global().with_no_break_jump();
                flow.pattern_reset = true;
                flow.end_advances = true;
                flow.jump_keeps_break_row = true;
                flow
            }
            // FLOW_MODE_ST3_301
            S3mLoopDialect::ScreamTracker301 => {
                let mut flow = PatternFlow::generic().with_global();
                flow.pattern_reset = true;
                flow.end_advances = true;
                flow.init_same_row = true;
                flow.jump_keeps_break_row = true;
                flow
            }
            // FLOW_MODE_MPT_116
            S3mLoopDialect::ModPlug116 => {
                let mut flow = PatternFlow::generic().with_no_break_jump();
                flow.one_at_a_time = true;
                flow.jump_keeps_break_row = true;
                flow
            }
            // FLOW_MODE_ORPHEUS. `FLOW_JUMP_THEN_BREAK` is a documented TODO upstream and
            // is therefore not modelled here either; the oracle dumps were generated
            // without it.
            S3mLoopDialect::ImagoOrpheus => {
                let mut flow = PatternFlow::generic();
                flow.pattern_reset = true;
                flow.shared_break = true;
                flow.unset_break = true;
                flow
            }
        }
    }
}

impl ModLoopDialect {
    /// The flow rules this dialect implies, mirroring libxmp's `FLOW_MODE_*` constants.
    pub const fn flow(self) -> PatternFlow {
        match self {
            ModLoopDialect::ProTracker => PatternFlow::generic(),
            // FLOW_MODE_OCTALYSER
            ModLoopDialect::Octalyser => {
                let mut flow = PatternFlow::generic().with_global().with_no_break_jump();
                flow.ignore_target_while_looping = true;
                flow.end_cancels = true;
                flow
            }
            // FLOW_MODE_DTM_2015
            ModLoopDialect::DigitalTracker => {
                let mut flow = PatternFlow::generic().with_global().with_no_break_jump();
                flow.first_effect_only = true;
                flow
            }
        }
    }
}

/// Every replay behaviour StarPlayer lets a host choose, as one `Copy` struct.
///
/// Each field's doc comment names the `plans/product/03-accuracy-policy.md` entry it
/// implements; a field with no policy entry is not allowed here. The two profiles are
/// [`QuirkSet::canonical`] (the default) and [`QuirkSet::starplayer_classic`], and
/// `canonical_and_classic_differ_only_where_the_policy_says_so` in this module's tests
/// asserts field by field that they differ in exactly the entries accuracy policy §2
/// lists — so adding a quirk without updating the policy fails the test suite.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct QuirkSet {
    /// How long one tracker tick lasts (accuracy policy **§2**, first row).
    ///
    /// [`TempoModelId::ExactFixedPoint`] is drift-free; the original's
    /// `(rate * 10 / bpm) >> 2` is [`TempoModelId::St3Truncating`], about 1.3 seconds of
    /// drift over a four-minute song at 130 BPM.
    pub tempo_model: TempoModelId,
    /// Whose `SBx` pattern-loop semantics an S3M is played with (accuracy policy **§2**,
    /// *Tracker dialects*; the ST3.21 baseline is **D30**).
    ///
    /// Flips `libxmp-s3m-pattern-loop-imf`, `-imf-breakjump`, `-mpt`, `-st301` and
    /// `-st301-breakjump` on the pinned corpus.
    pub s3m_pattern_loop: S3mLoopDialect,
    /// Whose `E6x` pattern-loop semantics a MOD is played with (accuracy policy **§2**,
    /// *Tracker dialects*).
    ///
    /// Flips `libxmp-mod-pattern-jump-octalyser-break`,
    /// `libxmp-mod-pattern-loop-octalyser`, `-octalyser-breakjump`,
    /// `libxmp-mod-pattern-loop-dt` and `-dt-breakjump` on the pinned corpus.
    pub mod_pattern_loop: ModLoopDialect,
    /// Which Paula clock a MOD's Amiga periods are divided by (accuracy policy **D14**).
    ///
    /// No corpus case turns on it — every affected fixture is a PAL four-channel `M.K.`
    /// file — so it is observed by a unit test rather than by the oracle.
    pub mod_paula_clock: PaulaClock,
    /// How a MOD-family `Dxx` pattern-break parameter is read (accuracy policy **§1**,
    /// *MOD `Dxx` pattern break*; C3 research point 3).
    pub mod_break_parameter: BreakParameter,
    /// Which interrupt a MOD's tracker tick ran off (accuracy policy **§2**, *Tracker
    /// dialects*).
    ///
    /// The one dialect field the file header cannot settle on its own: only the `M&K!`
    /// and `N.T.` tags say VBlank outright, and every other VBlank-timed MOD is found
    /// from its pattern cells and, when those are ambiguous, from which of the two
    /// timings gives the shorter song. `starplayer::scan_song` is what resolves it; no
    /// corpus case turns on it, so a unit test and the offline scan tests observe it.
    pub mod_timing: ModTiming,
    /// Whether `F00` ends the song (accuracy policy **§1**, *MOD `F00`*).
    ///
    /// ProTracker's `F00` is a stop marker; MultiTracker has no such rule and libxmp
    /// ignores a zero speed parameter outright.
    pub mod_f00_stops_song: bool,
    /// Whether an instrument-only or tone-portamento sample change waits for the sounding
    /// sample's loop point or end (accuracy policy **D12**).
    ///
    /// On under `canonical()`: the six D12 corpus cases depend on it. It also selects
    /// ProTracker's reading of an instrument slot with no PCM as its *null sample*, which
    /// is the same behaviour seen from the other side.
    pub protracker_sample_swap_at_boundary: bool,
    /// Whether tremolo's ramp waveform takes its half from the **vibrato** phase
    /// (accuracy policy **D20**).
    ///
    /// ProTracker's `mt_Tremolo2` tests `n_vibratopos` where it means `n_tremolopos`. Off
    /// under `canonical()`, and no oracle can see it either way — libxmp does not model
    /// the bug — so only a unit test observes it.
    pub protracker_tremolo_ramp_from_vibrato_phase: bool,
}

/// Written out rather than derived: a derived `Default` would give every `bool` `false`,
/// which is not the canonical profile — `mod_f00_stops_song` and
/// `protracker_sample_swap_at_boundary` are both on by default.
impl Default for QuirkSet {
    fn default() -> QuirkSet { QuirkSet::canonical() }
}

impl QuirkSet {
    /// The default profile: canonical ProTracker / Scream Tracker 3.21 behaviour.
    pub const fn canonical() -> QuirkSet {
        QuirkSet {
            tempo_model: TempoModelId::ExactFixedPoint,
            s3m_pattern_loop: S3mLoopDialect::ScreamTracker321,
            mod_pattern_loop: ModLoopDialect::ProTracker,
            mod_paula_clock: PaulaClock::Pal,
            mod_break_parameter: BreakParameter::BinaryCodedDecimal,
            mod_timing: ModTiming::Cia,
            mod_f00_stops_song: true,
            protracker_sample_swap_at_boundary: true,
            protracker_tremolo_ramp_from_vibrato_phase: false,
        }
    }

    /// Accuracy policy §2: the original DOS player's own behaviour where it differs from
    /// the canonical one and a modern default should not have it.
    ///
    /// §2 has exactly two rows. The first is the truncating tick length, which is this
    /// profile's only difference from [`QuirkSet::canonical`]. The second — MOD and MTM
    /// interpreted through an in-memory S3M conversion — is **not offered at all**
    /// (policy §4), so it is not a field.
    pub const fn starplayer_classic() -> QuirkSet {
        QuirkSet { tempo_model: TempoModelId::St3Truncating, ..QuirkSet::canonical() }
    }

    /// The profile a build's features ask for: [`QuirkSet::starplayer_classic`] when the
    /// `quirks-starplayer` feature is on, [`QuirkSet::canonical`] otherwise.
    ///
    /// This is the only place the feature is read. Every §2 quirk is a data field rather
    /// than a code path, so there is nothing else for it to gate — "where a quirk is a
    /// single branch on a `QuirkSet` field, no feature gate is needed" applies to all of
    /// them, and the feature therefore costs nothing at runtime.
    pub const fn profile_default() -> QuirkSet {
        #[cfg(feature = "quirks-starplayer")]
        {
            QuirkSet::starplayer_classic()
        }
        #[cfg(not(feature = "quirks-starplayer"))]
        {
            QuirkSet::canonical()
        }
    }

    /// ProTracker 1.x / 2.x and its many clones — the MOD baseline.
    pub const fn protracker() -> QuirkSet { QuirkSet::profile_default() }

    /// NoiseTracker (`M&K!`, `N.T.`): ProTracker's replay off the vertical blank, so
    /// there is no BPM and every non-zero `Fxx` is ticks per row.
    pub const fn noisetracker() -> QuirkSet {
        QuirkSet { mod_timing: ModTiming::VBlank, ..QuirkSet::protracker() }
    }

    /// MultiTracker: `Dxx` is hexadecimal, `F00` is a no-op, and there is no queued
    /// sample swap.
    pub const fn multitracker() -> QuirkSet {
        QuirkSet {
            mod_break_parameter: BreakParameter::Hexadecimal,
            mod_f00_stops_song: false,
            protracker_sample_swap_at_boundary: false,
            ..QuirkSet::profile_default()
        }
    }

    /// Atari Octalyser (`CD61` / `CD81`).
    pub const fn octalyser() -> QuirkSet {
        QuirkSet { mod_pattern_loop: ModLoopDialect::Octalyser, ..QuirkSet::profile_default() }
    }

    /// Atari Digital Tracker (`FA04` / `FA06` / `FA08`).
    pub const fn digital_tracker() -> QuirkSet {
        QuirkSet { mod_pattern_loop: ModLoopDialect::DigitalTracker, ..QuirkSet::profile_default() }
    }

    /// Scream Tracker 3.21 — the S3M baseline.
    pub const fn scream_tracker_321() -> QuirkSet {
        QuirkSet { s3m_pattern_loop: S3mLoopDialect::ScreamTracker321, ..QuirkSet::profile_default() }
    }

    /// Scream Tracker 3.00 / 3.01 (`Cwt/v` below `0x1303`).
    pub const fn scream_tracker_301() -> QuirkSet {
        QuirkSet { s3m_pattern_loop: S3mLoopDialect::ScreamTracker301, ..QuirkSet::profile_default() }
    }

    /// ModPlug Tracker 1.16 / early OpenMPT (`Cwt/v` `0x1320` with the ModPlug marker).
    pub const fn modplug_116() -> QuirkSet {
        QuirkSet { s3m_pattern_loop: S3mLoopDialect::ModPlug116, ..QuirkSet::profile_default() }
    }

    /// Imago Orpheus (`Cwt/v` high nibble 2).
    pub const fn imago_orpheus() -> QuirkSet {
        QuirkSet { s3m_pattern_loop: S3mLoopDialect::ImagoOrpheus, ..QuirkSet::profile_default() }
    }
}

/// Which tracker a loader decided wrote a module, from its file header alone.
///
/// Stored in the module header by the loader and never revised afterwards. It is the
/// *evidence*, not the behaviour: [`FormatDialect::quirks`] is the mapping, and a host
/// that disagrees supplies a [`QuirkSelection::Override`] instead of editing this.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum FormatDialect {
    /// The file header said nothing this loader recognises. Canonical behaviour.
    #[default]
    Unknown,
    /// ProTracker 1.x / 2.x — MOD tags `M.K.` and `M!K!`.
    ProTracker,
    /// NoiseTracker — MOD tags `M&K!` and `N.T.`, libxmp's `TRACKER_NOISETRACKER`.
    /// Played as ProTracker except for its timing: NoiseTracker has no CIA mode, so this
    /// dialect is the one that carries [`ModTiming::VBlank`] out of the header alone
    /// (libxmp `tracker_is_vblank`, `src/loaders/mod_load.c:99-108`).
    Noisetracker,
    /// The ProTracker 3.x family — `LARD`, `NSMS`, `.M.K`. Played as ProTracker;
    /// recorded separately because libxmp's own timing heuristics key off it (they treat
    /// these tags as an *unknown* tracker, so they get no VBlank shortcut) and M5 may
    /// need to.
    ProTracker3,
    /// Atari Octalyser — `CD61`, `CD81`.
    Octalyser,
    /// Atari Digital Tracker — `FA04`, `FA06`, `FA08`.
    DigitalTracker,
    /// Startrekker — `FLT4`, `FLT8`.
    Startrekker,
    /// FastTracker, TakeTracker and friends — `xCHN`, `xxCH`, `TDZx`.
    FastTracker,
    /// MultiTracker `.MTM`.
    MultiTracker,
    /// Scream Tracker 3.21 and later, and everything that writes a `Cwt/v` StarPlayer
    /// does not recognise. The S3M default.
    ScreamTracker321,
    /// Scream Tracker 3.00 / 3.01 — `Cwt/v` `0x1xxx` below `0x1303`.
    ScreamTracker301,
    /// ModPlug Tracker 1.16 / early OpenMPT — `Cwt/v` `0x1320` with the ModPlug marker.
    ModPlug116,
    /// Imago Orpheus — `Cwt/v` high nibble 2.
    ImagoOrpheus,
}

impl FormatDialect {
    /// The [`QuirkSet`] this dialect implies, before any host override.
    pub const fn quirks(self) -> QuirkSet {
        match self {
            FormatDialect::Unknown => QuirkSet::profile_default(),
            FormatDialect::ProTracker | FormatDialect::ProTracker3 | FormatDialect::Startrekker | FormatDialect::FastTracker => QuirkSet::protracker(),
            FormatDialect::Noisetracker => QuirkSet::noisetracker(),
            FormatDialect::Octalyser => QuirkSet::octalyser(),
            FormatDialect::DigitalTracker => QuirkSet::digital_tracker(),
            FormatDialect::MultiTracker => QuirkSet::multitracker(),
            FormatDialect::ScreamTracker321 => QuirkSet::scream_tracker_321(),
            FormatDialect::ScreamTracker301 => QuirkSet::scream_tracker_301(),
            FormatDialect::ModPlug116 => QuirkSet::modplug_116(),
            FormatDialect::ImagoOrpheus => QuirkSet::imago_orpheus(),
        }
    }
}

/// Where a module's [`QuirkSet`] comes from — the precedence, expressed in the type.
///
/// A sequencer takes one of these and resolves it once, against the [`FormatDialect`] its
/// loader stored in the module header. There is no third source: no global, no
/// environment variable, and no mid-playback setter.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum QuirkSelection {
    /// Take the quirks the loader-detected dialect implies. The default.
    #[default]
    FromDialect,
    /// Ignore the dialect and use these quirks. A host that knows better than the file
    /// header — a compatibility menu, a regression test — supplies this.
    ///
    /// It replaces **every** field, format-specific ones included, so to change one thing
    /// start from the dialect's own set rather than from a profile:
    /// `QuirkSet { tempo_model: TempoModelId::St3Truncating, ..FormatDialect::MultiTracker.quirks() }`
    /// keeps MultiTracker's hexadecimal `Dxx`, where `QuirkSet::starplayer_classic()`
    /// would silently give that module ProTracker's.
    Override(QuirkSet),
}

impl QuirkSelection {
    /// Resolve against the dialect a loader detected.
    pub const fn resolve(self, dialect: FormatDialect) -> QuirkSet {
        match self {
            QuirkSelection::FromDialect => dialect.quirks(),
            QuirkSelection::Override(quirks) => quirks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accuracy policy §2 lists exactly two quirks, and only the first is a field: the
    /// truncating tick length. Every other field must be identical, so a new quirk cannot
    /// be added to `starplayer_classic()` without this test — and therefore the policy —
    /// being updated.
    #[test]
    fn canonical_and_classic_differ_only_where_the_policy_says_so() {
        let canonical = QuirkSet::canonical();
        let classic = QuirkSet::starplayer_classic();
        assert_eq!(canonical.tempo_model, TempoModelId::ExactFixedPoint);
        assert_eq!(classic.tempo_model, TempoModelId::St3Truncating, "policy section 2 row 1: the original's double-truncated tick length");
        assert_eq!(canonical.s3m_pattern_loop, classic.s3m_pattern_loop);
        assert_eq!(canonical.mod_pattern_loop, classic.mod_pattern_loop);
        assert_eq!(canonical.mod_paula_clock, classic.mod_paula_clock);
        assert_eq!(canonical.mod_break_parameter, classic.mod_break_parameter);
        assert_eq!(canonical.mod_timing, classic.mod_timing);
        assert_eq!(canonical.mod_f00_stops_song, classic.mod_f00_stops_song);
        assert_eq!(canonical.protracker_sample_swap_at_boundary, classic.protracker_sample_swap_at_boundary);
        assert_eq!(canonical.protracker_tremolo_ramp_from_vibrato_phase, classic.protracker_tremolo_ramp_from_vibrato_phase);
        // And the whole struct differs in that one field and no other.
        assert_eq!(QuirkSet { tempo_model: TempoModelId::ExactFixedPoint, ..classic }, canonical);
    }

    #[test]
    fn the_default_quirk_set_is_the_canonical_profile() {
        assert_eq!(QuirkSet::default(), QuirkSet::canonical(), "every field's Default is the canonical value");
    }

    #[test]
    fn a_host_override_wins_over_the_loader_detected_dialect() {
        let classic = QuirkSet::starplayer_classic();
        assert_eq!(QuirkSelection::FromDialect.resolve(FormatDialect::Octalyser), QuirkSet::octalyser());
        assert_eq!(QuirkSelection::Override(classic).resolve(FormatDialect::Octalyser), classic, "an explicit QuirkSet ignores the dialect");
        assert_eq!(QuirkSelection::default(), QuirkSelection::FromDialect);
    }

    #[test]
    fn every_dialect_maps_to_the_quirk_set_it_names() {
        assert_eq!(FormatDialect::Unknown.quirks(), QuirkSet::profile_default());
        assert_eq!(FormatDialect::ProTracker.quirks().mod_pattern_loop, ModLoopDialect::ProTracker);
        assert_eq!(FormatDialect::Octalyser.quirks().mod_pattern_loop, ModLoopDialect::Octalyser);
        assert_eq!(FormatDialect::DigitalTracker.quirks().mod_pattern_loop, ModLoopDialect::DigitalTracker);
        assert_eq!(FormatDialect::ProTracker.quirks().mod_timing, ModTiming::Cia);
        assert_eq!(FormatDialect::Noisetracker.quirks().mod_timing, ModTiming::VBlank, "NoiseTracker has no CIA timer");
        assert_eq!(FormatDialect::ProTracker3.quirks().mod_timing, ModTiming::Cia, "LARD and NSMS are an unknown tracker to libxmp, not NoiseTracker");
        assert_eq!(FormatDialect::Noisetracker.quirks(), QuirkSet { mod_timing: ModTiming::VBlank, ..QuirkSet::protracker() }, "the VBlank tick is the only thing NoiseTracker changes");
        assert_eq!(FormatDialect::MultiTracker.quirks().mod_break_parameter, BreakParameter::Hexadecimal);
        assert!(!FormatDialect::MultiTracker.quirks().mod_f00_stops_song);
        assert!(!FormatDialect::MultiTracker.quirks().protracker_sample_swap_at_boundary);
        assert_eq!(FormatDialect::ScreamTracker321.quirks().s3m_pattern_loop, S3mLoopDialect::ScreamTracker321);
        assert_eq!(FormatDialect::ScreamTracker301.quirks().s3m_pattern_loop, S3mLoopDialect::ScreamTracker301);
        assert_eq!(FormatDialect::ModPlug116.quirks().s3m_pattern_loop, S3mLoopDialect::ModPlug116);
        assert_eq!(FormatDialect::ImagoOrpheus.quirks().s3m_pattern_loop, S3mLoopDialect::ImagoOrpheus);
    }

    /// Field for field against libxmp `src/common.h:353-443`.
    #[test]
    fn the_flow_tables_match_libxmps_flow_mode_constants() {
        let st321 = S3mLoopDialect::ScreamTracker321.flow();
        assert!(st321.global_target && st321.global_count && st321.pattern_reset && st321.end_advances && st321.jump_keeps_break_row);
        assert!(st321.delay_break && st321.delay_jump && st321.unset_break && st321.unset_jump);
        assert!(!st321.init_same_row && !st321.end_cancels && !st321.one_at_a_time && !st321.shared_break && !st321.first_effect_only);

        let st301 = S3mLoopDialect::ScreamTracker301.flow();
        assert!(st301.global_target && st301.global_count && st301.pattern_reset && st301.end_advances && st301.init_same_row && st301.jump_keeps_break_row);
        assert!(!st301.delay_break && !st301.delay_jump && !st301.unset_break && !st301.unset_jump);

        let modplug = S3mLoopDialect::ModPlug116.flow();
        assert!(modplug.one_at_a_time && modplug.delay_break && modplug.delay_jump && modplug.unset_break && modplug.unset_jump && modplug.jump_keeps_break_row);
        assert!(!modplug.global_target && !modplug.global_count && !modplug.pattern_reset && !modplug.end_advances);

        let orpheus = S3mLoopDialect::ImagoOrpheus.flow();
        assert!(orpheus.pattern_reset && orpheus.shared_break && orpheus.unset_break);
        assert!(!orpheus.global_target && !orpheus.global_count && !orpheus.end_advances && !orpheus.unset_jump && !orpheus.jump_keeps_break_row);

        assert_eq!(ModLoopDialect::ProTracker.flow(), PatternFlow::generic());

        let octalyser = ModLoopDialect::Octalyser.flow();
        assert!(octalyser.global_target && octalyser.global_count && octalyser.ignore_target_while_looping && octalyser.end_cancels);
        assert!(octalyser.delay_break && octalyser.delay_jump && octalyser.unset_break && octalyser.unset_jump);
        assert!(!octalyser.pattern_reset && !octalyser.end_advances && !octalyser.first_effect_only && !octalyser.jump_keeps_break_row);

        let digital = ModLoopDialect::DigitalTracker.flow();
        assert!(digital.global_target && digital.global_count && digital.first_effect_only);
        assert!(digital.delay_break && digital.delay_jump && digital.unset_break && digital.unset_jump);
        assert!(!digital.pattern_reset && !digital.end_advances && !digital.end_cancels && !digital.jump_keeps_break_row);
    }

    #[test]
    fn the_two_paula_clocks_are_the_amiga_video_rates() {
        assert_eq!(PaulaClock::Pal.hz(), 3_546_895);
        assert_eq!(PaulaClock::Ntsc.hz(), 3_579_545);
        assert_eq!(PaulaClock::default(), PaulaClock::Pal);
    }

    #[cfg(feature = "quirks-starplayer")]
    #[test]
    fn the_feature_selects_the_classic_profile_for_every_dialect() {
        assert_eq!(QuirkSet::profile_default(), QuirkSet::starplayer_classic());
        assert_eq!(FormatDialect::ProTracker.quirks().tempo_model, TempoModelId::St3Truncating);
        assert_eq!(FormatDialect::ScreamTracker321.quirks().tempo_model, TempoModelId::St3Truncating);
    }

    #[cfg(not(feature = "quirks-starplayer"))]
    #[test]
    fn without_the_feature_every_dialect_keeps_the_canonical_tick_length() {
        assert_eq!(QuirkSet::profile_default(), QuirkSet::canonical());
        assert_eq!(FormatDialect::ImagoOrpheus.quirks().tempo_model, TempoModelId::ExactFixedPoint);
    }
}

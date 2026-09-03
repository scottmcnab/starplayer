//! [`NativeSequencer`] — the one place a `Module` becomes a playing sequencer.
//!
//! Every host has the same problem: it holds an `Arc<Module>` whose format is a runtime
//! value, and it needs the [`PatternSequencer`] for that format. The type is different per
//! format — the processor and the pattern decoder are generic parameters, deliberately, so
//! that no format is lowered into another (AGENTS.md design goal 7) — so the dispatch has
//! to happen somewhere. It happens here, once, instead of in every host.
//!
//! `Engine` still stores `Box<dyn EventSource>` and its command handler still cannot seek:
//! that is the architecture's choice (§1.2, §3), and this enum does not change it. What it
//! changes is that the *host-side* bridge — the thing that owns the typed sequencer and
//! forwards seeks to it — is written once and shared, so a new format is one arm here and
//! a new host is a consumer of one type.
//!
//! The dispatch is a `match` per call. Every method on it is called at tick rate at worst,
//! around 50 Hz, and never inside the mixer's inner loop.

use starplayer_core::quirks::QuirkSelection;
use starplayer_core::{AtEnd, Error, Frame};
use starplayer_engine::{EngineContext, EventSource, RowMark, SequencerSettings, SongTimeline};
use starplayer_model::{Module, ModuleFormat};
use starplayer_rt::Arc;

#[cfg(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm"))]
use starplayer_core::TempoModelId;
#[cfg(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm"))]
use starplayer_engine::{PatternData, PatternSequencer, TrackerProcessor};

/// The S3M playback sequencer, exactly as [`starplayer_s3m::sequencer_with_quirks`] builds it.
#[cfg(feature = "s3m")]
pub type S3mSequencer = PatternSequencer<TempoModelId, starplayer_s3m::S3mProcessor, starplayer_s3m::S3mPatternData>;
/// The MOD playback sequencer, exactly as [`starplayer_mod::sequencer_with_quirks`] builds it.
#[cfg(feature = "mod")]
pub type ModSequencer = PatternSequencer<TempoModelId, starplayer_mod::ModProcessor, starplayer_mod::ModPatternData>;
/// The MTM playback sequencer, exactly as [`starplayer_mtm::sequencer_with_quirks`] builds it.
#[cfg(feature = "mtm")]
pub type MtmSequencer = PatternSequencer<TempoModelId, starplayer_mtm::MtmProcessor, starplayer_mtm::MtmPatternData>;
/// The XM playback sequencer, exactly as [`starplayer_xm::sequencer_with_quirks`] builds it.
#[cfg(feature = "xm")]
pub type XmSequencer = PatternSequencer<TempoModelId, starplayer_xm::XmProcessor, starplayer_xm::XmPatternData>;

/// A playing sequencer for whichever format the module turned out to be.
///
/// One arm per format crate compiled into this facade, so a build with `mtm` off has no
/// MTM arm and [`NativeSequencer::new`] reports the module as unsupported rather than
/// failing to compile. A build with every format off is a legal, uninhabited enum: the
/// facade still compiles, and every constructor returns `Err`.
///
/// It implements [`EventSource`], so a host hands it straight to
/// [`Engine::set_source`](starplayer_engine::Engine::set_source) — or wraps it, as the web
/// player does, to give itself a seek mailbox the engine's typed command ring cannot carry.
pub enum NativeSequencer {
    /// Scream Tracker 3.
    #[cfg(feature = "s3m")]
    S3m(S3mSequencer),
    /// ProTracker.
    #[cfg(feature = "mod")]
    Mod(ModSequencer),
    /// MultiTracker.
    #[cfg(feature = "mtm")]
    Mtm(MtmSequencer),
    /// FastTracker 2.
    #[cfg(feature = "xm")]
    Xm(XmSequencer),
    // The IT arm arrives with M6-G5.
}

/// The error every unsupported format reports, worded as [`crate::scan_song`] has always
/// worded it so a host's message does not change with this dispatch.
const NO_NATIVE_PROCESSOR: Error = Error::Invalid("no native processor for this module format");

/// Forward one shared method body to whichever arm is live.
///
/// Two details are load-bearing, and both are about the build with **every** format
/// feature off — legal, and checked by CI:
///
/// * `match *self` rather than `match self`, because an arm-less match is only accepted on
///   a place expression of an uninhabited type, never on a reference to one;
/// * the parenthesised argument list, whose only job is to consume the method's parameters
///   so that a body which expands to nothing does not leave them unused.
macro_rules! forward {
    ($self:expr, ($($argument:expr),*), |$sequencer:ident| $body:expr) => {{
        $(let _ = &$argument;)*
        match *$self {
            #[cfg(feature = "s3m")]
            NativeSequencer::S3m(ref $sequencer) => $body,
            #[cfg(feature = "mod")]
            NativeSequencer::Mod(ref $sequencer) => $body,
            #[cfg(feature = "mtm")]
            NativeSequencer::Mtm(ref $sequencer) => $body,
            #[cfg(feature = "xm")]
            NativeSequencer::Xm(ref $sequencer) => $body,
        }
    }};
}

macro_rules! forward_mut {
    ($self:expr, ($($argument:expr),*), |$sequencer:ident| $body:expr) => {{
        $(let _ = &$argument;)*
        match *$self {
            #[cfg(feature = "s3m")]
            NativeSequencer::S3m(ref mut $sequencer) => $body,
            #[cfg(feature = "mod")]
            NativeSequencer::Mod(ref mut $sequencer) => $body,
            #[cfg(feature = "mtm")]
            NativeSequencer::Mtm(ref mut $sequencer) => $body,
            #[cfg(feature = "xm")]
            NativeSequencer::Xm(ref mut $sequencer) => $body,
        }
    }};
}

impl NativeSequencer {
    /// The playback sequencer for `module`'s format, built exactly as that format crate's
    /// `sequencer_with_quirks` builds it: the selection is resolved once against the
    /// loader-detected dialect and supplies both the effect quirks and the tempo model.
    ///
    /// A host that has scanned the song passes `QuirkSelection::Override(scanned.quirks)`,
    /// never `FromDialect` — see the crate documentation for why the two must agree.
    ///
    /// Off the audio thread: it allocates the processor's per-channel state.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> Result<NativeSequencer, Error> {
        let _ = (&module, sample_rate_hz, quirks);
        match module.header().format {
            #[cfg(feature = "s3m")]
            ModuleFormat::S3m => Ok(NativeSequencer::S3m(starplayer_s3m::sequencer_with_quirks(module, sample_rate_hz, quirks))),
            #[cfg(feature = "mod")]
            ModuleFormat::Mod => Ok(NativeSequencer::Mod(starplayer_mod::sequencer_with_quirks(module, sample_rate_hz, quirks))),
            #[cfg(feature = "mtm")]
            ModuleFormat::Mtm => Ok(NativeSequencer::Mtm(starplayer_mtm::sequencer_with_quirks(module, sample_rate_hz, quirks))),
            #[cfg(feature = "xm")]
            ModuleFormat::Xm => Ok(NativeSequencer::Xm(starplayer_xm::sequencer_with_quirks(module, sample_rate_hz, quirks))),
            _ => Err(NO_NATIVE_PROCESSOR),
        }
    }

    /// [`NativeSequencer::new`] with the [`SequencerSettings`] spelled out.
    ///
    /// The one caller that needs this is the offline trace, which wants
    /// [`EndOfSongPolicy::Stop`](starplayer_engine::EndOfSongPolicy::Stop) so a capture
    /// ends with the order list instead of looping forever. Everything else — the speed,
    /// the tempo, the restart order — should be read off the module header, exactly as
    /// each format crate's private `sequencer_settings` reads it, or the sequencer starts
    /// on timing the module never asked for.
    ///
    /// The processor is built at `settings.sample_rate_hz`, so the settings are the single
    /// source of the rate.
    pub fn with_settings(module: Arc<Module>, quirks: QuirkSelection, settings: SequencerSettings) -> Result<NativeSequencer, Error> {
        let rate = settings.sample_rate_hz;
        let _ = (&module, quirks, settings, rate);
        match module.header().format {
            #[cfg(feature = "s3m")]
            ModuleFormat::S3m => {
                let resolved = quirks.resolve(module.header().dialect);
                let processor = starplayer_s3m::S3mProcessor::with_quirks(Arc::clone(&module), rate, QuirkSelection::Override(resolved));
                let data = starplayer_s3m::S3mPatternData(module);
                Ok(NativeSequencer::S3m(PatternSequencer::new(resolved.tempo_model, data, processor, settings)))
            }
            #[cfg(feature = "mod")]
            ModuleFormat::Mod => {
                let resolved = quirks.resolve(module.header().dialect);
                let semantics = starplayer_mod::EffectSemantics::ProTracker;
                let processor = starplayer_mod::ModProcessor::with_semantics_and_quirks(Arc::clone(&module), rate, semantics, QuirkSelection::Override(resolved));
                let data = starplayer_mod::ModPatternData(module);
                Ok(NativeSequencer::Mod(PatternSequencer::new(resolved.tempo_model, data, processor, settings)))
            }
            #[cfg(feature = "mtm")]
            ModuleFormat::Mtm => {
                // An MTM *is* the MultiTracker dialect by construction, so it resolves
                // against that rather than against the header — `sequencer_with_quirks`
                // does the same, and a header a test assembled by hand is still an MTM.
                let resolved = quirks.resolve(starplayer_core::quirks::FormatDialect::MultiTracker);
                let processor = starplayer_mtm::MtmProcessor::with_quirks(Arc::clone(&module), rate, QuirkSelection::Override(resolved));
                let data = starplayer_mtm::MtmPatternData(module);
                Ok(NativeSequencer::Mtm(PatternSequencer::new(resolved.tempo_model, data, processor, settings)))
            }
            #[cfg(feature = "xm")]
            ModuleFormat::Xm => {
                let resolved = quirks.resolve(module.header().dialect);
                let processor = starplayer_xm::XmProcessor::with_quirks(Arc::clone(&module), rate, QuirkSelection::Override(resolved));
                let data = starplayer_xm::XmPatternData(module);
                Ok(NativeSequencer::Xm(PatternSequencer::new(resolved.tempo_model, data, processor, settings)))
            }
            _ => Err(NO_NATIVE_PROCESSOR),
        }
    }

    /// Which arm this is.
    pub fn format(&self) -> ModuleFormat {
        match *self {
            #[cfg(feature = "s3m")]
            NativeSequencer::S3m(_) => ModuleFormat::S3m,
            #[cfg(feature = "mod")]
            NativeSequencer::Mod(_) => ModuleFormat::Mod,
            #[cfg(feature = "mtm")]
            NativeSequencer::Mtm(_) => ModuleFormat::Mtm,
            #[cfg(feature = "xm")]
            NativeSequencer::Xm(_) => ModuleFormat::Xm,
        }
    }

    /// The output rate the tick clock converts against.
    pub fn sample_rate_hz(&self) -> u32 { forward!(self, (), |sequencer| sequencer.sample_rate_hz()) }

    /// How many voices this module's processor wants the pool to hold.
    ///
    /// The processor's own answer, given the module's channel count — see
    /// [`TrackerProcessor::recommended_voice_capacity`]. A per-module host sizes its pool
    /// from this; a persistent host that plays every module through one engine takes
    /// [`MAX_VOICE_CAPACITY`](crate::MAX_VOICE_CAPACITY) instead, and
    /// [`crate::recommended_voice_capacity`] answers the same question without building a
    /// sequencer at all.
    pub fn recommended_voice_capacity(&self) -> usize {
        forward!(self, (), |sequencer| {
            let channel_count = sequencer.data().channel_count() as usize;
            sequencer.processor().recommended_voice_capacity(channel_count)
        })
    }

    /// Jump to a row of the pattern already playing.
    pub fn seek_row(&mut self, row: u16) { forward_mut!(self, (row), |sequencer| sequencer.seek_row(row)) }

    /// Jump to an order-list index and rebase the song clock so `now` reads as the elapsed
    /// position the scan recorded for it. Reports whether the index resolved.
    pub fn seek_order_at(&mut self, order: u16, now: Frame) -> bool {
        forward_mut!(self, (order, now), |sequencer| sequencer.seek_order_at(order, now))
    }

    /// Seek to an elapsed position in the current pass and report the row it landed on.
    /// Needs an installed timeline; `None` means there was none, or no such position.
    pub fn seek_frame(&mut self, song_frame: u64, now: Frame) -> Option<RowMark> {
        forward_mut!(self, (song_frame, now), |sequencer| sequencer.seek_frame(song_frame, now))
    }

    /// Restart the tick clock at `frame`, keeping the song position. A host follows every
    /// seek with this, at the same frame.
    pub fn restart_clock_at(&mut self, frame: Frame) { forward_mut!(self, (frame), |sequencer| sequencer.restart_clock_at(frame)) }

    /// Install the scanned shape of this song. Off the audio thread.
    pub fn set_timeline(&mut self, timeline: SongTimeline) { forward_mut!(self, (timeline), |sequencer| sequencer.set_timeline(timeline)) }

    /// Choose what happens at the detected loop point.
    pub fn set_at_end(&mut self, at_end: AtEnd) { forward_mut!(self, (at_end), |sequencer| sequencer.set_at_end(at_end)) }

    /// How far into the song `now` is, in frames — what a progress slider draws.
    pub fn song_frame(&self, now: Frame) -> u64 { forward!(self, (now), |sequencer| sequencer.song_frame(now)) }

    /// One pass of the song in frames, when a timeline says how long that is.
    pub fn song_length_frames(&self) -> Option<u64> { forward!(self, (), |sequencer| sequencer.song_length_frames()) }

    /// Whether the song has been heard through once — its detected loop point, or the end
    /// of its order list (tasks D1 and D2).
    pub fn end_reached(&self) -> bool { forward!(self, (), |sequencer| sequencer.end_reached()) }
}

impl EventSource for NativeSequencer {
    fn next_event_frame(&self) -> Option<Frame> { forward!(self, (), |sequencer| sequencer.next_event_frame()) }

    fn advance_to(&mut self, frame: Frame) { forward_mut!(self, (frame), |sequencer| sequencer.advance_to(frame)) }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        forward_mut!(self, (frame, context), |sequencer| sequencer.dispatch(frame, context))
    }
}

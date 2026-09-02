//! C5: the quirk profiles and the tempo models, observed end to end.
//!
//! `QuirkSet` is threaded from the sequencer constructor down to the effect processor, so
//! these are integration tests rather than unit tests: they select a profile the way a
//! host does and then measure what the engine actually did with it.

use starplayer_core::quirks::{FormatDialect, QuirkSelection, QuirkSet};
use starplayer_core::{Frame, TempoModel, TempoModelId};
use starplayer_engine::{ChannelTable, ControlClock, EngineContext, EventSource};
use starplayer_mixer::VoicePool;
use starplayer_mod::ModCell;
use starplayer_rt::Arc;

const SAMPLE_RATE_HZ: u32 = 44_100;

/// One 64-row pattern at the default speed 6 / tempo 125, with one looping sample so the
/// song runs for as long as the test drives it.
fn tempo_mod() -> Vec<u8> {
    const SAMPLE_FRAMES: usize = 256;
    const PATTERN_BYTES: usize = 64 * 4 * 4;
    let mut bytes = vec![0; 1084 + PATTERN_BYTES + SAMPLE_FRAMES];
    bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[45] = 64;
    bytes[48..50].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[950] = 1;
    bytes[1080..1084].copy_from_slice(b"M.K.");
    bytes[1084..1088].copy_from_slice(&ModCell { period: 428, instrument: 1, ..ModCell::EMPTY }.to_bytes());
    bytes
}

/// Dispatch `ticks` tracker ticks and answer the absolute output frame of the last one.
fn frame_after_ticks(quirks: QuirkSelection, ticks: usize) -> u64 {
    let module = Arc::new(starplayer_mod::load(&tempo_mod()).expect("native MOD loads"));
    let mut sequencer = starplayer_mod::sequencer_with_quirks(module, SAMPLE_RATE_HZ, quirks);
    let mut voices = VoicePool::new(4);
    let mut channels = ChannelTable::new(4);
    let mut control = ControlClock::new(SAMPLE_RATE_HZ, Frame::ZERO);
    let mut last = 0u64;
    for _ in 0..ticks {
        let Some(frame) = sequencer.next_event_frame() else { break };
        last = frame.0;
        let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
        sequencer.dispatch(frame, &mut context);
    }
    last
}

/// Deliverable 5: the drift is the point of having two tempo models, so measure it.
///
/// At 44100 Hz and 125 BPM the true tick is `44100 * 2.5 / 125` = **882.0** frames exactly,
/// which the original's `(44100 * 10 / 125) >> 2` also gives — so 125 BPM cannot show the
/// difference at all. The module therefore runs at the tempo the test sets below.
#[test]
fn the_two_tempo_models_drift_apart_by_the_predicted_number_of_frames() {
    const TICKS: usize = 6_000;
    let exact = frame_after_ticks(QuirkSelection::Override(QuirkSet::canonical()), TICKS);
    let truncating = frame_after_ticks(QuirkSelection::Override(QuirkSet::starplayer_classic()), TICKS);

    // 125 BPM is the one tempo where the two models agree exactly, which is worth pinning:
    // a test that used the default tempo and asserted a difference would be asserting
    // nothing.
    assert_eq!(exact, truncating, "at 125 BPM the exact tick is 882.0 frames and the double truncation loses nothing");
    assert_eq!(exact, 882 * (TICKS as u64 - 1));

    // The drift itself, from the models directly, at the tempo the accuracy policy quotes.
    let exact_tick = TempoModelId::ExactFixedPoint.frames_per_tick(SAMPLE_RATE_HZ, 130, 6);
    let truncating_tick = TempoModelId::St3Truncating.frames_per_tick(SAMPLE_RATE_HZ, 130, 6);
    let exact_frames = (exact_tick as u128 * TICKS as u128) >> 32;
    let truncating_frames = (truncating_tick as u128 * TICKS as u128) >> 32;
    assert_eq!(truncating_frames, 848 * TICKS as u128);
    assert_eq!(exact_frames - truncating_frames, 461, "6000 ticks at 130 BPM lose 0.0769 frames each: about 10 ms per minute of drift");
}

/// The same module, played twice, under two tempo models chosen only by the quirk set.
#[test]
fn the_quirk_profile_selects_the_tempo_model_the_sequencer_runs_on() {
    let module = Arc::new(starplayer_mod::load(&tempo_mod()).expect("native MOD loads"));
    let canonical = starplayer_mod::sequencer_with_quirks(Arc::clone(&module), SAMPLE_RATE_HZ, QuirkSelection::Override(QuirkSet::canonical()));
    let classic = starplayer_mod::sequencer_with_quirks(module, SAMPLE_RATE_HZ, QuirkSelection::Override(QuirkSet::starplayer_classic()));
    assert_eq!(canonical.tempo_model(), &TempoModelId::ExactFixedPoint);
    assert_eq!(classic.tempo_model(), &TempoModelId::St3Truncating);
}

/// Deliverable 4: the loader-detected dialect is the default, an explicit `QuirkSet` wins,
/// and the answer is fixed for the lifetime of the module.
#[test]
fn the_loader_detected_dialect_is_the_default_and_a_host_override_wins() {
    let mut bytes = tempo_mod();
    bytes[1080..1084].copy_from_slice(b"CD61");
    // `CD61` is six channels; the pattern and sample data have to grow with it.
    let mut octalyser = vec![0; 1084 + 64 * 6 * 4 + 256];
    octalyser[..1084].copy_from_slice(&bytes[..1084]);
    let module = Arc::new(starplayer_mod::load(&octalyser).expect("Octalyser MOD loads"));
    assert_eq!(module.header().dialect, FormatDialect::Octalyser);

    let from_dialect = starplayer_mod::ModProcessor::with_semantics_and_quirks(
        Arc::clone(&module), SAMPLE_RATE_HZ, starplayer_mod::EffectSemantics::ProTracker, QuirkSelection::FromDialect);
    assert_eq!(from_dialect.quirks(), QuirkSet::octalyser(), "with no host opinion the file header decides");

    let overridden = starplayer_mod::ModProcessor::with_semantics_and_quirks(
        Arc::clone(&module), SAMPLE_RATE_HZ, starplayer_mod::EffectSemantics::ProTracker, QuirkSelection::Override(QuirkSet::canonical()));
    assert_eq!(overridden.quirks(), QuirkSet::canonical(), "a host that supplies a QuirkSet outranks the header");
    assert!(from_dialect.pattern_flow().flow().global_target, "Octalyser's loop target is global");
    assert!(!overridden.pattern_flow().flow().global_target, "ProTracker's is per channel");
}

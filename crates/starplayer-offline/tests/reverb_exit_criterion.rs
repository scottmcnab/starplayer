//! **M7's exit criterion**: a reverb on channel 1 of `reflex.s3m` alone, audible and
//! correct (M7 master plan, "Exit criteria"; M7-H4 deliverable 4).
//!
//! Two claims, and each needs the other to mean anything:
//!
//! 1. **It is audible.** Channel 1's bus carries a reverb tail — measured after the
//!    channel has been silenced, where every sample of the output is tail and nothing
//!    else, so no part of the measurement is the dry signal leaking into it.
//! 2. **It is on channel 1 and nowhere else.** Every other channel's output is
//!    *byte-identical* to the same render with no insert installed at all. Not "sounds the
//!    same": the same bytes, which is the only version of this claim that a per-channel bus
//!    graph can be held to.
//!
//! # How a single channel's output is obtained
//!
//! By soloing: every other lane's `muted` flag is set before the first frame is rendered.
//! `VoicePool::accumulate_masked` checks `is_muted` *before* it looks for a bus (M7-H1), so
//! a muted channel's voices never reach one — they render into the scratch buffer that
//! keeps their position, loops and ramps advancing — and the mix is exactly the soloed
//! channel's bus. The flags are set on `Engine::channels_mut` directly rather than sent as
//! `Command::MuteChannel`s, so that they are in force from frame zero and not from whenever
//! the command ring happened to drain.
//!
//! # Why muting is the note-off
//!
//! The tail is measured over the 200 ms after channel 1 is muted part way through the song.
//! Muting removes the channel's voices from its bus at an exact, chosen frame while leaving
//! the chain running, which is precisely a note-off as the *bus* sees one — and unlike
//! hunting for a gap in the music it does not depend on what `reflex.s3m` happens to be
//! playing. The no-insert render of the same scenario is exactly zero over that window,
//! which is what makes the reverb render's level over it a measurement of the tail alone.

use starplayer::core::{ChannelId, ExactFixedPoint};
use starplayer::dsp::effects::reverb::{REVERB_MIX_PARAM, REVERB_ROOM_PARAM};
use starplayer::dsp::{InsertKind, Linear, build_insert};
use starplayer::engine::{Engine, EngineSettings, InsertTarget, RENDER_QUANTUM};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::Module;
use starplayer::rt::Arc;

/// The fixture the exit criterion names, and one of the eleven goldens.
const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

/// The golden contract's rate, so the numbers here are the ones the owner will hear.
const SAMPLE_RATE_HZ: u32 = 44_100;

/// The channel the reverb goes on. The exit criterion names it.
const REVERB_CHANNEL: u16 = 1;

/// The mix the exit criterion names.
const REVERB_MIX_PERCENT: i32 = 50;

/// Frames of music before channel 1 is silenced: four seconds, comfortably past the point
/// where every channel of `reflex.s3m` has played something.
///
/// Rounded **down to a whole number of render quanta**, so that the engine's output ring is
/// empty when the mute is applied and the tail window holds only frames rendered after it.
/// Without that the ring would hand the tail window up to a quantum of music rendered
/// before the note-off, and the "exactly silent" claim below would be about the wrong
/// frames.
const FRAMES_BEFORE_THE_NOTE_OFF: usize = (4 * SAMPLE_RATE_HZ as usize) / RENDER_QUANTUM * RENDER_QUANTUM;

/// Frames the tail is measured over: the 200 ms the deliverable names, likewise rounded to
/// whole quanta.
const TAIL_FRAMES: usize = (SAMPLE_RATE_HZ as usize / 5).div_ceil(RENDER_QUANTUM) * RENDER_QUANTUM;

/// The floor the tail has to clear, in dBFS.
const TAIL_FLOOR_DBFS: f64 = -40.0;

type ExitEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;

/// Render `reflex.s3m` with only `solo` audible, optionally with a reverb on
/// [`REVERB_CHANNEL`]'s bus, silencing `solo` after [`FRAMES_BEFORE_THE_NOTE_OFF`] and
/// carrying on for [`TAIL_FRAMES`] more.
fn render_solo(solo: u16, with_reverb: bool) -> Vec<i16> {
    let module = Arc::new(starplayer::s3m::load(REFLEX).expect("REFLEX loads"));
    let channel_count = (module.header().channel_count as usize).max(1);
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: starplayer::recommended_voice_capacity(&module).max(1),
        ..EngineSettings::default()
    };
    let mut engine: ExitEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("a fresh engine owns its control handle");
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");
    engine.set_source(Box::new(starplayer::s3m::sequencer_for(Arc::clone(&module), SAMPLE_RATE_HZ, ExactFixedPoint)));

    if with_reverb {
        let mut reverb = build_insert::<i32>(InsertKind::Reverb, SAMPLE_RATE_HZ);
        reverb.set_param(REVERB_MIX_PARAM, REVERB_MIX_PERCENT);
        // A large room, so the tail the exit criterion asks about is a tail and not a slap.
        reverb.set_param(REVERB_ROOM_PARAM, 80);
        reverb.reset();
        inserts.install(InsertTarget::Channel(ChannelId(REVERB_CHANNEL)), 0, reverb).map_err(|_| "full").expect("the ring has room");
    }

    // Solo, before the first frame: every lane but `solo` is muted where it stands.
    for channel in 0..channel_count {
        if let Some(lane) = engine.channels_mut().get_mut(ChannelId(channel as u16)) {
            lane.muted = channel as u16 != solo;
        }
    }

    let mut output = vec![0i16; (FRAMES_BEFORE_THE_NOTE_OFF + TAIL_FRAMES) * 2];
    let split = FRAMES_BEFORE_THE_NOTE_OFF * 2;
    if let Some(music) = output.get_mut(..split) {
        engine.render(music);
    }
    // The note-off: the soloed channel leaves its bus, and whatever the chain is holding
    // keeps sounding.
    if let Some(lane) = engine.channels_mut().get_mut(ChannelId(solo)) {
        lane.muted = true;
    }
    if let Some(tail) = output.get_mut(split..) {
        engine.render(tail);
    }

    assert!(!engine.warnings().any(), "channel {solo} raised an engine warning: {:?}", engine.warnings());
    control.collect_all_garbage();
    inserts.collect_all_garbage();
    output
}

/// The RMS of `samples` in dBFS against the `i16` full scale.
fn level_dbfs(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return -200.0;
    }
    let energy: f64 = samples.iter().map(|value| (*value as f64) * (*value as f64)).sum();
    let rms = (energy / samples.len() as f64).sqrt();
    if rms <= 0.0 { -200.0 } else { 20.0 * (rms / 32_768.0).log10() }
}

/// **The exit criterion.** A reverb on channel 1 of `reflex.s3m`, at 50 % mix, leaves a
/// tail on channel 1's bus and leaves every other channel's output bit-identical.
#[test]
fn a_reverb_on_channel_one_of_reflex_is_audible_there_and_nowhere_else() {
    let module = starplayer::s3m::load(REFLEX).expect("REFLEX loads");
    let channel_count = (module.header().channel_count as usize).max(1);
    assert!(channel_count > REVERB_CHANNEL as usize, "REFLEX has to have a channel 1 to put a reverb on");

    let mut audible_channels = 0usize;
    for channel in 0..channel_count as u16 {
        let plain = render_solo(channel, false);
        let reverberated = render_solo(channel, true);
        if plain.iter().any(|sample| *sample != 0) {
            audible_channels += 1;
        }

        if channel == REVERB_CHANNEL {
            assert_ne!(plain, reverberated, "the reverb has to actually change channel {channel}");
            continue;
        }
        // Byte-identical, not "close": an insert on one bus may not reach another one.
        assert_eq!(plain.len(), reverberated.len());
        let first_difference = plain.iter().zip(reverberated.iter()).position(|(left, right)| left != right);
        assert_eq!(first_difference, None, "channel {channel} changed at sample {first_difference:?} because of an insert on channel {REVERB_CHANNEL}");
    }
    assert!(audible_channels >= 2, "only {audible_channels} of REFLEX's channels made a sound, so the comparison proves nothing");
}

/// The tail itself: over the 200 ms after channel 1 is silenced, the bus with no insert is
/// **exactly** silent and the bus with the reverb is above −40 dBFS.
#[test]
fn channel_ones_bus_has_a_tail_after_its_last_note() {
    let plain = render_solo(REVERB_CHANNEL, false);
    let reverberated = render_solo(REVERB_CHANNEL, true);
    let split = FRAMES_BEFORE_THE_NOTE_OFF * 2;

    let dry_before = plain.get(..split).expect("the music is there");
    assert!(dry_before.iter().any(|sample| *sample != 0), "channel {REVERB_CHANNEL} of REFLEX has to make a sound in the first place");

    let dry_tail = plain.get(split..).expect("the tail window is there");
    assert!(dry_tail.iter().all(|sample| *sample == 0), "a muted channel with no insert is not the silence the measurement needs");

    let wet_tail = reverberated.get(split..).expect("the tail window is there");
    let level = level_dbfs(wet_tail);
    assert!(level > TAIL_FLOOR_DBFS, "the 200 ms after the note-off is at {level:.1} dBFS, under the {TAIL_FLOOR_DBFS} dBFS floor");
    // And it is a decaying tail rather than a stuck level.
    let first_half = wet_tail.get(..wet_tail.len() / 2).unwrap_or(&[]);
    let second_half = wet_tail.get(wet_tail.len() / 2..).unwrap_or(&[]);
    assert!(level_dbfs(second_half) < level_dbfs(first_half), "the tail is not decaying: {:.1} then {:.1} dBFS", level_dbfs(first_half), level_dbfs(second_half));
}

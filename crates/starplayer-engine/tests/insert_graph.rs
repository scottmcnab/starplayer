//! The insert graph's routing and its control plane (M7-H1 deliverable 7).
//!
//! Three claims, none of which the block-size sweep can make:
//!
//! 1. **Routing.** An effect on channel 0's bus changes channel 0 and nothing else, on
//!    both mixing paths, and a voice whose lane is past the engine's bus count is still
//!    heard through the spill lane.
//! 2. **Retirement.** An effect that leaves a chain — replaced, removed, or orphaned by a
//!    command addressed to a lane that does not exist — goes back to the control thread
//!    over the garbage channel and is dropped there, never on the audio thread.
//! 3. **The topology.** Four ordered slots per bus, slot order is processing order, and
//!    bypass is a bit the engine reads rather than something the effect implements.

use starplayer_core::{ChannelId, I1F15, Step, U0F16, VoiceParams};
use starplayer_dsp::effects::{GAIN_MIN_CENTI_DB, GAIN_PARAM};
use starplayer_dsp::{InsertKind, Linear, build_insert};
use starplayer_engine::{
    Engine, EngineSettings, InsertCommand, InsertTarget, MAX_INSERTS_PER_CHAIN, RENDER_QUANTUM, ScriptedSource,
};
use starplayer_mixer::{
    FixedPath, FloatPath, LoopSpan, MixPath, OutputFormat, SampleRegion, StereoF32, StereoI16, VoiceTag,
    append_guarded_sample,
};

/// Enough voices and lanes for every scenario here, and no more.
const VOICE_CAPACITY: usize = 8;
const CHANNEL_COUNT: usize = 4;
const SAMPLE_RATE_HZ: u32 = 44_100;

/// Eight quanta: past `SMOOTH_FRAMES`, so an effect built at a value is settled and one
/// ramping towards it has landed.
const RENDER_FRAMES: usize = RENDER_QUANTUM * 8;

/// A short looping waveform with no zero frames, so a silenced bus is unmistakable.
fn looping_blob() -> (Vec<i16>, SampleRegion) {
    let waveform: Vec<i16> = (0..64).map(|index: i16| 3_000 + index * 400).collect();
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &waveform, LoopSpan::new(16, 64));
    (blob, region)
}

fn voice_params() -> VoiceParams {
    VoiceParams { step: Step::from_ratio(8_363 * 3, 44_100), volume: U0F16::from_bits(49_152), pan: I1F15::ZERO, ..VoiceParams::SILENT }
}

fn settings() -> EngineSettings {
    EngineSettings { voice_capacity: VOICE_CAPACITY, channel_count: CHANNEL_COUNT, sample_rate_hz: SAMPLE_RATE_HZ, ..EngineSettings::default() }
}

/// An engine with one looping voice on each of `channels`.
fn engine_with_voices<Path, Out>(channels: &[u8]) -> Engine<Path, Linear, Out>
where
    Path: MixPath,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Linear, Out> = Engine::with_settings(settings());
    engine.set_pcm(blob);
    for channel in channels {
        let tag = VoiceTag { channel: *channel, instrument: 1, sample: 1, note: 60 };
        engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");
    }
    engine.set_source(Box::new(ScriptedSource::new(Vec::new())));
    engine
}

// ── routing ─────────────────────────────────────────────────────────────────────────

/// Silencing channel 0's bus must leave channel 1 sample-for-sample as it was.
///
/// The comparison is against a render of channel 1 **alone**, not against a difference:
/// that is the only way to say "untouched" rather than "changed by less than I noticed".
fn assert_one_bus_is_silenced_and_the_other_untouched<Path, Out>(what: &str)
where
    Path: MixPath,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: Default + PartialEq + core::fmt::Debug,
{
    let render = |channels: &[u8], silence_channel_zero: bool| {
        let mut engine = engine_with_voices::<Path, Out>(channels);
        let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
        if silence_channel_zero {
            let mut insert = build_insert::<Path::Mono>(InsertKind::Gain, SAMPLE_RATE_HZ);
            // Built at the bottom of the fader and reset, so it is muting from the first
            // frame rather than ramping there over two quanta.
            insert.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
            insert.reset();
            inserts.install(InsertTarget::Channel(ChannelId(0)), 0, insert).map_err(|_| "full").expect("the ring has room");
        }
        let mut output = vec![Out::Sample::default(); RENDER_FRAMES * Out::CHANNELS];
        engine.render(&mut output);
        output
    };

    let both = render(&[0, 1], false);
    let silenced = render(&[0, 1], true);
    let channel_one_alone = render(&[1], false);

    assert!(both.iter().any(|sample| *sample != Out::Sample::default()), "{what}: the scenario has to make sound");
    assert_ne!(both, silenced, "{what}: silencing channel 0's bus changed nothing");
    assert_eq!(silenced, channel_one_alone, "{what}: channel 1 is not what it is on its own");
}

#[test]
fn an_insert_on_one_bus_silences_it_and_leaves_the_others_alone() {
    assert_one_bus_is_silenced_and_the_other_untouched::<FixedPath, StereoI16>("fixed / stereo i16");
    assert_one_bus_is_silenced_and_the_other_untouched::<FloatPath, StereoF32>("float / stereo f32");
}

/// A voice on a lane past the engine's bus count has no bus of its own, so it lands in the
/// spill lane — which is summed into the mix after every bus, so it is still heard. That is
/// research point 2's answer for a narrow engine: nothing is lost, only the chance to put
/// an insert on it.
#[test]
fn a_voice_beyond_the_bus_count_is_still_heard() {
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[CHANNEL_COUNT as u8 + 2]);
    assert_eq!(engine.bus_count(), CHANNEL_COUNT, "the engine has exactly the buses it was asked for");
    let mut output = vec![0i16; RENDER_FRAMES * 2];
    engine.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0), "a spilled voice is still audible");
}

/// An insert command addressed to a lane the engine has not got must not touch anything —
/// and must not drop the effect on the audio thread either.
#[test]
fn a_command_for_a_lane_that_does_not_exist_is_ignored_and_the_effect_is_retired() {
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    assert!(engine.chain(InsertTarget::Channel(ChannelId(CHANNEL_COUNT as u16))).is_none(), "there is no such bus");

    let mut insert = build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ);
    insert.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
    insert.reset();
    inserts.install(InsertTarget::Channel(ChannelId(CHANNEL_COUNT as u16 + 1)), 0, insert).map_err(|_| "full").expect("the ring has room");

    let mut output = vec![0i16; RENDER_FRAMES * 2];
    engine.render(&mut output);

    assert!(output.iter().any(|sample| *sample != 0), "channel 0 is untouched");
    assert!(!engine.warnings().retired_insert_dropped, "the orphan went down the garbage channel, not onto the audio thread");
    assert_eq!(inserts.pending_garbage(), 1, "and is waiting for the control thread");
    assert_eq!(inserts.collect_all_garbage(), 1, "which is where it dies");
}

/// The master chain runs on the summed mix, ahead of the master volume and the limiter, so
/// it silences everything.
#[test]
fn an_insert_on_the_master_bus_silences_the_whole_mix() {
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0, 1, CHANNEL_COUNT as u8 + 2]);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    let mut insert = build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ);
    insert.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
    insert.reset();
    inserts.install(InsertTarget::Master, 0, insert).map_err(|_| "full").expect("the ring has room");

    let mut output = vec![0i16; RENDER_FRAMES * 2];
    engine.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0), "the master chain sees the spill lane too");
}

// ── the control plane ───────────────────────────────────────────────────────────────

#[test]
fn the_insert_handle_is_handed_out_exactly_once() {
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::with_settings(settings());
    assert!(engine.take_insert_control().is_some(), "a new engine owns it");
    assert!(engine.take_insert_control().is_none(), "and hands it out once, like the control handle");
}

/// Installing over an occupied slot has to deliver the old effect to the collector, for
/// exactly the reason a replaced module does: dropping it here would call `free()` inside
/// the callback.
#[test]
fn installing_over_an_occupied_slot_retires_the_old_effect() {
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    let mut output = vec![0i16; RENDER_QUANTUM * 2];

    inserts.install(InsertTarget::Channel(ChannelId(0)), 0, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("room");
    engine.render(&mut output);
    assert_eq!(inserts.pending_garbage(), 0, "an empty slot retires nothing");
    assert!(engine.chain(InsertTarget::Channel(ChannelId(0))).is_some_and(|chain| chain.is_occupied(0)));

    inserts.install(InsertTarget::Channel(ChannelId(0)), 0, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("room");
    engine.render(&mut output);
    assert!(!engine.warnings().retired_insert_dropped, "nothing was dropped on the audio thread");
    assert!(inserts.collect_garbage().is_some(), "the replaced effect came back whole, to be inspected before it dies");
    assert_eq!(inserts.pending_garbage(), 0);

    inserts.send(InsertCommand::Remove { target: InsertTarget::Channel(ChannelId(0)), slot: 0 }).map_err(|_| "full").expect("room");
    engine.render(&mut output);
    assert_eq!(inserts.collect_all_garbage(), 1, "and so does a removed one");
    assert!(engine.chain(InsertTarget::Channel(ChannelId(0))).is_some_and(|chain| !chain.is_occupied(0)), "the slot is empty again");
}

/// A full garbage channel is the one case where the audio thread has to drop an effect
/// itself. It must say so rather than doing it silently.
#[test]
fn a_full_garbage_channel_raises_a_warning_rather_than_leaking() {
    let narrow = EngineSettings { insert_garbage_capacity: 1, ..settings() };
    let (blob, region) = looping_blob();
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::with_settings(narrow);
    engine.set_pcm(blob);
    engine.voices_mut().allocate(VoiceTag::default(), region, voice_params(), 0).expect("a fresh pool has room");
    engine.set_source(Box::new(ScriptedSource::new(Vec::new())));
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    let mut output = vec![0i16; RENDER_QUANTUM * 2];

    // Three installs into one slot: the first occupies it, the next two each retire what
    // they replace, and the channel only holds one.
    for _ in 0..3 {
        inserts.install(InsertTarget::Master, 0, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("room");
        engine.render(&mut output);
    }

    assert!(engine.warnings().retired_insert_dropped, "the overflow is a warning, not a silent leak or a hang");
    assert!(engine.warnings().any());
    assert_eq!(inserts.collect_all_garbage(), 1, "the channel held what it could");
}

/// Bypass is a bit the engine reads, so a bypassed effect is skipped but still receives
/// its parameters and is still reset.
#[test]
fn a_bypassed_effect_is_skipped_but_still_parameterised() {
    let render = |bypassed: bool| {
        let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
        let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
        let target = InsertTarget::Channel(ChannelId(0));
        let mut insert = build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ);
        insert.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
        insert.reset();
        inserts.install(target, 0, insert).map_err(|_| "full").expect("room");
        if bypassed {
            inserts.send(InsertCommand::Bypass { target, slot: 0, bypassed: true }).map_err(|_| "full").expect("room");
        }
        // Sent whether or not the effect is bypassed: a bypassed effect still takes its
        // parameters, so un-bypassing does not step into a stale value.
        inserts.send(InsertCommand::SetParam { target, slot: 0, param: GAIN_PARAM, value: 0 }).map_err(|_| "full").expect("room");
        let mut output = vec![0i16; RENDER_FRAMES * 2];
        engine.render(&mut output);
        let param = engine.chain(target).and_then(|chain| chain.get(0)).and_then(|insert| insert.param(GAIN_PARAM));
        assert_eq!(param, Some(0), "the effect took the parameter either way");
        assert_eq!(engine.chain(target).is_some_and(|chain| chain.is_bypassed(0)), bypassed);
        output
    };

    // Un-bypassed, the gain ramps from silence back to unity over two quanta, so the
    // first frames are quiet; bypassed, the very first frame is at full level.
    let bypassed = render(true);
    let engaged = render(false);
    assert!(bypassed.iter().any(|sample| *sample != 0), "a bypassed effect is out of the way");
    assert_ne!(bypassed, engaged, "and an engaged one is not");
    assert!(bypassed.first() != engaged.first(), "the ramp back to unity is audible in the first frame");
}

/// Four ordered slots, and slot order is processing order. Two gain trims in series
/// multiply, which is the cheapest observable statement that both ran.
#[test]
fn a_chain_runs_every_slot_in_order() {
    let render = |slots: usize| {
        let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
        let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
        let target = InsertTarget::Channel(ChannelId(0));
        for slot in 0..slots {
            let mut insert = build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ);
            insert.set_param(GAIN_PARAM, -600);
            insert.reset();
            inserts.install(target, slot as u8, insert).map_err(|_| "full").expect("room");
        }
        let mut output = vec![0i16; RENDER_FRAMES * 2];
        engine.render(&mut output);
        output
    };

    let one = render(1);
    let four = render(MAX_INSERTS_PER_CHAIN);
    let peak = |samples: &[i16]| samples.iter().map(|sample| sample.unsigned_abs()).max().unwrap_or(0);
    assert!(peak(&four) < peak(&one), "four −6 dB trims in series are quieter than one");
    assert!(peak(&four) > 0, "and have not silenced it");

    // A fifth slot does not exist, so the effect addressed to it comes straight back.
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    inserts
        .install(InsertTarget::Channel(ChannelId(0)), MAX_INSERTS_PER_CHAIN as u8, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ))
        .map_err(|_| "full")
        .expect("room");
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert_eq!(inserts.collect_all_garbage(), 1, "a slot past the end of the chain retires the effect");
    assert!(!engine.warnings().retired_insert_dropped);
}

/// `ResetAll` is the host's seek: it clears state and leaves parameters where they are.
#[test]
fn reset_all_lands_every_moving_parameter_without_changing_it() {
    let mut engine = engine_with_voices::<FixedPath, StereoI16>(&[0]);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    let target = InsertTarget::Channel(ChannelId(0));
    inserts.install(target, 0, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("room");
    inserts.send(InsertCommand::SetParam { target, slot: 0, param: GAIN_PARAM, value: GAIN_MIN_CENTI_DB }).map_err(|_| "full").expect("room");

    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0), "one quantum in, the ramp is only part way down");

    inserts.send(InsertCommand::ResetAll).map_err(|_| "full").expect("room");
    engine.render(&mut output);
    let param = engine.chain(target).and_then(|chain| chain.get(0)).and_then(|insert| insert.param(GAIN_PARAM));
    assert_eq!(param, Some(GAIN_MIN_CENTI_DB), "the parameter is where it was heading");
    assert!(output.iter().all(|sample| *sample == 0), "and it got there at once");
}

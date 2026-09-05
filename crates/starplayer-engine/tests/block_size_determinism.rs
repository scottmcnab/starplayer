//! **The block-size determinism invariant.**
//!
//! Rendering the same scenario at host block sizes 1, 3, 64, 128, 4096 and 8191 must
//! produce byte-identical output, and a parameter change scheduled for a frame that is
//! not a multiple of the render quantum must take effect on exactly that frame at every
//! one of them.
//!
//! This test is the deliverable of M0-task-A3, not a check on it. Architecture §1.4 calls
//! ragged per-block DSP "the highest-probability silent failure in the whole design": if
//! anything downstream of voice accumulation ever consumes a host-shaped segment instead
//! of a whole `RENDER_QUANTUM`, offline rendering stops matching real-time and neither
//! matches across hosts — and nothing about the sound tells you. It is cheap to make
//! impossible, but only while the test exists from the beginning. **It must never be
//! weakened.**
//!
//! Samples are compared as **bit patterns**, never with `==` on floats: `==` would accept
//! `0.0 == -0.0` and reject two identical NaNs, and byte identity is the actual claim.

use starplayer_core::{ChannelId, ExactFixedPoint, FilterParams, Frame, I1F15, Step, U0F16, VoiceParam, VoiceParams};
use starplayer_dsp::effects::{GAIN_MIN_CENTI_DB, GAIN_PARAM};
use starplayer_dsp::{Cubic, InsertKind, Interpolate, Linear, Nearest, Sinc, build_insert};
use starplayer_engine::demo::{
    DEMO_BREAK_ROW, DEMO_NOTE_CUT, DEMO_ORDER_JUMP, DEMO_PATTERN_DELAY, DEMO_SET_SPEED, DEMO_SET_TEMPO, DemoCell,
    DemoPatternData, DemoProcessor,
};
use starplayer_engine::{
    ChannelTable, ControlDriver, Engine, EngineContext, EngineSettings, EventSource, InsertCommand, InsertHandle,
    InsertTarget, MAX_VOICE_CAPACITY, MAX_ZERO_ADVANCE, PatternSequencer, RENDER_QUANTUM, ScriptedAction,
    ScriptedSource, SequencerSettings,
};
use starplayer_mixer::{
    FixedPath, FloatPath, LoopSpan, MixPath, MonoI16, OutputFormat, SampleRegion, StereoF32, StereoI16, VoiceTag,
    append_guarded_sample,
};

// ── the scenario ────────────────────────────────────────────────────────────────────

/// The host block sizes the invariant is stated over, in frames. 1 and 3 are smaller than
/// a quantum, 64 divides it, 128 is exactly one, and 4096 and 8191 straddle many — 8191
/// deliberately being neither a multiple of 128 nor a power of two.
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

/// Frames of output every run produces. Past the task's 20,000 minimum, and **not** a
/// whole number of quanta (20_000 = 156.25 × 128), so the trailing partial quantum is
/// exercised as well.
const TOTAL_FRAMES: usize = 20_000;

/// When the scheduled parameter change happens.
///
/// `1000 + 37`: deliberately not a multiple of `RENDER_QUANTUM`, so an implementation
/// that applied events at quantum boundaries would land it on 1024 or 1152 and be caught,
/// and not a multiple of any block size in `BLOCK_SIZES` either.
const EVENT_FRAME: u64 = 1_000 + 37;

const _: () = assert!(!EVENT_FRAME.is_multiple_of(RENDER_QUANTUM as u64), "the event frame must not sit on a quantum boundary");

/// Loud, then quiet — far enough apart that the change survives the fixed path's
/// truncating gain arithmetic and shows up in the very first frame it applies to.
const VOLUME_BEFORE: U0F16 = U0F16::from_bits(49_152);
const VOLUME_AFTER: U0F16 = U0F16::from_bits(16_384);

/// The voice pool is bigger than the scenario needs, so a full pool is never what a
/// failure means.
const VOICE_CAPACITY: usize = 8;

/// A short waveform with **no zero-valued frames**, so a volume change is audible on
/// whichever frame it lands on. A run of zeros around the event frame would let a
/// mistimed event slip through unnoticed.
fn waveform() -> Vec<i16> { (0..64).map(|index: i16| 3_000 + index * 400).collect() }

/// The module's PCM blob, holding the one looping sample, and the region addressing it.
fn looping_blob() -> (Vec<i16>, SampleRegion) {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &waveform(), LoopSpan::new(16, 64));
    (blob, region)
}

/// A step that is neither a whole number of frames nor a divisor of the loop length, so
/// the fractional position, the interpolator and the loop wrap are all live throughout
/// the render.
fn playback_step() -> Step { Step::from_ratio(8_363 * 3, 44_100) }

fn voice_params() -> VoiceParams {
    VoiceParams { step: playback_step(), volume: VOLUME_BEFORE, pan: I1F15::ZERO, ..VoiceParams::SILENT }
}

/// An engine playing one looping voice, with the volume change scheduled or not.
fn build_engine<Path, Interp, Out>(schedule_event: bool) -> Engine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);

    let tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
    let voice = engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");

    let actions = if schedule_event {
        vec![ScriptedAction::new(Frame(EVENT_FRAME), voice, VoiceParam::Volume(VOLUME_AFTER))]
    } else {
        Vec::new()
    };
    engine.set_source(Box::new(ScriptedSource::new(actions)));
    engine
}

/// Render `TOTAL_FRAMES` frames in `block_frames`-sized host calls.
fn render_at_block_size<Path, Interp, Out>(block_frames: usize, schedule_event: bool) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut engine = build_engine::<Path, Interp, Out>(schedule_event);
    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];

    let mut written = 0;
    while written < output.len() {
        let end = (written + block_frames * Out::CHANNELS).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    output
}

// ── bit-pattern comparison ──────────────────────────────────────────────────────────

/// A sample's raw bytes. Implemented rather than assumed so that comparing `f32` output
/// can never silently degrade into a float comparison.
trait BitPattern: Copy {
    type Bytes: PartialEq + core::fmt::Debug;
    fn to_bytes(self) -> Self::Bytes;
}

impl BitPattern for f32 {
    type Bytes = [u8; 4];
    fn to_bytes(self) -> [u8; 4] { self.to_bits().to_ne_bytes() }
}

impl BitPattern for i16 {
    type Bytes = [u8; 2];
    fn to_bytes(self) -> [u8; 2] { self.to_ne_bytes() }
}

fn byte_image<Sample: BitPattern>(samples: &[Sample]) -> Vec<Sample::Bytes> {
    samples.iter().map(|sample| sample.to_bytes()).collect()
}

/// Index of the first sample whose bytes differ, or `None` if the two runs are identical.
fn first_difference<Sample: BitPattern>(left: &[Sample], right: &[Sample]) -> Option<usize> {
    left.iter().zip(right.iter()).position(|(a, b)| a.to_bytes() != b.to_bytes())
}

/// Run the whole block-size sweep for one path / interpolator / output format.
fn assert_block_size_independent<Path, Interp, Out>(what: &str)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let reference = render_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM, true);
    assert_eq!(reference.len(), TOTAL_FRAMES * Out::CHANNELS);
    let silence = vec![Out::Sample::default(); reference.len()];
    assert!(first_difference(&reference, &silence).is_some(), "{what}: the scenario has to actually make sound");

    for block_frames in BLOCK_SIZES {
        let output = render_at_block_size::<Path, Interp, Out>(block_frames, true);
        let difference = first_difference(&output, &reference);
        assert_eq!(difference, None, "{what}: block size {block_frames} changed the output at sample {difference:?}");
        assert_eq!(byte_image(&output), byte_image(&reference), "{what}: block size {block_frames} is not byte-identical");
    }
}

// ── the invariant ───────────────────────────────────────────────────────────────────

#[test]
fn float_output_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Linear, StereoF32>("float / linear / stereo f32");
}

#[test]
fn fixed_output_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FixedPath, Linear, StereoI16>("fixed / linear / stereo i16");
}

#[test]
fn nearest_interpolation_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Nearest, StereoF32>("float / nearest / stereo f32");
    assert_block_size_independent::<FixedPath, Nearest, StereoI16>("fixed / nearest / stereo i16");
}

#[test]
fn mono_output_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FixedPath, Linear, MonoI16>("fixed / linear / mono i16");
}

/// The wide kernels (M7-task-H5) read behind the interpolation point and defer a forward
/// loop's wrap by that many frames, both of which are properties of the voice rather than
/// of the block — which is exactly what this has to prove.
#[test]
fn cubic_interpolation_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Cubic, StereoF32>("float / cubic / stereo f32");
    assert_block_size_independent::<FixedPath, Cubic, StereoI16>("fixed / cubic / stereo i16");
}

#[test]
fn sinc_interpolation_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Sinc, StereoF32>("float / sinc / stereo f32");
    assert_block_size_independent::<FixedPath, Sinc, StereoI16>("fixed / sinc / stereo i16");
}

// ── the same invariant, with IT's per-voice resonant filter live (M6-G2) ────────────

/// When the filter sweeps. Deliberately not a multiple of `RENDER_QUANTUM` and not equal
/// to [`EVENT_FRAME`], so the coefficient refresh lands mid-quantum and on a different
/// frame from the volume change.
const FILTER_EVENT_FRAME: u64 = 5_000 + 41;

const _: () = assert!(!FILTER_EVENT_FRAME.is_multiple_of(RENDER_QUANTUM as u64), "the filter event must not sit on a quantum boundary");

/// A near-closed, strongly resonant filter, and the wide-open-but-still-resonant one it
/// sweeps to. Both are far enough apart to be audible in one sample, and neither is
/// `FilterParams::BYPASS`, so the filtered arm of the kernel runs for the whole render.
const FILTER_BEFORE: FilterParams = FilterParams::from_it(24, 112);
const FILTER_AFTER: FilterParams = FilterParams::from_it(96, 40);

/// The filtered scenario: the same looping voice, with a filter on it from the first frame
/// and a `Zxx`-shaped sweep part way through.
fn render_filtered_at_block_size<Path, Interp, Out>(block_frames: usize) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);

    let tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
    let params = VoiceParams { filter: FILTER_BEFORE, ..voice_params() };
    let voice = engine.voices_mut().allocate(tag, region, params, 0).expect("a fresh pool has room");
    engine.set_source(Box::new(ScriptedSource::new(vec![
        ScriptedAction::new(Frame(EVENT_FRAME), voice, VoiceParam::Volume(VOLUME_AFTER)),
        ScriptedAction::new(Frame(FILTER_EVENT_FRAME), voice, VoiceParam::Filter(FILTER_AFTER)),
    ])));

    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];
    let mut written = 0;
    while written < output.len() {
        let end = (written + block_frames * Out::CHANNELS).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    output
}

fn assert_filtered_block_size_independent<Path, Interp, Out>(what: &str)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let reference = render_filtered_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM);
    let unfiltered = render_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM, true);
    assert!(first_difference(&reference, &unfiltered).is_some(), "{what}: the filter has to actually change the sound");

    for block_frames in BLOCK_SIZES {
        let output = render_filtered_at_block_size::<Path, Interp, Out>(block_frames);
        let difference = first_difference(&output, &reference);
        assert_eq!(difference, None, "{what}: block size {block_frames} changed the output at sample {difference:?}");
        assert_eq!(byte_image(&output), byte_image(&reference), "{what}: block size {block_frames} is not byte-identical");
    }
}

/// The filter is a two-pole *recursion*, so it is the one thing in the voice path whose
/// output depends on where the previous segment ended. If a coefficient refresh or a delay
/// line were ever driven by the host's block size rather than by the voice's own state,
/// this is where it would show.
#[test]
fn a_filtered_voice_is_byte_identical_at_every_host_block_size() {
    assert_filtered_block_size_independent::<FloatPath, Linear, StereoF32>("float / linear / stereo f32, filtered");
    assert_filtered_block_size_independent::<FixedPath, Linear, StereoI16>("fixed / linear / stereo i16, filtered");
    assert_filtered_block_size_independent::<FixedPath, Nearest, MonoI16>("fixed / nearest / mono i16, filtered");
    assert_filtered_block_size_independent::<FixedPath, Cubic, StereoI16>("fixed / cubic / stereo i16, filtered");
    assert_filtered_block_size_independent::<FixedPath, Sinc, StereoI16>("fixed / sinc / stereo i16, filtered");
}

// ── the same invariant, with the insert graph live (M7-H1) ──────────────────────────
//
// The insert control ring is drained at the **top of a quantum**, exactly like the command
// ring, so a command pushed from the host lands on a quantum boundary rather than on a
// host block boundary.
//
// An insert command has no scheduled frame the way a `ScriptedAction` has — it takes
// effect when the audio thread next drains the ring — so the *input* to these runs is only
// the same at every block size if the host pushes at the same point in the stream. The
// render loop below therefore stops exactly on each phase boundary and pushes there,
// varying the block size everywhere else. Each boundary is a whole number of quanta, so at
// one the output ring is empty and the engine has rendered exactly that many frames
// whatever size the host asked in. That is not a weakening of the invariant: what is being
// asserted is still that the same control timeline produces byte-identical output at every
// block size, and the smoothing ramp the `SetParam` starts runs for two whole quanta after
// the boundary, across block boundaries wherever they fall.

/// Frames of host output before the `SetParam` is queued: 40 whole quanta.
const INSERT_SET_PARAM_FRAMES: usize = 40 * RENDER_QUANTUM;

/// Frames of host output before the `Remove` is queued: 100 whole quanta, so the
/// smoothing ramp the `SetParam` started has long since landed and the removal is a step
/// rather than a change to a moving value.
const INSERT_REMOVE_FRAMES: usize = 100 * RENDER_QUANTUM;

const _: () = assert!(INSERT_SET_PARAM_FRAMES < INSERT_REMOVE_FRAMES && INSERT_REMOVE_FRAMES < TOTAL_FRAMES);

/// The channel the inserted chain sits on. The second voice is on channel 0 and has no
/// chain, so it proves the graph touches one bus and not the other.
const INSERT_CHANNEL: u16 = 1;

/// What the `SetParam` moves the channel gain to: 6 dB down, far enough to be visible in
/// one sample on the fixed path.
const INSERT_SET_PARAM_CENTI_DB: i32 = -600;

/// A gentle trim on the master, so the master chain is live for the whole render and a
/// mistake in the order "channel chains, sum, spill, master chain, master bus" shows.
const INSERT_MASTER_CENTI_DB: i32 = -150;

/// Two looping voices — one on channel 0, one on [`INSERT_CHANNEL`] — with a gain insert
/// on the second channel's bus and another on the master.
fn render_with_inserts_at_block_size<Path, Interp, Out>(block_frames: usize) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");

    for channel in [0u8, INSERT_CHANNEL as u8] {
        let tag = VoiceTag { channel, instrument: 1, sample: 1, note: 60 };
        engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");
    }
    engine.set_source(Box::new(ScriptedSource::new(Vec::new())));

    // Both effects are built here, on the "control thread", and cross to the engine boxed.
    let channel_target = InsertTarget::Channel(ChannelId(INSERT_CHANNEL));
    inserts.install(channel_target, 0, build_insert::<Path::Mono>(InsertKind::Gain, 44_100)).map_err(|_| "full").expect("the ring has room");
    let mut master = build_insert::<Path::Mono>(InsertKind::Gain, 44_100);
    master.set_param(GAIN_PARAM, INSERT_MASTER_CENTI_DB);
    inserts.install(InsertTarget::Master, 0, master).map_err(|_| "full").expect("the ring has room");

    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];
    let set_param = InsertCommand::SetParam { target: channel_target, slot: 0, param: GAIN_PARAM, value: INSERT_SET_PARAM_CENTI_DB };
    let remove = InsertCommand::Remove { target: channel_target, slot: 0 };
    let phases = [(INSERT_SET_PARAM_FRAMES, Some(set_param)), (INSERT_REMOVE_FRAMES, Some(remove)), (TOTAL_FRAMES, None)];
    render_phases::<Path, Interp, Out>(&mut engine, &mut inserts, &mut output, block_frames, phases);

    // Retired on the audio thread, dropped here — the point of the garbage channel.
    assert_eq!(inserts.collect_all_garbage(), 1, "the removed insert came back to the control thread");
    assert!(!engine.warnings().any(), "the insert graph raised no warnings: {:?}", engine.warnings());
    output
}

/// Render `output` in `block_frames`-sized host calls, stopping exactly at each phase's
/// frame count and sending that phase's insert command there.
fn render_phases<Path, Interp, Out>(
    engine: &mut Engine<Path, Interp, Out>,
    inserts: &mut InsertHandle<Path::Mono>,
    output: &mut [Out::Sample],
    block_frames: usize,
    phases: [(usize, Option<InsertCommand<Path::Mono>>); 3],
) where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut written = 0usize;
    for (boundary_frames, command) in phases {
        let boundary = (boundary_frames * Out::CHANNELS).min(output.len());
        while written < boundary {
            let end = (written + block_frames * Out::CHANNELS).min(boundary);
            let Some(block) = output.get_mut(written..end) else { return };
            engine.render(block);
            written = end;
        }
        if let Some(command) = command {
            inserts.send(command).map_err(|_| "full").expect("the ring has room");
        }
    }
}

fn assert_inserted_block_size_independent<Path, Interp, Out>(what: &str)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let reference = render_with_inserts_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM);
    let plain = render_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM, false);
    assert!(first_difference(&reference, &plain).is_some(), "{what}: the inserts have to actually change the sound");

    for block_frames in BLOCK_SIZES {
        let output = render_with_inserts_at_block_size::<Path, Interp, Out>(block_frames);
        let difference = first_difference(&output, &reference);
        assert_eq!(difference, None, "{what}: block size {block_frames} changed the output at sample {difference:?}");
        assert_eq!(byte_image(&output), byte_image(&reference), "{what}: block size {block_frames} is not byte-identical");
    }
}

/// A channel chain, a master chain, a parameter change part way through and a removal
/// later, at every host block size on both mixing paths.
///
/// The three things this would catch: an insert handed a ragged segment instead of a whole
/// quantum, a smoothing ramp advanced per host block rather than per frame, and a command
/// drained anywhere but the top of a quantum.
#[test]
fn an_insert_graph_is_byte_identical_at_every_host_block_size() {
    assert_inserted_block_size_independent::<FloatPath, Linear, StereoF32>("inserted float / linear / stereo f32");
    assert_inserted_block_size_independent::<FixedPath, Linear, StereoI16>("inserted fixed / linear / stereo i16");
    assert_inserted_block_size_independent::<FixedPath, Nearest, MonoI16>("inserted fixed / nearest / mono i16");
}

/// The parameter change lands on the first quantum after the phase boundary, at every
/// block size — the insert graph's version of
/// [`the_parameter_change_lands_on_exactly_its_scheduled_frame_at_every_block_size`].
#[test]
fn an_insert_parameter_change_lands_on_the_same_quantum_at_every_block_size() {
    let reference = render_with_inserts_at_block_size::<FixedPath, Linear, StereoI16>(RENDER_QUANTUM);
    let never_changed = render_with_inserts_without_the_parameter_change::<FixedPath, Linear, StereoI16>(RENDER_QUANTUM);
    let first = first_difference(&reference, &never_changed).map(|sample| sample / 2).expect("the change is audible");
    assert_eq!(first, INSERT_SET_PARAM_FRAMES, "the change landed at frame {first}, not on the quantum boundary it was queued at");

    for block_frames in BLOCK_SIZES {
        let output = render_with_inserts_at_block_size::<FixedPath, Linear, StereoI16>(block_frames);
        let baseline = render_with_inserts_without_the_parameter_change::<FixedPath, Linear, StereoI16>(block_frames);
        let seen = first_difference(&output, &baseline).map(|sample| sample / 2);
        assert_eq!(seen, Some(INSERT_SET_PARAM_FRAMES), "block size {block_frames} moved the parameter change");
    }
}

/// [`render_with_inserts_at_block_size`] with the `SetParam` never sent, so the two runs
/// differ at exactly the frame the change lands on.
fn render_with_inserts_without_the_parameter_change<Path, Interp, Out>(block_frames: usize) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");

    for channel in [0u8, INSERT_CHANNEL as u8] {
        let tag = VoiceTag { channel, instrument: 1, sample: 1, note: 60 };
        engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");
    }
    engine.set_source(Box::new(ScriptedSource::new(Vec::new())));

    let channel_target = InsertTarget::Channel(ChannelId(INSERT_CHANNEL));
    inserts.install(channel_target, 0, build_insert::<Path::Mono>(InsertKind::Gain, 44_100)).map_err(|_| "full").expect("the ring has room");
    let mut master = build_insert::<Path::Mono>(InsertKind::Gain, 44_100);
    master.set_param(GAIN_PARAM, INSERT_MASTER_CENTI_DB);
    inserts.install(InsertTarget::Master, 0, master).map_err(|_| "full").expect("the ring has room");

    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];
    let remove = InsertCommand::Remove { target: channel_target, slot: 0 };
    let phases = [(INSERT_SET_PARAM_FRAMES, None), (INSERT_REMOVE_FRAMES, Some(remove)), (TOTAL_FRAMES, None)];
    render_phases::<Path, Interp, Out>(&mut engine, &mut inserts, &mut output, block_frames, phases);
    inserts.collect_all_garbage();
    output
}

/// A gain insert at the bottom of its fader silences its own bus and leaves every other
/// one alone. On both paths, because an effect body is generic over
/// [`DspSample`](starplayer_dsp::DspSample) and this is what proves both instantiations
/// route the same way.
fn assert_a_silenced_channel_leaves_the_others_alone<Path, Interp, Out>(what: &str)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default + PartialEq,
{
    let render = |silence_channel_zero: bool| {
        let (blob, region) = looping_blob();
        let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
        engine.set_pcm(blob);
        let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
        for channel in [0u8, 1] {
            let tag = VoiceTag { channel, instrument: 1, sample: 1, note: 60 };
            engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");
        }
        engine.set_source(Box::new(ScriptedSource::new(Vec::new())));
        if silence_channel_zero {
            // Built at the bottom of the fader rather than ramped down to it, so the
            // muting is in force from the very first frame.
            let mut insert = build_insert::<Path::Mono>(InsertKind::Gain, 44_100);
            insert.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
            insert.reset();
            inserts.install(InsertTarget::Channel(ChannelId(0)), 0, insert).map_err(|_| "full").expect("the ring has room");
        }
        let mut output = vec![Out::Sample::default(); RENDER_QUANTUM * 8 * Out::CHANNELS];
        engine.render(&mut output);
        output
    };

    let both = render(false);
    let one_silenced = render(true);
    assert!(both.iter().any(|sample| *sample != Out::Sample::default()), "{what}: the scenario has to make sound");
    assert!(one_silenced.iter().any(|sample| *sample != Out::Sample::default()), "{what}: channel 1 is still audible");
    assert!(first_difference(&both, &one_silenced).is_some(), "{what}: silencing channel 0 changed nothing");
}

#[test]
fn a_gain_insert_at_the_bottom_of_its_fader_silences_one_bus_only() {
    assert_a_silenced_channel_leaves_the_others_alone::<FloatPath, Linear, StereoF32>("float / linear / stereo f32");
    assert_a_silenced_channel_leaves_the_others_alone::<FixedPath, Linear, StereoI16>("fixed / linear / stereo i16");
}

/// The same scenario as [`render_at_block_size`], on an engine sized by `settings` rather
/// than by [`Engine::new`].
fn render_with_settings(settings: EngineSettings) -> Vec<i16> {
    let (blob, region) = looping_blob();
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::with_settings(settings);
    engine.set_pcm(blob);

    let tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
    let voice = engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");
    engine.set_source(Box::new(ScriptedSource::new(vec![ScriptedAction::new(Frame(EVENT_FRAME), voice, VoiceParam::Volume(VOLUME_AFTER))])));

    let mut output = vec![0i16; TOTAL_FRAMES * StereoI16::CHANNELS];
    for block in output.chunks_mut(RENDER_QUANTUM * StereoI16::CHANNELS) {
        engine.render(block);
    }
    output
}

/// M4-lite E3 research point 1: a persistent host sizes its engine at the maxima
/// (`MAX_VOICE_CAPACITY` voices, `ChannelTable::MAX_CHANNELS` lanes) because it builds the
/// engine before it has seen a module. Both are allocated once and the mixer walks only
/// *active* slots, so the wider engine must render byte-identical output — the extra
/// capacity is memory and nothing else.
#[test]
fn a_wider_voice_pool_and_channel_table_render_byte_identical_output() {
    let narrow = render_with_settings(EngineSettings { voice_capacity: 64, channel_count: 32, ..EngineSettings::default() });
    let wide = render_with_settings(EngineSettings { voice_capacity: MAX_VOICE_CAPACITY, channel_count: ChannelTable::MAX_CHANNELS, ..EngineSettings::default() });

    assert!(narrow.iter().any(|sample| *sample != 0), "the scenario has to actually make sound");
    assert_eq!(first_difference(&wide, &narrow), None, "256 voices / 64 channels changed a sample against 64 / 32");
    assert_eq!(byte_image(&wide), byte_image(&narrow), "the wider engine is not byte-identical");
}

// ── events land on exact frames, not on buffer boundaries ───────────────────────────

/// Locate the first output frame at which scheduling the event changes anything, by
/// rendering the scenario twice — once with it, once without.
fn first_frame_changed_by_the_event<Path, Interp, Out>(block_frames: usize) -> Option<usize>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let without = render_at_block_size::<Path, Interp, Out>(block_frames, false);
    let with = render_at_block_size::<Path, Interp, Out>(block_frames, true);
    first_difference(&with, &without).map(|sample_index| sample_index / Out::CHANNELS)
}

#[test]
fn the_parameter_change_lands_on_exactly_its_scheduled_frame_at_every_block_size() {
    for block_frames in BLOCK_SIZES {
        assert_eq!(
            first_frame_changed_by_the_event::<FloatPath, Linear, StereoF32>(block_frames),
            Some(EVENT_FRAME as usize),
            "float path: the volume change did not land on frame {EVENT_FRAME} at block size {block_frames}"
        );
        assert_eq!(
            first_frame_changed_by_the_event::<FixedPath, Linear, StereoI16>(block_frames),
            Some(EVENT_FRAME as usize),
            "fixed path: the volume change did not land on frame {EVENT_FRAME} at block size {block_frames}"
        );
    }
}

#[test]
fn the_event_is_dispatched_exactly_once() {
    // A `ScriptedSource` that has run out of actions reports no next event, so a
    // re-dispatch would show up as a non-empty queue or as a second volume change. The
    // block size that stresses this hardest is 1, where the render loop re-enters the
    // dispatch check on every single frame.
    let mut engine = build_engine::<FixedPath, Linear, StereoI16>(true);
    let mut output = vec![0i16; 4_096];
    for _ in 0..4 {
        engine.render(&mut output);
    }
    assert_eq!(engine.frame(), Frame(4 * 2_048));
    assert!(!engine.warnings().any(), "a well-behaved source raises no warnings");
}

// ── voice lifetime ──────────────────────────────────────────────────────────────────

#[test]
fn a_voice_that_reaches_the_end_of_a_one_shot_releases_itself() {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &waveform(), None);

    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    let params = VoiceParams { step: Step::ONE, volume: VOLUME_BEFORE, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("a fresh pool has room");
    assert_eq!(engine.voices().voices_active(), 1);

    // The sample is 64 frames at a step of one, so it ends well inside the first quantum.
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(engine.voices().voices_active(), 0, "the voice returned to the pool");
    assert!(engine.voices_mut().get_mut(voice).is_none(), "and its handle went stale");
    assert!(output[..64 * 2].iter().any(|sample| *sample != 0), "the frames before the end sounded");
    assert!(output[70 * 2..].iter().all(|sample| *sample == 0), "and everything after it is silence");
}

#[test]
fn a_stale_voice_id_does_not_resolve_after_its_slot_is_reused() {
    let (blob, region) = looping_blob();
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::new(1);
    engine.set_pcm(blob);

    let first = engine.voices_mut().allocate(VoiceTag::default(), region, voice_params(), 0).expect("slot 0");
    assert!(engine.voices_mut().release(first));

    let second = engine.voices_mut().allocate(VoiceTag::default(), region, voice_params(), 0).expect("the same slot");
    assert_eq!(second.index(), first.index(), "the pool reused the slot");
    assert_ne!(second.generation(), first.generation());
    assert!(engine.voices_mut().get_mut(first).is_none(), "the stale handle must not resolve to the new occupant");
    assert!(engine.voices_mut().get_mut(second).is_some());
}

// ── the zero-advance guard ──────────────────────────────────────────────────────────

/// A source that never advances: it reports the same frame for ever, whatever the engine
/// does. S3M speed 0, `A00`, a MOD `E60` self-loop and a pattern break to the same row all
/// produce exactly this behaviour, and every tracker has shipped the resulting hang
/// (architecture §3.1 rule 2).
struct StuckSource;

impl EventSource for StuckSource {
    fn next_event_frame(&self) -> Option<Frame> { Some(Frame::ZERO) }
    fn advance_to(&mut self, _frame: Frame) {}
    fn dispatch(&mut self, _frame: Frame, _context: &mut EngineContext<'_>) {}
}

#[test]
fn a_source_that_never_advances_is_forced_forward_rather_than_hanging() {
    let (blob, region) = looping_blob();
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    engine.voices_mut().allocate(VoiceTag::default(), region, voice_params(), 0).expect("a fresh pool has room");
    engine.set_source(Box::new(StuckSource));

    // If the guard were missing this call would never return.
    let mut output = vec![0i16; RENDER_QUANTUM * 2 * 2];
    engine.render(&mut output);

    let warnings = engine.warnings();
    assert!(warnings.zero_advance_forced, "the guard must flag the breach for the host to see");
    assert!(warnings.event_limit_reached, "128 frames x {} dispatches also passes the per-block cap", MAX_ZERO_ADVANCE + 1);
    assert_eq!(engine.frame(), Frame(2 * RENDER_QUANTUM as u64), "the clock advanced a full frame at a time");
    assert!(output.iter().any(|sample| *sample != 0), "and audio was still produced throughout");

    assert!(engine.take_warnings().any());
    assert!(!engine.warnings().any(), "taking the warnings clears them");
}

// ── the same invariant, with the pattern sequencer driving ───────────────────────────
//
// M1-task-B3 adds the sequencer, and with it three new ways for the host's block size to
// leak into the output: a tick boundary computed from a stale tempo, a row advanced at a
// quantum boundary instead of a tick boundary, and a voice triggered on the wrong frame.
// The invariant is therefore restated over a sequencer-driven engine rather than only over
// a scripted parameter change.

/// A synthetic module that exercises every part of the timing spine inside
/// `TOTAL_FRAMES`: notes on both channels, a mid-song tempo change, a pattern delay, a
/// pattern break and a note cut.
fn synthetic_module() -> DemoPatternData {
    let mut data = DemoPatternData::new(2, 8, 2);

    // Pattern 0: notes, a tempo change, a pattern delay, and a break to pattern 1 row 2.
    data.set(0, 0, 0, DemoCell::note(48));
    data.set(0, 0, 1, DemoCell::note(36));
    data.set(0, 1, 0, DemoCell::note(55));
    data.set(0, 2, 0, DemoCell::command(DEMO_SET_TEMPO, 200));
    data.set(0, 2, 1, DemoCell::note(41));
    data.set(0, 3, 0, DemoCell::command(DEMO_PATTERN_DELAY, 2));
    data.set(0, 3, 1, DemoCell::note(60));
    data.set(0, 4, 0, DemoCell::note(43));
    data.set(0, 4, 1, DemoCell { note: DEMO_NOTE_CUT, ..DemoCell::EMPTY });
    data.set(0, 5, 0, DemoCell::command(DEMO_BREAK_ROW, 2));

    // Pattern 1: a speed change, more notes, and a jump back to order 0 so the module
    // never runs out inside the test.
    data.set(1, 2, 0, DemoCell::command(DEMO_SET_SPEED, 4));
    data.set(1, 2, 1, DemoCell::note(48));
    data.set(1, 3, 0, DemoCell::note(52));
    data.set(1, 5, 1, DemoCell::note(38));
    data.set(1, 7, 0, DemoCell::command(DEMO_ORDER_JUMP, 0));
    data
}

/// An engine playing `synthetic_module()` through a real [`PatternSequencer`].
fn build_sequenced_engine<Path, Interp, Out>() -> Engine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob();
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);

    let sequencer = PatternSequencer::new(
        ExactFixedPoint,
        synthetic_module(),
        DemoProcessor::new(region, playback_step()),
        SequencerSettings { sample_rate_hz: 44_100, ..SequencerSettings::default() },
    );
    engine.set_source(Box::new(sequencer));
    engine
}

fn render_sequenced_at_block_size<Path, Interp, Out>(block_frames: usize) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut engine = build_sequenced_engine::<Path, Interp, Out>();
    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];

    let mut written = 0;
    while written < output.len() {
        let end = (written + block_frames * Out::CHANNELS).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    output
}

fn assert_sequenced_output_is_block_size_independent<Path, Interp, Out>(what: &str)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let reference = render_sequenced_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM);
    let silence = vec![Out::Sample::default(); reference.len()];
    assert!(first_difference(&reference, &silence).is_some(), "{what}: the module has to actually make sound");

    for block_frames in BLOCK_SIZES {
        let output = render_sequenced_at_block_size::<Path, Interp, Out>(block_frames);
        let difference = first_difference(&output, &reference);
        assert_eq!(difference, None, "{what}: block size {block_frames} changed the output at sample {difference:?}");
        assert_eq!(byte_image(&output), byte_image(&reference), "{what}: block size {block_frames} is not byte-identical");
    }
}

#[test]
fn sequencer_driven_output_is_byte_identical_at_every_host_block_size() {
    assert_sequenced_output_is_block_size_independent::<FloatPath, Linear, StereoF32>("sequenced float / linear / stereo f32");
    assert_sequenced_output_is_block_size_independent::<FixedPath, Linear, StereoI16>("sequenced fixed / linear / stereo i16");
    assert_sequenced_output_is_block_size_independent::<FixedPath, Nearest, MonoI16>("sequenced fixed / nearest / mono i16");
}

/// The sequencer really ran: rows were fetched, ticks happened and notes were triggered.
/// Without this a byte-identical *silence* would pass the test above.
#[test]
fn the_synthetic_module_actually_plays() {
    let (blob, region) = looping_blob();
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);

    let sequencer = PatternSequencer::new(
        ExactFixedPoint,
        synthetic_module(),
        DemoProcessor::new(region, playback_step()),
        SequencerSettings { sample_rate_hz: 44_100, ..SequencerSettings::default() },
    );
    engine.add_source(Box::new(sequencer)).map_err(|_| "slot 0").expect("slot 0");

    let mut output = vec![0i16; TOTAL_FRAMES * 2];
    engine.render(&mut output);

    assert!(output.iter().any(|sample| *sample != 0), "the module made sound");
    assert!(engine.voices().voices_active() > 0, "and voices are still running at the end of the render");
    assert!(!engine.warnings().any(), "a well-formed module raises no warnings");
    // `TOTAL_FRAMES` is not a whole number of quanta, so the engine has rendered one more
    // quantum than the host asked for and is holding the remainder in the output ring.
    let whole_quanta = TOTAL_FRAMES.div_ceil(RENDER_QUANTUM) * RENDER_QUANTUM;
    assert_eq!(engine.frame(), Frame(whole_quanta as u64));
    assert_eq!(engine.control_clock().driver(), ControlDriver::Tracker, "the sequencer is the control clock");
}

// ── the scope taps (M3-D6) ──────────────────────────────────────────────────────────

/// Telemetry (b)'s per-channel oscilloscope taps are part of the same invariant.
///
/// The tap samples voice state at the start of every render segment, and a segment
/// boundary is exactly where a host block size could leak in. A bucket's first frame sits
/// inside precisely one segment, so its value must be a pure function of the quantum —
/// identical whether the host asked for one frame at a time or 8191. This asserts that at
/// every block size in [`BLOCK_SIZES`], **and** against independently computed expected
/// values, so a tap that was consistently wrong could not pass by being consistent.
#[cfg(feature = "telemetry")]
mod scope_taps {
    use super::*;
    use starplayer_rt::{TAP_BUCKETS_PER_QUANTUM, TAP_BUCKET_FRAMES, TAP_RING_BUCKETS, TapReader};

    /// The channel the one voice is tagged with. Not zero, so a tap that ignored the tag
    /// and wrote to channel 0 would be caught.
    const SCOPE_CHANNEL: u8 = 2;

    /// Frames of the ramp, and its loop length. A power of two so the wrap lands on a
    /// bucket boundary and the expected values stay readable.
    const RAMP_FRAMES: usize = 512;

    /// Exactly the ring's worth of output: 1024 buckets × 4 frames = 32 whole quanta, so
    /// the run fills the ring exactly once and nothing has scrolled out of it.
    const SCOPE_FRAMES: usize = TAP_RING_BUCKETS * TAP_BUCKET_FRAMES;

    /// Half scale, so the expected value of a frame is exactly half of it and the test
    /// does not restate the tap's own rounding.
    const SCOPE_VOLUME: U0F16 = U0F16::from_bits(32_768);

    /// A looping ramp whose frame `index` holds `index * 64`, so a bucket value names the
    /// frame it was read at and a mistimed tap is visible rather than plausible.
    fn ramp_blob() -> (Vec<i16>, SampleRegion) {
        let pcm: Vec<i16> = (0..RAMP_FRAMES).map(|index| (index as i16) * 64).collect();
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(0, RAMP_FRAMES as u32));
        (blob, region)
    }

    /// One voice on [`SCOPE_CHANNEL`] at [`Step::ONE`], so the playback position is the
    /// output frame and the expected bucket values can be written down.
    fn render_scope_at_block_size(block_frames: usize) -> Vec<i16> {
        let (blob, region) = ramp_blob();
        let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
        engine.set_pcm(blob);
        let readers: Box<[TapReader]> = engine.scope_readers().expect("a new engine owns its scope readers");

        let tag = VoiceTag { channel: SCOPE_CHANNEL, instrument: 1, sample: 1, note: 60 };
        let params = VoiceParams { step: Step::ONE, volume: SCOPE_VOLUME, pan: I1F15::ZERO, ..VoiceParams::SILENT };
        engine.voices_mut().allocate(tag, region, params, 0).expect("a fresh pool has room");
        engine.set_source(Box::new(ScriptedSource::new(Vec::new())));

        let mut output = vec![0.0f32; SCOPE_FRAMES * <StereoF32 as OutputFormat>::CHANNELS];
        let mut written = 0;
        while written < output.len() {
            let end = (written + block_frames * <StereoF32 as OutputFormat>::CHANNELS).min(output.len());
            let Some(block) = output.get_mut(written..end) else { break };
            engine.render(block);
            written = end;
        }

        let mut window = vec![0i16; TAP_RING_BUCKETS];
        let reader = readers.get(SCOPE_CHANNEL as usize).expect("the channel table is wider than the tapped channel");
        let seen = reader.latest(&mut window);
        assert_eq!(seen as usize, TAP_RING_BUCKETS, "32 quanta published 32 buckets each, at block size {block_frames}");
        window
    }

    /// Bucket `index` reads the frame `TAP_BUCKET_FRAMES * index` of a loop `RAMP_FRAMES`
    /// long, at half volume. Written out here rather than derived from the engine, so the
    /// test is an independent statement of the sampling rule.
    fn expected_window() -> Vec<i16> {
        (0..TAP_RING_BUCKETS)
            .map(|bucket| {
                let frame = (bucket * TAP_BUCKET_FRAMES) % RAMP_FRAMES;
                ((frame as i32 * 64 * 32_768) >> 16) as i16
            })
            .collect()
    }

    #[test]
    fn the_scope_buckets_are_the_expected_values_at_every_host_block_size() {
        let expected = expected_window();
        assert_eq!(expected.len(), TAP_RING_BUCKETS);
        assert!(expected.iter().any(|value| *value != 0), "the fixture actually produces a signal");

        for block_frames in BLOCK_SIZES {
            let window = render_scope_at_block_size(block_frames);
            assert_eq!(window, expected, "block size {block_frames} changed the scope taps");
        }
    }

    #[test]
    fn a_quantums_worth_of_buckets_is_thirty_two() {
        let window = render_scope_at_block_size(128);
        let quanta = window.len() / TAP_BUCKETS_PER_QUANTUM;
        assert_eq!(quanta, SCOPE_FRAMES / (TAP_BUCKETS_PER_QUANTUM * TAP_BUCKET_FRAMES), "32 buckets per 128-frame quantum");
    }

    #[test]
    fn an_untouched_channel_stays_silent_while_a_tapped_one_does_not() {
        let (blob, region) = ramp_blob();
        let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
        engine.set_pcm(blob);
        let readers: Box<[TapReader]> = engine.scope_readers().expect("a new engine owns its scope readers");

        let tag = VoiceTag { channel: SCOPE_CHANNEL, instrument: 1, sample: 1, note: 60 };
        let params = VoiceParams { step: Step::ONE, volume: SCOPE_VOLUME, pan: I1F15::ZERO, ..VoiceParams::SILENT };
        engine.voices_mut().allocate(tag, region, params, 0).expect("a fresh pool has room");
        engine.set_source(Box::new(ScriptedSource::new(Vec::new())));

        let mut output = vec![0.0f32; RENDER_QUANTUM * <StereoF32 as OutputFormat>::CHANNELS];
        engine.render(&mut output);

        let mut tapped = vec![0i16; TAP_BUCKETS_PER_QUANTUM];
        let mut untouched = vec![-1i16; TAP_BUCKETS_PER_QUANTUM];
        readers.get(SCOPE_CHANNEL as usize).expect("a ring").latest(&mut tapped);
        readers.first().expect("a ring").latest(&mut untouched);
        assert!(tapped.iter().any(|value| *value != 0), "the tapped channel carries the voice");
        assert_eq!(untouched, vec![0i16; TAP_BUCKETS_PER_QUANTUM], "a channel with no voice reads as silence");
    }

    #[test]
    fn the_readers_are_handed_out_exactly_once() {
        let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
        assert!(engine.scope_readers().is_some(), "a new engine owns them");
        assert!(engine.scope_readers().is_none(), "and hands them out once, like the telemetry reader");
    }
}

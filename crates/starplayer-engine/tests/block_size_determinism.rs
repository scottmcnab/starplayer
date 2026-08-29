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

use starplayer_core::{Frame, I1F15, Step, U0F16, VoiceParam, VoiceParams};
use starplayer_dsp::{Interpolate, Linear, Nearest};
use starplayer_engine::{
    Engine, EngineContext, EventSource, MAX_ZERO_ADVANCE, RENDER_QUANTUM, ScriptedAction, ScriptedSource,
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

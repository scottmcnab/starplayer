//! **The M1 mixer against the block-size determinism invariant** (task B5).
//!
//! `block_size_determinism.rs` pins the M0 render loop: one centred voice, one volume
//! change, byte-identical output at every host block size. This file pins everything M1
//! added on top of it — stereo panning, forward and ping-pong looping, linear
//! interpolation, and volume/pan *ramping* — against the same invariant, plus the
//! properties of the mixer that are worth stating on their own: that a hard-panned voice
//! really is silent in the far channel, that a loop does not click at the wrap, and that no
//! degenerate voice can put a NaN into a host's buffer.
//!
//! Ramping is the reason this file exists. A ramp is *state that survives a render call*,
//! and the moment such a thing is driven by anything other than elapsed frames — a
//! per-block increment, a "start of buffer" reset — output stops being independent of the
//! host's buffer size, silently. Architecture §1.4.

use starplayer_core::{Frame, I1F15, Step, U0F16, VoiceParam, VoiceParams};
use starplayer_dsp::{Interpolate, Linear, Nearest};
use starplayer_engine::{Engine, RENDER_QUANTUM, ScriptedAction, ScriptedSource};
use starplayer_mixer::{
    FixedPath, FloatOut, FloatPath, Limiter, LoopSpan, MixPath, OutputFormat, RAMP_FRAMES, SampleRegion, StereoF32, StereoI16, VoiceTag, append_guarded_sample,
};

// ── the scenario ────────────────────────────────────────────────────────────────────

/// The host block sizes the invariant is stated over, in frames. Identical to the M0 set,
/// because it is the same invariant.
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

const TOTAL_FRAMES: usize = 20_000;

const VOICE_CAPACITY: usize = 8;

/// The frames the scheduled changes land on.
///
/// None of them is a multiple of `RENDER_QUANTUM`, and none is a multiple of any block
/// size in `BLOCK_SIZES`, so an implementation that applied a change at a quantum or a
/// buffer boundary would be caught. `RAMP_FRAMES` apart is deliberate too: the second
/// change lands **mid-ramp**, which is the case where a ramp that restarted per block
/// rather than per frame would diverge.
const VOLUME_FRAME: u64 = 1_037;
const PAN_FRAME: u64 = VOLUME_FRAME + RAMP_FRAMES as u64 / 2 + 1;
const STOP_FRAME: u64 = 9_001;

const _: () = assert!(!VOLUME_FRAME.is_multiple_of(RENDER_QUANTUM as u64));
const _: () = assert!(!PAN_FRAME.is_multiple_of(RENDER_QUANTUM as u64));

/// A waveform with no zero-valued frames, so a gain change is audible on whichever frame
/// it lands on, and which joins to itself so that a loop is continuous.
fn waveform() -> Vec<i16> {
    (0..64).map(|index: usize| {
        let phase = index as f64 / 64.0 * core::f64::consts::TAU;
        (phase.sin() * 20_000.0 + 6_000.0) as i16
    }).collect()
}

fn looping_blob(loop_span: Option<LoopSpan>) -> (Vec<i16>, SampleRegion) {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &waveform(), loop_span);
    (blob, region)
}

/// Neither a whole number of frames nor a divisor of the loop length, so the fractional
/// position, the interpolator and the loop boundary are all live throughout the render.
fn playback_step() -> Step { Step::from_ratio(8_363 * 3, 44_100) }

fn voice_params() -> VoiceParams {
    VoiceParams {
        step: playback_step(),
        volume: U0F16::from_bits(49_152),
        // Off-centre, so that the two channels carry different gains and a bug that mixed
        // one and copied it would show.
        pan: I1F15::from_bits(-12_000),
        ..VoiceParams::SILENT
    }
}

/// An engine playing one looping voice, with a volume change, a pan change landing
/// mid-ramp, and a stop — all on frames that sit nowhere near a block boundary.
fn build_engine<Path, Interp, Out>(loop_span: Option<LoopSpan>) -> Engine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let (blob, region) = looping_blob(loop_span);
    let mut engine: Engine<Path, Interp, Out> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);

    let tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
    let voice = engine.voices_mut().allocate(tag, region, voice_params(), 0).expect("a fresh pool has room");

    let actions = vec![
        ScriptedAction::new(Frame(VOLUME_FRAME), voice, VoiceParam::Volume(U0F16::from_bits(16_384))),
        ScriptedAction::new(Frame(PAN_FRAME), voice, VoiceParam::Pan(I1F15::MAX)),
        ScriptedAction::new(Frame(STOP_FRAME), voice, VoiceParam::Volume(U0F16::ZERO)),
    ];
    engine.set_source(Box::new(ScriptedSource::new(actions)));
    engine
}

fn render_at_block_size<Path, Interp, Out>(block_frames: usize, loop_span: Option<LoopSpan>) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut engine = build_engine::<Path, Interp, Out>(loop_span);
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
/// can never silently degrade into a float comparison — `==` would accept `0.0 == -0.0`
/// and reject two identical NaNs, and byte identity is the actual claim.
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

fn first_difference<Sample: BitPattern>(left: &[Sample], right: &[Sample]) -> Option<usize> {
    left.iter().zip(right.iter()).position(|(a, b)| a.to_bytes() != b.to_bytes())
}

fn assert_block_size_independent<Path, Interp, Out>(what: &str, loop_span: Option<LoopSpan>)
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: BitPattern + Default,
{
    let reference = render_at_block_size::<Path, Interp, Out>(RENDER_QUANTUM, loop_span);
    let silence = vec![Out::Sample::default(); reference.len()];
    assert!(first_difference(&reference, &silence).is_some(), "{what}: the scenario has to actually make sound");

    for block_frames in BLOCK_SIZES {
        let output = render_at_block_size::<Path, Interp, Out>(block_frames, loop_span);
        let difference = first_difference(&output, &reference);
        assert_eq!(difference, None, "{what}: block size {block_frames} changed the output at sample {difference:?}");
    }
}

// ── the invariant, with everything M1 added switched on ─────────────────────────────

#[test]
fn a_panned_ramped_forward_loop_is_byte_identical_at_every_host_block_size() {
    let span = LoopSpan::new(16, 64);
    assert_block_size_independent::<FloatPath, Linear, StereoF32>("float / linear / stereo f32", span);
    assert_block_size_independent::<FixedPath, Linear, StereoI16>("fixed / linear / stereo i16", span);
    assert_block_size_independent::<FloatPath, Nearest, StereoF32>("float / nearest / stereo f32", span);
}

#[test]
fn a_ping_pong_loop_is_byte_identical_at_every_host_block_size() {
    // The direction flip is voice state that survives a render call, exactly like a ramp,
    // and it turns round mid-buffer at most block sizes.
    let span = LoopSpan::ping_pong(16, 64);
    assert_block_size_independent::<FloatPath, Linear, StereoF32>("float / linear / ping-pong", span);
    assert_block_size_independent::<FixedPath, Linear, StereoI16>("fixed / linear / ping-pong", span);
}

#[test]
fn a_one_shot_that_ends_mid_buffer_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Linear, StereoF32>("float / linear / one-shot", None);
}

#[test]
fn integer_output_from_the_float_path_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<FloatPath, Linear, FloatOut<i16, 2>>("float / linear / i16 out", LoopSpan::new(16, 64));
}

/// The ramp is the thing this file exists for: a change on a frame that is not a multiple
/// of the quantum must produce the *same ramp* at every block size, and it must start on
/// exactly that frame rather than at the next boundary.
#[test]
fn a_ramp_starting_off_the_quantum_grid_is_identical_at_every_block_size() {
    for block_frames in BLOCK_SIZES {
        let output = render_at_block_size::<FloatPath, Linear, StereoF32>(block_frames, LoopSpan::new(16, 64));
        let reference = render_at_block_size::<FloatPath, Linear, StereoF32>(RENDER_QUANTUM, LoopSpan::new(16, 64));
        assert_eq!(first_difference(&output, &reference), None, "block size {block_frames}");
    }

}

/// The same change, on a DC sample, so that the output *is* the gain envelope and the shape
/// of the ramp can be asserted directly rather than inferred through a waveform.
#[test]
fn a_volume_change_arrives_as_a_ramp_on_exactly_its_frame() {
    const BEFORE: U0F16 = U0F16::MAX;
    const AFTER: U0F16 = U0F16::from_bits(16_384);

    let render = |block_frames: usize| {
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, &[20_000i16; 64], LoopSpan::new(0, 64));
        let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
        engine.set_pcm(blob);
        // Hard left, so the left channel carries the gain undivided by the pan law.
        let params = VoiceParams { step: Step::ONE, volume: BEFORE, pan: I1F15::MIN, ..VoiceParams::SILENT };
        let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");
        engine.voices_mut().get_mut(voice).expect("live").settle_gains();
        let actions = vec![ScriptedAction::new(Frame(VOLUME_FRAME), voice, VoiceParam::Volume(AFTER))];
        engine.set_source(Box::new(ScriptedSource::new(actions)));

        let total = VOLUME_FRAME as usize + RAMP_FRAMES as usize * 4;
        let mut output = vec![0.0f32; total * 2];
        let mut written = 0;
        while written < output.len() {
            let end = (written + block_frames * 2).min(output.len());
            engine.render(&mut output[written..end]);
            written = end;
        }
        let left: Vec<f32> = output.chunks_exact(2).map(|pair| pair.first().copied().unwrap_or(0.0)).collect();
        left
    };

    let envelope = render(RENDER_QUANTUM);
    let at = |frame: u64| envelope.get(frame as usize).copied().unwrap_or(0.0);

    let steady_before = at(VOLUME_FRAME - 1);
    let steady_after = at(VOLUME_FRAME + RAMP_FRAMES as u64 * 2);
    assert!(steady_before > steady_after * 3.0, "the change should have made the voice much quieter");
    assert_eq!(at(0), steady_before, "and nothing before it should have moved");

    // The very first frame of the ramp has moved — the change lands on its own frame, not
    // on the next boundary — but only by about one step of it.
    let first_step = steady_before - at(VOLUME_FRAME);
    assert!(first_step > 0.0, "the change did not land on frame {VOLUME_FRAME}");
    assert!(first_step * 32.0 < steady_before - steady_after, "it landed all at once rather than ramping");

    // Monotone all the way down, and settled exactly on the last frame of the ramp.
    for frame in VOLUME_FRAME..VOLUME_FRAME + RAMP_FRAMES as u64 {
        assert!(at(frame) < at(frame - 1), "the ramp went back up at frame {frame}");
    }
    assert_eq!(at(VOLUME_FRAME + RAMP_FRAMES as u64 - 1), steady_after, "the ramp lands on exactly its last frame");
    assert_eq!(at(VOLUME_FRAME + RAMP_FRAMES as u64), steady_after, "and stays there");

    for block_frames in BLOCK_SIZES {
        assert_eq!(first_difference(&render(block_frames), &envelope), None, "block size {block_frames} changed the ramp");
    }
}

/// A stop is a fade, not a cut: the voice keeps sounding for the length of a ramp and then
/// stops for good.
#[test]
fn a_stopped_voice_fades_rather_than_clicking() {
    let (blob, region) = looping_blob(LoopSpan::new(16, 64));
    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, voice_params(), 0).expect("room");
    engine.voices_mut().get_mut(voice).expect("live").settle_gains();

    let mut before = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut before);
    engine.voices_mut().get_mut(voice).expect("live").stop();

    let mut after = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut after);

    assert_eq!(engine.voices().voices_active(), 0, "the voice was released once the fade landed");
    let sounded = after.iter().take(RAMP_FRAMES as usize * 2).filter(|sample| **sample != 0.0).count();
    assert!(sounded > RAMP_FRAMES as usize, "the fade is audible: {sounded} non-zero samples");
    assert!(
        after.iter().skip(RAMP_FRAMES as usize * 2).all(|sample| *sample == 0.0),
        "and there is nothing at all after it"
    );

    // The fade is monotone in envelope terms: the last frame of it is far below the first.
    let first = after.first().copied().unwrap_or(0.0).abs();
    let last = after.get((RAMP_FRAMES as usize - 1) * 2).copied().unwrap_or(0.0).abs();
    assert!(last < first, "the fade descended: {first} then {last}");
}

// ── panning ─────────────────────────────────────────────────────────────────────────

/// Hard left: the right channel is *silent*, not merely quiet, and the left channel gets
/// the whole signal.
#[test]
fn a_hard_left_voice_is_silent_on_the_right_and_full_on_the_left() {
    let mut blob = Vec::new();
    // A constant full-scale sample, so "full amplitude" is a number rather than an
    // envelope.
    let region = append_guarded_sample(&mut blob, &[32_767i16; 64], LoopSpan::new(0, 64));

    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    // This measures the pan law, not the master bus: a full-scale signal through the
    // soft-knee limiter would read 0.94, so take the transparent path.
    engine.set_limiter(Limiter::Clamp);
    engine.set_pcm(blob);
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, pan: I1F15::MIN, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");
    engine.voices_mut().get_mut(voice).expect("live").settle_gains();

    let mut output = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    for (index, pair) in output.chunks_exact(2).enumerate() {
        if let [left, right] = pair {
            assert_eq!(*right, 0.0, "frame {index} leaked into the right channel");
            assert!(*left > 0.999, "frame {index} was only {left} on the left");
        }
    }

    // And the mirror image, so that the law is not merely silent on one side by accident.
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &[32_767i16; 64], LoopSpan::new(0, 64));
    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    engine.set_limiter(Limiter::Clamp);
    engine.set_pcm(blob);
    let params = VoiceParams { pan: I1F15::MAX, ..params };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");
    engine.voices_mut().get_mut(voice).expect("live").settle_gains();
    let mut output = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    for (index, pair) in output.chunks_exact(2).enumerate() {
        if let [left, right] = pair {
            assert_eq!(*left, 0.0, "frame {index} leaked into the left channel");
            assert!(*right > 0.999, "frame {index} was only {right} on the right");
        }
    }
}

/// Centre is 3 dB down on each channel, which is what "constant power" means and is the
/// audible consequence of the pan law chosen in `starplayer_mixer::gain`.
#[test]
fn a_centred_voice_is_three_decibels_down_on_each_channel() {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &[32_767i16; 64], LoopSpan::new(0, 64));

    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, pan: I1F15::ZERO, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");
    engine.voices_mut().get_mut(voice).expect("live").settle_gains();

    let mut output = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    let left = output.first().copied().unwrap_or(0.0);
    let right = output.get(1).copied().unwrap_or(0.0);
    assert_eq!(left, right, "a centred voice is symmetric");
    let expected = core::f32::consts::FRAC_1_SQRT_2;
    assert!((left - expected).abs() < 0.001, "centre should sit at 1/sqrt(2), not at {left}");
}

// ── the loop point ──────────────────────────────────────────────────────────────────

/// Task B5's verification: the sample-to-sample delta at the wrap never exceeds the
/// largest delta anywhere else in the loop.
#[test]
fn a_forward_loop_does_not_click_at_the_wrap() {
    let pcm = waveform();
    let loop_start = 0u64;
    let loop_end = 64u64;
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(loop_start as u32, loop_end as u32));

    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(blob);
    // A third of a frame per output frame: the wrap lands on a different fraction each
    // time round, so this is not one lucky alignment.
    let step = Step::from_ratio(1, 3);
    let params = VoiceParams { step, volume: U0F16::MAX, pan: I1F15::ZERO, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");
    engine.voices_mut().get_mut(voice).expect("live").settle_gains();

    let frames = 4_000usize;
    let mut output = vec![0.0f32; frames * 2];
    engine.render(&mut output);

    // Which output frames straddle a wrap: those where the whole-loops-completed count
    // changes between one frame and the next.
    let loops_completed = |frame: usize| (frame as u64 * step.to_bits()) >> 32 >= loop_end;
    let position_of = |frame: usize| (frame as u128 * step.to_bits() as u128) >> 32;
    let loop_index = |frame: usize| position_of(frame) / (loop_end - loop_start) as u128;
    assert!(loops_completed(3 * 64 * 2), "the render has to actually wrap");

    let mut largest_at_wrap = 0.0f32;
    let mut largest_elsewhere = 0.0f32;
    let mut wraps = 0;
    for frame in 1..frames {
        let previous = output.get((frame - 1) * 2).copied().unwrap_or(0.0);
        let current = output.get(frame * 2).copied().unwrap_or(0.0);
        let delta = (current - previous).abs();
        if loop_index(frame) != loop_index(frame - 1) {
            wraps += 1;
            largest_at_wrap = largest_at_wrap.max(delta);
        } else {
            largest_elsewhere = largest_elsewhere.max(delta);
        }
    }

    assert!(wraps > 10, "only {wraps} wraps were exercised");
    assert!(
        largest_at_wrap <= largest_elsewhere,
        "the wrap stepped by {largest_at_wrap}, more than the largest step elsewhere ({largest_elsewhere})"
    );
}

// ── degenerate voices ───────────────────────────────────────────────────────────────

/// Nothing a caller can do to a voice may put a NaN, an infinity, or a hang into a host's
/// buffer. `render()` degrades to silence rather than panicking (architecture §8).
#[test]
fn no_degenerate_voice_produces_a_nan_or_an_infinity() {
    let cases: [(&str, &[i16], Option<LoopSpan>, Step); 7] = [
        ("a zero-length sample", &[], None, Step::ONE),
        ("a zero-length sample with a step", &[], None, Step::from_ratio(7, 3)),
        ("a zero step", &[1_000, -1_000, 32_767], None, Step::ZERO),
        ("a step longer than the sample", &[1_000, -1_000, 32_767], None, Step::from_ratio(9_999, 1)),
        ("the largest possible step", &[1_000, -1_000, 32_767], LoopSpan::new(0, 3), Step::MAX),
        ("a one-frame forward loop", &[32_767], LoopSpan::new(0, 1), Step::from_ratio(7, 3)),
        ("a one-frame ping-pong loop", &[32_767], LoopSpan::ping_pong(0, 1), Step::from_ratio(7, 3)),
    ];

    for (what, pcm, loop_span, step) in cases {
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, pcm, loop_span);
        let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
        engine.set_pcm(blob);
        let params = VoiceParams { step, volume: U0F16::MAX, pan: I1F15::from_bits(-9_000), ..VoiceParams::SILENT };
        engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("room");

        let mut output = vec![0.0f32; RENDER_QUANTUM * 4 * 2];
        engine.render(&mut output);
        assert!(output.iter().all(|sample| sample.is_finite()), "{what} produced a non-finite sample");
        assert!(output.iter().all(|sample| sample.abs() <= 1.0), "{what} produced a sample outside full scale");
    }

    // And an empty pool is silence, not a fault.
    let mut engine: Engine<FloatPath, Linear, StereoF32> = Engine::new(VOICE_CAPACITY);
    let mut output = vec![0.0f32; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0.0), "an empty pool renders silence");
    assert!(!engine.warnings().any(), "and raises no warnings doing it");
}

/// A sample region that does not resolve against the blob — a corrupt or mismatched module
/// — ends the voice rather than reading out of bounds.
#[test]
fn an_unresolvable_sample_region_ends_the_voice() {
    let mut engine: Engine<FixedPath, Linear, StereoI16> = Engine::new(VOICE_CAPACITY);
    engine.set_pcm(vec![0i16; 16]);
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT };
    engine.voices_mut().allocate(VoiceTag::default(), SampleRegion::one_shot(9_999, 64), params, 0).expect("room");

    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert_eq!(engine.voices().voices_active(), 0);
    assert!(output.iter().all(|sample| *sample == 0));
}

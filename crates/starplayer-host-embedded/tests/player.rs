//! The `no_std` host driven end to end, on the host.
//!
//! Three claims, and they are the ones that make this crate a *host* rather than a wrapper
//! around `Engine::render`:
//!
//! 1. **Design goal 3 survives the host.** The same song, with the same commands issued on
//!    the same emitted frames, renders byte-identically at every block size — including
//!    block sizes that are smaller than a quantum, and including the ragged short blocks a
//!    command boundary produces.
//! 2. **It is the same audio the std host produces.** Byte for byte, on the fixed path at
//!    `OutputDepth::I16`, for ten seconds.
//! 3. **A command lands on a quantum boundary and not before**, and a retired module comes
//!    back to the control side rather than being freed in the refill.

use starplayer::core::{AtEnd, ChannelId, Interpolator, U0F16};
use starplayer::dsp::Linear;
use starplayer::engine::{MixPathKind, MixerMode, OutputDepth, RENDER_QUANTUM};
use starplayer::rt::Arc;
use starplayer_host::{AudioSpec, ManualBackend, Player};
use starplayer_host_embedded::{ControlHalf, EmbeddedPlayer, RenderHalf};

const PETRI: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");
const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

const RATE_HZ: u32 = 44_100;

/// The six block sizes design goal 3 names, in frames. 1 and 3 are smaller than a quantum,
/// 64 divides it, 128 is exactly one, and 4096 and 8191 straddle many — 8191 deliberately
/// being neither a multiple of 128 nor a power of two.
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4_096, 8_191];

/// Frames every determinism run produces: 200 whole quanta.
const TOTAL_FRAMES: usize = RENDER_QUANTUM * 200;

/// Where the scripted commands are issued, in frames emitted. **None is a multiple of
/// [`RENDER_QUANTUM`]**, so each one has to be carried to the next quantum boundary by the
/// cadence rather than landing where it was issued — which is the whole thing being tested.
const SEEK_AT: usize = 1_000 + 37;
const STOP_AT: usize = 12_345;
const PLAY_AT: usize = 16_501;

const _: () = assert!(!SEEK_AT.is_multiple_of(RENDER_QUANTUM) && !STOP_AT.is_multiple_of(RENDER_QUANTUM) && !PLAY_AT.is_multiple_of(RENDER_QUANTUM));

/// One scripted control action, and the emitted frame the control side takes it on.
#[derive(Copy, Clone, Debug)]
enum Action {
    SeekFrame(u64),
    Stop,
    Play,
}

fn module(bytes: &[u8]) -> Arc<starplayer::model::Module> {
    Arc::new(starplayer::load(bytes).expect("the fixture loads"))
}

fn open(bytes: &[u8]) -> (RenderHalf<Linear>, ControlHalf) {
    EmbeddedPlayer::<Linear>::open(module(bytes), RATE_HZ).expect("the embedded player opens")
}

/// Play `bytes` for [`TOTAL_FRAMES`] frames in blocks of `block_frames`, taking `script`'s
/// actions on exactly the frames it names.
///
/// The stream is cut at each action's frame — so the block handed over there is short —
/// and resumes at `block_frames` afterwards. That is deliberate: the invariant is about
/// *any* sequence of block lengths, and cutting at a fixed emitted frame is the only way to
/// issue a command at the same musical instant in every run.
fn render_script(bytes: &[u8], block_frames: usize, script: &[(usize, Action)]) -> Vec<i16> {
    let (mut render, mut control) = open(bytes);
    control.play().expect("play");

    let mut output = vec![0i16; TOTAL_FRAMES * starplayer_host_embedded::OUTPUT_CHANNELS];
    let mut emitted = 0usize;
    let mut next_action = 0usize;
    while emitted < TOTAL_FRAMES {
        while let Some(&(at_frame, action)) = script.get(next_action)
            && at_frame <= emitted
        {
            match action {
                Action::SeekFrame(song_frame) => control.seek_frame(song_frame).expect("seek"),
                Action::Stop => control.stop().expect("stop"),
                Action::Play => control.play().expect("play"),
            }
            next_action += 1;
        }
        let until = script.get(next_action).map_or(TOTAL_FRAMES, |&(at_frame, _)| at_frame).min(emitted + block_frames).min(TOTAL_FRAMES);
        let first = emitted * starplayer_host_embedded::OUTPUT_CHANNELS;
        let last = until * starplayer_host_embedded::OUTPUT_CHANNELS;
        let chunk = output.get_mut(first..last).expect("the output buffer is long enough");
        render.render(chunk);
        emitted = until;
    }
    output
}

// ── design goal 3: the output does not depend on the block size ────────────────────

#[test]
fn the_same_song_renders_byte_identically_at_every_block_size() {
    let script = [(SEEK_AT, Action::SeekFrame(RATE_HZ as u64)), (STOP_AT, Action::Stop), (PLAY_AT, Action::Play)];
    let reference = render_script(REFLEX, RENDER_QUANTUM, &script);
    assert!(reference.iter().any(|sample| *sample != 0), "the comparison covered non-silent output");

    for block_frames in BLOCK_SIZES {
        let rendered = render_script(REFLEX, block_frames, &script);
        assert_eq!(rendered.len(), reference.len());
        let differing = rendered.iter().zip(&reference).position(|(left, right)| left != right);
        assert_eq!(differing, None, "block size {block_frames} diverged from the 128-frame render at sample {differing:?}");
    }
}

/// The scripted commands really did do something, so the test above is not comparing six
/// copies of an unsteered render.
#[test]
fn the_scripted_commands_change_what_is_rendered() {
    let script = [(SEEK_AT, Action::SeekFrame(RATE_HZ as u64)), (STOP_AT, Action::Stop), (PLAY_AT, Action::Play)];
    let steered = render_script(REFLEX, RENDER_QUANTUM, &script);
    let plain = render_script(REFLEX, RENDER_QUANTUM, &[]);
    assert_ne!(steered, plain, "the seek, the stop and the play left no trace");

    let channels = starplayer_host_embedded::OUTPUT_CHANNELS;
    // The stop is carried to the next quantum boundary and then ramps for 64 frames, so
    // well before the next play the transport is at exact silence.
    let quiet = steered.get((STOP_AT + 512) * channels..(PLAY_AT - 64) * channels).expect("the stopped stretch is in range");
    assert!(quiet.iter().all(|sample| *sample == 0), "the stop reached exact silence and stayed there");
    let resumed = steered.get(PLAY_AT * channels..).expect("the resumed stretch is in range");
    assert!(resumed.iter().any(|sample| *sample != 0), "and the play brought it back");
}

/// A song played *through its end* is the case the alignment exists for: the end-of-song
/// decision has to land on the same frame at every block size, or a stop starts up to a
/// whole block late.
#[test]
fn a_song_played_past_its_end_still_renders_identically_at_every_block_size() {
    let play_to_the_end = |block_frames: usize| {
        let (mut render, mut control) = open(PETRI);
        control.set_at_end(AtEnd::Stop).expect("stop at the end");
        control.play().expect("play");
        let length = control.song_length().expect("a scanned song has a length") as usize;
        let frames = length + 4 * RENDER_QUANTUM;
        let mut output = vec![0i16; frames * starplayer_host_embedded::OUTPUT_CHANNELS];
        for block in output.chunks_mut(block_frames * starplayer_host_embedded::OUTPUT_CHANNELS) {
            render.render(block);
        }
        output
    };

    let reference = play_to_the_end(RENDER_QUANTUM);
    let tail = reference.get(reference.len() - 64..).expect("the tail is in range");
    assert!(tail.iter().all(|sample| *sample == 0), "the song really did stop before the end of the render");
    for block_frames in BLOCK_SIZES {
        assert!(play_to_the_end(block_frames) == reference, "block size {block_frames} took its end decision on a different frame");
    }
}

// ── the same audio the std host produces ───────────────────────────────────────────

/// The std host's fixed / i16 / undithered stereo arm, which is the one whose post-stage
/// `quantize_fixed_sample` reduces to the clamp this crate applies.
const FIXED_I16: MixerMode = MixerMode {
    path: MixPathKind::Fixed,
    interpolator: Interpolator::Linear,
    depth: OutputDepth::I16,
    dither: false,
    channels: 2,
};

#[test]
fn the_embedded_host_renders_what_the_std_host_renders_byte_for_byte() {
    let frames = RATE_HZ as usize * 10;
    let spec = AudioSpec::stereo(RATE_HZ);

    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, FIXED_I16).expect("the manual backend opens");
    let driver = backend.driver().expect("an open backend has a driver");
    player.load_module(module(REFLEX)).expect("the fixture loads");
    player.play().expect("play");
    // `quantize_fixed_sample` at `OutputDepth::I16` hands the callback `code / 32_767.0`,
    // so the exact integer the fixed path produced comes back out of the float by
    // multiplying it in again. No information is lost: every code in `-32_768 ..= 32_767`
    // is representable in an `f32` and so is the quotient.
    let std_host: Vec<i16> = driver
        .render_blocks(spec, frames, RENDER_QUANTUM)
        .into_iter()
        .map(|sample| (sample * 32_767.0).round() as i16)
        .collect();

    let (mut render, mut control) = open(REFLEX);
    control.play().expect("play");
    let mut embedded = vec![0i16; frames * starplayer_host_embedded::OUTPUT_CHANNELS];
    for block in embedded.chunks_mut(RENDER_QUANTUM * starplayer_host_embedded::OUTPUT_CHANNELS) {
        render.render(block);
    }

    assert_eq!(embedded.len(), std_host.len());
    assert!(embedded.iter().any(|sample| *sample != 0), "the comparison covered non-silent output");
    let differing = embedded.iter().zip(&std_host).position(|(left, right)| left != right);
    assert_eq!(differing, None, "the two hosts diverged at sample {differing:?}");
}

// ── the cadence, seen from the outside ─────────────────────────────────────────────

#[test]
fn a_stop_issued_mid_quantum_takes_effect_at_the_next_boundary_and_not_before() {
    let channels = starplayer_host_embedded::OUTPUT_CHANNELS;
    let frames = STOP_AT + 4 * RENDER_QUANTUM;
    let boundary = STOP_AT.next_multiple_of(RENDER_QUANTUM);

    let stopped = {
        let mut output = vec![0i16; frames * channels];
        let (mut render, mut control) = open(REFLEX);
        control.play().expect("play");
        let (before, after) = output.split_at_mut(STOP_AT * channels);
        render.render(before);
        assert!(!render.frames_emitted().is_multiple_of(RENDER_QUANTUM as u64), "the stop is issued mid-quantum");
        control.stop().expect("stop");
        render.render(after);
        output
    };
    let running = {
        let mut output = vec![0i16; frames * channels];
        let (mut render, mut control) = open(REFLEX);
        control.play().expect("play");
        render.render(&mut output);
        output
    };

    let differing = stopped.iter().zip(&running).position(|(left, right)| left != right).expect("the stop changed the output");
    assert!(differing / channels >= boundary, "the stop landed at frame {} — before the boundary at {boundary}", differing / channels);
    assert!(differing / channels < boundary + RENDER_QUANTUM, "and it landed by the quantum after it: {}", differing / channels);
    let tail = stopped.get(stopped.len() - 64..).expect("the tail is in range");
    assert!(tail.iter().all(|sample| *sample == 0), "the ramp reached exact silence");
}

#[test]
fn a_load_retires_the_previous_module_and_its_sequencer_to_the_control_side() {
    let (mut render, mut control) = open(PETRI);
    control.play().expect("play");
    let mut output = vec![0i16; RENDER_QUANTUM * 8 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut output);
    assert_eq!(control.collect_garbage(), 0, "nothing has been replaced yet");

    control.load(module(REFLEX)).expect("second load");
    render.render(&mut output);
    render.render(&mut output);
    // Two, not one: the engine hands back the retired **module** over its garbage channel
    // and `replace_source` hands back the retired **sequencer**, and both go down the same
    // ring to be dropped here. The std host's `Player` counts them the same way.
    assert_eq!(control.collect_garbage(), 2, "the first module and its sequencer both came back");
    assert_eq!(control.pending_garbage(), 0);
    assert!(!control.warnings().retired_module_dropped, "and neither was dropped in the refill");
}

// ── the rest of the control surface ────────────────────────────────────────────────

#[test]
fn a_player_is_silent_until_play_and_the_song_does_not_advance_under_it() {
    let (mut render, mut control) = open(PETRI);
    let mut output = vec![0i16; RENDER_QUANTUM * 40 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0), "an unplayed player is silent");
    assert_eq!(control.song_frame(), 0, "and the song has not advanced under it");
    assert!(!control.is_playing());

    control.play().expect("play");
    render.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0));
    assert!(control.song_frame() > 0);
    assert!(control.blocks_rendered() > 0);
    assert!(control.peak() > 0, "the peak tap moved");
    assert!(!control.warnings().any(), "a well-behaved module raises no engine warnings: {:?}", control.warnings());
}

#[test]
fn master_volume_and_mute_reach_the_engine() {
    let (mut render, mut control) = open(PETRI);
    control.play().expect("play");
    let mut warm = vec![0i16; RENDER_QUANTUM * 200 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut warm);
    let loud = warm.iter().fold(0i32, |peak, sample| peak.max(sample.unsigned_abs() as i32));
    assert!(loud > 0);

    control.set_master_volume(U0F16::from_bits(64)).expect("master volume");
    let mut quiet = vec![0i16; RENDER_QUANTUM * 40 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut quiet);
    let softened = quiet.iter().fold(0i32, |peak, sample| peak.max(sample.unsigned_abs() as i32));
    assert!(softened < loud / 5, "turning the master down turned the output down: {softened} vs {loud}");

    for channel in 0..16 {
        control.mute(ChannelId(channel), true).expect("mute");
    }
    let mut muted = vec![0i16; RENDER_QUANTUM * 40 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut muted);
    render.render(&mut muted);
    assert!(muted.iter().all(|sample| *sample == 0), "every channel muted is silence");
}

#[test]
fn seeking_moves_the_song_clock_without_flagging_the_engine() {
    let (mut render, mut control) = open(PETRI);
    control.play().expect("play");
    let mut output = vec![0i16; RENDER_QUANTUM * 16 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut output);

    let length = control.song_length().expect("a scanned song has a length");
    let target = length / 2;
    control.seek_frame(target).expect("seek");
    for _ in 0..8 { render.render(&mut output); }

    let song_frame = control.song_frame();
    assert!(song_frame >= target, "the seek landed at or after the target: {song_frame} vs {target}");
    assert!(song_frame < target + RATE_HZ as u64, "and not somewhere else entirely");
    assert!(!control.warnings().unsupported_command, "a seek is routed through the mailbox, not flagged");

    control.seek_order(0).expect("seek to the top");
    for _ in 0..8 { render.render(&mut output); }
    assert_eq!(control.telemetry().transport.order, 0);
}

#[test]
fn an_open_empty_player_takes_a_module_later() {
    let module = module(PETRI);
    let settings = starplayer_host_embedded::settings_for(&module, RATE_HZ);
    let (mut render, mut control) = EmbeddedPlayer::<Linear>::open_empty(RATE_HZ, settings).expect("it opens");
    assert!(control.module().is_none());
    assert_eq!(control.song_length(), None);

    let mut output = vec![0i16; RENDER_QUANTUM * 4 * starplayer_host_embedded::OUTPUT_CHANNELS];
    render.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0), "an empty player renders silence rather than refusing");

    control.load(module).expect("the module loads later");
    control.play().expect("play");
    for _ in 0..8 { render.render(&mut output); }
    assert!(output.iter().any(|sample| *sample != 0));
    assert!(control.song_length().is_some_and(|length| length > 0));
}

#[test]
fn a_render_that_is_not_a_whole_number_of_frames_keeps_the_stream_aligned() {
    let channels = starplayer_host_embedded::OUTPUT_CHANNELS;
    let frames = RENDER_QUANTUM * 40;

    let mut ragged = vec![0i16; frames * channels];
    let (mut render, mut control) = open(REFLEX);
    control.play().expect("play");
    // 2 001 samples is 1 000 frames and one sample: every block after the first starts
    // half way through a frame, which is what the sample-counted cadence exists for.
    for block in ragged.chunks_mut(2_001) {
        render.render(block);
    }

    let mut aligned = vec![0i16; frames * channels];
    let (mut render, mut control) = open(REFLEX);
    control.play().expect("play");
    for block in aligned.chunks_mut(RENDER_QUANTUM * channels) {
        render.render(block);
    }
    assert_eq!(ragged, aligned, "a block that is not a whole number of frames changed the stream");
}

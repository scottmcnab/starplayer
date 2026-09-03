//! The host abstraction driven end to end through the [`ManualBackend`], with no device.
//!
//! Everything here is what task D4 promises about a *host*, as opposed to about a backend:
//! that a module reaches the engine, that the transport is click-free, that a retired
//! module is dropped on the caller's thread, that the timeline is scanned at the rate the
//! device agreed to — and, above all, that the output does not depend on the block size the
//! device happens to ask for.

use std::sync::Arc;

use starplayer::core::{AtEnd, ChannelId, Interpolator, U0F16};
use starplayer::engine::{EndReason, MixPathKind, MixerMode, OutputDepth, RENDER_QUANTUM};
use starplayer::rt::Arc as RtArc;
use starplayer_host::{AudioSpec, ManualBackend, ManualDriver, Player, TRANSPORT_RAMP_FRAMES};

const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/NICETUNE.S3M");
const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

/// The six block sizes design goal 3 names, plus the two the engine's own determinism test
/// uses. 8191 is deliberately prime-ish and far larger than a quantum; 1 and 3 are the
/// pathological small ones a resampling backend really does produce.
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4_096, 8_191];

fn open(spec: AudioSpec, mode: MixerMode) -> (ManualBackend, Player, ManualDriver) {
    let mut backend = ManualBackend::new();
    let player = Player::open(&mut backend, None, spec, mode).expect("the manual backend opens");
    let driver = backend.driver().expect("an open backend has a driver");
    (backend, player, driver)
}

/// Play `bytes` for `frames` frames at `block_frames`, from a freshly opened player.
fn render(bytes: &[u8], spec: AudioSpec, mode: MixerMode, frames: usize, block_frames: usize) -> Vec<f32> {
    let (_backend, mut player, driver) = open(spec, mode);
    player.load(bytes).expect("the fixture loads");
    player.play().expect("play");
    driver.render_blocks(spec, frames, block_frames)
}

// ── design goal 3: the output does not depend on the host's block size ──────────────

#[test]
fn the_same_song_renders_byte_identically_at_every_block_size() {
    let spec = AudioSpec::stereo(44_100);
    let reference = render(FIXTURE, spec, MixerMode::DEFAULT, RENDER_QUANTUM * 200, 128);
    assert!(reference.iter().any(|sample| *sample != 0.0), "the comparison covered non-silent output");

    for block_frames in BLOCK_SIZES {
        let rendered = render(FIXTURE, spec, MixerMode::DEFAULT, RENDER_QUANTUM * 200, block_frames);
        assert_eq!(rendered.len(), reference.len());
        let differing = rendered.iter().zip(&reference).position(|(left, right)| left.to_bits() != right.to_bits());
        assert_eq!(differing, None, "block size {block_frames} diverged from the 128-frame render at sample {differing:?}");
    }
}

/// The same invariant on the canonical bit-exact path, where a single differing bit is a
/// real difference rather than a rounding one.
#[test]
fn the_fixed_path_is_block_size_independent_too() {
    let spec = AudioSpec::stereo(44_100);
    let fixed = MixerMode { path: MixPathKind::Fixed, depth: OutputDepth::I16, ..MixerMode::DEFAULT };
    let reference = render(FIXTURE, spec, fixed, RENDER_QUANTUM * 120, 128);
    for block_frames in BLOCK_SIZES {
        assert_eq!(render(FIXTURE, spec, fixed, RENDER_QUANTUM * 120, block_frames), reference, "block size {block_frames}");
    }
}

/// A song played *through its end* is the case the alignment in `RenderState::render`
/// exists for: the end-of-song decision has to land on the same frame at every block size,
/// or a fade or a stop starts up to a whole block late.
#[test]
fn a_song_played_past_its_end_still_renders_identically_at_every_block_size() {
    let spec = AudioSpec::stereo(22_050);
    let mode = MixerMode::DEFAULT;

    let play_to_the_end = |block_frames: usize| {
        let (_backend, mut player, driver) = open(spec, mode);
        player.load(FIXTURE).expect("the fixture loads");
        player.set_at_end(AtEnd::Stop).expect("stop at the end");
        player.play().expect("play");
        let length = player.song_length().expect("a scanned song has a length") as usize;
        driver.render_blocks(spec, length + 4 * RENDER_QUANTUM, block_frames)
    };

    let reference = play_to_the_end(128);
    let tail = &reference[reference.len() - 64..];
    assert!(tail.iter().all(|sample| *sample == 0.0), "the song really did stop before the end of the render");
    for block_frames in BLOCK_SIZES {
        assert_eq!(play_to_the_end(block_frames).len(), reference.len());
        assert!(play_to_the_end(block_frames) == reference, "block size {block_frames} took its end decision on a different frame");
    }
}

// ── the lifecycle ───────────────────────────────────────────────────────────────────

#[test]
fn a_module_reaches_the_engine_and_the_telemetry_reports_it() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    assert!(player.telemetry().sequence == 0, "nothing has been published before a module arrives");

    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 64)];
    driver.render(&mut output);

    assert!(output.iter().any(|sample| *sample != 0.0), "the S3M sequencer triggered sample audio");
    assert!(player.telemetry().sequence > 0, "and the audio thread published a snapshot");
    assert!(player.blocks_rendered() > 0);
    assert!(player.is_playing());
    assert!(player.peak() > 0.0, "the peak tap moved");
    assert!(!player.warnings().any(), "a well-behaved module raises no engine warnings: {:?}", player.warnings());
}

#[test]
fn a_stream_is_silent_until_play_and_the_song_does_not_advance_under_it() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");

    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 40)];
    driver.render(&mut output);
    assert!(output.iter().all(|sample| *sample == 0.0), "an unplayed stream is silent");
    assert_eq!(player.song_frame(), 0, "and the song has not advanced under it");
    assert!(!player.is_playing());

    player.play().expect("play");
    driver.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0.0));
    assert!(player.song_frame() > 0);
}

#[test]
fn play_and_stop_ramp_rather_than_stepping() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");

    let mut first = vec![0.0f32; spec.samples_for(TRANSPORT_RAMP_FRAMES as usize)];
    driver.render(&mut first);
    let opening: Vec<f32> = first.chunks_exact(2).map(|frame| frame[0].abs()).collect();
    assert!(opening.first().copied().unwrap_or(1.0) <= opening.last().copied().unwrap_or(0.0), "the first frames are quieter than the last");

    // Let the song get properly loud, then stop and watch it land on exact silence.
    let mut body = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 200)];
    driver.render(&mut body);
    player.stop().expect("stop");
    let mut stopping = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 4)];
    driver.render(&mut stopping);
    let tail = &stopping[stopping.len() - 32..];
    assert!(tail.iter().all(|sample| *sample == 0.0), "the ramp reached exact silence: {tail:?}");
    assert!(!player.is_playing());
}

#[test]
fn a_retired_module_is_dropped_on_the_control_thread() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 8)];

    player.load(FIXTURE).expect("first load");
    player.play().expect("play");
    driver.render(&mut output);
    assert_eq!(player.collect_garbage(), 0, "nothing has been replaced yet");

    player.load(REFLEX).expect("second load");
    driver.render(&mut output);
    driver.render(&mut output);
    assert_eq!(player.collect_garbage(), 2, "the first module and its sequencer both came back");
    assert_eq!(player.pending_garbage(), 0);
    assert!(!player.warnings().retired_module_dropped, "and neither was dropped on the audio thread");
}

#[test]
fn seeking_moves_the_song_clock_without_flagging_the_engine() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 16)];
    driver.render(&mut output);

    let length = player.song_length().expect("a scanned song has a length");
    let target = length / 2;
    player.seek_frame(target).expect("seek");
    for _ in 0..8 { driver.render(&mut output); }

    let song_frame = player.song_frame();
    assert!(song_frame >= target, "the seek landed at or after the target: {song_frame} vs {target}");
    assert!(song_frame < target + 48_000, "and not somewhere else entirely");
    assert!(!player.warnings().unsupported_command, "a seek is routed through the mailbox, not flagged");

    player.seek_order(0).expect("seek to the top");
    for _ in 0..8 { driver.render(&mut output); }
    assert_eq!(player.telemetry().transport.order, 0);
}

#[test]
fn master_volume_and_mute_reach_the_engine() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    let mut warm = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 200)];
    driver.render(&mut warm);

    let loud = warm.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(loud > 0.0);

    player.set_master_volume(U0F16::from_bits(64)).expect("master volume");
    let mut quiet = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 40)];
    driver.render(&mut quiet);
    let softened = quiet.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(softened < loud * 0.2, "turning the master down turned the output down: {softened} vs {loud}");

    for channel in 0..16 {
        player.mute(ChannelId(channel), true).expect("mute");
    }
    let mut muted = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 40)];
    driver.render(&mut muted);
    driver.render(&mut muted);
    assert!(muted.iter().all(|sample| sample.abs() < 1e-6), "every channel muted is silence");
}

// ── research point 4: the scan belongs to the rate the device agreed to ─────────────

#[test]
fn the_song_is_scanned_at_the_rate_the_device_agreed_to_not_the_rate_that_was_asked_for() {
    // 45 000 Hz is not one of the manual device's rates; it answers 44 100.
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, AudioSpec::stereo(45_000), MixerMode::DEFAULT).expect("it opens");
    assert_eq!(player.spec().sample_rate_hz, 44_100, "the device's answer, not the request");

    player.load(FIXTURE).expect("the fixture loads");
    let at_44100 = player.song_length().expect("scanned");
    let scanned = Arc::clone(player.scan().expect("the player keeps the scan"));
    assert_eq!(scanned.timeline.end(), EndReason::Ended, "NICETUNE runs out of order list");

    // A song's length in *frames* is a function of the output rate, so the same song at
    // twice the rate is twice as many frames — within a tick.
    let mut faster = ManualBackend::new();
    let mut player = Player::open(&mut faster, None, AudioSpec::stereo(88_200), MixerMode::DEFAULT).expect("it opens");
    assert_eq!(player.spec().sample_rate_hz, 96_000, "the nearest rate the manual device has");
    player.load(FIXTURE).expect("the fixture loads");
    let at_96000 = player.song_length().expect("scanned");
    let ratio = at_96000 as f64 / at_44100 as f64;
    assert!((ratio - 96_000.0 / 44_100.0).abs() < 0.01, "the scan followed the rate: {ratio}");
}

#[test]
fn reopening_at_another_rate_rebuilds_the_engine_and_rescans_the_module() {
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT).expect("it opens");
    player.load(FIXTURE).expect("the fixture loads");
    player.set_master_volume(U0F16::from_bits(1_000)).expect("master volume");
    let at_44100 = player.song_length().expect("scanned");
    let module = RtArc::clone(player.module().expect("the player keeps its module"));

    player.reopen(&mut backend, None, AudioSpec::stereo(48_000)).expect("it reopens");
    assert_eq!(player.spec().sample_rate_hz, 48_000);
    let at_48000 = player.song_length().expect("rescanned");
    assert!(at_48000 > at_44100, "the timeline was measured again at the new rate: {at_48000} vs {at_44100}");
    assert!(RtArc::ptr_eq(player.module().expect("module retained"), &module), "and it is the same module, not a reload");

    let driver = backend.driver().expect("the reopened stream has a driver");
    player.play().expect("play");
    let spec = player.spec();
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 64)];
    driver.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0.0), "the rebuilt engine plays");
}

// ── the engine arms ─────────────────────────────────────────────────────────────────

#[test]
fn every_arm_this_host_builds_opens_and_renders() {
    for path in [MixPathKind::Float, MixPathKind::Fixed] {
        for interpolator in [Interpolator::None, Interpolator::Linear] {
            for channels in [1u8, 2] {
                let mode = MixerMode { path, interpolator, depth: OutputDepth::F32, dither: false, channels };
                let spec = AudioSpec { sample_rate_hz: 48_000, channels: channels as u16, preferred_block_frames: None };
                let mut backend = ManualBackend::new();
                let mut player = Player::open(&mut backend, None, spec, mode).expect("the documented arm exists");
                let driver = backend.driver().expect("driver");
                player.load(FIXTURE).expect("the fixture loads");
                player.play().expect("play");
                let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 64)];
                driver.render(&mut output);
                assert!(output.iter().any(|sample| *sample != 0.0), "{mode:?} produced no audio");
            }
        }
    }
}

#[test]
fn an_arm_the_host_cannot_build_is_refused_rather_than_guessed() {
    let mut backend = ManualBackend::new();
    let cubic = MixerMode { interpolator: Interpolator::Cubic, ..MixerMode::DEFAULT };
    assert!(Player::open(&mut backend, None, AudioSpec::stereo(48_000), cubic).is_err());

    let mismatched = MixerMode { channels: 1, ..MixerMode::DEFAULT };
    assert!(Player::open(&mut backend, None, AudioSpec::stereo(48_000), mismatched).is_err(), "a mono engine cannot feed a stereo device");
}

#[test]
fn a_bad_load_leaves_the_previous_module_playing() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    assert!(player.load(b"not a module").is_err());

    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 64)];
    driver.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0.0), "the previous module is still playing");
}

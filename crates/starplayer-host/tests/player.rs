//! The host abstraction driven end to end through the [`ManualBackend`], with no device.
//!
//! Everything here is what task D4 promises about a *host*, as opposed to about a backend:
//! that a module reaches the engine, that the transport is click-free, that a retired
//! module is dropped on the caller's thread, that the timeline is scanned at the rate the
//! device agreed to — and, above all, that the output does not depend on the block size the
//! device happens to ask for.

use std::sync::Arc;

use starplayer::core::{AtEnd, ChannelId, Interpolator, U0F16};
use starplayer::dsp::InsertKind;
use starplayer::dsp::effects::reverb::REVERB_MIX_PARAM;
use starplayer::engine::{EndReason, InsertTarget, MixPathKind, MixerMode, OutputDepth, RENDER_QUANTUM};
use starplayer::rt::Arc as RtArc;
use starplayer_host::{AudioSpec, ManualBackend, ManualDriver, Player, SeekKind, TRANSPORT_RAMP_FRAMES};

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

// ── the mixer-mode rebuild, and the scope readers that go with it ───────────────────

#[test]
fn switching_the_mixer_mode_keeps_the_module_the_scan_and_the_transport() {
    let spec = AudioSpec::stereo(48_000);
    let (mut backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 200)];
    driver.render(&mut output);

    let module = RtArc::clone(player.module().expect("the player keeps its module"));
    let scan = Arc::clone(player.scan().expect("the player keeps the scan"));
    let sounding = player.song_frame();
    assert!(sounding > 0 && player.is_playing());

    let fixed = MixerMode { path: MixPathKind::Fixed, depth: OutputDepth::I16, ..MixerMode::DEFAULT };
    player.set_mixer_mode(&mut backend, None, fixed).expect("the fixed arm exists");
    assert_eq!(player.mixer_mode(), fixed);
    assert!(RtArc::ptr_eq(player.module().expect("module retained"), &module), "a mode switch is not a reload");
    assert!(Arc::ptr_eq(player.scan().expect("scan retained"), &scan), "and the timeline is reused, not measured again");
    assert_eq!(player.collect_garbage(), 0, "nothing went down the garbage channel");

    let driver = backend.driver().expect("the rebuilt stream has a driver");
    driver.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0.0), "the rebuilt engine plays");
    assert!(player.is_playing(), "a mode switch is not a stop");
    assert!(player.song_frame() >= sounding, "and it came back where the ear left it: {} vs {sounding}", player.song_frame());
}

#[test]
fn the_scope_readers_come_out_once_and_follow_a_rebuilt_engine() {
    let spec = AudioSpec::stereo(48_000);
    let (mut backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    let scopes = player.take_scope_readers().expect("a new engine owns its scope readers");
    assert!(!scopes.is_empty());
    assert!(player.take_scope_readers().is_none(), "they come out once");

    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 200)];
    driver.render(&mut output);

    let mut window = vec![0i16; 256];
    let filled = scopes[0].latest(&mut window);
    assert!(filled > 0, "the audio thread wrote into the ring the control thread is reading");
    assert!(window.iter().any(|value| *value != 0), "and the tap carries the module's signal");

    let fixed = MixerMode { path: MixPathKind::Fixed, depth: OutputDepth::I16, ..MixerMode::DEFAULT };
    player.set_mixer_mode(&mut backend, None, fixed).expect("the fixed arm exists");
    assert!(player.take_scope_readers().is_some(), "a rebuilt engine owns a fresh set");
}

// ── the end of the song ─────────────────────────────────────────────────────────────

/// A four-channel MOD one pattern long whose last row is a `B00`, so the song comes round
/// through its own flow rather than merely running out of order list — which is the only
/// case [`AtEnd::FadeOut`] fades (task D2).
fn looping_mod() -> Vec<u8> {
    const SAMPLE_FRAMES: usize = 256;
    let mut bytes = vec![0u8; 1084 + 64 * 4 * 4 + SAMPLE_FRAMES];
    bytes[..10].copy_from_slice(b"looping   ");
    bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[45] = 64;
    bytes[950] = 1;
    bytes[1080..1084].copy_from_slice(b"M.K.");
    let first = starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0, param: 0 };
    bytes[1084..1088].copy_from_slice(&first.to_bytes());
    let last_cell = 1084 + 63 * 4 * 4;
    let jump = starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xB, param: 0x00 };
    bytes[last_cell..last_cell + 4].copy_from_slice(&jump.to_bytes());
    let sample_offset = 1084 + 64 * 4 * 4;
    for (index, byte) in bytes[sample_offset..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0x7F } else { 0x80 }; }
    bytes
}

/// The fade must land **once**. The engine applies its command ring inside its own render,
/// so for one quantum after the fade's `Command::Stop` is sent the engine still reports
/// itself playing — and without the latch in `RenderState` the end-of-song arming fires
/// again on that quantum and re-arms the fade over a song that has already stopped.
#[test]
fn a_fade_that_has_landed_does_not_re_arm_itself_over_the_silence() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = open(spec, MixerMode::DEFAULT);
    player.load(&looping_mod()).expect("the looping fixture loads");
    player.set_fade_frames(4_096).expect("a short fade");
    player.set_at_end(AtEnd::FadeOut).expect("fade at the loop point");
    player.play().expect("play");

    let length = player.song_length().expect("a scanned song has a length") as usize;
    let mut block = vec![0.0f32; spec.samples_for(RENDER_QUANTUM)];
    let mut faded = false;
    for _ in 0..(length / RENDER_QUANTUM + 400) {
        driver.render(&mut block);
        faded |= player.is_fading();
        if !player.is_playing() { break; }
    }
    assert!(faded, "the song reached its loop point and the fade was armed");
    assert!(!player.is_playing(), "and the fade landed in the stop it queues");
    assert!(!player.is_fading(), "the fade is finished, not stuck");

    // Keep rendering over the silence: the snapshot still says the end was reached, and
    // nothing may act on it a second time.
    for _ in 0..40 { driver.render(&mut block); }
    assert!(!player.is_fading(), "a stopped transport does not fade over silence");
    assert!(block.iter().all(|sample| *sample == 0.0), "and it really is silence");
    assert_eq!(player.pending_seek().kind, SeekKind::Frame(0), "a faded-out song rewinds for the next Play");

    // Play is "again", not "louder": the rewind is consumed and the song sounds from the top.
    player.play().expect("play again");
    let mut heard = false;
    for _ in 0..40 {
        driver.render(&mut block);
        heard |= block.iter().any(|sample| *sample != 0.0);
    }
    assert!(heard, "the transport came back to unity rather than staying faded out");
    assert_eq!(player.pending_seek().kind, SeekKind::None, "the rewind was consumed");
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
    // Every interpolator has an arm since M7-task-H5, so the unbuildable mode is now a
    // channel count no arm is instantiated for.
    let six_channel = MixerMode { channels: 6, ..MixerMode::DEFAULT };
    assert!(Player::open(&mut backend, None, AudioSpec::stereo(48_000), six_channel).is_err());

    let mismatched = MixerMode { channels: 1, ..MixerMode::DEFAULT };
    assert!(Player::open(&mut backend, None, AudioSpec::stereo(48_000), mismatched).is_err(), "a mono engine cannot feed a stereo device");
}

#[test]
fn every_interpolator_the_engine_names_has_an_arm() {
    for interpolator in [Interpolator::None, Interpolator::Linear, Interpolator::Cubic, Interpolator::Sinc] {
        let mut backend = ManualBackend::new();
        let mode = MixerMode { interpolator, ..MixerMode::DEFAULT };
        assert!(Player::open(&mut backend, None, AudioSpec::stereo(48_000), mode).is_ok(), "{interpolator:?} has no arm");
    }
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

// ── M7-H7: the insert graph, reached through `Player` ───────────────────────────────
//
// `reflex.s3m` has three channels. `Player` exposes no way to read one channel's bus
// back on its own — the buses are summed inside `render()`, exactly as
// `starplayer-offline/tests/reverb_exit_criterion.rs`'s own comment explains — but
// `Player::mute` reaches the same `is_muted`-before-a-bus check H1 built
// (`VoicePool::accumulate_masked`), so soloing one channel through the host's own mute
// command gets to the same measurement H4's engine-level test makes, through the surface
// an owner's CLI or web page actually calls.

/// Render `bytes` with every channel muted except `solo`, optionally with a reverb
/// installed on `reverb_channel`'s own chain — which may or may not be `solo`. Mutes and
/// the insert are all queued before `play()`, so — like the fixture's own note-off in
/// `reverb_exit_criterion.rs` — they are in force from the render's first frame rather
/// than from whenever the command ring happened to drain.
fn render_soloed(spec: AudioSpec, mode: MixerMode, channel_count: u16, solo: ChannelId, reverb_channel: Option<ChannelId>, frames: usize) -> Vec<f32> {
    let (_backend, mut player, driver) = open(spec, mode);
    player.load(REFLEX).expect("REFLEX loads");
    for index in 0..channel_count {
        let channel = ChannelId(index);
        player.mute(channel, channel != solo).expect("mute queues");
    }
    if let Some(target) = reverb_channel {
        player.install_insert(InsertTarget::Channel(target), 0, InsertKind::Reverb).expect("install queues");
        player.set_insert_param(InsertTarget::Channel(target), 0, REVERB_MIX_PARAM, 50).expect("set_param queues");
    }
    player.play().expect("play");
    driver.render_blocks(spec, frames, 128)
}

/// The host-level form of M7's exit criterion: a reverb installed through
/// [`Player::install_insert`] on one channel changes that channel's own contribution to
/// the mix and leaves every other channel's contribution — measured by soloing it through
/// [`Player::mute`] — bit-identical to a render with no insert installed at all.
#[test]
fn a_reverb_installed_through_the_player_on_one_channel_does_not_change_the_others() {
    let spec = AudioSpec::stereo(44_100);
    let mode = MixerMode::DEFAULT;
    let channel_count: u16 = 3;
    let reverb_channel = ChannelId(1);
    let frames = RENDER_QUANTUM * 400;

    for other in [ChannelId(0), ChannelId(2)] {
        let without_insert = render_soloed(spec, mode, channel_count, other, None, frames);
        let with_reverb_elsewhere = render_soloed(spec, mode, channel_count, other, Some(reverb_channel), frames);
        assert_eq!(with_reverb_elsewhere, without_insert, "channel {other:?} changed when a reverb was installed on a different channel");
    }

    let reverb_channel_plain = render_soloed(spec, mode, channel_count, reverb_channel, None, frames);
    let reverb_channel_with_reverb = render_soloed(spec, mode, channel_count, reverb_channel, Some(reverb_channel), frames);
    assert_ne!(reverb_channel_with_reverb, reverb_channel_plain, "installing a reverb on its own channel changed nothing audible");
}

/// [`Player::inserts`] reports back what was installed, including after a slot is
/// removed and after the bypass bit flips — the host-side bookkeeping
/// [`Player::set_mixer_mode`] replays across a rebuild.
#[test]
fn the_player_reports_back_what_it_believes_is_installed() {
    let spec = AudioSpec::stereo(44_100);
    let (_backend, mut player, _driver) = open(spec, MixerMode::DEFAULT);
    player.load(REFLEX).expect("REFLEX loads");
    let target = InsertTarget::Channel(ChannelId(0));

    assert!(player.inserts().get(target, 0).is_none());
    player.install_insert(target, 0, InsertKind::Reverb).expect("install queues");
    assert_eq!(player.inserts().get(target, 0).map(|insert| insert.kind), Some(InsertKind::Reverb));

    player.set_insert_param(target, 0, REVERB_MIX_PARAM, 42).expect("set_param queues");
    assert_eq!(player.inserts().get(target, 0).and_then(|insert| insert.param(REVERB_MIX_PARAM)), Some(42));

    player.bypass_insert(target, 0, true).expect("bypass queues");
    assert!(player.inserts().get(target, 0).unwrap().bypassed);

    player.remove_insert(target, 0).expect("remove queues");
    assert!(player.inserts().get(target, 0).is_none());
}

/// A slot outside the fixed topology is refused rather than silently retiring whatever the
/// engine would have built.
#[test]
fn an_out_of_range_insert_slot_is_refused() {
    let spec = AudioSpec::stereo(44_100);
    let (_backend, mut player, _driver) = open(spec, MixerMode::DEFAULT);
    player.load(REFLEX).expect("REFLEX loads");
    let target = InsertTarget::Channel(ChannelId(0));
    assert!(player.install_insert(target, 4, InsertKind::Gain).is_err(), "there are only four slots, 0..=3");
}

/// [`Player::set_mixer_mode`] replays the insert layout into the rebuilt engine, so the
/// effect keeps sounding across a mode change exactly as the module itself does.
#[test]
fn the_insert_layout_survives_a_mixer_mode_rebuild() {
    let spec = AudioSpec::stereo(44_100);
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).expect("open");
    player.load(REFLEX).expect("REFLEX loads");
    let target = InsertTarget::Channel(ChannelId(1));
    player.install_insert(target, 0, InsertKind::Reverb).expect("install queues");
    player.set_insert_param(target, 0, REVERB_MIX_PARAM, 60).expect("set_param queues");

    let fixed = MixerMode { path: MixPathKind::Fixed, depth: OutputDepth::I16, ..MixerMode::DEFAULT };
    player.set_mixer_mode(&mut backend, None, fixed).expect("rebuild");

    assert_eq!(player.inserts().get(target, 0).map(|insert| insert.kind), Some(InsertKind::Reverb));
    assert_eq!(player.inserts().get(target, 0).and_then(|insert| insert.param(REVERB_MIX_PARAM)), Some(60));

    player.play().expect("play");
    let driver = backend.driver().expect("driver");
    let mut output = vec![0.0f32; spec.samples_for(RENDER_QUANTUM * 64)];
    driver.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0.0), "the rebuilt engine, with the replayed reverb, still produces audio");
}

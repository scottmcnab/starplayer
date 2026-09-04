//! Live input (task E6) driven end to end through the [`ManualBackend`], with no device
//! and no MIDI hardware.
//!
//! What this file is about is the *stamp* and the *queue*: that an event lands on
//! `source_frame + lead`, that a lead shorter than the device's block is raised to cover
//! it, that a full queue is counted rather than waited on, and that a note pushed at a
//! silent module's instruments actually sounds. The engine half — the rack, the held-note
//! bookkeeping, the pitch convention — is task E4's, and is tested there.

use starplayer::core::{Event, Frame, InstrumentId, Note, U0F16};
use starplayer::engine::{MixerMode, RENDER_QUANTUM};
use starplayer_host::{AudioSpec, HostError, ManualBackend, ManualDriver, Player};

const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/NICETUNE.S3M");

fn open(spec: AudioSpec) -> (ManualBackend, Player, ManualDriver) {
    let mut backend = ManualBackend::new();
    let player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).expect("the manual backend opens");
    let driver = backend.driver().expect("an open backend has a driver");
    (backend, player, driver)
}

/// A player with the fixture loaded, playing, and its own instruments on live input.
fn jamming(spec: AudioSpec) -> (ManualBackend, Player, ManualDriver) {
    let (backend, mut player, driver) = open(spec);
    player.load(FIXTURE).expect("the fixture loads");
    player.midi_only().expect("live input installs over a loaded module");
    player.play().expect("play");
    (backend, player, driver)
}

fn note_on(note: u8) -> Event { Event::NoteOn { note: Note::new(note), velocity: U0F16::MAX } }

fn peak(block: &[f32]) -> f32 { block.iter().fold(0.0f32, |loudest, sample| loudest.max(sample.abs())) }

#[test]
fn live_input_needs_a_module_because_the_rack_is_built_from_its_instruments() {
    let (_backend, mut player, _driver) = open(AudioSpec::stereo(48_000));
    assert_eq!(player.midi_only(), Err(HostError::NoModule));
    assert_eq!(player.send_event(0, note_on(60)), Err(HostError::NoEventQueue), "and nothing can be sent before one exists");
    assert!(!player.is_midi_only());
}

#[test]
fn an_event_is_stamped_at_the_musical_clock_plus_the_lead() {
    let spec = AudioSpec { preferred_block_frames: Some(RENDER_QUANTUM as u32), ..AudioSpec::stereo(48_000) };
    let (_backend, mut player, driver) = jamming(spec);

    // Two quanta of real render, so the musical clock is somewhere other than zero.
    let _ = driver.render_blocks(spec, RENDER_QUANTUM * 2, RENDER_QUANTUM);
    let source_frame = player.source_frame();
    assert!(source_frame > Frame::ZERO, "the transport is running");

    let lead = player.event_lead();
    let sender = player.take_event_sender().expect("live input installed a sender");
    assert_eq!(sender.next_stamp(), source_frame.saturating_add(lead as u64), "output_frame + lead, on the musical clock");
    assert_eq!(player.send_event(0, note_on(60)), Err(HostError::NoEventQueue), "the sender left, so the player cannot send");
}

/// Research point 2: a lead shorter than the device's own block is a guarantee of late
/// events, because the control thread's view of the clock is a whole block stale while a
/// callback is running. The lead is therefore floored at the observed block plus a quantum.
#[test]
fn the_lead_grows_to_cover_whatever_block_the_device_actually_asks_for() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, driver) = jamming(spec);
    assert_eq!(player.event_lead(), 2 * RENDER_QUANTUM as u32, "two quanta before a block has been seen");

    let _ = driver.render_blocks(spec, RENDER_QUANTUM, RENDER_QUANTUM);
    assert_eq!(player.event_lead(), 2 * RENDER_QUANTUM as u32, "a 128-frame block is already covered");

    // The measurement behind the floor: the control thread's view of the musical clock is
    // republished once per callback, so while a callback is running that view is stale by
    // exactly the block the device asked for. An event stamped less than a block ahead is
    // therefore stamped into audio that has already been rendered.
    let before = player.source_frame();
    let _ = driver.render_blocks(spec, 4_096, 4_096);
    assert_eq!(before.frames_until(player.source_frame()), 4_096, "one publication per callback, whatever its length");
    assert_eq!(player.event_lead(), 4_096 + RENDER_QUANTUM as u32, "a 4096-frame block raises the floor");
    assert!((player.event_lead_millis() - 88.0).abs() < 0.01, "{} ms at 48 kHz", player.event_lead_millis());

    player.set_event_lead(48_000);
    assert_eq!(player.event_lead(), 48_000, "a caller asking for a whole second gets one");
}

#[test]
fn a_full_queue_reports_rather_than_blocking_and_the_count_is_readable() {
    let spec = AudioSpec::stereo(48_000);
    let (_backend, mut player, _driver) = jamming(spec);

    // Nothing has rendered, so nothing drains: the queue fills at its capacity and every
    // event past it is refused. This is the pathological case a jammed audio thread would
    // produce, and the point is that it returns rather than waits.
    let mut accepted = 0u64;
    let mut refused = 0u64;
    for index in 0..1_000 {
        match player.send_event(0, note_on(60 + (index % 12) as u8)) {
            Ok(()) => accepted += 1,
            Err(HostError::EventQueueFull) => refused += 1,
            Err(other) => panic!("unexpected error {other}"),
        }
    }
    assert_eq!(accepted, starplayer_host::EVENT_QUEUE_CAPACITY as u64, "the ring took exactly its capacity");
    assert_eq!(refused, 1_000 - accepted);
    assert_eq!(player.events_sent(), accepted);
    assert_eq!(player.events_rejected(), refused);
}

/// The deliverable's own acceptance case: a module that is not playing, and a note that
/// sounds anyway, out of that module's instruments.
#[test]
fn a_note_sounds_from_a_silent_modules_instruments() {
    let spec = AudioSpec { preferred_block_frames: Some(RENDER_QUANTUM as u32), ..AudioSpec::stereo(48_000) };
    let (_backend, mut player, driver) = jamming(spec);
    assert!(player.is_midi_only());

    // Live input alone: the module's own pattern data is not playing, so this is silence.
    let silence = driver.render_blocks(spec, RENDER_QUANTUM * 8, RENDER_QUANTUM);
    assert_eq!(peak(&silence), 0.0, "the module itself is silent under a live-input source");

    player.send_event(0, Event::Program(InstrumentId(0))).expect("program change");
    player.send_event(0, note_on(60)).expect("note on");
    let sounding = driver.render_blocks(spec, RENDER_QUANTUM * 40, RENDER_QUANTUM);
    assert!(peak(&sounding) > 0.001, "the note sounded (peak {})", peak(&sounding));

    player.send_event(0, Event::AllSoundOff).expect("all sound off");
    // Two quanta of lead, so the first block still carries the note that was sounding when
    // the event was stamped; what matters is that everything after it is silent.
    let _ = driver.render_blocks(spec, RENDER_QUANTUM * 4, RENDER_QUANTUM);
    let after = driver.render_blocks(spec, RENDER_QUANTUM * 8, RENDER_QUANTUM);
    assert_eq!(peak(&after), 0.0, "and stopped when it was told to");
    assert_eq!(player.events_rejected(), 0, "nothing was refused along the way");
}

#[test]
fn restoring_the_module_source_ends_live_input_and_the_module_plays_again() {
    let spec = AudioSpec { preferred_block_frames: Some(RENDER_QUANTUM as u32), ..AudioSpec::stereo(48_000) };
    let (_backend, mut player, driver) = jamming(spec);
    let _ = driver.render_blocks(spec, RENDER_QUANTUM * 8, RENDER_QUANTUM);

    player.restore_module_source().expect("the module goes back");
    assert!(!player.is_midi_only());
    assert_eq!(player.send_event(0, note_on(60)), Err(HostError::NoEventQueue), "the queue went with the source");

    let rendered = driver.render_blocks(spec, RENDER_QUANTUM * 200, RENDER_QUANTUM);
    assert!(peak(&rendered) > 0.001, "the module's own pattern data is sounding again (peak {})", peak(&rendered));
}

/// Live input survives a mixer-mode rebuild, because the rebuilt engine is handed a fresh
/// queue. The sender a caller had already taken does not — which is what the doc comment
/// on [`Player::take_event_sender`] says, and what this pins down.
#[test]
fn a_rebuilt_engine_comes_back_jamming_with_a_new_queue() {
    let spec = AudioSpec { preferred_block_frames: Some(RENDER_QUANTUM as u32), ..AudioSpec::stereo(48_000) };
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).expect("open");
    player.load(FIXTURE).expect("the fixture loads");
    player.midi_only().expect("live input installs");
    player.set_event_lead(1_024);
    player.play().expect("play");
    // `set_mixer_mode` resumes the transport only if it was *seen* running, which is a tap
    // the render callback writes — so the rebuild has to happen after the stream has run.
    let before = backend.driver().expect("an open backend has a driver");
    let _ = before.render_blocks(spec, RENDER_QUANTUM * 4, RENDER_QUANTUM);
    assert!(player.is_playing());

    let mode = MixerMode { interpolator: starplayer::core::Interpolator::None, ..MixerMode::DEFAULT };
    player.set_mixer_mode(&mut backend, None, mode).expect("the mode rebuild succeeds");
    let driver = backend.driver().expect("the rebuilt backend has a driver");

    assert!(player.is_midi_only(), "the rebuilt engine came back on live input");
    assert_eq!(player.event_lead(), 1_024, "and kept the lead the caller asked for");
    player.send_event(0, Event::Program(InstrumentId(0))).expect("program change");
    player.send_event(0, note_on(60)).expect("note on");
    let sounding = driver.render_blocks(spec, RENDER_QUANTUM * 60, RENDER_QUANTUM);
    assert!(peak(&sounding) > 0.001, "and the new queue reaches the new engine (peak {})", peak(&sounding));
}

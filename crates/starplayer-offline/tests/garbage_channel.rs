//! M2-C7 deliverable 7: the garbage channel, proved across two real threads.
//!
//! `crates/starplayer-engine/tests/sequencer_and_control_plane.rs` already shows that a
//! retired module is not dropped *inside* `render()`. That test runs on one thread, so it
//! can only say "not during the call"; the architecture §8 claim is stronger than that —
//! **the destructor runs on the control thread and never on the audio thread**, because
//! `free()` takes the allocator's lock and an audio callback cannot wait for one.
//!
//! So this test puts the engine on a dedicated thread that does nothing but call
//! `render()` in a loop, keeps the control handle on the test thread, swaps a real loaded
//! module for a second one while the audio thread is running, and records the
//! [`ThreadId`](std::thread::ThreadId) each module's destructor actually ran on.
//!
//! Removing the `garbage.retire(retired)` call in `Engine::apply_command` — dropping the
//! handle inline instead — makes this test fail with the audio thread's id, which is the
//! negative check the C7 verification asks for.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc as StdArc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use starplayer::dsp::Linear;
use starplayer::engine::{Engine, EngineHandle, EngineSettings, PcmSource};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::Module;
use starplayer::rt::Arc;

/// How long a handshake may take before the test gives up rather than hanging CI.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A real loaded module that records where its destructor ran.
///
/// The `Module` inside is genuine — the C6a synthesised MOD and MTM, loaded by their own
/// loaders — so the `Arc` being retired owns exactly the pattern blob and PCM blob a host
/// would be freeing.
struct TrackedModule {
    module: Module,
    label: &'static str,
    drops: StdArc<Mutex<Vec<(&'static str, ThreadId)>>>,
}

impl PcmSource for TrackedModule {
    fn pcm(&self) -> &[i16] { self.module.pcm() }
}

impl Drop for TrackedModule {
    fn drop(&mut self) {
        if let Ok(mut log) = self.drops.lock() {
            log.push((self.label, thread::current().id()));
        }
    }
}

type TrackedEngine = Engine<FixedPath, Linear, StereoI16, Arc<TrackedModule>>;

fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

#[test]
fn a_retired_module_is_destroyed_on_the_control_thread_and_never_on_the_audio_thread() {
    let drops: StdArc<Mutex<Vec<(&'static str, ThreadId)>>> = StdArc::new(Mutex::new(Vec::new()));
    let control_thread = thread::current().id();

    let first = Arc::new(TrackedModule {
        module: starplayer::mod_file::load(&starplayer_offline::fixtures::synthetic_mod()).expect("the synthesised MOD loads"),
        label: "first",
        drops: StdArc::clone(&drops),
    });
    let second = Arc::new(TrackedModule {
        module: starplayer::mtm::load(&starplayer_offline::fixtures::synthetic_mtm()).expect("the synthesised MTM loads"),
        label: "second",
        drops: StdArc::clone(&drops),
    });

    // The engine is built on the audio thread and stays there: it is not `Send` — it owns
    // `Box<dyn EventSource>` slots — so the *handle* is what crosses, which is exactly the
    // split architecture §8 describes.
    let (handle_sender, handle_receiver) = mpsc::channel::<(EngineHandle<Arc<TrackedModule>>, ThreadId)>();
    let keep_rendering = StdArc::new(AtomicBool::new(true));
    let audio_thread_should_stop = StdArc::clone(&keep_rendering);

    let audio_thread = thread::Builder::new()
        .name("starplayer-audio".to_string())
        .spawn(move || {
            let settings = EngineSettings { sample_rate_hz: 44_100, channel_count: 8, voice_capacity: 8, ..EngineSettings::default() };
            let mut engine: TrackedEngine = Engine::with_settings(settings);
            let handle = engine.take_control().expect("a fresh engine owns its control handle");
            handle_sender.send((handle, thread::current().id())).expect("the test thread is waiting for the handle");

            // 128 frames per call is one whole `RENDER_QUANTUM`, which is what a real
            // callback looks like. Nothing here ever drops a module.
            let mut block = vec![0i16; 128 * 2];
            while audio_thread_should_stop.load(Ordering::Relaxed) {
                engine.render(&mut block);
            }
            engine.warnings()
        })
        .expect("the audio thread starts");

    let (mut control, audio_thread_id) = handle_receiver.recv_timeout(HANDSHAKE_TIMEOUT).expect("the audio thread handed its control handle over");
    assert_ne!(audio_thread_id, control_thread, "the two halves really are on different threads");

    // Load the first module and let go of it here, so the engine holds the only reference
    // and the destructor can only run where the last handle dies.
    control.load_module(Arc::clone(&first)).map_err(|_| "full").expect("the command ring has room");
    drop(first);
    assert!(wait_until(|| control.queued_commands() == 0), "the audio thread applied the first load");
    assert!(drops.lock().expect("the drop log").is_empty(), "nothing has been destroyed yet");

    // Swap the second module in while the audio thread is mid-flight. This is the moment
    // the first `Arc` is retired.
    control.load_module(Arc::clone(&second)).map_err(|_| "full").expect("the command ring has room");
    drop(second);
    assert!(wait_until(|| control.pending_garbage() > 0), "the retired module reached the garbage channel");
    assert!(drops.lock().expect("the drop log").is_empty(), "and it is still alive: retiring is not dropping");

    // Collect on the control thread. This is the only place a retired module dies.
    assert_eq!(control.collect_all_garbage(), 1);
    {
        let log = drops.lock().expect("the drop log");
        assert_eq!(log.len(), 1, "exactly one module has been destroyed");
        assert_eq!(log.first().map(|entry| entry.0), Some("first"), "and it is the one that was swapped out");
        assert_eq!(log.first().map(|entry| entry.1), Some(control_thread), "the control thread ran the destructor");
        assert_ne!(log.first().map(|entry| entry.1), Some(audio_thread_id), "the audio thread never did");
    }

    keep_rendering.store(false, Ordering::Relaxed);
    let warnings = audio_thread.join().expect("the audio thread finished cleanly");
    assert!(!warnings.retired_module_dropped, "no module had to be dropped inline on the audio thread");
    assert!(!warnings.any(), "and the render loop raised no other warning: {warnings:?}");

    // The engine still holds the second module; it dies when the audio thread's `Engine`
    // is dropped, which has already happened by the time `join` returned.
    let log = drops.lock().expect("the drop log");
    assert_eq!(log.len(), 2, "both modules are accounted for");
    assert_eq!(log.get(1).map(|entry| entry.0), Some("second"));
    assert_eq!(log.get(1).map(|entry| entry.1), Some(audio_thread_id), "a module still loaded at shutdown dies with the engine");
}

//! [`Player`] — the backend-neutral controller.
//!
//! Everything the browser host's `Host` does that is not browser-specific lives here, so
//! that a backend is only ever "implement [`AudioBackend`]" and never "reimplement the
//! transport".
//!
//! # The two sides, and what crosses between them
//!
//! A [`Player`] is the **control** side: it holds the stream, the telemetry, the seek
//! mailbox and the module. A [`RenderState`] is the **audio** side: it holds the engine,
//! its command handle and the transport, and it lives inside the backend's callback for as
//! long as the stream is open. Once [`Player::open`] returns, the audio side is
//! unreachable from this thread — which is the point.
//!
//! Several things cross, and each has exactly one mechanism:
//!
//! | | direction | mechanism |
//! |---|---|---|
//! | commands, and a freshly built module and source | control → audio | [`HostCommand`] over an SPSC ring |
//! | seeks | control → audio | [`SeekMailbox`], latest wins |
//! | retired modules and retired sources | audio → control | an SPSC ring; **dropped here**, never there |
//! | telemetry, position, peak | audio → control | a snapshot channel and a handful of atomics |
//! | live MIDI and keyboard events | control (or any thread) → audio | an [`ExternalEventQueue`](starplayer::engine::ExternalEventQueue) of absolutely-stamped events; see `src/events.rs` |
//! | insert commands (install, remove, set a parameter, bypass, reset) | control → audio | [`HostInsertControl`](crate::HostInsertControl) directly onto the engine's own insert ring — `Player` holds the handle itself rather than routing through `HostCommand`, because unlike the transport an insert change has no ramp to sequence around (M7-H7) |
//! | retired insert effects | audio → control | the engine's insert garbage channel, drained by [`Player::collect_garbage`] alongside retired modules |
//!
//! # Why the module travels over the ring rather than through `EngineHandle`
//!
//! Because the transport does. A click-free stop is "ramp for 64 frames, *then* send
//! `Command::Stop`", and the frame that lands on is only known inside the callback — so the
//! engine's own command producer has to be there, and there is only one of it. The control
//! side therefore speaks [`HostCommand`], the audio side speaks
//! [`Command`](starplayer::core::Command), and the audio side is the translator. The
//! browser host does not need this only because everything it owns is already on one
//! thread.

use starplayer::core::{InteractiveAction, InteractiveCommand, InteractiveTransport, PlaybackIdentity};
use std::boxed::Box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::vec::Vec;

use starplayer::core::{AtEnd, ChannelId, Command, Event, Frame, U0F16};
use starplayer::dsp::{InsertKind, ParamId};
use starplayer::engine::{
    EngineHandle, EngineSettings, EngineWarnings, EventSource, ExternalEventProducer, InsertTarget, InstrumentRack, MAX_INSERTS_PER_CHAIN,
    MidiSource, MixerMode, RENDER_QUANTUM, SourceMux, external_event_channel,
};
use starplayer::model::Module;
use starplayer::rt::{
    Arc as RtArc, Consumer, Producer, SnapshotPublisher, SnapshotReader, TapReader, channel, snapshot_channel,
};
use starplayer::telemetry::{Snapshot, SongEnd, TelemetryReader};
use starplayer::{ScannedSong, recommended_voice_capacity};

use crate::backend::{AudioBackend, AudioSpec, HostError, Stream, StreamHealth};
use crate::engine::HostEngine;
use crate::events::{EVENT_QUEUE_CAPACITY, EventClock, EventSender};
use crate::insert::{HostInsertControl, InsertLayout};
use crate::source::{SeekKind, SeekRequest, SourceHandles, build_source_with_settings};
use crate::transport::Transport;

/// Commands the control side may have outstanding before it starts getting refusals.
///
/// Generous: commands are sparse — load, seek, mute, master volume — and the audio side
/// drains the whole ring at the top of every render quantum.
pub const HOST_COMMAND_CAPACITY: usize = 64;

/// Retired modules and sources waiting to be dropped on the control thread.
///
/// Small on purpose, exactly as the engine's own garbage channel is: a host with eight
/// retirements outstanding has stopped calling [`Player::collect_garbage`], and the
/// backlog is the useful signal.
pub const RETIRED_CAPACITY: usize = 16;

/// Snapshots buffered on the way from the audio thread to the caller.
pub const TELEMETRY_DEPTH: usize = 3;

/// One instruction from the control side to the render callback.
enum HostCommand {
    AppliedBarrier { identity: PlaybackIdentity, serial: u64, rejected: u64 },
    Interactive(InteractiveCommand),
    RestoreTransport { identity: PlaybackIdentity, transport: Transport, state: InteractiveTransport, natural_end: bool },
    RestoreSelections { identity: PlaybackIdentity, enabled: bool, selections: Box<[bool]> },
    /// Ramp up and start the musical clock.
    Play,
    /// Ramp down and stop it when the ramp lands.
    Stop,
    /// A module and the source built for it, both already decoded, scanned and allocated
    /// on the control thread. Nothing here is constructed in the callback and nothing is
    /// dropped there.
    Load { module: RtArc<Module>, source: Box<dyn EventSource>, identity: Option<PlaybackIdentity>, selections: Option<Box<[bool]>>, new_session: bool },
    /// What happens at the loop point, and how long the fade is when there is one.
    SetAtEnd { at_end: AtEnd, fade_frames: u32 },
    /// Pass straight through to the engine.
    Engine(Command<RtArc<Module>>),
}

/// Something the audio thread finished with and may not drop.
///
/// Nothing ever reads either payload: being dropped on the *control* thread is the entire
/// value of this type, and the `expect` says so rather than leaving a reader to wonder.
#[expect(dead_code, reason = "the payload exists to be dropped on the control thread, never to be read")]
enum Retired {
    Module(RtArc<Module>),
    Installation { source: Option<Box<dyn EventSource>>, selections: Option<Box<[bool]>> },
    Selections(Box<[bool]>),
}

/// Which live-input arrangement the installed source uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LiveInputMode {
    #[default]
    Off,
    MidiOnly,
    Jam,
}

/// The lossy audio taps a caller reads without waiting for a snapshot.
///
/// Every field is written once per callback and read whenever the caller asks; a reading
/// one block old costs a progress bar nothing (architecture §9).
#[derive(Debug, Default)]
struct PlayerTaps {
    source_frame: AtomicU64,
    output_frame: AtomicU64,
    playing: AtomicBool,
    transport_state: AtomicU32,
    natural_end: AtomicBool,
    /// Whether the song fade is running, which a UI shows and a wire header carries.
    fading: AtomicBool,
    /// `f32::to_bits` of the last block's peak.
    peak_bits: AtomicU32,
    blocks_rendered: AtomicU64,
    commands_rejected: AtomicU64,
    pending_engine_commands: AtomicU32,
}

/// A control barrier published only after preceding commands have reached rendered audio.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedPlayback {
    pub identity: Option<PlaybackIdentity>,
    pub serial: u64,
    pub accepted: bool,
    pub capabilities: starplayer::core::PlaybackCapabilities,
}

#[path = "replacement.rs"]
mod replacement;

/// Coherent engine and host fade state, copied together for same-session rebuilds.
#[derive(Copy, Clone, Debug, Default)]
struct PlayerSnapshot {
    applied: AppliedPlayback,
    engine: Snapshot,
    source_frame: Frame,
    transport: Transport,
    state: InteractiveTransport,
    natural_end: bool,
}

/// The audio side: everything the backend's callback owns.
struct RenderState {
    applied: AppliedPlayback,
    pending_barrier: Option<(PlaybackIdentity, u64, u64)>,
    engine: HostEngine,
    control: EngineHandle<RtArc<Module>>,
    telemetry: TelemetryReader,
    forwarded: SnapshotPublisher<PlayerSnapshot>,
    commands: Consumer<HostCommand>,
    /// Preserve command order while a prepared load waits for off-thread retirement.
    pending_command: Option<HostCommand>,
    retired: Producer<Retired>,
    handles: SourceHandles,
    transport: Transport,
    interactive_identity: Option<PlaybackIdentity>,
    installed_identity: Option<PlaybackIdentity>,
    interactive_enabled: bool,
    interactive_state: InteractiveTransport,
    taps: Arc<PlayerTaps>,
    last_snapshot: Snapshot,
    /// A typed [`Command::Stop`] the engine has been sent and has not acted on yet.
    ///
    /// The engine applies its command ring inside its own render, so for one whole quantum
    /// after a stop is sent `engine.is_playing()` still says yes. Without this latch
    /// [`RenderState::arm_end_of_song`] fires a second time over a song that has already
    /// finished — re-arming the very fade that just landed, which leaves the transport
    /// reporting a fade over silence.
    awaiting_engine_stop: bool,
    awaiting_restart: bool,
    /// Frames handed to the backend so far. The clock the control-plane cadence is aligned
    /// to; `HostEngine::frame` cannot serve, because it counts frames *rendered* and so
    /// jumps a whole quantum at a time however few of them the host has taken.
    frames_emitted: u64,
    /// The musical clock and the block size a live-input sender stamps against
    /// ([`EventClock`]). Two relaxed stores per callback, and the block size is the
    /// only way a host can know how far ahead a live event has to be stamped.
    event_clock: Arc<EventClock>,
}

impl RenderState {
    /// One callback.
    ///
    /// # Why it walks the block rather than rendering it whole
    ///
    /// The engine already makes *audio* independent of the host's block size — that is what
    /// its output ring is for (design goal 3). What is not automatically independent is
    /// everything the host itself decides between renders: when the control plane is
    /// drained, and when the end of the song arms a fade or a stop. Decide those once per
    /// callback and a device handing over 4096 frames arms the fade up to 85 ms later than
    /// one handing over 128 — and the two renders of the same module differ, byte for byte.
    ///
    /// So the walk is aligned to the frames **emitted**, not to the block: every decision
    /// lands on a multiple of [`RENDER_QUANTUM`] emitted frames, whatever length the device
    /// asked for, and the first chunk is however much is left of the quantum the last
    /// callback ended part-way through. That is exactly the cadence the browser worklet
    /// gets for free by being called with 128 frames every time.
    ///
    /// # Why the *publication* is per callback and not per quantum
    ///
    /// A device block can be very large — PulseAudio's WSLg sink asks for 96 000 frames,
    /// two whole seconds, in one call — and the temptation is to publish telemetry per
    /// quantum so a UI is not two seconds behind. It does not help, and it hurts. It does
    /// not help because the lag is the device's playout buffer, not the publication: the
    /// callback renders those two seconds in about a millisecond and then nothing happens
    /// for two seconds, so the freshest state there is is already "the end of the block".
    /// And it hurts because the snapshot ring keeps the **oldest** three when it fills
    /// (`starplayer_rt::snapshot`), so a callback that published a hundred snapshots would
    /// leave the reader holding the one from the *start* of the block. One publication per
    /// callback, carrying the newest state, is exactly right at any block size.
    fn render(&mut self, output: &mut [f32]) {
        let channels = self.engine.channels().max(1);
        // What the device actually asks for, which is not always what it was asked for: a
        // backend given no preference answers with its own block, and the live-input lead
        // has to cover whatever that turns out to be.
        self.event_clock.observe_block((output.len() / channels) as u32);
        let mut peak = 0.0f32;
        let mut written = 0usize;
        while written < output.len() {
            let offset_in_quantum = (self.frames_emitted % RENDER_QUANTUM as u64) as usize;
            if offset_in_quantum == 0 {
                self.drain_commands();
                self.arm_end_of_song();
            }
            let take = (RENDER_QUANTUM - offset_in_quantum).saturating_mul(channels).min(output.len().saturating_sub(written));
            let end = written.saturating_add(take);
            let Some(chunk) = output.get_mut(written..end) else { break };
            peak = peak.max(self.engine.render(chunk, &mut self.transport));
            self.frames_emitted = self.frames_emitted.saturating_add((take / channels.max(1)) as u64);
            if self.frames_emitted.is_multiple_of(RENDER_QUANTUM as u64) {
                self.settle_transport();
            }
            written = end;
        }
        self.retire_engine_garbage();
        let pending_engine_commands = self.control.queued_commands();
        self.taps.pending_engine_commands.store(pending_engine_commands as u32, Ordering::Relaxed);
        // Engine drains are bounded independently of Host drains. A popped Host barrier
        // is complete only once actual Engine and DSP requests have reached audio.
        if pending_engine_commands == 0 && self.engine.pending_insert_commands() == 0
            && self.pending_command.is_none() && self.commands.is_empty()
            && let Some((identity, serial, rejected)) = self.pending_barrier.take() {
            self.applied = AppliedPlayback { identity: self.interactive_identity, serial,
                accepted: self.installed_identity == Some(identity) && rejected == self.taps.commands_rejected.load(Ordering::Relaxed),
                capabilities: self.applied.capabilities };
        }
        self.publish(peak);
    }

    fn drain_commands(&mut self) {
        // Move the previous quantum's module retirement before reserving this one.
        self.retire_engine_garbage();
        let mut prepared_load = false;
        let prepared_host = self.interactive_identity.is_some();
        for _ in 0..HOST_COMMAND_CAPACITY {
            let Some(command) = self.pending_command.take().or_else(|| self.commands.pop()) else { break };
            let available = self.retired.capacity().saturating_sub(self.retired.len());
            let retirement_room = match &command {
                // Source + old table share one entry; the old module arrives after
                // Engine::render drains LoadModule. One prepared load per quantum
                // keeps its engine garbage reservation bounded and explicit.
                HostCommand::Load { .. } if prepared_host => 2,
                HostCommand::RestoreSelections { .. } => 1 + usize::from(prepared_load),
                _ => 0,
            };
            let another_load = prepared_load && matches!(&command, HostCommand::Load { .. });
            let engine_load_commands = match &command { HostCommand::Load { new_session: true, .. } if self.interactive_enabled => 2, HostCommand::Load { .. } => 1, _ => 0 };
            let blocked_engine_load = prepared_host && self.control.command_capacity().saturating_sub(self.control.queued_commands()) < engine_load_commands;
            if another_load || blocked_engine_load || available < retirement_room {
                self.pending_command = Some(command);
                break;
            }
            prepared_load |= prepared_host && matches!(&command, HostCommand::Load { .. });
            self.apply(command);
        }
    }

    fn apply(&mut self, command: HostCommand) {
        match command {
            HostCommand::AppliedBarrier { identity, serial, rejected } => {
                self.pending_barrier = Some((identity, serial, rejected));
            }
            HostCommand::Interactive(command) => self.apply_interactive(command),
            HostCommand::RestoreTransport { identity, transport, state, natural_end } => {
                if self.interactive_identity == Some(identity) && self.interactive_enabled {
                    let action = match state { InteractiveTransport::Playing => InteractiveAction::Play, InteractiveTransport::Paused => InteractiveAction::Pause, InteractiveTransport::Stopped => InteractiveAction::Stop };
                    if self.send_interactive(InteractiveCommand { identity, action }) {
                        self.transport = transport;
                        self.interactive_state = state;
                        self.awaiting_engine_stop = natural_end;
                        self.taps.natural_end.store(natural_end, Ordering::Relaxed);
                    }
                } else { self.reject(); }
            }
            HostCommand::RestoreSelections { identity, enabled, selections } => {
                if self.interactive_identity == Some(identity) { self.engine.restore_selections(identity, enabled, &selections); }
                else { self.reject(); }
                self.retire(Retired::Selections(selections));
            }
            HostCommand::Play => {
                // Play during a fade, or after the song ran out of order list, means
                // "again", not "louder": the song is over, so it goes back to the top,
                // which also clears the sticky `end_reached` the fade was armed from.
                if self.transport.take_restart() {
                    self.request_seek(SeekKind::Frame(0));
                }
                self.awaiting_engine_stop = false;
                self.transport.begin_play();
                let _ = self.send(Command::Play);
            }
            HostCommand::Stop => self.transport.begin_stop(false),
            HostCommand::Load { module, source, identity, selections, new_session } => {
                let retired_selections = selections.map(|selections| self.engine.replace_instrument_selections(selections));
                if let Some(identity) = identity {
                    self.engine.set_playback_identity(identity);
                    self.interactive_identity = Some(identity);
                }
                let retired_source = self.engine.replace_source(source);
                // Keep source and selection ownership in one retirement record, so
                // preparing actual module bounds does not increase queue pressure.
                if retired_source.is_some() || retired_selections.is_some() {
                    self.retire(Retired::Installation { source: retired_source, selections: retired_selections });
                }
                if self.send(Command::LoadModule(module)) { self.installed_identity = identity; }
                if new_session && self.interactive_enabled && let Some(identity) = identity {
                    // A fresh source never inherits a playing clock, paused fade or end
                    // marker. The paired engine Stop fits the reservation above.
                    self.apply_interactive(InteractiveCommand { identity, action: InteractiveAction::Stop });
                    self.transport.reset_interactive_session();
                }
            }
            HostCommand::SetAtEnd { at_end, fade_frames } => {
                self.transport.set_fade_frames(fade_frames);
                self.handles.at_end.set(at_end);
                self.transport.cancel_fade(at_end);
            }
            HostCommand::Engine(command) => { let _ = self.send(command); }
        }
    }

    fn send_interactive(&mut self, command: InteractiveCommand) -> bool {
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        { self.send(Command::Interactive(command)) }
        #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
        { let _ = command; self.reject(); false }
    }

    fn apply_interactive(&mut self, command: InteractiveCommand) {
        if self.interactive_identity != Some(command.identity) { self.reject(); return; }
        if !self.interactive_enabled && matches!(command.action, InteractiveAction::Play | InteractiveAction::Pause | InteractiveAction::Resume | InteractiveAction::Stop | InteractiveAction::AllSoundOff | InteractiveAction::StopPreview) { self.reject(); return; }
        // Queue first: a full engine ring must not partially apply a host fade change.
        if !self.send_interactive(command) { return; }
        match command.action {
            InteractiveAction::Pause => {
                self.interactive_state = InteractiveTransport::Paused;
                self.transport.pause(true);
            }
            InteractiveAction::Play | InteractiveAction::Resume => {
                if self.transport.take_armed_rewind() {
                    self.request_seek(SeekKind::Frame(0));
                    self.awaiting_restart = true;
                }
                self.interactive_state = InteractiveTransport::Playing;
                self.transport.pause(false);
                self.awaiting_engine_stop = false;
                self.taps.natural_end.store(false, Ordering::Relaxed);
            }
            InteractiveAction::Stop => {
                self.interactive_state = InteractiveTransport::Stopped;
                self.transport.interactive_stop(false);
                self.awaiting_restart = false;
                self.awaiting_engine_stop = false;
                self.taps.natural_end.store(false, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    fn finish_interactive_song(&mut self) {
        let Some(identity) = self.interactive_identity else { return };
        if self.send_interactive(InteractiveCommand { identity, action: InteractiveAction::Stop }) {
            self.interactive_state = InteractiveTransport::Stopped;
            self.transport.interactive_stop(true);
            self.awaiting_engine_stop = true;
            self.taps.natural_end.store(true, Ordering::Relaxed);
        }
    }

    /// The song has been heard through once. What that means depends on *how* it ends
    /// (task D2), and only two cases do anything at all:
    ///
    /// * it **loops** — something in the module jumps back — and the caller asked for a
    ///   fade, so the second pass plays under a fading transport;
    /// * it **ends** — the order list ran out, or a stop marker fired — and the caller
    ///   asked for anything but Repeat. There is no second pass to fade into, so the
    ///   transport stops the way Stop stops it and rewinds for the next Play.
    ///
    /// Only while the transport is actually running: once the stop has landed nothing
    /// publishes again until the next Play consumes the rewind, so the snapshot keeps
    /// saying `end_reached` and would otherwise re-arm over silence.
    fn arm_end_of_song(&mut self) {
        self.last_snapshot = *self.telemetry.read();
        if self.awaiting_restart {
            if self.last_snapshot.transport.end_reached { return; }
            self.awaiting_restart = false;
        }
        if self.interactive_enabled && self.interactive_state != InteractiveTransport::Playing { return; }
        let running = self.engine.is_playing() && !self.transport.stop_is_pending() && !self.awaiting_engine_stop;
        if !running || !self.last_snapshot.transport.end_reached || self.transport.is_fading() {
            return;
        }
        let loops = self.last_snapshot.transport.song_end == SongEnd::Loops;
        match self.handles.at_end.get() {
            AtEnd::FadeOut if loops => self.transport.begin_fade(),
            AtEnd::FadeOut | AtEnd::Stop if !loops && self.interactive_enabled => self.finish_interactive_song(),
            AtEnd::FadeOut | AtEnd::Stop if !loops => self.transport.begin_stop(true),
            _ => {}
        }
    }

    /// A fade that has run its course, and a ramp that has reached zero, both end in the
    /// same typed engine stop. A song that faded out or ended is over, not paused, so it
    /// rewinds for the next Play; a Stop the caller asked for keeps its place.
    fn settle_transport(&mut self) {
        if self.interactive_enabled {
            if self.interactive_state == InteractiveTransport::Playing && self.transport.fade_has_landed() { self.finish_interactive_song(); }
            return;
        }
        if self.transport.fade_has_landed() && self.send(Command::Stop) {
            self.awaiting_engine_stop = true;
            self.transport.take_fade();
            self.request_seek(SeekKind::Frame(0));
        }
        if self.transport.stop_has_landed() && self.send(Command::Stop) {
            self.awaiting_engine_stop = true;
            if self.transport.take_stop() {
                self.request_seek(SeekKind::Frame(0));
            }
        }
    }

    fn request_seek(&mut self, kind: SeekKind) {
        self.handles.seek.request(SeekRequest { kind, frame: self.engine.source_frame() });
    }

    /// Queue one typed engine command. A rejected `LoadModule` carries an `Arc<Module>`,
    /// and dropping that here could be the last handle — so it goes down the retirement
    /// ring like any other, rather than calling `free()` in the callback.
    fn send(&mut self, command: Command<RtArc<Module>>) -> bool {
        match self.control.send(command) {
            Ok(()) => true,
            Err(rejected) => {
                self.reject();
                if let Command::LoadModule(module) = rejected {
                    self.retire(Retired::Module(module));
                }
                false
            }
        }
    }

    fn reject(&self) { self.taps.commands_rejected.fetch_add(1, Ordering::Relaxed); }

    /// Move every retired module handle out of the engine's garbage channel and on towards
    /// the control thread. It is **moved**, never dropped: a `free()` here is the stall the
    /// garbage channel exists to prevent.
    fn retire_engine_garbage(&mut self) {
        while !self.retired.is_full() {
            let Some(module) = self.control.collect_garbage() else { break };
            self.retire(Retired::Module(module));
        }
    }

    /// A retirement the control side has stopped collecting has nowhere left to go. Dropping
    /// it here is the last resort, and the engine's own `retired_module_dropped` warning
    /// says the same thing about the same situation.
    fn retire(&mut self, retired: Retired) {
        if let Err(orphan) = self.retired.push(retired) {
            drop(orphan);
        }
    }

    fn publish(&mut self, peak: f32) {
        self.taps.source_frame.store(self.engine.source_frame().0, Ordering::Relaxed);
        // Source time is monotonic in interactive mode; legacy transport can freeze it.
        // Live events always use the clock sources actually dispatch against.
        self.event_clock.publish(self.engine.source_frame());
        self.taps.output_frame.store(self.engine.frame().0, Ordering::Relaxed);
        self.taps.playing.store(self.engine.is_playing() && !self.transport.stop_is_pending(), Ordering::Relaxed);
        self.taps.transport_state.store(match self.interactive_state { InteractiveTransport::Playing => 1, InteractiveTransport::Paused => 2, InteractiveTransport::Stopped => 0 }, Ordering::Relaxed);
        self.taps.fading.store(self.transport.is_fading(), Ordering::Relaxed);
        self.taps.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        self.taps.blocks_rendered.fetch_add(1, Ordering::Relaxed);
        // The **freshest** snapshot, not the one this callback's first quantum armed its
        // end-of-song decision from: the decision has to be taken on a fixed cadence so that
        // it lands on the same frame at every block size, but the reading a caller gets
        // should be as new as the engine can make it.
        //
        // A snapshot the caller is too slow to collect is dropped rather than queued, which
        // is the same bargain the engine's own telemetry channel strikes.
        let mut snapshot = *self.telemetry.read();
        if self.interactive_enabled {
            snapshot.transport.interactive = Some(self.interactive_state);
            snapshot.transport.natural_end = self.taps.natural_end.load(Ordering::Relaxed);
        }
        let _ = self.forwarded.publish(PlayerSnapshot { applied: self.applied, engine: snapshot, source_frame: self.engine.source_frame(), transport: self.transport, state: self.interactive_state, natural_end: self.taps.natural_end.load(Ordering::Relaxed) });
    }
}

/// The control-side halves that [`RenderState`] does not take.
struct ControlSide {
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<PlayerSnapshot>,
    taps: Arc<PlayerTaps>,
    handles: SourceHandles,
    /// Taken from the engine here, because [`Player::open`] is the last moment a `&mut` to
    /// it exists on this thread.
    scopes: Option<Box<[TapReader]>>,
    event_clock: Arc<EventClock>,
    /// The insert graph's control handle (M7-H7). Unlike [`EngineHandle`] this is held by
    /// `Player` itself rather than by [`RenderState`]: installing an effect is a plain
    /// control-thread action with no transport ramp to sequence around, so there is no
    /// reason to route it through [`HostCommand`] and the render callback the way a
    /// `Command::LoadModule` has to be.
    insert_control: HostInsertControl,
}

/// Build both sides of one engine at `spec`'s rate, in `mode`.
///
/// The engine is stopped and the transport silent, so a stream may be started immediately
/// without the song advancing under it: nothing sounds until [`Player::play`].
fn build_sides(mode: MixerMode, spec: AudioSpec, mut settings: EngineSettings) -> Result<(RenderState, ControlSide), HostError> {
    if mode.channels as u16 != spec.channels {
        let reason = std::format!("the engine renders {} channels, the device wants {}", mode.channels, spec.channels);
        return Err(HostError::UnsupportedSpec { requested: spec, reason });
    }
    settings.sample_rate_hz = spec.sample_rate_hz;
    let (mut engine, mut control, insert_control, telemetry) = HostEngine::build_with_settings(mode, settings)?;
    let scopes = engine.scope_readers();
    // Freeze the musical clock until the first Play. Without this a stream that is started
    // as soon as it is open plays the song silently while the caller is still deciding.
    let interactive_identity = settings.playback_configuration().map(|configuration| configuration.identity);
    let interactive_enabled = settings.playback_configuration().is_some_and(|configuration| configuration.capacity.interactive_transport);
    if !interactive_enabled { control.send(Command::Stop).map_err(|_| HostError::ControlQueueFull)?; }

    let (command_producer, command_consumer) = channel(HOST_COMMAND_CAPACITY);
    let (retired_producer, retired_consumer) = channel(RETIRED_CAPACITY);
    let (forwarded, snapshot_reader) = snapshot_channel(TELEMETRY_DEPTH, PlayerSnapshot::default());
    let taps = Arc::new(PlayerTaps::default());
    let event_clock = EventClock::new(spec.sample_rate_hz);
    // Made once, for the life of the stream, and shared. A pair made per module would leave
    // the audio thread holding the last handle to the outgoing one on every load.
    let handles = SourceHandles::new(AtEnd::Continue);

    let render = RenderState {
        engine,
        control,
        telemetry,
        forwarded,
        commands: command_consumer,
        applied: AppliedPlayback { capabilities: settings.playback_configuration().map(|configuration| configuration.capabilities).unwrap_or_default(), ..AppliedPlayback::default() },
        pending_barrier: None,
        pending_command: None,
        retired: retired_producer,
        handles: handles.clone(),
        transport: if interactive_enabled { Transport::playing() } else { Transport::stopped() },
        interactive_identity,
        installed_identity: None,
        interactive_enabled,
        interactive_state: InteractiveTransport::Stopped,
        taps: Arc::clone(&taps),
        last_snapshot: Snapshot::default(),
        awaiting_engine_stop: false,
        awaiting_restart: false,
        frames_emitted: 0,
        event_clock: Arc::clone(&event_clock),
    };
    let control_side = ControlSide {
        commands: command_producer,
        retired: retired_consumer,
        telemetry: snapshot_reader,
        taps,
        handles,
        scopes,
        event_clock,
        insert_control,
    };
    Ok((render, control_side))
}

/// What a rebuild needs beyond the device it opens on.
///
/// A struct rather than four more parameters because the two callers differ in exactly these
/// four things and in nothing else, and a reader of either wants them named.
struct Rebuild {
    /// The mixer mode to instantiate the engine in.
    mode: MixerMode,
    /// Where the module should pick up in the rebuilt sequencer.
    start: SeekKind,
    /// A scan to reuse rather than measure again — only when nothing the timeline depends
    /// on has changed, which means the output rate and the module's dialect.
    cached_scan: Option<Arc<ScannedSong>>,
    /// Whether the transport should be running when the rebuild is done.
    resume: bool,
}

/// The backend-neutral player: one engine, one stream, one module at a time.
pub struct Player {
    engine_settings: EngineSettings,
    playback_identity: Option<PlaybackIdentity>,
    interactive_enabled: bool,
    overlap_shadow: bool,
    instrument_shadow: Vec<bool>,
    live_destination: starplayer::core::LiveDestination,
    transport_shadow: InteractiveTransport,
    rebuild_cursor: Option<starplayer::core::SequencerCursor>,
    stream: Stream,
    spec: AudioSpec,
    mode: MixerMode,
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<PlayerSnapshot>,
    taps: Arc<PlayerTaps>,
    handles: SourceHandles,
    at_end: AtEnd,
    /// Ticks added to every speed the module asks for, for a module written against a
    /// game's own replayer. Applied when a module is installed, so it is both what the
    /// song is scanned at and what it plays at.
    speed_adjust: i8,
    fade_frames: u32,
    master_volume: U0F16,
    module: Option<RtArc<Module>>,
    scan: Option<Arc<ScannedSong>>,
    smf: Option<Arc<starplayer::midi::smf::Smf>>,
    collected: usize,
    scopes: Option<Box<[TapReader]>>,
    /// The clock, the lead and the counters live input is stamped and reported against.
    /// Made once per stream, shared with the render callback and with every
    /// [`EventSender`] handed out.
    event_clock: Arc<EventClock>,
    /// The sending half of the live-input queue, once [`Player::midi_only`] or
    /// [`Player::jam`] has installed one and until [`Player::take_event_sender`] moves it
    /// to another thread.
    events: Option<EventSender>,
    #[cfg(feature = "interactive-audition")] live_epoch: Option<RtArc<starplayer::rt::atomic::AtomicU32>>,
    /// Whether live input is alone or sharing a mux with the module. Not the same question
    /// as `events.is_some()`, which goes false the moment a caller takes the sender.
    live_input: LiveInputMode,
    /// The insert graph's control handle (M7-H7): whichever mix path this engine's arm
    /// resolved to.
    insert_control: HostInsertControl,
    /// What this host believes is installed, per target and slot — replayed into a
    /// freshly built engine across [`Player::set_mixer_mode`] and
    /// [`Player::reopen`] (research point 1: the layout lives in `Player`, not the engine).
    insert_layout: InsertLayout,
}

impl Player {
    /// Negotiate a stream on `backend`, build the engine at the rate the device agreed to,
    /// and open it.
    ///
    /// The negotiation happens **first**, and separately, because its answer decides two
    /// things that cannot be corrected afterwards without rebuilding everything: the rate
    /// the engine's voice pool and control clock are built for, and the rate every module
    /// is scanned at (architecture §4.1).
    ///
    /// The stream comes back running but silent — the musical clock is stopped and the
    /// transport is at zero — so telemetry flows and [`Player::play`] is a 64-frame glide
    /// rather than a device round trip.
    pub fn open(backend: &mut dyn AudioBackend, device: Option<&str>, requested: AudioSpec, mode: MixerMode) -> Result<Player, HostError> {
        let settings = EngineSettings { voice_capacity: starplayer::MAX_VOICE_CAPACITY, channel_count: starplayer::engine::ChannelTable::MAX_CHANNELS, ..EngineSettings::default() };
        Self::open_with_settings(backend, device, requested, mode, settings)
    }

    /// Open with explicit prepared capacities. Rebuilds and every module-source builder
    /// retain these settings; the negotiated output rate replaces settings.sample_rate_hz.
    pub fn open_with_settings(backend: &mut dyn AudioBackend, device: Option<&str>, requested: AudioSpec, mode: MixerMode, mut settings: EngineSettings) -> Result<Player, HostError> {
        crate::engine::validate_host_settings(settings).map_err(HostError::EngineConfiguration)?;
        let negotiated = backend.negotiate(device, requested)?;
        settings.sample_rate_hz = negotiated.sample_rate_hz;
        let (mut render, control) = build_sides(mode, negotiated, settings)?;
        let stream = backend.open(device, negotiated, Box::new(move |output: &mut [f32]| render.render(output)))?;
        if stream.spec() != negotiated {
            return Err(HostError::UnsupportedSpec {
                requested: negotiated,
                reason: std::format!("the backend opened {} instead", stream.spec()),
            });
        }
        stream.play()?;
        Ok(Player {
            engine_settings: settings,
            playback_identity: settings.playback_configuration().map(|configuration| configuration.identity),
            interactive_enabled: settings.playback_configuration().is_some_and(|configuration| configuration.capacity.interactive_transport),
            overlap_shadow: false,
            live_destination: starplayer::core::LiveDestination::Off,
            instrument_shadow: vec![false; settings.playback_configuration().filter(|configuration| configuration.capabilities.contains(starplayer::core::PlaybackCapabilities::NOTE_OVERLAP)).map_or(0, |configuration| configuration.instrument_count)],
            transport_shadow: InteractiveTransport::Stopped,
            rebuild_cursor: None,
            smf: None,
            stream,
            spec: negotiated,
            mode,
            commands: control.commands,
            retired: control.retired,
            telemetry: control.telemetry,
            taps: control.taps,
            handles: control.handles,
            at_end: AtEnd::Continue,
            speed_adjust: 0,
            fade_frames: 0,
            master_volume: U0F16::MAX,
            module: None,
            scan: None,
            collected: 0,
            scopes: control.scopes,
            event_clock: control.event_clock,
            events: None,
            #[cfg(feature = "interactive-audition")] live_epoch: None,
            live_input: LiveInputMode::Off,
            insert_control: control.insert_control,
            insert_layout: InsertLayout::default(),
        })
    }

    /// Close this stream and open another, rebuilding the engine at the new rate.
    ///
    /// The loaded module is **re-scanned**, never carried over: a song timeline is measured
    /// in frames at one output rate, and installing a 44.1 kHz timeline in a 48 kHz
    /// sequencer leaves the progress slider, the loop point and the audio disagreeing about
    /// where the song is.
    ///
    /// Master volume and the repeat setting are restored; per-channel mutes are not, because
    /// the caller owns those and can reapply them without this having to shadow them.
    pub fn reopen(&mut self, backend: &mut dyn AudioBackend, device: Option<&str>, requested: AudioSpec) -> Result<(), HostError> {
        let plan = Rebuild { mode: self.mode, start: SeekKind::None, cached_scan: None, resume: false };
        self.rebuild(backend, device, requested, plan)
    }

    /// Rebuild the engine in another [`MixerMode`], on the same device, from the position
    /// that is sounding now.
    ///
    /// The mixer mode is a set of **type parameters** (architecture §7.1), so changing it is
    /// a re-instantiation and not a field write — which means a new engine, a new voice pool
    /// and a new source. What survives is everything the ear would notice losing: the module
    /// (the same `Arc`, never a reload, so nothing is retired), the scan (a timeline is a
    /// function of the rate and the module's dialect and of nothing the mixer chooses, so it
    /// is *reused* rather than measured again), the song frame that was sounding, the master
    /// volume, the repeat setting and whether the transport was running.
    ///
    /// Per-channel mutes are the caller's, exactly as they are across [`Player::reopen`]: a
    /// caller that shadows them reapplies them, and one that does not is not silently
    /// overruled.
    pub fn set_mixer_mode(&mut self, backend: &mut dyn AudioBackend, device: Option<&str>, mode: MixerMode) -> Result<(), HostError> {
        if mode == self.mode {
            return Ok(());
        }
        let playing = self.is_playing();
        let snapshot = self.telemetry.read().engine;
        // The sounding *song frame*, not just the order: a rebuild in the middle of a bar
        // should come back where the ear left it, and the scan can say where that is.
        let start = match self.scan {
            Some(_) => SeekKind::Frame(snapshot.transport.song_frame),
            None => SeekKind::Order(snapshot.transport.order),
        };
        let plan = Rebuild { mode, start, cached_scan: self.scan.clone(), resume: playing };
        self.rebuild(backend, device, self.spec, plan)
    }

    /// Open a second player on the same backend and become it.
    ///
    /// One code path for both rebuilds, because both are the same sentence: everything the
    /// caller has told this player is said again to a new one, the module is reinstalled at
    /// `start`, and the old stream — with the engine and the module handles inside its
    /// callback — dies here, on this thread.
    fn rebuild(&mut self, backend: &mut dyn AudioBackend, device: Option<&str>, requested: AudioSpec, plan: Rebuild) -> Result<(), HostError> {
        let module = self.module.clone();
        let saved_host = *self.telemetry.read();
        let interactive_state = if saved_host.engine.is_idle() { self.transport_shadow } else { saved_host.state };
        let held_song_frame = saved_host.engine.transport.song_frame;
        let mut rebuild_cursor = saved_host.engine.transport.sequencer_cursor();
        if interactive_state == InteractiveTransport::Playing && let Some(cursor) = rebuild_cursor.as_mut() {
            // Never extrapolate through a tick absent from this published record.
            let elapsed = cursor.captured_frame.frames_until(saved_host.source_frame).min(cursor.remaining_frames);
            cursor.remaining_frames = cursor.remaining_frames.saturating_sub(elapsed);
            cursor.song_frame = cursor.song_frame.saturating_add(elapsed);
        }
        let mut rebuild_smf = saved_host.engine.transport.smf_state();
        if interactive_state == InteractiveTransport::Playing && let Some(state) = rebuild_smf.as_mut() {
            let elapsed = state.cursor.captured_frame.frames_until(saved_host.source_frame);
            let elapsed = state.cursor.remaining_frames.map_or(elapsed, |remaining| elapsed.min(remaining));
            state.cursor.remaining_frames = state.cursor.remaining_frames.map(|remaining| remaining.saturating_sub(elapsed));
            state.cursor.song_frame = state.cursor.song_frame.saturating_add(elapsed);
        }
        let at_end = self.at_end;
        let fade_frames = self.fade_frames;
        let master_volume = self.master_volume;
        let collected = self.collected;
        let requested_lead = self.event_clock.requested_lead_frames();
        let live_input = self.live_input;
        // What this host believes is installed survives the rebuild (research point 1: the
        // layout lives in `Player`), replayed into the freshly built engine below — a new
        // engine's insert graph starts empty, exactly as a new engine's module does.
        let insert_layout = self.insert_layout.clone();
        let mut settings = self.engine_settings;
        let rebuild_identity = self.next_identity(false);
        if let Some(identity) = rebuild_identity { settings.set_playback_identity(identity); }
        let mut rebuilt = Player::open_with_settings(backend, device, requested, plan.mode, settings)?;
        // Carried, not restarted: the count is what a caller reports as "modules dropped on
        // this thread since start-up", and a rebuild is not a fresh start-up.
        rebuilt.collected = collected;
        rebuilt.playback_identity = rebuild_identity;
        rebuilt.speed_adjust = self.speed_adjust;
        rebuilt.rebuild_cursor = rebuild_cursor;
        let rebuild_start = if self.interactive_enabled {
            let converted = (held_song_frame as u128 * rebuilt.spec.sample_rate_hz as u128 / self.spec.sample_rate_hz.max(1) as u128).min(u64::MAX as u128) as u64;
            SeekKind::Frame(converted)
        } else { plan.start };
        rebuilt.set_master_volume(master_volume)?;
        if fade_frames > 0 {
            rebuilt.set_fade_frames(fade_frames)?;
        }
        rebuilt.set_at_end(at_end)?;
        rebuilt.set_event_lead(requested_lead);
        // Jam mode is part of module installation: establish it first so the rebuilt
        // player constructs one mux at `plan.start`, rather than installing a sequencer
        // and immediately replacing it with another one built from a not-yet-published
        // frame-zero snapshot.
        if live_input == LiveInputMode::Jam {
            rebuilt.live_input = LiveInputMode::Jam;
        }
        if let Some(module) = module {
            if let Some(smf) = self.smf.as_ref().filter(|_| self.interactive_enabled) {
                rebuilt.install_smf(Arc::clone(smf), module, rebuild_smf, false)?;
            } else { rebuilt.install(module, rebuild_start, plan.cached_scan, false)?; }
        }
        // A rebuilt engine gets a **new** live-input queue, because the old one's consumer
        // went into the retired engine. An [`EventSender`] a caller had already taken is
        // therefore dead after a rebuild and has to be taken again — which is the same
        // contract [`Player::take_scope_readers`] already has.
        match live_input {
            LiveInputMode::Off => {}
            LiveInputMode::MidiOnly => rebuilt.midi_only()?,
            LiveInputMode::Jam => {}
        }
        // Replay every insert this host believes is installed into the freshly built
        // engine: install, then every parameter this host has explicitly set, then the
        // bypass bit. `rebuilt.insert_layout` is left at its own fresh default by
        // `Player::open` and each of these calls updates it exactly as a caller's own
        // would, so the copy of `insert_layout` captured above is only needed to iterate —
        // it is not written back.
        for (target, slot, insert) in insert_layout.iter() {
            rebuilt.install_insert(target, slot, insert.kind)?;
            for &(param, value) in insert.params() {
                rebuilt.set_insert_param(target, slot, param, value)?;
            }
            if insert.bypassed {
                rebuilt.bypass_insert(target, slot, true)?;
            }
        }
        // One atomic replay, even when more than a command ring's worth are selected.
        if let Some(identity) = rebuilt.playback_identity {
            rebuilt.queue(HostCommand::RestoreSelections { identity, enabled: self.overlap_shadow, selections: self.instrument_shadow.clone().into_boxed_slice() })?;
            rebuilt.instrument_shadow.clone_from(&self.instrument_shadow);
            rebuilt.overlap_shadow = self.overlap_shadow;
        }
        if self.interactive_enabled {
            rebuilt.interactive(InteractiveAction::SetLiveDestination(self.live_destination))?;
            if let Some(identity) = rebuilt.playback_identity {
                rebuilt.queue(HostCommand::RestoreTransport { identity, transport: if saved_host.engine.is_idle() { Transport::playing() } else { saved_host.transport.converted(self.spec.sample_rate_hz, rebuilt.spec.sample_rate_hz) }, state: interactive_state, natural_end: saved_host.natural_end })?;
                rebuilt.transport_shadow = interactive_state;
            }
        } else if plan.resume { rebuilt.play()?; }

        *self = rebuilt;
        Ok(())
    }

    /// What the device actually agreed to.
    pub fn spec(&self) -> AudioSpec { self.spec }

    /// The mixer mode the engine was built in.
    pub fn mixer_mode(&self) -> MixerMode { self.mode }

    /// What the backend's error callback has reported.
    pub fn health(&self) -> &Arc<StreamHealth> { self.stream.health() }

    /// Decode, scan and activate `bytes`.
    ///
    /// Everything expensive happens here, on this thread: the decode, the scan, the
    /// sequencer's per-channel state. Only a `Box` and two `Arc`s cross to the audio side.
    /// A failed load leaves the previous module playing.
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), HostError> {
        let module = RtArc::new(starplayer::load(bytes)?);
        self.install(module, SeekKind::None, None, true)
    }

    /// [`Player::load`] for a module the caller has already decoded.
    pub fn load_module(&mut self, module: RtArc<Module>) -> Result<(), HostError> {
        self.install(module, SeekKind::None, None, true)
    }

    /// Play a Standard MIDI File through `instruments` without a tracker sequencer.
    ///
    /// Parsing, frame conversion and rack construction all happen on this control thread.
    /// The returned frame count is the file's end-of-track duration, including trailing
    /// silence after its last event, so a host can end playback even though the MIDI
    /// source's synthesised control clock intentionally runs forever.
    pub fn load_smf(&mut self, bytes: &[u8], instruments: RtArc<Module>) -> Result<u64, HostError> {
        let smf = starplayer::midi::smf::parse_smf(bytes).map_err(HostError::StandardMidiFile)?;
        self.install_smf(Arc::new(smf), instruments, None, true)
    }

    fn install_smf(&mut self, smf: Arc<starplayer::midi::smf::Smf>, instruments: RtArc<Module>, restore: Option<starplayer::core::SmfPlaybackState>, new_session: bool) -> Result<u64, HostError> {
        let frame = Frame(self.taps.source_frame.load(Ordering::Relaxed));
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        let prepared = self.engine_settings.playback_configuration().is_some_and(|configuration| !configuration.capabilities.is_empty());
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        let sequencer = {
            #[cfg(feature = "interactive-audition")]
            if self.interactive_enabled { starplayer::midi::SmfSequencer::interactive_at(&smf, self.spec.sample_rate_hz, frame, restore.map(|state| state.cursor)) }
            else if prepared { starplayer::midi::SmfSequencer::prepared_song_at(&smf, self.spec.sample_rate_hz, frame) }
            else { starplayer::midi::SmfSequencer::new_at(&smf, self.spec.sample_rate_hz, frame) }
            #[cfg(not(feature = "interactive-audition"))]
            if prepared { starplayer::midi::SmfSequencer::prepared_song_at(&smf, self.spec.sample_rate_hz, frame) }
            else { starplayer::midi::SmfSequencer::new_at(&smf, self.spec.sample_rate_hz, frame) }
        };
        #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
        let sequencer = starplayer::midi::SmfSequencer::new_at(&smf, self.spec.sample_rate_hz, frame);
        let length_frames = sequencer.length_frames();
        #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
        let rack = InstrumentRack::for_module(&instruments, self.spec.sample_rate_hz);
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        let rack = {
            #[allow(unused_mut)]
            let mut rack = if prepared { starplayer::prepare_instrument_rack(&instruments, self.spec.sample_rate_hz, self.engine_settings.voice_capacity, starplayer::engine::instrument::live::RackScope::Song)? } else { InstrumentRack::for_module(&instruments, self.spec.sample_rate_hz) };
            #[cfg(feature = "interactive-audition")]
            if let Some(state) = restore { rack.restore_controllers(state.channels); }
            rack
        };
        #[cfg(not(feature = "interactive-audition"))]
        let _ = restore;
        let mut source = MidiSource::new(sequencer, rack, self.spec.sample_rate_hz);
        source.start_control_at(frame);
        let source: Box<dyn starplayer::engine::EventSource> = Box::new(source);
        let (source, events) = if self.interactive_enabled {
            let (live, producer) = self.build_live_source(&instruments, frame)?;
            (crate::source::interactive_sources(source, live), Some(EventSender::interactive(producer, Arc::clone(&self.event_clock))))
        } else { (source, None) };
        let identity = self.next_identity(new_session);
        let selections = self.prepare_instrument_selections(&instruments, new_session)?;
        let instrument_shadow = selections.as_ref().map(|selections| selections.to_vec());
        self.commands.push(HostCommand::Load { module: RtArc::clone(&instruments), source, identity, selections, new_session }).map_err(|_| HostError::ControlQueueFull)?;
        if let Some(selections) = instrument_shadow { self.instrument_shadow = selections; }
        self.engine_settings.set_instrument_count(instruments.instruments().len());
        self.handles.seek.clear();
        self.playback_identity = identity;
        self.module = Some(instruments);
        self.scan = None;
        self.smf = self.interactive_enabled.then_some(smf);
        #[cfg(feature = "interactive-audition")]
        { self.live_epoch = events.as_ref().and_then(EventSender::cancellation_token); }
        self.events = events;
        self.live_input = LiveInputMode::Off;
        if new_session { self.transport_shadow = InteractiveTransport::Stopped; self.overlap_shadow = false; self.instrument_shadow.fill(false); self.live_destination = starplayer::core::LiveDestination::Off; if self.interactive_enabled { self.live_input = LiveInputMode::Off; } }
        Ok(length_frames)
    }

    /// Allocate to actual module bounds, including persistent hosts opened with count 0.
    /// Same-session replacements retain selections; explicit user loads start false.
    fn prepare_instrument_selections(&self, module: &Module, new_session: bool) -> Result<Option<Box<[bool]>>, HostError> {
        if !self.engine_settings.playback_configuration().is_some_and(|configuration| configuration.capabilities.contains(starplayer::core::PlaybackCapabilities::NOTE_OVERLAP)) { return Ok(None); }
        let mut selections = Vec::new();
        selections.try_reserve_exact(module.instruments().len()).map_err(|_| HostError::Backend(String::from("instrument selection allocation failed")))?;
        selections.resize(module.instruments().len(), false);
        if !new_session {
            for (value, old) in selections.iter_mut().zip(self.instrument_shadow.iter()) { *value = *old; }
        }
        Ok(Some(selections.into_boxed_slice()))
    }

    fn install(&mut self, module: RtArc<Module>, start: SeekKind, cached_scan: Option<Arc<ScannedSong>>, new_session: bool) -> Result<(), HostError> {
        self.install_identified(module, start, cached_scan, new_session, self.next_identity(new_session))
    }

    fn install_identified(&mut self, module: RtArc<Module>, start: SeekKind, cached_scan: Option<Arc<ScannedSong>>, new_session: bool, identity: Option<PlaybackIdentity>) -> Result<(), HostError> {
        // A new sequencer's tick clock starts at frame zero, but the engine's musical clock
        // is monotonic and has been running since the stream opened. Starting the clock
        // where the engine actually is stops the engine burning ticks catching up.
        let frame = Frame(self.taps.source_frame.load(Ordering::Relaxed));
        // A seek asked for against the outgoing module means nothing to the incoming one.
        self.handles.seek.clear();
        let built = crate::source::build_source_with_cursor(RtArc::clone(&module), crate::source::SourcePosition { seek: start, cursor: self.rebuild_cursor.take() }, frame, &self.handles, cached_scan, self.speed_adjust, self.engine_settings)?;
        let (source, events) = if self.interactive_enabled {
            {
                let (live, producer) = self.build_live_source(&module, frame)?;
                (crate::source::interactive_sources(built.source, live), Some(EventSender::interactive(producer, Arc::clone(&self.event_clock))))
            }
        } else if self.live_input == LiveInputMode::Jam {
            let (source, producer) = self.build_jam_source(&module, frame, built.source)?;
            (source, Some(EventSender::new(producer, Arc::clone(&self.event_clock))))
        } else {
            (built.source, None)
        };
        let selections = self.prepare_instrument_selections(&module, new_session)?;
        let instrument_shadow = selections.as_ref().map(|selections| selections.to_vec());
        let command = HostCommand::Load { module: RtArc::clone(&module), source, identity, selections, new_session };
        self.commands.push(command).map_err(|_| HostError::ControlQueueFull)?;
        if let Some(selections) = instrument_shadow { self.instrument_shadow = selections; }
        self.engine_settings.set_instrument_count(module.instruments().len());
        self.playback_identity = identity;
        if new_session { self.transport_shadow = InteractiveTransport::Stopped; self.overlap_shadow = false; self.instrument_shadow.fill(false); self.live_destination = starplayer::core::LiveDestination::Off; if self.interactive_enabled { self.live_input = LiveInputMode::Off; } }
        self.scan = Some(built.scanned);
        self.smf = None;
        self.module = Some(module);
        #[cfg(feature = "interactive-audition")]
        { self.live_epoch = events.as_ref().and_then(EventSender::cancellation_token); }
        self.events = events;
        if self.live_input != LiveInputMode::Jam {
            self.live_input = LiveInputMode::Off;
        }
        Ok(())
    }

    fn build_live_source(&self, module: &RtArc<Module>, frame: Frame) -> Result<(Box<dyn EventSource>, ExternalEventProducer), HostError> {
        #[cfg(feature = "interactive-audition")]
        {
            let rack = starplayer::prepare_live_rack(module, self.spec.sample_rate_hz, self.engine_settings.voice_capacity)?;
            let (producer, queue) = starplayer::engine::instrument::recoverable_event_channel(EVENT_QUEUE_CAPACITY);
            let mut source = MidiSource::new(queue, rack, self.spec.sample_rate_hz);
            source.start_control_at(frame);
            Ok((Box::new(source), producer))
        }
        #[cfg(not(feature = "interactive-audition"))]
        { let _ = (module, frame); Err(HostError::Backend(String::from("interactive input was not compiled"))) }
    }

    /// Put a seekable module source in slot 0 and live input in slot 1.
    ///
    /// Everything here allocates on the control thread. The fixed two-slot mux cannot be
    /// full while these two inserts are made; the error arms keep that invariant explicit
    /// without an `unwrap` becoming part of the host surface.
    fn build_jam_source(
        &self,
        module: &RtArc<Module>,
        frame: Frame,
        module_source: Box<dyn EventSource>,
    ) -> Result<(Box<dyn EventSource>, ExternalEventProducer), HostError> {
        let rack = InstrumentRack::for_module(module, self.spec.sample_rate_hz);
        let (producer, queue) = external_event_channel(EVENT_QUEUE_CAPACITY);
        let mut midi_source = MidiSource::new(queue, rack, self.spec.sample_rate_hz);
        midi_source.start_control_at(frame);
        let mut mux = SourceMux::new(2);
        mux.insert(module_source)
            .map_err(|_| HostError::Backend(String::from("the jam source mux refused slot 0")))?;
        mux.insert(Box::new(midi_source))
            .map_err(|_| HostError::Backend(String::from("the jam source mux refused slot 1")))?;
        Ok((Box::new(mux), producer))
    }

    /// The module currently loaded.
    pub fn module(&self) -> Option<&RtArc<Module>> { self.module.as_ref() }

    /// The scan the playing sequencer was built from: the timeline, and the quirks it was
    /// measured under.
    pub fn scan(&self) -> Option<&Arc<ScannedSong>> { self.scan.as_ref() }

    /// How many voices this module's own processor would have wanted, for a caller that
    /// wants to report it. The engine itself is built at [`MAX_VOICE_CAPACITY`](starplayer::MAX_VOICE_CAPACITY).
    pub fn recommended_voice_capacity(&self) -> Option<usize> {
        self.module.as_ref().map(|module| recommended_voice_capacity(module))
    }

    /// Start, or resume, playing. Glides up over 64 frames, so it does not click.
    pub fn play(&mut self) -> Result<(), HostError> {
        if self.interactive_enabled { self.interactive(InteractiveAction::Play) } else { self.queue(HostCommand::Play) }
    }

    pub fn pause(&mut self) -> Result<(), HostError> { self.interactive(InteractiveAction::Pause) }
    pub fn resume(&mut self) -> Result<(), HostError> { self.interactive(InteractiveAction::Resume) }
    pub fn clear_and_hold(&mut self) -> Result<(), HostError> { self.interactive(InteractiveAction::Stop) }
    pub fn all_sound_off(&mut self) -> Result<(), HostError> { self.interactive(InteractiveAction::AllSoundOff) }
    /// Queue after a complete load/replay transaction. A full queue returns an error;
    /// callers must not treat a queued load or the control shadow as applied audio.
    pub fn playback_barrier(&mut self, serial: u64) -> Result<(), HostError> {
        let identity = self.playback_identity.ok_or(HostError::InvalidInteractiveCommand)?;
        self.queue(HostCommand::AppliedBarrier { identity, serial, rejected: self.commands_rejected() })
    }

    pub fn pending_control_commands(&self) -> usize { self.commands.len() }

    pub fn applied_playback(&mut self) -> AppliedPlayback { self.telemetry.read().applied }

    /// Explicit identities let persistent hosts avoid reuse across replacement instances.
    pub fn load_module_identified(&mut self, module: RtArc<Module>, identity: PlaybackIdentity) -> Result<(), HostError> {
        if !self.interactive_enabled || self.playback_identity.is_none_or(|old| identity.build.0 <= old.build.0 || identity.session.0 <= old.session.0) { return Err(HostError::InvalidInteractiveCommand); }
        self.install_identified(module, SeekKind::None, None, true, Some(identity))
    }

    pub const fn playback_identity(&self) -> Option<PlaybackIdentity> { self.playback_identity }
    pub fn interactive_transport(&self) -> Option<InteractiveTransport> {
        self.interactive_enabled.then(|| match self.taps.transport_state.load(Ordering::Relaxed) { 1 => InteractiveTransport::Playing, 2 => InteractiveTransport::Paused, _ => InteractiveTransport::Stopped })
    }
    pub fn natural_end(&self) -> bool { self.taps.natural_end.load(Ordering::Relaxed) }

    /// Route a generation-bearing request through host fade handling, never directly to
    /// the engine. Later feature tasks extend the same shadow/replay seam for settings.
    /// Accept an atomic command from a wire/client, rejecting old sessions/builds before
    /// changing host shadow or fade state.
    pub fn submit_interactive(&mut self, command: InteractiveCommand) -> Result<(), HostError> {
        if self.playback_identity != Some(command.identity) { return Err(HostError::InvalidInteractiveCommand); }
        self.interactive(command.action)
    }

    /// Pure validation shared with bounded browser batch admission. No shadow or queue mutation.
    pub fn validate_interactive(&self, action: InteractiveAction) -> Result<(), HostError> {
        if !self.interactive_enabled && matches!(action, InteractiveAction::Play | InteractiveAction::Pause | InteractiveAction::Resume | InteractiveAction::Stop | InteractiveAction::AllSoundOff | InteractiveAction::StopPreview | InteractiveAction::SetLiveDestination(_) | InteractiveAction::PreviewInstrument(_) | InteractiveAction::ReleasePreview) {
            return Err(HostError::InvalidInteractiveCommand);
        }
        if matches!(action, InteractiveAction::SetNoteOverlapEnabled(_) | InteractiveAction::SetLetNotesRing { .. }) && !self.engine_settings.playback_configuration().is_some_and(|configuration| configuration.capabilities.contains(starplayer::core::PlaybackCapabilities::NOTE_OVERLAP)) { return Err(HostError::InvalidInteractiveCommand); }
        if let InteractiveAction::SetLetNotesRing { instrument, .. } = action && instrument.0 as usize >= self.instrument_shadow.len() { return Err(HostError::InvalidInteractiveCommand); }
        #[cfg(feature = "interactive-audition")]
        match action {
            InteractiveAction::PreviewInstrument(instrument) if !self.module.as_ref().is_some_and(|module| starplayer::live_instrument_capability(module, instrument).preview_playable) => return Err(HostError::InvalidInteractiveCommand),
            InteractiveAction::SetLiveDestination(starplayer::core::LiveDestination::Selected(instrument)) if !self.module.as_ref().is_some_and(|module| starplayer::live_instrument_capability(module, instrument).playable) => return Err(HostError::InvalidInteractiveCommand),
            _ => {}
        }
        self.playback_identity.ok_or(HostError::InvalidInteractiveCommand)?;
        Ok(())
    }

    /// Conservatively reserve a whole browser group and its final applied barrier.
    /// Existing Host commands can expand to at most two Engine commands (fresh Load).
    /// The last callback reports actual Engine backlog; four further slots cover a held
    /// fresh Load (two), plus end/settlement work. Completion also checks both real queues.
    /// The browser owns the sole producers and cannot race another sender between checks.
    pub fn can_admit_browser_batch(&self, host_commands: usize, inserts: usize, events: usize) -> bool {
        let pending = self.commands.len();
        pending.saturating_add(host_commands).saturating_add(1) <= self.commands.capacity()
            && (self.taps.pending_engine_commands.load(Ordering::Relaxed) as usize).saturating_add(pending.saturating_mul(2)).saturating_add(host_commands).saturating_add(4) <= self.engine_settings.command_capacity.min(starplayer::engine::MAX_COMMANDS_PER_QUANTUM)
            && inserts <= self.insert_control.command_room()
            && (events == 0 || self.events.as_ref().is_some_and(|sender| events <= sender.capacity().saturating_sub(sender.queued())))
    }

    pub fn interactive(&mut self, action: InteractiveAction) -> Result<(), HostError> {
        self.validate_interactive(action)?;
        let identity = self.playback_identity.ok_or(HostError::InvalidInteractiveCommand)?;
        self.queue(HostCommand::Interactive(InteractiveCommand { identity, action }))?;
        #[cfg(feature = "interactive-audition")]
        if (matches!(action, InteractiveAction::Stop | InteractiveAction::StopPreview | InteractiveAction::AllSoundOff) || matches!(action, InteractiveAction::SetLiveDestination(destination) if destination != self.live_destination))
            && let Some(epoch) = &self.live_epoch { epoch.fetch_add(1, starplayer::rt::atomic::Ordering::AcqRel); }
        match action {
            InteractiveAction::SetLiveDestination(destination) => { self.live_destination = destination; self.live_input = if destination == starplayer::core::LiveDestination::Jam { LiveInputMode::Jam } else { LiveInputMode::Off }; },
            InteractiveAction::SetNoteOverlapEnabled(enabled) => self.overlap_shadow = enabled,
            InteractiveAction::SetLetNotesRing { instrument, enabled } => { if let Some(value) = self.instrument_shadow.get_mut(instrument.0 as usize) { *value = enabled; } }
            _ => {}
        }
        self.transport_shadow = match action {
            InteractiveAction::Play | InteractiveAction::Resume => InteractiveTransport::Playing,
            InteractiveAction::Pause => InteractiveTransport::Paused,
            InteractiveAction::Stop => InteractiveTransport::Stopped,
            _ => self.transport_shadow,
        };
        Ok(())
    }

    fn next_identity(&self, new_session: bool) -> Option<PlaybackIdentity> {
        self.playback_identity.map(|mut identity| {
            if new_session { identity.session.0 = identity.session.0.wrapping_add(1); }
            identity.build.0 = identity.build.0.wrapping_add(1);
            identity
        })
    }

    /// Stop. Glides down over 64 frames and stops the musical clock when the ramp lands, so
    /// it does not click either.
    pub fn stop(&mut self) -> Result<(), HostError> { self.queue(HostCommand::Stop) }

    /// Jump to an order-list index.
    pub fn seek_order(&mut self, order: u16) -> Result<(), HostError> { self.request_seek(SeekKind::Order(order)) }

    /// Jump to a row of the pattern already playing.
    pub fn seek_row(&mut self, row: u16) -> Result<(), HostError> { self.request_seek(SeekKind::Row(row)) }

    /// Jump to an elapsed position in the song, in frames.
    pub fn seek_frame(&mut self, song_frame: u64) -> Result<(), HostError> { self.request_seek(SeekKind::Frame(song_frame)) }

    fn request_seek(&mut self, kind: SeekKind) -> Result<(), HostError> {
        if self.module.is_none() {
            return Err(HostError::NoModule);
        }
        self.handles.seek.request(SeekRequest { kind, frame: Frame(self.taps.source_frame.load(Ordering::Relaxed)) });
        // A seek jumps every delay line and envelope to a different musical moment, so an
        // installed effect's state — a reverb tail, a delay's feedback loop — has to be
        // cleared with it or it carries audio from the position the ear just left into the
        // position it landed on. Parameters are untouched: `InsertCommand::ResetAll` clears
        // state only.
        // Interactive musical validity belongs to the audio-side source. Its successful
        // seek requests the reset in that same dispatch; rejected seeks leave DSP intact.
        if !self.interactive_enabled { let _ = self.insert_control.reset_all(); }
        Ok(())
    }

    /// What the caller last chose to happen when the song has been heard through once.
    pub fn at_end(&self) -> AtEnd { self.at_end }

    /// The bias [`Player::set_speed_adjust`] is applying.
    pub fn speed_adjust(&self) -> i8 { self.speed_adjust }

    /// Add `adjust` ticks per row to every speed a module asks for.
    ///
    /// Zero — the default — is canonical playback. A non-zero value is for a module
    /// written against a game's own replayer rather than against a tracker; see
    /// [`NativeSequencer::set_speed_adjust`](starplayer::NativeSequencer::set_speed_adjust).
    ///
    /// **It takes effect on the next load.** The song's timeline is measured at the
    /// adjustment it plays at, and the scan happens at load; changing this without
    /// reloading would leave the progress slider describing a song that is no longer
    /// being played. Dropping the cached scan here is what makes a caller that sets this
    /// and then reloads the same bytes get a fresh measurement.
    pub fn set_speed_adjust(&mut self, adjust: i8) {
        if adjust == self.speed_adjust {
            return;
        }
        self.speed_adjust = adjust;
        self.scan = None;
    }

    /// Choose what happens when the song has been heard through once.
    pub fn set_at_end(&mut self, at_end: AtEnd) -> Result<(), HostError> {
        self.at_end = at_end;
        self.handles.at_end.set(at_end);
        self.queue(HostCommand::SetAtEnd { at_end, fade_frames: 0 })
    }

    /// How long the fade lasts, in frames. Zero leaves the current length alone.
    pub fn set_fade_frames(&mut self, fade_frames: u32) -> Result<(), HostError> {
        self.fade_frames = fade_frames;
        self.queue(HostCommand::SetAtEnd { at_end: self.at_end, fade_frames })
    }

    /// Set the master volume, applied by the master bus before the limiter.
    pub fn set_master_volume(&mut self, volume: U0F16) -> Result<(), HostError> {
        self.master_volume = volume;
        self.queue(HostCommand::Engine(Command::SetMasterVolume(volume)))
    }

    /// Mute or unmute one channel. A muted channel's voices still render, into scratch, so
    /// its state advances exactly as if it were audible.
    pub fn mute(&mut self, channel: ChannelId, muted: bool) -> Result<(), HostError> {
        self.queue(HostCommand::Engine(Command::MuteChannel { channel, muted }))
    }

    // ── live input ──────────────────────────────────────────────────────────────────
    //
    // Task E6. The engine half is `starplayer_engine::instrument`: an `InstrumentRack`
    // bound to the loaded module's instruments, fed by an `ExternalEventQueue` through a
    // `MidiSource`. What a host adds is the *stamp* — see `crate::events` for which clock
    // it is taken from and why the lead has a floor.

    /// Play the loaded module's **instruments** from live input, with the module itself
    /// silent.
    ///
    /// The queue is made here rather than at [`Player::open`] because the rack it feeds
    /// needs the module: sixteen MIDI channels, each bound to one of the module's
    /// instruments. Everything expensive happens on this thread — the rack, the queue's
    /// ring and the boxed source — and only a `Box` and an `Arc` cross to the audio side.
    ///
    /// The module stops playing. [`Player::jam`] is the public surface for putting the
    /// same source beside a playing module; this lower-level mode remains useful to an
    /// instrument-only host. [`Player::restore_module_source`] puts the module back.
    ///
    /// The transport has to be **running** for anything to sound: the musical clock is
    /// what a source is dispatched against, and it stops with the transport.
    pub fn midi_only(&mut self) -> Result<(), HostError> {
        if self.interactive_enabled { return Err(HostError::Backend(String::from("interactive input uses the permanent live slot"))); }
        let module = self.module.clone().ok_or(HostError::NoModule)?;
        let rack = InstrumentRack::for_module(&module, self.spec.sample_rate_hz);
        let (producer, queue) = external_event_channel(EVENT_QUEUE_CAPACITY);
        let mut source = MidiSource::new(queue, rack, self.spec.sample_rate_hz);
        // The engine's musical clock has been running since the stream opened; a source
        // whose control tick starts at zero would otherwise spend its first dispatch
        // resynchronising.
        source.start_control_at(Frame(self.taps.source_frame.load(Ordering::Relaxed)));
        // The same module handle, sent again on purpose: `Command::LoadModule` releases
        // every voice and forgets every binding, which is exactly what silencing the
        // tracker lanes as the source is swapped means. Nothing is decoded or scanned.
        self.commands
            .push(HostCommand::Load { module, source: Box::new(source), identity: None, selections: None, new_session: true })
            .map_err(|_| HostError::ControlQueueFull)?;
        self.events = Some(EventSender::new(producer, Arc::clone(&self.event_clock)));
        self.live_input = LiveInputMode::MidiOnly;
        Ok(())
    }

    /// Enable or disable live input over the playing module.
    ///
    /// The module sequencer is always slot 0 and the MIDI source is slot 1, making a
    /// same-frame collision deterministic. Both transitions rebuild the sequencer at the
    /// sounding song position and on the current monotonic musical frame, so toggling jam
    /// mode does not return the song to its first row. The seek mailbox stays inside slot
    /// 0, and therefore order, row and elapsed-frame seeks keep working unchanged.
    pub fn jam(&mut self, enabled: bool) -> Result<(), HostError> {
        if self.interactive_enabled {
            self.module.as_ref().ok_or(HostError::NoModule)?;
            self.interactive(InteractiveAction::SetLiveDestination(if enabled { starplayer::core::LiveDestination::Jam } else { starplayer::core::LiveDestination::Off }))?;
            self.live_input = if enabled { LiveInputMode::Jam } else { LiveInputMode::Off };
            return Ok(());
        }
        if enabled == (self.live_input == LiveInputMode::Jam) {
            return Ok(());
        }
        let module = self.module.clone().ok_or(HostError::NoModule)?;
        let frame = Frame(self.taps.source_frame.load(Ordering::Relaxed));
        let song_frame = self.telemetry.read().engine.transport.song_frame;
        self.handles.seek.clear();
        let built = build_source_with_settings(
            RtArc::clone(&module),
            SeekKind::Frame(song_frame),
            frame,
            &self.handles,
            self.scan.clone(),
            self.speed_adjust,
            self.engine_settings,
        )?;
        let (source, events) = if enabled {
            let (source, producer) = self.build_jam_source(&module, frame, built.source)?;
            (source, Some(EventSender::new(producer, Arc::clone(&self.event_clock))))
        } else {
            (built.source, None)
        };
        self.commands
            .push(HostCommand::Load { module, source, identity: None, selections: None, new_session: true })
            .map_err(|_| HostError::ControlQueueFull)?;
        self.scan = Some(built.scanned);
        self.smf = None;
        #[cfg(feature = "interactive-audition")]
        { self.live_epoch = events.as_ref().and_then(EventSender::cancellation_token); }
        self.events = events;
        self.live_input = if enabled { LiveInputMode::Jam } else { LiveInputMode::Off };
        Ok(())
    }

    /// Put the module's own sequencer back, ending live input.
    ///
    /// The scan is reused — nothing the timeline depends on changed — so this costs a
    /// sequencer, not a rescan, and the song restarts from the top of the order list.
    pub fn restore_module_source(&mut self) -> Result<(), HostError> {
        if self.interactive_enabled { return self.jam(false); }
        if self.live_input == LiveInputMode::Jam {
            return self.jam(false);
        }
        let module = self.module.clone().ok_or(HostError::NoModule)?;
        let cached_scan = self.scan.clone();
        self.live_input = LiveInputMode::Off;
        self.install(module, SeekKind::None, cached_scan, false)
    }

    /// Whether the engine is playing a live-input source.
    pub const fn is_midi_only(&self) -> bool { matches!(self.live_input, LiveInputMode::MidiOnly) }

    /// Whether live input and the module's sequencer share the source mux.
    pub const fn is_jamming(&self) -> bool { matches!(self.live_input, LiveInputMode::Jam) }

    /// Queue one live event on MIDI channel `channel` (0–15), stamped
    /// `source_frame + lead`.
    ///
    /// Allocation-free and non-blocking, so the browser host calls it from inside the
    /// worklet's command drain. A full queue is counted and reported
    /// ([`HostError::EventQueueFull`]), never waited on.
    pub fn send_event(&mut self, channel: u8, event: Event) -> Result<(), HostError> {
        let sender = self.events.as_mut().ok_or(HostError::NoEventQueue)?;
        sender.send_event(channel, event)
    }

    /// Take the sending half away, so another thread can own it.
    ///
    /// The queue underneath is single-producer, so there is exactly one of these and
    /// [`Player::send_event`] stops working once it has gone. A native host moves it into
    /// its `midir` callback; the browser host never calls this.
    pub fn take_event_sender(&mut self) -> Option<EventSender> { self.events.take() }

    /// The clock, the lead and the live-input counters, shareable with whoever is sending.
    pub fn event_clock(&self) -> &Arc<EventClock> { &self.event_clock }

    /// Ask for a stamping lead in frames. The effective lead never drops below what the
    /// device's own block size requires — see [`EventClock::lead_frames`].
    pub fn set_event_lead(&mut self, frames: u32) { self.event_clock.set_requested_lead_frames(frames); }

    /// The lead actually applied to the next live event, in frames.
    pub fn event_lead(&self) -> u32 { self.event_clock.lead_frames() }

    /// The same lead in milliseconds, which is what a UI shows next to a keyboard toggle.
    pub fn event_lead_millis(&self) -> f32 { self.event_clock.lead_millis() }

    /// Live events a full queue refused.
    pub fn events_rejected(&self) -> u64 { self.event_clock.rejected() }

    /// Live events accepted onto the queue.
    pub fn events_sent(&self) -> u64 { self.event_clock.sent() }

    fn queue(&mut self, command: HostCommand) -> Result<(), HostError> {
        self.commands.push(command).map_err(|_| HostError::ControlQueueFull)
    }

    /// The most recent coherent snapshot the audio thread published: transport position,
    /// per-channel note, effect and VU.
    pub fn telemetry(&mut self) -> &Snapshot { &self.telemetry.read().engine }

    /// Sticky engine warnings, as of the last snapshot.
    pub fn warnings(&mut self) -> EngineWarnings {
        let snapshot = self.telemetry.read().engine;
        EngineWarnings {
            zero_advance_forced: snapshot.warnings.zero_advance_forced,
            event_limit_reached: snapshot.warnings.event_limit_reached,
            retired_module_dropped: snapshot.warnings.retired_module_dropped,
            unsupported_command: snapshot.warnings.unsupported_command,
            late_events: snapshot.warnings.late_events,
            // `retired_insert_dropped` (M7-H1) now has a telemetry home: H7 widened
            // `starplayer_telemetry::WarningFlags` with the same name, and it is folded
            // into the snapshot on every publish (`starplayer-engine`'s
            // `From<EngineWarnings> for WarningFlags`), so it reads back here exactly
            // like every other sticky flag.
            retired_insert_dropped: snapshot.warnings.retired_insert_dropped,
        }
    }

    /// One pass of the song in frames, from the scan.
    pub fn song_length(&self) -> Option<u64> { self.scan.as_ref().map(|scanned| scanned.timeline.end_frame()) }

    /// How far into the song the audio thread is, in frames — what a progress bar draws.
    pub fn song_frame(&mut self) -> u64 { self.telemetry.read().engine.transport.song_frame }

    /// The musical clock, in frames since the stream opened.
    pub fn source_frame(&self) -> Frame { Frame(self.taps.source_frame.load(Ordering::Relaxed)) }

    /// The output clock, which never stops.
    pub fn output_frame(&self) -> Frame { Frame(self.taps.output_frame.load(Ordering::Relaxed)) }

    /// Whether the transport is running: the musical clock is going and no stop is queued.
    pub fn is_playing(&self) -> bool { self.taps.playing.load(Ordering::Relaxed) }

    /// Whether the song fade is running — the transport is playing a pass it will fade out
    /// of, rather than merely being quiet.
    pub fn is_fading(&self) -> bool { self.taps.fading.load(Ordering::Relaxed) }

    /// The last block's peak, `0.0..=1.0`. A lossy tap: a stale reading costs a meter
    /// nothing.
    pub fn peak(&self) -> f32 { f32::from_bits(self.taps.peak_bits.load(Ordering::Relaxed)) }

    /// Blocks the backend has asked for since the stream opened. Zero after a second of
    /// playing means the callback is not running.
    pub fn blocks_rendered(&self) -> u64 { self.taps.blocks_rendered.load(Ordering::Relaxed) }

    /// Commands the audio side could not deliver, because a ring was full.
    pub fn commands_rejected(&self) -> u64 { self.taps.commands_rejected.load(Ordering::Relaxed) }

    /// Drop everything the audio thread has finished with, and report the running total.
    ///
    /// **Call this regularly.** It is the only place a retired module, sequencer or insert
    /// effect is returned to the allocator — a retired reverb's delay lines are exactly as
    /// real an allocation as a retired module, and [`HostInsertControl::collect_all_garbage`]
    /// is what frees them.
    pub fn collect_garbage(&mut self) -> usize {
        while let Some(retired) = self.retired.pop() {
            drop(retired);
            self.collected = self.collected.saturating_add(1);
        }
        self.insert_control.collect_all_garbage();
        self.collected
    }

    // ── inserts ─────────────────────────────────────────────────────────────────────
    //
    // M7-H7. `InsertHandle::install`/`send` push onto the audio side's insert ring
    // directly from this thread — unlike `Command`, an insert change has no transport
    // ramp to sequence around, so there is nothing for `HostCommand` to add. Building an
    // effect happens **here**, in `HostInsertControl::install`, never inside a render
    // callback or the ring's own drain.

    /// A slot outside `0 ..= starplayer_engine::insert::MAX_INSERTS_PER_CHAIN - 1` refuses
    /// rather than silently retiring whatever the engine would build for it.
    fn validate_insert_slot(slot: u8) -> Result<(), HostError> {
        if (slot as usize) < MAX_INSERTS_PER_CHAIN { Ok(()) } else { Err(HostError::InvalidInsertSlot(slot)) }
    }

    /// Build `kind` off the audio thread and install it in `slot` of `target`'s chain,
    /// retiring whatever was there. See `starplayer_engine::insert` for the fixed
    /// topology: four ordered slots per channel bus, four more on the master, ahead of the
    /// master volume and the limiter.
    pub fn install_insert(&mut self, target: InsertTarget, slot: u8, kind: InsertKind) -> Result<(), HostError> {
        Player::validate_insert_slot(slot)?;
        self.insert_control.install(target, slot, kind, self.spec.sample_rate_hz)?;
        self.insert_layout.record_install(target, slot, kind);
        Ok(())
    }

    /// Take whatever is in `slot` of `target`'s chain out and retire it.
    pub fn remove_insert(&mut self, target: InsertTarget, slot: u8) -> Result<(), HostError> {
        Player::validate_insert_slot(slot)?;
        self.insert_control.remove(target, slot)?;
        self.insert_layout.record_remove(target, slot);
        Ok(())
    }

    /// Set one parameter of the effect in `slot` of `target`'s chain, in the parameter's
    /// own fixed unit (its `InsertDescriptor::params` entry says which). Real-time safe on
    /// the audio side: a copy and the start of a smoothing ramp, never a rebuild.
    pub fn set_insert_param(&mut self, target: InsertTarget, slot: u8, param: ParamId, value: i32) -> Result<(), HostError> {
        Player::validate_insert_slot(slot)?;
        self.insert_control.set_param(target, slot, param, value)?;
        self.insert_layout.record_param(target, slot, param, value);
        Ok(())
    }

    /// Skip, or stop skipping, the effect in `slot` of `target`'s chain. A bypassed effect
    /// still receives parameter changes and is still reset by a seek, so un-bypassing it
    /// does not step into a stale state.
    pub fn bypass_insert(&mut self, target: InsertTarget, slot: u8, bypassed: bool) -> Result<(), HostError> {
        Player::validate_insert_slot(slot)?;
        self.insert_control.bypass(target, slot, bypassed)?;
        self.insert_layout.record_bypassed(target, slot, bypassed);
        Ok(())
    }

    /// What this host believes is installed, per target and slot.
    pub fn inserts(&self) -> &InsertLayout { &self.insert_layout }

    /// One [`TapReader`] per channel, claimed once (architecture §9(b)).
    ///
    /// The scope taps are the lossy half of telemetry: they sample voice state, never the
    /// mix, and a reader that races the audio thread sees a torn window, which on a scope is
    /// invisible. A host that draws no scope never calls this and pays nothing for the rings,
    /// which the engine allocates either way.
    ///
    /// The readers belong to *this* engine, so a [`Player::reopen`] or a
    /// [`Player::set_mixer_mode`] retires them: take them again from the rebuilt player.
    pub fn take_scope_readers(&mut self) -> Option<Box<[TapReader]>> { self.scopes.take() }

    /// Stop the stream and let the callback — and the engine inside it — go.
    pub fn close(self) { drop(self) }
}

/// Every module the audio thread finished with is dropped here, on the control thread,
/// including the ones still in flight when the player goes away.
impl Drop for Player {
    fn drop(&mut self) {
        while self.retired.pop().is_some() {}
    }
}

/// Frames one call to a `Player`'s callback walks before it drains the control plane again.
///
/// Exposed because it is the cadence a test drives the manual backend at, and because it is
/// the answer to "how late can a seek or an end-of-song stop be?" — 128 frames, which is
/// 2.7 ms at 48 kHz, whatever block size the device asked for.
pub const CONTROL_CADENCE_FRAMES: usize = RENDER_QUANTUM;

/// The retired handles waiting for a caller to drop them, for a test that wants to see the
/// hand-back happen rather than infer it.
impl Player {
    /// How many retirements are waiting for [`Player::collect_garbage`].
    pub fn pending_garbage(&self) -> usize { self.retired.len() }

    /// The seek the audio side has not consumed yet, for a test that wants to see a rewind
    /// queued rather than infer it from where the song restarts.
    pub fn pending_seek(&self) -> SeekRequest { self.handles.seek.peek() }

    /// Every device the backend can see. A convenience so a caller need not import the
    /// backend trait to print a list.
    pub fn devices(backend: &dyn AudioBackend) -> Vec<crate::backend::DeviceInfo> { backend.devices() }
}

#[cfg(all(test, feature = "interactive-audition"))]
mod no08_attachment_tests {
    use super::*;
    use crate::{ManualBackend, ManualDriver};
    use starplayer::core::{PlaybackCapabilities, RowClock, SequencerCursor, SmfCursor, SmfPlaybackState};
    use starplayer::engine::{PlaybackConfiguration, VoiceCapacity};

    fn open() -> (Player, ManualDriver) {
        let mut backend = ManualBackend::new();
        let settings = EngineSettings::default().with_playback(PlaybackConfiguration { identity: Default::default(), capabilities: PlaybackCapabilities::INTERACTIVE_AUDITION, instrument_count: 31, capacity: VoiceCapacity { native: 64, custom: 0, live: 2, interactive_transport: true } }).unwrap();
        let player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
        (player, backend.driver().unwrap())
    }
    fn pump(player: &mut Player, driver: &ManualDriver, frames: usize) { driver.render(&mut vec![0.0; frames * 2]); player.telemetry(); }
    fn pressure(player: &mut Player, driver: &ManualDriver) {
        for _ in 0..12 { player.load(include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod")).unwrap(); }
        pump(player, driver, 8192);
        assert!(player.pending_control_commands() > 0, "must defer on the real retirement queue");
    }
    fn install_wait(player: &mut Player, driver: &ManualDriver) -> Frame {
        player.playback_barrier(123).unwrap();
        for _ in 0..64 {
            player.collect_garbage();
            let before = player.source_frame();
            pump(player, driver, 128);
            if player.applied_playback().serial == 123 { return before; }
        }
        panic!("deferred install did not settle");
    }
    #[test]
    fn no08_real_retirement_deferral_preserves_saved_tracker_cursor_in_every_transport() {
        for state in [InteractiveTransport::Playing, InteractiveTransport::Paused, InteractiveTransport::Stopped] {
            let (mut player, driver) = open();
            pressure(&mut player, &driver);
            let saved = SequencerCursor { row_clock: RowClock { tick_in_row: 2, ..RowClock::new(6) }, tempo_bpm: 130, requested_speed: 6, remaining_frames: 617, fractional_remainder: 0x87654321, song_frame: 1234, sample_rate_hz: 44_100, ..Default::default() };
            player.rebuild_cursor = Some(saved);
            player.install(RtArc::new(starplayer::load(include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod")).unwrap()), SeekKind::None, None, false).unwrap();
            let prepared = player.source_frame();
            player.queue(HostCommand::RestoreTransport { identity: player.playback_identity.unwrap(), transport: Transport::playing(), state, natural_end: false }).unwrap();
            pump(&mut player, &driver, 4096); assert!(player.pending_control_commands() > 0);
            let attached = install_wait(&mut player, &driver);
            assert!(attached.0 >= prepared.0 + 4096);
            let restored = player.telemetry().transport.sequencer_cursor().unwrap();
            assert_eq!(restored.song_frame, saved.song_frame); assert_eq!(restored.remaining_frames, saved.remaining_frames); assert_eq!(restored.fractional_remainder, saved.fractional_remainder);
            let resume = if state == InteractiveTransport::Playing { attached } else { player.source_frame() };
            if state != InteractiveTransport::Playing { player.resume().unwrap(); }
            while player.source_frame().0 <= resume.0 + saved.remaining_frames { pump(&mut player, &driver, 128); }
            let next = player.telemetry().transport.sequencer_cursor().unwrap();
            assert_eq!(next.captured_frame.0, resume.0 + saved.remaining_frames);
            assert_eq!(next.row_clock.tick_in_row, 3);
        }
    }
    #[test]
    fn no08_real_retirement_deferral_preserves_saved_smf_cursor_in_every_transport() {
        let bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x0d\0\x90\x3c\x64\x60\x90\x40\x64\x60\xff\x2f\0";
        // Track length is twelve bytes: two notes then end-of-track.
        let mut bytes = bytes.to_vec(); bytes[21] = 12;
        for state in [InteractiveTransport::Playing, InteractiveTransport::Paused, InteractiveTransport::Stopped] {
            let (mut player, driver) = open(); pressure(&mut player, &driver);
            let saved = SmfPlaybackState { cursor: SmfCursor { event_index: 1, song_frame: 1234, remaining_frames: Some(617), captured_frame: player.source_frame(), sample_rate_hz: 44_100 }, ..Default::default() };
            let smf = Arc::new(starplayer::midi::smf::parse_smf(&bytes).unwrap());
            let instruments = RtArc::new(starplayer::load(include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod")).unwrap());
            player.install_smf(smf, instruments, Some(saved), false).unwrap();
            let prepared = player.source_frame();
            player.queue(HostCommand::RestoreTransport { identity: player.playback_identity.unwrap(), transport: Transport::playing(), state, natural_end: false }).unwrap();
            pump(&mut player, &driver, 4096); assert!(player.pending_control_commands() > 0);
            let attached = install_wait(&mut player, &driver); assert!(attached.0 >= prepared.0 + 4096);
            let restored = player.telemetry().transport.smf_state().unwrap();
            assert_eq!(restored.cursor.song_frame, saved.cursor.song_frame); assert_eq!(restored.cursor.remaining_frames, saved.cursor.remaining_frames); assert_eq!(restored.cursor.event_index, 1);
            let resume = if state == InteractiveTransport::Playing { attached } else { player.source_frame() };
            if state != InteractiveTransport::Playing { player.resume().unwrap(); }
            while player.source_frame().0 <= resume.0 + 1024 { pump(&mut player, &driver, 128); }
            let next = player.telemetry().transport.smf_state().unwrap(); assert_eq!(next.cursor.event_index, 2);
        }
    }
}

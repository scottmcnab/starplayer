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
//! Four things cross, and each has exactly one mechanism:
//!
//! | | direction | mechanism |
//! |---|---|---|
//! | commands, and a freshly built module and source | control → audio | [`HostCommand`] over an SPSC ring |
//! | seeks | control → audio | [`SeekMailbox`], latest wins |
//! | retired modules and retired sources | audio → control | an SPSC ring; **dropped here**, never there |
//! | telemetry, position, peak | audio → control | a snapshot channel and a handful of atomics |
//! | live MIDI and keyboard events | control (or any thread) → audio | an [`ExternalEventQueue`](starplayer::engine::ExternalEventQueue) of absolutely-stamped events; see `src/events.rs` |
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

use std::boxed::Box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::vec::Vec;

use starplayer::core::{AtEnd, ChannelId, Command, Event, Frame, U0F16};
use starplayer::engine::{
    EngineHandle, EngineWarnings, EventSource, InstrumentRack, MidiSource, MixerMode, RENDER_QUANTUM,
    external_event_channel,
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
use crate::source::{SeekKind, SeekRequest, SourceHandles, build_source};
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
    /// Ramp up and start the musical clock.
    Play,
    /// Ramp down and stop it when the ramp lands.
    Stop,
    /// A module and the source built for it, both already decoded, scanned and allocated
    /// on the control thread. Nothing here is constructed in the callback and nothing is
    /// dropped there.
    Load { module: RtArc<Module>, source: Box<dyn EventSource> },
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
    Source(Box<dyn EventSource>),
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
    /// Whether the song fade is running, which a UI shows and a wire header carries.
    fading: AtomicBool,
    /// `f32::to_bits` of the last block's peak.
    peak_bits: AtomicU32,
    blocks_rendered: AtomicU64,
    commands_rejected: AtomicU64,
}

/// The audio side: everything the backend's callback owns.
struct RenderState {
    engine: HostEngine,
    control: EngineHandle<RtArc<Module>>,
    telemetry: TelemetryReader,
    forwarded: SnapshotPublisher<Snapshot>,
    commands: Consumer<HostCommand>,
    retired: Producer<Retired>,
    handles: SourceHandles,
    transport: Transport,
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
        self.publish(peak);
    }

    fn drain_commands(&mut self) {
        for _ in 0..HOST_COMMAND_CAPACITY {
            let Some(command) = self.commands.pop() else { break };
            self.apply(command);
        }
    }

    fn apply(&mut self, command: HostCommand) {
        match command {
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
            HostCommand::Load { module, source } => {
                if let Some(old) = self.engine.replace_source(source) {
                    self.retire(Retired::Source(old));
                }
                let _ = self.send(Command::LoadModule(module));
            }
            HostCommand::SetAtEnd { at_end, fade_frames } => {
                self.transport.set_fade_frames(fade_frames);
                self.handles.at_end.set(at_end);
                self.transport.cancel_fade(at_end);
            }
            HostCommand::Engine(command) => { let _ = self.send(command); }
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
        let running = self.engine.is_playing() && !self.transport.stop_is_pending() && !self.awaiting_engine_stop;
        if !running || !self.last_snapshot.transport.end_reached || self.transport.is_fading() {
            return;
        }
        let loops = self.last_snapshot.transport.song_end == SongEnd::Loops;
        match self.handles.at_end.get() {
            AtEnd::FadeOut if loops => self.transport.begin_fade(),
            AtEnd::FadeOut | AtEnd::Stop if !loops => self.transport.begin_stop(true),
            _ => {}
        }
    }

    /// A fade that has run its course, and a ramp that has reached zero, both end in the
    /// same typed engine stop. A song that faded out or ended is over, not paused, so it
    /// rewinds for the next Play; a Stop the caller asked for keeps its place.
    fn settle_transport(&mut self) {
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
        while let Some(module) = self.control.collect_garbage() {
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
        // The *musical* clock, not the output one: a live event is dispatched against the
        // frame an `EventSource` reports in, which stops with the transport. See
        // `crate::events`.
        self.event_clock.publish(self.engine.source_frame());
        self.taps.output_frame.store(self.engine.frame().0, Ordering::Relaxed);
        self.taps.playing.store(self.engine.is_playing() && !self.transport.stop_is_pending(), Ordering::Relaxed);
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
        let _ = self.forwarded.publish(*self.telemetry.read());
    }
}

/// The control-side halves that [`RenderState`] does not take.
struct ControlSide {
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<Snapshot>,
    taps: Arc<PlayerTaps>,
    handles: SourceHandles,
    /// Taken from the engine here, because [`Player::open`] is the last moment a `&mut` to
    /// it exists on this thread.
    scopes: Option<Box<[TapReader]>>,
    event_clock: Arc<EventClock>,
}

/// Build both sides of one engine at `spec`'s rate, in `mode`.
///
/// The engine is stopped and the transport silent, so a stream may be started immediately
/// without the song advancing under it: nothing sounds until [`Player::play`].
fn build_sides(mode: MixerMode, spec: AudioSpec) -> Result<(RenderState, ControlSide), HostError> {
    if mode.channels as u16 != spec.channels {
        let reason = std::format!("the engine renders {} channels, the device wants {}", mode.channels, spec.channels);
        return Err(HostError::UnsupportedSpec { requested: spec, reason });
    }
    let (mut engine, mut control, telemetry) = HostEngine::build(mode, spec.sample_rate_hz)?;
    let scopes = engine.scope_readers();
    // Freeze the musical clock until the first Play. Without this a stream that is started
    // as soon as it is open plays the song silently while the caller is still deciding.
    control.send(Command::Stop).map_err(|_| HostError::ControlQueueFull)?;

    let (command_producer, command_consumer) = channel(HOST_COMMAND_CAPACITY);
    let (retired_producer, retired_consumer) = channel(RETIRED_CAPACITY);
    let (forwarded, snapshot_reader) = snapshot_channel(TELEMETRY_DEPTH, Snapshot::default());
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
        retired: retired_producer,
        handles: handles.clone(),
        transport: Transport::stopped(),
        taps: Arc::clone(&taps),
        last_snapshot: Snapshot::default(),
        awaiting_engine_stop: false,
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
    stream: Stream,
    spec: AudioSpec,
    mode: MixerMode,
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<Snapshot>,
    taps: Arc<PlayerTaps>,
    handles: SourceHandles,
    at_end: AtEnd,
    fade_frames: u32,
    master_volume: U0F16,
    module: Option<RtArc<Module>>,
    scan: Option<Arc<ScannedSong>>,
    collected: usize,
    scopes: Option<Box<[TapReader]>>,
    /// The clock, the lead and the counters live input is stamped and reported against.
    /// Made once per stream, shared with the render callback and with every
    /// [`EventSender`] handed out.
    event_clock: Arc<EventClock>,
    /// The sending half of the live-input queue, once [`Player::midi_only`] has installed
    /// one and until [`Player::take_event_sender`] moves it to another thread.
    events: Option<EventSender>,
    /// Whether the source the engine is playing is a live-input one. Not the same question
    /// as `events.is_some()`, which goes false the moment a caller takes the sender.
    live_input: bool,
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
        let negotiated = backend.negotiate(device, requested)?;
        let (mut render, control) = build_sides(mode, negotiated)?;
        let stream = backend.open(device, negotiated, Box::new(move |output: &mut [f32]| render.render(output)))?;
        if stream.spec() != negotiated {
            return Err(HostError::UnsupportedSpec {
                requested: negotiated,
                reason: std::format!("the backend opened {} instead", stream.spec()),
            });
        }
        stream.play()?;
        Ok(Player {
            stream,
            spec: negotiated,
            mode,
            commands: control.commands,
            retired: control.retired,
            telemetry: control.telemetry,
            taps: control.taps,
            handles: control.handles,
            at_end: AtEnd::Continue,
            fade_frames: 0,
            master_volume: U0F16::MAX,
            module: None,
            scan: None,
            collected: 0,
            scopes: control.scopes,
            event_clock: control.event_clock,
            events: None,
            live_input: false,
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
        let snapshot = *self.telemetry.read();
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
        let at_end = self.at_end;
        let fade_frames = self.fade_frames;
        let master_volume = self.master_volume;
        let collected = self.collected;
        let requested_lead = self.event_clock.requested_lead_frames();
        let live_input = self.live_input;
        let mut rebuilt = Player::open(backend, device, requested, plan.mode)?;
        // Carried, not restarted: the count is what a caller reports as "modules dropped on
        // this thread since start-up", and a rebuild is not a fresh start-up.
        rebuilt.collected = collected;
        rebuilt.set_master_volume(master_volume)?;
        if fade_frames > 0 {
            rebuilt.set_fade_frames(fade_frames)?;
        }
        rebuilt.set_at_end(at_end)?;
        rebuilt.set_event_lead(requested_lead);
        if let Some(module) = module {
            rebuilt.install(module, plan.start, plan.cached_scan)?;
        }
        // A rebuilt engine gets a **new** live-input queue, because the old one's consumer
        // went into the retired engine. An [`EventSender`] a caller had already taken is
        // therefore dead after a rebuild and has to be taken again — which is the same
        // contract [`Player::take_scope_readers`] already has.
        if live_input {
            rebuilt.midi_only()?;
        }
        if plan.resume {
            rebuilt.play()?;
        }
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
        self.install(module, SeekKind::None, None)
    }

    /// [`Player::load`] for a module the caller has already decoded.
    pub fn load_module(&mut self, module: RtArc<Module>) -> Result<(), HostError> {
        self.install(module, SeekKind::None, None)
    }

    fn install(&mut self, module: RtArc<Module>, start: SeekKind, cached_scan: Option<Arc<ScannedSong>>) -> Result<(), HostError> {
        // A new sequencer's tick clock starts at frame zero, but the engine's musical clock
        // is monotonic and has been running since the stream opened. Starting the clock
        // where the engine actually is stops the engine burning ticks catching up.
        let frame = Frame(self.taps.source_frame.load(Ordering::Relaxed));
        // A seek asked for against the outgoing module means nothing to the incoming one.
        self.handles.seek.clear();
        let built = build_source(RtArc::clone(&module), self.spec.sample_rate_hz, start, frame, &self.handles, cached_scan)?;
        let command = HostCommand::Load { module: RtArc::clone(&module), source: built.source };
        self.commands.push(command).map_err(|_| HostError::ControlQueueFull)?;
        self.scan = Some(built.scanned);
        self.module = Some(module);
        self.live_input = false;
        self.events = None;
        Ok(())
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
    pub fn play(&mut self) -> Result<(), HostError> { self.queue(HostCommand::Play) }

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
        Ok(())
    }

    /// What the caller last chose to happen when the song has been heard through once.
    pub fn at_end(&self) -> AtEnd { self.at_end }

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
    /// The module stops playing. Merging the two, so a keyboard can be jammed over a
    /// playing module, is task E7's `SourceMux`; until then this is the honest half of it,
    /// and it is what the CLI's `--midi` and the web player's keyboard toggle install.
    /// [`Player::restore_module_source`] puts the module back.
    ///
    /// The transport has to be **running** for anything to sound: the musical clock is
    /// what a source is dispatched against, and it stops with the transport.
    pub fn midi_only(&mut self) -> Result<(), HostError> {
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
            .push(HostCommand::Load { module, source: Box::new(source) })
            .map_err(|_| HostError::ControlQueueFull)?;
        self.events = Some(EventSender::new(producer, Arc::clone(&self.event_clock)));
        self.live_input = true;
        Ok(())
    }

    /// Put the module's own sequencer back, ending live input.
    ///
    /// The scan is reused — nothing the timeline depends on changed — so this costs a
    /// sequencer, not a rescan, and the song restarts from the top of the order list.
    pub fn restore_module_source(&mut self) -> Result<(), HostError> {
        let module = self.module.clone().ok_or(HostError::NoModule)?;
        let cached_scan = self.scan.clone();
        self.install(module, SeekKind::None, cached_scan)
    }

    /// Whether the engine is playing a live-input source.
    pub const fn is_midi_only(&self) -> bool { self.live_input }

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
    pub fn telemetry(&mut self) -> &Snapshot { self.telemetry.read() }

    /// Sticky engine warnings, as of the last snapshot.
    pub fn warnings(&mut self) -> EngineWarnings {
        let snapshot = *self.telemetry.read();
        EngineWarnings {
            zero_advance_forced: snapshot.warnings.zero_advance_forced,
            event_limit_reached: snapshot.warnings.event_limit_reached,
            retired_module_dropped: snapshot.warnings.retired_module_dropped,
            unsupported_command: snapshot.warnings.unsupported_command,
            late_events: snapshot.warnings.late_events,
        }
    }

    /// One pass of the song in frames, from the scan.
    pub fn song_length(&self) -> Option<u64> { self.scan.as_ref().map(|scanned| scanned.timeline.end_frame()) }

    /// How far into the song the audio thread is, in frames — what a progress bar draws.
    pub fn song_frame(&mut self) -> u64 { self.telemetry.read().transport.song_frame }

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
    /// **Call this regularly.** It is the only place a retired module or sequencer is
    /// returned to the allocator.
    pub fn collect_garbage(&mut self) -> usize {
        while let Some(retired) = self.retired.pop() {
            drop(retired);
            self.collected = self.collected.saturating_add(1);
        }
        self.collected
    }

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

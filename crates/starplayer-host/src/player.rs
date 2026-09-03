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

use starplayer::core::{AtEnd, ChannelId, Command, Frame, U0F16};
use starplayer::engine::{EngineHandle, EngineWarnings, EventSource, MixerMode, RENDER_QUANTUM};
use starplayer::model::Module;
use starplayer::rt::{Arc as RtArc, Consumer, Producer, SnapshotPublisher, SnapshotReader, channel, snapshot_channel};
use starplayer::telemetry::{Snapshot, SongEnd, TelemetryReader};
use starplayer::{ScannedSong, recommended_voice_capacity};

use crate::backend::{AudioBackend, AudioSpec, HostError, Stream, StreamHealth};
use crate::engine::HostEngine;
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
    /// Frames handed to the backend so far. The clock the control-plane cadence is aligned
    /// to; `HostEngine::frame` cannot serve, because it counts frames *rendered* and so
    /// jumps a whole quantum at a time however few of them the host has taken.
    frames_emitted: u64,
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
        let running = self.engine.is_playing() && !self.transport.stop_is_pending();
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
            self.transport.take_fade();
            self.request_seek(SeekKind::Frame(0));
        }
        if self.transport.stop_has_landed() && self.send(Command::Stop) && self.transport.take_stop() {
            self.request_seek(SeekKind::Frame(0));
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
        self.taps.output_frame.store(self.engine.frame().0, Ordering::Relaxed);
        self.taps.playing.store(self.engine.is_playing() && !self.transport.stop_is_pending(), Ordering::Relaxed);
        self.taps.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        self.taps.blocks_rendered.fetch_add(1, Ordering::Relaxed);
        // A snapshot the caller is too slow to collect is dropped rather than queued, which
        // is the same bargain the engine's own telemetry channel strikes.
        let _ = self.forwarded.publish(self.last_snapshot);
    }
}

/// The control-side halves that [`RenderState`] does not take.
struct ControlSide {
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<Snapshot>,
    taps: Arc<PlayerTaps>,
    handles: SourceHandles,
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
    let (engine, mut control, telemetry) = HostEngine::build(mode, spec.sample_rate_hz)?;
    // Freeze the musical clock until the first Play. Without this a stream that is started
    // as soon as it is open plays the song silently while the caller is still deciding.
    control.send(Command::Stop).map_err(|_| HostError::ControlQueueFull)?;

    let (command_producer, command_consumer) = channel(HOST_COMMAND_CAPACITY);
    let (retired_producer, retired_consumer) = channel(RETIRED_CAPACITY);
    let (forwarded, snapshot_reader) = snapshot_channel(TELEMETRY_DEPTH, Snapshot::default());
    let taps = Arc::new(PlayerTaps::default());
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
        frames_emitted: 0,
    };
    let control_side =
        ControlSide { commands: command_producer, retired: retired_consumer, telemetry: snapshot_reader, taps, handles };
    Ok((render, control_side))
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
        let module = self.module.clone();
        let at_end = self.at_end;
        let fade_frames = self.fade_frames;
        let master_volume = self.master_volume;
        let mut reopened = Player::open(backend, device, requested, self.mode)?;
        reopened.set_master_volume(master_volume)?;
        if fade_frames > 0 {
            reopened.set_fade_frames(fade_frames)?;
        }
        reopened.set_at_end(at_end)?;
        if let Some(module) = module {
            reopened.install(module, SeekKind::None, None)?;
        }
        // The old stream — and the engine and module handles inside its callback — dies
        // here, on this thread.
        *self = reopened;
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

    /// Every device the backend can see. A convenience so a caller need not import the
    /// backend trait to print a list.
    pub fn devices(backend: &dyn AudioBackend) -> Vec<crate::backend::DeviceInfo> { backend.devices() }
}

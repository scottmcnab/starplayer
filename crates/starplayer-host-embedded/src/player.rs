//! [`EmbeddedPlayer`] — the `no_std` host: an engine, a transport, and the std host's
//! quantum-aligned control cadence, over `&mut [i16]`.
//!
//! # The two halves, and what crosses between them
//!
//! A [`ControlHalf`] is the **control** side: it holds the module, the scan, the telemetry
//! reader, the seek mailbox and the retirement ring. A [`RenderHalf`] is the **audio**
//! side: it holds the engine, its command handle and the transport, and it is what the
//! firmware's I2S DMA refill calls. The two are separate values, so the firmware can move
//! the render half into an interrupt or onto the other core and keep the control half in
//! an Embassy task.
//!
//! | | direction | mechanism |
//! |---|---|---|
//! | commands, and a freshly built module and source | control → audio | `HostCommand` over an SPSC ring of [`HOST_COMMAND_CAPACITY`] |
//! | seeks | control → audio | [`SeekMailbox`](crate::SeekMailbox), latest wins |
//! | retired modules and retired sources | audio → control | an SPSC ring of [`RETIRED_CAPACITY`]; **dropped there**, never here |
//! | telemetry, position, peak | audio → control | a snapshot channel of [`TELEMETRY_DEPTH`] and a handful of atomics |
//!
//! The three capacities are the std host's
//! ([`starplayer_host::HOST_COMMAND_CAPACITY`](https://docs.rs/starplayer-host) and its
//! neighbours) **by value, for the same reasons**: commands are sparse and the audio side
//! drains the whole ring at the top of every quantum; a backlog of sixteen retirements
//! means the control side has stopped collecting and the backlog is the useful signal; and
//! a snapshot ring keeps the oldest when it fills, so three is depth enough to absorb a
//! late reader without handing it stale state.
//!
//! # The cadence rule
//!
//! Commands are drained and the end of the song is armed only at multiples of
//! [`RENDER_QUANTUM`] **frames emitted**, never at device-block boundaries; the transport
//! is settled after each whole quantum. That is what makes design goal 3 hold through a
//! host: a device handing over 4096 frames takes the same decisions on the same frames as
//! one handing over 128, so a stop, a seek or an end-of-song fade lands on one frame
//! whatever the DMA buffer happens to be. Telemetry is published once per call, carrying
//! the freshest state the engine has rather than the one the first quantum decided from.

use alloc::boxed::Box;
use core::marker::PhantomData;

use starplayer::core::{AtEnd, ChannelId, Command, Frame, U0F16};
use starplayer::dsp::{Interpolate, Linear};
use starplayer::engine::{ChannelTable, Engine, EngineHandle, EngineSettings, EngineWarnings, EventSource, RENDER_QUANTUM};
use starplayer::mixer::{Dither, FixedOut, FixedPath, HostSample};
use starplayer::model::Module;
use starplayer::{ScannedSong, recommended_voice_capacity};
use starplayer_rt::atomic::{AtomicBool, AtomicU32, Ordering};
use starplayer_rt::{Arc, Consumer, Producer, SnapshotPublisher, SnapshotReader, channel, snapshot_channel};
use starplayer_telemetry::{Snapshot, SongEnd, TelemetryReader};

use crate::Error;
use crate::seqlock::SeqlockU64;
use crate::source::{SeekKind, SeekRequest, SourceHandles, build_source};
use crate::transport::{TRANSPORT_GAIN_UNITY, Transport};

/// Commands the control side may have outstanding before it starts getting refusals.
///
/// [`starplayer_host::HOST_COMMAND_CAPACITY`](https://docs.rs/starplayer-host)'s value, for
/// its reason: commands are sparse — load, seek, mute, master volume — and the audio side
/// drains the whole ring at the top of every render quantum.
pub const HOST_COMMAND_CAPACITY: usize = 64;

/// Retired modules and sources waiting to be dropped on the control side.
///
/// The std host's `RETIRED_CAPACITY`, for its reason: small on purpose, exactly as the
/// engine's own garbage channel is. A host with sixteen retirements outstanding has stopped
/// calling [`ControlHalf::collect_garbage`], and the backlog is the useful signal.
pub const RETIRED_CAPACITY: usize = 16;

/// Snapshots buffered on the way from the render half to the control half.
///
/// The std host's `TELEMETRY_DEPTH`, for its reason: the snapshot ring keeps the **oldest**
/// entries when it fills, so a deep one would hand a late reader the state at the start of
/// the backlog rather than the end.
pub const TELEMETRY_DEPTH: usize = 3;

/// Frames one call to [`RenderHalf::render`] walks before it drains the control plane
/// again — and therefore the answer to "how late can a seek or a stop be?".
///
/// 128 frames, which is 2.9 ms at 44.1 kHz, whatever the DMA buffer size is.
pub const CONTROL_CADENCE_FRAMES: usize = RENDER_QUANTUM;

/// Output channels. Stereo, fixed: this host exists to feed an I2S codec, and master-plan
/// decision 2 fixes the configuration at fixed path, `Linear`, i16 stereo.
pub const OUTPUT_CHANNELS: usize = 2;

/// Interleaved samples in one render quantum — the unit [`RenderHalf::render`] walks in.
const QUANTUM_SAMPLES: usize = RENDER_QUANTUM * OUTPUT_CHANNELS;

/// The one engine instantiation this host builds (master-plan decision 2).
type EmbeddedEngine<Interp> = Engine<FixedPath, Interp, FixedOut<i16, OUTPUT_CHANNELS>, Arc<Module>>;

/// The engine settings one module wants, at `sample_rate_hz`.
///
/// The sizing rule of `starplayer_offline`'s `render_with_kernel`: the channel table is the
/// module header's own width rather than [`ChannelTable::MAX_CHANNELS`], and the voice pool
/// is what this module's own processor asks for rather than `MAX_VOICE_CAPACITY`. A
/// persistent desktop host takes the maxima once and plays everything through them; a
/// device builds an engine per module and pays for what that module uses.
///
/// # What that costs, in bytes
///
/// Every allocation the engine makes happens in `Engine::with_settings`, and everything
/// else this host allocates happens in [`EmbeddedPlayer::open_empty`]. The total is linear
/// in the two counts, and `tests/render_allocation.rs` measures it rather than asserting
/// it — the figures below are that test's output on this instantiation (`FixedPath`, so an
/// accumulator frame is two `i32`; `FixedOut<i16, 2>`; `telemetry` on):
///
/// ```text
///   heap bytes = 27_800 + 184 × voice_capacity + 5_288 × channel_count
/// ```
///
/// So `REFLEX.S3M` (3 channels, 3 voices) costs 44 216 bytes, `PETRI.S3M` (8 and 8) costs
/// 71 576, and a 32-channel module with a 64-voice pool costs 209 kB — all before the
/// module's own PCM, which is what I2's flash-resident images exist to keep out of RAM.
///
/// Where it goes, and the three things worth knowing before writing a budget:
///
/// * **184 bytes per voice**, which is `size_of::<Voice>()` (176) plus the pool's
///   free-list link. About twenty of those bytes are the `PathFilter<f32>` that only the
///   float path reads: a `Voice` is not generic over the mix path, so a fixed-path build
///   carries the float filter's delay line and coefficients and never touches them
///   (research point 3). Removing it is a `starplayer-mixer` change and is out of scope
///   here.
/// * **5 288 bytes per channel**, of which about 2.1 kB is a **scope tap ring** — 1024
///   `i16` buckets plus two `Arc` headers — and 1 kB is the channel's own bus. The task
///   file assumed the taps exist only once `Engine::scope_readers` has been taken; they do
///   not. `Engine::with_settings` allocates one per channel whenever the `telemetry`
///   feature is on, and this host needs `telemetry` for the snapshot its cadence reads
///   `end_reached` from, so taking the readers or dropping them changes nothing. Giving
///   the taps their own feature would save `channel_count × 2.1 kB` on a device that draws
///   no scope; it is a follow-up.
/// * **27.8 kB fixed**, of which about 15.7 kB is two rings of `Snapshot` — the engine's
///   own telemetry channel and this host's forwarding one, three deep each, and a
///   `Snapshot` is 2 616 bytes because it carries 64 channels whatever the module has
///   (`starplayer_telemetry::MAX_CHANNELS`, fixed by M1-B6). The rest is the three
///   quantum-sized scratch buffers, the output ring and the five command and garbage
///   rings.
///
/// One more figure a firmware needs and this formula does not cover: a
/// [`RenderHalf<Linear>`] is **11 744 bytes by value**, because the engine's telemetry
/// publisher holds a working `Snapshot` inline and this host holds another. Move it into a
/// `static` or a `Box` rather than down a call chain.
pub fn settings_for(module: &Module, sample_rate_hz: u32) -> EngineSettings {
    EngineSettings {
        sample_rate_hz,
        channel_count: (module.header().channel_count as usize).min(ChannelTable::MAX_CHANNELS),
        voice_capacity: recommended_voice_capacity(module).max(1),
        ..EngineSettings::default()
    }
}

/// One instruction from the control side to the render half.
enum HostCommand {
    /// Ramp up and start the musical clock.
    Play,
    /// Ramp down and stop it when the ramp lands.
    Stop,
    /// A module and the source built for it, both already scanned and allocated on the
    /// control side. Nothing here is constructed in the render half and nothing is dropped
    /// there.
    Load { module: Arc<Module>, source: Box<dyn EventSource> },
    /// What happens at the loop point, and how long the fade is when there is one.
    SetAtEnd { at_end: AtEnd, fade_frames: u32 },
    /// Pass straight through to the engine.
    Engine(Command<Arc<Module>>),
}

/// Something the render half finished with and may not drop.
///
/// Nothing ever reads either payload: being dropped on the *control* side is the entire
/// value of this type, and the `expect` says so rather than leaving a reader to wonder.
#[expect(dead_code, reason = "the payload exists to be dropped on the control side, never to be read")]
enum Retired {
    Module(Arc<Module>),
    Source(Box<dyn EventSource>),
}

/// The lossy readings the control half takes without waiting for a snapshot.
///
/// Every field is written once per render call and read whenever the control side asks; a
/// reading one block old costs a progress bar nothing (architecture §9).
#[derive(Debug, Default)]
struct Taps {
    source_frame: SeqlockU64,
    output_frame: SeqlockU64,
    playing: AtomicBool,
    /// Whether the song fade is running, which a screen shows.
    fading: AtomicBool,
    /// The last block's peak, as an absolute `i16` magnitude.
    peak: AtomicU32,
    /// Blocks and refusals since the player opened. `u32` rather than a second pair of
    /// seqlocks: these are diagnostics, and one that wrapped after four billion DMA blocks
    /// — five months at 128 frames and 44.1 kHz — would still be telling the truth about
    /// whether the refill is running.
    blocks_rendered: AtomicU32,
    commands_rejected: AtomicU32,
}

/// The audio side: everything the DMA refill owns.
///
/// Nothing reachable from [`RenderHalf::render`] allocates, locks, or can panic
/// (design goal 5).
pub struct RenderHalf<Interp: Interpolate = Linear> {
    engine: EmbeddedEngine<Interp>,
    control: EngineHandle<Arc<Module>>,
    telemetry: TelemetryReader,
    forwarded: SnapshotPublisher<Snapshot>,
    commands: Consumer<HostCommand>,
    retired: Producer<Retired>,
    handles: SourceHandles,
    transport: Transport,
    taps: Arc<Taps>,
    last_snapshot: Snapshot,
    /// A typed [`Command::Stop`] the engine has been sent and has not acted on yet.
    ///
    /// The engine applies its command ring inside its own render, so for one whole quantum
    /// after a stop is sent `engine.is_playing()` still says yes. Without this latch
    /// [`RenderHalf::arm_end_of_song`] fires a second time over a song that has already
    /// finished — re-arming the very fade that just landed, which leaves the transport
    /// reporting a fade over silence.
    awaiting_engine_stop: bool,
    /// Interleaved samples handed to the device so far.
    ///
    /// The clock the control cadence is aligned to, counted in **samples** rather than
    /// frames so that a device block which is not a whole number of frames cannot slide the
    /// cadence off a frame boundary. `Engine::frame` cannot serve: it counts frames
    /// *rendered*, and so jumps a whole quantum at a time however few of them the device has
    /// taken.
    samples_emitted: u64,
    /// The transport gain in force for the frame currently being emitted, held across calls
    /// so that a block ending mid-frame does not advance the ramp twice for it.
    frame_gain: i32,
}

impl<Interp: Interpolate> RenderHalf<Interp> {
    /// Fill `output` with interleaved stereo `i16`, and report the block's peak magnitude.
    ///
    /// Any length is accepted, including one that is not a whole number of frames: the
    /// engine's output ring carries the remainder of a quantum between calls and this
    /// carries the remainder of a *frame*, so the stream is identical at every block size.
    ///
    /// # Why it walks the block rather than rendering it whole
    ///
    /// The engine already makes *audio* independent of the block size — that is what its
    /// output ring is for. What is not automatically independent is everything the host
    /// itself decides between renders: when the control plane is drained, and when the end
    /// of the song arms a fade or a stop. Decide those once per call and a device handing
    /// over 4096 frames arms the fade up to 93 ms later than one handing over 128 — and the
    /// two renders of the same module differ, byte for byte.
    pub fn render(&mut self, output: &mut [i16]) -> i16 {
        let mut peak = 0i32;
        let mut written = 0usize;
        while written < output.len() {
            let offset_in_quantum = (self.samples_emitted % QUANTUM_SAMPLES as u64) as usize;
            if offset_in_quantum == 0 {
                self.drain_commands();
                self.arm_end_of_song();
            }
            let take = QUANTUM_SAMPLES.saturating_sub(offset_in_quantum).min(output.len().saturating_sub(written));
            let end = written.saturating_add(take);
            let Some(chunk) = output.get_mut(written..end) else { break };
            self.engine.render(chunk);
            peak = peak.max(apply_transport(chunk, self.samples_emitted, &mut self.transport, &mut self.frame_gain));
            self.samples_emitted = self.samples_emitted.saturating_add(take as u64);
            if self.samples_emitted.is_multiple_of(QUANTUM_SAMPLES as u64) {
                self.settle_transport();
            }
            written = end;
        }
        self.retire_engine_garbage();
        self.publish(peak);
        peak.min(i16::MAX as i32) as i16
    }

    /// Frames handed to the device since the stream opened — the clock the cadence aligns
    /// to. A partial trailing frame is not counted until it is complete.
    pub const fn frames_emitted(&self) -> u64 { self.samples_emitted / OUTPUT_CHANNELS as u64 }

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

    /// The song has been heard through once. What that means depends on *how* it ends, and
    /// only two cases do anything at all:
    ///
    /// * it **loops** — something in the module jumps back — and the caller asked for a
    ///   fade, so the second pass plays under a fading transport;
    /// * it **ends** — the order list ran out, or a stop marker fired — and the caller
    ///   asked for anything but Continue. There is no second pass to fade into, so the
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
    /// ring like any other, rather than calling `free()` in the DMA refill.
    fn send(&mut self, command: Command<Arc<Module>>) -> bool {
        match self.control.send(command) {
            Ok(()) => true,
            Err(rejected) => {
                self.taps.commands_rejected.fetch_add(1, Ordering::Relaxed);
                if let Command::LoadModule(module) = rejected {
                    self.retire(Retired::Module(module));
                }
                false
            }
        }
    }

    /// Move every retired module handle out of the engine's garbage channel and on towards
    /// the control side. It is **moved**, never dropped: a `free()` here is the stall the
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

    /// One publication per call, carrying the newest state the engine has.
    ///
    /// Not per quantum: a snapshot ring keeps the **oldest** entries when it fills, so a
    /// call that published twenty snapshots would leave the reader holding the one from the
    /// *start* of the block. The end-of-song decision above is taken on a fixed cadence
    /// because it has to land on the same frame at every block size; what a screen reads
    /// should simply be as new as possible.
    fn publish(&mut self, peak: i32) {
        self.taps.source_frame.store(self.engine.source_frame().0);
        self.taps.output_frame.store(self.engine.frame().0);
        self.taps.playing.store(self.engine.is_playing() && !self.transport.stop_is_pending(), Ordering::Relaxed);
        self.taps.fading.store(self.transport.is_fading(), Ordering::Relaxed);
        self.taps.peak.store(peak.clamp(0, i16::MAX as i32) as u32, Ordering::Relaxed);
        self.taps.blocks_rendered.fetch_add(1, Ordering::Relaxed);
        // A snapshot the control side is too slow to collect is dropped rather than queued,
        // which is the same bargain the engine's own telemetry channel strikes.
        let _ = self.forwarded.publish(*self.telemetry.read());
    }
}

/// Apply the transport's per-frame gain to `chunk` in place, and report its peak magnitude.
///
/// `first_sample` is the absolute interleaved index of `chunk[0]`, so the gain advances on
/// exactly the frames it would have advanced on had the whole stream arrived in one block.
///
/// The arithmetic is the std host's `HostEngine::render_chunk` fixed arm, unchanged: widen,
/// scale by the gain over [`TRANSPORT_GAIN_UNITY`], and clamp back through
/// `<i16 as HostSample>::from_i16_scale` — which at `OutputDepth::I16` is what
/// `starplayer_host::quantize_fixed_sample` reduces to. Byte equality between the two hosts
/// depends on this staying the same expression.
fn apply_transport(chunk: &mut [i16], first_sample: u64, transport: &mut Transport, frame_gain: &mut i32) -> i32 {
    let mut dither = Dither::OFF;
    let mut peak = 0i32;
    for (offset, sample) in chunk.iter_mut().enumerate() {
        if (first_sample.wrapping_add(offset as u64)).is_multiple_of(OUTPUT_CHANNELS as u64) {
            *frame_gain = transport.advance();
        }
        let gained = (*sample as i64 * *frame_gain as i64) / TRANSPORT_GAIN_UNITY as i64;
        let quantized = <i16 as HostSample>::from_i16_scale(gained as i32, &mut dither);
        *sample = quantized;
        peak = peak.max(quantized.unsigned_abs() as i32);
    }
    peak
}

/// The control side: everything the firmware's own task owns.
///
/// Everything here allocates freely — it scans songs and builds sequencers — because none
/// of it runs in the DMA refill.
pub struct ControlHalf {
    commands: Producer<HostCommand>,
    retired: Consumer<Retired>,
    telemetry: SnapshotReader<Snapshot>,
    taps: Arc<Taps>,
    handles: SourceHandles,
    sample_rate_hz: u32,
    at_end: AtEnd,
    fade_frames: u32,
    master_volume: U0F16,
    module: Option<Arc<Module>>,
    scanned: Option<ScannedSong>,
    collected: usize,
}

impl ControlHalf {
    /// Start, or resume, playing. Glides up over 64 frames, so it does not click.
    pub fn play(&mut self) -> Result<(), Error> { self.queue(HostCommand::Play) }

    /// Stop. Glides down over 64 frames and stops the musical clock when the ramp lands, so
    /// it does not click either.
    pub fn stop(&mut self) -> Result<(), Error> { self.queue(HostCommand::Stop) }

    /// Scan `module`, build its sequencer, and send both to the render half.
    ///
    /// Everything expensive happens here: the scan, the playback sequencer, its per-channel
    /// state. Only a `Box` and an `Arc` cross. A failed load leaves whatever was playing
    /// playing.
    pub fn load(&mut self, module: Arc<Module>) -> Result<(), Error> {
        // A new sequencer's tick clock starts at frame zero, but the engine's musical clock
        // is monotonic and has been running since the player opened. Starting the clock
        // where the engine actually is stops the engine burning ticks catching up.
        let frame = Frame(self.taps.source_frame.load());
        // A seek asked for against the outgoing module means nothing to the incoming one.
        self.handles.seek.clear();
        let built = build_source(Arc::clone(&module), self.sample_rate_hz, SeekKind::None, frame, &self.handles)?;
        self.queue(HostCommand::Load { module: Arc::clone(&module), source: built.source })?;
        self.scanned = Some(built.scanned);
        self.module = Some(module);
        Ok(())
    }

    /// Jump to an order-list index.
    pub fn seek_order(&mut self, order: u16) -> Result<(), Error> { self.request_seek(SeekKind::Order(order)) }

    /// Jump to a row of the pattern already playing.
    pub fn seek_row(&mut self, row: u16) -> Result<(), Error> { self.request_seek(SeekKind::Row(row)) }

    /// Jump to an elapsed position in the song, in frames.
    pub fn seek_frame(&mut self, song_frame: u64) -> Result<(), Error> { self.request_seek(SeekKind::Frame(song_frame)) }

    fn request_seek(&mut self, kind: SeekKind) -> Result<(), Error> {
        if self.module.is_none() {
            return Err(Error::NoModule);
        }
        self.handles.seek.request(SeekRequest { kind, frame: Frame(self.taps.source_frame.load()) });
        Ok(())
    }

    /// Set the master volume, applied by the master bus before the limiter.
    pub fn set_master_volume(&mut self, volume: U0F16) -> Result<(), Error> {
        self.master_volume = volume;
        self.queue(HostCommand::Engine(Command::SetMasterVolume(volume)))
    }

    /// What the control side last asked for.
    pub const fn master_volume(&self) -> U0F16 { self.master_volume }

    /// Mute or unmute one channel. A muted channel's voices still render, into scratch, so
    /// its state advances exactly as if it were audible.
    pub fn mute(&mut self, channel: ChannelId, muted: bool) -> Result<(), Error> {
        self.queue(HostCommand::Engine(Command::MuteChannel { channel, muted }))
    }

    /// Choose what happens when the song has been heard through once.
    pub fn set_at_end(&mut self, at_end: AtEnd) -> Result<(), Error> {
        self.at_end = at_end;
        self.handles.at_end.set(at_end);
        self.queue(HostCommand::SetAtEnd { at_end, fade_frames: 0 })
    }

    /// What the caller last chose to happen at the end of the song.
    pub const fn at_end(&self) -> AtEnd { self.at_end }

    /// How long the fade lasts, in frames. Zero leaves the current length alone.
    pub fn set_fade_frames(&mut self, fade_frames: u32) -> Result<(), Error> {
        self.fade_frames = fade_frames;
        self.queue(HostCommand::SetAtEnd { at_end: self.at_end, fade_frames })
    }

    /// The most recent coherent snapshot the render half published: transport position,
    /// per-channel note, effect and VU.
    ///
    /// `&mut self` rather than `&self` because
    /// [`SnapshotReader::read`](starplayer_rt::SnapshotReader::read) consumes from a ring to
    /// get there — the same signature `starplayer_host::Player::telemetry` has.
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
            retired_insert_dropped: snapshot.warnings.retired_insert_dropped,
        }
    }

    /// Whether the transport is running: the musical clock is going and no stop is queued.
    pub fn is_playing(&self) -> bool { self.taps.playing.load(Ordering::Relaxed) }

    /// Whether the song fade is running — the transport is playing a pass it will fade out
    /// of, rather than merely being quiet.
    pub fn is_fading(&self) -> bool { self.taps.fading.load(Ordering::Relaxed) }

    /// How far into the song the render half is, in frames — what a progress bar draws.
    pub fn song_frame(&mut self) -> u64 { self.telemetry.read().transport.song_frame }

    /// One pass of the song in frames, from the scan.
    pub fn song_length(&self) -> Option<u64> { self.scanned.as_ref().map(|scanned| scanned.timeline.end_frame()) }

    /// The module currently loaded.
    pub const fn module(&self) -> Option<&Arc<Module>> { self.module.as_ref() }

    /// The scan the playing sequencer was built from: the timeline, and the quirks it was
    /// measured under.
    pub const fn scan(&self) -> Option<&ScannedSong> { self.scanned.as_ref() }

    /// The musical clock, in frames since the player opened.
    pub fn source_frame(&self) -> Frame { Frame(self.taps.source_frame.load()) }

    /// The output clock, which never stops.
    pub fn output_frame(&self) -> Frame { Frame(self.taps.output_frame.load()) }

    /// The last block's peak magnitude, `0 ..= 32767`. A lossy tap: a stale reading costs a
    /// meter nothing.
    pub fn peak(&self) -> i16 { self.taps.peak.load(Ordering::Relaxed).min(i16::MAX as u32) as i16 }

    /// Blocks the device has asked for since the player opened. Zero after a second of
    /// playing means the DMA refill is not running.
    pub fn blocks_rendered(&self) -> u32 { self.taps.blocks_rendered.load(Ordering::Relaxed) }

    /// Commands the render half could not deliver, because a ring was full.
    pub fn commands_rejected(&self) -> u32 { self.taps.commands_rejected.load(Ordering::Relaxed) }

    /// Drop everything the render half has finished with, and report the running total.
    ///
    /// **Call this regularly.** It is the only place a retired module or sequencer is
    /// returned to the allocator; the render half only ever *moves* them onto the ring.
    pub fn collect_garbage(&mut self) -> usize {
        while let Some(retired) = self.retired.pop() {
            drop(retired);
            self.collected = self.collected.saturating_add(1);
        }
        self.collected
    }

    /// How many retirements are waiting for [`ControlHalf::collect_garbage`].
    pub fn pending_garbage(&self) -> usize { self.retired.len() }

    /// The seek the render half has not consumed yet, for a caller that wants to see a
    /// rewind queued rather than infer it from where the song restarts.
    pub fn pending_seek(&self) -> SeekRequest { self.handles.seek.peek() }

    fn queue(&mut self, command: HostCommand) -> Result<(), Error> {
        self.commands.push(command).map_err(|_| Error::CommandQueueFull)
    }
}

/// Every module the render half finished with is dropped here, including the ones still in
/// flight when the control half goes away.
impl Drop for ControlHalf {
    fn drop(&mut self) {
        while self.retired.pop().is_some() {}
    }
}

/// The `no_std` player: one engine, one module at a time, split into the two halves above.
///
/// A constructor rather than a live object — [`EmbeddedPlayer::open`] hands back the halves
/// and keeps nothing — because the two halves are meant to be moved apart immediately, and
/// a value that owned both would only be a thing to take them out of.
///
/// `Interp` selects the resampling kernel as a **type parameter**, so the inner loop
/// contains no branch on it (architecture §7.1). [`Linear`] is the default and the kernel
/// the goldens are hashed on.
pub struct EmbeddedPlayer<Interp: Interpolate = Linear> {
    interpolator: PhantomData<Interp>,
}

impl<Interp: Interpolate> EmbeddedPlayer<Interp> {
    /// Build an engine sized for `module` at `sample_rate_hz`, load it, and hand back the
    /// two halves.
    ///
    /// The player comes back **stopped and silent** — the musical clock is frozen and the
    /// transport is at zero — so a DMA stream may be started immediately without the song
    /// advancing under it. Nothing sounds until [`ControlHalf::play`].
    pub fn open(module: Arc<Module>, sample_rate_hz: u32) -> Result<(RenderHalf<Interp>, ControlHalf), Error> {
        let settings = settings_for(&module, sample_rate_hz);
        let (render, mut control) = EmbeddedPlayer::<Interp>::open_empty(sample_rate_hz, settings)?;
        control.load(module)?;
        Ok((render, control))
    }

    /// [`EmbeddedPlayer::open`] for a host that has not got a module yet — I6's upload path,
    /// and any firmware that wants its I2S stream running before the first module arrives.
    ///
    /// `settings` is the caller's, because without a module there is nothing to derive them
    /// from; [`settings_for`] is what a caller with one uses.
    pub fn open_empty(sample_rate_hz: u32, settings: EngineSettings) -> Result<(RenderHalf<Interp>, ControlHalf), Error> {
        let settings = EngineSettings { sample_rate_hz, ..settings };
        let mut engine = EmbeddedEngine::<Interp>::with_settings(settings);
        let mut control = engine.take_control().ok_or(Error::EngineHandleUnavailable)?;
        let telemetry = engine.telemetry_reader().ok_or(Error::EngineHandleUnavailable)?;
        // Freeze the musical clock until the first Play. Without this a stream that is
        // started as soon as it is open plays the song silently while the firmware is still
        // deciding.
        control.send(Command::Stop).map_err(|_| Error::CommandQueueFull)?;

        let (command_producer, command_consumer) = channel(HOST_COMMAND_CAPACITY);
        let (retired_producer, retired_consumer) = channel(RETIRED_CAPACITY);
        let (forwarded, snapshot_reader) = snapshot_channel(TELEMETRY_DEPTH, Snapshot::default());
        let taps = Arc::new(Taps::default());
        // Made once, for the life of the player, and shared. A pair made per module would
        // leave the render half holding the last handle to the outgoing one on every load.
        let handles = SourceHandles::new(AtEnd::Continue);

        let render = RenderHalf {
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
            samples_emitted: 0,
            frame_gain: 0,
        };
        let control_half = ControlHalf {
            commands: command_producer,
            retired: retired_consumer,
            telemetry: snapshot_reader,
            taps,
            handles,
            sample_rate_hz,
            at_end: AtEnd::Continue,
            fade_frames: 0,
            master_volume: U0F16::MAX,
            module: None,
            scanned: None,
            collected: 0,
        };
        Ok((render, control_half))
    }
}

//! Browser AudioWorklet host for the real StarPlayer engine.
//!
//! The worklet owns this wasm instance. A second, page-side wasm instance validates
//! files and supplies metadata without touching the render instance. Once validation
//! succeeds the original `ArrayBuffer` is transferred here; activation builds the
//! `Arc<Module>` and format-native sequencer outside `process()`, then hands the module through the
//! engine's typed command ring. Every buffer used by `process()` is allocated by `init`.
//!
//! # What is browser-specific, and what is not
//!
//! Everything true of *a host* rather than of *a browser* lives in `starplayer-host`: the
//! engine arms, the transport ramp, the song fade, the seek mailbox, the output-depth
//! post-stage, module activation and the retirement of whatever it replaces. This crate
//! drives a [`Player`] over a [`WorkletBackend`], and keeps only what the browser really
//! imposes — the wire command decoding, the `SharedArrayBuffer` and `postMessage`
//! transports, the flat scope window a JavaScript view can address, the planar output buffer
//! the `AudioWorkletProcessor` copies from, and the heap pre-reservation that keeps
//! `WebAssembly.Memory` from growing under a live callback.
//!
//! [`backend`] answers task D9's research point 1 in full — why a *pushed* worklet fits a
//! trait written for a *pulling* cpal, and why there is no second trait.
//!
//! # Why the module is decoded twice
//!
//! The two instances do not share memory, so an `Arc<Module>` built on the page cannot be
//! handed to the worklet — only bytes can cross, and they cross once, as a transferred
//! `ArrayBuffer`. Each instance therefore runs the facade's native format autodetection on
//! those bytes.
//! That is milliseconds of work and a second copy of the module, and it buys two things
//! worth more than either: an untrusted file is rejected on the page, before it can reach
//! the live audio graph at all, and the pattern view's `PatternCell` decoding runs on the
//! page thread rather than in the realm that has to hit a 2.7 ms deadline.
//!
//! The alternative — one instance with shared wasm memory — needs the `atomics` target
//! feature, which in turn needs a `std` rebuilt with `-Zbuild-std` on a nightly
//! toolchain, and would put the whole loader inside the audio realm. Rejected for a
//! stable toolchain and a clean realm boundary.

#![deny(unsafe_code)]

pub mod backend;
mod command;

use core::cell::RefCell;
use std::boxed::Box;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use backend::WorkletBackend;
use command::{CommandRing, WireCommand};
use starplayer::core::{AtEnd, ChannelId, U0F16, PlaybackIdentity, SessionId, BuildGeneration, InteractiveAction, InteractiveCommand, LiveDestination, InstrumentId};
use starplayer::dsp::{InsertKind, ParamId};
use starplayer::engine::{InsertTarget, MixerMode, RENDER_QUANTUM};
use starplayer::model::{Module, ModuleFormat};
use starplayer::rt::{Arc, TAP_BUCKET_FRAMES, TapReader};
use starplayer::telemetry::{Snapshot, SongEnd};
use starplayer_host::{AudioSpec, HostError, Player, message_to_event};

pub use command::COMMAND_RING_CAPACITY;

/// Frames the wasm-owned planar buffer can expose in one call.
pub const MAX_FRAMES_PER_CALL: usize = 1024;
/// The largest channel count the browser player exposes.
pub const MAX_OUTPUT_CHANNELS: usize = 2;

/// The transient reservation forces wasm pages to be committed before any view is taken.
/// It is returned to the allocator immediately and then reused by module activation.
///
/// Sized for the *host*, not for a module: the engine is built at
/// [`MAX_VOICE_CAPACITY`](starplayer::MAX_VOICE_CAPACITY) voices (including sixteen jam
/// slots) and 64 channels whatever is loaded, and 16 MiB covers that several times over.
/// Task D9 measured it again after the [`Player`] retrofit — see that task's research
/// resolution.
const HEAP_RESERVE_BYTES: usize = 16 * 1024 * 1024;

/// The largest total PCM frame count a load-time enhancer rebuild may produce. The IT
/// loader's own `decoded_pcm_budget` is load-time only and a rebuild bypasses it entirely,
/// so the host enforces its own ceiling here: a `sinc4x` request that would cross it is
/// dropped to `sinc2x`, and a `sinc2x` request that would still cross it is dropped to
/// identity (M10-W4 deliverable 1). See [`build_enhancement`].
const ENHANCE_FRAME_BUDGET: usize = 16_000_000;

/// [`CATALOGUE`](starplayer::enhance::CATALOGUE)'s bit for the sinc-upsampler checkbox.
/// Reused in [`Host::last_applied_enhancement_flags`] for whichever factor actually ran —
/// `sinc2x` has no bit of its own, so a budget fallback still reports through this bit,
/// and [`Host::last_enhancement_factor`] is what tells the two apart.
const ENHANCE_FLAG_UPSAMPLE: u32 = 1 << 0;
/// [`CATALOGUE`](starplayer::enhance::CATALOGUE)'s bit for the loop-seam smoother
/// checkbox. Named only in this crate's tests since M10-K5c made `build_enhancement` walk
/// the catalogue instead of naming bits, which is the point: the loop smoother is no
/// longer special to the host, only to the tests that check it arrives.
#[cfg(test)]
const ENHANCE_FLAG_LOOP: u32 = 1 << 1;
/// The catalogue id of the enhancer [`ENHANCE_FRAME_BUDGET`] applies to. It is the only
/// entry [`build_enhancement`] treats specially, because it is the only one that changes
/// how much memory the rebuild costs.
const ENHANCE_UPSAMPLE_ID: &str = "sinc4x";
/// The id the frame budget falls back to before falling back to nothing.
const ENHANCE_HALF_UPSAMPLE_ID: &str = "sinc2x";

/// Fraction of the held master peak that survives one quantum: about a 200 ms fall from
/// full scale to silence at 48 kHz, which reads well on a bar.
const MASTER_PEAK_DECAY_PER_QUANTUM: f32 = 0.94;

// Wire opcodes. Kept in one obvious block beside `ring.js`'s matching constants.
const OPCODE_PLAY: u8 = 1;
const OPCODE_STOP: u8 = 2;
const OPCODE_SEEK_ORDER: u8 = 3;
const OPCODE_SEEK_ROW: u8 = 4;
const OPCODE_MASTER_VOLUME: u8 = 5;
const OPCODE_MUTE_CHANNEL: u8 = 6;
const OPCODE_SET_MIXER_MODE: u8 = 7;
/// Seek to an elapsed position in the song. `argument` is a song frame.
const OPCODE_SEEK_FRAME: u8 = 8;
/// What to do at the detected loop point: `argument` 0 fade out, 1 continue, 2 stop;
/// `extra` is the fade length in frames.
const OPCODE_AT_END: u8 = 9;
/// One live MIDI channel-voice message — Web MIDI, or the page's tracker keyboard (task
/// E6). `argument` packs the three bytes a browser already hands over framed: status in
/// bits 0–7, `data1` in 8–15, `data2` in 16–23. `extra` is unused.
///
/// It rides the ordinary command ring rather than a second one because it *is* an ordinary
/// command: bounded, decoded between render quanta, and turned into one allocation-free
/// push onto the player's live-input queue. What it is not is the *install* — building the
/// instrument rack allocates, so that is [`exports::set_midi_input`], called from a worklet
/// message task exactly as `set_mixer_mode` is.
const OPCODE_MIDI_EVENT: u8 = 10;
/// Change one parameter of an already-installed insert effect (M7-H7). `argument` packs
/// the target in bits 0–7, the slot in bits 8–11 and the [`ParamId`] in bits 12–19;
/// `extra` is the new value, in the parameter's own fixed unit, reinterpreted as `i32`.
///
/// Rides the ordinary command ring for the same reason [`OPCODE_MIDI_EVENT`] does: it is
/// bounded, decoded between render quanta, and turned into one allocation-free
/// `Player::set_insert_param` call. What it is not is the *install* — building an effect
/// allocates its delay lines, so that is [`exports::install_insert`], called from a
/// worklet message task exactly as [`exports::set_midi_input`] is.
const OPCODE_INSERT_PARAM: u8 = 11;
/// Skip, or stop skipping, an already-installed insert effect. `argument` packs the target
/// and slot exactly as [`OPCODE_INSERT_PARAM`] does; `extra` is `0` or `1`.
const OPCODE_INSERT_BYPASS: u8 = 12;

/// The one reserved value of an insert wire target's low byte that does not name a
/// channel: the master bus. Channel indices are `0..64` ([`ChannelTable::MAX_CHANNELS`](starplayer::engine::ChannelTable::MAX_CHANNELS)),
/// so this sits far above any real one and still fits the 8-bit field
/// [`OPCODE_INSERT_PARAM`] and [`OPCODE_INSERT_BYPASS`] pack it into.
const INSERT_TARGET_MASTER: u32 = 0xFF;

/// Decode an insert wire target's low byte into an [`InsertTarget`].
const fn insert_target_from_wire(value: u32) -> InsertTarget {
    if value == INSERT_TARGET_MASTER { InsertTarget::Master } else { InsertTarget::Channel(ChannelId(value as u16)) }
}

/// The wire spelling of [`InsertKind`]: its position in [`InsertKind::ALL`], which
/// [`exports::effects_json`] also reports so the page never has to guess the numbering.
fn insert_kind_from_wire(value: u32) -> Option<InsertKind> { InsertKind::ALL.get(value as usize).copied() }

/// Wire spelling of [`AtEnd`], chosen so the page's default repeat-off value is zero.
const AT_END_FADE_OUT: u32 = 0;
const AT_END_CONTINUE: u32 = 1;
const AT_END_STOP: u32 = 2;

/// Layout exported to the worklet, then copied coherently to the SAB telemetry block.
const TELEMETRY_HEADER_WORDS: usize = 22;
const TELEMETRY_CHANNEL_WORDS: usize = 8;
const TELEMETRY_CHANNELS: usize = 64;
const TELEMETRY_LIVE_OFFSET: usize = TELEMETRY_HEADER_WORDS + TELEMETRY_CHANNELS * TELEMETRY_CHANNEL_WORDS;
const TELEMETRY_WORDS: usize = TELEMETRY_LIVE_OFFSET + 3;

/// Scope taps (architecture §9(b)): the newest window of each channel's tap ring, copied
/// out of the engine's rings into one contiguous block the worklet can view.
///
/// # Why a window rather than the whole ring
///
/// The engine's rings are one `Arc<[AtomicI16]>` per channel, so they are neither
/// contiguous nor addressable from JavaScript. The worklet needs one pointer, and it has
/// to *copy* whatever it publishes into the `SharedArrayBuffer` anyway — the render
/// instance's `WebAssembly.Memory` is not shared with the page (see this module's
/// preamble). So the host keeps its own flat block and refreshes it from the readers.
///
/// 256 buckets is 1024 output frames, ~21 ms at 48 kHz: comfortably more than one
/// animation frame, and a trace wide enough to fill a 128-pixel canvas twice over. The
/// whole block is 64 × 256 × 2 bytes = 32 KiB, allocated once with everything else.
const SCOPE_CHANNELS: usize = TELEMETRY_CHANNELS;
const SCOPE_WINDOW_BUCKETS: usize = 256;
const SCOPE_VALUES: usize = SCOPE_CHANNELS * SCOPE_WINDOW_BUCKETS;

/// Quanta between refreshes of the scope block.
///
/// A refresh copies 256 buckets per *active* channel, and four quanta is 128 new buckets —
/// half the window, so nothing is ever missed — at about 94 Hz on a 48 kHz context. That
/// is faster than any display refreshes and an eighth of the copying a per-quantum refresh
/// would do inside `process()`.
const SCOPE_REFRESH_QUANTA: u64 = 4;

/// Decode `bytes` with the web player's own loading preferences, then apply
/// `enhancement_flags` as a load-time [`Module::enhanced`] rebuild (M10-W4). Returns the
/// finished module alongside the flags actually applied and the upsample factor that ran
/// (1 for none), which may be smaller than what `enhancement_flags` asked for — see
/// [`build_enhancement`].
///
/// [`Player::load`] would do the autodetection, but not the MOD stereo-separation option:
/// that is a *player* preference the page exposes, so the decode happens here and the
/// finished module goes to [`Player::load_module`].
fn decode(bytes: &[u8], headphone_friendly_mod_panning: bool, enhancement_flags: u32, rate_ceiling_hz: u32) -> Result<(Module, u32, u32), starplayer::core::Error> {
    let module = match starplayer::probe(bytes) {
        Some(ModuleFormat::Mod) => {
            let options = starplayer::mod_file::LoadOptions {
                stereo_separation: starplayer::mod_file::StereoSeparation::percent(if headphone_friendly_mod_panning { 60 } else { 100 }),
            };
            if starplayer::mod_file::probe(bytes) { starplayer::mod_file::load_with_options(bytes, options) }
            else { starplayer::mod_file::soundtracker::load_with_options(bytes, options) }
        },
        Some(ModuleFormat::S3m) => starplayer::s3m::load(bytes),
        Some(ModuleFormat::Mtm) => starplayer::mtm::load(bytes),
        Some(ModuleFormat::Xm) => starplayer::xm::load(bytes),
        Some(ModuleFormat::It) => starplayer::it::load(bytes),
        _ => Err(starplayer::core::Error::BadMagic),
    }?;
    if enhancement_flags == 0 {
        return Ok((module, 0, 1));
    }
    let (chain, applied_flags, factor) = build_enhancement(enhancement_flags, module.pcm().len(), rate_ceiling_hz);
    let module = match chain {
        Some(chain) => module.enhanced(&chain)?,
        None => module,
    };
    Ok((module, applied_flags, factor))
}

/// Build the enhancer chain `enhancement_flags` (the same bits [`exports::enhancements_json`]
/// describes) asks for, downgrading a requested 4x upsample to 2x and then to identity if
/// `source_frames × factor` would cross [`ENHANCE_FRAME_BUDGET`] — a rebuild bypasses the
/// IT loader's own load-time budget entirely, so nothing else limits it. `rate_ceiling_hz`
/// is passed straight through to the upsampler, exactly as `starplayer::enhance::from_flags`
/// would.
///
/// The walk is over `CATALOGUE` itself, in **catalogue order**, which since M10-K5c is the
/// order the stages have to run in rather than the order their bits were assigned. So an
/// enhancer added to that table appears here — and in the page's checkboxes, and in the
/// CLI's `--enhance` — without this function changing. The upsampler is the one entry with
/// a special case, and only because it is the one entry that decides how much memory the
/// rebuild costs.
///
/// Returns the chain (`None` for the identity — no enhancer ran at all), the flags actually
/// engaged (`sinc2x` reports through [`ENHANCE_FLAG_UPSAMPLE`], the same bit `sinc4x` would
/// have used, since the checkbox catalogue gives it no bit of its own), and the upsample
/// factor that actually ran, 1 meaning none.
fn build_enhancement(enhancement_flags: u32, source_frames: usize, rate_ceiling_hz: u32) -> (Option<starplayer::enhance::Chain>, u32, u32) {
    use starplayer::enhance::{CATALOGUE, NO_FLAG_BIT, enhancer_for_id};

    let wants_upsample = enhancement_flags & ENHANCE_FLAG_UPSAMPLE != 0;
    let mut factor: u32 = if wants_upsample { 4 } else { 1 };
    while factor > 1 && source_frames.saturating_mul(factor as usize) > ENHANCE_FRAME_BUDGET {
        factor = if factor == 4 { 2 } else { 1 };
    }

    let mut chain = starplayer::enhance::Chain::new();
    let mut applied_flags = 0u32;
    for descriptor in CATALOGUE.iter() {
        if descriptor.flag_bit == NO_FLAG_BIT || descriptor.flag_bit >= u32::BITS as u8 {
            continue;
        }
        if enhancement_flags & (1 << descriptor.flag_bit) == 0 {
            continue;
        }
        // The upsampler is the only entry the frame budget can narrow or drop, and the
        // only one that wants the rate ceiling — nothing else changes a sample's rate.
        let (id, ceiling_hz) = match descriptor.id {
            ENHANCE_UPSAMPLE_ID if factor == 4 => (ENHANCE_UPSAMPLE_ID, Some(rate_ceiling_hz)),
            ENHANCE_UPSAMPLE_ID if factor == 2 => (ENHANCE_HALF_UPSAMPLE_ID, Some(rate_ceiling_hz)),
            ENHANCE_UPSAMPLE_ID => continue,
            id => (id, None),
        };
        if let Some(enhancer) = enhancer_for_id(id, ceiling_hz) {
            chain = chain.then(enhancer);
            applied_flags |= 1 << descriptor.flag_bit;
        }
    }
    let chain = if chain.is_empty() { None } else { Some(chain) };
    (chain, applied_flags, factor)
}

/// The wire spelling of [`AtEnd`], or `None` for a value the page should never have sent.
const fn at_end_from_wire(argument: u32) -> Option<AtEnd> {
    match argument {
        AT_END_FADE_OUT => Some(AtEnd::FadeOut),
        AT_END_CONTINUE => Some(AtEnd::Continue),
        AT_END_STOP => Some(AtEnd::Stop),
        _ => None,
    }
}

/// The worklet's half of the player: a [`Player`] over a [`WorkletBackend`], plus the blocks
/// JavaScript views.
struct Host {
    /// Holds the render callback the browser's `process()` pushes into. It sits beside the
    /// player rather than inside it because a [`Player`] owns a `Stream`, not a backend.
    backend: WorkletBackend,
    player: Player,
    /// Wire records staged between render quanta, decoded straight from the SAB ring or from
    /// a fallback batch.
    commands: CommandRing,
    interactive_status: [u32; 10],
    /// Records this host refused: an opcode it does not know, an `AT_END` argument that
    /// spells nothing, or a command the player's ring would not take.
    commands_rejected: u32,
    /// Interleaved output, as the backend's callback writes it.
    interleaved: Vec<f32>,
    /// The same block de-interleaved, channel-major with a [`MAX_FRAMES_PER_CALL`] stride,
    /// which is the layout an `AudioWorkletProcessor`'s output arrays want.
    planar: Vec<f32>,
    telemetry_words: Box<[i32]>,
    /// The reading half of one scope tap ring per channel (architecture §9(b)). Taken from
    /// the player at construction, and again whenever the engine is rebuilt.
    scopes: Box<[TapReader]>,
    /// The newest [`SCOPE_WINDOW_BUCKETS`] buckets of each channel, oldest first, laid out
    /// channel-major. The worklet views this and copies it onward.
    scope_values: Box<[i16]>,
    /// Each channel's tap write index as of the last refresh, so the page can tell a
    /// stalled channel from a silent one.
    scope_indices: Box<[i32]>,
    /// Bumped on every refresh, so the worklet publishes a window once rather than once
    /// per quantum.
    scope_generation: u32,
    /// Channels the last refresh actually filled.
    scope_channels: u32,
    quanta_rendered: u64,
    module_generation: u32,
    retired_modules_collected: u32,
    last_peak: f32,
    /// The context's own negotiated rate (`player.spec().sample_rate_hz`), stored so
    /// [`Host::load_module_with_options`] does not have to reach back into the player just
    /// to compute the upsampler's rate ceiling.
    sample_rate_hz: u32,
    /// The enhancement flags actually engaged by the most recent load, reported to the page
    /// through [`exports::applied_enhancement_flags`]. May be narrower than what was asked
    /// for — see [`build_enhancement`].
    last_applied_enhancement_flags: u32,
    /// The upsample factor the most recent load actually ran at (1, 2 or 4), reported
    /// through [`exports::applied_enhancement_factor`] so the page can tell a budget
    /// fallback from the request it made.
    last_enhancement_factor: u32,
}

impl Host {
    #[cfg(test)]
    fn new(sample_rate_hz: u32) -> Host {
        Host::with_mode(sample_rate_hz, MixerMode::DEFAULT).expect("the default mixer mode has a host engine arm")
    }

    fn with_mode(sample_rate_hz: u32, active_mode: MixerMode) -> Result<Host, String> { Self::with_identity(sample_rate_hz, active_mode, None) }

    fn with_identity(sample_rate_hz: u32, active_mode: MixerMode, identity: Option<PlaybackIdentity>) -> Result<Host, String> {
        let mut backend = WorkletBackend::new(sample_rate_hz);
        // `negotiate` answers with the context's own rate, so this is the rate the engine is
        // built at and the rate every module is scanned at, whatever is asked for here.
        let requested = AudioSpec {
            sample_rate_hz,
            channels: active_mode.channels as u16,
            preferred_block_frames: Some(RENDER_QUANTUM as u32),
        };
        let mut player = if let Some(identity) = identity {
            #[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
            {
            let mut settings = starplayer::engine::EngineSettings::interactive_web(identity, 0).map_err(|error| format!("{error:?}"))?;
            settings.sample_rate_hz = sample_rate_hz;
            Player::open_with_settings(&mut backend, None, requested, active_mode, settings)
            }
            #[cfg(not(all(feature = "note-overlap", feature = "interactive-audition")))]
            { let _ = identity; return Err(String::from("interactive web support was not compiled")); }
        } else { Player::open(&mut backend, None, requested, active_mode) }.map_err(|error| error.to_string())?;
        let negotiated_sample_rate_hz = player.spec().sample_rate_hz;
        let scopes = player.take_scope_readers().ok_or_else(|| String::from("a new engine owns its scope readers"))?;
        // A `Player` opens with its transport silent and its musical clock stopped; the page
        // expects a module to sound the moment it is activated, rather than after a separate
        // Play. This is one 64-frame glide up where the pre-D9 host started flat at unity,
        // which is one click fewer and nothing else.
        if identity.is_none() { player.play().map_err(|error| error.to_string())?; }
        Ok(Host {
            backend,
            player,
            commands: CommandRing::new(),
            interactive_status: [0; 10],
            commands_rejected: 0,
            interleaved: vec![0.0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            planar: vec![0.0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            telemetry_words: vec![0; TELEMETRY_WORDS].into_boxed_slice(),
            scopes,
            scope_values: vec![0; SCOPE_VALUES].into_boxed_slice(),
            scope_indices: vec![0; SCOPE_CHANNELS].into_boxed_slice(),
            scope_generation: 0,
            scope_channels: 0,
            quanta_rendered: 0,
            module_generation: 0,
            retired_modules_collected: 0,
            last_peak: 0.0,
            sample_rate_hz: negotiated_sample_rate_hz,
            last_applied_enhancement_flags: 0,
            last_enhancement_factor: 1,
        })
    }

    /// Decode, construct and queue a module. Called from the worklet's message handler,
    /// never from `process()`; a failed decode leaves the previous module and source live.
    #[cfg(test)]
    fn load_module(&mut self, bytes: &[u8]) -> Result<u32, String> { self.load_module_with_options(bytes, false, 0, 0) }

    /// `rate_ceiling_hz` for the upsampler is twice the context's own negotiated rate: the
    /// mixer interpolates between real frames up to that rate, and doubling it (rather than
    /// passing the rate itself) leaves room for a resampling interpolator to still find
    /// something to do above the output rate. See [`build_enhancement`] for the frame-budget
    /// fallback `enhancement_flags` may trigger.
    ///
    /// `speed_adjust` is added to every speed the module asks for, for a module written
    /// against a game's own replayer rather than against a tracker. It is set on the
    /// player *before* the load, because the load is where the song is scanned and the
    /// timeline has to be measured at the speed the song will play at.
    fn load_module_with_options(&mut self, bytes: &[u8], headphone_friendly_mod_panning: bool, enhancement_flags: u32, speed_adjust: i8) -> Result<u32, String> {
        let rate_ceiling_hz = self.sample_rate_hz.saturating_mul(2);
        let (module, applied_flags, factor) =
            decode(bytes, headphone_friendly_mod_panning, enhancement_flags, rate_ceiling_hz).map_err(|error| error.to_string())?;
        self.player.set_speed_adjust(speed_adjust);
        // Everything expensive — the scan, the sequencer, the per-channel state — happens
        // inside this call, on this thread. Only a `Box` and two `Arc`s cross to the render
        // callback, and whatever they replace comes back here to be dropped.
        self.player.load_module(Arc::new(module)).map_err(|error| error.to_string())?;
        self.module_generation = self.module_generation.wrapping_add(1).max(1);
        self.last_applied_enhancement_flags = applied_flags;
        self.last_enhancement_factor = factor;
        Ok(self.module_generation)
    }

    /// Rebuild the typed engine in a worklet message task, retaining the same module Arc and
    /// the position whose audio is currently sounding.
    fn set_mixer_mode(&mut self, mode: MixerMode) -> Result<u32, String> {
        if mode == self.player.mixer_mode() {
            return Ok(mode.to_wire());
        }
        // Mutes are the caller's across a rebuild, by `Player`'s documented contract, and
        // this caller is the one that has them: the page's own view of a muted channel is
        // the snapshot, so the snapshot is what puts them back.
        let snapshot = *self.player.telemetry();
        self.player.set_mixer_mode(&mut self.backend, None, mode).map_err(|error| error.to_string())?;
        for (channel_index, channel) in snapshot.channels.iter().enumerate() {
            if channel.muted {
                self.player.mute(ChannelId(channel_index as u16), true).map_err(|error| error.to_string())?;
            }
        }
        self.scopes = self.player.take_scope_readers().ok_or_else(|| String::from("a rebuilt engine owns its scope readers"))?;
        Ok(mode.to_wire())
    }

    /// Turn live input on or off, from a worklet message task.
    ///
    /// On: the module's instruments are bound to sixteen MIDI channels beside the
    /// module's own sequencer in a `SourceMux`. Off: the MIDI source is removed without
    /// changing the song position. Both allocate — a rack, a queue, a sequencer — which
    /// is exactly why this is not an opcode.
    fn set_midi_input(&mut self, enabled: bool) -> Result<bool, String> {
        if self.player.playback_identity().is_some() { return Err(String::from("interactive routing requires an identified destination command")); }
        if enabled == self.player.is_jamming() {
            return Ok(enabled);
        }
        self.player.jam(enabled).map_err(|error| error.to_string())?;
        Ok(self.player.is_jamming())
    }

    /// Build `kind` and install it in `slot` of `target`'s chain, from a worklet message
    /// task. Allocates the effect's delay lines, so — like [`Host::set_midi_input`] — this
    /// must never be called from `process()`.
    fn install_insert(&mut self, target: u32, slot: u32, kind: u32) -> Result<(), String> {
        let kind = insert_kind_from_wire(kind).ok_or_else(|| format!("{kind}: not a known effect kind"))?;
        self.player.install_insert(insert_target_from_wire(target), slot as u8, kind).map_err(|error| error.to_string())
    }

    /// Take whatever is in `slot` of `target`'s chain out and retire it.
    fn remove_insert(&mut self, target: u32, slot: u32) -> Result<(), String> {
        self.player.remove_insert(insert_target_from_wire(target), slot as u8).map_err(|error| error.to_string())
    }

    fn enqueue(&mut self, command: WireCommand) -> bool { self.commands.push(command) }

    /// Turn each staged wire record into one call on the player.
    ///
    /// Bounded, and on the render path: every arm pushes onto the player's command ring or
    /// writes its seek mailbox, so nothing here allocates, locks or can panic.
    fn drain_commands(&mut self) {
        for _ in 0..COMMAND_RING_CAPACITY {
            let Some(wire) = self.commands.pop() else { break };
            if !self.apply(wire) {
                self.commands_rejected = self.commands_rejected.saturating_add(1);
            }
        }
    }

    fn interactive_action(wire: WireCommand) -> Option<InteractiveAction> {
        Some(
            match wire.opcode {
                13 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::Play,
                14 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::Pause,
                15 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::Resume,
                16 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::Stop,
                17 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::AllSoundOff,
                18 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::StopPreview,
                19 if wire.extra == 0 => InteractiveAction::SetLiveDestination(match wire.argument {
                    0 => LiveDestination::Off, 1 => LiveDestination::Jam,
                    value if value - 2 <= u16::MAX as u32 => LiveDestination::Selected(InstrumentId((value - 2) as u16)), _ => return None,
                }),
                20 if wire.argument <= u16::MAX as u32 && wire.extra == 0 => InteractiveAction::PreviewInstrument(InstrumentId(wire.argument as u16)),
                25 if wire.argument <= u16::MAX as u32 && wire.extra < 128 => InteractiveAction::PreviewInstrumentNote { instrument: InstrumentId(wire.argument as u16), note: wire.extra as u8 },
                26 if wire.argument <= 1 && wire.extra == 0 => InteractiveAction::SetUniqueNotesAcrossChannels(wire.argument != 0),
                21 if wire.argument == 0 && wire.extra == 0 => InteractiveAction::ReleasePreview,
                22 if wire.argument <= 1 && wire.extra == 0 => InteractiveAction::SetNoteOverlapEnabled(wire.argument != 0),
                23 if wire.argument <= u16::MAX as u32 && wire.extra <= 1 => InteractiveAction::SetLetNotesRing { instrument: InstrumentId(wire.argument as u16), enabled: wire.extra != 0 },
                _ => return None,
            }
        )
    }

    /// Validate and reserve the entire v2 acknowledgement group before touching state.
    /// Fixed staging remains in CommandRing; rejection discards every member together.
    fn drain_interactive_batch(&mut self, complete: bool) -> bool {
        let mut host_commands = 0;
        let mut inserts = 0;
        let mut events = 0;
        let mut records = [WireCommand::default(); COMMAND_RING_CAPACITY];
        let mut count = 0;
        for (wire, slot) in self.commands.iter().zip(records.iter_mut()) { *slot = wire; count += 1; }
        let valid = complete && records[..count].iter().copied().all(|wire| {
            if wire.identity.is_none() || !self.valid_wire(wire) { return false; }
            match wire.opcode {
                OPCODE_MIDI_EVENT | 24 => {
                    events += 1;
                    message_to_event(wire.argument as u8, (wire.argument >> 8) as u8, (wire.argument >> 16) as u8).is_some()
                }
                OPCODE_INSERT_PARAM | OPCODE_INSERT_BYPASS => { inserts += 1; true }
                // Interactive seeks reset DSP only after audio-side musical validation,
                // using a dispatch-local signal rather than an insert-ring reservation.
                OPCODE_SEEK_ORDER | OPCODE_SEEK_ROW | OPCODE_SEEK_FRAME => self.player.module().is_some() && self.player.scan().is_some(),
                OPCODE_AT_END => { host_commands += if wire.extra == 0 { 1 } else { 2 }; true }
                13..=26 => { host_commands += 1; Self::interactive_action(wire).is_some_and(|action| self.player.validate_interactive(action).is_ok()) }
                _ => { host_commands += 1; true }
            }
        }) && self.player.can_admit_browser_batch(host_commands, inserts, events);
        if !valid {
            while self.commands.pop().is_some() {}
            self.commands_rejected = self.commands_rejected.saturating_add(1);
            return false;
        }
        self.drain_commands();
        true
    }

    /// One wire record. `false` means nothing was queued — the opcode is unknown, its
    /// argument spells nothing, or the player's ring was full.
    fn valid_wire(&mut self, wire: WireCommand) -> bool {
        if self.player.playback_identity().is_some() && wire.identity != self.player.playback_identity() { return false; }
        if wire.identity.is_some() && wire.opcode < 13 {
            let valid = match wire.opcode {
                OPCODE_PLAY => wire.argument == 0 && wire.extra == 0,
                OPCODE_SEEK_ORDER | OPCODE_SEEK_ROW => wire.argument <= u16::MAX as u32 && wire.extra == 0,
                OPCODE_SEEK_FRAME => wire.extra == 0,
                OPCODE_MASTER_VOLUME => wire.argument <= u16::MAX as u32 && wire.extra == 0,
                OPCODE_MUTE_CHANNEL => wire.argument < 64 && wire.extra <= 1,
                OPCODE_AT_END => at_end_from_wire(wire.argument).is_some(),
                OPCODE_MIDI_EVENT => wire.extra == 0 && wire.argument & 0xff80_8000 == 0,
                OPCODE_INSERT_PARAM | OPCODE_INSERT_BYPASS => {
                    let target = wire.argument & 0xff;
                    let slot = (wire.argument >> 8) & 0xf;
                    (target < 64 || target == INSERT_TARGET_MASTER) && slot < 4 && if wire.opcode == OPCODE_INSERT_BYPASS { wire.argument & !0xfff == 0 && wire.extra <= 1 } else { wire.argument & !0xfffff == 0 }
                }
                _ => false,
            };
            if !valid { return false; }
        }
        if (matches!(wire.opcode, OPCODE_MIDI_EVENT | 20 | 24 | 25) || wire.identity.is_some() && matches!(wire.opcode, OPCODE_SEEK_ORDER | OPCODE_SEEK_ROW | OPCODE_SEEK_FRAME)) && self.player.playback_identity().is_some() {
            let applied = self.player.applied_playback();
            if !applied.accepted || applied.identity != wire.identity { return false; }
        }
        wire.opcode <= 12 || (wire.opcode == 24 && wire.extra == 0 && wire.argument & 0xff80_8000 == 0) || Self::interactive_action(wire).is_some()
    }

    fn apply(&mut self, wire: WireCommand) -> bool {
        if !self.valid_wire(wire) { return false; }
        if wire.opcode >= 13 {
            let Some(identity) = wire.identity else { return false };
            if wire.opcode == 24 {
                return message_to_event(wire.argument as u8, (wire.argument >> 8) as u8, (wire.argument >> 16) as u8).is_some_and(|(channel, event)| self.player.send_event(channel, event).is_ok());
            }
            let Some(action) = Self::interactive_action(wire) else { return false };
            return self.player.submit_interactive(InteractiveCommand { identity, action }).is_ok();
        }
        let queued = match wire.opcode {
            OPCODE_PLAY => self.player.play(),
            OPCODE_STOP => self.player.stop(),
            OPCODE_SEEK_ORDER => self.player.seek_order(wire.argument as u16),
            OPCODE_SEEK_ROW => self.player.seek_row(wire.argument as u16),
            OPCODE_SEEK_FRAME => self.player.seek_frame(wire.argument as u64),
            OPCODE_MASTER_VOLUME => self.player.set_master_volume(U0F16::from_bits(wire.argument as u16)),
            OPCODE_MUTE_CHANNEL => self.player.mute(ChannelId(wire.argument as u16), wire.extra != 0),
            // The record carries the fade length beside the mode, because `AtEnd` has no
            // room for it. An argument that spells no mode is refused rather than guessed.
            OPCODE_AT_END => match at_end_from_wire(wire.argument) {
                Some(at_end) => self.set_at_end(at_end, wire.extra),
                None => return false,
            },
            // A framed browser MIDI message. An `argument` that spells no channel voice
            // message is refused rather than guessed at, exactly as `AT_END` is.
            OPCODE_MIDI_EVENT => {
                let status = wire.argument as u8;
                let data1 = (wire.argument >> 8) as u8;
                let data2 = (wire.argument >> 16) as u8;
                match message_to_event(status, data1, data2) {
                    Some((channel, event)) => self.player.send_event(channel, event),
                    None => return false,
                }
            }
            // Handled synchronously by the worklet message handler, because rebuilding a
            // typed engine allocates; it must never reach the render path.
            OPCODE_SET_MIXER_MODE => return false,
            OPCODE_INSERT_PARAM => {
                let target = insert_target_from_wire(wire.argument & 0xFF);
                let slot = ((wire.argument >> 8) & 0xF) as u8;
                let param = ParamId(((wire.argument >> 12) & 0xFF) as u8);
                self.player.set_insert_param(target, slot, param, wire.extra as i32)
            }
            OPCODE_INSERT_BYPASS => {
                let target = insert_target_from_wire(wire.argument & 0xFF);
                let slot = ((wire.argument >> 8) & 0xF) as u8;
                self.player.bypass_insert(target, slot, wire.extra != 0)
            }
            _ => return false,
        };
        queued.is_ok()
    }

    /// Choose what happens at the end of the song, and how long the fade is when the page
    /// named a length. Zero means "leave the previous length alone", so it is not sent on.
    fn set_at_end(&mut self, at_end: AtEnd, fade_frames: u32) -> Result<(), HostError> {
        self.player.set_at_end(at_end)?;
        if fade_frames > 0 {
            self.player.set_fade_frames(fade_frames)?;
        }
        Ok(())
    }

    fn process(&mut self, frames: usize) -> f32 {
        self.drain_commands();
        let frames = frames.min(MAX_FRAMES_PER_CALL);
        let channels = self.player.mixer_mode().channels as usize;
        let samples = frames.saturating_mul(channels);
        if let Some(block) = self.interleaved.get_mut(..samples) {
            self.backend.render(block);
        }

        // Interleaved to channel-major, which is the layout an `AudioWorkletProcessor`'s
        // output arrays want and the only reason this copy exists.
        for frame in 0..frames {
            for channel in 0..channels {
                let interleaved_index = frame.saturating_mul(channels).saturating_add(channel);
                let planar_index = channel.saturating_mul(MAX_FRAMES_PER_CALL).saturating_add(frame);
                let sample = self.interleaved.get(interleaved_index).copied().unwrap_or(0.0);
                if let Some(destination) = self.planar.get_mut(planar_index) { *destination = sample; }
            }
        }

        let peak = self.player.peak();
        self.quanta_rendered = self.quanta_rendered.wrapping_add(1);
        // Peak-hold with decay, as the original walked `_VUBarLevel` down every tick: a
        // bare 2.7 ms quantum peak flickers, and a quantum that lands between transients
        // reads as silence.
        self.last_peak = peak.max(self.last_peak * MASTER_PEAK_DECAY_PER_QUANTUM);
        let snapshot = *self.player.telemetry();
        self.pack_telemetry(snapshot);
        let applied = self.player.applied_playback();
        let identity = applied.identity.unwrap_or_default();
        self.interactive_status = [applied.serial as u32, (applied.serial >> 32) as u32, identity.session.0 as u32, (identity.session.0 >> 32) as u32, identity.build.0 as u32, (identity.build.0 >> 32) as u32, applied.capabilities.bits() as u32, applied.accepted as u32, self.player.commands_rejected() as u32, (self.player.commands_rejected() >> 32) as u32];
        if self.quanta_rendered.is_multiple_of(SCOPE_REFRESH_QUANTA) {
            self.refresh_scopes(self.tracker_channel_count(snapshot));
        }
        peak
    }

    /// Copy the newest window of each active channel's tap ring into the flat block the
    /// worklet views (architecture §9(b)).
    ///
    /// Only `channel_count` channels: the engine's channel table is always at its maximum
    /// here (the worklet builds one engine and plays every module through it), so copying
    /// all 64 would copy 60 rings of silence for a four-channel MOD.
    ///
    /// No allocation: both the readers' copy and the destination already exist. The window
    /// may be torn — the engine is writing into the same rings — which is what §9(b)'s
    /// lossy tap is for.
    fn refresh_scopes(&mut self, channel_count: usize) {
        let channels = channel_count.min(self.scopes.len()).min(SCOPE_CHANNELS);
        for index in 0..channels {
            let Some(reader) = self.scopes.get(index) else { continue };
            let base = index * SCOPE_WINDOW_BUCKETS;
            let Some(window) = self.scope_values.get_mut(base..base + SCOPE_WINDOW_BUCKETS) else { continue };
            let write_index = reader.latest(window);
            if let Some(slot) = self.scope_indices.get_mut(index) {
                *slot = write_index as i32;
            }
        }
        self.scope_channels = channels as u32;
        self.scope_generation = self.scope_generation.wrapping_add(1);
    }

    fn tracker_channel_count(&self, snapshot: Snapshot) -> usize {
        if self.player.interactive_transport().is_some() { self.player.module().map_or(0, |module| module.header().channel_count as usize) }
        else { snapshot.channel_count as usize }
    }

    fn pack_telemetry(&mut self, snapshot: Snapshot) {
        let song_flags = ((snapshot.transport.song_end != SongEnd::Unknown) as i32)
            | (((snapshot.transport.song_end == SongEnd::Loops) as i32) << 1)
            | ((snapshot.transport.end_reached as i32) << 2)
            | ((self.player.is_fading() as i32) << 3);
        let warnings = (snapshot.warnings.zero_advance_forced as i32)
            | ((snapshot.warnings.event_limit_reached as i32) << 1)
            | ((snapshot.warnings.retired_module_dropped as i32) << 2)
            | ((snapshot.warnings.unsupported_command as i32) << 3)
            | ((snapshot.warnings.late_events as i32) << 4)
            | ((snapshot.warnings.retired_insert_dropped as i32) << 5);
        let header = [
            snapshot.sequence as i32,
            snapshot.publishes_dropped as i32,
            self.tracker_channel_count(snapshot) as i32,
            snapshot.voices_active as i32,
            snapshot.transport.order as i32,
            snapshot.transport.pattern as i32,
            snapshot.transport.row as i32,
            snapshot.transport.tick as i32,
            snapshot.transport.speed as i32,
            snapshot.transport.tempo_bpm as i32,
            snapshot.transport.global_volume.to_bits() as i32,
            warnings,
            self.player.output_frame().0 as i32,
            self.player.pending_garbage() as i32,
            self.module_generation as i32,
            if let Some(transport) = self.player.interactive_transport() {
                8 | i32::from(self.player.is_playing()) | (i32::from(transport == starplayer::core::InteractiveTransport::Paused) << 1) | (i32::from(self.player.natural_end()) << 2)
            } else { self.player.is_playing() as i32 },
            // The master peak is a lossy audio tap (architecture §9): a torn or stale
            // reading costs a UI nothing, so it rides the same block as scalars for now.
            (self.last_peak.clamp(0.0, 1.0) * 65_535.0) as i32,
            self.retired_modules_collected as i32,
            self.player.mixer_mode().to_wire() as i32,
            snapshot.transport.song_frame.min(i32::MAX as u64) as i32,
            snapshot.transport.song_length_frames.min(i32::MAX as u64) as i32,
            song_flags,
        ];
        if let Some(destination) = self.telemetry_words.get_mut(..TELEMETRY_HEADER_WORDS) {
            destination.copy_from_slice(&header);
        }

        let live = snapshot.live_summary();
        if let Some(destination) = self.telemetry_words.get_mut(TELEMETRY_LIVE_OFFSET..) { destination.copy_from_slice(&[live.active as i32, live.retiring as i32, live.peak.to_bits() as i32]); }
        for (index, channel) in snapshot.channels.iter().enumerate() {
            let base = TELEMETRY_HEADER_WORDS + index * TELEMETRY_CHANNEL_WORDS;
            let flags = (channel.active as i32) | ((channel.muted as i32) << 1);
            let values = [
                channel.note.map(|note| note.semitone as i32).unwrap_or(-1),
                channel.instrument as i32,
                channel.volume.to_bits() as i32,
                channel.pan.to_bits() as i32,
                channel.effect.code as i32,
                channel.effect.param as i32,
                channel.vu_level.to_bits() as i32,
                flags,
            ];
            if let Some(destination) = self.telemetry_words.get_mut(base..base + TELEMETRY_CHANNEL_WORDS) {
                destination.copy_from_slice(&values);
            }
        }
    }

    /// Drop everything the render callback has handed back — retired modules and retired
    /// sequencers both — and report how many came back on *this* call.
    ///
    /// The page asserts on the running total rather than on one call's return value:
    /// collection is polled from a message task, so which task sees a retired handle is a
    /// scheduling detail, but the cumulative count is not. [`Player::collect_garbage`]
    /// reports that total and carries it across an engine rebuild, so the difference since
    /// the last call is this call's own count.
    fn collect_garbage(&mut self) -> usize {
        let total = self.player.collect_garbage();
        let collected = total.saturating_sub(self.retired_modules_collected as usize);
        self.retired_modules_collected = total as u32;
        collected
    }

    /// Commands nothing could deliver: the staging ring overflowed, this host refused the
    /// record, or the player's own ring was full when the render callback tried to send it.
    fn dropped_commands(&self) -> u32 {
        self.commands
            .dropped()
            .saturating_add(self.commands_rejected)
            .saturating_add(self.player.commands_rejected().min(u32::MAX as u64) as u32)
    }
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn with_host<T>(fallback: T, action: impl FnOnce(&mut Host) -> T) -> T {
    HOST.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => match slot.as_mut() {
            Some(host) => action(host),
            None => fallback,
        },
        Err(_) => fallback,
    })
}

#[allow(unsafe_code, reason = "`#[wasm_bindgen]` expands to unsafe ABI shims")]
mod exports {
    use super::*;
    use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

    /// Allocate the engine, rings and all steady-state render buffers.
    #[wasm_bindgen]
    pub fn init(sample_rate: f32, mixer_mode_wire: u32) -> bool {
        drop(core::hint::black_box(vec![0u8; HEAP_RESERVE_BYTES]));
        let sample_rate_hz = sample_rate.round().clamp(8_000.0, 384_000.0) as u32;
        let mode = MixerMode::from_wire(mixer_mode_wire).unwrap_or(MixerMode::DEFAULT);
        let mut initialized = false;
        HOST.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut()
                && let Ok(host) = Host::with_mode(sample_rate_hz, mode)
            {
                *slot = Some(host);
                initialized = true;
            }
        });
        initialized
    }

    /// Runtime opt-in is explicit and independent of Cargo feature unification.
    #[wasm_bindgen]
    pub fn init_interactive(sample_rate: f32, mode: u32, session: u64, build: u64) -> bool {
        drop(core::hint::black_box(vec![0u8; HEAP_RESERVE_BYTES]));
        let identity = PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) };
        let rate = sample_rate.round().clamp(8_000.0, 384_000.0) as u32;
        HOST.with(|cell| {
            let Ok(mut slot) = cell.try_borrow_mut() else { return false };
            let Ok(host) = Host::with_identity(rate, MixerMode::from_wire(mode).unwrap_or(MixerMode::DEFAULT), Some(identity)) else { return false };
            *slot = Some(host);
            true
        })
    }

    #[wasm_bindgen]
    pub fn interactive_wire_version() -> u32 { 2 }

    #[wasm_bindgen]
    pub fn interactive_status_ptr() -> u32 { with_host(0, |host| host.interactive_status.as_ptr() as usize as u32) }
    #[wasm_bindgen]
    pub fn queue_applied_barrier_words(low: u32, high: u32) -> bool { with_host(false, |host| host.player.playback_barrier(low as u64 | ((high as u64) << 32)).is_ok()) }
    #[wasm_bindgen]
    pub fn queue_applied_barrier(serial: u32) -> bool { with_host(false, |host| host.player.playback_barrier(serial as u64).is_ok()) }
    #[wasm_bindgen]
    pub fn enqueue_interactive_words(opcode: u32, argument: u32, extra: u32, session_low: u32, session_high: u32, build_low: u32, build_high: u32) -> bool {
        if !(1..=26).contains(&opcode) { return false; }
        let identity = PlaybackIdentity { session: SessionId(session_low as u64 | ((session_high as u64) << 32)), build: BuildGeneration(build_low as u64 | ((build_high as u64) << 32)) };
        with_host(false, |host| host.enqueue(WireCommand { identity: Some(identity), opcode: opcode as u8, argument, extra }))
    }

    #[wasm_bindgen]
    pub fn enqueue_interactive(opcode: u32, argument: u32, extra: u32, session: u64, build: u64) -> bool {
        if !(13..=26).contains(&opcode) { return false; }
        with_host(false, |host| host.enqueue(WireCommand { identity: Some(PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) }), opcode: opcode as u8, argument, extra }))
    }

    #[wasm_bindgen]
    pub fn pending_control_commands() -> u32 { with_host(0, |host| host.player.pending_control_commands() as u32) }

    #[wasm_bindgen]
    pub fn flush_interactive_batch(complete: bool) -> bool { with_host(false, |host| host.drain_interactive_batch(complete)) }

    #[wasm_bindgen]
    pub fn flush_commands() { with_host((), |host| host.drain_commands()); }

    #[wasm_bindgen]
    pub fn playback_barrier(serial: u64) -> bool { with_host(false, |host| host.player.playback_barrier(serial).is_ok()) }
    #[wasm_bindgen]
    pub fn applied_serial() -> u64 { with_host(0, |host| host.player.applied_playback().serial) }
    #[wasm_bindgen]
    pub fn applied_session() -> u64 { with_host(0, |host| host.player.applied_playback().identity.map_or(0, |identity| identity.session.0)) }
    #[wasm_bindgen]
    pub fn applied_build() -> u64 { with_host(0, |host| host.player.applied_playback().identity.map_or(0, |identity| identity.build.0)) }
    #[wasm_bindgen]
    pub fn applied_accepted() -> bool { with_host(false, |host| host.player.applied_playback().accepted) }
    #[wasm_bindgen]
    pub fn applied_capabilities() -> u32 { with_host(0, |host| host.player.applied_playback().capabilities.bits() as u32) }
    #[wasm_bindgen]
    pub fn current_session() -> u64 { with_host(0, |host| host.player.playback_identity().map_or(0, |identity| identity.session.0)) }
    #[wasm_bindgen]
    pub fn current_build() -> u64 { with_host(0, |host| host.player.playback_identity().map_or(0, |identity| identity.build.0)) }

    /// Off-render, versioned and field-wise. No raw Rust layouts cross realms.
    #[wasm_bindgen]
    pub fn replacement_state() -> Result<Vec<u64>, JsValue> {
        with_host(Err(JsValue::NULL), |host| host.player.replacement_state().map_err(|error| JsValue::from_str(&error.to_string())))
    }

    #[wasm_bindgen]
    pub fn load_module_identified(bytes: &[u8], panning: bool, enhancement: u32, speed_adjust: i32, session: u64, build: u64, replacement: &[u64]) -> Result<u32, JsValue> {
        with_host(Err(JsValue::NULL), |host| {
            let (module, flags, factor) = decode(bytes, panning, enhancement, host.sample_rate_hz.saturating_mul(2)).map_err(|error| JsValue::from_str(&error.to_string()))?;
            let identity = PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) };
            host.player.set_speed_adjust(speed_adjust.clamp(i8::MIN as i32, i8::MAX as i32) as i8);
            if replacement.is_empty() { host.player.load_module_identified(Arc::new(module), identity) }
            else { host.player.load_replacement_deferred(Arc::new(module), identity, replacement) }.map_err(|error| JsValue::from_str(&error.to_string()))?;
            host.module_generation = host.module_generation.wrapping_add(1).max(1);
            host.last_applied_enhancement_flags = flags;
            host.last_enhancement_factor = factor;
            Ok(host.module_generation)
        })
    }

    /// Finish a browser replacement only after its policy replay is audio-applied.
    #[wasm_bindgen]
    pub fn finish_replacement_transport(session: u64, build: u64, opcode: u32) -> bool {
        let requested = match opcode { 0 => None, 13 | 15 => Some(starplayer::core::InteractiveTransport::Playing), 14 => Some(starplayer::core::InteractiveTransport::Paused), 16 => Some(starplayer::core::InteractiveTransport::Stopped), _ => return false };
        with_host(false, |host| host.player.finish_replacement_transport(PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) }, requested).is_ok())
    }

    #[wasm_bindgen]
    pub fn cancel_load_transport(session: u64, build: u64) -> bool {
        with_host(false, |host| host.player.cancel_load_transport(PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) }).is_ok())
    }

    #[wasm_bindgen]
    pub fn finish_load_transport(session: u64, build: u64, opcode: u32) -> bool {
        let action = match opcode { 13 => InteractiveAction::Play, 14 => InteractiveAction::Pause, 15 => InteractiveAction::Resume, 16 => InteractiveAction::Stop, _ => return false };
        with_host(false, |host| host.player.finish_load_transport(PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) }, action).is_ok())
    }

    /// Decode and activate one validated native module byte buffer outside `process()`.
    #[wasm_bindgen]
    pub fn load_module(bytes: &[u8]) -> Result<u32, JsValue> { load_module_with_options(bytes, false, 0, 0) }

    /// Decode and activate a module with web-player loading preferences. The panning
    /// option is deliberately MOD-specific; every other format continues through its
    /// native loader. `enhancement_flags` is the bit word [`enhancements_json`] describes
    /// (M10-W4); [`applied_enhancement_flags`] and [`applied_enhancement_factor`] report
    /// what this call actually did, since a frame-budget fallback can narrow the request.
    ///
    /// `speed_adjust` is added to every speed the module asks for — zero is canonical
    /// playback, and the page's own "Adjust speed" control is the only thing that sets it.
    /// It is clamped to a tick count a row clock can hold, so a page cannot ask for a
    /// bias the engine has no way to express.
    #[wasm_bindgen]
    pub fn load_module_with_options(bytes: &[u8], headphone_friendly_mod_panning: bool, enhancement_flags: u32, speed_adjust: i32) -> Result<u32, JsValue> {
        let speed_adjust = speed_adjust.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.load_module_with_options(bytes, headphone_friendly_mod_panning, enhancement_flags, speed_adjust).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// The enhancement flags actually engaged by the most recent [`load_module_with_options`]
    /// call (M10-W4). Narrower than what was asked for when the frame budget forced a
    /// fallback; see [`applied_enhancement_factor`] for which upsample size actually ran.
    #[wasm_bindgen]
    pub fn applied_enhancement_flags() -> u32 { with_host(0, |host| host.last_applied_enhancement_flags) }

    /// The upsample factor the most recent load actually ran at: 4, 2, or 1 for none —
    /// smaller than the checkbox asked for exactly when the frame budget forced a
    /// fallback, which is what the page reports in its status line.
    #[wasm_bindgen]
    pub fn applied_enhancement_factor() -> u32 { with_host(1, |host| host.last_enhancement_factor) }

    /// Every load-time sample enhancer with a checkbox bit, as a JSON array — read
    /// straight from `starplayer::enhance::CATALOGUE` so the page's load panel can never
    /// drift from what `load_module_with_options`'s `enhancement_flags` actually accepts
    /// (M10-W4, mirroring [`effects_json`]'s contract for the Effects panel).
    ///
    /// One object per checkbox-eligible entry, in `CATALOGUE` order:
    /// `{"bit", "id", "label", "description"}`. `starplayer-enhance`'s command-line-only
    /// entries (`NO_FLAG_BIT`, currently `sinc2x`) are omitted — a command line spells
    /// them by id, but a checkbox has no bit to set. Callable before [`init`] — it names
    /// what this build can do, not what any particular host instance has loaded.
    #[wasm_bindgen]
    pub fn enhancements_json() -> String {
        use std::fmt::Write;

        let mut json = String::from("[");
        let mut first = true;
        for descriptor in starplayer::enhance::CATALOGUE.iter() {
            if descriptor.flag_bit == starplayer::enhance::NO_FLAG_BIT {
                continue;
            }
            if !first {
                json.push(',');
            }
            first = false;
            let _ = write!(
                json,
                r#"{{"bit":{},"id":"{}","label":"{}","description":"{}"}}"#,
                descriptor.flag_bit, descriptor.id, descriptor.label, descriptor.description
            );
        }
        json.push(']');
        json
    }

    /// Decode one SAB/fallback wire record into the wasm-side fixed command ring.
    #[wasm_bindgen]
    pub fn enqueue_command(opcode: u32, argument: u32, extra: u32) -> bool {
        let command = WireCommand { identity: None, opcode: opcode as u8, argument, extra };
        with_host(false, |host| host.enqueue(command))
    }

    /// Rebuild the selected typed engine outside `process()`.
    #[wasm_bindgen]
    pub fn set_mixer_mode(wire: u32) -> Result<u32, JsValue> {
        let mode = MixerMode::from_wire(wire).ok_or_else(|| JsValue::from_str("invalid mixer mode wire value"))?;
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.set_mixer_mode(mode).map_err(|message| JsValue::from_str(&message))
        })
    }

    #[wasm_bindgen]
    pub fn process(frames: u32) -> f32 { with_host(0.0, |host| host.process(frames as usize)) }

    #[wasm_bindgen]
    pub fn output_ptr() -> u32 { with_host(0, |host| host.planar.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn output_len() -> u32 { (MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS) as u32 }

    #[wasm_bindgen]
    pub fn output_channel_stride() -> u32 { MAX_FRAMES_PER_CALL as u32 }

    #[wasm_bindgen]
    pub fn telemetry_ptr() -> u32 { with_host(0, |host| host.telemetry_words.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn telemetry_len() -> u32 { TELEMETRY_WORDS as u32 }

    /// The scope block: `scope_len()` `i16` values, channel-major, each channel's
    /// `scope_window_buckets()` buckets oldest first (architecture §9(b)).
    #[wasm_bindgen]
    pub fn scope_ptr() -> u32 { with_host(0, |host| host.scope_values.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn scope_len() -> u32 { SCOPE_VALUES as u32 }

    /// Buckets per channel in the block above.
    #[wasm_bindgen]
    pub fn scope_window_buckets() -> u32 { SCOPE_WINDOW_BUCKETS as u32 }

    /// Output frames one bucket covers, so a page can label the time axis.
    #[wasm_bindgen]
    pub fn scope_bucket_frames() -> u32 { TAP_BUCKET_FRAMES as u32 }

    /// One `i32` write index per channel, as of the last refresh.
    #[wasm_bindgen]
    pub fn scope_index_ptr() -> u32 { with_host(0, |host| host.scope_indices.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn scope_index_len() -> u32 { SCOPE_CHANNELS as u32 }

    /// Bumped on every refresh of the block, so the worklet publishes each window once.
    #[wasm_bindgen]
    pub fn scope_generation() -> u32 { with_host(0, |host| host.scope_generation) }

    /// Channels the last refresh filled.
    #[wasm_bindgen]
    pub fn scope_channels() -> u32 { with_host(0, |host| host.scope_channels) }

    /// Install or remove the live-input source, outside `process()`.
    ///
    /// Returns whether live input is on afterwards. It allocates the instrument rack and
    /// the event queue, so — like [`set_mixer_mode`] — it is only ever reached from a
    /// worklet message task.
    #[wasm_bindgen]
    pub fn set_midi_input(enabled: bool) -> Result<bool, JsValue> {
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.set_midi_input(enabled).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Build an effect and install it, outside `process()` (M7-H7). `target` is a channel
    /// index, or `0xFF` for the master bus; `kind` is the effect's position in
    /// `InsertKind::ALL`, the same numbering [`effects_json`] reports.
    #[wasm_bindgen]
    pub fn install_insert(target: u32, slot: u32, kind: u32) -> Result<(), JsValue> {
        HOST.with(|cell| {
            let mut slot_ref = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot_ref.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.install_insert(target, slot, kind).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Take whatever is installed out and retire it, outside `process()`.
    #[wasm_bindgen]
    pub fn remove_insert(target: u32, slot: u32) -> Result<(), JsValue> {
        HOST.with(|cell| {
            let mut slot_ref = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot_ref.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.remove_insert(target, slot).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Every effect this build can install, its parameters, their units, ranges and
    /// defaults, as a JSON array — read straight from each effect's own
    /// `InsertDescriptor`, so the page's Effects panel can never drift from what
    /// `install_insert` and `OPCODE_INSERT_PARAM` actually accept.
    ///
    /// One object per effect, in [`InsertKind::ALL`] order: `{"name", "wire", "params":
    /// [{"name", "unit", "min", "max", "default"}]}`. `wire` is the `kind` `install_insert`
    /// takes; `unit` is the `ParamUnit` variant's own name (`"CentiDecibels"`, `"Percent"`,
    /// …), left as an enum tag rather than a display string so the page chooses its own
    /// wording. Callable before [`init`] — it names what this build can do, not what any
    /// particular host instance has done.
    #[wasm_bindgen]
    pub fn effects_json() -> String {
        use std::fmt::Write;

        let mut json = String::from("[");
        for (wire, kind) in starplayer::dsp::InsertKind::ALL.into_iter().enumerate() {
            let described: Box<dyn starplayer::dsp::Insert<f32>> = starplayer::dsp::build_insert(kind, 44_100);
            let descriptor = described.descriptor();
            if wire > 0 {
                json.push(',');
            }
            let _ = write!(json, r#"{{"name":"{}","wire":{wire},"params":["#, descriptor.name);
            for (index, parameter) in descriptor.params.iter().enumerate() {
                if index > 0 {
                    json.push(',');
                }
                let _ = write!(
                    json,
                    r#"{{"name":"{}","unit":"{:?}","min":{},"max":{},"default":{}}}"#,
                    parameter.name, parameter.unit, parameter.min, parameter.max, parameter.default
                );
            }
            json.push_str("]}");
        }
        json.push(']');
        json
    }

    /// How far ahead of the audio clock a live event is stamped, in frames — the latency
    /// the page reports next to its keyboard toggle.
    ///
    /// Live events a full queue refused are **not** exported separately: a refused live
    /// event fails `Host::apply` like any other undeliverable record, so it is already in
    /// [`dropped_commands`].
    #[wasm_bindgen]
    pub fn event_lead_frames() -> u32 { with_host(0, |host| host.player.event_lead()) }

    /// The same lead in milliseconds, computed against the context's own rate.
    #[wasm_bindgen]
    pub fn event_lead_millis() -> f32 { with_host(0.0, |host| host.player.event_lead_millis()) }

    #[wasm_bindgen]
    pub fn render_quantum() -> u32 { RENDER_QUANTUM as u32 }

    #[wasm_bindgen]
    pub fn dropped_commands() -> u32 { with_host(0, |host| host.dropped_commands()) }

    #[wasm_bindgen]
    pub fn quanta_rendered() -> f64 { with_host(0.0, |host| host.quanta_rendered as f64) }

    /// Drop retired module Arcs from a worklet message task, never from `process()`.
    #[wasm_bindgen]
    pub fn collect_garbage() -> u32 { with_host(0, |host| host.collect_garbage() as u32) }

    #[wasm_bindgen]
    pub fn pending_garbage() -> u32 { with_host(0, |host| host.player.pending_garbage() as u32) }

    /// Retired module handles dropped off the audio callback since `init`.
    #[wasm_bindgen]
    pub fn retired_modules_collected() -> u32 { with_host(0, |host| host.retired_modules_collected) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer::core::Interpolator;
    use starplayer::enhance::SampleEnhancer;
    use starplayer::engine::{EndReason, MixPathKind, OutputDepth};
    use starplayer_host::SeekKind;
    use std::collections::BTreeSet;

    const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");
    /// The largest module fixture committed to the repository (36 kB): W4 research point
    /// 1's timing measurement runs against this one, not `FIXTURE`.
    const LARGEST_FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");

    fn minimal_mod() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 256;
        let mut bytes = vec![0; 1084 + 64 * 4 * 4 + SAMPLE_FRAMES];
        bytes[..10].copy_from_slice(b"native mod");
        bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
        bytes[45] = 64;
        bytes[950] = 1;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes[1084..1088].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0, param: 0 }.to_bytes());
        let sample_offset = 1084 + 64 * 4 * 4;
        for (index, byte) in bytes[sample_offset..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0x7F } else { 0x80 }; }
        bytes
    }

    #[test]
    fn tagless_soundtracker_is_admitted_with_the_web_panning_preference_and_renders() {
        let mut bytes = vec![0; 600 + 1024 + 8];
        bytes[42..44].copy_from_slice(&4u16.to_be_bytes());
        bytes[45] = 64;
        bytes[48..50].copy_from_slice(&4u16.to_be_bytes());
        bytes[470] = 1;
        bytes[471] = 120;
        bytes[600..604].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0xF, param: 6 }.to_bytes());
        bytes[600 + 1024..].copy_from_slice(&[0x7f, 0x80, 0x40, 0xc0, 0x7f, 0x80, 0x40, 0xc0]);
        for headphone_friendly in [false, true] {
            let (module, flags, factor) = decode(&bytes, headphone_friendly, 0, 48_000).expect("web legacy decode");
            assert_eq!((flags, factor), (0, 1));
            assert_eq!(module.header().dialect, starplayer::core::quirks::FormatDialect::Soundtracker);
            let amount = if headphone_friendly { 60 } else { 100 };
            assert_eq!(module.header().channel_pan(0), Some(starplayer::core::fixed::bipolar_from_ratio(-amount, 100)));
            let mut host = Host::new(48_000);
            assert_eq!(host.load_module_with_options(&bytes, headphone_friendly, 0, 0), Ok(1));
            let mut audible = false;
            for _ in 0..8 {
                audible |= host.process(RENDER_QUANTUM) > 0.0;
            }
            assert!(audible, "legacy MOD reaches the worklet output");
        }
    }

    /// [`minimal_mod`] with a `B00` on its very last row, so the song genuinely loops
    /// rather than merely running out of order list — which task D2 reads as an end.
    fn looping_mod() -> Vec<u8> {
        let mut bytes = minimal_mod();
        let last_cell = 1084 + 63 * 4 * 4;
        let jump = starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xB, param: 0x00 };
        bytes[last_cell..last_cell + 4].copy_from_slice(&jump.to_bytes());
        bytes
    }

    /// [`minimal_mod`] whose first row asks for speed 5, so a speed *request* exists for
    /// `load_module_with_options`'s `speed_adjust` to bias.
    fn speed_requesting_mod() -> Vec<u8> {
        let mut bytes = minimal_mod();
        let cell = starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0xF, param: 0x05 };
        bytes[1084..1088].copy_from_slice(&cell.to_bytes());
        bytes
    }

    /// The page's "Adjust speed" control, end to end: the module asks for speed 5 and the
    /// engine runs the row at 5 + the adjustment, with the module's own bytes untouched.
    #[test]
    fn the_speed_adjustment_biases_what_the_module_asks_for() {
        let bytes = speed_requesting_mod();
        for (adjust, expected) in [(0i32, 5u8), (1, 6), (3, 8), (-4, 1)] {
            let mut host = Host::new(48_000);
            assert_eq!(host.load_module_with_options(&bytes, false, 0, adjust as i8), Ok(1));
            for _ in 0..4 { host.process(RENDER_QUANTUM); }
            assert_eq!(host.player.telemetry().transport.speed, expected, "speed 5 biased by {adjust}");
        }
    }

    fn minimal_mtm() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 256;
        const SAMPLE_HEADER: usize = 66;
        const ORDER_TABLE: usize = SAMPLE_HEADER + 37;
        const TRACK_DATA: usize = ORDER_TABLE + 128;
        const PATTERN_TABLE: usize = TRACK_DATA + 192;
        const SAMPLE_DATA: usize = PATTERN_TABLE + 64;
        let mut bytes = vec![0; SAMPLE_DATA + SAMPLE_FRAMES];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[4..14].copy_from_slice(b"native mtm");
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[30] = 1;
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        bytes[SAMPLE_HEADER..SAMPLE_HEADER + 6].copy_from_slice(b"sample");
        bytes[SAMPLE_HEADER + 22..SAMPLE_HEADER + 26].copy_from_slice(&(SAMPLE_FRAMES as u32).to_le_bytes());
        bytes[SAMPLE_HEADER + 35] = 64;
        bytes[TRACK_DATA..TRACK_DATA + 3].copy_from_slice(&starplayer::mtm::MtmCell { pitch: 12, instrument: 1, effect: 0, param: 0 }.to_bytes());
        bytes[PATTERN_TABLE..PATTERN_TABLE + 2].copy_from_slice(&1u16.to_le_bytes());
        for (index, byte) in bytes[SAMPLE_DATA..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0xFF } else { 0x00 }; }
        bytes
    }

    fn mode(path: MixPathKind, interpolator: Interpolator, depth: OutputDepth, dither: bool, channels: u8) -> MixerMode {
        MixerMode { path, interpolator, depth, dither, channels }
    }

    fn rendered_planar(mode: MixerMode, quanta: usize) -> Vec<f32> {
        let mut host = Host::with_mode(48_000, mode).expect("test mode has an engine arm");
        assert!(host.load_module(FIXTURE).is_ok());
        let mut output = Vec::with_capacity(quanta * RENDER_QUANTUM * mode.channels as usize);
        for _ in 0..quanta {
            host.process(RENDER_QUANTUM);
            for channel in 0..mode.channels as usize {
                let first = channel * MAX_FRAMES_PER_CALL;
                output.extend_from_slice(&host.planar[first..first + RENDER_QUANTUM]);
            }
        }
        output
    }

    #[test]
    fn the_scope_block_fills_from_the_engines_tap_rings() {
        let mut host = Host::new(48_000);
        assert_eq!(host.scope_generation, 0, "nothing is refreshed before the first quanta");
        assert!(host.load_module(FIXTURE).is_ok());

        // Enough quanta for several refreshes, and for REFLEX to actually strike a note.
        for _ in 0..(SCOPE_REFRESH_QUANTA as usize * 40) {
            host.process(RENDER_QUANTUM);
        }

        assert!(host.scope_generation >= 40, "the block refreshed once every {SCOPE_REFRESH_QUANTA} quanta");
        assert!(host.scope_channels > 0, "and filled the module's channels");
        assert_eq!(host.scope_values.len(), SCOPE_VALUES);
        assert_eq!(host.scope_indices.len(), SCOPE_CHANNELS);

        let active = host.scope_channels as usize;
        let filled = host.scope_values[..active * SCOPE_WINDOW_BUCKETS].iter().any(|value| *value != 0);
        assert!(filled, "the tap carries a signal for the sounding channels");
        assert!(host.scope_indices[..active].iter().all(|index| *index > 0), "each active channel published buckets");
        assert!(
            host.scope_values[active * SCOPE_WINDOW_BUCKETS..].iter().all(|value| *value == 0),
            "channels the module does not use are never refreshed and stay silent",
        );
    }

    #[test]
    fn a_mixer_mode_rebuild_takes_the_new_engines_scope_readers() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..64 { host.process(RENDER_QUANTUM); }
        assert!(host.scope_indices[0] > 0);

        let rebuilt = mode(MixPathKind::Fixed, Interpolator::Linear, OutputDepth::I16, false, 2);
        assert!(host.set_mixer_mode(rebuilt).is_ok());
        for _ in 0..64 { host.process(RENDER_QUANTUM); }
        assert!(host.scope_indices[0] > 0, "the rebuilt engine's rings are the ones being read");
        assert!(
            host.scope_values[..SCOPE_WINDOW_BUCKETS].iter().any(|value| *value != 0),
            "and they carry a signal rather than the retired engine's silence",
        );
    }

    #[test]
    fn all_sixteen_typed_engine_arms_build_and_render_a_quantum() {
        for path in [MixPathKind::Float, MixPathKind::Fixed] {
            for interpolator in [Interpolator::None, Interpolator::Linear, Interpolator::Cubic, Interpolator::Sinc] {
                for channels in [1, 2] {
                    let mode = mode(path, interpolator, OutputDepth::F32, false, channels);
                    let mut host = Host::with_mode(48_000, mode).expect("the documented arm exists");
                    assert!(host.load_module(FIXTURE).is_ok());
                    host.process(RENDER_QUANTUM);
                    assert_eq!(host.player.mixer_mode(), mode);
                    assert_eq!(host.telemetry_words[18] as u32, mode.to_wire());
                }
            }
        }
    }

    #[test]
    fn switching_mode_keeps_the_sounding_order_generation_and_same_module_arc() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_SEEK_ORDER, argument: 1, extra: 0 }));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }
        assert_eq!(host.player.telemetry().transport.order, 1);
        let generation = host.module_generation;
        let retired = host.retired_modules_collected;
        let module = Arc::clone(host.player.module().expect("the host retains its module"));

        let retro = mode(MixPathKind::Fixed, Interpolator::None, OutputDepth::I8, false, 2);
        assert_eq!(host.set_mixer_mode(retro), Ok(retro.to_wire()));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        assert_eq!(host.player.telemetry().transport.order, 1, "the rebuilt sequencer seeks to the sounding order");
        assert!(host.player.telemetry().transport.song_frame > 0, "and to the song frame that was sounding, not to the top");
        assert_eq!(host.module_generation, generation, "a mode switch is not a module reload");
        assert_eq!(host.retired_modules_collected, retired, "the same Arc is not retired through the audio channel");
        assert_eq!(host.collect_garbage(), 0, "no module was retired by the switch");
        assert!(Arc::ptr_eq(host.player.module().expect("module retained"), &module));
    }

    /// A mode switch is a rebuild, not a stop: the transport comes back running and the new
    /// engine is the one making the sound.
    #[test]
    fn switching_mode_while_playing_keeps_playing() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..300 { host.process(RENDER_QUANTUM); }
        assert!(host.player.is_playing());

        let rebuilt = mode(MixPathKind::Fixed, Interpolator::Linear, OutputDepth::I16, false, 2);
        assert!(host.set_mixer_mode(rebuilt).is_ok());
        let mut heard = false;
        for _ in 0..300 { heard |= host.process(RENDER_QUANTUM) > 0.0; }
        assert!(host.player.is_playing(), "a mode switch is not a stop");
        assert!(heard, "and the rebuilt engine sounds");
        assert_eq!(host.telemetry_words[15], 1, "the wire header says so too");
    }

    /// The fixture opens quietly, so a short render only reaches a couple of dozen codes.
    /// Two thousand quanta reach a peak of about 0.39 full scale, where the same passage at
    /// full depth takes thousands of distinct values and the 8-bit one still cannot.
    #[test]
    fn i8_output_uses_no_more_than_256_values() {
        let quanta = 2_000;
        let eight_bit = rendered_planar(mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::I8, false, 2), quanta);
        let values: BTreeSet<u32> = eight_bit.iter().map(|sample| sample.to_bits()).collect();
        assert!(values.len() <= 256, "8-bit output produced {} distinct values", values.len());
        assert!(values.len() > 1, "the fixture produced more than one 8-bit value");
        assert!(eight_bit.iter().any(|sample| *sample != 0.0), "the comparison covered non-silent output");

        let full_depth = rendered_planar(mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::F32, false, 2), quanta);
        let full_values: BTreeSet<u32> = full_depth.iter().map(|sample| sample.to_bits()).collect();
        assert!(full_values.len() > 256, "the same passage at full depth takes only {} distinct values", full_values.len());
    }

    #[test]
    fn dither_changes_output_and_is_deterministic() {
        let dithered_mode = mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::I8, true, 2);
        let first = rendered_planar(dithered_mode, 100);
        let second = rendered_planar(dithered_mode, 100);
        let undithered = rendered_planar(MixerMode { dither: false, ..dithered_mode }, 100);
        assert_eq!(first, second, "the seeded TPDF stream repeats exactly");
        assert_ne!(first, undithered, "enabling dither changes reduced-depth output");
    }

    #[test]
    fn a_real_s3m_reaches_the_real_engine_and_renders() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(FIXTURE), Ok(1));
        let mut heard = false;
        for _ in 0..200 {
            heard |= host.process(RENDER_QUANTUM) > 0.0;
        }
        assert!(heard, "the S3M sequencer should trigger sample audio");
        assert!(host.player.telemetry().sequence > 0);
    }

    #[test]
    fn a_mod_uses_the_native_processor_and_reaches_the_real_engine() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(&minimal_mod()), Ok(1));
        assert_eq!(host.player.module().map(|module| module.header().format), Some(ModuleFormat::Mod));
        assert!((0..20).any(|_| host.process(RENDER_QUANTUM) > 0.0), "the MOD sequencer should trigger sample audio");
        assert!(host.player.telemetry().sequence > 0);
    }

    #[test]
    fn mod_headphone_option_changes_only_mod_initial_panning() {
        let mut host = Host::new(48_000);
        let mod_bytes = minimal_mod();
        assert_eq!(host.load_module_with_options(&mod_bytes, false, 0, 0), Ok(1));
        let hard: Vec<i16> = host.player.module().expect("MOD retained").header().default_pan.iter().map(|pan| pan.to_bits()).collect();
        assert_eq!(hard, [-32_767, 32_767, 32_767, -32_767]);
        assert_eq!(host.load_module_with_options(&mod_bytes, true, 0, 0), Ok(2));
        let headphone: Vec<i16> = host.player.module().expect("MOD retained").header().default_pan.iter().map(|pan| pan.to_bits()).collect();
        assert_eq!(headphone, [-19_660, 19_660, 19_660, -19_660]);

        assert_eq!(host.load_module_with_options(FIXTURE, false, 0, 0), Ok(3));
        let s3m_authentic = host.player.module().expect("S3M retained").header().default_pan.to_vec();
        assert_eq!(host.load_module_with_options(FIXTURE, true, 0, 0), Ok(4));
        assert_eq!(host.player.module().expect("S3M retained").header().default_pan.as_ref(), s3m_authentic.as_slice());

        let mtm_bytes = minimal_mtm();
        assert_eq!(host.load_module_with_options(&mtm_bytes, false, 0, 0), Ok(5));
        let mtm_authentic = host.player.module().expect("MTM retained").header().default_pan.to_vec();
        assert_eq!(host.load_module_with_options(&mtm_bytes, true, 0, 0), Ok(6));
        assert_eq!(host.player.module().expect("MTM retained").header().default_pan.as_ref(), mtm_authentic.as_slice());
    }

    // ── M10-W4: the enhancement checkboxes' wasm surface ────────────────────────────

    #[test]
    fn enhancement_flags_zero_is_the_plain_load() {
        let mut plain = Host::new(48_000);
        assert!(plain.load_module(FIXTURE).is_ok());
        let plain_lengths: Vec<u32> = plain.player.module().expect("retained").samples().iter().map(|sample| sample.length_frames()).collect();

        let mut unflagged = Host::new(48_000);
        assert_eq!(unflagged.load_module_with_options(FIXTURE, false, 0, 0), Ok(1));
        let unflagged_lengths: Vec<u32> =
            unflagged.player.module().expect("retained").samples().iter().map(|sample| sample.length_frames()).collect();
        assert_eq!(unflagged_lengths, plain_lengths, "flags 0 is byte-identical to the no-options load");
        assert_eq!(unflagged.last_applied_enhancement_flags, 0);
        assert_eq!(unflagged.last_enhancement_factor, 1);
    }

    #[test]
    fn the_sinc4x_flag_quadruples_every_samples_length_and_records_the_scale() {
        let mut plain = Host::new(48_000);
        assert!(plain.load_module(FIXTURE).is_ok());
        let plain_lengths: Vec<u32> = plain.player.module().expect("retained").samples().iter().map(|sample| sample.length_frames()).collect();
        assert!(plain_lengths.iter().all(|length| *length > 0), "the fixture actually carries samples to enhance");

        let mut enhanced = Host::new(48_000);
        assert_eq!(enhanced.load_module_with_options(FIXTURE, false, ENHANCE_FLAG_UPSAMPLE, 0), Ok(1));
        let enhanced_module = enhanced.player.module().expect("retained");
        for (plain_length, sample) in plain_lengths.iter().zip(enhanced_module.samples().iter()) {
            assert_eq!(sample.length_frames(), plain_length * 4, "sinc4x quadruples every sample's stored length");
            assert_eq!(sample.rate_scale_log2(), 2, "a 4x rebuild is log2(4) = 2");
        }
        assert_eq!(enhanced.last_applied_enhancement_flags, ENHANCE_FLAG_UPSAMPLE, "the upsample bit reports as engaged");
        assert_eq!(enhanced.last_enhancement_factor, 4, "and at the requested factor, since the fixture is well under budget");
    }

    #[test]
    fn a_source_too_large_for_any_upsample_still_runs_the_loop_smoother() {
        // Larger than `ENHANCE_FRAME_BUDGET` even at 2x: the upsample bit is asked for but
        // nothing runs, while the loop bit — unaffected by the budget — still does.
        let source_frames = ENHANCE_FRAME_BUDGET;
        let (chain, applied_flags, factor) = build_enhancement(ENHANCE_FLAG_UPSAMPLE | ENHANCE_FLAG_LOOP, source_frames, 48_000);
        assert!(chain.is_some(), "the loop smoother still runs");
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("loop=64")));
        assert_eq!(applied_flags, ENHANCE_FLAG_LOOP, "the upsample bit did not survive an impossible budget");
        assert_eq!(factor, 1);
    }

    #[test]
    fn build_enhancement_downgrades_a_tiny_budget_step_by_step() {
        // Independent of any fixture's real size: a synthetic frame count exercises every
        // rung of the fallback deterministically.
        let (chain_4x_fits, flags_4x_fits, factor_4x_fits) = build_enhancement(ENHANCE_FLAG_UPSAMPLE, 1_000, 48_000);
        assert!(chain_4x_fits.is_some());
        assert_eq!(flags_4x_fits, ENHANCE_FLAG_UPSAMPLE);
        assert_eq!(factor_4x_fits, 4, "1,000 x 4 is nowhere near the budget");

        let (chain_2x, flags_2x, factor_2x) = build_enhancement(ENHANCE_FLAG_UPSAMPLE, ENHANCE_FRAME_BUDGET / 3, 48_000);
        assert!(chain_2x.is_some());
        assert_eq!(flags_2x, ENHANCE_FLAG_UPSAMPLE);
        assert_eq!(factor_2x, 2, "x4 would cross the budget, x2 does not");

        let (chain_identity, flags_identity, factor_identity) = build_enhancement(ENHANCE_FLAG_UPSAMPLE, ENHANCE_FRAME_BUDGET, 48_000);
        assert!(chain_identity.is_none(), "even x2 crosses the budget at this size, so nothing runs");
        assert_eq!(flags_identity, 0);
        assert_eq!(factor_identity, 1);
    }

    /// M10-K5c's two new bits reach their enhancers through the catalogue walk, in
    /// catalogue order rather than bit order.
    #[test]
    fn the_denoise_and_sbr_bits_build_their_stages_in_run_order() {
        const DENOISE: u32 = 1 << 2;
        const SBR: u32 = 1 << 3;

        let (chain, applied_flags, factor) = build_enhancement(DENOISE, 1_000, 48_000);
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("denoise")));
        assert_eq!(applied_flags, DENOISE);
        assert_eq!(factor, 1, "the denoiser changes no rate");

        let (chain, applied_flags, _) = build_enhancement(SBR, 1_000, 48_000);
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("sbr")));
        assert_eq!(applied_flags, SBR);

        // Bits set in any order build the chain in the order the stages must run in.
        let (chain, applied_flags, factor) = build_enhancement(SBR | ENHANCE_FLAG_LOOP | ENHANCE_FLAG_UPSAMPLE | DENOISE, 1_000, 48_000);
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("denoise+sinc4x-ceil48000+sbr+loop=64")));
        assert_eq!(applied_flags, DENOISE | ENHANCE_FLAG_UPSAMPLE | SBR | ENHANCE_FLAG_LOOP);
        assert_eq!(factor, 4);
    }

    /// The frame budget narrows the upsampler and leaves every other stage alone.
    #[test]
    fn the_frame_budget_narrows_only_the_upsampler() {
        const DENOISE: u32 = 1 << 2;
        const SBR: u32 = 1 << 3;

        let (chain, applied_flags, factor) = build_enhancement(DENOISE | ENHANCE_FLAG_UPSAMPLE | SBR, ENHANCE_FRAME_BUDGET / 3, 48_000);
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("denoise+sinc2x-ceil48000+sbr")));
        assert_eq!(applied_flags, DENOISE | ENHANCE_FLAG_UPSAMPLE | SBR, "2x still reports through the upsample bit");
        assert_eq!(factor, 2);

        let (chain, applied_flags, factor) = build_enhancement(DENOISE | ENHANCE_FLAG_UPSAMPLE | SBR, ENHANCE_FRAME_BUDGET, 48_000);
        assert_eq!(chain.map(|chain| chain.name()), Some(String::from("denoise+sbr")), "the upsampler drops out and the rest stays");
        assert_eq!(applied_flags, DENOISE | SBR);
        assert_eq!(factor, 1);
    }

    /// A load through the real host with every checkbox bit set reaches the engine.
    #[test]
    fn every_checkbox_bit_together_loads_and_plays() {
        let all_bits: u32 = starplayer::enhance::CATALOGUE
            .iter()
            .filter(|descriptor| descriptor.flag_bit != starplayer::enhance::NO_FLAG_BIT)
            .map(|descriptor| 1u32 << descriptor.flag_bit)
            .fold(0, |flags, bit| flags | bit);
        assert_eq!(all_bits, 0b1111, "four checkbox enhancers, bits 0 to 3");

        let mut host = Host::new(48_000);
        assert_eq!(host.load_module_with_options(FIXTURE, false, all_bits, 0), Ok(1));
        assert_eq!(host.last_applied_enhancement_flags, all_bits, "every bit engaged");
        assert_eq!(host.last_enhancement_factor, 4);
        assert!((0..40).any(|_| host.process(RENDER_QUANTUM) > 0.0), "the enhanced module still plays");
    }

    #[test]
    fn an_unknown_enhancement_bit_is_ignored_like_from_flags() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module_with_options(FIXTURE, false, 0xFFFF_FFF0, 0), Ok(1), "no known bit is set");
        assert_eq!(host.last_applied_enhancement_flags, 0);
        assert_eq!(host.last_enhancement_factor, 1);
    }

    #[test]
    fn enhancements_json_names_every_checkbox_eligible_enhancer_and_its_bit() {
        let json = exports::enhancements_json();
        for descriptor in starplayer::enhance::CATALOGUE.iter() {
            if descriptor.flag_bit == starplayer::enhance::NO_FLAG_BIT {
                assert!(!json.contains(&format!(r#""id":"{}""#, descriptor.id)), "{} has no checkbox bit and must not appear", descriptor.id);
                continue;
            }
            assert!(json.contains(&format!(r#""bit":{}"#, descriptor.flag_bit)), "{}: missing bit in enhancements_json", descriptor.id);
            assert!(json.contains(&format!(r#""id":"{}""#, descriptor.id)), "{}: missing from enhancements_json", descriptor.id);
            assert!(json.contains(&format!(r#""label":"{}""#, descriptor.label)), "{}: missing label", descriptor.id);
        }
    }

    /// W4 research point 1: how long a `sinc4x` rebuild of the largest fixture in the
    /// repository takes. This is `Module::enhanced` running natively (`cargo test` builds
    /// this crate's `rlib` for the host target, not wasm) rather than a genuine
    /// worklet-thread measurement — see the task's Research resolution for why that is the
    /// honest number available here. Also asserts a generous upper bound so a real
    /// regression in the polyphase filter shows up as a test failure, not just a slow page.
    #[test]
    fn a_sinc4x_rebuild_of_the_largest_fixture_completes_quickly() {
        // Two loads on the same already-constructed `Host` (so `Host::new`'s own engine
        // and ring allocation, unrelated to the enhancer, is excluded from both) isolate
        // what the enhancer itself costs: the plain load is the baseline, and the
        // difference is `Module::enhanced`'s own polyphase-filter work.
        let mut host = Host::new(48_000);
        let plain_started = std::time::Instant::now();
        assert_eq!(host.load_module_with_options(LARGEST_FIXTURE, false, 0, 0), Ok(1));
        let plain_elapsed = plain_started.elapsed();

        let enhanced_started = std::time::Instant::now();
        assert_eq!(host.load_module_with_options(LARGEST_FIXTURE, false, ENHANCE_FLAG_UPSAMPLE, 0), Ok(2));
        let enhanced_elapsed = enhanced_started.elapsed();
        assert_eq!(host.last_enhancement_factor, 4, "PETRI.S3M is nowhere near the frame budget, so the full 4x ran");

        eprintln!(
            "W4 research point 1: PETRI.S3M (36 kB) plain load {plain_elapsed:?}, sinc4x load {enhanced_elapsed:?} \
             (native `cargo test`, unoptimized; not the worklet thread — see the task's Research resolution)"
        );
        assert!(enhanced_elapsed.as_millis() < 1_000, "a sinc4x rebuild of the largest fixture took {enhanced_elapsed:?}, which is surprisingly slow for 36 kB");
    }

    #[test]
    fn an_mtm_uses_the_native_processor_and_reaches_the_real_engine() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(&minimal_mtm()), Ok(1));
        assert_eq!(host.player.module().map(|module| module.header().format), Some(ModuleFormat::Mtm));
        assert!((0..20).any(|_| host.process(RENDER_QUANTUM) > 0.0), "the MTM sequencer should trigger sample audio");
        assert!(host.player.telemetry().sequence > 0);
    }

    #[test]
    fn the_sequencer_is_built_at_the_context_s_own_sample_rate() {
        let mut host = Host::new(44_100);
        assert_eq!(host.player.spec().sample_rate_hz, 44_100, "not the control clock's rounded-down 44 000");
        assert_eq!(host.backend.sample_rate_hz(), 44_100);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!((0..200).any(|_| host.process(RENDER_QUANTUM) > 0.0));
    }

    #[test]
    fn a_bad_load_keeps_the_previous_module_usable() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(host.load_module(b"not an s3m").is_err());
        assert_eq!(host.module_generation, 1);
        for _ in 0..10 { host.process(RENDER_QUANTUM); }
        assert!(host.player.telemetry().sequence > 0);
    }

    #[test]
    fn a_replaced_module_is_counted_when_it_returns_down_the_garbage_channel() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        assert_eq!(host.retired_modules_collected, 0, "nothing has been replaced yet");
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        host.process(RENDER_QUANTUM);
        // Both halves come back: the module handle down the engine's garbage channel, and
        // the sequencer the incoming source replaced. Neither is dropped in the callback.
        assert_eq!(host.collect_garbage(), 2, "the first module and its sequencer both came back");
        assert_eq!(host.retired_modules_collected, 2);
        assert_eq!(host.player.pending_garbage(), 0);
        assert!(!host.player.warnings().retired_module_dropped);
    }

    #[test]
    fn swapping_a_module_mid_song_does_not_force_the_musical_clock_forward() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 });
        // Long enough that a clock left at zero would be more than `MAX_ZERO_ADVANCE`
        // ticks behind the engine when the second module arrives.
        for _ in 0..1_500 { host.process(RENDER_QUANTUM); }
        assert!(!host.player.warnings().zero_advance_forced, "the first module plays cleanly");

        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..100 { host.process(RENDER_QUANTUM); }
        assert!(!host.player.warnings().zero_advance_forced, "and so does the one loaded over the top of it");
    }

    #[test]
    fn a_loaded_module_is_scanned_and_its_length_reaches_the_telemetry() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        let end_frame = host.player.song_length().expect("activation scans the module");
        assert!(end_frame > 48_000, "REFLEX is longer than a second");
        // REFLEX's order list runs out; nothing in it jumps backwards (task D2).
        assert_eq!(host.player.scan().expect("the player keeps the scan").timeline.end(), EndReason::Ended, "REFLEX ends");

        host.process(RENDER_QUANTUM);
        assert_eq!(host.telemetry_words[20] as u64, end_frame, "the song length rides in word 20");
        assert_eq!(host.telemetry_words[21] & 0b11, 0b01, "length known, and the song does not end by looping");
        assert_eq!(host.telemetry_words[21] & 0b1000, 0, "nothing is fading");

        // …and a module that really loops says so, which is the bit the page's "add the
        // fade to the displayed length" decision keys on.
        let mut host = Host::new(48_000);
        assert!(host.load_module(&looping_mod()).is_ok());
        host.process(RENDER_QUANTUM);
        assert_eq!(host.telemetry_words[21] & 0b11, 0b11, "length known, and the song ends by looping");
    }

    #[test]
    fn a_frame_seek_moves_the_song_clock_through_the_mailbox() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        let scanned = std::sync::Arc::clone(host.player.scan().expect("the module was scanned"));
        let timeline = &scanned.timeline;
        let target = timeline.end_frame() / 2;
        let expected = *timeline.mark_at_frame(target).expect("a frame inside the song resolves");
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_SEEK_FRAME, argument: target as u32, extra: 0 }));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        let snapshot = *host.player.telemetry();
        assert_eq!(snapshot.transport.order, expected.order, "the seek landed on the scanned order");
        assert!(snapshot.transport.song_frame >= expected.frame, "elapsed picks up where the scan says that row is");
        assert!(snapshot.transport.song_frame < expected.frame + 48_000, "and not somewhere else entirely");
        assert!(!host.player.warnings().unsupported_command, "a frame seek is routed, not flagged");
        assert_eq!(host.telemetry_words[19] as u64, snapshot.transport.song_frame, "the elapsed frame rides in word 19");
    }

    #[test]
    fn the_at_end_opcode_carries_the_mode_and_an_unknown_one_is_refused() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert_eq!(host.player.at_end(), AtEnd::Continue, "repeat is on by default");

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.player.at_end(), AtEnd::FadeOut);

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_STOP, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.player.at_end(), AtEnd::Stop);

        let rejected = host.dropped_commands();
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: 99, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.player.at_end(), AtEnd::Stop, "an unknown mode is rejected rather than guessed");
        assert_eq!(host.dropped_commands(), rejected + 1);

        // The mixer-mode opcode belongs to the worklet's message handler, because rebuilding
        // a typed engine allocates. Reaching the render path is a refusal, not a rebuild.
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_SET_MIXER_MODE, argument: 0x0000_0221, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.dropped_commands(), rejected + 2);
        assert_eq!(host.player.mixer_mode(), MixerMode::DEFAULT, "and no engine was rebuilt");
    }

    // ── live input (task E6) ────────────────────────────────────────────────────────

    /// Pack a channel voice message the way `ring.js` does.
    fn midi_wire(status: u8, data1: u8, data2: u8) -> WireCommand {
        WireCommand {
            identity: None,
            opcode: OPCODE_MIDI_EVENT,
            argument: status as u32 | ((data1 as u32) << 8) | ((data2 as u32) << 16),
            extra: 0,
        }
    }

    /// The loudest sample in `quanta` quanta of the planar block.
    fn render_peak(host: &mut Host, quanta: usize) -> f32 {
        let mut peak = 0.0f32;
        for _ in 0..quanta {
            host.process(RENDER_QUANTUM);
            for channel in 0..host.player.mixer_mode().channels as usize {
                let first = channel * MAX_FRAMES_PER_CALL;
                for sample in &host.planar[first..first + RENDER_QUANTUM] {
                    peak = peak.max(sample.abs());
                }
            }
        }
        peak
    }

    #[test]
    fn a_midi_event_opcode_sounds_beside_the_module_and_both_reach_telemetry() {
        // Host::new remains the legacy constructor even when web Cargo defaults compile
        // optional capabilities. Its 48-based assertions protect that contract; NO-02/06
        // thread explicit interactive settings and install the 64-based live rack.
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(!host.player.is_jamming());

        assert_eq!(host.set_midi_input(true), Ok(true), "the rack installs from a worklet message task");
        assert!(host.player.is_jamming());
        assert!(render_peak(&mut host, 8) > 0.001, "the module keeps playing in jam mode");

        assert!(host.enqueue(midi_wire(0xC0, 0, 0)), "program change: MIDI channel 0 takes instrument 0");
        assert!(host.enqueue(midi_wire(0x90, 60, 100)), "note on");
        let mut together_peak = 0.0f32;
        let mut simultaneous = None;
        for _ in 0..16 {
            together_peak = together_peak.max(render_peak(&mut host, 1));
            let snapshot = *host.player.telemetry();
            if snapshot.channels[48].active && snapshot.channels[..48].iter().any(|channel| channel.active) {
                simultaneous = Some(snapshot);
                break;
            }
        }
        assert!(together_peak > 0.001, "the note sounded through the wire opcode");
        let snapshot = simultaneous.expect("one coherent snapshot shows tracker and MIDI voices together");
        assert_eq!(snapshot.channel_count, 64, "module and MIDI lanes are visible together");
        assert!(snapshot.channels[..48].iter().any(|channel| channel.active), "a tracker lane is sounding");
        assert!(snapshot.channels[48].active, "the live MIDI lane is sounding beside it");
        assert_eq!(host.player.events_sent(), 2);
        assert_eq!(host.player.events_rejected(), 0);

        assert!(host.enqueue(midi_wire(0xB0, 120, 0)), "controller 120 is all-sound-off");
        let _ = render_peak(&mut host, 4);
        assert!(render_peak(&mut host, 8) > 0.001, "all-sound-off leaves the module playing");

        assert_eq!(host.set_midi_input(false), Ok(false), "and the module comes back");
        assert!(!host.player.is_jamming());
        assert!(render_peak(&mut host, 200) > 0.001, "and disabling jam does not interrupt the module");
    }

    /// An `argument` that spells no channel voice message is refused, exactly as an unknown
    /// `AT_END` mode is — and a live event with no queue installed is refused too, rather
    /// than installing one on the render path.
    #[test]
    fn a_midi_event_the_host_cannot_use_is_counted_rather_than_guessed_at() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());

        let rejected = host.dropped_commands();
        assert!(host.enqueue(midi_wire(0x90, 60, 100)));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.dropped_commands(), rejected + 1, "no live-input queue is installed yet");

        assert_eq!(host.set_midi_input(true), Ok(true));
        // 0xF0 is a system message, which `message_to_event` has no channel voice meaning
        // for; the page should never send one.
        assert!(host.enqueue(midi_wire(0xF0, 0x7E, 0x00)));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.dropped_commands(), rejected + 2);
        assert_eq!(host.player.events_sent(), 0, "and nothing reached the queue");
    }

    /// The lead the page shows next to its keyboard toggle. A worklet is always called
    /// with a whole render quantum, so the floor never rises above the two quanta
    /// master-plan decision 6 chose.
    #[test]
    fn the_reported_lead_is_two_quanta_on_a_worklets_block_size() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..8 {
            host.process(RENDER_QUANTUM);
        }
        assert_eq!(host.player.event_lead(), 2 * RENDER_QUANTUM as u32);
        assert!((host.player.event_lead_millis() - 256.0 * 1_000.0 / 48_000.0).abs() < 1e-3);
    }

    /// Live input has to survive a mixer-mode rebuild, because the page's Mixer panel and
    /// its keyboard are independent controls and a rebuild is the one thing that throws
    /// away the engine the queue was talking to.
    #[test]
    fn live_input_survives_a_mixer_mode_rebuild() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert_eq!(host.set_midi_input(true), Ok(true));
        host.process(RENDER_QUANTUM);

        assert!(host.set_mixer_mode(mode(MixPathKind::Fixed, Interpolator::Linear, OutputDepth::I16, false, 2)).is_ok());
        assert!(host.player.is_jamming(), "the rebuilt engine came back in jam mode");

        assert!(host.enqueue(midi_wire(0xC0, 0, 0)));
        assert!(host.enqueue(midi_wire(0x90, 60, 100)));
        assert!(render_peak(&mut host, 60) > 0.001, "and the new queue reaches the new engine");
    }

    #[test]
    fn a_song_that_reaches_its_loop_point_under_fade_out_ramps_down_and_rewinds() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.player.song_length().expect("the module was scanned");
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fade_seen = false;
        let mut stopped_at = None;
        for quantum in 0..(end_frame as usize / RENDER_QUANTUM + 200) {
            host.process(RENDER_QUANTUM);
            fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
            if !host.player.is_playing() {
                stopped_at = Some(quantum);
                break;
            }
        }
        assert!(fade_seen, "the fade was armed and reported");
        let stopped_at = stopped_at.expect("the transport stopped once the fade finished");
        assert!(stopped_at as u64 * RENDER_QUANTUM as u64 >= end_frame, "it did not stop before the loop point");
        assert!(!host.player.is_fading(), "the fade is finished, not stuck");
        assert_eq!(host.player.pending_seek().kind, SeekKind::Frame(0), "a faded-out song rewinds for the next Play");
    }

    /// Task D2, the owner's case: with Repeat off, a song whose order list simply runs out
    /// stops at its end frame instead of playing on under a five-second fade.
    #[test]
    fn a_song_that_runs_out_of_order_list_stops_at_its_end_instead_of_fading() {
        let module = minimal_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.player.song_length().expect("the module was scanned");
        assert_eq!(host.player.scan().expect("scanned").timeline.end(), EndReason::Ended);
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 48_000 * 5 }));
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fade_seen = false;
        let mut stopped_at = None;
        for quantum in 0..(end_frame as usize / RENDER_QUANTUM + 200) {
            host.process(RENDER_QUANTUM);
            fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
            if !host.player.is_playing() {
                stopped_at = Some(quantum);
                break;
            }
        }
        assert!(!fade_seen, "a song that ends has nothing to fade into");
        assert!(!host.player.is_fading());
        let stopped_at = stopped_at.expect("the transport stopped");
        let stopped_frame = stopped_at as u64 * RENDER_QUANTUM as u64;
        assert!(stopped_frame >= end_frame, "it did not stop before the end of the song");
        // The transport glide, plus the quantum the arming decision is taken in.
        assert!(stopped_frame < end_frame + 4 * RENDER_QUANTUM as u64, "and it stopped there, not five seconds later: {stopped_frame} vs {end_frame}");
        assert_eq!(host.player.pending_seek().kind, SeekKind::Frame(0), "a song that ended rewinds for the next Play");

        // A stopped engine whose snapshot still says `end_reached` must not stop again.
        let rejected = host.dropped_commands();
        for _ in 0..20 { host.process(RENDER_QUANTUM); }
        assert_eq!(host.dropped_commands(), rejected, "nothing re-armed over the silence");

        // Play after an end-stop is "again", not "resume": the pending rewind is consumed,
        // the transport comes back to unity and the sticky end flag clears.
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));
        let mut heard = false;
        for _ in 0..8 { heard |= host.process(RENDER_QUANTUM) > 0.0; }
        assert!(host.player.is_playing(), "the song plays again");
        assert!(heard, "at unity, not at the faded-out level");
        assert_eq!(host.player.pending_seek().kind, SeekKind::None, "the rewind was consumed");
        assert_eq!(host.telemetry_words[21] & 0b100, 0, "and the sticky end flag went with it");
        assert!((host.telemetry_words[19] as u64) < end_frame / 4, "it restarted from the top, not from the end");
    }

    #[test]
    fn a_faded_out_song_fades_again_on_its_next_pass() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.player.song_length().expect("the module was scanned");
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        let budget = end_frame as usize / RENDER_QUANTUM + 200;

        let play_through = |host: &mut Host| {
            assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));
            let mut fade_seen = false;
            for _ in 0..budget {
                host.process(RENDER_QUANTUM);
                fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
                if !host.player.is_playing() { break; }
            }
            assert!(!host.player.is_playing(), "the fade landed and the transport stopped");
            fade_seen
        };

        assert!(play_through(&mut host), "the first pass fades");
        // Stopped, with the snapshot still saying the end was reached: nothing may re-arm.
        for _ in 0..20 { host.process(RENDER_QUANTUM); }
        assert!(!host.player.is_fading(), "a stopped transport does not fade over silence");
        assert_eq!(host.telemetry_words[21] & 0b1000, 0, "and does not report a fade");

        assert!(play_through(&mut host), "the second pass fades again from the top");
        assert!(!host.player.is_fading());
    }

    /// The Repeat checkbox is a toggle, not a one-way door: choosing Continue while the fade
    /// runs takes the fade back and the song plays on.
    #[test]
    fn choosing_continue_while_a_fade_runs_takes_the_fade_back() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.player.song_length().expect("the module was scanned");
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 48_000 * 5 }));
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        for _ in 0..(end_frame as usize / RENDER_QUANTUM + 400) {
            host.process(RENDER_QUANTUM);
            if host.player.is_fading() { break; }
        }
        assert!(host.player.is_fading(), "the fade started at the loop point");

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_CONTINUE, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.player.at_end(), AtEnd::Continue);
        assert!(!host.player.is_fading(), "the fade was taken back");

        let mut heard = false;
        for _ in 0..200 { heard |= host.process(RENDER_QUANTUM) > 0.0; }
        assert!(host.player.is_playing(), "and no stop was queued behind it");
        assert!(heard, "the gain came home rather than staying faded");
        assert_eq!(host.telemetry_words[21] & 0b1000, 0, "the wire header stops reporting a fade");
    }

    #[test]
    fn the_song_fade_attenuates_the_output_all_the_way_to_silence() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.player.song_length().expect("the module was scanned");
        let fade_frames: u32 = 48_000;
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: fade_frames }));
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fading_peaks = Vec::new();
        let mut playing_words = Vec::new();
        for _ in 0..((end_frame as usize + fade_frames as usize) / RENDER_QUANTUM + 50) {
            let peak = host.process(RENDER_QUANTUM);
            if host.player.is_fading() {
                fading_peaks.push(peak);
                playing_words.push(host.telemetry_words[15]);
            }
            if !host.player.is_playing() { break; }
        }
        let fade_quanta = fade_frames as usize / RENDER_QUANTUM;
        assert!(fading_peaks.len() >= fade_quanta - 2 && fading_peaks.len() <= fade_quanta + 2, "the fade ran for its whole length: {} quanta", fading_peaks.len());
        assert!(playing_words.iter().all(|word| *word == 1), "the transport reports playing for the whole fade");

        let quarter = fading_peaks.len() / 4;
        let loudest = |peaks: &[f32]| peaks.iter().copied().fold(0.0f32, f32::max);
        let first = loudest(&fading_peaks[..quarter]);
        let last = loudest(&fading_peaks[fading_peaks.len() - quarter..]);
        assert!(first > 0.0, "the fixture makes sound going into the fade");
        assert!(last < first * 0.3, "the last quarter of the fade is well down on the first: {last} vs {first}");
        assert!(!host.player.is_playing(), "and the transport stopped when the fade landed");
    }

    #[test]
    fn seek_commands_use_the_fixed_mailbox_without_flagging_engine_unsupported() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_SEEK_ORDER, argument: 1, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert!(!host.player.warnings().unsupported_command);
    }

    /// Stop is a glide, not a step, and it lands on exact silence — the assertion the Node
    /// worklet harness makes on the shipped bundle, made here on the Rust side.
    #[test]
    fn stop_ramps_to_exact_silence_and_play_brings_it_back() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        let mut heard = false;
        for _ in 0..400 { heard |= host.process(RENDER_QUANTUM) > 0.0; }
        assert!(heard, "the fixture is sounding before the stop");

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_STOP, argument: 0, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.process(RENDER_QUANTUM), 0.0, "the ramp reached exact silence");
        assert!(!host.player.is_playing());

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_PLAY, argument: 0, extra: 0 }));
        let mut again = false;
        for _ in 0..20 { again |= host.process(RENDER_QUANTUM) > 0.0; }
        assert!(again, "Play brings it back");
        assert!(host.player.is_playing());
    }

    // ── M7-H7: the insert graph over the wire protocol ──────────────────────────────

    #[test]
    fn install_insert_builds_the_effect_and_the_layout_reports_it() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());

        let reverb_wire = InsertKind::ALL.iter().position(|kind| *kind == InsertKind::Reverb).unwrap() as u32;
        host.install_insert(0, 0, reverb_wire).expect("install queues");
        assert_eq!(
            host.player.inserts().get(InsertTarget::Channel(ChannelId(0)), 0).map(|installed| installed.kind),
            Some(InsertKind::Reverb)
        );

        host.remove_insert(0, 0).expect("remove queues");
        assert!(host.player.inserts().get(InsertTarget::Channel(ChannelId(0)), 0).is_none());
    }

    #[test]
    fn an_unknown_kind_is_refused_rather_than_guessed() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(host.install_insert(0, 0, 999).is_err());
    }

    #[test]
    fn the_master_target_wire_value_installs_on_the_master_bus() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        let gain_wire = InsertKind::ALL.iter().position(|kind| *kind == InsertKind::Gain).unwrap() as u32;
        host.install_insert(INSERT_TARGET_MASTER, 0, gain_wire).expect("install queues");
        assert_eq!(host.player.inserts().get(InsertTarget::Master, 0).map(|installed| installed.kind), Some(InsertKind::Gain));
    }

    #[test]
    fn opcode_insert_param_and_opcode_insert_bypass_reach_the_player() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        let reverb_wire = InsertKind::ALL.iter().position(|kind| *kind == InsertKind::Reverb).unwrap() as u32;
        host.install_insert(0, 0, reverb_wire).expect("install queues");

        // room is ParamId(0): target 0, slot 0, param 0 all pack to argument 0; value 80.
        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_INSERT_PARAM, argument: 0, extra: 80u32 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(
            host.player.inserts().get(InsertTarget::Channel(ChannelId(0)), 0).and_then(|installed| installed.param(ParamId(0))),
            Some(80)
        );

        assert!(host.enqueue(WireCommand { identity: None, opcode: OPCODE_INSERT_BYPASS, argument: 0, extra: 1 }));
        host.process(RENDER_QUANTUM);
        assert!(host.player.inserts().get(InsertTarget::Channel(ChannelId(0)), 0).unwrap().bypassed);
    }

    #[test]
    fn effects_json_names_every_effect_and_its_wire_number() {
        let json = exports::effects_json();
        for (wire, kind) in InsertKind::ALL.into_iter().enumerate() {
            assert!(json.contains(&format!(r#""name":"{}""#, kind.name())), "{}: missing from effects_json", kind.name());
            assert!(json.contains(&format!(r#""wire":{wire}"#)), "{}: wrong wire position in effects_json", kind.name());
        }
    }
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn interactive_host(rate: u32) -> Host { Host::with_identity(rate, MixerMode::DEFAULT, Some(PlaybackIdentity::default())).unwrap() }

    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn identified_load(host: &mut Host, bytes: &[u8], session: u64, build: u64) {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        host.player.load_module_identified(module, PlaybackIdentity { session: SessionId(session), build: BuildGeneration(build) }).unwrap();
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn no08_seek_admission_does_not_reserve_an_insert_slot() {
        let mut host = interactive_host(48_000); identified_load(&mut host, &minimal_mod(), 1, 1);
        host.player.playback_barrier(1).unwrap(); host.process(128);
        for _ in 0..16 { host.player.bypass_insert(InsertTarget::Master, 0, false).unwrap(); }
        let identity = host.player.playback_identity();
        assert!(host.enqueue(WireCommand { identity, opcode: OPCODE_SEEK_ROW, argument: 1, extra: 0 }));
        assert!(host.drain_interactive_batch(true));
        assert_eq!(host.player.pending_seek().kind, starplayer_host::SeekKind::Row(1));
        host.process(128);
        assert_eq!(host.player.pending_seek().kind, starplayer_host::SeekKind::None);
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn versioned_groups_reject_without_partial_shadows_and_retry_after_queue_pressure() {
        fn ready() -> Host {
            let mut host = interactive_host(48_000); identified_load(&mut host, &minimal_mod(), 1, 1);
            host.player.playback_barrier(1).unwrap(); host.process(128); host
        }
        for failure in 0..7 {
            let mut host = ready(); let identity = host.player.playback_identity();
            let enable = WireCommand { identity, opcode: 22, argument: 1, extra: 0 };
            let mut bad = WireCommand { identity, opcode: 23, argument: 0, extra: 1 };
            match failure {
                0 => bad.identity.as_mut().unwrap().build.0 += 1,
                1 => bad.argument = u16::MAX as u32,
                2 => { for _ in 0..30 { host.player.set_master_volume(U0F16::MAX).unwrap(); } }
                3 => {
                    for _ in 0..16 { host.player.bypass_insert(InsertTarget::Master, 0, false).unwrap(); }
                    bad = WireCommand { identity, opcode: OPCODE_INSERT_BYPASS, argument: 255, extra: 1 };
                }
                4 => {
                    for _ in 0..256 { host.player.send_event(0, starplayer::core::Event::NoteOn { note: starplayer::core::Note::new(60), velocity: U0F16::MAX }).unwrap(); }
                    bad = WireCommand { identity, opcode: 24, argument: 0x7f_3c_90, extra: 0 };
                }
                5 => {
                    // Seek no longer consumes an insert entry: its successful audio-side
                    // validation resets DSP directly. A full insert mutation still rejects
                    // the entire seek group before the mailbox can be written.
                    for _ in 0..16 { host.player.bypass_insert(InsertTarget::Master, 0, false).unwrap(); }
                    host.enqueue(WireCommand { identity, opcode: OPCODE_SEEK_ROW, argument: 0, extra: 0 });
                    bad = WireCommand { identity, opcode: OPCODE_INSERT_BYPASS, argument: 255, extra: 1 };
                }
                6 => bad = WireCommand { identity, opcode: OPCODE_SEEK_ORDER, argument: u32::MAX, extra: 0 },
                _ => unreachable!(),
            }
            assert!(host.enqueue(enable)); assert!(host.enqueue(bad));
            assert!(!host.drain_interactive_batch(true), "case{failure}");
            assert_eq!(host.player.pending_seek().kind, starplayer_host::SeekKind::None, "rejected seek never reaches mailbox");
            for _ in 0..8 { host.process(128); }
            assert_eq!(host.player.replacement_state().unwrap()[6], 0, "no partial overlap shadow case{failure}");
            assert!(host.enqueue(enable)); assert!(host.drain_interactive_batch(true));
            host.player.playback_barrier(2).unwrap(); host.process(128);
            assert!(host.player.applied_playback().accepted);
            assert_eq!(host.player.replacement_state().unwrap()[6], 1);
        }
        let mut empty = interactive_host(48_000); let identity = empty.player.playback_identity();
        empty.enqueue(WireCommand { identity, opcode: 22, argument: 1, extra: 0 });
        empty.enqueue(WireCommand { identity, opcode: OPCODE_SEEK_ROW, argument: 0, extra: 0 });
        assert!(!empty.drain_interactive_batch(true)); assert_eq!(empty.player.pending_control_commands(), 0);
        assert_eq!(empty.player.pending_seek().kind, starplayer_host::SeekKind::None);
        let mut host = ready(); let identity = host.player.playback_identity();
        // Existing throughput: one group admits63 real MIDI events, not one per quantum.
        let sent = host.player.events_sent();
        for _ in 0..63 { assert!(host.enqueue(WireCommand { identity, opcode: 24, argument: 0x7f_3c_90, extra: 0 })); }
        assert!(host.drain_interactive_batch(true)); assert_eq!(host.player.events_sent() - sent, 63);
        host.player.playback_barrier(2).unwrap(); host.process(128); assert!(host.player.applied_playback().accepted);
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn applied_barriers_wait_for_actual_engine_backlog_and_rejected_pause_leaves_song_running() {
        let mut host = interactive_host(48_000); identified_load(&mut host, &minimal_mod(), 1, 1);
        host.player.playback_barrier(1).unwrap(); host.process(128);
        let identity = host.player.playback_identity();
        host.player.play().unwrap(); host.process(128);
        let before = host.player.telemetry().transport.song_frame;
        host.enqueue(WireCommand { identity, opcode: 14, argument: 0, extra: 0 });
        host.enqueue(WireCommand { identity: Some(PlaybackIdentity::default()), opcode: 23, argument: 0, extra: 1 });
        assert!(!host.drain_interactive_batch(true)); for _ in 0..8 { host.process(128); }
        assert_eq!(host.player.interactive_transport(), Some(starplayer::core::InteractiveTransport::Playing));
        assert!(host.player.telemetry().transport.song_frame > before);
        for index in 0..63 { host.player.set_master_volume(if index == 62 { U0F16::ZERO } else { U0F16::MAX }).unwrap(); }
        host.player.playback_barrier(2).unwrap(); host.process(128);
        assert_eq!(host.player.applied_playback().serial, 1, "Host drain does not imply Engine drain");
        host.enqueue(WireCommand { identity, opcode: 22, argument: 1, extra: 0 });
        assert!(!host.drain_interactive_batch(true), "existing Engine backlog consumes admission headroom");
        host.process(128);
        assert_eq!(host.player.applied_playback().serial, 2); assert!(host.player.applied_playback().accepted);
        assert!(host.interleaved[..256].iter().all(|sample| *sample == 0.0), "final zero gain reached actual rendered audio before ack");
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn applied_barrier_is_absent_initially_and_waits_for_deferred_loads() {
        let mut host = interactive_host(44_100);
        assert!(!host.player.applied_playback().accepted);
        assert_eq!(host.player.applied_playback().identity, None);
        for serial in 1..=14 {
            identified_load(&mut host, &minimal_mod(), serial, (1u64 << 54) + serial);
            host.player.playback_barrier(serial).unwrap();
        }
        assert_eq!(host.player.applied_playback().identity, None, "control shadow cannot acknowledge an installation");
        for _ in 0..24 { host.process(128); }
        assert!(host.player.applied_playback().serial < 14, "retirement pressure defers the whole load and its barrier");
        for _ in 0..30 { host.collect_garbage(); host.process(128); }
        let applied = host.player.applied_playback();
        assert_eq!(applied.serial, 14);
        assert_eq!(applied.identity, host.player.playback_identity());
        assert!(applied.accepted);
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn explicit_new_session_is_clean_stopped_before_readiness_from_every_old_transport() {
        for case in 0..4 {
            let bytes = if case == 2 { looping_mod() } else { minimal_mod() };
            let mut host = interactive_host(44_117);
            identified_load(&mut host, &bytes, 1, 1);
            host.player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap();
            host.player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }).unwrap();
            host.player.interactive(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))).unwrap();
            host.player.set_at_end(if case == 2 { AtEnd::FadeOut } else { AtEnd::Stop }).unwrap();
            host.player.set_fade_frames(10_000).unwrap(); host.player.play().unwrap();
            if case >= 2 {
                for _ in 0..4000 { host.process(128); if if case == 2 { host.player.is_fading() } else { host.player.natural_end() } { break; } }
                assert!(if case == 2 { host.player.is_fading() } else { host.player.natural_end() });
            } else { for _ in 0..8 { host.process(128); } }
            if case == 1 { host.player.pause().unwrap(); host.process(128); }
            host.collect_garbage();
            identified_load(&mut host, &minimal_mod(), 2, 2);
            // A late Stop for the old identity is rejected; installation itself must
            // remain stopped without relying on that old command for cancellation.
            assert!(!host.apply(WireCommand { identity: Some(PlaybackIdentity { session: SessionId(1), build: BuildGeneration(1) }), opcode: 16, argument: 0, extra: 0 }));
            host.process(128);
            assert_eq!(host.player.interactive_transport(), Some(starplayer::core::InteractiveTransport::Stopped), "case{case}");
            assert!(!host.player.natural_end()); assert!(!host.player.is_fading());
            host.player.playback_barrier(99).unwrap(); host.process(128);
            let saved = host.player.replacement_state().unwrap();
            assert_eq!(&saved[4..8], &[0, 0, 0, 0], "transport/end/overlap/destination reset case{case}");
            assert_eq!(saved[9], 32767); assert_eq!(saved[13], 10_000);
            assert_eq!(&saved[14..19], &[0, 0, 0, 0, 0]);
            assert!(saved[47..].iter().all(|word| *word == 0));
            let held_frame = saved[41];
            for _ in 0..8 { host.process(128); }
            assert_eq!(host.player.replacement_state().unwrap()[41], held_frame);
            assert_eq!(host.telemetry_words[2], 4, "actual tracker width, not reserved lanes");
            assert_eq!(host.scope_channels, 4);
            host.player.interactive(InteractiveAction::PreviewInstrument(InstrumentId(0))).unwrap();
            let mut peak = 0.0f32; for _ in 0..8 { peak = peak.max(host.process(128)); }
            assert!(peak > 0.001, "first preview after clean load case{case}");
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn wide_tracker_keeps_all_sixty_four_visible_lanes_with_separate_live_summary() {
        use starplayer::model::{InstrumentDef, ModuleBuilder, ModuleHeader, SampleSpec};
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[1000; 1024], SampleSpec::one_shot("wide audition")).unwrap();
        let mut instrument = InstrumentDef::from_sample("mapped", sample, U0F16::MAX);
        instrument.note_sample_map.fill(1);
        builder.add_instrument(instrument).unwrap();
        builder.add_pattern(&[0; 5 * 64], 1, 64).unwrap(); builder.set_orders(&[0]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Xm, 64));
        let mut host = interactive_host(44_100);
        host.player.load_module_identified(Arc::new(builder.build().unwrap()), PlaybackIdentity { session: SessionId(1), build: BuildGeneration(1) }).unwrap();
        host.player.playback_barrier(1).unwrap();
        for _ in 0..8 { host.process(128); }
        assert_eq!(host.telemetry_words[2], 64); assert_eq!(host.scope_channels, 64);
        host.player.interactive(InteractiveAction::PreviewInstrument(InstrumentId(0))).unwrap();
        assert!(host.process(128) > 0.001);
        assert_eq!(host.telemetry_words[2], 64);
        assert!(host.telemetry_words[TELEMETRY_LIVE_OFFSET] > 0, "live summary follows the fixed tracker record block");
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn same_player_panning_replacement_preserves_forward_seek_order_after_subsequent_ticks() {
        let mut bytes = minimal_mod();
        let pattern = bytes[1084..2108].to_vec(); bytes.splice(2108..2108, pattern);
        bytes[950] = 2; bytes[953] = 1;
        for quanta in [1, 7, 42, 47, 80] {
            let mut host = interactive_host(44_100); identified_load(&mut host, &bytes, 1, 1);
            host.player.play().unwrap(); host.player.playback_barrier(1).unwrap(); host.process(128);
            host.player.mute(ChannelId(0), true).unwrap(); host.player.seek_order(1).unwrap();
            for _ in 0..quanta { host.process(128); }
            let saved = host.player.replacement_state().unwrap();
            assert_eq!(saved[19], 1); assert_eq!(saved[25], 1);
            let (module, _, _) = decode(&bytes, true, 0, 88_200).unwrap();
            host.player.load_replacement(Arc::new(module), PlaybackIdentity { session: SessionId(1), build: BuildGeneration(2) }, &saved).unwrap();
            host.player.playback_barrier(2).unwrap();
            for index in 0..80 {
                host.process(128);
                let restored = host.player.replacement_state().unwrap();
                assert_eq!(restored[19], 1, "seek quanta{quanta}, rebuild quantum{index}, saved tick{} song{} next{}", saved[23], saved[41], saved[39]);
                assert_eq!(restored[25], 1, "sounding order must stay on the forward seek");
            }
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn fresh_player_replacement_preserves_exact_held_cursor_and_fractional_conversion() {
        let mut host = interactive_host(44_117);
        let bytes = minimal_mod();
        identified_load(&mut host, &bytes, 1u64 << 55, 1u64 << 56);
        host.player.play().unwrap();
        for _ in 0..7 { host.process(128); }
        host.player.pause().unwrap(); host.player.playback_barrier(1).unwrap(); host.process(128);
        let saved = host.player.replacement_state().unwrap();
        for rate in [44_117, 48_000] {
            let mut replacement = interactive_host(rate);
            let identity = PlaybackIdentity { session: SessionId(saved[1]), build: BuildGeneration(saved[2] + 1) };
            replacement.player.load_replacement(Arc::new(starplayer::load(&bytes).unwrap()), identity, &saved).unwrap();
            assert!(!replacement.player.applied_playback().accepted, "seeded identity is not applied state");
            replacement.player.playback_barrier(1).unwrap(); replacement.process(128);
            let restored = replacement.player.replacement_state().unwrap();
            assert_eq!(&restored[19..39], &saved[19..39]);
            let pending = (((saved[39] as u128) << 32) + saved[40] as u128) * rate as u128 / 44_117;
            assert_eq!(restored[39], (pending >> 32) as u64);
            assert_eq!(restored[40], (pending & 0xffff_ffff) as u64);
            assert_eq!(restored[41], saved[41] * rate as u64 / 44_117);
            assert_eq!(restored[4], 2);
            assert_eq!(restored[1], saved[1]);
            assert_eq!(restored[2], saved[2] + 1);
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn replacement_codec_rejects_malformed_records_before_installation() {
        let bytes = minimal_mod();
        let module = Arc::new(starplayer::load(&bytes).unwrap());
        let mut source = interactive_host(44_100);
        identified_load(&mut source, &bytes, 7, 12);
        source.player.play().unwrap(); source.process(128); source.player.pause().unwrap(); source.player.playback_barrier(1).unwrap(); source.process(128);
        let valid = source.player.replacement_state().unwrap();
        let identity = PlaybackIdentity { session: SessionId(7), build: BuildGeneration(13) };
        let corruptions = [(0, 99), (1, 8), (4, 3), (5, 2), (6, 2), (9, u32::MAX as u64), (10, 32768), (11, i32::MAX as u64), (21, 0), (23, u16::MAX as u64 + 1), (31, 6), (40, 1u64 << 32), (46, 0)];
        for (index, value) in corruptions {
            let mut words = valid.clone(); words[index] = value;
            let mut target = interactive_host(44_100);
            assert!(target.player.load_replacement(Arc::clone(&module), identity, &words).is_err(), "field {index}");
            assert!(target.player.module().is_none(), "field {index} installed partially");
            assert_eq!(target.player.playback_identity(), Some(PlaybackIdentity::default()));
        }
        for length in [0, 1, 46, valid.len() - 1] {
            let mut target = interactive_host(44_100);
            assert!(target.player.load_replacement(Arc::clone(&module), identity, &valid[..length]).is_err());
            assert!(target.player.module().is_none());
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn cleanup_invalidates_future_live_notes_but_preserves_new_input() {
        let mut bytes = minimal_mod(); bytes[48..50].copy_from_slice(&128u16.to_be_bytes());
        for action in [InteractiveAction::StopPreview, InteractiveAction::AllSoundOff, InteractiveAction::Stop, InteractiveAction::SetLiveDestination(LiveDestination::Jam)] {
            let mut host = interactive_host(44_100);
            identified_load(&mut host, &bytes, 1, 1);
            host.player.interactive(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))).unwrap(); host.process(128);
            host.player.send_event(0, starplayer::core::Event::NoteOn { note: starplayer::core::Note::new(60), velocity: U0F16::MAX }).unwrap();
            host.player.interactive(action).unwrap();
            for _ in 0..8 { assert_eq!(host.process(128), 0.0, "old future note survived {action:?}"); }
            host.player.send_event(0, starplayer::core::Event::NoteOn { note: starplayer::core::Note::new(60), velocity: U0F16::MAX }).unwrap();
            let mut peak = 0.0f32;
            for _ in 0..8 { peak = peak.max(host.process(128)); }
            assert!(peak > 0.01, "new input was lost after {action:?}");
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn note_wire_requires_audio_applied_load_and_full_replay_barrier() {
        let mut host = interactive_host(44_100);
        host.player.playback_barrier(1).unwrap(); host.process(128);
        assert!(!host.player.applied_playback().accepted, "seeded engine identity is not an installed rack");
        identified_load(&mut host, &minimal_mod(), 7, 1u64 << 54);
        let identity = host.player.playback_identity();
        let preview = WireCommand { identity, opcode: 20, argument: 0, extra: 0 };
        let selected_preview = WireCommand { identity, opcode: 25, argument: 0, extra: 61 };
        let note = WireCommand { identity, opcode: 24, argument: 0x7f_3c_90, extra: 0 };
        assert!(!host.apply(preview)); assert!(!host.apply(selected_preview)); assert!(!host.apply(note));
        host.process(128);
        assert!(!host.apply(note), "installation alone is not the full replay barrier");
        host.player.playback_barrier(2).unwrap(); host.process(128);
        assert!(host.apply(preview));
        assert!(host.apply(selected_preview));
        assert!(!host.apply(WireCommand { extra: 128, ..selected_preview }));
        assert!(host.apply(note));
        assert!(!host.apply(WireCommand { identity: None, opcode: OPCODE_MIDI_EVENT, argument: 0x7f_3c_90, extra: 0 }));
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn codec_round_trips_natural_end_active_fade_and_repeated_rows() {
        for case in 0..3 {
            let mut bytes = if case == 1 { looping_mod() } else { minimal_mod() };
            if case == 2 { bytes[1084..1088].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0xe, param: 0xe2 }.to_bytes()); }
            let mut source = interactive_host(44_117); identified_load(&mut source, &bytes, 9, 20);
            source.player.set_at_end(if case == 1 { AtEnd::FadeOut } else { AtEnd::Stop }).unwrap();
            source.player.set_fade_frames(10_000).unwrap(); source.player.play().unwrap();
            if case == 2 { for _ in 0..64 { source.process(128); } }
            else {
                for _ in 0..4000 { source.process(128); if if case == 1 { source.player.is_fading() } else { source.player.natural_end() } { break; } }
            }
            if case != 0 { if case == 1 { for _ in 0..4 { source.process(128); } } source.player.pause().unwrap(); }
            source.player.playback_barrier(1).unwrap(); source.process(128);
            let saved = source.player.replacement_state().unwrap();
            if case == 0 { assert_eq!(saved[5], 1); assert_eq!(saved[44], 1); }
            if case == 1 { assert_eq!(saved[14], 1); assert!(saved[15] > 0); }
            if case == 2 { assert_eq!(saved[22], 2); assert!(saved[24] > 0); }
            let mut target = interactive_host(44_117);
            target.player.load_replacement(Arc::new(starplayer::load(&bytes).unwrap()), PlaybackIdentity { session: SessionId(9), build: BuildGeneration(21) }, &saved).unwrap();
            target.player.playback_barrier(1).unwrap(); target.process(128);
            let restored = target.player.replacement_state().unwrap();
            assert_eq!(&restored[9..19], &saved[9..19], "transport case{case}");
            assert_eq!(&restored[19..42], &saved[19..42], "cursor case{case}");
            assert_eq!(restored[5], saved[5]);
        }
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn full_control_queue_cannot_ack_or_partially_install_replacement() {
        let bytes = minimal_mod();
        let mut source = interactive_host(44_100); identified_load(&mut source, &bytes, 1, 1);
        source.player.playback_barrier(1).unwrap(); source.process(128);
        let saved = source.player.replacement_state().unwrap();
        let mut target = interactive_host(44_100);
        for _ in 0..starplayer_host::HOST_COMMAND_CAPACITY { target.player.set_master_volume(U0F16::MAX).unwrap(); }
        assert!(target.player.playback_barrier(1).is_err());
        assert!(target.player.load_replacement(Arc::new(starplayer::load(&bytes).unwrap()), PlaybackIdentity { session: SessionId(1), build: BuildGeneration(2) }, &saved).is_err());
        assert!(target.player.module().is_none());
        assert!(!target.player.applied_playback().accepted);
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn cleanup_cancels_future_events_after_the_midi_sender_is_handed_away() {
        let mut host = interactive_host(44_100); identified_load(&mut host, &minimal_mod(), 1, 1);
        host.player.interactive(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))).unwrap(); host.process(128);
        let mut sender = host.player.take_event_sender().unwrap();
        let event = starplayer::core::Event::NoteOn { note: starplayer::core::Note::new(60), velocity: U0F16::MAX };
        sender.send_event(0, event).unwrap(); host.player.interactive(InteractiveAction::StopPreview).unwrap();
        for _ in 0..8 { assert_eq!(host.process(128), 0.0); }
        sender.send_event(0, event).unwrap();
        let mut peak = 0.0f32; for _ in 0..8 { peak = peak.max(host.process(128)); }
        assert!(peak > 0.001);
    }

    #[test]
    #[cfg(all(feature = "interactive-audition", feature = "note-overlap"))]
    fn explicit_preview_and_unique_setting_are_validated_before_atomic_group_admission() {
        let mut host = interactive_host(44_100); identified_load(&mut host, &minimal_mod(), 4, 7);
        host.player.playback_barrier(1).unwrap(); host.process(128);
        let identity = host.player.playback_identity();
        for wire in [WireCommand { identity, opcode: 26, argument: 1, extra: 0 }, WireCommand { identity, opcode: 25, argument: 0, extra: 61 }] { assert!(host.enqueue(wire)); }
        assert!(host.drain_interactive_batch(true)); host.player.playback_barrier(2).unwrap(); host.process(128);
        assert_eq!(*host.player.replacement_state().unwrap().last().unwrap(), 1, "unique setting is captured in replacement codec");
        assert!(host.enqueue(WireCommand { identity, opcode: 26, argument: 0, extra: 0 }));
        assert!(host.enqueue(WireCommand { identity, opcode: 25, argument: 65535, extra: 60 }));
        assert!(!host.drain_interactive_batch(true), "invalid selected instrument rejects the whole group before mutation");
        host.player.playback_barrier(3).unwrap(); host.process(128);
        assert_eq!(*host.player.replacement_state().unwrap().last().unwrap(), 1);
    }

}

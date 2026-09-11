//! The host-testable half of M8-I6's web API: the status JSON encoder, the WebSocket
//! telemetry word packer, the WebSocket command decoder, and the module-upload state
//! machine.
//!
//! Everything in this module is pure data transformation — no `picoserve` handler, no
//! GPIO, no flash. The board crate's `web.rs` wires these functions to actual HTTP routes
//! and an actual `embassy-net` socket; this module is what makes that wiring thin enough
//! to trust, because the interesting logic — "does this snapshot pack into the wire words
//! the page expects", "does a second upload while one is in flight get refused" — is
//! proven here under `cargo test -p starplayer-firmware-common`, not on a device nobody
//! can attach to CI.
//!
//! # The telemetry layout is copied, not shared
//!
//! [`TELEMETRY_HEADER_WORDS`], [`TELEMETRY_CHANNEL_WORDS`], the 22 header words' order and
//! the 8 channel words' order all mirror `crates/starplayer-host-wasm/src/lib.rs`
//! (`pack_telemetry`, and the `OPCODE_*`/`TELEMETRY_*`/`AT_END_*` constants above it)
//! **exactly** — that crate is the wire protocol's origin, since the browser page and its
//! `ring.js` were written against it first. It cannot be imported from here: it is a
//! `std` + `wasm-bindgen` crate outside this workspace, its layout constants are private,
//! and even if they were public this crate would still have to redeclare them because
//! `starplayer-host-wasm` is not on the dependency graph a `no_std` firmware crate can
//! join. So `crates/starplayer-host-wasm/src/lib.rs` is the **source of truth** for this
//! wire layout, this module's copy is the second, independent expression of it, and the
//! `a_snapshot_packs_into_the_wire_header_words_the_page_expects` test below (built from
//! literal indices, the same way the wasm host's own test would be) is what keeps the two
//! in step — a change to one without the other fails that test the next time this crate's
//! suite runs.
//!
//! One deliberate divergence: the wasm host always packs all 64 channel blocks into a
//! fixed-size `SharedArrayBuffer` because the page addresses it by a constant offset.
//! This host instead sends the WebSocket frame it actually needs — [`pack_telemetry`]
//! writes `channel_count.min(TELEMETRY_MAX_CHANNELS)` channel blocks and returns exactly
//! that many bytes, not the full 2,136-byte block — because a `no_std` board has no shared
//! memory to address by offset and every byte not sent is a byte the radio does not have
//! to carry.
//!
//! # Why `serde-json-core` and not `serde_json`
//!
//! `serde_json` allocates; the status and module-list bodies below serialise into a
//! caller-supplied `&mut [u8]` instead, which is what a `no_std` HTTP handler with a fixed
//! response buffer wants. `serde-json-core` is pinned to `0.6` specifically, not a newer
//! release, because `picoserve 0.18` — the HTTP server the board crate builds its routes
//! on — depends on `serde-json-core 0.6.0` and `heapless 0.8`; pinning here to the same
//! versions means the workspace resolves one copy of each rather than two.

use serde::{Deserialize, Serialize};

use starplayer::core::{AtEnd, U0F16};
use starplayer_telemetry::{Snapshot, SongEnd};

use crate::now_playing::{MAX_DISPLAY_CHANNELS, NowPlaying};

// ---------------------------------------------------------------------------------------
// Telemetry: the WebSocket word packer.
// ---------------------------------------------------------------------------------------

/// Header words before the first channel block — see `crates/starplayer-host-wasm/src/lib.rs`.
pub const TELEMETRY_HEADER_WORDS: usize = 22;
/// Words per channel block.
pub const TELEMETRY_CHANNEL_WORDS: usize = 8;
/// The widest pattern channel count any format in scope has — [`Snapshot::channels`]'s own
/// length, and the most channel blocks one telemetry frame ever carries.
pub const TELEMETRY_MAX_CHANNELS: usize = 64;
/// The most words one telemetry frame ever carries: the header plus every channel block.
pub const TELEMETRY_MAX_WORDS: usize = TELEMETRY_HEADER_WORDS + TELEMETRY_MAX_CHANNELS * TELEMETRY_CHANNEL_WORDS;
/// [`TELEMETRY_MAX_WORDS`] as bytes — the largest buffer [`pack_telemetry`] can ever fill,
/// and the size a caller with no smaller module in mind can safely allocate once.
pub const TELEMETRY_MAX_BYTES: usize = TELEMETRY_MAX_WORDS * 4;

/// What the device knows that a [`Snapshot`] does not: the control half's own state.
///
/// A [`Snapshot`] describes the engine's tracker state — transport position, per-channel
/// scalars — but the telemetry frame the page reads also carries a handful of fields that
/// live on [`ControlHalf`](starplayer_host_embedded::ControlHalf) or the board's own web
/// task instead, because they are host state rather than engine state: whether the
/// transport is actually running right now (`playing`), the master peak tap, how far
/// output has advanced, how much retirement backlog is waiting, and so on. This struct is
/// the board's one job when it calls [`pack_telemetry`]: read those fields off
/// `ControlHalf` and its own bookkeeping and hand them across.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HostState {
    pub playing: bool,
    /// [`ControlHalf::peak`](starplayer_host_embedded::ControlHalf::peak), `0..=i16::MAX`
    /// (that accessor already clamps out the top bit before handing the value over).
    pub peak: i16,
    pub output_frame: u64,
    pub pending_garbage: u16,
    /// Bumped by the board every time a different module is swapped in, so the page can
    /// tell "the same song, later" from "a different song".
    pub module_generation: u32,
    pub retired_collected: u32,
    pub master_volume: U0F16,
    /// [`ControlHalf::is_fading`](starplayer_host_embedded::ControlHalf::is_fading) — the
    /// wasm host reads this straight off its own `Player`; this host has no such accessor
    /// on [`Snapshot`] itself, so it rides in here instead. Packed into song-flags bit 3.
    pub fading: bool,
}

/// Scale a `0..=i16::MAX` peak reading to the `0..=65535` range word 16 and
/// [`Status::peak`] both use. `i16::MAX * 2 == 65534`, one short of full scale — the wasm
/// host reaches the same ceiling from a `0.0..=1.0` float multiplied by `65_535.0`; the
/// fixed-point route here saturates instead of rounding, but the two never disagree at
/// either end of the range, which is all either UI actually reads off it.
fn scaled_peak(peak: i16) -> u16 { (i32::from(peak).max(0) * 2).min(i32::from(u16::MAX)) as u16 }

/// Write one 32-bit little-endian word at `index` into `out`. Private: every caller here
/// already knows `out` is long enough, checked once up front in [`pack_telemetry`], so the
/// per-word write itself can stay a plain slice index rather than repeating the check.
fn put_word(out: &mut [u8], index: usize, value: i32) {
    let offset = index * 4;
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Pack one snapshot into the flat little-endian `i32` word block the page reads over the
/// WebSocket. Writes the 22 header words then
/// `snapshot.channel_count.min(TELEMETRY_MAX_CHANNELS)` eight-word channel blocks, in
/// [`Snapshot::channels`] order — not [`Snapshot::active_channels`] — so a channel index
/// in the page always means the engine's own channel index, never a compacted one.
///
/// Returns the number of bytes written, or `None` when `out` is too small for the words
/// this snapshot needs; nothing is written in that case.
pub fn pack_telemetry(snapshot: &Snapshot, host: &HostState, out: &mut [u8]) -> Option<usize> {
    let channel_count = (snapshot.channel_count as usize).min(TELEMETRY_MAX_CHANNELS);
    let word_count = TELEMETRY_HEADER_WORDS + channel_count * TELEMETRY_CHANNEL_WORDS;
    let byte_count = word_count * 4;
    if out.len() < byte_count {
        return None;
    }

    let transport = &snapshot.transport;
    let warnings = &snapshot.warnings;
    let warning_bits = (warnings.zero_advance_forced as i32)
        | ((warnings.event_limit_reached as i32) << 1)
        | ((warnings.retired_module_dropped as i32) << 2)
        | ((warnings.unsupported_command as i32) << 3)
        | ((warnings.late_events as i32) << 4)
        | ((warnings.retired_insert_dropped as i32) << 5);
    // Bit 0 mirrors the wasm host's own test: "length known" is read off `song_end`
    // rather than off `song_length_frames`, because a scan sets both together and
    // `song_end` is the field that actually distinguishes "never scanned" from "scanned".
    let song_flags = ((transport.song_end != SongEnd::Unknown) as i32)
        | (((transport.song_end == SongEnd::Loops) as i32) << 1)
        | ((transport.end_reached as i32) << 2)
        | ((host.fading as i32) << 3);

    let header = [
        snapshot.sequence as i32,
        snapshot.publishes_dropped as i32,
        snapshot.channel_count as i32,
        snapshot.voices_active as i32,
        transport.order as i32,
        transport.pattern as i32,
        transport.row as i32,
        transport.tick as i32,
        transport.speed as i32,
        transport.tempo_bpm as i32,
        transport.global_volume.to_bits() as i32,
        warning_bits,
        host.output_frame.min(i32::MAX as u64) as i32,
        host.pending_garbage as i32,
        host.module_generation as i32,
        host.playing as i32,
        scaled_peak(host.peak) as i32,
        host.retired_collected as i32,
        // This host has exactly one mixer mode (the fixed path with `Linear`) and no way
        // to change it, unlike the wasm host's `OPCODE_SET_MIXER_MODE`. The word is kept
        // anyway so the two layouts stay index-for-index identical.
        0,
        transport.song_frame.min(i32::MAX as u64) as i32,
        transport.song_length_frames.min(i32::MAX as u64) as i32,
        song_flags,
    ];
    for (index, word) in header.iter().enumerate() {
        put_word(out, index, *word);
    }

    for (index, channel) in snapshot.channels.iter().take(channel_count).enumerate() {
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
        for (offset, word) in values.iter().enumerate() {
            put_word(out, base + offset, *word);
        }
    }

    Some(byte_count)
}

// ---------------------------------------------------------------------------------------
// The WebSocket command decoder.
// ---------------------------------------------------------------------------------------

/// Bytes in one binary command frame: one opcode byte, then two little-endian `u32`s.
pub const WIRE_COMMAND_BYTES: usize = 9;

/// Wire opcodes, mirroring `crates/starplayer-host-wasm/src/lib.rs`'s `OPCODE_*` constants
/// for the subset this host acts on. `OPCODE_SET_MIXER_MODE` (7), `OPCODE_MIDI_EVENT`
/// (10), `OPCODE_INSERT_PARAM` (11) and `OPCODE_INSERT_BYPASS` (12) are deliberately
/// absent: this milestone's board has one fixed mixer path and no insert-effect rack, so
/// there is nothing on this host for those opcodes to do.
pub mod opcode {
    pub const PLAY: u8 = 1;
    pub const STOP: u8 = 2;
    pub const SEEK_ORDER: u8 = 3;
    pub const SEEK_ROW: u8 = 4;
    pub const MASTER_VOLUME: u8 = 5;
    pub const MUTE_CHANNEL: u8 = 6;
    /// Seek to an elapsed position in the song. `argument` is a song frame.
    pub const SEEK_FRAME: u8 = 8;
    /// What to do at the detected loop point. `argument` is one of the `AT_END_*`
    /// constants; `extra` is the fade length in frames.
    pub const AT_END: u8 = 9;
}

/// The decoded form of one nine-byte binary WebSocket frame.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireCommand {
    pub opcode: u8,
    pub argument: u32,
    pub extra: u32,
}

/// Decode one nine-byte binary frame: opcode, then `argument` and `extra` as
/// little-endian `u32`s. `None` for any other length — the caller drops the frame rather
/// than guessing at a truncated or padded one.
pub fn decode_wire_command(frame: &[u8]) -> Option<WireCommand> {
    match frame {
        &[opcode, a0, a1, a2, a3, e0, e1, e2, e3] => {
            Some(WireCommand { opcode, argument: u32::from_le_bytes([a0, a1, a2, a3]), extra: u32::from_le_bytes([e0, e1, e2, e3]) })
        }
        _ => None,
    }
}

/// Wire spelling of [`AtEnd`], matching `crates/starplayer-host-wasm/src/lib.rs`'s
/// `AT_END_*` constants exactly, chosen there so the page's default repeat-off value is
/// zero.
pub const AT_END_FADE_OUT: u32 = 0;
pub const AT_END_CONTINUE: u32 = 1;
pub const AT_END_STOP: u32 = 2;

/// Decode `OPCODE_AT_END`'s `argument` into an [`AtEnd`]. `None` for anything else, so the
/// caller can answer 400 rather than silently falling back to a default — unlike
/// [`AtEndSlot::get`](starplayer_host_embedded::source::AtEndSlot::get), which decodes the
/// same three values for a different purpose (an atomically shared slot that must always
/// hold *some* valid `AtEnd`) and so falls back to `FadeOut` instead of failing.
pub fn at_end_from_wire(value: u32) -> Option<AtEnd> {
    match value {
        AT_END_FADE_OUT => Some(AtEnd::FadeOut),
        AT_END_CONTINUE => Some(AtEnd::Continue),
        AT_END_STOP => Some(AtEnd::Stop),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// Status JSON.
// ---------------------------------------------------------------------------------------

/// One channel row, as the page's table renders it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatusChannel<'a> {
    pub instrument: u8,
    pub note: &'a str,
    /// Peak-hold VU level on [`ChannelRow`](crate::now_playing::ChannelRow)'s own
    /// `0..=16` bar scale, not the `0..=65535` scale [`Status::peak`] uses — this is a
    /// small table cell, not a meter.
    pub vu: u8,
    pub effect: &'a str,
    pub active: bool,
}

/// `GET /api/status`'s body.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status<'a> {
    pub title: &'a str,
    pub format: &'a str,
    pub source: &'a str,
    pub playing: bool,
    pub order: u16,
    pub pattern: u16,
    pub row: u16,
    pub speed: u8,
    pub bpm: u16,
    pub volume: u16,
    pub voices: u16,
    pub channel_count: u8,
    pub elapsed: u32,
    pub total: Option<u32>,
    pub peak: u16,
    pub loops: bool,
    pub end_reached: bool,
    pub warned: bool,
    pub channels: heapless::Vec<StatusChannel<'a>, MAX_DISPLAY_CHANNELS>,
}

impl<'a> Status<'a> {
    /// Borrow a [`NowPlaying`]'s note bytes and effect names rather than copying them —
    /// `view` and the returned `Status` share the borrow that made `view` itself, and
    /// `format`/`source` are named separately because neither lives on `NowPlaying`
    /// (format is chosen by the loader, `source` by which flash slot or the compiled-in
    /// image supplied the bytes).
    pub fn new(view: &'a NowPlaying, host: &HostState, format: &'a str, source: &'a str) -> Status<'a> {
        let mut channels = heapless::Vec::new();
        for row in view.displayed_channels() {
            // `NowPlaying::from_snapshot`'s `note_label` only ever writes ASCII letters,
            // digits and `.`, so this never actually falls back — see `FixedStr::as_str`
            // for the same reasoning applied to the title.
            let note = core::str::from_utf8(&row.note).unwrap_or("...");
            let _ = channels.push(StatusChannel { instrument: row.instrument, note, vu: row.vu, effect: row.effect_name, active: row.active });
        }
        Status {
            title: view.title.as_str(),
            format,
            source,
            playing: host.playing,
            order: view.order,
            pattern: view.pattern,
            row: view.row,
            speed: view.speed,
            bpm: view.tempo_bpm,
            volume: host.master_volume.to_bits(),
            voices: view.voices_active,
            channel_count: view.channel_count,
            elapsed: view.elapsed_seconds,
            total: view.total_seconds,
            peak: scaled_peak(host.peak),
            loops: view.loops,
            end_reached: view.end_reached,
            warned: view.warned,
            channels,
        }
    }
}

/// Serialise into a fixed buffer. `None` when the buffer is too small — the caller answers
/// 500 rather than truncating a body it has already promised a `Content-Length` for.
pub fn write_status_json(out: &mut [u8], status: &Status) -> Option<usize> { serde_json_core::to_slice(status, out).ok() }

/// One entry in the module picker: the compiled-in image (`id` 0) or a flash slot
/// (`id` 1..=N).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleEntry<'a> {
    pub id: u8,
    pub name: &'a str,
    pub bytes: u32,
    pub source: &'a str,
    pub current: bool,
}

/// Serialise the module list as a JSON array. `None` when the buffer is too small — same
/// contract as [`write_status_json`].
pub fn write_modules_json(out: &mut [u8], entries: &[ModuleEntry]) -> Option<usize> { serde_json_core::to_slice(entries, out).ok() }

/// `POST /api/seek`'s body.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct SeekRequest {
    pub order: u16,
}

/// `POST /api/volume`'s body. `level` is a [`U0F16`]'s raw bits, the same unit
/// [`Status::volume`] reports.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct VolumeRequest {
    pub level: u16,
}

/// `POST /api/mute`'s body.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct MuteRequest {
    pub channel: u16,
    pub muted: bool,
}

/// `POST /api/module`'s body: select an already-uploaded flash slot (or the compiled-in
/// image, `id` 0) as the module to play, without uploading new bytes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct SlotRequest {
    pub id: u8,
}

// ---------------------------------------------------------------------------------------
// The upload state machine.
// ---------------------------------------------------------------------------------------

/// Why a call into [`Upload`] was refused.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UploadError {
    /// An upload is already in flight — the handler should answer 409.
    Busy,
    /// [`Upload::reserve`] was asked to stage a zero-length body.
    Empty,
    /// [`Upload::reserve`]'s `expected` exceeds the staging buffer's `capacity`.
    TooLarge,
    /// [`Upload::append`] would carry the received count past `expected` — the handler
    /// must stop reading the request body and call [`Upload::abort`].
    Overrun,
    /// [`Upload::complete`] was called before `expected` bytes had arrived, or
    /// [`Upload::append`]/[`Upload::complete`] was called on an idle machine — see those
    /// methods for why the two situations share one variant.
    Incomplete,
}

/// Idle, or claimed for a body of `expected` bytes with `received` counted so far.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
enum UploadState {
    #[default]
    Idle,
    InFlight { expected: usize, received: usize },
}

/// The bookkeeping side of a module upload: no buffers, no I/O, just enough state that
/// "one upload at a time" and "the body I was promised is the body I got" are host-tested
/// facts rather than device-only ones. Modelled on [`KeyDebounce`](crate::keys::KeyDebounce) —
/// a small state machine fed plain values, proven with a synthetic sequence of calls
/// rather than a live socket.
///
/// The board's HTTP handler owns the actual staging buffer (a fixed region of PSRAM or
/// flash-write scratch); this type only ever answers "is that allowed right now", so it
/// stays `Copy` and carries nothing the handler could double-free or alias.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Upload {
    state: UploadState,
}

impl Upload {
    /// Idle, nothing claimed.
    pub const fn new() -> Upload { Upload { state: UploadState::Idle } }

    /// Claim the staging buffer for a body of `expected` bytes. [`UploadError::Busy`] when
    /// one is already in flight, [`UploadError::Empty`] for a zero-length body,
    /// [`UploadError::TooLarge`] when `expected` exceeds `capacity`.
    pub fn reserve(&mut self, expected: usize, capacity: usize) -> Result<(), UploadError> {
        if self.in_flight() {
            return Err(UploadError::Busy);
        }
        if expected == 0 {
            return Err(UploadError::Empty);
        }
        if expected > capacity {
            return Err(UploadError::TooLarge);
        }
        self.state = UploadState::InFlight { expected, received: 0 };
        Ok(())
    }

    /// Record `len` bytes appended. [`UploadError::Overrun`] when that would pass
    /// `expected` — the handler must stop reading and abort. [`UploadError::Incomplete`]
    /// on an idle machine: there is no upload to append to, and that is exactly the state
    /// [`Upload::complete`] also reports as `Incomplete`, so a caller that mismatches its
    /// own reserve/append pairing gets one consistent error rather than two.
    pub fn append(&mut self, len: usize) -> Result<(), UploadError> {
        match self.state {
            UploadState::Idle => Err(UploadError::Incomplete),
            UploadState::InFlight { expected, received } => {
                let received = received + len;
                if received > expected {
                    return Err(UploadError::Overrun);
                }
                self.state = UploadState::InFlight { expected, received };
                Ok(())
            }
        }
    }

    /// Finish. [`UploadError::Incomplete`] when fewer than `expected` bytes arrived — the
    /// machine stays in flight so the handler can keep calling [`Upload::append`] rather
    /// than losing the count, unlike a successful completion, which resets to idle and
    /// returns the byte count. Also `Incomplete` on an idle machine, for the reason
    /// [`Upload::append`] documents.
    pub fn complete(&mut self) -> Result<usize, UploadError> {
        match self.state {
            UploadState::Idle => Err(UploadError::Incomplete),
            UploadState::InFlight { expected, received } => {
                if received < expected {
                    return Err(UploadError::Incomplete);
                }
                self.state = UploadState::Idle;
                Ok(received)
            }
        }
    }

    /// Give the staging buffer back after a failure. Idempotent: aborting an already-idle
    /// machine is not an error.
    pub fn abort(&mut self) { self.state = UploadState::Idle; }

    pub const fn in_flight(&self) -> bool { matches!(self.state, UploadState::InFlight { .. }) }

    pub const fn received(&self) -> usize {
        match self.state {
            UploadState::Idle => 0,
            UploadState::InFlight { received, .. } => received,
        }
    }

    pub const fn expected(&self) -> usize {
        match self.state {
            UploadState::Idle => 0,
            UploadState::InFlight { expected, .. } => expected,
        }
    }
}

#[cfg(test)]
mod tests {
    use starplayer::core::{I1F15, Note};
    use starplayer_telemetry::{ChannelState, EffectDisplay, TransportState, WarningFlags};

    use super::*;
    use crate::now_playing::{ChannelRow, FixedStr};

    // -- telemetry packer ----------------------------------------------------------------

    fn word_at(bytes: &[u8], index: usize) -> i32 { i32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap()) }

    fn distinctive_snapshot() -> Snapshot {
        let mut snapshot = Snapshot {
            sequence: 9_001,
            publishes_dropped: 3,
            channel_count: 2,
            voices_active: 5,
            transport: TransportState {
                order: 11,
                pattern: 22,
                row: 33,
                tick: 4,
                speed: 6,
                tempo_bpm: 125,
                global_volume: U0F16::from_bits(40_000),
                song_frame: 3_969_000,
                song_length_frames: 8_820_000,
                song_end: SongEnd::Loops,
                end_reached: true,
            },
            warnings: WarningFlags {
                zero_advance_forced: true,
                event_limit_reached: false,
                retired_module_dropped: true,
                unsupported_command: false,
                late_events: true,
                retired_insert_dropped: false,
            },
            ..Snapshot::IDLE
        };
        snapshot.channels[0] = ChannelState {
            note: Some(Note::MIDDLE_C),
            instrument: 3,
            volume: U0F16::from_bits(20_000),
            pan: I1F15::from_bits(-1_000),
            effect: EffectDisplay::raw(6, 0x20).with_name("change speed"),
            vu_level: U0F16::from_bits(50_000),
            active: true,
            muted: false,
        };
        snapshot.channels[1] = ChannelState { active: false, muted: true, ..ChannelState::SILENT };
        snapshot
    }

    fn distinctive_host() -> HostState {
        HostState { playing: true, peak: 16_000, output_frame: 123_456, pending_garbage: 7, module_generation: 42, retired_collected: 9, master_volume: U0F16::from_bits(60_000), fading: true }
    }

    #[test]
    fn a_snapshot_packs_into_the_wire_header_words_the_page_expects() {
        let snapshot = distinctive_snapshot();
        let host = distinctive_host();
        let mut buffer = [0u8; TELEMETRY_MAX_BYTES];
        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();

        assert_eq!(word_at(&buffer, 0), 9_001, "word 0 is sequence");
        assert_eq!(word_at(&buffer, 1), 3, "word 1 is publishes_dropped");
        assert_eq!(word_at(&buffer, 2), 2, "word 2 is channel_count");
        assert_eq!(word_at(&buffer, 3), 5, "word 3 is voices_active");
        assert_eq!(word_at(&buffer, 4), 11, "word 4 is transport.order");
        assert_eq!(word_at(&buffer, 5), 22, "word 5 is transport.pattern");
        assert_eq!(word_at(&buffer, 6), 33, "word 6 is transport.row");
        assert_eq!(word_at(&buffer, 7), 4, "word 7 is transport.tick");
        assert_eq!(word_at(&buffer, 8), 6, "word 8 is transport.speed");
        assert_eq!(word_at(&buffer, 9), 125, "word 9 is transport.tempo_bpm");
        assert_eq!(word_at(&buffer, 10), 40_000, "word 10 is transport.global_volume bits");
        assert_eq!(word_at(&buffer, 11), 0b010101, "word 11 is the warnings bitfield");
        assert_eq!(word_at(&buffer, 12), 123_456, "word 12 is host.output_frame");
        assert_eq!(word_at(&buffer, 13), 7, "word 13 is host.pending_garbage");
        assert_eq!(word_at(&buffer, 14), 42, "word 14 is host.module_generation");
        assert_eq!(word_at(&buffer, 15), 1, "word 15 is host.playing");
        assert_eq!(word_at(&buffer, 16), 32_000, "word 16 is the scaled peak");
        assert_eq!(word_at(&buffer, 17), 9, "word 17 is host.retired_collected");
        assert_eq!(word_at(&buffer, 18), 0, "word 18 is the reserved mixer-mode slot, always 0 on this host");
        assert_eq!(word_at(&buffer, 19), 3_969_000, "word 19 is transport.song_frame");
        assert_eq!(word_at(&buffer, 20), 8_820_000, "word 20 is transport.song_length_frames");
        assert_eq!(word_at(&buffer, 21), 0b1111, "word 21 is the song-flags bitfield");
    }

    #[test]
    fn a_channel_block_packs_its_eight_words_in_order() {
        let snapshot = distinctive_snapshot();
        let host = distinctive_host();
        let mut buffer = [0u8; TELEMETRY_MAX_BYTES];
        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();

        let base = TELEMETRY_HEADER_WORDS;
        assert_eq!(word_at(&buffer, base), 60, "channel word 0 is the note semitone");
        assert_eq!(word_at(&buffer, base + 1), 3, "channel word 1 is instrument");
        assert_eq!(word_at(&buffer, base + 2), 20_000, "channel word 2 is volume bits");
        assert_eq!(word_at(&buffer, base + 3), -1_000, "channel word 3 is pan bits");
        assert_eq!(word_at(&buffer, base + 4), 6, "channel word 4 is effect.code");
        assert_eq!(word_at(&buffer, base + 5), 0x20, "channel word 5 is effect.param");
        assert_eq!(word_at(&buffer, base + 6), 50_000, "channel word 6 is vu_level bits");
        assert_eq!(word_at(&buffer, base + 7), 0b01, "channel word 7 is flags: active set, muted clear");

        let second = base + TELEMETRY_CHANNEL_WORDS;
        assert_eq!(word_at(&buffer, second), -1, "a channel with no note reports -1");
        assert_eq!(word_at(&buffer, second + 7), 0b10, "flags: active clear, muted set");
    }

    #[test]
    fn pack_telemetry_writes_exactly_the_header_plus_one_block_per_channel() {
        let snapshot = distinctive_snapshot();
        let host = distinctive_host();
        let mut buffer = [0u8; TELEMETRY_MAX_BYTES];
        let written = pack_telemetry(&snapshot, &host, &mut buffer).unwrap();
        assert_eq!(written, (TELEMETRY_HEADER_WORDS + 2 * TELEMETRY_CHANNEL_WORDS) * 4);
    }

    #[test]
    fn pack_telemetry_refuses_a_buffer_too_small_for_the_snapshot() {
        let snapshot = distinctive_snapshot();
        let host = distinctive_host();
        let needed = (TELEMETRY_HEADER_WORDS + 2 * TELEMETRY_CHANNEL_WORDS) * 4;
        let mut buffer = alloc::vec![0u8; needed - 1];
        assert_eq!(pack_telemetry(&snapshot, &host, &mut buffer), None);
    }

    #[test]
    fn the_peak_scale_reaches_65534_at_full_scale_and_0_at_silence() {
        assert_eq!(scaled_peak(i16::MAX), 65_534);
        assert_eq!(scaled_peak(0), 0);
    }

    #[test]
    fn song_flags_report_length_known_looping_end_reached_and_fading_independently() {
        let mut snapshot = Snapshot::IDLE;
        let mut host = HostState::default();
        let mut buffer = [0u8; TELEMETRY_MAX_BYTES];

        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();
        assert_eq!(word_at(&buffer, 21), 0, "an unscanned song has no flags set");

        snapshot.transport.song_end = SongEnd::Stops;
        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();
        assert_eq!(word_at(&buffer, 21), 0b0001, "a scanned song reports length known even when it does not loop");

        snapshot.transport.song_end = SongEnd::Loops;
        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();
        assert_eq!(word_at(&buffer, 21), 0b0011, "a looping song reports both bits");

        snapshot.transport.end_reached = true;
        host.fading = true;
        pack_telemetry(&snapshot, &host, &mut buffer).unwrap();
        assert_eq!(word_at(&buffer, 21), 0b1111, "all four bits can be set together");
    }

    // -- command decoder ------------------------------------------------------------------

    #[test]
    fn a_nine_byte_frame_round_trips_through_decode_wire_command() {
        let frame = [opcode::MASTER_VOLUME, 0x01, 0x02, 0x03, 0x04, 0xAA, 0xBB, 0xCC, 0xDD];
        let command = decode_wire_command(&frame).unwrap();
        assert_eq!(command, WireCommand { opcode: opcode::MASTER_VOLUME, argument: 0x0403_0201, extra: 0xDDCC_BBAA });
    }

    #[test]
    fn a_frame_of_the_wrong_length_is_rejected() {
        assert_eq!(decode_wire_command(&[0u8; 8]), None, "one byte short of a full frame");
        assert_eq!(decode_wire_command(&[0u8; 10]), None, "one byte past a full frame");
    }

    #[test]
    fn at_end_from_wire_decodes_the_three_known_values_and_rejects_the_rest() {
        assert_eq!(at_end_from_wire(AT_END_FADE_OUT), Some(AtEnd::FadeOut));
        assert_eq!(at_end_from_wire(AT_END_CONTINUE), Some(AtEnd::Continue));
        assert_eq!(at_end_from_wire(AT_END_STOP), Some(AtEnd::Stop));
        assert_eq!(at_end_from_wire(3), None);
    }

    // -- status JSON ------------------------------------------------------------------------

    fn distinctive_now_playing() -> NowPlaying {
        let mut channels = [ChannelRow::default(); MAX_DISPLAY_CHANNELS];
        channels[0] = ChannelRow { instrument: 2, note: *b"C-5", vu: 8, effect_name: "vibrato", active: true };
        NowPlaying {
            title: FixedStr::new("Ace"),
            order: 1,
            pattern: 2,
            row: 3,
            speed: 6,
            tempo_bpm: 125,
            volume: U0F16::MAX,
            voices_active: 4,
            channel_count: 1,
            elapsed_seconds: 10,
            total_seconds: Some(20),
            end_reached: false,
            loops: true,
            warned: false,
            channels,
        }
    }

    #[test]
    fn write_status_json_matches_the_documented_wire_shape_exactly() {
        let view = distinctive_now_playing();
        let host = HostState { playing: true, peak: 100, master_volume: U0F16::MAX, ..HostState::default() };
        let status = Status::new(&view, &host, "S3M", "flash");

        let mut buffer = [0u8; 512];
        let written = write_status_json(&mut buffer, &status).unwrap();
        let json = core::str::from_utf8(&buffer[..written]).unwrap();

        assert_eq!(
            json,
            "{\"title\":\"Ace\",\"format\":\"S3M\",\"source\":\"flash\",\"playing\":true,\"order\":1,\"pattern\":2,\
             \"row\":3,\"speed\":6,\"bpm\":125,\"volume\":65535,\"voices\":4,\"channel_count\":1,\"elapsed\":10,\
             \"total\":20,\"peak\":200,\"loops\":true,\"end_reached\":false,\"warned\":false,\"channels\":\
             [{\"instrument\":2,\"note\":\"C-5\",\"vu\":8,\"effect\":\"vibrato\",\"active\":true}]}"
        );
    }

    #[test]
    fn write_status_json_returns_none_for_a_buffer_too_small_to_hold_the_body() {
        let view = distinctive_now_playing();
        let host = HostState::default();
        let status = Status::new(&view, &host, "S3M", "flash");
        let mut buffer = [0u8; 4];
        assert_eq!(write_status_json(&mut buffer, &status), None);
    }

    // -- upload state machine --------------------------------------------------------------

    #[test]
    fn reserve_on_an_idle_machine_claims_it() {
        let mut upload = Upload::new();
        assert!(!upload.in_flight());
        assert_eq!(upload.reserve(100, 1_000), Ok(()));
        assert!(upload.in_flight());
        assert_eq!(upload.expected(), 100);
        assert_eq!(upload.received(), 0);
    }

    #[test]
    fn a_second_reserve_while_one_is_in_flight_is_busy() {
        let mut upload = Upload::new();
        assert_eq!(upload.reserve(100, 1_000), Ok(()));
        assert_eq!(upload.reserve(50, 1_000), Err(UploadError::Busy), "a second upload while one is in flight is refused, not queued");
    }

    #[test]
    fn a_zero_length_reserve_is_rejected_as_empty() {
        let mut upload = Upload::new();
        assert_eq!(upload.reserve(0, 1_000), Err(UploadError::Empty));
        assert!(!upload.in_flight(), "a refused reserve does not claim the machine");
    }

    #[test]
    fn a_reserve_past_the_staging_capacity_is_rejected_as_too_large() {
        let mut upload = Upload::new();
        assert_eq!(upload.reserve(1_001, 1_000), Err(UploadError::TooLarge));
        assert!(!upload.in_flight());
    }

    #[test]
    fn append_accumulates_the_received_count() {
        let mut upload = Upload::new();
        upload.reserve(100, 1_000).unwrap();
        assert_eq!(upload.append(40), Ok(()));
        assert_eq!(upload.append(40), Ok(()));
        assert_eq!(upload.received(), 80);
    }

    #[test]
    fn append_past_the_expected_length_is_an_overrun() {
        let mut upload = Upload::new();
        upload.reserve(100, 1_000).unwrap();
        upload.append(90).unwrap();
        assert_eq!(upload.append(20), Err(UploadError::Overrun));
    }

    #[test]
    fn complete_returns_the_byte_count_and_leaves_the_machine_idle() {
        let mut upload = Upload::new();
        upload.reserve(100, 1_000).unwrap();
        upload.append(100).unwrap();
        assert_eq!(upload.complete(), Ok(100));
        assert!(!upload.in_flight());
        assert_eq!(upload.reserve(50, 1_000), Ok(()), "a completed upload frees the machine for a new one");
    }

    #[test]
    fn complete_before_the_expected_count_arrives_is_incomplete_and_stays_in_flight() {
        let mut upload = Upload::new();
        upload.reserve(100, 1_000).unwrap();
        upload.append(40).unwrap();
        assert_eq!(upload.complete(), Err(UploadError::Incomplete));
        assert!(upload.in_flight(), "an incomplete upload is not silently discarded");
        assert_eq!(upload.received(), 40);
    }

    #[test]
    fn abort_frees_the_staging_buffer_for_a_new_reserve() {
        let mut upload = Upload::new();
        upload.reserve(100, 1_000).unwrap();
        upload.append(40).unwrap();
        upload.abort();
        assert!(!upload.in_flight());
        assert_eq!(upload.reserve(200, 1_000), Ok(()));
    }

    #[test]
    fn abort_on_an_idle_machine_is_not_an_error() {
        let mut upload = Upload::new();
        upload.abort();
        assert!(!upload.in_flight());
    }

    #[test]
    fn append_and_complete_on_an_idle_machine_both_report_incomplete() {
        let mut upload = Upload::new();
        assert_eq!(upload.append(1), Err(UploadError::Incomplete));
        assert_eq!(upload.complete(), Err(UploadError::Incomplete));
    }
}

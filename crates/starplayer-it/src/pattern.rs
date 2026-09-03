//! Impulse Tracker's **native** pattern cell, the unpacker that produces it, the read-side
//! view over a loaded [`Module`]'s blob, and the [`PatternData`] seam the sequencer reads
//! rows through.
//!
//! # The stored form
//!
//! A pattern in the file is a packed byte stream with five kinds of memory — a per-channel
//! *mask* memory and one memory each for note, instrument, volume and command — and a `0`
//! byte per row. A pattern in [`Module::blob`](starplayer_model::Module::blob) is a **fixed
//! stride** array of [`ItCell`]s: `rows` rows of `channels` cells, row-major,
//! [`CELL_BYTES`] bytes each, with every memory already resolved. The effect processor
//! indexes it as `row * channels + channel` and never walks a variable-length stream on the
//! audio thread.
//!
//! This is *not* a lowering into a shared cell type (design goal 7). [`ItCell`] is IT's own
//! encoding, byte for byte — an IT note byte, an IT volume-column byte, an IT command
//! number — and only [`ItCell::display`] ever leaves it.
//!
//! # The note byte, and the one value that is ours
//!
//! ITTECH.TXT gives the note column three sentinels and one range:
//!
//! | Byte | Meaning |
//! |---|---|
//! | `0..=119` | C-0 to B-9 |
//! | `254` | note cut |
//! | `255` | note off |
//! | anything else | note fade — "already programmed into IT's player but not available in the editor" |
//!
//! So every byte from 120 to 253 means the same thing, and the unpacker normalises them
//! all to [`NOTE_FADE`] (253) — which is what libxmp's `it_load.c` does with them too.
//! That frees exactly one byte for "this cell has no note at all", which the file itself
//! spells as the absence of the note bit in the mask rather than as a value:
//! [`NOTE_NONE`] is **252**, the byte immediately below the fade sentinel, and it can
//! never collide with a file's own value because 252 normalises to 253.

use alloc::vec;
use alloc::vec::Vec;
use starplayer_engine::{OrderEntry, PatternData};
use starplayer_model::{
    EffectCell, EffectNames, Module, NoteCell, OrderEntry as ModelOrderEntry, PatternCell, PatternId,
};
use starplayer_rt::Arc;

/// Bytes one [`ItCell`] occupies in the module blob.
pub const CELL_BYTES: usize = 5;

/// Rows an IT pattern has when its parapointer is zero, and Impulse Tracker's own default.
pub const DEFAULT_ROWS: u16 = 64;

/// Rows Impulse Tracker itself allows in a pattern (ITTECH.TXT: "Ranges from 32->200").
pub const SPEC_MAX_ROWS: u16 = 200;

/// Rows this loader accepts, research point 5: ModPlug Tracker 1.16 writes up to 256 into
/// an `.it`, and OpenMPT's larger limits belong to `.mptm`. A pattern claiming more is
/// *skipped* rather than truncated — it becomes an empty [`DEFAULT_ROWS`]-row pattern, the
/// same as OpenMPT's and libxmp's own out-of-range handling — because truncating would
/// reinterpret the packed stream's row terminators as channel bytes.
pub const MAX_ROWS: u16 = 256;

/// Channels the packed stream can name: the channel byte's low seven bits, minus one.
///
/// Impulse Tracker's own unpacker masks this to six bits, and so does libxmp; OpenMPT
/// deliberately does not, which is how it reads the wider patterns ModPlug wrote. The
/// per-channel memories are sized for OpenMPT's range so that a byte above 64 is *parsed*
/// with its own memory rather than aliasing channel 0's.
pub const MAX_PACKED_CHANNELS: usize = 127;

/// Channels an IT **song** has, which is what the `ChnPan` and `ChnVol` tables size.
pub const MAX_CHANNELS: u8 = 64;

/// Note byte meaning "this cell has no note". See the [module documentation](self).
pub const NOTE_NONE: u8 = 252;

/// Note byte meaning "fade the voice out" — every note byte from 120 to 253 in the file.
pub const NOTE_FADE: u8 = 253;

/// Note byte meaning "cut the voice now" — IT's `^^^`.
pub const NOTE_CUT: u8 = 254;

/// Note byte meaning "release the voice" — IT's `===`.
pub const NOTE_OFF: u8 = 255;

/// Highest playable note byte: B-9.
pub const NOTE_MAX: u8 = 119;

/// Instrument byte meaning "this cell has no instrument". Instruments and samples are both
/// one-based in the file, so zero is the empty column.
pub const INSTRUMENT_NONE: u8 = 0;

/// Volume-column byte meaning "this cell has no volume column".
///
/// The format's own ranges stop at 232, so 255 is free. A file that writes 255 there has
/// written a value no tracker defines, and it reads back as an empty column.
pub const VOLUME_NONE: u8 = 255;

/// Command byte meaning "this cell has no effect". Command codes are 1..=26 for `A`..=`Z`.
pub const COMMAND_NONE: u8 = 0;

/// Highest command code the IT pattern format defines (ITTECH.TXT: "Valid ranges from
/// 0->31"). Codes 27..=31 have no letter and no meaning; they are stored as the file
/// spelled them.
pub const COMMAND_MAX: u8 = 31;

// ── the packed stream's mask bits ───────────────────────────────────────────────────

/// Channel byte bit 7 — a new mask byte follows.
pub const CHANNEL_HAS_MASK: u8 = 0x80;
/// Mask bit 0 — a note byte follows.
pub const MASK_NOTE: u8 = 0x01;
/// Mask bit 1 — an instrument byte follows.
pub const MASK_INSTRUMENT: u8 = 0x02;
/// Mask bit 2 — a volume/panning byte follows.
pub const MASK_VOLUME: u8 = 0x04;
/// Mask bit 3 — a command byte and a parameter byte follow.
pub const MASK_COMMAND: u8 = 0x08;
/// Mask bit 4 — reuse the channel's last note.
pub const MASK_LAST_NOTE: u8 = 0x10;
/// Mask bit 5 — reuse the channel's last instrument.
pub const MASK_LAST_INSTRUMENT: u8 = 0x20;
/// Mask bit 6 — reuse the channel's last volume column.
pub const MASK_LAST_VOLUME: u8 = 0x40;
/// Mask bit 7 — reuse the channel's last command and parameter.
pub const MASK_LAST_COMMAND: u8 = 0x80;

/// Bytes that follow a mask, indexed by its low nybble. OpenMPT's `maskToSkips`.
const MASK_TO_SKIPS: [u8; 16] = [0, 1, 1, 2, 1, 2, 2, 3, 2, 3, 3, 4, 3, 4, 4, 5];

/// The two-character note names an IT pattern view prints.
pub const NOTE_NAMES: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];

/// One cell of one channel of one row, in Impulse Tracker's own encoding.
///
/// # Encoding
///
/// | Field | Empty | Otherwise |
/// |---|---|---|
/// | `note` | [`NOTE_NONE`] (252) | `0..=119`, [`NOTE_FADE`], [`NOTE_CUT`], [`NOTE_OFF`] |
/// | `instrument` | [`INSTRUMENT_NONE`] (0) | one-based instrument (or sample) number |
/// | `volume` | [`VOLUME_NONE`] (255) | the raw volume-column byte; see [`ItVolumeCommand`] |
/// | `command` | [`COMMAND_NONE`] (0) | 1..=26 for `A`..=`Z`, up to [`COMMAND_MAX`] |
/// | `info` | — | the command's parameter byte, meaningless without a command |
///
/// Nothing is clamped beyond the note normalisation the [module documentation](self)
/// describes: a `volume` of 240 or a `command` of 30 reaches the effect processor exactly
/// as the file spelled it, and deciding what to do about it is the processor's business.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItCell {
    /// The note column.
    pub note: u8,
    /// The instrument column, one-based; [`INSTRUMENT_NONE`] when empty.
    pub instrument: u8,
    /// The volume/panning column, raw; [`VOLUME_NONE`] when empty.
    pub volume: u8,
    /// The effect column: [`COMMAND_NONE`], or 1..=26 for `A`..=`Z`.
    pub command: u8,
    /// The effect's parameter byte.
    pub info: u8,
}

impl ItCell {
    /// A cell with every column empty — what an unpacked pattern is filled with before the
    /// packed stream writes over it.
    pub const EMPTY: ItCell = ItCell {
        note: NOTE_NONE,
        instrument: INSTRUMENT_NONE,
        volume: VOLUME_NONE,
        command: COMMAND_NONE,
        info: 0,
    };

    /// Read a cell from the [`CELL_BYTES`] bytes that hold it, or `None` if there are fewer
    /// than that.
    pub fn from_bytes(bytes: &[u8]) -> Option<ItCell> {
        match bytes.get(..CELL_BYTES)? {
            [note, instrument, volume, command, info] => Some(ItCell {
                note: *note,
                instrument: *instrument,
                volume: *volume,
                command: *command,
                info: *info,
            }),
            _ => None,
        }
    }

    /// The [`CELL_BYTES`] bytes this cell is stored as.
    pub const fn to_bytes(self) -> [u8; CELL_BYTES] {
        [self.note, self.instrument, self.volume, self.command, self.info]
    }

    /// Whether every column is empty.
    pub const fn is_empty(&self) -> bool {
        self.note == NOTE_NONE
            && self.instrument == INSTRUMENT_NONE
            && self.volume == VOLUME_NONE
            && self.command == COMMAND_NONE
    }

    /// The note as a linear semitone index with C-0 as zero — IT's own numbering, so this
    /// is the identity for a playable note and `None` for everything else.
    pub const fn linear_semitone(&self) -> Option<u8> {
        match self.note {
            note if note <= NOTE_MAX => Some(note),
            _ => None,
        }
    }

    /// The note's octave, 0..=9, or `None` when the cell has no playable note.
    pub const fn octave(&self) -> Option<u8> {
        match self.linear_semitone() {
            Some(note) => Some(note / 12),
            None => None,
        }
    }

    /// The note's two-character name, `"C-"` to `"B-"`, or `None` when the cell has no
    /// playable note.
    pub fn note_name(&self) -> Option<&'static str> {
        let note = self.linear_semitone()?;
        NOTE_NAMES.get(note as usize % 12).copied()
    }

    /// The three ASCII characters a pattern view prints in the note column: `"C-4"`,
    /// `"^^^"` for a cut, `"==="` for a note off, `"~~~"` for a fade, `"..."` for nothing.
    pub fn tracker_notation(&self) -> [u8; 3] {
        match self.note {
            NOTE_NONE => *b"...",
            NOTE_CUT => *b"^^^",
            NOTE_OFF => *b"===",
            note if note > NOTE_MAX => *b"~~~",
            note => {
                let name = NOTE_NAMES.get(note as usize % 12).copied().unwrap_or("??").as_bytes();
                [
                    name.first().copied().unwrap_or(b'?'),
                    name.get(1).copied().unwrap_or(b'?'),
                    b'0' + note / 12,
                ]
            }
        }
    }

    /// The volume column decoded into the effect it names.
    pub const fn volume_command(&self) -> ItVolumeCommand { ItVolumeCommand::from_byte(self.volume) }

    /// The display-only view of this cell, with its effect named in English.
    ///
    /// Nothing in the audio path calls this; it is what a pattern view or a channel readout
    /// renders. The `volume` field carries the **decoded** value rather than the raw byte —
    /// a set-volume of 40 and a pan of 40 both display as 40 — because
    /// [`PatternCell::volume`] is a `u8` with no room for the command beside it. A UI that
    /// wants the command asks [`ItCell::volume_command`].
    pub fn display(&self) -> PatternCell {
        PatternCell {
            note: match self.note {
                NOTE_NONE => NoteCell::None,
                NOTE_CUT => NoteCell::Cut,
                NOTE_OFF | NOTE_FADE => NoteCell::Off,
                note if note > NOTE_MAX => NoteCell::Off,
                note => NoteCell::Note(note),
            },
            instrument: match self.instrument {
                INSTRUMENT_NONE => None,
                instrument => Some(instrument),
            },
            volume: self.volume_command().value(),
            effect: match self.command {
                COMMAND_NONE => None,
                command => Some(EffectCell::new(command, self.info, &EffectNames::IT)),
            },
        }
    }
}

impl Default for ItCell {
    fn default() -> ItCell { ItCell::EMPTY }
}

/// The volume column's effect, decoded from its raw byte.
///
/// The ranges are ITTECH.TXT's, plus OpenMPT's `223..=232` sample-offset extension. Every
/// parameter is in the range the file gives it — `0..=64` for a volume or a pan, `0..=9`
/// for everything else — with no further interpretation: turning a `Vx` into the `Dxx` it
/// is equivalent to is the effect processor's job (G4), not the loader's.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItVolumeCommand {
    /// The column is empty.
    None,
    /// `0..=64` — set the channel volume.
    Volume(u8),
    /// `65..=74` — fine volume up, equivalent to `D0F`..`D9F`.
    FineVolumeUp(u8),
    /// `75..=84` — fine volume down, equivalent to `DF0`..`DF9`.
    FineVolumeDown(u8),
    /// `85..=94` — volume slide up.
    VolumeSlideUp(u8),
    /// `95..=104` — volume slide down.
    VolumeSlideDown(u8),
    /// `105..=114` — pitch slide down, four times the `Exx` amount.
    PitchSlideDown(u8),
    /// `115..=124` — pitch slide up, four times the `Fxx` amount.
    PitchSlideUp(u8),
    /// `128..=192` — set the panning, `0..=64`.
    Panning(u8),
    /// `193..=202` — tone portamento, through ITTECH.TXT's own slide table.
    TonePortamento(u8),
    /// `203..=212` — vibrato depth, sharing `Hxy`/`Uxy`'s memory.
    VibratoDepth(u8),
    /// `223..=232` — sample offset (OpenMPT's extension).
    Offset(u8),
    /// A byte in none of the format's ranges, kept as the file spelled it.
    Unknown(u8),
}

impl ItVolumeCommand {
    /// Decode a raw volume-column byte.
    pub const fn from_byte(raw: u8) -> ItVolumeCommand {
        match raw {
            VOLUME_NONE => ItVolumeCommand::None,
            0..=64 => ItVolumeCommand::Volume(raw),
            65..=74 => ItVolumeCommand::FineVolumeUp(raw - 65),
            75..=84 => ItVolumeCommand::FineVolumeDown(raw - 75),
            85..=94 => ItVolumeCommand::VolumeSlideUp(raw - 85),
            95..=104 => ItVolumeCommand::VolumeSlideDown(raw - 95),
            105..=114 => ItVolumeCommand::PitchSlideDown(raw - 105),
            115..=124 => ItVolumeCommand::PitchSlideUp(raw - 115),
            128..=192 => ItVolumeCommand::Panning(raw - 128),
            193..=202 => ItVolumeCommand::TonePortamento(raw - 193),
            203..=212 => ItVolumeCommand::VibratoDepth(raw - 203),
            223..=232 => ItVolumeCommand::Offset(raw - 223),
            other => ItVolumeCommand::Unknown(other),
        }
    }

    /// The command's parameter, or `None` for an empty column.
    pub const fn value(self) -> Option<u8> {
        match self {
            ItVolumeCommand::None => None,
            ItVolumeCommand::Volume(value)
            | ItVolumeCommand::FineVolumeUp(value)
            | ItVolumeCommand::FineVolumeDown(value)
            | ItVolumeCommand::VolumeSlideUp(value)
            | ItVolumeCommand::VolumeSlideDown(value)
            | ItVolumeCommand::PitchSlideDown(value)
            | ItVolumeCommand::PitchSlideUp(value)
            | ItVolumeCommand::Panning(value)
            | ItVolumeCommand::TonePortamento(value)
            | ItVolumeCommand::VibratoDepth(value)
            | ItVolumeCommand::Offset(value)
            | ItVolumeCommand::Unknown(value) => Some(value),
        }
    }

    /// The first raw byte of this command's range — the key
    /// [`EffectNames::IT_VOLUME`](starplayer_model::EffectNames::IT_VOLUME) is indexed by.
    pub const fn range_base(self) -> Option<u8> {
        match self {
            ItVolumeCommand::None | ItVolumeCommand::Unknown(_) => None,
            ItVolumeCommand::Volume(_) => Some(0),
            ItVolumeCommand::FineVolumeUp(_) => Some(65),
            ItVolumeCommand::FineVolumeDown(_) => Some(75),
            ItVolumeCommand::VolumeSlideUp(_) => Some(85),
            ItVolumeCommand::VolumeSlideDown(_) => Some(95),
            ItVolumeCommand::PitchSlideDown(_) => Some(105),
            ItVolumeCommand::PitchSlideUp(_) => Some(115),
            ItVolumeCommand::Panning(_) => Some(128),
            ItVolumeCommand::TonePortamento(_) => Some(193),
            ItVolumeCommand::VibratoDepth(_) => Some(203),
            ItVolumeCommand::Offset(_) => Some(223),
        }
    }

    /// The command spelled out in English, or `None` for an empty or unrecognised column.
    pub fn name(self) -> Option<&'static str> {
        EffectNames::IT_VOLUME.name(self.range_base()?, 0)
    }
}

/// Normalise a raw note byte: `0..=119` is a note, 254 and 255 are the two sentinels, and
/// everything else is [`NOTE_FADE`]. See the [module documentation](self).
pub const fn normalise_note(raw: u8) -> u8 {
    match raw {
        NOTE_CUT => NOTE_CUT,
        NOTE_OFF => NOTE_OFF,
        note if note <= NOTE_MAX => note,
        _ => NOTE_FADE,
    }
}

/// The per-channel memories the packed stream reads back through mask bits 4..=7.
struct ChannelMemory {
    mask: [u8; MAX_PACKED_CHANNELS],
    note: [u8; MAX_PACKED_CHANNELS],
    instrument: [u8; MAX_PACKED_CHANNELS],
    volume: [u8; MAX_PACKED_CHANNELS],
    command: [u8; MAX_PACKED_CHANNELS],
    info: [u8; MAX_PACKED_CHANNELS],
}

impl ChannelMemory {
    fn new() -> ChannelMemory {
        ChannelMemory {
            mask: [0; MAX_PACKED_CHANNELS],
            note: [NOTE_NONE; MAX_PACKED_CHANNELS],
            instrument: [INSTRUMENT_NONE; MAX_PACKED_CHANNELS],
            volume: [VOLUME_NONE; MAX_PACKED_CHANNELS],
            command: [COMMAND_NONE; MAX_PACKED_CHANNELS],
            info: [0; MAX_PACKED_CHANNELS],
        }
    }
}

/// The channel a packed stream's channel byte names, or `None` for a byte that names one
/// past what the memories cover.
///
/// `(b & 0x7F) - 1`, with zero staying zero — OpenMPT's rule, which deliberately does not
/// mask to six bits (research point 3).
const fn packed_channel(byte: u8) -> Option<usize> {
    let channel = (byte & 0x7F) as usize;
    let channel = match channel {
        0 => 0,
        channel => channel - 1,
    };
    match channel < MAX_PACKED_CHANNELS {
        true => Some(channel),
        false => None,
    }
}

/// The channels a packed pattern actually writes to, as a count.
///
/// This is OpenMPT's `ReadIT` pre-scan: a channel counts as used when an event for it
/// carries a mask whose **low nybble** is non-zero — that is, when the stream really writes
/// a note, an instrument, a volume or a command there. A channel the header disables in
/// `ChnPan` still counts, because IT processes effects in muted channels.
///
/// Nothing here allocates and nothing can fail; a stream that ends early simply stops
/// contributing.
pub fn used_channels(packed: &[u8], rows: u16) -> u8 {
    let mut mask = [0u8; MAX_PACKED_CHANNELS];
    let mut highest: Option<usize> = None;
    let mut position = 0usize;
    let mut row = 0u16;

    while row < rows {
        let Some(byte) = packed.get(position).copied() else { break };
        position += 1;
        if byte == 0 {
            row += 1;
            continue;
        }
        let Some(channel) = packed_channel(byte) else { break };
        if byte & CHANNEL_HAS_MASK != 0 {
            let Some(next) = packed.get(position).copied() else { break };
            position += 1;
            if let Some(slot) = mask.get_mut(channel) {
                *slot = next;
            }
        }
        let low = mask.get(channel).copied().unwrap_or(0) & 0x0F;
        if low != 0 {
            if channel < MAX_CHANNELS as usize && highest.is_none_or(|current| channel > current) {
                highest = Some(channel);
            }
            position += MASK_TO_SKIPS.get(low as usize).copied().unwrap_or(0) as usize;
        }
    }

    match highest {
        Some(channel) => (channel + 1) as u8,
        None => 0,
    }
}

/// Unpack one pattern's packed byte stream into `rows * channels` [`ItCell`]s, stored
/// row-major at [`CELL_BYTES`] bytes each.
///
/// `packed` is the stream **after** the eight-byte pattern header, which is not part of it.
///
/// # What a malformed stream does
///
/// Nothing here can fail, because there is no failure a tracker would not have played
/// through:
///
/// * a stream that ends before `rows` rows leaves the remaining rows empty;
/// * a stream that describes more rows than the pattern has is cut off;
/// * an event naming a channel at or past `channels` is *parsed* — its memories are updated
///   and its bytes consumed, so the rest of the row still decodes — and then discarded;
/// * two events for one channel in one row merge, later columns winning, exactly as
///   OpenMPT's and libxmp's in-place cell writes do.
pub fn unpack(packed: &[u8], rows: u16, channels: u8) -> Vec<u8> {
    let mut cells = vec![0u8; rows as usize * channels as usize * CELL_BYTES];
    for cell in cells.chunks_exact_mut(CELL_BYTES) {
        cell.copy_from_slice(&ItCell::EMPTY.to_bytes());
    }

    let mut memory = ChannelMemory::new();
    let mut position = 0usize;
    let mut row = 0u16;

    while row < rows {
        let Some(byte) = packed.get(position).copied() else { break };
        position += 1;
        if byte == 0 {
            row += 1;
            continue;
        }
        let Some(channel) = packed_channel(byte) else { break };

        if byte & CHANNEL_HAS_MASK != 0 {
            let next = packed.get(position).copied().unwrap_or(0);
            position += 1;
            if let Some(slot) = memory.mask.get_mut(channel) {
                *slot = next;
            }
        }
        let mask = memory.mask.get(channel).copied().unwrap_or(0);

        // A second event for the same channel in the same row merges into the first.
        let offset = (row as usize * channels as usize + channel) * CELL_BYTES;
        let mut cell = match channel < channels as usize {
            true => cells.get(offset..offset + CELL_BYTES).and_then(ItCell::from_bytes).unwrap_or(ItCell::EMPTY),
            false => ItCell::EMPTY,
        };

        if mask & MASK_LAST_NOTE != 0 {
            cell.note = memory.note.get(channel).copied().unwrap_or(NOTE_NONE);
        }
        if mask & MASK_LAST_INSTRUMENT != 0 {
            cell.instrument = memory.instrument.get(channel).copied().unwrap_or(INSTRUMENT_NONE);
        }
        if mask & MASK_LAST_VOLUME != 0 {
            cell.volume = memory.volume.get(channel).copied().unwrap_or(VOLUME_NONE);
        }
        if mask & MASK_LAST_COMMAND != 0 {
            cell.command = memory.command.get(channel).copied().unwrap_or(COMMAND_NONE);
            cell.info = memory.info.get(channel).copied().unwrap_or(0);
        }

        if mask & MASK_NOTE != 0 {
            cell.note = match packed.get(position).copied() {
                Some(raw) => normalise_note(raw),
                // A truncated note byte is not a fade: the column stays empty.
                None => NOTE_NONE,
            };
            position += 1;
            if let Some(slot) = memory.note.get_mut(channel) {
                *slot = cell.note;
            }
        }
        if mask & MASK_INSTRUMENT != 0 {
            cell.instrument = packed.get(position).copied().unwrap_or(INSTRUMENT_NONE);
            position += 1;
            if let Some(slot) = memory.instrument.get_mut(channel) {
                *slot = cell.instrument;
            }
        }
        if mask & MASK_VOLUME != 0 {
            cell.volume = packed.get(position).copied().unwrap_or(VOLUME_NONE);
            position += 1;
            if let Some(slot) = memory.volume.get_mut(channel) {
                *slot = cell.volume;
            }
        }
        if mask & MASK_COMMAND != 0 {
            cell.command = packed.get(position).copied().unwrap_or(COMMAND_NONE);
            cell.info = packed.get(position + 1).copied().unwrap_or(0);
            position += 2;
            if let Some(slot) = memory.command.get_mut(channel) {
                *slot = cell.command;
            }
            if let Some(slot) = memory.info.get_mut(channel) {
                *slot = cell.info;
            }
        }

        let in_range = channel < channels as usize;
        if let Some(destination) = cells.get_mut(offset..offset + CELL_BYTES).filter(|_| in_range) {
            destination.copy_from_slice(&cell.to_bytes());
        }
    }
    cells
}

/// A read-only view over one unpacked pattern inside a loaded [`Module`].
///
/// Every accessor returns [`Option`], because it is read from inside `render()` where a
/// panic is fatal.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternView<'module> {
    cells: &'module [u8],
    rows: u16,
    channels: u8,
}

impl<'module> PatternView<'module> {
    /// Borrow pattern `id` out of `module`.
    ///
    /// `None` if `id` names no pattern, or if the pattern's region is smaller than
    /// `rows * channels` cells.
    pub fn new(module: &'module Module, id: PatternId) -> Option<PatternView<'module>> {
        let index = module.pattern(id)?;
        let bytes = module.pattern_bytes(id)?;
        let needed = index.rows() as usize * index.channels() as usize * CELL_BYTES;
        Some(PatternView { cells: bytes.get(..needed)?, rows: index.rows(), channels: index.channels() })
    }

    /// Build a view directly over unpacked bytes — what [`unpack`] returns.
    pub fn from_cells(cells: &'module [u8], rows: u16, channels: u8) -> Option<PatternView<'module>> {
        let needed = rows as usize * channels as usize * CELL_BYTES;
        Some(PatternView { cells: cells.get(..needed)?, rows, channels })
    }

    /// Rows in this pattern.
    pub const fn rows(&self) -> u16 { self.rows }

    /// Channel columns this pattern stores.
    pub const fn channels(&self) -> u8 { self.channels }

    /// The raw cell bytes, `rows * channels * CELL_BYTES` of them.
    pub const fn cells(&self) -> &'module [u8] { self.cells }

    /// One cell, or `None` if `row` or `channel` is out of range.
    pub fn cell(&self, row: u16, channel: u8) -> Option<ItCell> {
        if row >= self.rows || channel >= self.channels {
            return None;
        }
        let offset = (row as usize * self.channels as usize + channel as usize) * CELL_BYTES;
        ItCell::from_bytes(self.cells.get(offset..offset + CELL_BYTES)?)
    }

    /// Every cell of one row, left channel to right, or `None` if `row` is out of range.
    pub fn row(&self, row: u16) -> Option<impl Iterator<Item = ItCell> + 'module> {
        if row >= self.rows {
            return None;
        }
        let start = row as usize * self.channels as usize * CELL_BYTES;
        let length = self.channels as usize * CELL_BYTES;
        let bytes = self.cells.get(start..start + length)?;
        Some(bytes.chunks_exact(CELL_BYTES).filter_map(ItCell::from_bytes))
    }
}

/// The [`PatternData`] seam over a loaded IT module: the order list, and one fixed-stride
/// row slice at a time.
///
/// Task G4 builds the effect processor that reads those bytes; this is everything the
/// sequencer itself needs.
pub struct ItPatternData(pub Arc<Module>);

impl PatternData for ItPatternData {
    fn order_count(&self) -> u16 { self.0.orders().len().min(u16::MAX as usize) as u16 }

    fn order(&self, order: u16) -> Option<OrderEntry> {
        match self.0.order_entry(order as usize)? {
            ModelOrderEntry::Pattern(pattern) => Some(OrderEntry::Pattern(pattern.0)),
            ModelOrderEntry::Marker => Some(OrderEntry::Skip),
            ModelOrderEntry::End => Some(OrderEntry::End),
        }
    }

    fn channel_count(&self) -> u8 { self.0.header().channel_count }

    fn rows_in_pattern(&self, pattern: u16) -> Option<u16> {
        self.0.pattern(PatternId(pattern)).map(|index| index.rows())
    }

    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]> {
        let pattern_id = PatternId(pattern);
        let pattern_index = self.0.pattern(pattern_id)?;
        if row >= pattern_index.rows() {
            return None;
        }
        let row_length = pattern_index.channels() as usize * CELL_BYTES;
        let start = row as usize * row_length;
        self.0.pattern_bytes(pattern_id)?.get(start..start + row_length)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use starplayer_model::pattern::it_command_code;

    #[test]
    fn an_empty_cell_round_trips_through_its_bytes() {
        assert_eq!(ItCell::from_bytes(&ItCell::EMPTY.to_bytes()), Some(ItCell::EMPTY));
        assert_eq!(ItCell::EMPTY.to_bytes(), [NOTE_NONE, 0, VOLUME_NONE, 0, 0]);
        assert!(ItCell::EMPTY.is_empty());
        assert_eq!(ItCell::from_bytes(&[1, 2, 3, 4]), None);
        assert_eq!(ItCell::default(), ItCell::EMPTY);
    }

    #[test]
    fn the_no_note_byte_cannot_collide_with_any_value_a_file_can_write() {
        for raw in 0..=255u8 {
            assert_ne!(normalise_note(raw), NOTE_NONE, "raw note byte {raw} normalised onto the empty marker");
        }
        assert_eq!(normalise_note(0), 0);
        assert_eq!(normalise_note(NOTE_MAX), NOTE_MAX);
        assert_eq!(normalise_note(120), NOTE_FADE, "everything above B-9 is a fade");
        assert_eq!(normalise_note(NOTE_NONE), NOTE_FADE);
        assert_eq!(normalise_note(NOTE_FADE), NOTE_FADE);
        assert_eq!(normalise_note(NOTE_CUT), NOTE_CUT);
        assert_eq!(normalise_note(NOTE_OFF), NOTE_OFF);
    }

    #[test]
    fn a_note_byte_spells_itself_in_tracker_notation() {
        assert_eq!(ItCell { note: 60, ..ItCell::EMPTY }.tracker_notation(), *b"C-5");
        assert_eq!(ItCell { note: 61, ..ItCell::EMPTY }.tracker_notation(), *b"C#5");
        assert_eq!(ItCell { note: 0, ..ItCell::EMPTY }.tracker_notation(), *b"C-0");
        assert_eq!(ItCell { note: NOTE_MAX, ..ItCell::EMPTY }.tracker_notation(), *b"B-9");
        assert_eq!(ItCell::EMPTY.tracker_notation(), *b"...");
        assert_eq!(ItCell { note: NOTE_CUT, ..ItCell::EMPTY }.tracker_notation(), *b"^^^");
        assert_eq!(ItCell { note: NOTE_OFF, ..ItCell::EMPTY }.tracker_notation(), *b"===");
        assert_eq!(ItCell { note: NOTE_FADE, ..ItCell::EMPTY }.tracker_notation(), *b"~~~");

        let cell = ItCell { note: 61, ..ItCell::EMPTY };
        assert_eq!(cell.note_name(), Some("C#"));
        assert_eq!(cell.octave(), Some(5));
        assert_eq!(cell.linear_semitone(), Some(61));
        assert_eq!(ItCell { note: NOTE_CUT, ..ItCell::EMPTY }.octave(), None);
    }

    #[test]
    fn every_volume_column_range_decodes_to_its_own_command() {
        assert_eq!(ItVolumeCommand::from_byte(VOLUME_NONE), ItVolumeCommand::None);
        assert_eq!(ItVolumeCommand::from_byte(0), ItVolumeCommand::Volume(0));
        assert_eq!(ItVolumeCommand::from_byte(64), ItVolumeCommand::Volume(64));
        assert_eq!(ItVolumeCommand::from_byte(65), ItVolumeCommand::FineVolumeUp(0));
        assert_eq!(ItVolumeCommand::from_byte(74), ItVolumeCommand::FineVolumeUp(9));
        assert_eq!(ItVolumeCommand::from_byte(75), ItVolumeCommand::FineVolumeDown(0));
        assert_eq!(ItVolumeCommand::from_byte(84), ItVolumeCommand::FineVolumeDown(9));
        assert_eq!(ItVolumeCommand::from_byte(85), ItVolumeCommand::VolumeSlideUp(0));
        assert_eq!(ItVolumeCommand::from_byte(95), ItVolumeCommand::VolumeSlideDown(0));
        assert_eq!(ItVolumeCommand::from_byte(105), ItVolumeCommand::PitchSlideDown(0));
        assert_eq!(ItVolumeCommand::from_byte(115), ItVolumeCommand::PitchSlideUp(0));
        assert_eq!(ItVolumeCommand::from_byte(128), ItVolumeCommand::Panning(0));
        assert_eq!(ItVolumeCommand::from_byte(192), ItVolumeCommand::Panning(64));
        assert_eq!(ItVolumeCommand::from_byte(193), ItVolumeCommand::TonePortamento(0));
        assert_eq!(ItVolumeCommand::from_byte(203), ItVolumeCommand::VibratoDepth(0));
        assert_eq!(ItVolumeCommand::from_byte(223), ItVolumeCommand::Offset(0));
        assert_eq!(ItVolumeCommand::from_byte(232), ItVolumeCommand::Offset(9));
        assert_eq!(ItVolumeCommand::from_byte(125), ItVolumeCommand::Unknown(125), "125..=127 is in none of the ranges");
        assert_eq!(ItVolumeCommand::from_byte(213), ItVolumeCommand::Unknown(213), "213..=222 was velocity");
    }

    #[test]
    fn a_volume_column_command_names_itself_in_english() {
        assert_eq!(ItVolumeCommand::from_byte(40).name(), Some("set volume"));
        assert_eq!(ItVolumeCommand::from_byte(70).name(), Some("fine volume up"));
        assert_eq!(ItVolumeCommand::from_byte(160).name(), Some("set panning"));
        assert_eq!(ItVolumeCommand::from_byte(205).name(), Some("vibrato depth"));
        assert_eq!(ItVolumeCommand::from_byte(VOLUME_NONE).name(), None);
        assert_eq!(ItVolumeCommand::from_byte(250).name(), None);
        assert_eq!(ItVolumeCommand::from_byte(40).value(), Some(40));
        assert_eq!(ItVolumeCommand::from_byte(VOLUME_NONE).value(), None);
    }

    #[test]
    fn a_cell_displays_with_its_effect_named_in_english() {
        let cell = ItCell { note: 60, instrument: 3, volume: 40, command: it_command_code(b'D'), info: 0x0F };
        let display = cell.display();

        assert_eq!(display.note, NoteCell::Note(60));
        assert_eq!(display.instrument, Some(3));
        assert_eq!(display.volume, Some(40));
        assert_eq!(display.effect.map(|effect| effect.name), Some("volume slide"));

        assert_eq!(ItCell { note: NOTE_CUT, ..ItCell::EMPTY }.display().note, NoteCell::Cut);
        assert_eq!(ItCell { note: NOTE_OFF, ..ItCell::EMPTY }.display().note, NoteCell::Off);
        assert_eq!(ItCell { note: NOTE_FADE, ..ItCell::EMPTY }.display().note, NoteCell::Off, "the model has no fade cell of its own");
        assert_eq!(ItCell::EMPTY.display(), PatternCell::default());
    }

    /// Channel byte, mask, then the mask's own data bytes.
    fn event(channel: u8, mask: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![(channel + 1) | CHANNEL_HAS_MASK, mask];
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn the_unpacker_places_every_combination_of_the_four_data_groups() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE | MASK_INSTRUMENT, &[60, 1]));
        packed.extend(event(1, MASK_VOLUME, &[40]));
        packed.extend(event(2, MASK_COMMAND, &[it_command_code(b'A'), 6]));
        packed.push(0);
        packed.extend(event(0, MASK_NOTE | MASK_INSTRUMENT | MASK_VOLUME | MASK_COMMAND, &[61, 2, 128, it_command_code(b'H'), 0x42]));
        packed.push(0);

        let cells = unpack(&packed, 2, 4);
        let view = PatternView::from_cells(&cells, 2, 4).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(ItCell { note: 60, instrument: 1, ..ItCell::EMPTY }));
        assert_eq!(view.cell(0, 1), Some(ItCell { volume: 40, ..ItCell::EMPTY }));
        assert_eq!(view.cell(0, 2), Some(ItCell { command: 1, info: 6, ..ItCell::EMPTY }));
        assert_eq!(view.cell(0, 3), Some(ItCell::EMPTY));
        assert_eq!(view.cell(1, 0), Some(ItCell { note: 61, instrument: 2, volume: 128, command: 8, info: 0x42 }));
        assert_eq!(view.cell(2, 0), None);
        assert_eq!(view.cell(0, 4), None);
    }

    #[test]
    fn a_channel_reuses_its_mask_when_the_top_bit_is_clear() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE, &[60]));
        packed.push(0);
        // No 0x80, so the channel's stored mask (note) applies again.
        packed.extend_from_slice(&[1, 62]);
        packed.push(0);

        let cells = unpack(&packed, 2, 1);
        let view = PatternView::from_cells(&cells, 2, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0).map(|cell| cell.note), Some(60));
        assert_eq!(view.cell(1, 0).map(|cell| cell.note), Some(62));
    }

    #[test]
    fn the_four_memory_bits_replay_a_channels_last_values() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE | MASK_INSTRUMENT | MASK_VOLUME | MASK_COMMAND, &[60, 1, 40, it_command_code(b'H'), 0x42]));
        packed.push(0);
        packed.extend(event(0, MASK_LAST_NOTE | MASK_LAST_INSTRUMENT | MASK_LAST_VOLUME | MASK_LAST_COMMAND, &[]));
        packed.push(0);

        let cells = unpack(&packed, 2, 1);
        let view = PatternView::from_cells(&cells, 2, 1).expect("the view covers the cells");

        assert_eq!(view.cell(1, 0), view.cell(0, 0));
        assert_eq!(view.cell(1, 0), Some(ItCell { note: 60, instrument: 1, volume: 40, command: 8, info: 0x42 }));
    }

    #[test]
    fn a_note_byte_above_the_range_becomes_a_fade_in_the_cell_and_in_the_memory() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE, &[200]));
        packed.push(0);
        packed.extend(event(0, MASK_LAST_NOTE, &[]));
        packed.push(0);

        let cells = unpack(&packed, 2, 1);
        let view = PatternView::from_cells(&cells, 2, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0).map(|cell| cell.note), Some(NOTE_FADE));
        assert_eq!(view.cell(1, 0).map(|cell| cell.note), Some(NOTE_FADE));
    }

    #[test]
    fn an_event_for_a_channel_past_the_end_is_consumed_and_discarded() {
        let mut packed = Vec::new();
        packed.extend(event(9, MASK_NOTE | MASK_INSTRUMENT, &[60, 1]));
        packed.extend(event(1, MASK_NOTE | MASK_INSTRUMENT, &[62, 5]));
        packed.push(0);

        let cells = unpack(&packed, 1, 2);
        let view = PatternView::from_cells(&cells, 1, 2).expect("the view covers the cells");

        assert_eq!(view.cell(0, 1), Some(ItCell { note: 62, instrument: 5, ..ItCell::EMPTY }));
        assert_eq!(view.cell(0, 0), Some(ItCell::EMPTY));
    }

    #[test]
    fn two_events_for_one_channel_in_one_row_merge() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE, &[60]));
        packed.extend(event(0, MASK_VOLUME, &[32]));
        packed.push(0);

        let cells = unpack(&packed, 1, 1);
        let view = PatternView::from_cells(&cells, 1, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(ItCell { note: 60, volume: 32, ..ItCell::EMPTY }));
    }

    #[test]
    fn a_stream_that_ends_early_or_runs_long_neither_panics_nor_overruns() {
        let short = unpack(&[0x81, MASK_NOTE], 4, 2);
        assert_eq!(short.len(), 4 * 2 * CELL_BYTES);
        let view = PatternView::from_cells(&short, 4, 2).expect("the view covers the cells");
        assert_eq!(view.cell(0, 0).map(|cell| cell.note), Some(NOTE_NONE), "a truncated note byte reads as no note");
        assert_eq!(view.cell(3, 1), Some(ItCell::EMPTY));

        let long = unpack(&[0, 0, 0, 0, 0x81, MASK_NOTE, 60], 2, 1);
        let view = PatternView::from_cells(&long, 2, 1).expect("the view covers the cells");
        assert_eq!(view.cell(0, 0), Some(ItCell::EMPTY));
        assert_eq!(view.cell(1, 0), Some(ItCell::EMPTY));

        assert_eq!(unpack(&[], 1, 1).len(), CELL_BYTES);
    }

    #[test]
    fn the_used_channel_count_follows_the_low_nybble_of_each_mask() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE, &[60]));
        packed.extend(event(5, MASK_COMMAND, &[1, 6]));
        packed.push(0);

        assert_eq!(used_channels(&packed, 1), 6, "channels 0 and 5 are written, so six columns are in use");

        // A mask with no data bits names a channel without writing to it.
        let mut idle = Vec::new();
        idle.extend(event(9, 0, &[]));
        idle.push(0);
        assert_eq!(used_channels(&idle, 1), 0);

        assert_eq!(used_channels(&[], 64), 0);
        assert_eq!(used_channels(&[0, 0, 0], 64), 0);
    }

    #[test]
    fn the_used_channel_count_stops_at_the_songs_sixty_four_columns() {
        let mut packed = Vec::new();
        packed.extend(event(70, MASK_NOTE, &[60]));
        packed.extend(event(3, MASK_NOTE, &[60]));
        packed.push(0);

        assert_eq!(used_channels(&packed, 1), 4, "an event past channel 63 does not widen the song");
    }

    #[test]
    fn a_row_iterates_left_to_right() {
        let mut packed = Vec::new();
        packed.extend(event(0, MASK_NOTE, &[60]));
        packed.extend(event(1, MASK_NOTE, &[61]));
        packed.push(0);

        let cells = unpack(&packed, 1, 3);
        let view = PatternView::from_cells(&cells, 1, 3).expect("the view covers the cells");
        let row: Vec<ItCell> = view.row(0).expect("row 0 exists").collect();

        assert_eq!(row.len(), 3);
        assert_eq!(row[0].note, 60);
        assert_eq!(row[1].note, 61);
        assert_eq!(row[2], ItCell::EMPTY);
        assert!(view.row(1).is_none());
        assert_eq!(view.cells().len(), 3 * CELL_BYTES);
        assert_eq!((view.rows(), view.channels()), (1, 3));
    }

    #[test]
    fn a_channel_byte_past_the_packed_range_ends_the_stream_rather_than_aliasing_channel_zero() {
        assert_eq!(packed_channel(0x80), Some(0), "a bare 0x80 names channel 0");
        assert_eq!(packed_channel(1), Some(0));
        assert_eq!(packed_channel(0x7F), Some(126));
        assert_eq!(packed_channel(0xFF), Some(126));
    }
}

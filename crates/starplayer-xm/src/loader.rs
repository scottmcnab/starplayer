//! `load` — bytes in, [`Module`] out.
//!
//! # The layout this walks
//!
//! ```text
//! 0x00  "Extended Module: ", 17 bytes
//! 0x11  module name, 20 bytes      0x25  0x1A
//! 0x26  tracker name, 20 bytes     0x3A  version
//! 0x3C  header size, measured from 0x3C itself
//! 0x40  song length   0x42 restart position  0x44 channels
//! 0x46  patterns      0x48 instruments       0x4A flags
//! 0x4C  default speed 0x4E default BPM
//! 0x50  the order table, `header size - 20` bytes of it
//!       ── at 0x3C + header size ──
//!       version 0x0104 and later: every pattern, then every instrument, each
//!         instrument's sample headers followed immediately by that instrument's PCM
//!       version 0x0102 / 0x0103: every instrument and its sample headers, then every
//!         pattern, then every sample's PCM in one run
//!       anything after the last sample — OpenMPT's `text`, `MIDI`, `PNAM`, `CNAM` and
//!         `XTPM` chunks — is ignored
//! ```
//!
//! # Where each XM header field ends up
//!
//! The effect processor reads its inputs from the loaded [`Module`], never from the file,
//! so this is the whole contract between the loader and the rest of the crate:
//!
//! | File field | Where it lands |
//! |---|---|
//! | module name (`0x11`) | `header().title` |
//! | tracker name (`0x26`) | `header().dialect`, via [`XmHeader::dialect`] |
//! | version (`0x3A`) | consumed by the loader; it selects the layout above |
//! | header size (`0x3C`) | consumed by the loader; where the body starts |
//! | song length (`0x40`) | `orders().len()` |
//! | restart position (`0x42`) | [`XmFormatExtra::restart_position`] |
//! | channels (`0x44`) | `header().channel_count` |
//! | patterns (`0x46`) | `patterns().len()`, plus one empty pattern if an order needs it |
//! | instruments (`0x48`) | `instruments().len()`; instrument *n* in a cell is `InstrumentId(n - 1)` |
//! | flags (`0x4A`) bit 0 | `header().flags.linear_slides`, and the whole word in [`XmFormatExtra::flags`] |
//! | default speed (`0x4C`) | `header().initial_speed` |
//! | default BPM (`0x4E`) | `header().initial_tempo` |
//! | the order table (`0x50`) | `orders()` |
//! | packed patterns | [`XmCell`](crate::XmCell)s in `Module::blob`, read through [`PatternView`](crate::PatternView) |
//! | instrument envelopes, fadeout, note map | the module's `InstrumentDef` for that instrument |
//! | instrument auto-vibrato | copied onto **each** of the instrument's samples |
//! | sample volume, pan, relative note, finetune, loop | the module's `SampleIndex` for that sample |
//!
//! `header().default_pan` is deliberately **empty**: XM starts every channel centred and
//! takes its panning from the instrument, the sample and the `8xx` effect instead.
//!
//! # Clamp or reject
//!
//! The rule S3M's loader states applies here too: *clamp wherever a tracker would have
//! played the file, reject only where the file contradicts itself*. Every case, and
//! nothing else fails:
//!
//! | Case | Behaviour |
//! |---|---|
//! | No `Extended Module: ` at 0, or no `0x1A` at 37, or fewer than 80 bytes | [`Error::BadMagic`] / [`Error::Truncated`] |
//! | Fewer bytes left than the order list plus four per declared pattern and instrument | [`Error::Truncated`] — OpenMPT's `GetHeaderMinimumAdditionalSize` |
//! | Channel count of 0 | [`Error::Invalid`] |
//! | Channel count above 64 (`ChannelTable::MAX_CHANNELS`) | [`Error::TooLarge`] |
//! | Pattern count above 256, instrument count above 255 | [`Error::TooLarge`] |
//! | More decoded pattern data than the file's own size can justify | [`Error::TooLarge`] — see [`MINIMUM_PATTERN_BUDGET_BYTES`] |
//! | `song_length` above 256 | clamped to 256, the order table's own size |
//! | `song_length` of 0, in a file not written by OpenMPT | becomes a one-entry list naming pattern 0, which is what FastTracker 2 plays |
//! | Order naming a pattern the file does not have | plays **one shared empty pattern**, appended after the file's own — FastTracker 2 always has 256 pattern slots and the unstored ones are empty |
//! | `header_size` below 20 | raised to 20, so the body starts after the fields already read |
//! | Default speed of 0 | 6, ProTracker's and FastTracker 2's default |
//! | Default BPM of 0 | 125; anything else is clamped to 32..=255 |
//! | Pattern header size below 8, or past the end of the file | that pattern and every one after it is an empty 64-row pattern |
//! | Pattern rows of 0 | 64 — FastTracker 2's default, and what OpenMPT substitutes |
//! | Pattern rows above 256 | clamped to 256, the format's own maximum |
//! | Packed size of 0, or a packed stream that ends early | the rest of the pattern is empty cells; see [`crate::pattern::unpack`] |
//! | Packed size running past the end of the file | clamped to what is there |
//! | Instrument `size` of 0 | 263, the largest header any tracker writes |
//! | Instrument `size` below 29, or a header past the end of the file | an empty instrument, numbering preserved |
//! | Instrument header shorter than the extended part | the missing fields read as zero, as OpenMPT's `ReadStructPartial` gives them |
//! | Envelope point count above 12 | cut to 12 |
//! | Envelope sustain or loop index past the last point, or a loop that runs backwards | that span is dropped, not clamped |
//! | Envelope with its enable bit clear, or with no points | no envelope at all — `Option<Envelope>` in the model *is* the enable bit |
//! | A sample header past the end of the file | that sample, and the rest of the instrument's, are not read |
//! | Sample data running past the end of the file | clamped to the frames that are there |
//! | Sample `vol` above 64 | clamped to 64 |
//! | Loop end past the sample's length | clamped to the length |
//! | `loop_start >= loop_end` after clamping, or a loop kind of 0 | the sample does not loop |
//! | Loop kind 3 — ModPlug up to 1.11 set both bits | read as ping-pong, as OpenMPT reads it |
//! | Stereo sample (`type` bit 5) | the two channels are averaged into one; see [`crate::sample::decode_frames`] |
//! | ModPlug ADPCM sample (`reserved` == `0xAD`) | decoded; see [`crate::sample::decode_frames`] |
//! | Anything after the last sample | ignored |

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader,
    ModuleReader, SampleSpec,
};

use crate::header::{self, XmFormatExtra, XmHeader};
use crate::instrument::{self, XmInstrumentHeader};
use crate::pattern::{self, DEFAULT_ROWS, MAX_ROWS};
use crate::sample::{self, XmSampleHeader};

/// Ticks per row a file that asks for zero is given instead — FastTracker 2's own default.
const FALLBACK_SPEED: u8 = 6;

/// Beats per minute a file that asks for zero is given instead.
const FALLBACK_TEMPO: u16 = 125;

/// Slowest tempo the format allows. A slower one would make a tick longer than the
/// sequencer's arithmetic expects.
const MINIMUM_TEMPO: u16 = 32;

/// Fastest tempo the format allows: FastTracker 2's `Fxx` cannot express more.
const MAXIMUM_TEMPO: u16 = 255;

/// Bytes a pattern header occupies before its packed data in version `0x0103` and later:
/// the four-byte length, the packing type, a `u16` row count and a `u16` packed size.
const PATTERN_HEADER_LENGTH: usize = 9;

/// The same for version `0x0102`, whose row count is one byte biased by one.
const PATTERN_HEADER_LENGTH_1_02: usize = 8;

/// Smallest pattern header OpenMPT will read: below this it stops reading patterns
/// altogether (`Load_xm.cpp`, `if(headerSize < 8 …) break;`).
const MINIMUM_PATTERN_HEADER_LENGTH: usize = 8;

/// Decoded pattern bytes a file is allowed regardless of how small it is.
///
/// The same defence S3M's loader documents, sized for XM's wider patterns. A pattern
/// costs at least nine bytes of header, so a 3 KB file may declare 256 patterns; each one
/// unpacks to a fixed `rows × channels × 5` bytes, which at 256 rows and 64 channels is
/// 80 KB — 20 MB of decoded patterns out of a file that fits in an email. Surviving that
/// is not enough: a loader that allocates for it has already lost on a phone or an
/// embedded target, so it is refused on principle, with a budget no real file approaches.
/// The floor alone is 256 patterns of 64 rows and 32 channels, four times FastTracker 2's
/// own limit of 256 patterns of 64 rows and 32 channels.
const MINIMUM_PATTERN_BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// Decoded pattern bytes a file earns per byte of its own size, once it is big enough for
/// that to beat [`MINIMUM_PATTERN_BUDGET_BYTES`]. A packed XM pattern of ordinary density
/// is well under a fifth of its decoded size, so 64× leaves two orders of magnitude of
/// headroom.
const PATTERN_BUDGET_PER_FILE_BYTE: usize = 64;

/// The decoded-pattern budget for a file of `file_bytes`.
fn decoded_pattern_budget(file_bytes: usize) -> usize {
    file_bytes.saturating_mul(PATTERN_BUDGET_PER_FILE_BYTE).max(MINIMUM_PATTERN_BUDGET_BYTES)
}

/// Whether `bytes` looks like an XM: the 17-byte signature at 0 and `0x1A` at 37.
///
/// Cheap and total — it reads 38 bytes and never allocates — so a host can use it to pick
/// a loader before committing to one. The `0x1A` is part of the test because the
/// signature on its own is printable text.
pub fn probe(bytes: &[u8]) -> bool {
    bytes.get(..header::MAGIC.len()) == Some(&header::MAGIC[..])
        && bytes.get(header::EOF_MARKER_OFFSET) == Some(&header::EOF_MARKER)
}

/// Whether `reader` looks like an XM. The [`ModuleReader`] form of [`probe`].
pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool {
    let mut prefix = [0u8; header::EOF_MARKER_OFFSET + 1];
    reader.read_at(0, &mut prefix).is_ok() && probe(&prefix)
}

/// Load an XM from bytes already in memory.
pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_from(bytes) }

/// Load an XM from any [`ModuleReader`].
///
/// The two are the same function: `[u8]` is a `ModuleReader` whose
/// [`slice_at`](ModuleReader::slice_at) borrows, so loading from a slice copies nothing
/// beyond what the module itself owns.
///
/// # Errors
///
/// See the clamp-or-reject table in the [module documentation](self).
pub fn load_from<R: ModuleReader + ?Sized>(reader: &R) -> Result<Module, Error> {
    let source = Source { reader };

    let fixed_header: [u8; header::FIXED_HEADER_LENGTH] = source.array(0)?;
    let file_header = XmHeader::parse(&fixed_header)?;

    // OpenMPT's `GetHeaderMinimumAdditionalSize`, checked before anything is allocated:
    // the order list, plus four bytes for each declared pattern and instrument, is the
    // least a file that means what it says can hold.
    let minimum_body = (file_header.song_length as usize)
        .saturating_add(4usize.saturating_mul(file_header.pattern_count as usize + file_header.instrument_count as usize));
    if source.len().saturating_sub(header::FIXED_HEADER_LENGTH) < minimum_body {
        return Err(Error::Truncated { offset: header::FIXED_HEADER_LENGTH, needed: minimum_body });
    }

    let channel_count = file_header.channel_count as u8;
    let dialect = file_header.dialect();
    let mut builder = ModuleBuilder::new();
    let mut cursor = file_header.body_offset();

    // Version 0x0104 and later put the patterns first; the two older ones put them after
    // the instrument and sample headers and gather all the PCM at the very end.
    if file_header.patterns_precede_instruments() {
        cursor = load_patterns(&source, &file_header, channel_count, cursor, &mut builder)?;
    }
    let mut instruments = read_instrument_headers(&source, &file_header, &mut cursor)?;
    if !file_header.patterns_precede_instruments() {
        cursor = load_patterns(&source, &file_header, channel_count, cursor, &mut builder)?;
        // The PCM of every sample of every instrument, in order, in one run after the
        // patterns. Assigning the offsets here is the only thing the older layout changes.
        for pending in instruments.iter_mut() {
            for pending_sample in pending.samples.iter_mut() {
                pending_sample.data_offset = cursor;
                cursor = cursor.saturating_add(pending_sample.header.encoded_bytes());
            }
        }
    }

    for pending in &instruments {
        add_instrument(&source, pending, &mut builder)?;
    }

    let declared_patterns = file_header.pattern_count as usize;
    let orders = source.with_slice(header::FIXED_HEADER_LENGTH, file_header.song_length as usize, <[u8]>::to_vec)?;
    let orders = match orders.is_empty() && dialect != starplayer_model::FormatDialect::OpenMptXm {
        // `lamb_-_dark_lighthouse.xm` declares no orders at all and FastTracker 2 plays
        // pattern 0; OpenMPT's own files mean an empty order list literally.
        true => vec![0u8],
        false => orders,
    };

    // FastTracker 2 always holds 256 pattern slots and the ones the file does not store
    // are empty, so an order naming one of those plays an empty pattern rather than being
    // skipped. One shared pattern serves every such order.
    let empty_pattern = match orders.iter().any(|order| (*order as usize) >= declared_patterns) {
        true => {
            let cells = pattern::unpack(&[], DEFAULT_ROWS, channel_count);
            Some(builder.add_pattern(&cells, DEFAULT_ROWS, channel_count)?.0)
        }
        false => None,
    };
    let order_entries: Vec<u16> = orders.iter()
        .map(|order| match (*order as usize) < declared_patterns {
            true => *order as u16,
            false => empty_pattern.unwrap_or(0),
        })
        .collect();

    builder.set_orders(&order_entries);
    builder.set_header(song_header(&source, &file_header, channel_count, dialect)?);
    builder.build()
}

/// Build the format-neutral [`ModuleHeader`] out of the file header.
fn song_header<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &XmHeader, channel_count: u8, dialect: starplayer_model::FormatDialect) -> Result<ModuleHeader, Error> {
    let title = source.with_slice(header::TITLE_OFFSET, header::TITLE_LENGTH, text)?;

    let extra = XmFormatExtra { restart_position: file_header.restart_position, flags: file_header.flags };

    Ok(ModuleHeader {
        title: title.into_boxed_str(),
        format: ModuleFormat::Xm,
        channel_count,
        initial_speed: match file_header.initial_speed {
            0 => FALLBACK_SPEED,
            speed => core::cmp::min(speed, u8::MAX as u16) as u8,
        },
        initial_tempo: match file_header.initial_tempo {
            0 => FALLBACK_TEMPO,
            tempo => tempo.clamp(MINIMUM_TEMPO, MAXIMUM_TEMPO),
        },
        // XM has no song-wide volume field: `Gxx` sets one at run time and it starts at
        // full scale.
        global_volume: U0F16::MAX,
        master_volume: U0F16::MAX,
        // Empty means "centre every channel", which is where FastTracker 2 starts; the
        // instrument, the sample and `8xx` supply the panning from there.
        default_pan: Vec::new().into_boxed_slice(),
        flags: ModuleFlags {
            amiga_limits: false,
            linear_slides: file_header.linear_slides(),
            fast_volume_slides: false,
            stereo: true,
        },
        dialect,
        format_extra: extra.encode(),
        default_channel_volume: Vec::new().into_boxed_slice(),
        format_data: Vec::new().into_boxed_slice(),
    })
}

/// Read every pattern from `cursor` on, adding each to `builder`, and answer the offset
/// the next structure starts at.
///
/// Exactly `pattern_count` patterns are always added, whatever the file turns out to
/// hold: the order list indexes them, so a missing one has to be an empty pattern rather
/// than an absent one.
fn load_patterns<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &XmHeader, channel_count: u8, cursor: usize, builder: &mut ModuleBuilder) -> Result<usize, Error> {
    let budget = decoded_pattern_budget(source.len());
    let mut decoded_bytes = 0usize;
    let mut cursor = cursor;
    let mut readable = true;

    for _ in 0..file_header.pattern_count {
        let mut rows = DEFAULT_ROWS;
        let mut packed_start = cursor;
        let mut packed_length = 0usize;

        if readable && source.available(cursor) >= MINIMUM_PATTERN_HEADER_LENGTH {
            let fixed: [u8; PATTERN_HEADER_LENGTH] = source.padded_array(cursor);
            let byte = |offset: usize| fixed.get(offset).copied().unwrap_or(0);
            let header_size = u32::from_le_bytes([byte(0), byte(1), byte(2), byte(3)]) as usize;
            match header_size < MINIMUM_PATTERN_HEADER_LENGTH || source.available(cursor) < header_size {
                // OpenMPT stops reading patterns here rather than trying to resynchronise,
                // and so does this: every pattern from now on is an empty one.
                true => readable = false,
                false => {
                    // Version 0x0102 stores the row count as one byte biased by one and so
                    // has its packed size two bytes earlier than every later version.
                    let (declared_rows, size_offset) = match file_header.rows_are_a_biased_byte() {
                        true => (byte(5) as u16 + 1, PATTERN_HEADER_LENGTH_1_02 - 2),
                        false => (u16::from_le_bytes([byte(5), byte(6)]), PATTERN_HEADER_LENGTH - 2),
                    };
                    packed_length = u16::from_le_bytes([byte(size_offset), byte(size_offset + 1)]) as usize;
                    packed_start = cursor.saturating_add(header_size);
                    cursor = packed_start.saturating_add(packed_length);
                    rows = match declared_rows {
                        0 => DEFAULT_ROWS,
                        rows => core::cmp::min(rows, MAX_ROWS),
                    };
                }
            }
        } else {
            readable = false;
        }

        decoded_bytes = decoded_bytes
            .saturating_add((rows as usize).saturating_mul(channel_count as usize).saturating_mul(pattern::CELL_BYTES));
        if decoded_bytes > budget {
            return Err(Error::TooLarge("XM pattern data"));
        }

        let available = core::cmp::min(packed_length, source.available(packed_start));
        let cells = source.with_slice(packed_start, available, |packed| pattern::unpack(packed, rows, channel_count))?;
        builder.add_pattern(&cells, rows, channel_count)?;
    }
    Ok(cursor)
}

/// One sample's header, its name and where its PCM lives.
struct PendingSample {
    header: XmSampleHeader,
    name: String,
    /// Offset of the sample's encoded data. Filled while the headers are read for version
    /// `0x0104` and later, and after the patterns for the two older versions.
    data_offset: usize,
}

/// One instrument's header, its name and its samples.
struct PendingInstrument {
    name: Box<str>,
    header: XmInstrumentHeader,
    samples: Vec<PendingSample>,
}

/// Read every instrument header and its sample headers, advancing `cursor` past
/// everything read — including each instrument's PCM, for version `0x0104` and later,
/// where it follows that instrument's sample headers.
fn read_instrument_headers<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &XmHeader, cursor: &mut usize) -> Result<Vec<PendingInstrument>, Error> {
    let inline_sample_data = file_header.patterns_precede_instruments();
    let mut instruments = Vec::with_capacity(file_header.instrument_count as usize);

    for _ in 0..file_header.instrument_count {
        let start = *cursor;
        let readable = core::cmp::min(instrument::MAX_HEADER_LENGTH, source.available(start));
        if readable < 4 {
            // Past the end of the file: an empty instrument, numbering preserved, which is
            // what OpenMPT's `if(!file.CanRead(4)) continue;` produces.
            instruments.push(PendingInstrument { name: String::new().into_boxed_str(), header: XmInstrumentHeader::parse(&[]), samples: Vec::new() });
            continue;
        }

        let raw = source.with_slice(start, readable, <[u8]>::to_vec)?;
        let declared = XmInstrumentHeader::parse(&raw).header_size as usize;
        // Fields past the header's own declared size read as zero, exactly as OpenMPT's
        // `ReadStructPartial` leaves them.
        let visible = core::cmp::min(declared, raw.len());
        let header = XmInstrumentHeader::parse(raw.get(..visible).unwrap_or_default());
        let name = instrument::name(&raw, instrument::NAME_OFFSET, instrument::NAME_LENGTH);

        *cursor = start.saturating_add(declared);

        let mut samples = Vec::new();
        for _ in 0..header.sample_count {
            // FastTracker 2, OpenMPT and libxmp all step a fixed 40 bytes per sample
            // header whatever `sample_header_size` says; see the crate documentation.
            if source.available(*cursor) < sample::HEADER_LENGTH {
                break;
            }
            let bytes: [u8; sample::HEADER_LENGTH] = source.array(*cursor)?;
            samples.push(PendingSample {
                header: XmSampleHeader::parse(&bytes),
                name: text(bytes.get(sample::NAME_OFFSET..sample::NAME_OFFSET + sample::NAME_LENGTH).unwrap_or_default()),
                data_offset: 0,
            });
            *cursor = cursor.saturating_add(sample::HEADER_LENGTH);
        }

        if inline_sample_data {
            for pending_sample in samples.iter_mut() {
                pending_sample.data_offset = *cursor;
                *cursor = cursor.saturating_add(pending_sample.header.encoded_bytes());
            }
        }

        instruments.push(PendingInstrument { name, header, samples });
    }
    Ok(instruments)
}

/// Decode one instrument's samples into `builder` and add its [`InstrumentDef`].
fn add_instrument<R: ModuleReader + ?Sized>(source: &Source<'_, R>, pending: &PendingInstrument, builder: &mut ModuleBuilder) -> Result<(), Error> {
    let mut sample_ids = Vec::with_capacity(pending.samples.len());
    for pending_sample in &pending.samples {
        sample_ids.push(add_sample(source, pending_sample, &pending.header, builder)?);
    }

    builder.add_instrument(InstrumentDef {
        name: pending.name.clone(),
        // XM instruments always select their sample through the note map, even when they
        // own exactly one.
        sample: None,
        default_volume: U0F16::MAX,
        note_sample_map: instrument::global_note_sample_map(&pending.header.note_sample_map, &sample_ids),
        volume_envelope: pending.header.volume_envelope.to_model(),
        panning_envelope: pending.header.panning_envelope.to_model(),
        fadeout: pending.header.fadeout,
        ..InstrumentDef::default()
    })?;
    Ok(())
}

/// Decode one sample's PCM and add it to `builder`, answering the global id it was given.
fn add_sample<R: ModuleReader + ?Sized>(source: &Source<'_, R>, pending: &PendingSample, instrument_header: &XmInstrumentHeader, builder: &mut ModuleBuilder) -> Result<u16, Error> {
    let sample_header = pending.header;

    // Frames the header says are there, against frames that actually fit in the file.
    let available_bytes = source.available(pending.data_offset);
    let available_frames = match sample_header.is_adpcm() {
        true => available_bytes.saturating_sub(sample::ADPCM_TABLE_BYTES).saturating_mul(2),
        false => available_bytes / (sample_header.bytes_per_channel_frame() * sample_header.channels()),
    };
    let frames = core::cmp::min(sample_header.frames(), available_frames);

    let pcm = match frames {
        0 => Vec::new(),
        frames => {
            let wanted = core::cmp::min(sample_header.encoded_bytes(), available_bytes);
            source.with_slice(pending.data_offset, wanted, |raw| sample::decode_frames(raw, frames, &sample_header))?
        }
    };

    let loop_end = core::cmp::min(sample_header.loop_end_frames(), pcm.len());
    let loops = sample_header.loops() && sample_header.loop_start_frames() < loop_end;
    let specification = SampleSpec {
        name: pending.name.clone(),
        loop_mode: match (loops, sample_header.is_ping_pong()) {
            (false, _) => LoopMode::None,
            (true, false) => LoopMode::Forward,
            (true, true) => LoopMode::PingPong,
        },
        loop_start: match loops {
            true => sample_header.loop_start_frames() as u32,
            false => 0,
        },
        loop_end: match loops {
            true => loop_end as u32,
            false => 0,
        },
        default_volume: unit_from_ratio(sample_header.volume as u32, 64),
        // Task E1: XM never round-trips its pitch through Hz. The nominal rate is the
        // format default and `relative_note` / `finetune` stay raw, because XM applies
        // both *after* the note is known and in either the linear or the Amiga table.
        reference_rate_hz: starplayer_model::DEFAULT_REFERENCE_RATE_HZ,
        relative_note: sample_header.relative_note,
        finetune: sample_header.finetune,
        default_pan: Some(bipolar_from_ratio(sample_header.pan as i32 - 128, 128)),
        // FastTracker 2 stores one auto-vibrato per instrument and applies it to every
        // sample the instrument owns, so it is copied down onto each of them here.
        auto_vibrato: instrument_header.auto_vibrato,
        sustain_loop: None,
    };

    Ok(builder.add_sample(&pcm, specification)?.0)
}

/// A fixed-width text field, decoded as the code page FastTracker 2 displayed it in. See
/// [`starplayer_model::decode_cp437`] for why that is CP437 rather than Latin-1.
fn text(bytes: &[u8]) -> String { starplayer_model::decode_cp437(bytes) }

/// Bounds-checked reads over a [`ModuleReader`], with the borrowing fast path taken
/// wherever the source offers one.
struct Source<'reader, R: ModuleReader + ?Sized> {
    reader: &'reader R,
}

impl<R: ModuleReader + ?Sized> Source<'_, R> {
    fn len(&self) -> usize { self.reader.len() }

    /// Bytes still available at `offset`, saturating at zero past the end.
    fn available(&self, offset: usize) -> usize { self.reader.len().saturating_sub(offset) }

    /// `N` bytes at `offset`, on the stack. [`Error::Truncated`] if they are not all
    /// there.
    fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], Error> {
        let mut buffer = [0u8; N];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(buffer)
    }

    /// `N` bytes at `offset`, on the stack, zero-padded when fewer are there. Used where
    /// the format's own reading of a short field is "the missing bytes are zero".
    fn padded_array<const N: usize>(&self, offset: usize) -> [u8; N] {
        let mut buffer = [0u8; N];
        let readable = core::cmp::min(N, self.available(offset));
        for (index, byte) in buffer.iter_mut().enumerate().take(readable) {
            let mut one = [0u8; 1];
            if self.reader.read_at(offset + index, &mut one).is_ok() {
                *byte = one[0];
            }
        }
        buffer
    }

    /// Hand `length` bytes at `offset` to `consume`, borrowing them when the source can
    /// and buffering them once when it cannot.
    fn with_slice<T>(&self, offset: usize, length: usize, consume: impl FnOnce(&[u8]) -> T) -> Result<T, Error> {
        if let Some(borrowed) = self.reader.slice_at(offset, length) {
            return Ok(consume(borrowed));
        }
        let mut buffer = vec![0u8; length];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(consume(&buffer))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::pattern::{PatternView, XmCell};
    use starplayer_model::{AutoVibratoWaveform, FormatDialect, PatternId};

    /// A builder for synthetic XMs: enough of one to exercise every branch of the loader
    /// without a fixture file.
    struct XmFile {
        tracker: [u8; 20],
        version: u16,
        channels: u16,
        orders: Vec<u8>,
        patterns: Vec<(u16, Vec<u8>)>,
        instruments: Vec<Instrument>,
    }

    /// A synthetic instrument's volume envelope: its points, then the point count and the
    /// sustain / loop-start / loop-end indices the header carries beside them.
    struct Envelope {
        points: Vec<(u16, u16)>,
        point_count: u8,
        sustain: u8,
        loop_start: u8,
        loop_end: u8,
    }

    struct Instrument {
        name: &'static str,
        samples: Vec<Sample>,
        vibrato: (u8, u8, u8, u8),
        volume_envelope: Option<Envelope>,
    }

    struct Sample {
        deltas: Vec<u8>,
        flags: u8,
        volume: u8,
        pan: u8,
        relative_note: i8,
        finetune: i8,
        loop_start: u32,
        loop_length: u32,
        reserved: u8,
    }

    impl Sample {
        fn simple(deltas: &[u8]) -> Sample {
            Sample { deltas: deltas.to_vec(), flags: 0, volume: 64, pan: 128, relative_note: 0, finetune: 0, loop_start: 0, loop_length: 0, reserved: 0 }
        }
    }

    impl XmFile {
        fn minimal() -> XmFile {
            XmFile {
                tracker: *b"FastTracker v2.00   ",
                version: 0x0104,
                channels: 4,
                orders: vec![0],
                patterns: vec![(64, Vec::new())],
                instruments: Vec::new(),
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![0u8; header::FIXED_HEADER_LENGTH];
            out[..header::MAGIC.len()].copy_from_slice(&header::MAGIC);
            out[header::TITLE_OFFSET..header::TITLE_OFFSET + 4].copy_from_slice(b"test");
            out[header::EOF_MARKER_OFFSET] = header::EOF_MARKER;
            out[38..58].copy_from_slice(&self.tracker);
            out[0x3A..0x3C].copy_from_slice(&self.version.to_le_bytes());
            let header_size = header::MINIMUM_HEADER_SIZE + header::ORDER_TABLE_LENGTH as u32;
            out[0x3C..0x40].copy_from_slice(&header_size.to_le_bytes());
            out[0x40..0x42].copy_from_slice(&(self.orders.len() as u16).to_le_bytes());
            out[0x42..0x44].copy_from_slice(&0u16.to_le_bytes());
            out[0x44..0x46].copy_from_slice(&self.channels.to_le_bytes());
            out[0x46..0x48].copy_from_slice(&(self.patterns.len() as u16).to_le_bytes());
            out[0x48..0x4A].copy_from_slice(&(self.instruments.len() as u16).to_le_bytes());
            out[0x4A..0x4C].copy_from_slice(&1u16.to_le_bytes());
            out[0x4C..0x4E].copy_from_slice(&6u16.to_le_bytes());
            out[0x4E..0x50].copy_from_slice(&125u16.to_le_bytes());
            out.resize(header::FIXED_HEADER_LENGTH + header::ORDER_TABLE_LENGTH, 0);
            out[header::FIXED_HEADER_LENGTH..header::FIXED_HEADER_LENGTH + self.orders.len()].copy_from_slice(&self.orders);

            let patterns_first = self.version >= 0x0104;
            if patterns_first {
                self.append_patterns(&mut out);
            }
            self.append_instruments(&mut out, patterns_first);
            if !patterns_first {
                self.append_patterns(&mut out);
                for instrument in &self.instruments {
                    for sample in &instrument.samples {
                        out.extend_from_slice(&sample.deltas);
                    }
                }
            }
            out
        }

        fn append_patterns(&self, out: &mut Vec<u8>) {
            for (rows, packed) in &self.patterns {
                match self.version == 0x0102 {
                    true => {
                        out.extend_from_slice(&(PATTERN_HEADER_LENGTH_1_02 as u32).to_le_bytes());
                        out.push(0);
                        out.push((*rows - 1) as u8);
                    }
                    false => {
                        out.extend_from_slice(&(PATTERN_HEADER_LENGTH as u32).to_le_bytes());
                        out.push(0);
                        out.extend_from_slice(&rows.to_le_bytes());
                    }
                }
                out.extend_from_slice(&(packed.len() as u16).to_le_bytes());
                out.extend_from_slice(packed);
            }
        }

        fn append_instruments(&self, out: &mut Vec<u8>, inline_sample_data: bool) {
            for instrument in &self.instruments {
                let header_length = match instrument.samples.is_empty() {
                    true => 33usize,
                    false => instrument::MAX_HEADER_LENGTH,
                };
                let mut header = vec![0u8; header_length];
                header[0..4].copy_from_slice(&(header_length as u32).to_le_bytes());
                let name = instrument.name.as_bytes();
                header[4..4 + name.len()].copy_from_slice(name);
                header[0x1B..0x1D].copy_from_slice(&(instrument.samples.len() as u16).to_le_bytes());
                if !instrument.samples.is_empty() {
                    header[0x1D..0x21].copy_from_slice(&40u32.to_le_bytes());
                    for note in 0..instrument::NOTE_MAP_ENTRIES {
                        header[0x21 + note] = (note % instrument.samples.len()) as u8;
                    }
                    if let Some(envelope) = &instrument.volume_envelope {
                        for (index, (tick, value)) in envelope.points.iter().enumerate() {
                            header[0x81 + index * 4..0x81 + index * 4 + 2].copy_from_slice(&tick.to_le_bytes());
                            header[0x81 + index * 4 + 2..0x81 + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
                        }
                        header[0xE1] = envelope.point_count;
                        header[0xE3] = envelope.sustain;
                        header[0xE4] = envelope.loop_start;
                        header[0xE5] = envelope.loop_end;
                        header[0xE9] = instrument::ENVELOPE_ENABLED | instrument::ENVELOPE_SUSTAIN | instrument::ENVELOPE_LOOP;
                    }
                    let (kind, sweep, depth, rate) = instrument.vibrato;
                    header[0xEB] = kind;
                    header[0xEC] = sweep;
                    header[0xED] = depth;
                    header[0xEE] = rate;
                    header[0xEF..0xF1].copy_from_slice(&1024u16.to_le_bytes());
                }
                out.extend_from_slice(&header);

                for sample in &instrument.samples {
                    let mut sample_header = vec![0u8; sample::HEADER_LENGTH];
                    sample_header[0x00..0x04].copy_from_slice(&(sample.deltas.len() as u32).to_le_bytes());
                    sample_header[0x04..0x08].copy_from_slice(&sample.loop_start.to_le_bytes());
                    sample_header[0x08..0x0C].copy_from_slice(&sample.loop_length.to_le_bytes());
                    sample_header[0x0C] = sample.volume;
                    sample_header[0x0D] = sample.finetune as u8;
                    sample_header[0x0E] = sample.flags;
                    sample_header[0x0F] = sample.pan;
                    sample_header[0x10] = sample.relative_note as u8;
                    sample_header[0x11] = sample.reserved;
                    sample_header[sample::NAME_OFFSET..sample::NAME_OFFSET + 4].copy_from_slice(b"samp");
                    out.extend_from_slice(&sample_header);
                }
                if inline_sample_data {
                    for sample in &instrument.samples {
                        out.extend_from_slice(&sample.deltas);
                    }
                }
            }
        }
    }

    fn one_instrument(samples: Vec<Sample>) -> Instrument {
        Instrument { name: "lead", samples, vibrato: (3, 20, 8, 4), volume_envelope: None }
    }

    #[test]
    fn probe_wants_the_signature_and_the_end_of_file_marker() {
        let bytes = XmFile::minimal().bytes();
        assert!(probe(&bytes));
        assert!(probe_reader(&bytes[..]));

        let mut broken = bytes.clone();
        broken[0] = b'x';
        assert!(!probe(&broken));

        let mut broken = bytes.clone();
        broken[header::EOF_MARKER_OFFSET] = 0;
        assert!(!probe(&broken));

        assert!(!probe(&bytes[..header::EOF_MARKER_OFFSET]));
        assert!(!probe(&[]));
        assert!(!probe_reader(&[][..]));
    }

    #[test]
    fn a_minimal_module_loads_with_the_header_the_file_asked_for() {
        let module = load(&XmFile::minimal().bytes()).expect("a minimal XM is a module");

        assert_eq!(module.header().title.as_ref(), "test");
        assert_eq!(module.header().format, ModuleFormat::Xm);
        assert_eq!(module.header().channel_count, 4);
        assert_eq!(module.header().initial_speed, 6);
        assert_eq!(module.header().initial_tempo, 125);
        assert!(module.header().flags.linear_slides);
        assert_eq!(module.header().dialect, FormatDialect::FastTracker2);
        assert!(module.header().default_pan.is_empty(), "XM starts every channel centred");
        assert_eq!(module.patterns().len(), 1);
        assert_eq!(module.orders(), &[0]);
        assert_eq!(XmFormatExtra::from_header(module.header()), XmFormatExtra { restart_position: 0, flags: 1 });
    }

    #[test]
    fn a_file_shorter_than_the_fixed_header_is_truncated_not_a_panic() {
        assert_eq!(load(&[]), Err(Error::Truncated { offset: 0, needed: header::FIXED_HEADER_LENGTH }));
        assert_eq!(load(&[0u8; 79]), Err(Error::Truncated { offset: 0, needed: header::FIXED_HEADER_LENGTH }));
    }

    #[test]
    fn a_file_without_the_signature_is_bad_magic() {
        assert_eq!(load(&[0u8; 128]), Err(Error::BadMagic));
    }

    #[test]
    fn a_file_that_cannot_hold_what_it_declares_is_truncated() {
        let mut bytes = XmFile::minimal().bytes();
        bytes[0x46..0x48].copy_from_slice(&256u16.to_le_bytes());
        bytes.truncate(header::FIXED_HEADER_LENGTH + 4);
        assert!(matches!(load(&bytes), Err(Error::Truncated { .. })));
    }

    #[test]
    fn a_packed_pattern_unpacks_into_the_blob_at_a_fixed_stride() {
        let mut file = XmFile::minimal();
        file.channels = 2;
        // Row 0: channel 0 gets a full record, channel 1 a note-only mask.
        // Row 1: two note-only masks.
        file.patterns = vec![(2, vec![
            49, 1, 0x30, 0x0A, 0xF0,
            pattern::MASK_IS_MASK | pattern::MASK_NOTE, 50,
            pattern::MASK_IS_MASK | pattern::MASK_NOTE, 51,
            pattern::MASK_IS_MASK | pattern::MASK_NOTE, 52,
        ])];
        let module = load(&file.bytes()).expect("a two-row pattern loads");
        let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");

        assert_eq!(view.rows(), 2);
        assert_eq!(view.channels(), 2);
        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, instrument: 1, volume: 0x30, effect: 0x0A, parameter: 0xF0 }));
        assert_eq!(view.cell(0, 1), Some(XmCell { note: 50, ..XmCell::EMPTY }));
        assert_eq!(view.cell(1, 0), Some(XmCell { note: 51, ..XmCell::EMPTY }));
        assert_eq!(view.cell(1, 1), Some(XmCell { note: 52, ..XmCell::EMPTY }));
    }

    #[test]
    fn a_packed_size_of_zero_is_an_all_empty_pattern_of_the_rows_it_declares() {
        let mut file = XmFile::minimal();
        file.patterns = vec![(32, Vec::new())];
        let module = load(&file.bytes()).expect("an empty pattern loads");
        let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");

        assert_eq!(view.rows(), 32);
        assert_eq!(view.cell(0, 0), Some(XmCell::EMPTY));
        assert_eq!(view.cell(31, 3), Some(XmCell::EMPTY));
    }

    #[test]
    fn a_row_count_of_zero_becomes_sixty_four() {
        let mut file = XmFile::minimal();
        file.patterns = vec![(1, Vec::new())];
        let mut bytes = file.bytes();
        let pattern_offset = header::FIXED_HEADER_LENGTH + header::ORDER_TABLE_LENGTH;
        bytes[pattern_offset + 5..pattern_offset + 7].copy_from_slice(&0u16.to_le_bytes());
        let module = load(&bytes).expect("a zero-row pattern loads as 64 rows");
        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(64));
    }

    #[test]
    fn a_pattern_named_by_no_order_still_loads() {
        let mut file = XmFile::minimal();
        file.patterns = vec![(64, Vec::new()), (16, Vec::new())];
        file.orders = vec![0];
        let module = load(&file.bytes()).expect("an unreferenced pattern is still a pattern");
        assert_eq!(module.patterns().len(), 2);
        assert_eq!(module.pattern(PatternId(1)).map(|index| index.rows()), Some(16));
    }

    #[test]
    fn an_order_past_the_pattern_count_plays_one_shared_empty_pattern() {
        let mut file = XmFile::minimal();
        file.patterns = vec![(64, Vec::new())];
        file.orders = vec![0, 5, 200, 0];
        let module = load(&file.bytes()).expect("FastTracker 2 plays an empty pattern for these");

        assert_eq!(module.patterns().len(), 2, "one empty pattern is appended, however many orders need it");
        assert_eq!(module.orders(), &[0, 1, 1, 0]);
        assert_eq!(module.pattern(PatternId(1)).map(|index| index.rows()), Some(64));
    }

    #[test]
    fn an_empty_order_list_plays_pattern_zero_unless_openmpt_wrote_the_file() {
        let mut file = XmFile::minimal();
        file.orders = Vec::new();
        assert_eq!(load(&file.bytes()).expect("a module").orders(), &[0]);

        let mut file = XmFile::minimal();
        file.orders = Vec::new();
        file.tracker = *b"OpenMPT 1.29.13.00  ";
        assert!(load(&file.bytes()).expect("a module").orders().is_empty(), "OpenMPT means an empty order list");
    }

    #[test]
    fn an_eight_bit_sample_delta_decodes_and_widens() {
        let mut file = XmFile::minimal();
        file.instruments = vec![one_instrument(vec![Sample::simple(&[10, 10, (-20i8) as u8])])];
        let module = load(&file.bytes()).expect("a module with one sample");

        let sample = module.sample(starplayer_core::SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.length_frames(), 3);
        assert_eq!(sample.name(), "samp");
        assert_eq!(sample.reference_rate_hz(), starplayer_model::DEFAULT_REFERENCE_RATE_HZ);
        assert_eq!(sample.loop_mode(), LoopMode::None);
        let pcm = module.sample_pcm(starplayer_core::SampleId(0)).expect("its PCM");
        assert_eq!(&pcm[..3], &[10 * 256, 20 * 256, 0]);
    }

    #[test]
    fn a_samples_tuning_and_pan_are_kept_raw_and_its_instruments_vibrato_is_copied_down() {
        let mut file = XmFile::minimal();
        let mut sample = Sample::simple(&[0; 8]);
        sample.relative_note = -12;
        sample.finetune = 64;
        sample.pan = 255;
        sample.volume = 32;
        file.instruments = vec![one_instrument(vec![sample])];
        let module = load(&file.bytes()).expect("a module");

        let sample = module.sample(starplayer_core::SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.relative_note(), -12);
        assert_eq!(sample.finetune(), 64);
        assert_eq!(sample.default_volume(), unit_from_ratio(32, 64));
        assert!(sample.default_pan().is_some_and(|pan| pan > starplayer_core::I1F15::ZERO), "pan 255 is hard right");
        assert_eq!(sample.auto_vibrato().waveform, AutoVibratoWaveform::RampUp, "vibType 3");
        assert_eq!(sample.auto_vibrato().sweep, 20);
        assert_eq!(sample.auto_vibrato().depth, 8);
        assert_eq!(sample.auto_vibrato().rate, 4);
    }

    #[test]
    fn a_volume_above_sixty_four_is_clamped_rather_than_wrapped() {
        let mut file = XmFile::minimal();
        let mut sample = Sample::simple(&[0; 4]);
        sample.volume = 200;
        file.instruments = vec![one_instrument(vec![sample])];
        let module = load(&file.bytes()).expect("a module");
        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.default_volume()), Some(U0F16::MAX));
    }

    #[test]
    fn a_sixteen_bit_ping_pong_loop_halves_every_byte_count() {
        let mut file = XmFile::minimal();
        let mut deltas = Vec::new();
        for value in [100i16, 100, 100, 100, 100, 100, 100, 100] {
            deltas.extend_from_slice(&value.to_le_bytes());
        }
        let sample = Sample {
            deltas,
            flags: sample::FLAG_PING_PONG_LOOP | sample::FLAG_SIXTEEN_BIT,
            volume: 64,
            pan: 128,
            relative_note: 0,
            finetune: 0,
            loop_start: 4,  // bytes -> frame 2
            loop_length: 8, // bytes -> 4 frames
            reserved: 0,
        };
        file.instruments = vec![one_instrument(vec![sample])];
        let module = load(&file.bytes()).expect("a module");

        let sample = module.sample(starplayer_core::SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.loop_mode(), LoopMode::PingPong);
        assert_eq!(sample.loop_start(), 2);
        assert_eq!(sample.loop_end(), 6);
        assert_eq!(sample.length_frames(), 6, "a loop keeps only the frames up to its end");
    }

    #[test]
    fn a_loop_of_zero_length_or_a_loop_kind_of_none_does_not_loop() {
        let mut file = XmFile::minimal();
        let mut looping = Sample::simple(&[0; 8]);
        looping.flags = sample::FLAG_FORWARD_LOOP;
        looping.loop_start = 2;
        looping.loop_length = 0;
        let mut unmarked = Sample::simple(&[0; 8]);
        unmarked.loop_start = 0;
        unmarked.loop_length = 8;
        file.instruments = vec![one_instrument(vec![looping, unmarked])];
        let module = load(&file.bytes()).expect("a module");

        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.loop_mode()), Some(LoopMode::None));
        assert_eq!(module.sample(starplayer_core::SampleId(1)).map(|sample| sample.loop_mode()), Some(LoopMode::None));
    }

    #[test]
    fn a_loop_end_past_the_sample_is_clamped_to_it() {
        let mut file = XmFile::minimal();
        let mut sample = Sample::simple(&[0; 8]);
        sample.flags = sample::FLAG_FORWARD_LOOP;
        sample.loop_start = 2;
        sample.loop_length = 1000;
        file.instruments = vec![one_instrument(vec![sample])];
        let module = load(&file.bytes()).expect("a module");

        let sample = module.sample(starplayer_core::SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.loop_mode(), LoopMode::Forward);
        assert_eq!(sample.loop_end(), 8);
    }

    #[test]
    fn an_instrument_with_no_samples_is_a_valid_silent_instrument() {
        let mut file = XmFile::minimal();
        file.instruments = vec![
            Instrument { name: "empty", samples: Vec::new(), vibrato: (0, 0, 0, 0), volume_envelope: None },
            one_instrument(vec![Sample::simple(&[1, 2, 3])]),
        ];
        let module = load(&file.bytes()).expect("a module");

        assert_eq!(module.instruments().len(), 2, "numbering is preserved");
        let empty = module.instrument(starplayer_core::InstrumentId(0)).expect("instrument 0 exists");
        assert_eq!(empty.name.as_ref(), "empty");
        assert_eq!(empty.sample, None);
        assert!(empty.note_sample_map.iter().all(|entry| *entry == 0));
        assert_eq!(module.samples().len(), 1, "only the second instrument owns a sample");
    }

    #[test]
    fn the_note_map_names_global_sample_ids_one_based() {
        let mut file = XmFile::minimal();
        file.instruments = vec![
            one_instrument(vec![Sample::simple(&[1, 2, 3])]),
            one_instrument(vec![Sample::simple(&[4, 5]), Sample::simple(&[6, 7])]),
        ];
        let module = load(&file.bytes()).expect("a module");

        let second = module.instrument(starplayer_core::InstrumentId(1)).expect("instrument 1 exists");
        // The synthetic file maps note n to local sample n % sample_count, and this
        // instrument's two locals are global samples 1 and 2.
        assert_eq!(second.note_sample_map[0], 2, "local 0 is global sample 1, one-based");
        assert_eq!(second.note_sample_map[1], 3);
        assert_eq!(second.note_sample_map[96], 0, "notes above B-7 are never mapped");
        assert_eq!(second.sample, None, "an XM instrument always goes through the map");
    }

    #[test]
    fn an_enabled_volume_envelope_reaches_the_instrument() {
        let mut file = XmFile::minimal();
        let mut instrument = one_instrument(vec![Sample::simple(&[1, 2, 3])]);
        instrument.volume_envelope = Some(Envelope { points: vec![(0, 64), (8, 32), (16, 0)], point_count: 3, sustain: 1, loop_start: 0, loop_end: 2 });
        file.instruments = vec![instrument];
        let module = load(&file.bytes()).expect("a module");

        let definition = module.instrument(starplayer_core::InstrumentId(0)).expect("instrument 0 exists");
        let envelope = definition.volume_envelope.as_ref().expect("the envelope is on");
        assert_eq!(envelope.points.len(), 3);
        assert_eq!(envelope.points[1], starplayer_model::EnvelopePoint { tick: 8, value: 32 });
        assert_eq!(envelope.sustain, Some(starplayer_model::EnvelopeSpan { start: 1, end: 1 }));
        assert_eq!(envelope.loop_span, Some(starplayer_model::EnvelopeSpan { start: 0, end: 2 }));
        assert_eq!(definition.fadeout, 1024);
        assert_eq!(definition.panning_envelope, None, "the file leaves the panning envelope off");
    }

    #[test]
    fn version_one_oh_two_reads_its_rows_from_one_byte_and_its_pcm_after_the_patterns() {
        let mut file = XmFile::minimal();
        file.version = 0x0102;
        file.channels = 1;
        file.patterns = vec![(4, vec![pattern::MASK_IS_MASK | pattern::MASK_NOTE, 49])];
        file.instruments = vec![one_instrument(vec![Sample::simple(&[10, 10])])];
        let module = load(&file.bytes()).expect("a 1.02 file is still an XM");

        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(4));
        let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");
        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, ..XmCell::EMPTY }));

        let pcm = module.sample_pcm(starplayer_core::SampleId(0)).expect("its PCM");
        assert_eq!(&pcm[..2], &[10 * 256, 20 * 256], "the PCM is at the very end, after the patterns");
    }

    #[test]
    fn version_one_oh_three_uses_the_word_row_count_with_the_old_data_order() {
        let mut file = XmFile::minimal();
        file.version = 0x0103;
        file.channels = 1;
        file.patterns = vec![(96, Vec::new())];
        file.instruments = vec![one_instrument(vec![Sample::simple(&[5, 5, 5])])];
        let module = load(&file.bytes()).expect("a 1.03 file loads");

        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(96));
        let pcm = module.sample_pcm(starplayer_core::SampleId(0)).expect("its PCM");
        assert_eq!(&pcm[..3], &[5 * 256, 10 * 256, 15 * 256]);
    }

    #[test]
    fn sample_data_running_past_the_end_of_the_file_is_clamped_to_what_is_there() {
        let mut file = XmFile::minimal();
        file.instruments = vec![one_instrument(vec![Sample::simple(&[1, 1, 1, 1, 1, 1, 1, 1])])];
        let mut bytes = file.bytes();
        bytes.truncate(bytes.len() - 5);
        let module = load(&bytes).expect("a truncated tail is not a broken module");
        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.length_frames()), Some(3));
    }

    #[test]
    fn a_declared_sample_whose_header_is_past_the_end_is_simply_not_there() {
        let mut file = XmFile::minimal();
        file.instruments = vec![one_instrument(vec![Sample::simple(&[1, 2, 3]), Sample::simple(&[4, 5, 6])])];
        let mut bytes = file.bytes();
        // Leaves the first sample header whole and two bytes of the second.
        bytes.truncate(bytes.len() - 44);
        let module = load(&bytes).expect("a module");
        assert_eq!(module.samples().len(), 1, "the second sample's header did not fit");
        assert_eq!(module.instruments().len(), 1);
    }

    #[test]
    fn an_adpcm_sample_is_decoded_rather_than_refused() {
        let mut file = XmFile::minimal();
        let mut table = vec![0u8; sample::ADPCM_TABLE_BYTES];
        table[1] = 4;
        table[2] = (-8i8) as u8;
        table.push(0x21); // nybbles 1 then 2
        let mut adpcm = Sample::simple(&table);
        adpcm.reserved = sample::RESERVED_ADPCM;
        file.instruments = vec![one_instrument(vec![adpcm])];
        let mut bytes = file.bytes();
        // The header's `length` is a frame count for ADPCM, not a byte count.
        let length_offset = bytes.len() - (sample::ADPCM_TABLE_BYTES + 1) - sample::HEADER_LENGTH;
        bytes[length_offset..length_offset + 4].copy_from_slice(&2u32.to_le_bytes());
        let module = load(&bytes).expect("ModPlug ADPCM is decoded");

        let pcm = module.sample_pcm(starplayer_core::SampleId(0)).expect("its PCM");
        assert_eq!(&pcm[..2], &[4 * 256, -4 * 256]);
    }

    #[test]
    fn a_stereo_sample_is_averaged_into_the_mono_frame_the_mixer_plays() {
        let mut file = XmFile::minimal();
        let mut stereo = Sample::simple(&[10, 10, 30, 30]);
        stereo.flags = sample::FLAG_STEREO;
        file.instruments = vec![one_instrument(vec![stereo])];
        let module = load(&file.bytes()).expect("a stereo sample loads");

        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.length_frames()), Some(2));
        let pcm = module.sample_pcm(starplayer_core::SampleId(0)).expect("its PCM");
        assert_eq!(&pcm[..2], &[20 * 256, 40 * 256]);
    }

    #[test]
    fn a_speed_or_tempo_of_zero_falls_back_and_a_tempo_out_of_range_is_clamped() {
        let mut bytes = XmFile::minimal().bytes();
        bytes[0x4C..0x4E].copy_from_slice(&0u16.to_le_bytes());
        bytes[0x4E..0x50].copy_from_slice(&0u16.to_le_bytes());
        let module = load(&bytes).expect("a module");
        assert_eq!(module.header().initial_speed, 6);
        assert_eq!(module.header().initial_tempo, 125);

        let mut bytes = XmFile::minimal().bytes();
        bytes[0x4E..0x50].copy_from_slice(&10u16.to_le_bytes());
        assert_eq!(load(&bytes).expect("a module").header().initial_tempo, 32);

        let mut bytes = XmFile::minimal().bytes();
        bytes[0x4E..0x50].copy_from_slice(&1000u16.to_le_bytes());
        assert_eq!(load(&bytes).expect("a module").header().initial_tempo, 255);
    }

    #[test]
    fn a_restart_position_survives_in_the_format_extra_word() {
        let mut bytes = XmFile::minimal().bytes();
        bytes[0x42..0x44].copy_from_slice(&7u16.to_le_bytes());
        let module = load(&bytes).expect("a module");
        assert_eq!(XmFormatExtra::from_header(module.header()).restart_position, 7);
    }

    #[test]
    fn trailing_chunks_after_the_last_sample_are_ignored() {
        let mut file = XmFile::minimal();
        file.instruments = vec![one_instrument(vec![Sample::simple(&[1, 2, 3])])];
        let mut bytes = file.bytes();
        bytes.extend_from_slice(b"text");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(b"a song comment!!");
        let module = load(&bytes).expect("a module with an OpenMPT text chunk");
        assert_eq!(module.samples().len(), 1);
        assert_eq!(module.instruments().len(), 1);
    }

    #[test]
    fn a_pattern_header_that_is_too_short_stops_the_pattern_run_without_losing_the_count() {
        let mut file = XmFile::minimal();
        file.patterns = vec![(64, Vec::new()), (32, Vec::new())];
        let mut bytes = file.bytes();
        let pattern_offset = header::FIXED_HEADER_LENGTH + header::ORDER_TABLE_LENGTH;
        bytes[pattern_offset..pattern_offset + 4].copy_from_slice(&4u32.to_le_bytes());
        let module = load(&bytes).expect("a module whose patterns are unreadable is still a module");

        assert_eq!(module.patterns().len(), 2, "the declared count is what the order list indexes");
        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(64));
        assert_eq!(module.pattern(PatternId(1)).map(|index| index.rows()), Some(64));
    }

    // ── the production allocation cap ───────────────────────────────────────────────

    /// A header declaring `pattern_count` patterns whose headers are all present but
    /// which carry no packed data at all — the cheapest amplification the format allows.
    fn amplifying_xm(pattern_count: u16, rows: u16, channels: u16) -> Vec<u8> {
        let mut file = XmFile::minimal();
        file.channels = channels;
        file.orders = vec![0];
        file.patterns = (0..pattern_count).map(|_| (rows, Vec::new())).collect();
        file.bytes()
    }

    #[test]
    fn a_pattern_declaration_the_file_cannot_justify_is_refused_rather_than_allocated() {
        // 256 patterns of 256 rows and 64 channels is 21 MB of decoded cells out of a file
        // of under 3 KB. The budget for a file that small is the 8 MiB floor.
        let bytes = amplifying_xm(256, 256, 64);
        assert!(bytes.len() < 3 * 1024, "the whole attack fits in 3 KB: {} bytes", bytes.len());
        assert_eq!(load(&bytes), Err(Error::TooLarge("XM pattern data")));
    }

    #[test]
    fn a_pattern_declaration_a_real_module_would_use_is_still_accepted() {
        // FastTracker 2's own limits: 256 patterns of 64 rows, 32 channels.
        let module = load(&amplifying_xm(256, 64, 32)).expect("256 empty 32-channel patterns are a plausible module");
        assert_eq!(module.patterns().len(), 256);
        assert_eq!(module.header().channel_count, 32);
    }

    #[test]
    fn the_budget_is_a_floor_below_which_file_size_does_not_matter() {
        assert_eq!(decoded_pattern_budget(0), MINIMUM_PATTERN_BUDGET_BYTES);
        assert_eq!(decoded_pattern_budget(1_024), MINIMUM_PATTERN_BUDGET_BYTES, "a small file still gets the floor");
        assert_eq!(decoded_pattern_budget(1_000_000), 64_000_000, "a big one earns its size instead");
        assert_eq!(decoded_pattern_budget(usize::MAX), usize::MAX, "and the arithmetic saturates rather than wrapping");
    }

    #[test]
    fn a_fixed_field_becomes_text_without_its_padding() {
        assert_eq!(text(b"Reflex\0\0\0"), "Reflex");
        assert_eq!(text(b"Reflex   "), "Reflex");
        assert_eq!(text(b""), "");
    }
}

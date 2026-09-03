//! `load` — bytes in, [`Module`] out.
//!
//! # The layout this walks
//!
//! ```text
//! 0x00  "IMPM"   0x04 song name, 26 bytes   0x1E PHiligt
//! 0x20  OrdNum   0x22 InsNum   0x24 SmpNum   0x26 PatNum
//! 0x28  Cwt/v    0x2A Cmwt     0x2C Flags    0x2E Special
//! 0x30  GV  0x31 MV  0x32 IS  0x33 IT  0x34 Sep  0x35 PWD
//! 0x36  MsgLgth  0x38 Message Offset  0x3C Reserved
//! 0x40  ChnPan[64]      0x80 ChnVol[64]
//! 0xC0  OrdNum order bytes
//!       InsNum instrument offsets (u32)
//!       SmpNum sample-header offsets (u32)
//!       PatNum pattern offsets (u32)
//!       [u16 count + count x 8 bytes of edit history, iff Special bit 1]
//!       [4896 bytes of MIDI configuration, iff Flags bit 7 or Special bit 3]
//!       … the song message, instrument headers, sample headers, sample data and
//!         patterns, at their own offsets, in any order
//! ```
//!
//! Every offset is a plain byte offset from the start of the file, unlike S3M's
//! paragraph parapointers.
//!
//! # Two passes over the patterns
//!
//! An IT file never says how many channels it uses: `ChnPan` and `ChnVol` are always 64
//! entries long, and the answer is "the highest channel any pattern writes to, plus one".
//! So the patterns are **scanned** first — a walk that allocates nothing and only counts —
//! and only then unpacked, at the fixed stride that scan produced. That is also what makes
//! the decoded-pattern budget exact before a byte of it is allocated.
//!
//! # Clamp or reject
//!
//! The rule is S3M's: *clamp wherever a tracker would have played the file, reject only
//! where the file contradicts itself*. Every case, and nothing else fails:
//!
//! | Case | Behaviour |
//! |---|---|
//! | No `IMPM` at offset 0, or fewer than `0xC0` bytes | [`Error::BadMagic`] / [`Error::Truncated`] |
//! | Order list or an offset table running past EOF | [`Error::Truncated`] — an `InsNum`, `SmpNum` or `PatNum` the file cannot support is exactly this |
//! | More decoded pattern data than the file's own size can justify | [`Error::TooLarge`] — see [`MINIMUM_PATTERN_BUDGET_BYTES`] |
//! | More decoded sample frames than the file's own size can justify | clamped, sample by sample — see [`MINIMUM_PCM_BUDGET_FRAMES`] |
//! | `IS` of 0 | clamped to 6; a speed of zero advances no rows |
//! | `IT` below 32 | clamped to 32, the sequencer's own floor |
//! | `GV` or `MV` above 128 | clamped to 128 |
//! | `ChnPan` above 64 that is not 100 (surround) | centred; the raw byte is kept in `format_data` |
//! | A mono module (`Flags` bit 0 clear) | every channel centred, spelled as an empty pan table |
//! | Order naming a pattern that does not exist | becomes [`ORDER_MARKER`], which the sequencer steps over |
//! | Instrument offset of 0, or a header past EOF | empty instrument slot, numbering preserved |
//! | An instrument in sample mode | ignored; one instrument per sample is synthesised instead, so `InstrumentId(n - 1)` is sample *n* |
//! | A keyboard entry naming a sample the file does not have | "no sample" for that key |
//! | Sample offset of 0, or a header past EOF | empty sample slot, numbering preserved |
//! | `C5Speed` of 0 | replaced with 8363; below 256, raised to 256 (`ITSample::ConvertToMPT`'s own floor) |
//! | Sample data running past EOF | clamped to what is there |
//! | A truncated or malformed compressed block | decodes to a shorter sample; never a failure and never an allocation the data cannot justify |
//! | A stereo sample | downmixed `(left + right) / 2` — accuracy policy **D60** |
//! | `loop_end` past the sample's length | clamped to the length |
//! | `loop_start >= loop_end` after clamping | the sample does not loop; the same rule for the sustain loop |
//! | Pattern offset of 0 | an empty 64-row pattern, which is what Impulse Tracker means by it |
//! | Pattern offset past EOF, or a header that does not fit | an empty 64-row pattern |
//! | Pattern rows outside `1..=256` | an empty 64-row pattern — research point 5 |
//! | A packed length that overruns the file | clamped to what is there |
//! | A packed stream that ends early, overruns its rows, or names a channel the song does not have | see [`crate::pattern::unpack`] |
//! | A module no pattern writes to | one channel, so the module still validates |

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, I1F15, SampleId, U0F16};
use starplayer_model::{
    AutoVibrato, InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader,
    ModuleReader, ORDER_END, ORDER_MARKER, SampleSpec, SustainLoop,
};

use crate::compression;
use crate::header::{self, ItFormatExtra, ItHeader, MAX_CHANNELS};
use crate::instrument::{self, ItInstrument};
use crate::pattern::{self, DEFAULT_ROWS, MAX_ROWS};
use crate::sample::{self, ItSampleHeader};

/// Lowest tempo the sequencer's tick arithmetic expects, matching the S3M loader's floor.
/// ITTECH.TXT allows 31; `Load_it.cpp` clamps to 31 too, and one BPM either way is below
/// what any file in the corpus asks for.
const MINIMUM_TEMPO: u16 = 32;

/// Speed a file that asks for zero ticks per row is given instead.
const FALLBACK_SPEED: u8 = 6;

/// Largest `GV` / `MV` the format defines.
const MAX_SONG_VOLUME: u8 = 128;

/// Largest `ChnVol` the format defines.
const MAX_CHANNEL_VOLUME: u8 = 64;

/// Bytes in one pattern's own header, before its packed stream.
const PATTERN_HEADER_BYTES: usize = 8;

/// Decoded pattern bytes a file is allowed regardless of how small it is.
///
/// The same reasoning as the S3M loader's, at IT's dimensions: `PatNum` is a `u16` and a
/// pattern costs **four bytes** of offset, so a 256 KB file may declare 65,535 patterns;
/// an offset of zero is not even malformed — Impulse Tracker spells an empty 64-row
/// pattern exactly that way — and each one unpacks to `64 x 64 x 5` bytes, which is 1.3 GB
/// of decoded patterns out of a file that fits on a floppy.
///
/// The floor has to clear a *legitimate* file's worst case, which is bigger than S3M's:
/// Impulse Tracker allows 240 patterns of up to 200 rows over 64 channels, or 15.4 MB, and
/// this loader accepts 256 rows, or 19.7 MB. Thirty-two mebibytes clears that and still
/// refuses the amplification above, whose 256 KB of offsets earn 16 MB.
const MINIMUM_PATTERN_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// Decoded pattern bytes a file earns per byte of its own size, once it is big enough for
/// that to beat [`MINIMUM_PATTERN_BUDGET_BYTES`]. Packed IT patterns are roughly a fifth
/// of their decoded size, so 64x leaves two orders of magnitude of headroom.
const PATTERN_BUDGET_PER_FILE_BYTE: usize = 64;

/// Decoded sample frames a file is allowed regardless of how small it is.
///
/// `SmpNum` samples may all point at the same bytes, and a compressed sample amplifies:
/// the narrowest IT 2.14 residual is one bit, so a byte of compressed data can become
/// eight frames — sixteen bytes of `i16`. Without a *shared* budget a 100 KB file with
/// 4,000 sample slots all aimed at the same compressed block would decode to gigaframes.
/// Eight mebiframes is 16 MB of PCM, far more than any file this small can honestly hold.
const MINIMUM_PCM_BUDGET_FRAMES: usize = 8 * 1024 * 1024;

/// Decoded sample frames a file earns per byte of its own size. Sixteen is twice the
/// worst case a compressed stream can reach, so no real module is ever clamped.
const PCM_BUDGET_PER_FILE_BYTE: usize = 16;

/// The decoded-pattern budget for a file of `file_bytes`.
fn decoded_pattern_budget(file_bytes: usize) -> usize {
    file_bytes.saturating_mul(PATTERN_BUDGET_PER_FILE_BYTE).max(MINIMUM_PATTERN_BUDGET_BYTES)
}

/// The decoded-PCM budget, in frames, for a file of `file_bytes`.
fn decoded_pcm_budget(file_bytes: usize) -> usize {
    file_bytes.saturating_mul(PCM_BUDGET_PER_FILE_BYTE).max(MINIMUM_PCM_BUDGET_FRAMES)
}

/// Whether `bytes` looks like an IT: `IMPM` at offset 0.
///
/// Cheap and total — it reads four bytes and never allocates — so a host can use it to
/// pick a loader before committing to one.
pub fn probe(bytes: &[u8]) -> bool {
    bytes.get(header::MAGIC_OFFSET..header::MAGIC_OFFSET + header::MAGIC.len()) == Some(&header::MAGIC[..])
}

/// Whether `reader` looks like an IT. The [`ModuleReader`] form of [`probe`].
pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool {
    let mut magic = [0u8; 4];
    reader.read_at(header::MAGIC_OFFSET, &mut magic).is_ok() && magic == header::MAGIC
}

/// Load an IT from bytes already in memory.
pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_from(bytes) }

/// Load an IT from any [`ModuleReader`].
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

    let fixed_header: [u8; header::HEADER_LENGTH] = source.array(0)?;
    let file_header = ItHeader::parse(&fixed_header)?;

    let order_offset = header::HEADER_LENGTH;
    let instrument_table = order_offset + file_header.order_count as usize;
    let sample_table = instrument_table + 4 * file_header.instrument_count as usize;
    let pattern_table = sample_table + 4 * file_header.sample_count as usize;
    let tables_end = pattern_table + 4 * file_header.pattern_count as usize;
    if tables_end > source.len() {
        // A count bigger than the file can hold lands here, which is the whole point of
        // checking the tables as one region.
        return Err(Error::Truncated { offset: order_offset, needed: tables_end - order_offset });
    }

    let orders = source.with_slice(order_offset, file_header.order_count as usize, <[u8]>::to_vec)?;
    let instrument_offsets = source.offsets(instrument_table, file_header.instrument_count)?;
    let sample_offsets = source.offsets(sample_table, file_header.sample_count)?;
    let pattern_offsets = source.offsets(pattern_table, file_header.pattern_count)?;

    let midi_configuration = read_midi_configuration(&source, &file_header, tables_end, &instrument_offsets, &sample_offsets, &pattern_offsets)?;

    // Pass one: how wide is this song, and what will its patterns cost?
    let scan = scan_patterns(&source, &pattern_offsets)?;
    let channel_count = scan.channel_count.clamp(1, MAX_CHANNELS as u8);
    let decoded_pattern_bytes = scan.decoded_rows
        .saturating_mul(channel_count as usize)
        .saturating_mul(pattern::CELL_BYTES);
    if decoded_pattern_bytes > decoded_pattern_budget(source.len()) {
        return Err(Error::TooLarge("IT pattern data"));
    }

    let mut builder = ModuleBuilder::new();

    let mut sample_global_volumes = Vec::with_capacity(sample_offsets.len());
    let mut sample_names: Vec<String> = Vec::with_capacity(sample_offsets.len());
    let mut pcm_budget = decoded_pcm_budget(source.len());
    for offset in &sample_offsets {
        let summary = load_sample(&source, *offset as usize, file_header.tracker_version, &mut pcm_budget, &mut builder)?;
        sample_global_volumes.push(summary.global_volume);
        sample_names.push(summary.name);
    }

    match file_header.is_instrument_mode() {
        true => {
            for offset in &instrument_offsets {
                load_instrument(&source, *offset as usize, file_header.has_old_instruments(), sample_offsets.len(), &mut builder)?;
            }
        }
        false => {
            // Sample mode: the instrument column names a sample, so one instrument per
            // sample keeps `InstrumentId(n - 1)` meaning sample *n*, exactly as S3M does.
            for (index, name) in sample_names.iter().enumerate() {
                builder.add_instrument(InstrumentDef::from_sample(name, SampleId(index as u16), U0F16::MAX))?;
            }
        }
    }

    for offset in &pattern_offsets {
        load_pattern(&source, *offset as usize, channel_count, &mut builder)?;
    }

    let pattern_count = pattern_offsets.len();
    let order_entries: Vec<u16> = orders.iter()
        .map(|order| match *order {
            255 => ORDER_END,
            254 => ORDER_MARKER,
            order if (order as usize) < pattern_count => order as u16,
            _ => ORDER_MARKER,
        })
        .collect();
    builder.set_orders(&order_entries);
    builder.set_header(song_header(&source, &file_header, channel_count, &sample_global_volumes, midi_configuration.as_deref())?);
    builder.build()
}

/// What pass one over the patterns produced.
struct PatternScan {
    /// The highest channel any pattern writes to, plus one. Zero when nothing is written.
    channel_count: u8,
    /// Rows every pattern will unpack to, summed — the decoded cost, less the stride.
    decoded_rows: usize,
}

/// Walk every pattern's packed stream without allocating, to find the song's channel count
/// and its decoded size.
fn scan_patterns<R: ModuleReader + ?Sized>(source: &Source<'_, R>, offsets: &[u32]) -> Result<PatternScan, Error> {
    let mut channel_count = 0u8;
    let mut decoded_rows = 0usize;

    for offset in offsets {
        let Some((body_offset, body_length, rows)) = pattern_extent(source, *offset as usize) else {
            decoded_rows = decoded_rows.saturating_add(DEFAULT_ROWS as usize);
            continue;
        };
        decoded_rows = decoded_rows.saturating_add(rows as usize);
        let used = source.with_slice(body_offset, body_length, |body| pattern::used_channels(body, rows))?;
        channel_count = core::cmp::max(channel_count, used);
    }

    Ok(PatternScan { channel_count, decoded_rows })
}

/// Where one pattern's packed stream lives and how many rows it fills, or `None` for a
/// pattern this loader turns into an empty 64-row one.
fn pattern_extent<R: ModuleReader + ?Sized>(source: &Source<'_, R>, offset: usize) -> Option<(usize, usize, u16)> {
    if offset == 0 || offset.checked_add(PATTERN_HEADER_BYTES)? > source.len() {
        return None;
    }
    let bytes: [u8; PATTERN_HEADER_BYTES] = source.array(offset).ok()?;
    let packed_length = u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]) as usize;
    let rows = u16::from_le_bytes([*bytes.get(2)?, *bytes.get(3)?]);
    if rows == 0 || rows > MAX_ROWS {
        // Research point 5: `Load_it.cpp` and libxmp both skip a pattern whose row count is
        // outside what the format can express rather than clamping it into a different
        // shape, and so does this.
        return None;
    }
    let body_offset = offset + PATTERN_HEADER_BYTES;
    let body_length = core::cmp::min(packed_length, source.len().saturating_sub(body_offset));
    Some((body_offset, body_length, rows))
}

/// Read the embedded MIDI configuration, if the file has one.
///
/// The block sits after the offset tables, behind an optional edit-history block whose
/// presence bit some trackers set without writing anything. `Load_it.cpp` resolves that by
/// checking the history against the **first parapointer**: if the history would end past
/// the earliest thing anything else points at, it was never there.
fn read_midi_configuration<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &ItHeader, tables_end: usize, instruments: &[u32], samples: &[u32], patterns: &[u32]) -> Result<Option<Vec<u8>>, Error> {
    let mut cursor = tables_end;

    if file_header.special & header::SPECIAL_EDIT_HISTORY != 0 {
        let first_pointer = [instruments, samples, patterns].into_iter()
            .flatten()
            .copied()
            .filter(|pointer| *pointer > 0)
            .chain(match file_header.special & header::SPECIAL_SONG_MESSAGE != 0 {
                true => Some(file_header.message_offset),
                false => None,
            })
            .min()
            .unwrap_or(u32::MAX) as usize;

        if let Ok(count) = source.array::<2>(cursor) {
            let entries = u16::from_le_bytes(count) as usize;
            let end = cursor.saturating_add(2).saturating_add(entries.saturating_mul(header::EDIT_HISTORY_ENTRY_BYTES));
            if end <= source.len() && end <= first_pointer {
                cursor = end;
            }
        }
    }

    if !file_header.has_midi_configuration() {
        return Ok(None);
    }
    if cursor.saturating_add(header::MIDI_CONFIGURATION_BYTES) > source.len() {
        return Ok(None);
    }
    Ok(Some(source.with_slice(cursor, header::MIDI_CONFIGURATION_BYTES, <[u8]>::to_vec)?))
}

/// Build the format-neutral [`ModuleHeader`] out of the file header.
fn song_header<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &ItHeader, channel_count: u8, sample_global_volumes: &[u8], midi_configuration: Option<&[u8]>) -> Result<ModuleHeader, Error> {
    let title = source.with_slice(0x04, 26, text)?;

    // A mono module is centred on every channel, which the model spells as an empty table
    // rather than as a run of zeroes — the same rule the S3M loader applies.
    let default_pan = match file_header.is_stereo() {
        false => Vec::<I1F15>::new().into_boxed_slice(),
        true => file_header.channel_pan.iter()
            .take(channel_count as usize)
            .map(|raw| header::pan_to_bipolar(*raw))
            .collect::<Vec<I1F15>>()
            .into_boxed_slice(),
    };

    let default_channel_volume: Vec<U0F16> = file_header.channel_volume.iter()
        .take(channel_count as usize)
        .map(|volume| unit_from_ratio(core::cmp::min(*volume, MAX_CHANNEL_VOLUME) as u32, MAX_CHANNEL_VOLUME as u32))
        .collect();

    let extra = ItFormatExtra {
        flags: file_header.flags,
        special: file_header.special as u8,
        old_instruments: file_header.has_old_instruments(),
        has_midi_configuration: midi_configuration.is_some(),
    };

    Ok(ModuleHeader {
        title: title.into_boxed_str(),
        format: ModuleFormat::It,
        channel_count,
        initial_speed: match file_header.initial_speed {
            0 => FALLBACK_SPEED,
            speed => speed,
        },
        initial_tempo: core::cmp::max(file_header.initial_tempo as u16, MINIMUM_TEMPO),
        global_volume: unit_from_ratio(core::cmp::min(file_header.global_volume, MAX_SONG_VOLUME) as u32, MAX_SONG_VOLUME as u32),
        master_volume: unit_from_ratio(core::cmp::min(file_header.mix_volume, MAX_SONG_VOLUME) as u32, MAX_SONG_VOLUME as u32),
        default_pan,
        flags: ModuleFlags {
            amiga_limits: false,
            linear_slides: file_header.is_linear_slides(),
            fast_volume_slides: false,
            stereo: file_header.is_stereo(),
        },
        dialect: file_header.dialect(),
        format_extra: extra.encode(),
        default_channel_volume: default_channel_volume.into_boxed_slice(),
        format_data: header::encode_format_data(file_header, sample_global_volumes, midi_configuration).into_boxed_slice(),
    })
}

/// What one sample slot contributed beyond its PCM: the `GvL`
/// [`header::encode_format_data`] carries to the effect processor, and the name sample
/// mode's synthesised instruments take.
struct SampleSummary {
    global_volume: u8,
    name: String,
}

/// Read one sample slot and add its PCM to `builder`.
///
/// One sample is added per slot whatever the slot holds, so the file's one-based sample
/// numbers stay `SampleId(number - 1)` for the instrument keyboard tables and for sample
/// mode's synthesised instruments.
fn load_sample<R: ModuleReader + ?Sized>(source: &Source<'_, R>, header_offset: usize, tracker_version: u16, pcm_budget: &mut usize, builder: &mut ModuleBuilder) -> Result<SampleSummary, Error> {
    if header_offset == 0 || header_offset.saturating_add(sample::HEADER_LENGTH) > source.len() {
        builder.add_sample(&[], SampleSpec::one_shot(""))?;
        return Ok(SampleSummary { global_volume: MAX_CHANNEL_VOLUME, name: String::new() });
    }

    let bytes: [u8; sample::HEADER_LENGTH] = source.array(header_offset)?;
    let sample_header = ItSampleHeader::parse(&bytes)?;
    let name = source.with_slice(header_offset + 0x14, 26, text)?;

    let pcm = decode_sample_pcm(source, &sample_header, tracker_version, pcm_budget)?;
    *pcm_budget = pcm_budget.saturating_sub(pcm.len());

    let loop_end = core::cmp::min(sample_header.loop_end as usize, pcm.len()) as u32;
    let loops = sample_header.loops() && sample_header.loop_start < loop_end;
    let sustain_end = core::cmp::min(sample_header.sustain_end as usize, pcm.len()) as u32;
    let sustains = sample_header.sustain_loops() && sample_header.sustain_start < sustain_end;

    let specification = SampleSpec {
        name: name.clone(),
        loop_mode: match (loops, sample_header.loop_is_ping_pong()) {
            (false, _) => LoopMode::None,
            (true, false) => LoopMode::Forward,
            (true, true) => LoopMode::PingPong,
        },
        loop_start: match loops {
            true => sample_header.loop_start,
            false => 0,
        },
        loop_end: match loops {
            true => loop_end,
            false => 0,
        },
        default_volume: unit_from_ratio(core::cmp::min(sample_header.volume, MAX_CHANNEL_VOLUME) as u32, MAX_CHANNEL_VOLUME as u32),
        reference_rate_hz: sample_header.reference_rate_hz(),
        relative_note: 0,
        finetune: 0,
        default_pan: sample_header.pan_position().map(header::pan_to_bipolar),
        auto_vibrato: AutoVibrato {
            waveform: sample_header.auto_vibrato_waveform(),
            sweep: sample_header.vibrato_sweep,
            depth: sample_header.vibrato_depth & 0x7F,
            rate: sample_header.vibrato_speed,
        },
        sustain_loop: match sustains {
            true => Some(SustainLoop {
                mode: match sample_header.sustain_is_ping_pong() {
                    true => LoopMode::PingPong,
                    false => LoopMode::Forward,
                },
                start: sample_header.sustain_start,
                end: sustain_end,
            }),
            false => None,
        },
    };

    builder.add_sample(&pcm, specification)?;
    Ok(SampleSummary { global_volume: core::cmp::min(sample_header.global_volume, MAX_CHANNEL_VOLUME), name })
}

/// Decode one sample's PCM: raw or compressed, mono or downmixed stereo.
fn decode_sample_pcm<R: ModuleReader + ?Sized>(source: &Source<'_, R>, sample_header: &ItSampleHeader, tracker_version: u16, pcm_budget: &usize) -> Result<Vec<i16>, Error> {
    let data_offset = sample_header.data_offset as usize;
    if sample_header.length == 0 || data_offset == 0 || data_offset >= source.len() {
        return Ok(Vec::new());
    }
    let available = source.len() - data_offset;
    let channels = sample_header.stored_channels(tracker_version);

    if sample_header.is_compressed() {
        // The compressed length is nowhere in the file: it is the sum of the block headers
        // walked, so the decoder is handed everything from the sample pointer onwards and
        // stops itself. It allocates only what it decodes, so a four-gigaframe claim over
        // an empty tail costs nothing.
        let frames = core::cmp::min(sample_header.length as usize, *pcm_budget);
        let wide = sample_header.is_sixteen_bit();
        let it215 = sample_header.is_delta();
        return source.with_slice(data_offset, available, |data| {
            let (left, consumed) = compression::decompress(data, frames, wide, it215);
            match channels {
                2 => {
                    let (right, _) = compression::decompress(data.get(consumed..).unwrap_or_default(), frames, wide, it215);
                    sample::downmix(&left, &right)
                }
                _ => left,
            }
        });
    }

    let width = match sample_header.is_sixteen_bit() {
        true => 2,
        false => 1,
    };
    let frames_in_file = available / (width * channels);
    let frames = core::cmp::min(core::cmp::min(sample_header.length as usize, frames_in_file), *pcm_budget);
    if frames == 0 {
        return Ok(Vec::new());
    }
    let block = frames * width;
    source.with_slice(data_offset, block * channels, |raw| match channels {
        2 => {
            let left = sample::decode_frames(raw.get(..block).unwrap_or_default(), frames, sample_header);
            let right = sample::decode_frames(raw.get(block..).unwrap_or_default(), frames, sample_header);
            sample::downmix(&left, &right)
        }
        _ => sample::decode_frames(raw, frames, sample_header),
    })
}

/// Read one instrument slot and add its [`InstrumentDef`] to `builder`.
fn load_instrument<R: ModuleReader + ?Sized>(source: &Source<'_, R>, header_offset: usize, old_format: bool, sample_count: usize, builder: &mut ModuleBuilder) -> Result<(), Error> {
    if header_offset == 0 || header_offset.saturating_add(instrument::HEADER_LENGTH) > source.len() {
        builder.add_instrument(InstrumentDef::default())?;
        return Ok(());
    }
    let name = source.with_slice(header_offset + 0x20, 26, text)?;
    let parsed = source.with_slice(header_offset, instrument::HEADER_LENGTH, |bytes| match old_format {
        true => ItInstrument::parse_old(bytes, name.clone()),
        false => ItInstrument::parse(bytes, name.clone()),
    })??;
    builder.add_instrument(parsed.to_model(sample_count))?;
    Ok(())
}

/// Unpack one pattern and add it to `builder`.
fn load_pattern<R: ModuleReader + ?Sized>(source: &Source<'_, R>, offset: usize, channels: u8, builder: &mut ModuleBuilder) -> Result<(), Error> {
    let Some((body_offset, body_length, rows)) = pattern_extent(source, offset) else {
        // Impulse Tracker writes an offset of zero for a pattern with nothing in it, and
        // this loader gives a pattern it cannot read the same shape rather than failing.
        let empty = pattern::unpack(&[], DEFAULT_ROWS, channels);
        builder.add_pattern(&empty, DEFAULT_ROWS, channels)?;
        return Ok(());
    };
    let cells = source.with_slice(body_offset, body_length, |body| pattern::unpack(body, rows, channels))?;
    builder.add_pattern(&cells, rows, channels)?;
    Ok(())
}

/// A fixed-width text field, decoded as the code page Impulse Tracker displayed it in.
fn text(bytes: &[u8]) -> String { starplayer_model::decode_cp437(bytes) }

/// Bounds-checked reads over a [`ModuleReader`], with the borrowing fast path taken
/// wherever the source offers one.
struct Source<'reader, R: ModuleReader + ?Sized> {
    reader: &'reader R,
}

impl<R: ModuleReader + ?Sized> Source<'_, R> {
    fn len(&self) -> usize { self.reader.len() }

    /// `N` bytes at `offset`, on the stack.
    fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], Error> {
        let mut buffer = [0u8; N];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(buffer)
    }

    /// Hand `length` bytes at `offset` to `consume`, borrowing them when the source can and
    /// buffering them once when it cannot.
    fn with_slice<T>(&self, offset: usize, length: usize, consume: impl FnOnce(&[u8]) -> T) -> Result<T, Error> {
        if let Some(borrowed) = self.reader.slice_at(offset, length) {
            return Ok(consume(borrowed));
        }
        let mut buffer = vec![0u8; length];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(consume(&buffer))
    }

    /// `count` little-endian `u32` file offsets at `offset`.
    fn offsets(&self, offset: usize, count: u16) -> Result<Vec<u32>, Error> {
        self.with_slice(offset, 4 * count as usize, |bytes| {
            bytes.chunks_exact(4)
                .map(|quad| match quad {
                    [a, b, c, d] => u32::from_le_bytes([*a, *b, *c, *d]),
                    _ => 0,
                })
                .collect()
        })
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use starplayer_core::quirks::FormatDialect;
    use starplayer_model::{OrderEntry, PatternId};

    use crate::pattern::{ItCell, PatternView};

    /// A minimal but complete IT: one order, one sample, one pattern.
    struct Builder {
        bytes: Vec<u8>,
    }

    impl Builder {
        fn new(orders: &[u8], instruments: usize, samples: usize, patterns: usize) -> Builder {
            let mut bytes = vec![0u8; header::HEADER_LENGTH];
            bytes[..4].copy_from_slice(&header::MAGIC);
            bytes[0x20..0x22].copy_from_slice(&(orders.len() as u16).to_le_bytes());
            bytes[0x22..0x24].copy_from_slice(&(instruments as u16).to_le_bytes());
            bytes[0x24..0x26].copy_from_slice(&(samples as u16).to_le_bytes());
            bytes[0x26..0x28].copy_from_slice(&(patterns as u16).to_le_bytes());
            bytes[0x28..0x2A].copy_from_slice(&0x0214u16.to_le_bytes());
            bytes[0x2A..0x2C].copy_from_slice(&0x0214u16.to_le_bytes());
            bytes[0x2C..0x2E].copy_from_slice(&header::FLAG_STEREO.to_le_bytes());
            bytes[0x30] = 128;
            bytes[0x31] = 48;
            bytes[0x32] = 6;
            bytes[0x33] = 125;
            bytes[0x34] = 128;
            for channel in 0..MAX_CHANNELS {
                bytes[0x40 + channel] = 32;
                bytes[0x80 + channel] = 64;
            }
            bytes.extend_from_slice(orders);
            // Room for the three offset tables, filled in later.
            bytes.resize(bytes.len() + 4 * (instruments + samples + patterns), 0);
            Builder { bytes }
        }

        fn table_offset(&self, kind: usize) -> usize {
            let orders = u16::from_le_bytes([self.bytes[0x20], self.bytes[0x21]]) as usize;
            let instruments = u16::from_le_bytes([self.bytes[0x22], self.bytes[0x23]]) as usize;
            let samples = u16::from_le_bytes([self.bytes[0x24], self.bytes[0x25]]) as usize;
            let base = header::HEADER_LENGTH + orders;
            match kind {
                0 => base,
                1 => base + 4 * instruments,
                _ => base + 4 * (instruments + samples),
            }
        }

        fn place(&mut self, kind: usize, index: usize, blob: &[u8]) -> u32 {
            let offset = self.bytes.len() as u32;
            self.bytes.extend_from_slice(blob);
            let slot = self.table_offset(kind) + 4 * index;
            self.bytes[slot..slot + 4].copy_from_slice(&offset.to_le_bytes());
            offset
        }

        fn append(&mut self, blob: &[u8]) -> u32 {
            let offset = self.bytes.len() as u32;
            self.bytes.extend_from_slice(blob);
            offset
        }

        fn point_at(&mut self, kind: usize, index: usize, offset: u32) {
            let slot = self.table_offset(kind) + 4 * index;
            self.bytes[slot..slot + 4].copy_from_slice(&offset.to_le_bytes());
        }
    }

    /// An 80-byte sample header for `frames` unsigned 8-bit frames, with the loop flags
    /// and convert byte the caller wants.
    fn sample_header(name: &str, frames: u32, flags: u8, convert: u8, data_offset: u32) -> [u8; sample::HEADER_LENGTH] {
        let mut bytes = [0u8; sample::HEADER_LENGTH];
        bytes[..4].copy_from_slice(&sample::MAGIC);
        bytes[0x11] = 64;
        bytes[0x12] = sample::FLAG_HAS_DATA | flags;
        bytes[0x13] = 48;
        for (index, byte) in name.bytes().take(25).enumerate() {
            bytes[0x14 + index] = byte;
        }
        bytes[0x2E] = convert;
        bytes[0x30..0x34].copy_from_slice(&frames.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&0u32.to_le_bytes());
        bytes[0x38..0x3C].copy_from_slice(&frames.to_le_bytes());
        bytes[0x3C..0x40].copy_from_slice(&22050u32.to_le_bytes());
        bytes[0x48..0x4C].copy_from_slice(&data_offset.to_le_bytes());
        bytes
    }

    /// A pattern: the eight-byte header, then the packed stream.
    fn packed_pattern(rows: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = (body.len() as u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(&rows.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(body);
        bytes
    }

    /// The smallest file this loader accepts, with one four-channel pattern and one
    /// sample.
    fn minimal_module() -> Vec<u8> {
        let mut builder = Builder::new(&[0, 255], 0, 1, 1);
        let data = builder.append(&[0x00, 0x40, 0xC0, 0xFF]);
        builder.place(1, 0, &sample_header("hit", 4, 0, 0, data));
        let body = [0x81, pattern::MASK_NOTE | pattern::MASK_INSTRUMENT, 60, 1, 0x84, pattern::MASK_VOLUME, 40, 0];
        builder.place(2, 0, &packed_pattern(64, &body));
        builder.bytes
    }

    #[test]
    fn probe_wants_impm_at_offset_zero() {
        let bytes = minimal_module();
        assert!(probe(&bytes));
        assert!(probe_reader(&bytes[..]));
        assert!(!probe(&[0u8; 4]));
        assert!(!probe(&[]));
    }

    #[test]
    fn a_minimal_module_loads_with_every_field_the_engine_reads() {
        let module = load(&minimal_module()).expect("a valid module");

        assert_eq!(module.header().format, ModuleFormat::It);
        assert_eq!(module.header().channel_count, 4, "the widest pattern event names channel 3");
        assert_eq!(module.header().initial_speed, 6);
        assert_eq!(module.header().initial_tempo, 125);
        assert_eq!(module.header().dialect, FormatDialect::ImpulseTracker);
        assert!(module.header().flags.stereo && !module.header().flags.linear_slides);
        assert_eq!(module.samples().len(), 1);
        assert_eq!(module.instruments().len(), 1, "sample mode synthesises one instrument per sample");
        assert_eq!(module.patterns().len(), 1);
        assert_eq!(module.order_entry(0), Some(OrderEntry::Pattern(PatternId(0))));
        assert_eq!(module.order_entry(1), Some(OrderEntry::End));

        let sample = module.sample(SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.name(), "hit");
        assert_eq!(sample.reference_rate_hz(), 22050);
        assert_eq!(sample.length_frames(), 4);
        assert_eq!(sample.loop_mode(), LoopMode::None);
        assert_eq!(module.sample_pcm(SampleId(0)).map(|pcm| pcm[0]), Some(i16::MIN), "unsigned 8-bit 0x00 is full negative");

        let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");
        assert_eq!(view.rows(), 64);
        assert_eq!(view.channels(), 4);
        assert_eq!(view.cell(0, 0), Some(ItCell { note: 60, instrument: 1, ..ItCell::EMPTY }));
        assert_eq!(view.cell(0, 3), Some(ItCell { volume: 40, ..ItCell::EMPTY }));
    }

    #[test]
    fn a_file_shorter_than_the_fixed_header_or_without_the_signature_is_rejected() {
        assert_eq!(load(&[]), Err(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH }));
        assert_eq!(load(&[0u8; header::HEADER_LENGTH - 1]), Err(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH }));
        assert_eq!(load(&[0u8; header::HEADER_LENGTH]), Err(Error::BadMagic));
    }

    #[test]
    fn a_count_the_file_cannot_support_is_truncated_rather_than_a_panic() {
        let mut bytes = vec![0u8; header::HEADER_LENGTH];
        bytes[..4].copy_from_slice(&header::MAGIC);
        bytes[0x24..0x26].copy_from_slice(&1000u16.to_le_bytes());
        assert!(matches!(load(&bytes), Err(Error::Truncated { .. })));
    }

    #[test]
    fn a_module_no_pattern_writes_to_still_has_a_channel() {
        let mut builder = Builder::new(&[255], 0, 0, 1);
        builder.place(2, 0, &packed_pattern(64, &[0, 0, 0]));
        let module = load(&builder.bytes).expect("a valid module");

        assert_eq!(module.header().channel_count, 1);
        assert_eq!(module.patterns().len(), 1);
    }

    #[test]
    fn a_pattern_offset_of_zero_is_an_empty_sixty_four_row_pattern() {
        let mut builder = Builder::new(&[0, 255], 0, 0, 2);
        builder.place(2, 1, &packed_pattern(32, &[0x81, pattern::MASK_NOTE, 60, 0]));
        let module = load(&builder.bytes).expect("a valid module");

        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(64));
        assert_eq!(module.pattern(PatternId(1)).map(|index| index.rows()), Some(32));
        let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");
        assert_eq!(view.cell(0, 0), Some(ItCell::EMPTY));
    }

    #[test]
    fn a_pattern_row_count_outside_the_formats_range_becomes_an_empty_pattern() {
        for rows in [0u16, MAX_ROWS + 1, 1024] {
            let mut builder = Builder::new(&[0, 255], 0, 0, 1);
            builder.place(2, 0, &packed_pattern(rows, &[0x81, pattern::MASK_NOTE, 60, 0]));
            let module = load(&builder.bytes).expect("a valid module");
            assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(DEFAULT_ROWS), "rows = {rows}");
        }

        // Research point 5: 256 rows is accepted, which is what ModPlug Tracker wrote.
        let mut builder = Builder::new(&[0, 255], 0, 0, 1);
        builder.place(2, 0, &packed_pattern(MAX_ROWS, &[0x81, pattern::MASK_NOTE, 60, 0]));
        let module = load(&builder.bytes).expect("a valid module");
        assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(MAX_ROWS));
    }

    #[test]
    fn an_order_naming_a_pattern_that_does_not_exist_becomes_a_marker() {
        let mut builder = Builder::new(&[0, 9, 254, 255], 0, 0, 1);
        builder.place(2, 0, &packed_pattern(64, &[0]));
        let module = load(&builder.bytes).expect("a valid module");

        assert_eq!(module.order_entry(0), Some(OrderEntry::Pattern(PatternId(0))));
        assert_eq!(module.order_entry(1), Some(OrderEntry::Marker));
        assert_eq!(module.order_entry(2), Some(OrderEntry::Marker));
        assert_eq!(module.order_entry(3), Some(OrderEntry::End));
    }

    #[test]
    fn a_sample_running_past_the_end_of_the_file_is_clamped_to_what_is_there() {
        let mut builder = Builder::new(&[255], 0, 1, 0);
        let data = builder.append(&[0x80; 8]);
        builder.place(1, 0, &sample_header("short", 10_000, 0, 0, data));
        // The sample header itself was appended after the data, so only eight bytes of it
        // are actually sample data.
        let module = load(&builder.bytes).expect("a valid module");

        let sample = module.sample(SampleId(0)).expect("sample 0 exists");
        assert!(sample.length_frames() < 10_000, "{} frames", sample.length_frames());
    }

    #[test]
    fn a_sample_offset_of_zero_or_past_the_end_is_an_empty_slot_that_keeps_its_number() {
        let mut builder = Builder::new(&[255], 0, 3, 0);
        let data = builder.append(&[0x80; 4]);
        builder.place(1, 1, &sample_header("real", 4, 0, 0, data));
        builder.point_at(1, 2, 0xFFFF_0000);
        let module = load(&builder.bytes).expect("a valid module");

        assert_eq!(module.samples().len(), 3);
        assert_eq!(module.sample(SampleId(0)).map(|sample| sample.length_frames()), Some(0));
        assert_eq!(module.sample(SampleId(1)).map(|sample| sample.name()), Some("real"));
        assert_eq!(module.sample(SampleId(2)).map(|sample| sample.length_frames()), Some(0));
    }

    #[test]
    fn a_loop_end_past_the_sample_is_clamped_and_an_inverted_loop_is_dropped() {
        let mut builder = Builder::new(&[255], 0, 2, 0);
        let data = builder.append(&[0x80; 8]);
        let mut looping = sample_header("loop", 8, sample::FLAG_LOOP, 0, data);
        looping[0x34..0x38].copy_from_slice(&2u32.to_le_bytes());
        looping[0x38..0x3C].copy_from_slice(&9_999u32.to_le_bytes());
        builder.place(1, 0, &looping);
        let mut inverted = sample_header("inverted", 8, sample::FLAG_LOOP, 0, data);
        inverted[0x34..0x38].copy_from_slice(&6u32.to_le_bytes());
        inverted[0x38..0x3C].copy_from_slice(&2u32.to_le_bytes());
        builder.place(1, 1, &inverted);

        let module = load(&builder.bytes).expect("a valid module");
        let looping = module.sample(SampleId(0)).expect("sample 0 exists");
        assert_eq!(looping.loop_mode(), LoopMode::Forward);
        assert_eq!(looping.loop_start(), 2);
        assert_eq!(looping.loop_end(), 8, "clamped to the sample's own length");
        assert_eq!(module.sample(SampleId(1)).map(|sample| sample.loop_mode()), Some(LoopMode::None));
    }

    #[test]
    fn a_sustain_ping_pong_loop_reaches_the_model_intact() {
        let mut builder = Builder::new(&[255], 0, 1, 0);
        let data = builder.append(&[0x80; 16]);
        let flags = sample::FLAG_LOOP | sample::FLAG_SUSTAIN_LOOP | sample::FLAG_PING_PONG_SUSTAIN;
        let mut header_bytes = sample_header("sustained", 16, flags, 0, data);
        header_bytes[0x34..0x38].copy_from_slice(&0u32.to_le_bytes());
        header_bytes[0x38..0x3C].copy_from_slice(&8u32.to_le_bytes());
        header_bytes[0x40..0x44].copy_from_slice(&8u32.to_le_bytes());
        header_bytes[0x44..0x48].copy_from_slice(&16u32.to_le_bytes());
        builder.place(1, 0, &header_bytes);

        let module = load(&builder.bytes).expect("a valid module");
        let sample = module.sample(SampleId(0)).expect("sample 0 exists");
        assert_eq!(sample.loop_mode(), LoopMode::Forward);
        assert_eq!(sample.sustain_loop(), Some(SustainLoop { mode: LoopMode::PingPong, start: 8, end: 16 }));
        assert_eq!(sample.length_frames(), 16, "a sustain loop keeps the whole sample");
    }

    #[test]
    fn a_stereo_sample_is_downmixed() {
        let mut builder = Builder::new(&[255], 0, 1, 0);
        // Four left frames at full negative, four right frames at full positive.
        let data = builder.append(&[0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]);
        builder.place(1, 0, &sample_header("wide", 4, sample::FLAG_STEREO, 0, data));

        let module = load(&builder.bytes).expect("a valid module");
        let pcm = module.sample_pcm(SampleId(0)).expect("sample 0 exists");
        assert_eq!(module.sample(SampleId(0)).map(|sample| sample.length_frames()), Some(4));
        // D60: `(-32768 + 32512) / 2`, the average of the two channels' widened frames.
        assert_eq!(pcm[0], -128);
    }

    #[test]
    fn a_compressed_sample_decodes_through_the_it_two_fourteen_decompressor() {
        let mut builder = Builder::new(&[255], 0, 1, 0);
        // Three nine-bit mode-C residuals of +1, packed least significant bit first.
        let mut bits: Vec<u8> = Vec::new();
        let mut accumulator: u32 = 0;
        let mut used = 0u32;
        for _ in 0..3 {
            accumulator |= 1 << used;
            used += 9;
            while used >= 8 {
                bits.push((accumulator & 0xFF) as u8);
                accumulator >>= 8;
                used -= 8;
            }
        }
        if used > 0 {
            bits.push(accumulator as u8);
        }
        let mut blob = (bits.len() as u16).to_le_bytes().to_vec();
        blob.extend_from_slice(&bits);
        let data = builder.append(&blob);
        builder.place(1, 0, &sample_header("packed", 3, sample::FLAG_COMPRESSED, 0, data));

        let module = load(&builder.bytes).expect("a valid module");
        let pcm = module.sample_pcm(SampleId(0)).expect("sample 0 exists");
        assert_eq!(&pcm[..3], &[256, 512, 768]);
    }

    #[test]
    fn an_instrument_mode_module_reads_its_instruments_and_keyboard_tables() {
        let mut builder = Builder::new(&[255], 1, 1, 0);
        builder.bytes[0x2C..0x2E].copy_from_slice(&(header::FLAG_STEREO | header::FLAG_INSTRUMENT_MODE).to_le_bytes());
        let data = builder.append(&[0x80; 4]);
        builder.place(1, 0, &sample_header("smp", 4, 0, 0, data));

        let mut instrument_bytes = [0u8; instrument::HEADER_LENGTH];
        instrument_bytes[..4].copy_from_slice(&instrument::MAGIC);
        instrument_bytes[0x11] = 3; // NNA: note fade
        instrument_bytes[0x14..0x16].copy_from_slice(&256u16.to_le_bytes());
        instrument_bytes[0x18] = 128;
        for (index, byte) in b"lead".iter().enumerate() {
            instrument_bytes[0x20 + index] = *byte;
        }
        for note in 0..120 {
            instrument_bytes[instrument::KEYBOARD_OFFSET + note * 2] = note as u8;
            instrument_bytes[instrument::KEYBOARD_OFFSET + note * 2 + 1] = 1;
        }
        instrument_bytes[instrument::VOLUME_ENVELOPE_OFFSET] = instrument::ENVELOPE_ENABLED;
        instrument_bytes[instrument::VOLUME_ENVELOPE_OFFSET + 1] = 2;
        instrument_bytes[instrument::VOLUME_ENVELOPE_OFFSET + 6] = 64;
        instrument_bytes[instrument::VOLUME_ENVELOPE_OFFSET + 9] = 0;
        instrument_bytes[instrument::VOLUME_ENVELOPE_OFFSET + 10..instrument::VOLUME_ENVELOPE_OFFSET + 12].copy_from_slice(&20u16.to_le_bytes());
        builder.place(0, 0, &instrument_bytes);

        let module = load(&builder.bytes).expect("a valid module");
        let instrument = module.instrument(starplayer_core::InstrumentId(0)).expect("instrument 0 exists");

        assert_eq!(&*instrument.name, "lead");
        assert_eq!(instrument.sample, None);
        assert_eq!(instrument.note_sample_map[60], 1);
        assert_eq!(instrument.fadeout, 256);
        assert_eq!(instrument.volume_envelope.as_ref().map(|envelope| envelope.points.len()), Some(2));
        assert!(ItFormatExtra::from_header(module.header()).is_instrument_mode());
    }

    #[test]
    fn an_old_format_instrument_module_loads_through_the_pre_two_hundred_layout() {
        let mut builder = Builder::new(&[255], 1, 1, 0);
        builder.bytes[0x2A..0x2C].copy_from_slice(&0x0106u16.to_le_bytes());
        builder.bytes[0x28..0x2A].copy_from_slice(&0x0106u16.to_le_bytes());
        builder.bytes[0x2C..0x2E].copy_from_slice(&(header::FLAG_STEREO | header::FLAG_INSTRUMENT_MODE).to_le_bytes());
        let data = builder.append(&[0x80; 4]);
        builder.place(1, 0, &sample_header("smp", 4, 0, 0, data));

        let mut instrument_bytes = [0u8; instrument::HEADER_LENGTH];
        instrument_bytes[..4].copy_from_slice(&instrument::MAGIC);
        instrument_bytes[0x11] = instrument::ENVELOPE_ENABLED;
        instrument_bytes[0x18..0x1A].copy_from_slice(&32u16.to_le_bytes());
        instrument_bytes[0x1A] = 3; // NNA: note fade
        for note in 0..120 {
            instrument_bytes[instrument::KEYBOARD_OFFSET + note * 2] = note as u8;
            instrument_bytes[instrument::KEYBOARD_OFFSET + note * 2 + 1] = 1;
        }
        instrument_bytes[instrument::OLD_ENVELOPE_NODES_OFFSET] = 0;
        instrument_bytes[instrument::OLD_ENVELOPE_NODES_OFFSET + 1] = 64;
        instrument_bytes[instrument::OLD_ENVELOPE_NODES_OFFSET + 2] = 0xFF;
        builder.place(0, 0, &instrument_bytes);

        let module = load(&builder.bytes).expect("a valid module");
        let instrument = module.instrument(starplayer_core::InstrumentId(0)).expect("instrument 0 exists");

        assert!(ItFormatExtra::from_header(module.header()).old_instruments);
        assert_eq!(instrument.fadeout, 64, "the old scale doubles into the new one");
        assert_eq!(instrument.volume_envelope.as_ref().map(|envelope| envelope.points.len()), Some(1));
        assert_eq!(instrument.note_sample_map[60], 1);
    }

    #[test]
    fn the_embedded_midi_configuration_reaches_format_data() {
        let mut builder = Builder::new(&[255], 0, 0, 0);
        builder.bytes[0x2E..0x30].copy_from_slice(&header::SPECIAL_MIDI_CONFIGURATION.to_le_bytes());
        let mut configuration = vec![0u8; header::MIDI_CONFIGURATION_BYTES];
        configuration[0] = b'F';
        configuration[header::MIDI_GLOBAL_MACROS * header::MIDI_MACRO_BYTES] = b'S';
        configuration[(header::MIDI_GLOBAL_MACROS + header::MIDI_PARAMETERED_MACROS) * header::MIDI_MACRO_BYTES] = b'Z';
        builder.bytes.extend_from_slice(&configuration);

        let module = load(&builder.bytes).expect("a valid module");
        let data = header::ItFormatData::from_header(module.header()).expect("the block is there");

        assert!(ItFormatExtra::from_header(module.header()).has_midi_configuration);
        assert_eq!(data.global_macro(0).map(|bytes| bytes[0]), Some(b'F'));
        assert_eq!(data.parametered_macro(0).map(|bytes| bytes[0]), Some(b'S'));
        assert_eq!(data.fixed_macro(0).map(|bytes| bytes[0]), Some(b'Z'));
    }

    #[test]
    fn a_missing_midi_block_leaves_format_data_without_one() {
        let mut builder = Builder::new(&[255], 0, 0, 0);
        builder.bytes[0x2E..0x30].copy_from_slice(&header::SPECIAL_MIDI_CONFIGURATION.to_le_bytes());
        let module = load(&builder.bytes).expect("a valid module");

        assert!(!ItFormatExtra::from_header(module.header()).has_midi_configuration);
        assert_eq!(header::ItFormatData::from_header(module.header()).and_then(|data| data.midi_configuration()), None);
    }

    #[test]
    fn the_channel_pan_and_volume_tables_reach_the_header() {
        let mut builder = Builder::new(&[0, 255], 0, 0, 1);
        builder.bytes[0x40] = 0;
        builder.bytes[0x41] = header::PAN_SURROUND;
        builder.bytes[0x42] = 64;
        builder.bytes[0x43] = 32 | header::PAN_DISABLED;
        builder.bytes[0x80] = 64;
        builder.bytes[0x81] = 32;
        builder.place(2, 0, &packed_pattern(64, &[0x84, pattern::MASK_NOTE, 60, 0]));

        let module = load(&builder.bytes).expect("a valid module");
        assert_eq!(module.header().channel_count, 4);
        assert_eq!(module.header().channel_pan(0), Some(header::pan_to_bipolar(0)));
        assert_eq!(module.header().channel_pan(2), Some(I1F15::MAX));
        assert_eq!(module.header().channel_volume(1), Some(unit_from_ratio(32, 64)));

        let data = header::ItFormatData::from_header(module.header()).expect("the block is there");
        assert!(data.is_surround(1));
        assert!(data.is_disabled(3), "a disabled channel still occupies a column");
    }

    #[test]
    fn a_mono_module_centres_every_channel_with_an_empty_pan_table() {
        let mut builder = Builder::new(&[0, 255], 0, 0, 1);
        builder.bytes[0x2C..0x2E].copy_from_slice(&0u16.to_le_bytes());
        builder.bytes[0x40] = 0;
        builder.place(2, 0, &packed_pattern(64, &[0x81, pattern::MASK_NOTE, 60, 0]));

        let module = load(&builder.bytes).expect("a valid module");
        assert!(module.header().default_pan.is_empty());
        assert_eq!(module.header().channel_pan(0), Some(I1F15::ZERO));
    }

    #[test]
    fn a_zero_speed_and_a_slow_tempo_are_clamped() {
        let mut builder = Builder::new(&[255], 0, 0, 0);
        builder.bytes[0x32] = 0;
        builder.bytes[0x33] = 4;
        builder.bytes[0x30] = 255;
        builder.bytes[0x31] = 255;

        let module = load(&builder.bytes).expect("a valid module");
        assert_eq!(module.header().initial_speed, FALLBACK_SPEED);
        assert_eq!(module.header().initial_tempo, MINIMUM_TEMPO);
        assert_eq!(module.header().global_volume, U0F16::MAX, "GV is clamped to 128");
        assert_eq!(module.header().master_volume, U0F16::MAX);
    }

    // ── the production allocation caps ────────────────────────────────────────────

    #[test]
    fn a_pattern_count_the_file_cannot_justify_is_refused_rather_than_allocated() {
        // 20,000 patterns of 64 rows over 64 channels is 409 MB of decoded patterns out of
        // an 80 KB file, whose budget is the 32 MiB floor. Every offset is zero, which is
        // Impulse Tracker's own spelling of an empty pattern and costs nothing more.
        let mut builder = Builder::new(&[255], 0, 0, 20_000);
        // One real pattern gives the song its 64 columns.
        let wide = [0xC0u8, pattern::MASK_NOTE, 60, 0];
        builder.place(2, 0, &packed_pattern(64, &wide));

        assert!(builder.bytes.len() < 100_000, "the whole attack fits in 100 KB: {} bytes", builder.bytes.len());
        assert_eq!(load(&builder.bytes), Err(Error::TooLarge("IT pattern data")));
    }

    #[test]
    fn a_pattern_count_a_real_module_would_use_is_still_accepted() {
        // Impulse Tracker's own limit is 200 patterns; 240 is what the trackers that
        // extended the format allow. Both stay inside the floor at the widest stride.
        let mut builder = Builder::new(&[255], 0, 0, 240);
        let wide = [0xC0u8, pattern::MASK_NOTE, 60, 0];
        builder.place(2, 0, &packed_pattern(200, &wide));

        let module = load(&builder.bytes).expect("240 patterns of 64 channels is a plausible module");
        assert_eq!(module.patterns().len(), 240);
        assert_eq!(module.header().channel_count, 64);
    }

    #[test]
    fn the_budgets_are_floors_below_which_file_size_does_not_matter() {
        assert_eq!(decoded_pattern_budget(0), MINIMUM_PATTERN_BUDGET_BYTES);
        assert_eq!(decoded_pattern_budget(1_024), MINIMUM_PATTERN_BUDGET_BYTES);
        assert_eq!(decoded_pattern_budget(1_000_000), 64_000_000);
        assert_eq!(decoded_pattern_budget(usize::MAX), usize::MAX);
        assert_eq!(decoded_pcm_budget(0), MINIMUM_PCM_BUDGET_FRAMES);
        assert_eq!(decoded_pcm_budget(2_000_000), 32_000_000);
        assert_eq!(decoded_pcm_budget(usize::MAX), usize::MAX);
    }

    #[test]
    fn many_samples_aimed_at_one_compressed_block_share_one_budget() {
        // Sixty-four sample slots, every one claiming four megaframes of compressed data
        // and every one pointing at the same handful of bytes. Without a shared budget the
        // claims would be honoured one at a time.
        let mut builder = Builder::new(&[255], 0, 64, 0);
        // One block of 62 zero bytes: mode C reads nine zero bits per residual, so the
        // block decodes 55 frames and then runs out.
        let mut blob = 62u16.to_le_bytes().to_vec();
        blob.extend_from_slice(&[0u8; 62]);
        let data = builder.append(&blob);
        for slot in 0..64 {
            builder.place(1, slot, &sample_header("packed", 4_000_000, sample::FLAG_COMPRESSED, 0, data));
        }

        let module = load(&builder.bytes).expect("a valid module");
        let frames: usize = module.samples().iter().map(|sample| sample.length_frames() as usize).sum();
        assert!(frames <= decoded_pcm_budget(builder.bytes.len()), "{frames} frames decoded");
        assert!(module.pcm().len() < 4_000_000, "{} frames of PCM", module.pcm().len());
    }

    #[test]
    fn an_uncompressed_sample_claiming_four_gigaframes_costs_only_what_the_file_holds() {
        let mut builder = Builder::new(&[255], 0, 1, 0);
        let data = builder.append(&[0x80; 32]);
        builder.place(1, 0, &sample_header("huge", u32::MAX, 0, 0, data));

        let module = load(&builder.bytes).expect("a valid module");
        assert!(module.sample(SampleId(0)).is_some_and(|sample| sample.length_frames() < 200));
    }

    #[test]
    fn every_prefix_of_a_minimal_module_returns_rather_than_panicking() {
        let bytes = minimal_module();
        for length in 0..bytes.len() {
            let _ = load(&bytes[..length]);
        }
    }
}

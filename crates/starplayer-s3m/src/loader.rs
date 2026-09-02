//! `load` — bytes in, [`Module`] out.
//!
//! # The layout this walks
//!
//! ```text
//! 0x00  title, 28 bytes
//! 0x20  Ordnum   0x22 Insnum   0x24 Patnum   0x26 generalflags
//! 0x28  Cwt/v    0x2A ffi      0x2C "SCRM"
//! 0x30  globalvol  0x31 initialspd  0x32 initialBPM  0x33 mastervol
//! 0x34  ultraclick 0x35 defaultpan  0x36 8 reserved bytes  0x3E special
//! 0x40  32 channel-settings bytes
//! 0x60  Ordnum order bytes
//!       Insnum instrument parapointers (u16)
//!       Patnum pattern parapointers (u16)
//!       32 default-pan bytes, iff byte 0x35 == 252
//!       … parapointed sample headers, sample data and packed patterns, in any order
//! ```
//!
//! Every parapointer is a paragraph count: multiply by 16 for a byte offset.
//!
//! # Clamp or reject
//!
//! Task B2 research point 1: real files break the specification in known ways, and the
//! rule applied here is *clamp wherever a tracker would have played the file, reject only
//! where the file contradicts itself*. Every case, and nothing else fails:
//!
//! | Case | Behaviour |
//! |---|---|
//! | No `SCRM` at `0x2C`, or fewer than `0x60` bytes | [`Error::BadMagic`] / [`Error::Truncated`] |
//! | Order list, parapointer arrays or an announced pan block past EOF | [`Error::Truncated`] — an `Insnum` or `Patnum` the file cannot support is exactly this |
//! | No enabled channel in the 32-byte settings array | [`Error::Invalid`] |
//! | More decoded pattern data than the file's own size can justify | [`Error::TooLarge`] — see [`MINIMUM_PATTERN_BUDGET_BYTES`] |
//! | `initialspd` of 0 | clamped to 6; a speed of zero advances no rows |
//! | `initialBPM` below 32 | clamped to 32, Scream Tracker 3's own floor |
//! | `globalvol` above 64 | clamped to 64 |
//! | Order naming a pattern that does not exist | becomes [`ORDER_MARKER`] — `__UpdateTracker` skips exactly these (`S3MLIB.ASM:2606`) |
//! | Instrument parapointer of 0, or a header past EOF | empty instrument slot, numbering preserved |
//! | Instrument `type` not 1 (empty slot, or Adlib 2..=7) | instrument with no sample |
//! | Sample `pack` field not 0 | [`Error::Unsupported`] — Scream Tracker 3 never wrote one |
//! | Sample data parapointer past EOF | empty sample; the header's volume and C2SPD are still kept |
//! | Sample data running past EOF | clamped to what is there |
//! | `C2Spd` of 0 | replaced with 8363, the format default; the builder rejects a zero rate |
//! | Sample `vol` above 64 | coerced to 0, matching ST3 instrument-change semantics |
//! | `loopend` past the sample's length | clamped to the length |
//! | `loopstart >= loopend` after clamping | the sample does not loop |
//! | Pattern parapointer of 0 | an empty 64-row pattern, which is what Scream Tracker 3 means by it |
//! | Pattern parapointer past EOF, or a packed length that overruns the file | [`Error::Truncated`] |
//! | Packed length below 2 (it counts itself) | an empty pattern |
//! | A packed stream that ends early, overruns 64 rows, or names a channel that does not exist | see [`crate::pattern::unpack`] |

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, I1F15, U0F16};
use starplayer_model::{
    InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader, ModuleReader,
    ORDER_END, ORDER_MARKER, SampleSpec,
};

use crate::header::{self, MAGIC, MAGIC_OFFSET, MAX_CHANNELS, S3mFormatExtra, S3mHeader};
use crate::pattern;
use crate::sample::{self, S3mSampleHeader};

/// Lowest tempo Scream Tracker 3 allows; a slower one would make a tick longer than the
/// sequencer's arithmetic expects.
const MINIMUM_TEMPO: u16 = 32;

/// Speed a file that asks for zero ticks per row is given instead — ProTracker's and
/// Scream Tracker 3's own default.
const FALLBACK_SPEED: u8 = 6;

/// Reference rate a sample whose `C2Spd` is zero is given: the format default, and what
/// `ClearChannels` initialises `_C4SPD` to.
const FALLBACK_C2SPD: u32 = starplayer_model::DEFAULT_REFERENCE_RATE_HZ;

/// Decoded pattern bytes a file is allowed regardless of how small it is.
///
/// M2-C7 research point 3: *should a loader allocation be capped in production, not only
/// under the fuzzer?* Here, yes, and this is the one place it matters. `Patnum` is a
/// `u16` and a pattern costs **two bytes** of parapointer, so a 132 KB file may declare
/// 65,535 patterns; each one unpacks to a fixed `64 × channels × 5` bytes, which at 32
/// channels is 10 KB — 670 MB of decoded patterns out of a file that fits in a mail
/// attachment. A parapointer of zero costs no further file bytes at all and is not even
/// malformed: Scream Tracker 3 spells an empty pattern exactly that way.
///
/// Surviving such a file is not enough. A 2 GB length field in a 4 KB file is not a
/// module, and a loader that allocates for it has already lost: on a phone or an embedded
/// target the allocation succeeds and something else dies. So it is refused on principle,
/// with a budget no real file approaches — the floor alone is 819 patterns of 32 channels,
/// against Scream Tracker 3's own limit of 100.
const MINIMUM_PATTERN_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// Decoded pattern bytes a file earns per byte of its own size, once it is big enough for
/// that to beat [`MINIMUM_PATTERN_BUDGET_BYTES`]. Packed S3M patterns are roughly a fifth
/// of their decoded size, so 64× leaves two orders of magnitude of headroom.
const PATTERN_BUDGET_PER_FILE_BYTE: usize = 64;

/// The decoded-pattern budget for a file of `file_bytes`.
fn decoded_pattern_budget(file_bytes: usize) -> usize {
    file_bytes.saturating_mul(PATTERN_BUDGET_PER_FILE_BYTE).max(MINIMUM_PATTERN_BUDGET_BYTES)
}

/// Whether `bytes` looks like an S3M: `SCRM` at `0x2C`.
///
/// Cheap and total — it reads four bytes and never allocates — so a host can use it to
/// pick a loader before committing to one.
pub fn probe(bytes: &[u8]) -> bool { bytes.get(MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()) == Some(&MAGIC[..]) }

/// Whether `reader` looks like an S3M. The [`ModuleReader`] form of [`probe`].
pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool {
    let mut magic = [0u8; 4];
    reader.read_at(MAGIC_OFFSET, &mut magic).is_ok() && magic == MAGIC
}

/// Load an S3M from bytes already in memory.
pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_from(bytes) }

/// Load an S3M from any [`ModuleReader`].
///
/// The two are the same function: `[u8]` is a `ModuleReader` whose
/// [`slice_at`](ModuleReader::slice_at) borrows, so loading from a slice copies nothing
/// beyond what the module itself owns. A reader that cannot borrow gets one temporary
/// buffer per pattern and per sample instead.
///
/// # Errors
///
/// See the clamp-or-reject table in the [module documentation](self).
pub fn load_from<R: ModuleReader + ?Sized>(reader: &R) -> Result<Module, Error> {
    let source = Source { reader };

    let fixed_header: [u8; header::HEADER_LENGTH] = source.array(0)?;
    let file_header = S3mHeader::parse(&fixed_header)?;

    let channel_count = file_header.channel_count();
    if channel_count == 0 {
        return Err(Error::Invalid("no enabled channels in the channel-settings array"));
    }
    let addressed_channels = file_header.addressed_channels();

    let order_offset = header::HEADER_LENGTH;
    let instrument_pointer_offset = order_offset + file_header.order_count as usize;
    let pattern_pointer_offset = instrument_pointer_offset + 2 * file_header.instrument_count as usize;
    let pan_block_offset = file_header.tables_end();
    let tables_end = pan_block_offset + if file_header.has_default_pan_block() { MAX_CHANNELS } else { 0 };
    if tables_end > source.len() {
        // `Insnum` or `Patnum` bigger than the file can hold lands here, which is the
        // whole point of checking the tables as one region.
        return Err(Error::Truncated { offset: order_offset, needed: tables_end - order_offset });
    }

    let orders = source.with_slice(order_offset, file_header.order_count as usize, <[u8]>::to_vec)?;
    let instrument_pointers = source.parapointers(instrument_pointer_offset, file_header.instrument_count)?;
    let pattern_pointers = source.parapointers(pattern_pointer_offset, file_header.pattern_count)?;
    let pan_block: Option<[u8; MAX_CHANNELS]> = match file_header.has_default_pan_block() {
        true => Some(source.array(pan_block_offset)?),
        false => None,
    };

    // Every pattern unpacks to the same fixed stride whatever its parapointer says, so the
    // whole decoded cost is known before a byte of it is allocated. See
    // [`MINIMUM_PATTERN_BUDGET_BYTES`].
    let decoded_pattern_bytes = pattern_pointers.len()
        .saturating_mul(pattern::ROWS as usize)
        .saturating_mul(addressed_channels as usize)
        .saturating_mul(pattern::CELL_BYTES);
    if decoded_pattern_bytes > decoded_pattern_budget(source.len()) {
        return Err(Error::TooLarge("S3M pattern data"));
    }

    let mut builder = ModuleBuilder::new();
    for pointer in &instrument_pointers {
        load_instrument(&source, *pointer as usize * 16, &mut builder)?;
    }
    for pointer in &pattern_pointers {
        load_pattern(&source, *pointer as usize * 16, addressed_channels, &mut builder)?;
    }

    let pattern_count = pattern_pointers.len();
    let order_entries: Vec<u16> = orders.iter()
        .map(|order| match *order {
            255 => ORDER_END,
            254 => ORDER_MARKER,
            // `__UpdateTracker` skips an order naming a pattern the file does not have,
            // which is what the marker means to the sequencer.
            order if (order as usize) < pattern_count => order as u16,
            _ => ORDER_MARKER,
        })
        .collect();
    builder.set_orders(&order_entries);
    builder.set_header(song_header(&source, &file_header, channel_count, pan_block.as_ref())?);
    builder.build()
}

/// Build the format-neutral [`ModuleHeader`] out of the file header.
fn song_header<R: ModuleReader + ?Sized>(source: &Source<'_, R>, file_header: &S3mHeader, channel_count: u8, pan_block: Option<&[u8; MAX_CHANNELS]>) -> Result<ModuleHeader, Error> {
    let title = source.with_slice(0, 28, text)?;

    // A mono module with no pan block is centred on every channel, which the model spells
    // as an empty table rather than as a run of zeroes.
    let default_pan = match file_header.is_stereo() || pan_block.is_some() {
        false => Vec::<I1F15>::new().into_boxed_slice(),
        true => {
            let nibbles = header::default_pan_nibbles(&file_header.channel_settings, file_header.is_stereo(), pan_block);
            nibbles.iter()
                .take(channel_count as usize)
                .map(|nibble| header::pan_nibble_to_bipolar(*nibble))
                .collect::<Vec<I1F15>>()
                .into_boxed_slice()
        }
    };

    let extra = S3mFormatExtra {
        tracker_version: file_header.tracker_version,
        master_volume: file_header.master_volume,
        general_flags: file_header.general_flags as u8,
    };

    Ok(ModuleHeader {
        title: title.into_boxed_str(),
        format: ModuleFormat::S3m,
        channel_count,
        initial_speed: match file_header.initial_speed {
            0 => FALLBACK_SPEED,
            speed => speed,
        },
        initial_tempo: core::cmp::max(file_header.initial_tempo as u16, MINIMUM_TEMPO),
        global_volume: unit_from_ratio(file_header.global_volume as u32, 64),
        master_volume: unit_from_ratio(file_header.master_volume_level() as u32, 127),
        default_pan,
        flags: ModuleFlags {
            amiga_limits: file_header.amiga_limits(),
            linear_slides: false,
            fast_volume_slides: file_header.fast_volume_slides(),
            stereo: file_header.is_stereo(),
        },
        format_extra: extra.encode(),
    })
}

/// Read one instrument slot and add its sample and its [`InstrumentDef`] to `builder`.
///
/// One `InstrumentDef` is added per slot whatever the slot holds, so the file's one-based
/// instrument numbers stay `InstrumentId(number - 1)` for the effect processor. A slot
/// that holds no PCM gets an instrument with no sample rather than being skipped.
fn load_instrument<R: ModuleReader + ?Sized>(source: &Source<'_, R>, header_offset: usize, builder: &mut ModuleBuilder) -> Result<(), Error> {
    if header_offset == 0 || header_offset + sample::HEADER_LENGTH > source.len() {
        builder.add_instrument(InstrumentDef::default())?;
        return Ok(());
    }

    let bytes: [u8; sample::HEADER_LENGTH] = source.array(header_offset)?;
    let sample_header = S3mSampleHeader::parse(&bytes)?;
    let name = source.with_slice(header_offset + 0x30, 0x1C, text)?;

    if !sample_header.is_pcm() {
        builder.add_instrument(InstrumentDef { name: name.into_boxed_str(), ..InstrumentDef::default() })?;
        return Ok(());
    }
    if sample_header.packing != 0 {
        return Err(Error::Unsupported("packed S3M sample data"));
    }

    // Frames the file says are there, against frames that actually fit in it.
    let bytes_per_frame = sample_header.bytes_per_frame();
    let available_frames = match sample_header.data_offset == 0 || sample_header.data_offset >= source.len() {
        true => 0,
        false => (source.len() - sample_header.data_offset) / bytes_per_frame,
    };
    let frames = core::cmp::min(sample_header.length as usize, available_frames);

    let pcm = match frames {
        0 => Vec::new(),
        frames => source.with_slice(sample_header.data_offset, frames * bytes_per_frame, |raw| {
            sample::decode_frames(raw, frames, sample_header.flags)
        })?,
    };

    let loop_end = core::cmp::min(sample_header.loop_end as usize, pcm.len());
    let loops = sample_header.loops() && (sample_header.loop_start as usize) < loop_end;
    let specification = SampleSpec {
        name: name.clone(),
        loop_mode: match loops {
            true => LoopMode::Forward,
            false => LoopMode::None,
        },
        loop_start: match loops {
            true => sample_header.loop_start,
            false => 0,
        },
        loop_end: match loops {
            true => loop_end as u32,
            false => 0,
        },
        default_volume: unit_from_ratio(if sample_header.volume > 64 { 0 } else { sample_header.volume as u32 }, 64),
        reference_rate_hz: match sample_header.c2spd {
            0 => FALLBACK_C2SPD,
            rate => rate,
        },
    };

    let sample_id = builder.add_sample(&pcm, specification)?;
    builder.add_instrument(InstrumentDef::from_sample(&name, sample_id, U0F16::MAX))?;
    Ok(())
}

/// Unpack one pattern and add it to `builder`.
fn load_pattern<R: ModuleReader + ?Sized>(source: &Source<'_, R>, pattern_offset: usize, channels: u8, builder: &mut ModuleBuilder) -> Result<(), Error> {
    // Scream Tracker 3 writes a parapointer of zero for a pattern with nothing in it.
    if pattern_offset == 0 {
        let empty = pattern::unpack(&[], pattern::ROWS, channels);
        builder.add_pattern(&empty, pattern::ROWS, channels)?;
        return Ok(());
    }

    let length_bytes: [u8; 2] = source.array(pattern_offset)?;
    // The packed length counts its own two bytes — which is why walking `plen` bytes from
    // the stream's start leaves exactly two bytes over on every file in the collection.
    let packed_length = u16::from_le_bytes(length_bytes) as usize;
    let body_length = packed_length.saturating_sub(2);
    if pattern_offset + 2 + body_length > source.len() {
        return Err(Error::Truncated { offset: pattern_offset, needed: packed_length });
    }

    let cells = source.with_slice(pattern_offset + 2, body_length, |body| pattern::unpack(body, pattern::ROWS, channels))?;
    builder.add_pattern(&cells, pattern::ROWS, channels)?;
    Ok(())
}

/// A fixed-width text field, decoded as the code page Scream Tracker 3 displayed it in.
/// See [`starplayer_model::decode_cp437`] for why that is CP437 rather than Latin-1.
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

    /// `count` little-endian `u16` parapointers at `offset`.
    fn parapointers(&self, offset: usize, count: u16) -> Result<Vec<u16>, Error> {
        self.with_slice(offset, 2 * count as usize, |bytes| {
            bytes.chunks_exact(2)
                .map(|pair| match pair {
                    [low, high] => u16::from_le_bytes([*low, *high]),
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

    #[test]
    fn probe_wants_scrm_at_forty_four() {
        let mut bytes = vec![0u8; 0x60];
        assert!(!probe(&bytes));
        bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"SCRM");
        assert!(probe(&bytes));
        assert!(probe_reader(&bytes[..]));
        assert!(!probe(&bytes[..0x2E]));
    }

    #[test]
    fn a_fixed_field_becomes_text_without_its_padding() {
        assert_eq!(text(b"Reflex\0\0\0"), "Reflex");
        assert_eq!(text(b"Reflex   "), "Reflex");
        assert_eq!(text(b"  centred  "), "  centred");
        assert_eq!(text(b""), "");
    }

    #[test]
    fn a_file_shorter_than_the_fixed_header_is_truncated_not_a_panic() {
        assert_eq!(load(&[]), Err(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH }));
        assert_eq!(load(&[0u8; 0x5F]), Err(Error::Truncated { offset: 0, needed: header::HEADER_LENGTH }));
    }

    #[test]
    fn a_file_without_the_signature_is_bad_magic() {
        assert_eq!(load(&[0u8; 0x60]), Err(Error::BadMagic));
    }

    // ── C7 research point 3: the production allocation cap ─────────────────────────

    /// A header with `pattern_count` patterns, 32 enabled channels, and exactly the
    /// parapointer table that count requires — every pointer zero, which is Scream
    /// Tracker 3's own spelling of an empty pattern and costs nothing beyond its two
    /// bytes.
    fn amplifying_s3m(pattern_count: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; header::HEADER_LENGTH];
        bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        bytes[0x20..0x22].copy_from_slice(&0u16.to_le_bytes());
        bytes[0x22..0x24].copy_from_slice(&0u16.to_le_bytes());
        bytes[0x24..0x26].copy_from_slice(&pattern_count.to_le_bytes());
        bytes[0x31] = 6;
        bytes[0x32] = 125;
        // All 32 settings enabled, so every pattern unpacks at the maximum stride.
        bytes.resize(header::HEADER_LENGTH + 2 * pattern_count as usize, 0);
        bytes
    }

    #[test]
    fn a_pattern_count_the_file_cannot_justify_is_refused_rather_than_allocated() {
        // 4000 patterns × 64 rows × 32 channels × 5 bytes is 41 MB of decoded patterns out
        // of an 8 KB file. The budget for 8 KB is the 4 MiB floor.
        let bytes = amplifying_s3m(4_000);
        assert!(bytes.len() < 9_000, "the whole attack fits in 9 KB: {} bytes", bytes.len());
        assert_eq!(load(&bytes), Err(Error::TooLarge("S3M pattern data")));
    }

    #[test]
    fn a_pattern_count_a_real_module_would_use_is_still_accepted() {
        // Scream Tracker 3's own limit is 100 patterns; 256 is what the trackers that
        // extended the format allow. Both stay inside the floor at the widest stride.
        let module = load(&amplifying_s3m(256)).expect("256 empty 32-channel patterns are a plausible module");
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
}

//! The S3M file header, the channel-settings array, and the default-panning derivation.
//!
//! Everything here is the file's own spelling of its header — the numbers exactly as the
//! bytes hold them. Turning them into a [`ModuleHeader`](starplayer_model::ModuleHeader)
//! is [`crate::loader`]'s job.

use starplayer_core::fixed::bipolar_from_ratio;
use starplayer_core::quirks::FormatDialect;
use starplayer_core::{Error, I1F15};

/// Offset of the `SCRM` signature.
pub const MAGIC_OFFSET: usize = 0x2C;

/// The signature every S3M carries at [`MAGIC_OFFSET`].
pub const MAGIC: [u8; 4] = *b"SCRM";

/// Bytes of fixed header before the order list — the order list starts at `0x60`.
pub const HEADER_LENGTH: usize = 0x60;

/// Offset of the 32-byte channel-settings array.
pub const CHANNEL_SETTINGS_OFFSET: usize = 0x40;

/// Channels an S3M can describe: the channel-settings array is 32 bytes and the packed
/// pattern format spends five bits on the channel number.
pub const MAX_CHANNELS: usize = 32;

/// Largest channel-settings byte that means "this channel is enabled"
/// (`ParseModule`, `S3MLIB.ASM:2213`: `cmp al,0fh / ja @@nochan`).
pub const CHANNEL_ENABLED_LIMIT: u8 = 0x0F;

/// Value of header byte `0x35` that says a 32-byte default-pan block follows the pattern
/// parapointers (`LoadPanSettings`, `S3MLIB.ASM:3585`).
///
/// Task B2 research point 2 asked whether Scream Tracker 3's own writer ever emits the
/// block *without* this marker. In the repository owner's 29-module collection it never
/// does: the two files that carry a block both set `0x35` to 252, and in the twenty-seven
/// that do not, the bytes where a block would sit are the first parapointed sample header
/// — real data, unmistakably not pan values. So the marker is the only test applied, and
/// no heuristic is guessed at.
pub const DEFAULT_PAN_PRESENT: u8 = 252;

/// The pan nibble a channel starts on before anything overrides it.
///
/// D24: the original's `ClearChannels` writes 7 here; Scream Tracker 3 writes 8, which is
/// also what its own default-pan blocks contain and what libxmp and OpenMPT decode a
/// centred S3M channel to. The canonical value wins.
pub const PAN_CENTRE: u8 = 8;

/// The pan nibble a stereo module's *left* channels get (`ClearChannels`: `mov al,03h`).
pub const PAN_LEFT: u8 = 3;

/// The pan nibble a stereo module's *right* channels get (`ClearChannels`: `mov al,0ch`).
pub const PAN_RIGHT: u8 = 0x0C;

/// Bit of a default-pan block byte that says "this entry overrides the derived pan"
/// (`LoadPanSettings`: `test byte ptr [esi],00100000b`).
pub const PAN_BLOCK_VALID: u8 = 0b0010_0000;

/// `generalflags` bit 4 — clamp periods to the Amiga range. The one general flag the
/// engine itself acts on, so it is lifted into
/// [`ModuleFlags::amiga_limits`](starplayer_model::ModuleFlags::amiga_limits).
pub const GENERAL_FLAG_AMIGA_LIMITS: u16 = 1 << 4;

/// `generalflags` bit 6 — Scream Tracker 3.00's volume slides, which also apply on tick
/// zero. Lifted into
/// [`ModuleFlags::fast_volume_slides`](starplayer_model::ModuleFlags::fast_volume_slides).
pub const GENERAL_FLAG_ST300_VOLUME_SLIDES: u16 = 1 << 6;

/// Tracker version (`Cwt/v`, header `0x28`) written by Scream Tracker 3.00, whose volume
/// slides behave as [`GENERAL_FLAG_ST300_VOLUME_SLIDES`] describes whether or not the
/// flag is set. ST3.01 and later write `0x1301` and up.
pub const TRACKER_VERSION_ST300: u16 = 0x1300;

/// The S3M file header, field for field.
///
/// Field names follow `plans/reference/original-s3mlib-analysis.md` §5 rather than the
/// original's abbreviations, but the values are raw: no clamping, no scaling, no
/// interpretation. `title` is not here because it is a string and this type is `Copy`;
/// the loader reads it straight out of the first 28 bytes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct S3mHeader {
    /// `Ordnum` (`0x20`): entries in the order list.
    pub order_count: u16,
    /// `Insnum` (`0x22`): instrument parapointers that follow the order list.
    pub instrument_count: u16,
    /// `Patnum` (`0x24`): pattern parapointers that follow the instrument parapointers.
    pub pattern_count: u16,
    /// `generalflags` (`0x26`).
    pub general_flags: u16,
    /// `Cwt/v` (`0x28`): the tracker that wrote the file, `0x1xxx` for Scream Tracker 3.
    pub tracker_version: u16,
    /// `ffi` (`0x2A`): 1 = signed sample data, 2 = unsigned. See
    /// [`crate::sample`] for why the loader does not act on it.
    pub file_format_info: u16,
    /// `globalvol` (`0x30`), 0..=64.
    pub global_volume: u8,
    /// `initialspd` (`0x31`): ticks per row.
    pub initial_speed: u8,
    /// `initialBPM` (`0x32`).
    pub initial_tempo: u8,
    /// `mastervol` (`0x33`): amplification in bits 0..=6, stereo in bit 7.
    pub master_volume: u8,
    /// `ultraclick` (`0x34`): the original's GUS click-removal reserve. Historical.
    pub ultra_click_removal: u8,
    /// `defaultpan` (`0x35`): [`DEFAULT_PAN_PRESENT`] iff a 32-byte pan block follows the
    /// pattern parapointers.
    pub default_pan_marker: u8,
    /// `special` (`0x3E`): a parapointer to custom data. Scream Tracker 3 never wrote one,
    /// and its being zero is part of libxmp's ModPlug Tracker fingerprint — which is the
    /// only reason it is parsed here.
    pub special: u16,
    /// The 32-byte channel-settings array at [`CHANNEL_SETTINGS_OFFSET`].
    pub channel_settings: [u8; MAX_CHANNELS],
}

impl S3mHeader {
    /// Parse the fixed header out of the first [`HEADER_LENGTH`] bytes of a file.
    ///
    /// # Errors
    ///
    /// * [`Error::Truncated`] — fewer than [`HEADER_LENGTH`] bytes were supplied.
    /// * [`Error::BadMagic`] — no `SCRM` at [`MAGIC_OFFSET`].
    pub fn parse(header_bytes: &[u8]) -> Result<S3mHeader, Error> {
        let bytes = header_bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;
        if bytes.get(MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()) != Some(&MAGIC[..]) {
            return Err(Error::BadMagic);
        }

        let mut channel_settings = [0u8; MAX_CHANNELS];
        let source = bytes
            .get(CHANNEL_SETTINGS_OFFSET..CHANNEL_SETTINGS_OFFSET + MAX_CHANNELS)
            .ok_or(Error::Truncated { offset: CHANNEL_SETTINGS_OFFSET, needed: MAX_CHANNELS })?;
        channel_settings.copy_from_slice(source);

        Ok(S3mHeader {
            order_count: read_u16(bytes, 0x20)?,
            instrument_count: read_u16(bytes, 0x22)?,
            pattern_count: read_u16(bytes, 0x24)?,
            general_flags: read_u16(bytes, 0x26)?,
            tracker_version: read_u16(bytes, 0x28)?,
            file_format_info: read_u16(bytes, 0x2A)?,
            global_volume: read_u8(bytes, 0x30)?,
            initial_speed: read_u8(bytes, 0x31)?,
            initial_tempo: read_u8(bytes, 0x32)?,
            master_volume: read_u8(bytes, 0x33)?,
            ultra_click_removal: read_u8(bytes, 0x34)?,
            default_pan_marker: read_u8(bytes, 0x35)?,
            special: read_u16(bytes, 0x3E)?,
            channel_settings,
        })
    }

    /// Bit 7 of `mastervol` — the module asks for stereo playback.
    pub const fn is_stereo(&self) -> bool { self.master_volume & 0x80 != 0 }

    /// `mastervol` without its stereo bit: the amplification setting, 0..=127.
    pub const fn master_volume_level(&self) -> u8 { self.master_volume & 0x7F }

    /// `generalflags` bit 4: clamp periods to `[113*4 .. 856*4]`.
    pub const fn amiga_limits(&self) -> bool { self.general_flags & GENERAL_FLAG_AMIGA_LIMITS != 0 }

    /// Whether volume slides also apply on tick zero: `generalflags` bit 6, or a file
    /// written by Scream Tracker 3.00, which behaved that way regardless of the flag.
    pub const fn fast_volume_slides(&self) -> bool {
        self.general_flags & GENERAL_FLAG_ST300_VOLUME_SLIDES != 0 || self.tracker_version == TRACKER_VERSION_ST300
    }

    /// Whether a 32-byte default-pan block follows the pattern parapointers.
    pub const fn has_default_pan_block(&self) -> bool { self.default_pan_marker == DEFAULT_PAN_PRESENT }

    /// Which tracker wrote this file, as far as `Cwt/v` and its companion fields say.
    ///
    /// Reproduces libxmp `src/loaders/s3m_load.c:390-432` predicate for predicate, because
    /// the pinned conformance dumps for the dialect fixtures were generated by exactly
    /// that code. Only the four tracker profiles whose *pattern-loop* behaviour differs
    /// are distinguished; a `Cwt/v` naming Impulse Tracker, Schism, BeRoTracker, OpenMPT,
    /// PlayerPRO or Velvet Studio keeps the Scream Tracker 3.21 default, which is what
    /// libxmp does for all but Impulse Tracker (whose S3M flow arrives with M6).
    pub const fn dialect(&self) -> FormatDialect {
        match self.tracker_version >> 12 {
            1 => {
                // ModPlug Tracker 1.16 / OpenMPT 1.17: `Cwt/v` 0x1320 with no special
                // parapointer, an order count that is a multiple of 16, no ultra-click
                // reserve, no general flag outside bits 4 and 6, and a pan block present.
                if self.tracker_version == 0x1320
                    && self.special == 0
                    && self.order_count & 0x0F == 0
                    && self.ultra_click_removal == 0
                    && self.general_flags & !0x50 == 0
                    && self.default_pan_marker == DEFAULT_PAN_PRESENT
                {
                    return FormatDialect::ModPlug116;
                }
                // PlayerPRO and Velvet Studio also write 0x1320; libxmp names them but
                // leaves the flow at ST3.21.
                if self.tracker_version == 0x1320
                    && self.special == 0
                    && self.ultra_click_removal == 0
                    && self.general_flags == 0
                    && self.default_pan_marker == 0
                {
                    return FormatDialect::ScreamTracker321;
                }
                match self.tracker_version < 0x1303 {
                    true => FormatDialect::ScreamTracker301,
                    false => FormatDialect::ScreamTracker321,
                }
            }
            // 0x2013 is PlayerPRO on a little-endian host, not Imago Orpheus.
            2 if self.tracker_version != 0x2013 => FormatDialect::ImagoOrpheus,
            _ => FormatDialect::ScreamTracker321,
        }
    }

    /// Channels the song plays on: the number of channel-settings bytes that are
    /// [`CHANNEL_ENABLED_LIMIT`] or less, exactly as `ParseModule` counts them.
    ///
    /// Note that this is a *count*, not a highest index: a file whose enabled channels
    /// are not a contiguous prefix (which nothing in the owner's 1994–96 collection is,
    /// and which Scream Tracker 3 never wrote) has a count smaller than the channel
    /// numbers its patterns use. [`S3mHeader::addressed_channels`] is the number the
    /// loader stores patterns with, for exactly that reason.
    pub fn channel_count(&self) -> u8 {
        let count = self.channel_settings.iter().filter(|setting| **setting <= CHANNEL_ENABLED_LIMIT).count();
        // `MAX_CHANNELS` is 32, so the count always fits a `u8`.
        count as u8
    }

    /// Channel columns a pattern has to store: [`S3mHeader::channel_count`], widened to
    /// cover the highest *enabled* channel index when the enabled set is not a contiguous
    /// prefix. Equal to `channel_count` for every well-formed file.
    ///
    /// The original kept 32 channels of state and simply looped over the first
    /// `_TotalChanNum` of them, so a packed cell's channel number indexes the channel
    /// directly. Storing `channel_count` columns would silently drop the data of an
    /// enabled channel sitting above a disabled one; storing all 32 would cost 10 KB a
    /// pattern on an embedded target. This is the middle: no data lost, nothing wasted on
    /// a file Scream Tracker 3 actually wrote.
    pub fn addressed_channels(&self) -> u8 {
        let highest_enabled = self.channel_settings.iter()
            .rposition(|setting| *setting <= CHANNEL_ENABLED_LIMIT)
            .map_or(0, |index| index + 1);
        // Both operands are at most `MAX_CHANNELS` = 32.
        core::cmp::max(self.channel_count() as usize, highest_enabled) as u8
    }

    /// Offset of the 32-byte default-pan block: past the order list, the instrument
    /// parapointers and the pattern parapointers (`LoadPanSettings`, `S3MLIB.ASM:3591`).
    ///
    /// Also the offset the first parapointed thing may not start before, so the loader
    /// uses it as the end of the header tables whether or not a pan block is there.
    pub const fn tables_end(&self) -> usize {
        HEADER_LENGTH + self.order_count as usize + 2 * self.instrument_count as usize + 2 * self.pattern_count as usize
    }
}

/// The S3M-owned header bits, packed into
/// [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader::format_extra).
///
/// The engine passes `format_extra` through untouched; this crate is the only thing that
/// may interpret it, and this type is the interpretation. The effect processor reads it
/// with [`S3mFormatExtra::decode`].
///
/// # What fits, and what does not
///
/// `format_extra` is a `u32` and the three fields here are exactly 32 bits. The dropped
/// half of `generalflags` is its high byte, which the S3M format leaves undefined — one
/// file in the owner's collection (`LOSTIN8.S3M`) has bit 8 set with no meaning attached
/// to it, and Scream Tracker 3 never reads it. `ffi` (`0x2A`) is not here either because
/// the loader consumes it (see [`crate::sample`]) and nothing downstream needs it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct S3mFormatExtra {
    /// `Cwt/v` (`0x28`): the tracker and version that wrote the file.
    pub tracker_version: u16,
    /// `mastervol` (`0x33`) raw, stereo bit included.
    pub master_volume: u8,
    /// The low, defined byte of `generalflags` (`0x26`).
    pub general_flags: u8,
}

impl S3mFormatExtra {
    /// Pack into the `u32` the module header carries.
    pub const fn encode(&self) -> u32 {
        self.tracker_version as u32 | ((self.master_volume as u32) << 16) | ((self.general_flags as u32) << 24)
    }

    /// Unpack the `u32` a module built by this crate carries.
    pub const fn decode(bits: u32) -> S3mFormatExtra {
        S3mFormatExtra {
            tracker_version: bits as u16,
            master_volume: (bits >> 16) as u8,
            general_flags: (bits >> 24) as u8,
        }
    }

    /// Extract the S3M-owned bits from a header this crate produced.
    pub fn from_header(header: &starplayer_model::ModuleHeader) -> S3mFormatExtra {
        S3mFormatExtra::decode(header.format_extra)
    }

    /// Bit 7 of `mastervol`.
    pub const fn is_stereo(&self) -> bool { self.master_volume & 0x80 != 0 }

    /// `mastervol` without its stereo bit, 0..=127.
    pub const fn master_volume_level(&self) -> u8 { self.master_volume & 0x7F }
}

/// The per-channel pan nibbles a module starts with, as `ClearChannels`
/// (`S3MLIB.ASM:3617`) and `LoadPanSettings` (`S3MLIB.ASM:3585`) between them produce
/// them.
///
/// Reproduced exactly, in the original's own order:
///
/// 1. every channel starts at [`PAN_CENTRE`];
/// 2. if `stereo`, channel `n`'s setting byte decides: `>= 128` (a disabled or
///    Adlib channel) stays centred, `< 8` is [`PAN_LEFT`], anything else is
///    [`PAN_RIGHT`];
/// 3. if a pan block is present it overrides, per entry, but **only** where
///    [`PAN_BLOCK_VALID`] is set in that entry; the value taken is the low nibble.
///
/// Step 3 runs whether or not the module is stereo, because `LoadPanSettings` does.
pub fn default_pan_nibbles(channel_settings: &[u8; MAX_CHANNELS], stereo: bool, pan_block: Option<&[u8; MAX_CHANNELS]>) -> [u8; MAX_CHANNELS] {
    let mut nibbles = [PAN_CENTRE; MAX_CHANNELS];

    for (channel, nibble) in nibbles.iter_mut().enumerate() {
        if stereo && let Some(setting) = channel_settings.get(channel) {
            *nibble = match *setting {
                128.. => PAN_CENTRE,
                0..8 => PAN_LEFT,
                _ => PAN_RIGHT,
            };
        }
        if let Some(block) = pan_block
            && let Some(entry) = block.get(channel)
            && entry & PAN_BLOCK_VALID != 0
        {
            *nibble = entry & 0x0F;
        }
    }
    nibbles
}

/// Turn one 0..=15 pan nibble into the model's bipolar pan.
///
/// **Centre is 7.5, not 7 or 8.** The original's two stereo defaults are [`PAN_LEFT`] = 3
/// and [`PAN_RIGHT`] = 12, which are equidistant from 7.5. Both 7 and 8 are the integer
/// neighbours of a centre that the 4-bit GUS balance register cannot express exactly, so
/// the mapping used here is `(2 * nibble - 15) / 15`: hard left and hard right reach full
/// scale, 3 and 12 stay symmetric, and 7 and 8 land 6.7 % either side of centre. Scream
/// Tracker 3 picks 8 as its own centre, so [`PAN_CENTRE`] is 8.
///
/// A mono module never goes through this at all — the loader leaves its pan table empty,
/// which is the model's "centre every channel".
pub fn pan_nibble_to_bipolar(nibble: u8) -> I1F15 {
    bipolar_from_ratio(2 * (nibble & 0x0F) as i32 - 15, 15)
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Truncated { offset, needed: 1 })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    let pair = bytes.get(offset..offset + 2).ok_or(Error::Truncated { offset, needed: 2 })?;
    match pair {
        [low, high] => Ok(u16::from_le_bytes([*low, *high])),
        _ => Err(Error::Truncated { offset, needed: 2 }),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A 0x60-byte header with `SCRM` in place and `channel_settings` as given.
    fn synthetic_header(master_volume: u8, default_pan_marker: u8, channel_settings: &[u8]) -> [u8; HEADER_LENGTH] {
        let mut bytes = [0xFFu8; HEADER_LENGTH];
        bytes[..28].fill(0);
        bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        bytes[0x33] = master_volume;
        bytes[0x35] = default_pan_marker;
        for (index, setting) in channel_settings.iter().enumerate() {
            bytes[CHANNEL_SETTINGS_OFFSET + index] = *setting;
        }
        bytes
    }

    /// One test per branch of libxmp's `Cwt/v` predicate
    /// (`src/loaders/s3m_load.c:390-432`), with the value that selects it.
    #[test]
    fn the_cwtv_field_selects_one_dialect_per_libxmp_branch() {
        // A file that satisfies every ModPlug 1.16 fingerprint field except `Cwt/v`.
        fn modplug_shaped(tracker_version: u16) -> [u8; HEADER_LENGTH] {
            let mut bytes = synthetic_header(0xB0, DEFAULT_PAN_PRESENT, &[0, 8]);
            bytes[0x20..0x22].copy_from_slice(&16u16.to_le_bytes()); // Ordnum, a multiple of 16
            bytes[0x26..0x28].copy_from_slice(&0u16.to_le_bytes()); // generalflags
            bytes[0x28..0x2A].copy_from_slice(&tracker_version.to_le_bytes());
            bytes[0x34] = 0; // ultraclick
            bytes[0x3E..0x40].copy_from_slice(&0u16.to_le_bytes()); // special
            bytes
        }
        fn dialect_of(bytes: &[u8; HEADER_LENGTH]) -> FormatDialect {
            S3mHeader::parse(bytes).expect("a valid header").dialect()
        }

        assert_eq!(dialect_of(&modplug_shaped(0x1320)), FormatDialect::ModPlug116, "0x1320 with the ModPlug marker fields");
        // The same version without the pan block is PlayerPRO / Velvet Studio, which
        // libxmp names but leaves on the ST3.21 flow.
        let mut velvet = modplug_shaped(0x1320);
        velvet[0x35] = 0;
        assert_eq!(dialect_of(&velvet), FormatDialect::ScreamTracker321);
        // And a 0x1320 that matches neither fingerprint is just a Scream Tracker version.
        let mut neither = modplug_shaped(0x1320);
        neither[0x34] = 1;
        assert_eq!(dialect_of(&neither), FormatDialect::ScreamTracker321, "0x1320 >= 0x1303 is ST3.21 behaviour");

        assert_eq!(dialect_of(&modplug_shaped(0x1300)), FormatDialect::ScreamTracker301, "ST3.00");
        assert_eq!(dialect_of(&modplug_shaped(0x1301)), FormatDialect::ScreamTracker301, "ST3.01");
        assert_eq!(dialect_of(&modplug_shaped(0x1302)), FormatDialect::ScreamTracker301, "the last version below 0x1303");
        assert_eq!(dialect_of(&modplug_shaped(0x1303)), FormatDialect::ScreamTracker321, "ST3.21 itself");

        assert_eq!(dialect_of(&modplug_shaped(0x2100)), FormatDialect::ImagoOrpheus, "high nibble 2");
        assert_eq!(dialect_of(&modplug_shaped(0x2013)), FormatDialect::ScreamTracker321, "0x2013 is PlayerPRO byte-swapped, not Imago Orpheus");

        // Impulse Tracker, Schism, BeRoTracker and OpenMPT keep the default; their own S3M
        // flow modes arrive with their formats.
        for version in [0x3216u16, 0x4100, 0x5000, 0x6000, 0x0000] {
            assert_eq!(dialect_of(&modplug_shaped(version)), FormatDialect::ScreamTracker321, "{version:#06x}");
        }
    }

    #[test]
    fn the_special_parapointer_is_read_because_the_modplug_fingerprint_needs_it() {
        let mut bytes = synthetic_header(0xB0, DEFAULT_PAN_PRESENT, &[0, 8]);
        bytes[0x3E..0x40].copy_from_slice(&0x1234u16.to_le_bytes());
        assert_eq!(S3mHeader::parse(&bytes).expect("a valid header").special, 0x1234);
    }

    #[test]
    fn a_header_without_the_signature_is_rejected() {
        let mut bytes = synthetic_header(0x80, 0, &[0, 8]);
        bytes[MAGIC_OFFSET] = b'X';
        assert_eq!(S3mHeader::parse(&bytes), Err(Error::BadMagic));
        assert_eq!(S3mHeader::parse(&bytes[..HEADER_LENGTH - 1]), Err(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
    }

    #[test]
    fn the_channel_count_is_the_number_of_settings_at_or_below_fifteen() {
        let bytes = synthetic_header(0x80, 0, &[0, 8, 1, 9, 2]);
        let header = S3mHeader::parse(&bytes).expect("a valid header");

        assert_eq!(header.channel_count(), 5);
        assert_eq!(header.addressed_channels(), 5);
        assert!(header.is_stereo());
    }

    #[test]
    fn a_non_contiguous_enabled_set_widens_the_addressed_channels_but_not_the_count() {
        // Channel 1 disabled, channel 2 enabled: two enabled channels spanning indices 0..3.
        let bytes = synthetic_header(0x80, 0, &[0, 255, 8]);
        let header = S3mHeader::parse(&bytes).expect("a valid header");

        assert_eq!(header.channel_count(), 2, "ParseModule counts, it does not span");
        assert_eq!(header.addressed_channels(), 3, "channel 2's pattern data still has somewhere to go");
    }

    #[test]
    fn a_stereo_module_without_a_pan_block_gets_the_clearchannels_defaults() {
        let settings = [0u8, 8, 1, 9, 128, 255];
        let mut channel_settings = [255u8; MAX_CHANNELS];
        channel_settings[..settings.len()].copy_from_slice(&settings);

        let nibbles = default_pan_nibbles(&channel_settings, true, None);
        assert_eq!(&nibbles[..6], &[PAN_LEFT, PAN_RIGHT, PAN_LEFT, PAN_RIGHT, PAN_CENTRE, PAN_CENTRE]);
    }

    #[test]
    fn a_mono_module_is_centred_whatever_its_channel_settings_say() {
        let mut channel_settings = [255u8; MAX_CHANNELS];
        channel_settings[..4].copy_from_slice(&[0, 8, 1, 9]);

        assert_eq!(default_pan_nibbles(&channel_settings, false, None), [PAN_CENTRE; MAX_CHANNELS]);
    }

    #[test]
    fn a_pan_block_overrides_only_the_entries_whose_bit_five_is_set() {
        let mut channel_settings = [255u8; MAX_CHANNELS];
        channel_settings[..4].copy_from_slice(&[0, 8, 1, 9]);
        let mut block = [0u8; MAX_CHANNELS];
        // 0x25 = valid, pan 5. 0x0C = pan 12 but bit 5 clear, so it is ignored.
        block[..4].copy_from_slice(&[0x25, 0x0C, 0x2F, 0x80]);

        let nibbles = default_pan_nibbles(&channel_settings, true, Some(&block));
        assert_eq!(&nibbles[..4], &[5, PAN_RIGHT, 15, PAN_RIGHT]);
    }

    #[test]
    fn a_pan_block_applies_to_a_mono_module_too_because_loadpansettings_does() {
        let channel_settings = [255u8; MAX_CHANNELS];
        let mut block = [0u8; MAX_CHANNELS];
        block[0] = 0x20;

        assert_eq!(default_pan_nibbles(&channel_settings, false, Some(&block))[0], 0);
    }

    #[test]
    fn the_pan_law_is_symmetric_about_the_originals_two_stereo_defaults() {
        assert_eq!(pan_nibble_to_bipolar(0), I1F15::MIN + I1F15::DELTA, "hard left is full scale");
        assert_eq!(pan_nibble_to_bipolar(15), I1F15::MAX, "hard right is full scale");
        assert_eq!(pan_nibble_to_bipolar(PAN_LEFT), -pan_nibble_to_bipolar(PAN_RIGHT));
        assert_eq!(pan_nibble_to_bipolar(PAN_CENTRE), -pan_nibble_to_bipolar(7));
        assert!(pan_nibble_to_bipolar(PAN_CENTRE) > I1F15::ZERO, "D24: ST3's centre nibble sits just right of centre");
    }

    #[test]
    fn the_format_extra_word_round_trips_every_field() {
        let extra = S3mFormatExtra { tracker_version: 0x1320, master_volume: 0xB0, general_flags: 0x50 };
        assert_eq!(S3mFormatExtra::decode(extra.encode()), extra);
        assert_eq!(extra.master_volume_level(), 0x30);
        assert!(extra.is_stereo());
    }

    #[test]
    fn st3_00_gets_fast_volume_slides_even_without_the_flag() {
        let mut bytes = synthetic_header(0x80, 0, &[0, 8]);
        bytes[0x26] = 0;
        bytes[0x27] = 0;
        bytes[0x28] = 0x00;
        bytes[0x29] = 0x13;
        let header = S3mHeader::parse(&bytes).expect("a valid header");

        assert!(header.fast_volume_slides());
        assert!(!header.amiga_limits());
    }
}

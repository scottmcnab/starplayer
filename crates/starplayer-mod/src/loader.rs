//! Bounds-checked 31-sample MOD loading.

use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::{Error, I1F15, U0F16};
use starplayer_model::{InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader, ModuleReader, ORDER_END, SampleSpec};

use crate::pattern::{CELL_BYTES, ROWS};
use crate::tables::{AMIGA_CHANNEL_MAP, FINETUNE_REFERENCE_RATES};

const HEADER_BYTES: usize = 1084;
const MAGIC_OFFSET: usize = 1080;
const SAMPLE_COUNT: usize = 31;
const SAMPLE_HEADER_BYTES: usize = 30;
const SONG_LENGTH_OFFSET: usize = 950;
const ORDER_OFFSET: usize = 952;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StereoSeparation(u8);

impl StereoSeparation {
    pub const HARD: StereoSeparation = StereoSeparation(100);
    pub const MONO: StereoSeparation = StereoSeparation(0);

    pub const fn percent(percent: u8) -> StereoSeparation {
        StereoSeparation(if percent > 100 { 100 } else { percent })
    }

    pub const fn get(self) -> u8 { self.0 }
}

impl Default for StereoSeparation {
    fn default() -> StereoSeparation { StereoSeparation::HARD }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LoadOptions {
    pub stereo_separation: StereoSeparation,
}

pub fn probe(bytes: &[u8]) -> bool {
    bytes.get(MAGIC_OFFSET..MAGIC_OFFSET + 4).and_then(layout).is_some()
}

pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool {
    let mut magic = [0u8; 4];
    reader.read_at(MAGIC_OFFSET, &mut magic).is_ok() && layout(&magic).is_some()
}

pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_with_options(bytes, LoadOptions::default()) }
pub fn load_with_options(bytes: &[u8], options: LoadOptions) -> Result<Module, Error> { load_from_with_options(bytes, options) }
pub fn load_from<R: ModuleReader + ?Sized>(reader: &R) -> Result<Module, Error> { load_from_with_options(reader, LoadOptions::default()) }

pub fn load_from_with_options<R: ModuleReader + ?Sized>(reader: &R, options: LoadOptions) -> Result<Module, Error> {
    let source = Source { reader };
    let fixed: [u8; HEADER_BYTES] = source.array(0)?;
    let layout = layout(fixed.get(MAGIC_OFFSET..MAGIC_OFFSET + 4).unwrap_or(&[])).ok_or(Error::BadMagic)?;
    let channels = layout.channels;
    // libxmp loads a zero-length order list and plays nothing; only a length past the
    // 128-entry table is a structural error.
    let song_length = (fixed.get(SONG_LENGTH_OFFSET).copied().unwrap_or(0) as usize).min(128);

    let order_bytes = fixed.get(ORDER_OFFSET..ORDER_OFFSET + 128).ok_or(Error::Truncated { offset: ORDER_OFFSET, needed: 128 })?;
    // ProTracker scans the complete on-disk table when locating sample data. Entries
    // after song_length are not played, but their patterns still occupy file space.
    // ProTracker's mt_init compares each byte signed (`cmp.b` / `bgt`), so any entry
    // with the high bit set can never raise the maximum; libxmp reaches the same result
    // by breaking at the first byte above 0x7f (its "dragnet.mod" fix).
    let pattern_count = order_bytes.iter().copied().filter(|order| *order < 0x80)
        .map(|order| logical_order(order, layout) as usize).max().map(|order| order + 1).unwrap_or(0);
    let pattern_bytes = (pattern_count as u64)
        .checked_mul(ROWS as u64).and_then(|value| value.checked_mul(channels as u64)).and_then(|value| value.checked_mul(CELL_BYTES as u64))
        .and_then(|value| usize::try_from(value).ok()).ok_or(Error::TooLarge("MOD pattern data"))?;
    let sample_data_offset = HEADER_BYTES.checked_add(pattern_bytes).ok_or(Error::TooLarge("MOD data offset"))?;
    if sample_data_offset > source.len() { return Err(Error::Truncated { offset: HEADER_BYTES, needed: pattern_bytes }); }

    let mut builder = ModuleBuilder::new();
    let pattern_body = source.with_slice(HEADER_BYTES, pattern_bytes, <[u8]>::to_vec)?;
    let pattern_stride = ROWS as usize * channels as usize * CELL_BYTES;
    if layout.paired_four_channel_patterns {
        // Startrekker FLT8 stores channels 0..3 for all 64 rows, then channels 4..7
        // for all 64 rows. Keep the model's native MOD cells but reorder each logical
        // pattern into the row-major stride used by PatternView and ModProcessor.
        let half_stride = ROWS as usize * 4 * CELL_BYTES;
        for stored in pattern_body.chunks_exact(pattern_stride) {
            let mut pattern = vec![0; pattern_stride];
            for row in 0..ROWS as usize {
                let target = row * 8 * CELL_BYTES;
                let first = row * 4 * CELL_BYTES;
                let second = half_stride + first;
                pattern[target..target + 4 * CELL_BYTES].copy_from_slice(&stored[first..first + 4 * CELL_BYTES]);
                pattern[target + 4 * CELL_BYTES..target + 8 * CELL_BYTES].copy_from_slice(&stored[second..second + 4 * CELL_BYTES]);
            }
            builder.add_pattern(&pattern, ROWS, channels)?;
        }
    } else {
        for pattern in pattern_body.chunks_exact(pattern_stride) { builder.add_pattern(pattern, ROWS, channels)?; }
    }

    let amiga_limits = pattern_body.chunks_exact(CELL_BYTES).filter_map(crate::pattern::ModCell::from_bytes)
        .all(|cell| cell.period == 0 || (113..=856).contains(&cell.period));

    let mut declared_sample_offset = sample_data_offset;
    for index in 0..SAMPLE_COUNT {
        let header_offset = 20 + index * SAMPLE_HEADER_BYTES;
        let header = fixed.get(header_offset..header_offset + SAMPLE_HEADER_BYTES).ok_or(Error::Truncated { offset: header_offset, needed: SAMPLE_HEADER_BYTES })?;
        let name = starplayer_model::decode_cp437(header.get(..22).unwrap_or(&[]));
        let declared_length = be_u16(header, 22)? as usize * 2;
        let finetune = header.get(24).copied().unwrap_or(0) & 15;
        let volume = header.get(25).copied().unwrap_or(0).min(64);
        let loop_start = (be_u16(header, 26)? as usize * 2).min(declared_length);
        let loop_length = be_u16(header, 28)? as usize * 2;
        let available = source.len().saturating_sub(declared_sample_offset).min(declared_length);
        let raw = source.with_slice(declared_sample_offset.min(source.len()), available, <[u8]>::to_vec)?;
        let pcm: Vec<i16> = raw.iter().map(|byte| (*byte as i8 as i16) * 256).collect();
        declared_sample_offset = declared_sample_offset.checked_add(declared_length).ok_or(Error::TooLarge("MOD sample data"))?;

        let loop_end = loop_start.saturating_add(loop_length).min(pcm.len());
        // ProTracker loops whenever n_replen is more than one word, that is a repeat
        // length of four bytes or more; libxmp's `loop_size > 1` is the same test. The
        // `> 4` rule came from the DOS original's ConvertSamps, not from the format.
        let loops = loop_length >= 4 && loop_start < loop_end;
        let specification = SampleSpec {
            name: name.clone(),
            loop_mode: if loops { LoopMode::Forward } else { LoopMode::None },
            loop_start: if loops { loop_start as u32 } else { 0 },
            loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(volume as u32, 64),
            reference_rate_hz: FINETUNE_REFERENCE_RATES[finetune as usize],
        };
        let sample = builder.add_sample(&pcm, specification)?;
        builder.add_instrument(InstrumentDef::from_sample(&name, sample, U0F16::MAX))?;
    }

    let mut orders: Vec<u16> = order_bytes.iter().take(song_length)
        .map(|order| if *order == 255 { ORDER_END } else { logical_order(*order, layout) as u16 }).collect();
    if orders.last().copied() != Some(ORDER_END) { orders.push(ORDER_END); }
    builder.set_orders(&orders);
    builder.set_header(ModuleHeader {
        title: starplayer_model::decode_cp437(fixed.get(..20).unwrap_or(&[])).into_boxed_str(),
        format: ModuleFormat::Mod,
        channel_count: channels,
        initial_speed: 6,
        initial_tempo: 125,
        global_volume: U0F16::MAX,
        master_volume: U0F16::MAX,
        default_pan: default_pan(channels, options.stereo_separation),
        flags: ModuleFlags { amiga_limits, linear_slides: false, fast_volume_slides: false, stereo: options.stereo_separation.get() != 0 },
        format_extra: 0,
    });
    builder.build()
}

#[derive(Copy, Clone)]
struct ModLayout {
    channels: u8,
    paired_four_channel_patterns: bool,
}

fn layout(magic: &[u8]) -> Option<ModLayout> {
    match magic {
        // `M!K!` is what ProTracker itself writes once a module exceeds 64 patterns.
        b"M.K." | b"M!K!" | b"FLT4" => Some(ModLayout { channels: 4, paired_four_channel_patterns: false }),
        b"6CHN" => Some(ModLayout { channels: 6, paired_four_channel_patterns: false }),
        b"8CHN" => Some(ModLayout { channels: 8, paired_four_channel_patterns: false }),
        b"FLT8" => Some(ModLayout { channels: 8, paired_four_channel_patterns: true }),
        [tens, ones, b'C', b'H'] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
            let count = (tens - b'0') * 10 + (ones - b'0');
            (count > 0 && count <= 32).then_some(ModLayout { channels: count, paired_four_channel_patterns: false })
        }
        // Single-digit `dCHN`, the form several trackers write for 1..9 channels.
        [digit, b'C', b'H', b'N'] if digit.is_ascii_digit() && *digit != b'0' => {
            Some(ModLayout { channels: digit - b'0', paired_four_channel_patterns: false })
        }
        _ => None,
    }
}

fn logical_order(order: u8, layout: ModLayout) -> u8 {
    if layout.paired_four_channel_patterns { order >> 1 } else { order }
}

fn default_pan(channels: u8, separation: StereoSeparation) -> alloc::boxed::Box<[I1F15]> {
    let amount = separation.get() as i32;
    (0..channels).map(|channel| {
        let mapped = AMIGA_CHANNEL_MAP[channel as usize % AMIGA_CHANNEL_MAP.len()];
        bipolar_from_ratio(if mapped < 8 { -amount } else { amount }, 100)
    }).collect::<Vec<_>>().into_boxed_slice()
}

fn be_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    match bytes.get(offset..offset + 2) {
        Some([high, low]) => Ok(u16::from_be_bytes([*high, *low])),
        _ => Err(Error::Truncated { offset, needed: 2 }),
    }
}

struct Source<'reader, R: ModuleReader + ?Sized> { reader: &'reader R }

impl<R: ModuleReader + ?Sized> Source<'_, R> {
    fn len(&self) -> usize { self.reader.len() }
    fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], Error> {
        let mut result = [0; N];
        self.reader.read_at(offset, &mut result)?;
        Ok(result)
    }
    fn with_slice<T>(&self, offset: usize, length: usize, consume: impl FnOnce(&[u8]) -> T) -> Result<T, Error> {
        if let Some(slice) = self.reader.slice_at(offset, length) { return Ok(consume(slice)); }
        let mut buffer = vec![0; length];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(consume(&buffer))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::ModCell;

    fn minimal_mod() -> Vec<u8> {
        let mut bytes = vec![0; HEADER_BYTES + ROWS as usize * 4 * CELL_BYTES + 12];
        bytes[..5].copy_from_slice(b"title");
        bytes[20..26].copy_from_slice(b"sample");
        bytes[42..44].copy_from_slice(&6u16.to_be_bytes());
        bytes[44] = 15;
        bytes[45] = 80;
        bytes[46..48].copy_from_slice(&2u16.to_be_bytes());
        bytes[48..50].copy_from_slice(&2u16.to_be_bytes());
        bytes[SONG_LENGTH_OFFSET] = 1;
        bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"M.K.");
        bytes[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&ModCell { period: 428, instrument: 1, ..ModCell::EMPTY }.to_bytes());
        let sample_offset = HEADER_BYTES + ROWS as usize * 4 * CELL_BYTES;
        bytes[sample_offset..sample_offset + 12].copy_from_slice(&[0x80, 0xFF, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        bytes
    }

    #[test]
    fn the_native_layout_loads_with_signed_pcm_and_hard_lrrl_pan() {
        let module = load(&minimal_mod()).expect("valid MOD");
        assert_eq!(module.header().format, ModuleFormat::Mod);
        assert_eq!(module.pattern_bytes(starplayer_model::PatternId(0)).and_then(|bytes| ModCell::from_bytes(bytes.get(..4)?)), Some(ModCell { period: 428, instrument: 1, ..ModCell::EMPTY }));
        assert_eq!(module.sample_pcm(starplayer_core::SampleId(0)).and_then(|pcm| pcm.first()).copied(), Some(i16::MIN));
        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.reference_rate_hz()), Some(8280));
        assert_eq!(module.sample(starplayer_core::SampleId(0)).map(|sample| sample.loop_mode()), Some(LoopMode::Forward), "ProTracker loops a two-word repeat");
        assert!(module.header().channel_pan(0).is_some_and(|pan| pan < I1F15::ZERO));
        assert!(module.header().channel_pan(1).is_some_and(|pan| pan > I1F15::ZERO));
        assert!(module.header().channel_pan(2).is_some_and(|pan| pan > I1F15::ZERO));
        assert!(module.header().channel_pan(3).is_some_and(|pan| pan < I1F15::ZERO));
        assert!(module.header().flags.amiga_limits);
    }

    #[test]
    fn an_extended_note_disables_amiga_limits() {
        let mut bytes = minimal_mod();
        bytes[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&ModCell { period: 1712, instrument: 1, ..ModCell::EMPTY }.to_bytes());
        assert!(!load(&bytes).expect("extended MOD").header().flags.amiga_limits);
    }

    #[test]
    fn sixty_percent_separation_is_exact_and_keeps_the_lrrl_assignment() {
        let hard = load(&minimal_mod()).expect("hard");
        let headphone = load_with_options(&minimal_mod(), LoadOptions { stereo_separation: StereoSeparation::percent(60) }).expect("headphone");
        let full = bipolar_from_ratio(1, 1);
        let three_fifths = bipolar_from_ratio(3, 5);
        assert_eq!((0..4).map(|channel| hard.header().channel_pan(channel).expect("hard pan")).collect::<Vec<_>>(), [-full, full, full, -full]);
        assert_eq!((0..4).map(|channel| headphone.header().channel_pan(channel).expect("headphone pan")).collect::<Vec<_>>(), [-three_fifths, three_fifths, three_fifths, -three_fifths]);
    }

    #[test]
    fn tagless_fifteen_sample_files_are_explicitly_not_probed() {
        assert!(!probe(&vec![0; 1084]));
        assert_eq!(load(&vec![0; 1084]), Err(Error::BadMagic));
    }

    #[test]
    fn inactive_order_tail_still_locates_stored_patterns_and_sample_data() {
        let one_pattern = minimal_mod();
        let pattern_stride = ROWS as usize * 4 * CELL_BYTES;
        let mut bytes = vec![0; one_pattern.len() + pattern_stride];
        bytes[..HEADER_BYTES + pattern_stride].copy_from_slice(&one_pattern[..HEADER_BYTES + pattern_stride]);
        bytes[HEADER_BYTES + pattern_stride..HEADER_BYTES + pattern_stride * 2].fill(0x55);
        bytes[HEADER_BYTES + pattern_stride * 2..].copy_from_slice(&one_pattern[HEADER_BYTES + pattern_stride..]);
        bytes[ORDER_OFFSET + 126] = 255;
        bytes[ORDER_OFFSET + 127] = 1;

        let module = load(&bytes).expect("inactive order patterns still occupy the file");
        assert!(module.pattern(starplayer_model::PatternId(0)).is_some());
        assert!(module.pattern(starplayer_model::PatternId(1)).is_some());
        assert!(module.pattern(starplayer_model::PatternId(2)).is_none());
        assert_eq!(module.sample_pcm(starplayer_core::SampleId(0)).and_then(|pcm| pcm.first()).copied(), Some(i16::MIN));
    }

    #[test]
    fn flt8_pairs_four_channel_pattern_halves_and_order_numbers() {
        let pattern_stride = ROWS as usize * 8 * CELL_BYTES;
        let mut bytes = vec![0; HEADER_BYTES + pattern_stride * 2];
        bytes[SONG_LENGTH_OFFSET] = 2;
        bytes[ORDER_OFFSET + 1] = 2;
        bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"FLT8");
        bytes[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&ModCell { period: 428, instrument: 1, ..ModCell::EMPTY }.to_bytes());
        let second_half = HEADER_BYTES + ROWS as usize * 4 * CELL_BYTES;
        bytes[second_half..second_half + 4].copy_from_slice(&ModCell { period: 214, instrument: 2, ..ModCell::EMPTY }.to_bytes());
        let second_pattern = HEADER_BYTES + pattern_stride;
        bytes[second_pattern..second_pattern + 4].copy_from_slice(&ModCell { period: 856, instrument: 3, ..ModCell::EMPTY }.to_bytes());

        let module = load(&bytes).expect("paired FLT8");
        assert_eq!(module.header().channel_count, 8);
        assert_eq!(module.order_entry(0), Some(starplayer_model::OrderEntry::Pattern(starplayer_model::PatternId(0))));
        assert_eq!(module.order_entry(1), Some(starplayer_model::OrderEntry::Pattern(starplayer_model::PatternId(1))));
        let first = crate::PatternView::new(&module, starplayer_model::PatternId(0)).expect("first pattern");
        assert_eq!(first.cell(0, 0).map(|cell| cell.period), Some(428));
        assert_eq!(first.cell(0, 4).map(|cell| cell.period), Some(214));
        let second = crate::PatternView::new(&module, starplayer_model::PatternId(1)).expect("second pattern");
        assert_eq!(second.cell(0, 0).map(|cell| cell.period), Some(856));
    }

    // ── C3b: ProTracker fidelity repairs ───────────────────────────────────────────

    #[test]
    fn a_two_word_loop_length_is_a_real_protracker_loop() {
        let module = load(&minimal_mod()).expect("valid MOD");
        let sample = module.sample(starplayer_core::SampleId(0)).expect("sample");
        assert_eq!(sample.loop_mode(), LoopMode::Forward, "n_replen of one word loops in ProTracker");
        assert_eq!((sample.loop_start(), sample.loop_end()), (4, 8));

        // And the voice built from it wraps rather than ending.
        let region = starplayer_mixer::LoopSpan::new(sample.loop_start(), sample.loop_end())
            .map(|span| starplayer_mixer::SampleRegion::looping(sample.pcm_offset(), span)).expect("a looping region");
        let mut voices = starplayer_mixer::VoicePool::new(1);
        let params = starplayer_core::VoiceParams { step: starplayer_core::Step::ONE, volume: U0F16::MAX, ..starplayer_core::VoiceParams::SILENT };
        voices.allocate(starplayer_mixer::VoiceTag::default(), region, params, 0).expect("slot 0");
        let mut destination = [starplayer_mixer::FixedFrame::default(); 64];
        voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(module.pcm(), &mut destination);
        assert_eq!(voices.voices_active(), 1, "a four-byte loop wraps instead of ending the voice");

        let mut one_shot = minimal_mod();
        one_shot[48..50].copy_from_slice(&1u16.to_be_bytes());
        assert_eq!(load(&one_shot).expect("valid MOD").sample(starplayer_core::SampleId(0)).map(|sample| sample.loop_mode()), Some(LoopMode::None), "a one-word repeat is still a one-shot");
    }

    #[test]
    fn the_pattern_count_scan_ignores_order_bytes_at_or_above_0x80() {
        let mut with_garbage = minimal_mod();
        with_garbage[ORDER_OFFSET + 127] = 0x80;
        let mut clean = minimal_mod();
        clean[ORDER_OFFSET + 127] = 0x00;

        let garbage_module = load(&with_garbage).expect("a 0x80 tail entry is not a pattern reference");
        let clean_module = load(&clean).expect("valid MOD");
        assert!(garbage_module.pattern(starplayer_model::PatternId(0)).is_some());
        assert!(garbage_module.pattern(starplayer_model::PatternId(1)).is_none(), "0x80 never raises the stored pattern maximum");
        assert!(clean_module.pattern(starplayer_model::PatternId(1)).is_none());
        assert_eq!(garbage_module.sample_pcm(starplayer_core::SampleId(0)), clean_module.sample_pcm(starplayer_core::SampleId(0)));
        assert_eq!(garbage_module.sample_pcm(starplayer_core::SampleId(0)).and_then(|pcm| pcm.first()).copied(), Some(i16::MIN));
    }

    #[test]
    fn the_accepted_tag_set_covers_m_bang_k_bang_and_single_digit_chn() {
        let mut bang = minimal_mod();
        bang[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"M!K!");
        assert!(probe(&bang));
        assert_eq!(load(&bang).expect("M!K! is ProTracker's own >64-pattern tag").header().channel_count, 4);

        let mut five = vec![0; HEADER_BYTES + ROWS as usize * 5 * CELL_BYTES];
        five[SONG_LENGTH_OFFSET] = 1;
        five[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"5CHN");
        assert!(probe(&five));
        assert_eq!(load(&five).expect("single-digit CHN").header().channel_count, 5);

        let mut octalyser = minimal_mod();
        octalyser[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"CD61");
        assert!(!probe(&octalyser), "tracker dialects stay outside the tag set until C5 owns their behaviour");
    }

    #[test]
    fn a_zero_length_order_list_loads_and_plays_nothing() {
        let mut bytes = minimal_mod();
        bytes[SONG_LENGTH_OFFSET] = 0;
        let module = load(&bytes).expect("libxmp loads this and plays nothing");
        assert_eq!(module.order_entry(0), Some(starplayer_model::OrderEntry::End));
        assert_eq!(module.sample_pcm(starplayer_core::SampleId(0)).and_then(|pcm| pcm.first()).copied(), Some(i16::MIN), "the stored pattern still located the sample data");
    }
}

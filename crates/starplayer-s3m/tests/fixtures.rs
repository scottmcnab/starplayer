//! Loader tests against the two real Scream Tracker 3 modules in `tests/fixtures/`, plus
//! the synthetic headers that cover the cases no real file exercises.
//!
//! Every expected value in `EXPECTATIONS` was read out of the files with a Python hex
//! dump — there is no `openmpt123` on the machine this was written on — and is written
//! here as a literal so that a change in the loader shows up as a diff against the file's
//! bytes rather than against the loader's own opinion.

use starplayer_core::{I1F15, SampleId};
use starplayer_model::{ORDER_END, OrderEntry, PatternId};
use starplayer_s3m::{Error, PatternView, S3mCell, S3mFormatExtra, pan_nibble_to_bipolar};

const REFLEX: &[u8] = include_bytes!("fixtures/REFLEX.S3M");
const PETRI: &[u8] = include_bytes!("fixtures/PETRI.S3M");

/// What a hex dump of one fixture says its header holds.
struct Expectation {
    file_name: &'static str,
    bytes: &'static [u8],
    title: &'static str,
    channel_count: u8,
    order_count: usize,
    instrument_count: usize,
    sample_count: usize,
    pattern_count: usize,
    initial_speed: u8,
    initial_tempo: u16,
    global_volume: u8,
    master_volume_raw: u8,
    stereo: bool,
    amiga_limits: bool,
    tracker_version: u16,
    /// The pan nibbles the loader should derive, or `None` for a module whose pan table
    /// is empty because every channel is centred.
    pan_nibbles: Option<&'static [u8]>,
    /// The shortest prefix of the file that still loads: past the header tables, every
    /// sample *header* and every pattern's packed data, but not necessarily past any
    /// sample data, which the loader clamps rather than rejects.
    ///
    /// Read out of the hex dump as `max(tables_end, max sample-header end, max pattern
    /// end)`.
    structure_end: usize,
}

const EXPECTATIONS: &[Expectation] = &[
    Expectation {
        file_name: "REFLEX.S3M",
        bytes: REFLEX,
        title: "Reflex",
        channel_count: 3,
        order_count: 18,
        instrument_count: 7,
        sample_count: 4,
        pattern_count: 10,
        initial_speed: 8,
        initial_tempo: 125,
        global_volume: 64,
        master_volume_raw: 176,
        stereo: true,
        amiga_limits: true,
        tracker_version: 0x1320,
        // Header 0x35 == 252, and every entry of the block is 0x28: valid bit set, pan 8.
        pan_nibbles: Some(&[8, 8, 8]),
        structure_end: 7410,
    },
    Expectation {
        file_name: "PETRI.S3M",
        bytes: PETRI,
        title: "Petrified",
        channel_count: 8,
        order_count: 14,
        instrument_count: 6,
        sample_count: 5,
        pattern_count: 9,
        initial_speed: 3,
        initial_tempo: 140,
        global_volume: 64,
        master_volume_raw: 176,
        stereo: true,
        amiga_limits: false,
        tracker_version: 0x1301,
        // No pan block; channel settings 0,8,9,1,2,10,3,11 through `ClearChannels`.
        pan_nibbles: Some(&[3, 12, 12, 3, 3, 12, 3, 12]),
        structure_end: 3586,
    },
];

#[test]
fn every_fixture_loads_with_the_header_its_bytes_describe() {
    for expected in EXPECTATIONS {
        let module = starplayer_s3m::load(expected.bytes).unwrap_or_else(|error| panic!("{} should load: {error}", expected.file_name));
        let header = module.header();
        let name = expected.file_name;

        assert_eq!(header.title.as_ref(), expected.title, "{name} title");
        assert_eq!(header.channel_count, expected.channel_count, "{name} channel count");
        assert_eq!(module.orders().len(), expected.order_count, "{name} order count");
        assert_eq!(module.instruments().len(), expected.instrument_count, "{name} instrument count");
        assert_eq!(module.samples().len(), expected.sample_count, "{name} sample count");
        assert_eq!(module.patterns().len(), expected.pattern_count, "{name} pattern count");
        assert_eq!(header.initial_speed, expected.initial_speed, "{name} initial speed");
        assert_eq!(header.initial_tempo, expected.initial_tempo, "{name} initial tempo");
        assert_eq!(header.global_volume, starplayer_core::fixed::unit_from_ratio(expected.global_volume as u32, 64), "{name} global volume");
        assert_eq!(header.flags.stereo, expected.stereo, "{name} stereo flag");
        assert_eq!(header.flags.amiga_limits, expected.amiga_limits, "{name} Amiga limits");

        let extra = S3mFormatExtra::from_header(header);
        assert_eq!(extra.tracker_version, expected.tracker_version, "{name} Cwt/v");
        assert_eq!(extra.master_volume, expected.master_volume_raw, "{name} raw master volume");
        assert_eq!(extra.is_stereo(), expected.stereo, "{name} stereo bit inside format_extra");

        match expected.pan_nibbles {
            None => assert!(header.default_pan.is_empty(), "{name} should be centred everywhere"),
            Some(nibbles) => {
                let expected_pan: Vec<I1F15> = nibbles.iter().map(|nibble| pan_nibble_to_bipolar(*nibble)).collect();
                assert_eq!(header.default_pan.as_ref(), expected_pan.as_slice(), "{name} default pan");
            }
        }
    }
}

#[test]
fn every_fixtures_patterns_are_sixty_four_rows_of_whole_cells() {
    for expected in EXPECTATIONS {
        let module = starplayer_s3m::load(expected.bytes).expect("the fixture loads");
        for index in 0..module.patterns().len() {
            let id = PatternId(index as u16);
            let view = PatternView::new(&module, id).unwrap_or_else(|| panic!("{} pattern {index} has a view", expected.file_name));

            assert_eq!(view.rows(), 64, "{} pattern {index} rows", expected.file_name);
            assert_eq!(view.channels(), expected.channel_count, "{} pattern {index} channels", expected.file_name);
            assert_eq!(view.cells().len(), 64 * expected.channel_count as usize * 5);
            assert!(view.cell(63, expected.channel_count - 1).is_some());
            assert!(view.cell(64, 0).is_none());
        }
    }
}

/// The first four rows of `REFLEX.S3M`'s pattern 0, decoded by hand from the packed bytes.
///
/// The pattern's parapointer is 0x2F, so it starts at 0x2F0 = 752; its packed length word
/// says 878, which **includes its own two bytes**, so the stream is 876 bytes starting at
/// 754. The first sixteen of them are
///
/// ```text
/// E0 40 01 01 08 71   channel 0: note+instrument, volume, command
/// E1 47 01 01 08 71   channel 1: the same, a fifth up
/// E2 47 04 01 01 0A   channel 2: instrument 4, command A (set speed) 0x0A
/// 00                  end of row 0
/// C0 01 08 00         channel 0: volume 1, command H (vibrato) 0x00
/// ```
#[test]
fn reflex_pattern_zero_decodes_to_the_cells_its_packed_bytes_spell() {
    let module = starplayer_s3m::load(REFLEX).expect("REFLEX.S3M loads");
    let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");

    // 0xE0: mask bits 5, 6 and 7 -> note 0x40 (C-4), instrument 1, volume 1, command 8
    // (`H`, vibrato) with parameter 0x71.
    assert_eq!(view.cell(0, 0), Some(S3mCell { note: 0x40, instrument: 1, volume: 1, command: 8, info: 0x71 }));
    assert_eq!(view.cell(0, 1), Some(S3mCell { note: 0x47, instrument: 1, volume: 1, command: 8, info: 0x71 }));
    assert_eq!(view.cell(0, 2), Some(S3mCell { note: 0x47, instrument: 4, volume: 1, command: 1, info: 0x0A }));

    // 0xC0: volume and command only, so the note and instrument columns stay empty.
    assert_eq!(view.cell(1, 0), Some(S3mCell { note: 255, instrument: 0, volume: 1, command: 8, info: 0x00 }));
    assert_eq!(view.cell(1, 2), Some(S3mCell { note: 255, instrument: 0, volume: 2, command: 8, info: 0x81 }));
    assert_eq!(view.cell(2, 2), Some(S3mCell { note: 255, instrument: 0, volume: 3, command: 8, info: 0x82 }));
    assert_eq!(view.cell(3, 2), Some(S3mCell { note: 255, instrument: 0, volume: 4, command: 8, info: 0x81 }));

    // The note columns say what the display view spells out.
    let display = view.cell(0, 0).expect("cell 0,0").display();
    assert_eq!(display.effect.map(|effect| effect.name), Some("vibrato"));
    assert_eq!(view.cell(0, 2).expect("cell 0,2").display().effect.map(|effect| effect.name), Some("change speed"));
}

#[test]
fn reflexs_first_sample_carries_its_loop_volume_and_c2spd() {
    let module = starplayer_s3m::load(REFLEX).expect("REFLEX.S3M loads");
    let sample = module.sample(SampleId(0)).expect("sample 0 exists");

    assert_eq!(sample.name(), "This module started almost");
    assert_eq!(sample.length_frames(), 34, "a forward loop stores exactly loop_end frames");
    assert_eq!(sample.loop_start(), 2);
    assert_eq!(sample.loop_end(), 34);
    assert_eq!(sample.reference_rate_hz(), 8363);
    assert_eq!(module.sample_pcm(SampleId(0)).map(<[i16]>::len), Some(34 + starplayer_core::GUARD_FRAMES));

    // Three of REFLEX's seven instrument slots are message-only (`type` 0): they become
    // instruments with no sample, so instrument numbering still matches the file.
    assert_eq!(module.instruments().len(), 7);
    assert_eq!(module.instruments()[6].sample, None);
    assert_eq!(module.instruments()[6].name.as_ref(), "  jedi@tartarus.uwa.edu.au");
}

#[test]
fn a_sample_name_with_a_high_byte_is_read_as_code_page_437() {
    let module = starplayer_s3m::load(PETRI).expect("PETRI.S3M loads");
    // `tclosedh.sam (no\xFFheader)` — the byte is 0xFF, not UTF-8.
    assert_eq!(module.sample(SampleId(1)).expect("sample 1").name(), "tclosedh.sam (no\u{a0}header)");
}

#[test]
fn every_fixtures_orders_resolve_to_a_pattern_or_to_the_end_of_the_song() {
    for expected in EXPECTATIONS {
        let module = starplayer_s3m::load(expected.bytes).expect("the fixture loads");
        for position in 0..module.orders().len() {
            let entry = module.order_entry(position).expect("every position is in range");
            if let OrderEntry::Pattern(PatternId(pattern)) = entry {
                assert!((pattern as usize) < module.patterns().len(), "{} order {position}", expected.file_name);
            }
        }
    }
    let module = starplayer_s3m::load(REFLEX).expect("REFLEX.S3M loads");
    assert_eq!(module.orders().last(), Some(&ORDER_END), "REFLEX's order list ends with 255");
    assert_eq!(module.order_entry(17), Some(OrderEntry::End));
}

/// Every cell of every pattern and every sample's frames are reachable — what has to hold
/// of any module the loader hands back, however mangled the file behind it was.
fn assert_module_is_sound(module: &starplayer_model::Module, what: &str) {
    assert!(module.header().channel_count > 0, "{what}: a module has channels");
    for index in 0..module.patterns().len() {
        let view = PatternView::new(module, PatternId(index as u16)).unwrap_or_else(|| panic!("{what}: pattern {index} has a view"));
        for row in 0..view.rows() {
            for channel in 0..view.channels() {
                assert!(view.cell(row, channel).is_some(), "{what}: pattern {index} cell {row},{channel}");
            }
        }
    }
    for index in 0..module.samples().len() {
        let id = SampleId(index as u16);
        let sample = module.sample(id).unwrap_or_else(|| panic!("{what}: sample {index} exists"));
        let pcm = module.sample_pcm(id).unwrap_or_else(|| panic!("{what}: sample {index} has frames"));
        assert_eq!(pcm.len(), sample.readable_frames(), "{what}: sample {index} frame count");
    }
}

/// Truncation sweep: at every 64-byte boundary, the loader either refuses the file or
/// returns a module that is internally sound. It never panics, and where it draws the
/// line is not a matter of luck — it is `structure_end`.
///
/// A prefix shorter than `structure_end` is missing a header table, a sample header or a
/// pattern, and is rejected. A longer one is missing only sample data, which the loader
/// clamps to whatever is there (see the clamp-or-reject table), so it loads.
#[test]
fn truncating_a_fixture_at_every_sixty_four_byte_boundary_never_panics_and_errs_where_it_should() {
    for expected in EXPECTATIONS {
        let mut truncation = 0usize;
        while truncation <= expected.bytes.len() {
            let what = format!("{} truncated to {truncation} bytes", expected.file_name);
            match starplayer_s3m::load(&expected.bytes[..truncation]) {
                Err(_) => assert!(truncation < expected.structure_end, "{what} should have loaded"),
                Ok(module) => {
                    assert!(truncation >= expected.structure_end, "{what} should not have loaded");
                    assert_module_is_sound(&module, &what);
                }
            }
            truncation += 64;
        }
        // The untruncated file still loads, so the sweep above was not vacuous.
        assert!(starplayer_s3m::load(expected.bytes).is_ok(), "{} loads whole", expected.file_name);
    }
}

#[test]
fn truncating_a_fixture_at_every_single_byte_still_never_panics() {
    // The 64-byte sweep is the task's requirement; this one walks the smallest fixture a
    // byte at a time, so no boundary inside the header, the tables or a pattern is missed.
    for truncation in 0..=REFLEX.len() {
        let what = format!("REFLEX truncated to {truncation} bytes");
        match starplayer_s3m::load(&REFLEX[..truncation]) {
            Err(_) => assert!(truncation < 7410, "{what} should have loaded"),
            Ok(module) => assert_module_is_sound(&module, &what),
        }
    }
}

#[test]
fn a_module_that_is_not_an_s3m_is_refused_by_probe_and_by_load() {
    assert!(starplayer_s3m::probe(REFLEX));
    assert!(!starplayer_s3m::probe(b"not a module at all, not even close"));

    let mut corrupted = REFLEX.to_vec();
    corrupted[0x2C] = b'X';
    assert!(!starplayer_s3m::probe(&corrupted));
    assert_eq!(starplayer_s3m::load(&corrupted), Err(Error::BadMagic));
}

// ── synthetic modules ───────────────────────────────────────────────────────────────
//
// The fixtures are two well-formed files by one author; these cover what they cannot.

/// Offsets the synthetic file below places its parapointed blocks at.
const SYNTHETIC_SAMPLE_HEADER: usize = 16 * 16;
const SYNTHETIC_PATTERN: usize = 32 * 16;
const SYNTHETIC_SAMPLE_DATA: usize = 48 * 16;
const SYNTHETIC_LENGTH: usize = SYNTHETIC_SAMPLE_DATA + 16;

/// The four unsigned sample bytes the synthetic module carries, and what they must widen
/// to: minimum, silence, near-maximum, and a quarter-scale negative.
const SYNTHETIC_PCM: [u8; 4] = [0x00, 0x80, 0xFF, 0x40];
const SYNTHETIC_PCM_WIDENED: [i16; 4] = [i16::MIN, 0, 32512, -16384];

/// A one-pattern, one-sample S3M built by hand, so the panning rules and the sample
/// widening can be tested on inputs no real file provides.
fn synthetic_s3m(stereo: bool, channel_settings: &[u8], pan_block: Option<[u8; 32]>) -> Vec<u8> {
    let mut bytes = vec![0u8; SYNTHETIC_LENGTH];

    bytes[..8].copy_from_slice(b"Synth\0\0\0");
    bytes[0x1C] = 0x1A; // the format's end-of-file marker byte
    bytes[0x20..0x22].copy_from_slice(&1u16.to_le_bytes()); // Ordnum
    bytes[0x22..0x24].copy_from_slice(&1u16.to_le_bytes()); // Insnum
    bytes[0x24..0x26].copy_from_slice(&1u16.to_le_bytes()); // Patnum
    bytes[0x26..0x28].copy_from_slice(&0u16.to_le_bytes()); // generalflags
    bytes[0x28..0x2A].copy_from_slice(&0x1320u16.to_le_bytes()); // Cwt/v
    bytes[0x2A..0x2C].copy_from_slice(&2u16.to_le_bytes()); // ffi: unsigned samples
    bytes[0x2C..0x30].copy_from_slice(b"SCRM");
    bytes[0x30] = 64; // globalvol
    bytes[0x31] = 6; // initialspd
    bytes[0x32] = 125; // initialBPM
    bytes[0x33] = if stereo { 0x80 | 48 } else { 48 };
    bytes[0x35] = if pan_block.is_some() { 252 } else { 0 };
    bytes[0x40..0x60].fill(255);
    bytes[0x40..0x40 + channel_settings.len()].copy_from_slice(channel_settings);

    bytes[0x60] = 0; // the single order plays pattern 0
    bytes[0x61..0x63].copy_from_slice(&((SYNTHETIC_SAMPLE_HEADER / 16) as u16).to_le_bytes());
    bytes[0x63..0x65].copy_from_slice(&((SYNTHETIC_PATTERN / 16) as u16).to_le_bytes());
    if let Some(block) = pan_block {
        bytes[0x65..0x85].copy_from_slice(&block);
    }

    let sample = SYNTHETIC_SAMPLE_HEADER;
    bytes[sample] = 1; // type: PCM
    bytes[sample + 0x0E..sample + 0x10].copy_from_slice(&((SYNTHETIC_SAMPLE_DATA / 16) as u16).to_le_bytes());
    bytes[sample + 0x10..sample + 0x14].copy_from_slice(&(SYNTHETIC_PCM.len() as u32).to_le_bytes());
    bytes[sample + 0x1C] = 64; // vol
    bytes[sample + 0x20..sample + 0x24].copy_from_slice(&8363u32.to_le_bytes());
    bytes[sample + 0x30..sample + 0x36].copy_from_slice(b"synth\0");
    bytes[sample + 0x4C..sample + 0x50].copy_from_slice(b"SCRS");

    // One event on channel 0 of row 0, then 64 row terminators. The packed length counts
    // its own two bytes.
    let mut packed = vec![0xE0u8, 0x40, 0x01, 0x20, 0x01, 0x0F, 0x00];
    packed.extend(core::iter::repeat_n(0u8, 63));
    bytes[SYNTHETIC_PATTERN..SYNTHETIC_PATTERN + 2].copy_from_slice(&((packed.len() + 2) as u16).to_le_bytes());
    bytes[SYNTHETIC_PATTERN + 2..SYNTHETIC_PATTERN + 2 + packed.len()].copy_from_slice(&packed);

    bytes[SYNTHETIC_SAMPLE_DATA..SYNTHETIC_SAMPLE_DATA + SYNTHETIC_PCM.len()].copy_from_slice(&SYNTHETIC_PCM);
    bytes
}

#[test]
fn the_synthetic_module_is_a_module_the_loader_accepts() {
    let module = starplayer_s3m::load(&synthetic_s3m(true, &[0, 8], None)).expect("the synthetic module loads");

    assert_eq!(module.header().title.as_ref(), "Synth");
    assert_eq!(module.header().channel_count, 2);
    assert_eq!(module.patterns().len(), 1);
    assert_eq!(module.samples().len(), 1);

    let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");
    assert_eq!(view.cell(0, 0), Some(S3mCell { note: 0x40, instrument: 1, volume: 0x20, command: 1, info: 0x0F }));
    assert_eq!(view.cell(0, 1), Some(S3mCell::EMPTY));
}

#[test]
fn unsigned_sample_bytes_widen_with_zero_at_the_minimum_and_0x80_at_silence() {
    let module = starplayer_s3m::load(&synthetic_s3m(true, &[0, 8], None)).expect("the synthetic module loads");
    let pcm = module.sample_pcm(SampleId(0)).expect("the sample's frames");

    assert_eq!(&pcm[..4], &SYNTHETIC_PCM_WIDENED);
    assert_eq!(&pcm[4..], &[0i16; starplayer_core::GUARD_FRAMES], "a one-shot's guard frames are silence");
}

#[test]
fn a_stereo_module_without_a_pan_block_pans_by_its_channel_settings() {
    // Settings 0 and 1 are below 8 (left); 8 and 9 are not (right); 255 is disabled.
    let module = starplayer_s3m::load(&synthetic_s3m(true, &[0, 8, 1, 9], None)).expect("the synthetic module loads");

    let expected: Vec<I1F15> = [3u8, 12, 3, 12].iter().map(|nibble| pan_nibble_to_bipolar(*nibble)).collect();
    assert_eq!(module.header().default_pan.as_ref(), expected.as_slice());
    assert!(module.header().flags.stereo);
}

/// A full 16-entry channel-settings table, the Amiga interleave a 16-channel module
/// carries: settings alternate below 8 and at-or-above 8, so the derived pan nibbles
/// alternate hard left (3) and hard right (12) all sixteen times. No real fixture
/// declares sixteen channels, so only this synthetic header covers the case.
#[test]
fn a_sixteen_channel_settings_table_derives_sixteen_alternating_pan_nibbles() {
    let settings = [0u8, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15];
    let module = starplayer_s3m::load(&synthetic_s3m(true, &settings, None)).expect("the synthetic module loads");

    assert_eq!(module.header().channel_count, 16, "sixteen enabled channel settings mean sixteen channels");
    let nibbles = [3u8, 12, 3, 12, 3, 12, 3, 12, 3, 12, 3, 12, 3, 12, 3, 12];
    let expected: Vec<I1F15> = nibbles.iter().map(|nibble| pan_nibble_to_bipolar(*nibble)).collect();
    assert_eq!(module.header().default_pan.as_ref(), expected.as_slice());
}

#[test]
fn a_stereo_module_with_a_pan_block_takes_the_low_nibble_only_where_bit_five_is_set() {
    let mut block = [0u8; 32];
    // 0x25 -> pan 5. 0x0C -> bit 5 clear, so channel 1 keeps the derived hard right.
    // 0x2F -> pan 15. 0x20 -> pan 0.
    block[..4].copy_from_slice(&[0x25, 0x0C, 0x2F, 0x20]);
    let module = starplayer_s3m::load(&synthetic_s3m(true, &[0, 8, 1, 9], Some(block))).expect("the synthetic module loads");

    let expected: Vec<I1F15> = [5u8, 12, 15, 0].iter().map(|nibble| pan_nibble_to_bipolar(*nibble)).collect();
    assert_eq!(module.header().default_pan.as_ref(), expected.as_slice());
}

#[test]
fn a_mono_module_gets_the_empty_pan_table_that_means_centred() {
    let module = starplayer_s3m::load(&synthetic_s3m(false, &[0, 8, 1, 9], None)).expect("the synthetic module loads");

    assert!(module.header().default_pan.is_empty());
    assert!(!module.header().flags.stereo);
    assert_eq!(module.header().channel_pan(0), Some(I1F15::ZERO));
}

#[test]
fn a_mono_module_with_a_pan_block_still_takes_the_block_because_loadpansettings_does() {
    let mut block = [0u8; 32];
    block[..2].copy_from_slice(&[0x20, 0x2F]);
    let module = starplayer_s3m::load(&synthetic_s3m(false, &[0, 8], Some(block))).expect("the synthetic module loads");

    let expected: Vec<I1F15> = [0u8, 15].iter().map(|nibble| pan_nibble_to_bipolar(*nibble)).collect();
    assert_eq!(module.header().default_pan.as_ref(), expected.as_slice());
}

#[test]
fn a_module_with_no_enabled_channel_is_invalid() {
    let module = starplayer_s3m::load(&synthetic_s3m(true, &[], None));
    assert_eq!(module, Err(Error::Invalid("no enabled channels in the channel-settings array")));
}

#[test]
fn a_pan_block_the_file_promises_but_does_not_hold_is_truncated() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[0x35] = 252;
    bytes.truncate(0x70);
    assert!(matches!(starplayer_s3m::load(&bytes), Err(Error::Truncated { .. })));
}

#[test]
fn an_instrument_count_the_file_cannot_hold_is_truncated() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[0x22..0x24].copy_from_slice(&40_000u16.to_le_bytes());
    assert!(matches!(starplayer_s3m::load(&bytes), Err(Error::Truncated { .. })));
}

#[test]
fn a_pattern_whose_packed_length_overruns_the_file_is_truncated() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_PATTERN..SYNTHETIC_PATTERN + 2].copy_from_slice(&40_000u16.to_le_bytes());
    assert!(matches!(starplayer_s3m::load(&bytes), Err(Error::Truncated { .. })));
}

#[test]
fn a_pattern_parapointer_of_zero_is_an_empty_pattern() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[0x63..0x65].copy_from_slice(&0u16.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("an empty pattern is not an error");

    let view = PatternView::new(&module, PatternId(0)).expect("pattern 0 exists");
    assert_eq!(view.cell(0, 0), Some(S3mCell::EMPTY));
    assert_eq!(view.rows(), 64);
}

#[test]
fn a_sample_whose_data_lies_past_the_end_of_the_file_loads_as_an_empty_sample() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x0E..SYNTHETIC_SAMPLE_HEADER + 0x10].copy_from_slice(&9_000u16.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("a sample past EOF is not an error");

    assert_eq!(module.sample(SampleId(0)).expect("sample 0").length_frames(), 0);
    assert_eq!(module.instruments()[0].sample, Some(SampleId(0)), "the instrument still names it");
}

#[test]
fn a_sample_whose_data_runs_off_the_end_is_clamped_to_what_is_there() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x10..SYNTHETIC_SAMPLE_HEADER + 0x14].copy_from_slice(&4_000u32.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("a sample running off the end is not an error");

    let available = (SYNTHETIC_LENGTH - SYNTHETIC_SAMPLE_DATA) as u32;
    assert_eq!(module.sample(SampleId(0)).expect("sample 0").length_frames(), available);
}

#[test]
fn a_loop_end_past_the_sample_is_clamped_and_an_empty_loop_switches_looping_off() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x1F] = 1; // loop
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x14..SYNTHETIC_SAMPLE_HEADER + 0x18].copy_from_slice(&1u32.to_le_bytes());
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x18..SYNTHETIC_SAMPLE_HEADER + 0x1C].copy_from_slice(&999u32.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("a runaway loop end is clamped, not rejected");
    let sample = module.sample(SampleId(0)).expect("sample 0");

    assert_eq!(sample.loop_end(), SYNTHETIC_PCM.len() as u32);
    assert_eq!(sample.loop_start(), 1);

    // Now a loop that starts at or after where it ends: no loop at all.
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x14..SYNTHETIC_SAMPLE_HEADER + 0x18].copy_from_slice(&4u32.to_le_bytes());
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x18..SYNTHETIC_SAMPLE_HEADER + 0x1C].copy_from_slice(&4u32.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("an empty loop is not an error");
    assert!(!module.sample(SampleId(0)).expect("sample 0").loop_mode().is_looping());
}

#[test]
fn a_zero_c2spd_falls_back_to_the_format_default_rather_than_failing_the_builder() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x20..SYNTHETIC_SAMPLE_HEADER + 0x24].copy_from_slice(&0u32.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("a zero C2SPD is clamped, not rejected");

    assert_eq!(module.sample(SampleId(0)).expect("sample 0").reference_rate_hz(), 8363);
}

#[test]
fn the_full_thirty_two_bit_c2spd_survives_the_load_which_is_deviation_d7() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x20..SYNTHETIC_SAMPLE_HEADER + 0x24].copy_from_slice(&96_000u32.to_le_bytes());
    let module = starplayer_s3m::load(&bytes).expect("the synthetic module loads");

    assert_eq!(module.sample(SampleId(0)).expect("sample 0").reference_rate_hz(), 96_000);
    assert_ne!(module.sample(SampleId(0)).expect("sample 0").reference_rate_hz(), 96_000u32 & 0xFFFF);
}

#[test]
fn an_adlib_instrument_becomes_an_instrument_with_no_sample() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER] = 2; // Adlib melody instrument
    let module = starplayer_s3m::load(&bytes).expect("an Adlib instrument is not an error");

    assert_eq!(module.instruments().len(), 1);
    assert_eq!(module.instruments()[0].sample, None);
    assert_eq!(module.samples().len(), 0);
}

#[test]
fn packed_sample_data_is_reported_as_unsupported() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[SYNTHETIC_SAMPLE_HEADER + 0x1E] = 1; // DP30ADPCM
    assert_eq!(starplayer_s3m::load(&bytes), Err(Error::Unsupported("packed S3M sample data")));
}

#[test]
fn an_order_naming_a_pattern_that_does_not_exist_becomes_a_skip_marker() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[0x60] = 42; // the module has one pattern
    let module = starplayer_s3m::load(&bytes).expect("a stray order is skipped, not rejected");

    assert_eq!(module.order_entry(0), Some(OrderEntry::Marker));
}

#[test]
fn a_zero_speed_and_an_impossible_tempo_are_clamped_to_playable_values() {
    let mut bytes = synthetic_s3m(true, &[0, 8], None);
    bytes[0x31] = 0;
    bytes[0x32] = 5;
    let module = starplayer_s3m::load(&bytes).expect("clamped, not rejected");

    assert_eq!(module.header().initial_speed, 6);
    assert_eq!(module.header().initial_tempo, 32);
}

/// Loads every S3M in the repository owner's own collection, which lives outside the
/// repository — so this test is ignored by default and is not part of `cargo xtask ci`.
///
/// Run it with
/// `cargo test -p starplayer-s3m -- --ignored --nocapture`.
///
/// It asserts only that nothing panics and that whatever comes back is either a module or
/// one of the documented errors; the point is coverage over 29 real files, not fixed
/// expectations for files the repository does not hold.
#[test]
#[ignore = "reads the owner's module collection from outside the repository"]
fn loads_every_module_in_the_owners_collection() {
    let directory = std::path::Path::new("/mnt/c/Users/scott/Documents/Projects/music/Scott");
    if !directory.is_dir() {
        eprintln!("skipped: {} is not present on this machine", directory.display());
        return;
    }

    let mut loaded = 0usize;
    let mut refused = 0usize;
    for entry in std::fs::read_dir(directory).expect("the collection directory is readable") {
        let path = entry.expect("a directory entry").path();
        if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("s3m")) {
            continue;
        }
        let bytes = std::fs::read(&path).expect("the module is readable");
        match starplayer_s3m::load(&bytes) {
            Ok(module) => {
                loaded += 1;
                assert!(module.header().channel_count > 0, "{} has channels", path.display());
                for index in 0..module.patterns().len() {
                    let view = PatternView::new(&module, PatternId(index as u16)).expect("every pattern has a view");
                    for row in 0..view.rows() {
                        for channel in 0..view.channels() {
                            assert!(view.cell(row, channel).is_some());
                        }
                    }
                }
                println!("{:>14}  {:>2} ch  {:>3} pat  {:>3} ins  \"{}\"", path.file_name().and_then(|name| name.to_str()).unwrap_or(""), module.header().channel_count, module.patterns().len(), module.instruments().len(), module.header().title);
            }
            Err(error) => {
                refused += 1;
                println!("{:>14}  refused: {error}", path.file_name().and_then(|name| name.to_str()).unwrap_or(""));
            }
        }
    }
    println!("{loaded} loaded, {refused} refused");
    assert!(loaded > 0, "the collection should contain loadable modules");
}

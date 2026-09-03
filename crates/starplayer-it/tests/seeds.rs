//! The committed fuzz seeds, and the feature each one exists to pin.
//!
//! `crates/starplayer-offline/tests/fuzz_seeds.rs` already asserts that every seed still
//! *loads*. This file asserts that each one still loads as the thing it was synthesised to
//! be, so a seed cannot quietly rot into a file that exercises a different path — a
//! compressed sample that decodes to nothing, say, would still "load".

use starplayer_core::{InstrumentId, SampleId};
use starplayer_it::{ItFormatData, ItFormatExtra, MAX_ROWS};
use starplayer_model::{FormatDialect, LoopMode, PatternId};

const MINIMAL: &[u8] = include_bytes!("../../../fuzz/seeds/it/minimal.it");
const INSTRUMENT_MODE: &[u8] = include_bytes!("../../../fuzz/seeds/it/instrument-mode.it");
const COMPRESSED_8BIT: &[u8] = include_bytes!("../../../fuzz/seeds/it/compressed-8bit.it");
const COMPRESSED_16BIT: &[u8] = include_bytes!("../../../fuzz/seeds/it/compressed-16bit.it");
const SUSTAIN_PING_PONG: &[u8] = include_bytes!("../../../fuzz/seeds/it/sustain-ping-pong.it");
const MIDI_MACROS: &[u8] = include_bytes!("../../../fuzz/seeds/it/midi-macros.it");
const MAXIMUM_COUNTS: &[u8] = include_bytes!("../../../fuzz/seeds/it/maximum-counts.it");
const OLD_INSTRUMENTS: &[u8] = include_bytes!("../../../fuzz/seeds/it/old-instruments.it");

const SEEDS: [(&str, &[u8]); 8] = [
    ("minimal.it", MINIMAL),
    ("instrument-mode.it", INSTRUMENT_MODE),
    ("compressed-8bit.it", COMPRESSED_8BIT),
    ("compressed-16bit.it", COMPRESSED_16BIT),
    ("sustain-ping-pong.it", SUSTAIN_PING_PONG),
    ("midi-macros.it", MIDI_MACROS),
    ("maximum-counts.it", MAXIMUM_COUNTS),
    ("old-instruments.it", OLD_INSTRUMENTS),
];

#[test]
fn every_seed_probes_and_loads() {
    for (name, bytes) in SEEDS {
        assert!(starplayer_it::probe(bytes), "{name} must probe as an IT");
        let module = starplayer_it::load(bytes);
        assert!(module.is_ok(), "{name} must load: {:?}", module.err());
        assert_eq!(module.map(|module| module.header().dialect), Ok(FormatDialect::ImpulseTracker), "{name}");
    }
}

#[test]
fn the_minimal_seed_is_a_sample_mode_module_with_one_pattern() {
    let module = starplayer_it::load(MINIMAL).expect("minimal.it loads");
    let extra = ItFormatExtra::from_header(module.header());

    assert!(!extra.is_instrument_mode());
    assert_eq!(module.samples().len(), 1);
    assert_eq!(module.instruments().len(), 1, "sample mode synthesises one instrument per sample");
    assert_eq!(module.instrument(InstrumentId(0)).and_then(|instrument| instrument.sample), Some(SampleId(0)));
    assert_eq!(module.patterns().len(), 1);
    assert_eq!(module.header().channel_count, 2);
}

#[test]
fn the_instrument_mode_seed_carries_all_three_envelopes_and_a_note_fade_nna() {
    use starplayer_model::{DuplicateAction, DuplicateCheck, NewNoteAction};

    let module = starplayer_it::load(INSTRUMENT_MODE).expect("instrument-mode.it loads");
    let instrument = module.instrument(InstrumentId(0)).expect("instrument 0 exists");

    assert!(ItFormatExtra::from_header(module.header()).is_instrument_mode());
    assert!(module.header().flags.linear_slides);
    assert_eq!(instrument.new_note_action, NewNoteAction::NoteFade);
    assert_eq!(instrument.duplicate_check, DuplicateCheck::Note);
    assert_eq!(instrument.duplicate_action, DuplicateAction::NoteFade);
    assert_eq!(instrument.fadeout, 512);
    assert!(instrument.volume_envelope.as_ref().is_some_and(|envelope| envelope.carry && envelope.loop_span.is_some()));
    assert!(instrument.panning_envelope.as_ref().is_some_and(|envelope| envelope.sustain.is_some()));
    assert!(instrument.pitch_envelope.is_some() && instrument.pitch_envelope_is_filter);
    assert_eq!(instrument.initial_filter_cutoff, Some(100));
    assert_eq!(instrument.note_sample_map[60], 1);
}

#[test]
fn the_compressed_seeds_decode_to_the_lengths_their_headers_claim() {
    let eight = starplayer_it::load(COMPRESSED_8BIT).expect("compressed-8bit.it loads");
    let eight_sample = eight.sample(SampleId(0)).expect("sample 0 exists");
    assert_eq!(eight_sample.length_frames(), 40, "the whole compressed block decodes");
    assert!(eight.pcm().iter().any(|frame| *frame != 0), "the decompressed data is not silence");

    let sixteen = starplayer_it::load(COMPRESSED_16BIT).expect("compressed-16bit.it loads");
    let sixteen_sample = sixteen.sample(SampleId(0)).expect("sample 0 exists");
    assert_eq!(sixteen_sample.length_frames(), 64);
    assert!(sixteen.pcm().iter().any(|frame| *frame != 0));
}

#[test]
fn the_sustain_seed_keeps_both_ping_pong_loops() {
    use starplayer_model::SustainLoop;

    let module = starplayer_it::load(SUSTAIN_PING_PONG).expect("sustain-ping-pong.it loads");
    let sample = module.sample(SampleId(0)).expect("sample 0 exists");

    assert_eq!(sample.loop_mode(), LoopMode::PingPong);
    assert_eq!((sample.loop_start(), sample.loop_end()), (8, 40));
    assert_eq!(sample.sustain_loop(), Some(SustainLoop { mode: LoopMode::PingPong, start: 40, end: 64 }));
    assert_eq!(sample.length_frames(), 64, "a sustain loop keeps the whole sample");
}

#[test]
fn the_midi_seed_carries_its_macro_block_into_format_data() {
    let module = starplayer_it::load(MIDI_MACROS).expect("midi-macros.it loads");
    let data = ItFormatData::from_header(module.header()).expect("the block is there");

    assert!(ItFormatExtra::from_header(module.header()).has_midi_configuration);
    assert_eq!(data.global_macro(0).map(|bytes| &bytes[..8]), Some(&b"F0F00001"[..]));
    assert_eq!(data.parametered_macro(0).map(|bytes| &bytes[..6]), Some(&b"F0F001"[..]));
    assert_eq!(data.fixed_macro(0).map(|bytes| &bytes[..6]), Some(&b"F0F002"[..]));
}

#[test]
fn the_maximum_counts_seed_is_the_widest_and_longest_shape_the_loader_accepts() {
    let module = starplayer_it::load(MAXIMUM_COUNTS).expect("maximum-counts.it loads");

    assert_eq!(module.header().channel_count, 64);
    assert_eq!(module.patterns().len(), 4);
    assert_eq!(module.pattern(PatternId(0)).map(|index| index.rows()), Some(200));
    assert!(module.pattern(PatternId(0)).is_some_and(|index| index.rows() <= MAX_ROWS));
    assert_eq!(module.samples().len(), 4);
    assert_eq!(module.orders().len(), 103);
}

#[test]
fn the_old_instrument_seed_goes_through_the_pre_two_hundred_layout() {
    let module = starplayer_it::load(OLD_INSTRUMENTS).expect("old-instruments.it loads");
    let instrument = module.instrument(InstrumentId(0)).expect("instrument 0 exists");

    assert!(ItFormatExtra::from_header(module.header()).old_instruments);
    assert_eq!(instrument.fadeout, 64, "the pre-2.00 fadeout scale doubles into the new one");
    assert_eq!(instrument.volume_envelope.as_ref().map(|envelope| envelope.points.len()), Some(3));
    assert_eq!(instrument.panning_envelope, None, "the pre-2.00 layout has one envelope");
    assert_eq!(instrument.note_sample_map[60], 1);
}

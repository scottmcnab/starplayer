//! Synthetic regressions for the signatureless legacy layout; no library music is embedded.

use starplayer_core::{ExactFixedPoint, Frame, RowClock, SampleId, quirks::{FormatDialect, ModTiming}};
use starplayer_dsp::Linear;
use starplayer_engine::{ChannelTable, Engine, EngineSettings, RowRef, SongPosition, TickContext, TrackerProcessor};
use starplayer_mixer::{FixedPath, StereoI16, VoicePool};
use starplayer_model::{DecodeBudget, ImageDecodeStatus, LoopMode, Module, ModuleReader, PatternId};
use starplayer_mod::{ModCell, ModProcessor, soundtracker};
use starplayer_rt::Arc;

fn fixture(effect: u8, param: u8) -> Vec<u8> {
    let mut bytes = vec![0; 600 + 2 * 1024 + 16];
    bytes[..6].copy_from_slice(b"legacy");
    bytes[20..25].copy_from_slice(b"first");
    bytes[42..44].copy_from_slice(&6u16.to_be_bytes());
    bytes[45] = 64;
    bytes[46..48].copy_from_slice(&2u16.to_be_bytes()); // two bytes, not two words
    bytes[48..50].copy_from_slice(&4u16.to_be_bytes()); // eight-byte loop
    bytes[50..56].copy_from_slice(b"second");
    bytes[72..74].copy_from_slice(&2u16.to_be_bytes());
    bytes[75] = 32;
    bytes[470] = 1;
    bytes[471] = 120;
    bytes[472 + 127] = 1; // inactive stored pattern still precedes the PCM
    bytes[600..604].copy_from_slice(&ModCell { period: 428, instrument: 1, effect, param }.to_bytes());
    bytes[600 + 2 * 1024..].copy_from_slice(&[11, 22, 0x80, 0x7f, 0x40, 0xc0, 0, 0x10, 0xf0, 0x20, 0xe0, 0, 0x81, 2, 3, 4]);
    bytes
}

struct CopyReader<'a>(&'a [u8]);
impl ModuleReader for CopyReader<'_> {
    fn len(&self) -> usize { self.0.len() }
    fn read_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), starplayer_core::Error> { self.0.read_at(offset, buffer) }
}

#[test]
fn legacy_layout_skips_byte_unit_attack_and_keeps_following_sample_aligned() {
    let bytes = fixture(0xF, 6);
    assert!(soundtracker::probe(&bytes));
    assert!(soundtracker::probe_reader(&CopyReader(&bytes)));
    assert!(!starplayer_mod::probe(&bytes), "the tagged MOD entry point remains explicit");
    let module = soundtracker::load(&bytes).expect("Soundtracker");
    assert_eq!(module.to_image(), soundtracker::load_from(&CopyReader(&bytes)).expect("copying reader").to_image());
    assert_eq!(module.header().dialect, FormatDialect::Soundtracker);
    assert_eq!(module.header().channel_count, 4);
    assert_eq!(module.header().initial_tempo, 125);
    assert_eq!(module.header().dialect.quirks().mod_timing, ModTiming::VBlank);
    assert_eq!(starplayer_mod::timing_verdict_for(&module), Some(starplayer_mod::TimingVerdict::VBlank));
    assert_eq!(module.pattern_bytes(PatternId(0)).map(|pattern| &pattern[..4]), Some(&bytes[600..604]));
    assert!(module.pattern(PatternId(1)).is_some());
    assert!(module.sample(SampleId(14)).is_some());
    assert!(module.sample(SampleId(15)).is_none());
    let sample = module.sample(SampleId(0)).expect("first sample");
    assert_eq!((sample.length_frames(), sample.loop_start(), sample.loop_end(), sample.loop_mode()), (8, 0, 8, LoopMode::Forward));
    assert_eq!(module.sample_pcm(SampleId(0)).map(|pcm| pcm[0]), Some(i16::MIN));
    assert_eq!(module.sample_pcm(SampleId(1)).map(|pcm| pcm[0]), Some(-127 * 256));
}

#[test]
fn legacy_images_match_owned_modules_for_both_dialects_with_small_pcm_budgets() {
    for (effect, param) in [(1, 0x37), (0xF, 6)] {
        let source = fixture(effect, param);
        let expected = soundtracker::load(&source).expect("legacy module").to_image();
        let mut destination = vec![0u8; expected.len()];
        let mut workspace = [0u8; 4096];
        let mut decoder = starplayer_mod::ImageDecoder::new(&source, &mut destination, &mut workspace);
        loop {
            if let ImageDecodeStatus::Complete { image_length } = decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 1 }).expect("incremental decode") {
                assert_eq!(image_length, expected.len());
                break;
            }
        }
        assert_eq!(destination, expected);
        let image = Module::from_image_copied(&destination).expect("image validates");
        assert_eq!(image.header().dialect, soundtracker::load(&source).expect("legacy").header().dialect);
    }
}

#[test]
fn ultimate_tempo_and_commands_are_decoded_without_rewriting_native_cells() {
    for (effect, param, command, command_data, tick_period) in [(1, 0x37, 0, 0x37, 360), (2, 4, 1, 4, 424), (2, 0x40, 2, 4, 432)] {
        let bytes = fixture(effect, param);
        let module = Arc::new(soundtracker::load(&bytes).expect("Ultimate Soundtracker"));
        assert_eq!(module.header().dialect, FormatDialect::UltimateSoundtracker);
        assert_eq!(module.header().initial_tempo, 120);
        assert_eq!(module.pattern_bytes(PatternId(0)).map(|pattern| &pattern[..4]), Some(&bytes[600..604]));
        let mut processor = ModProcessor::new(module, 44_100);
        let mut voices = VoicePool::new(4);
        let mut channels = ChannelTable::new(4);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 120);
        let _ = processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes[600..616] });
        assert_eq!(processor.channel(0).map(|channel| (channel.command, channel.command_data)), Some((command, command_data)));
        let mut clock = RowClock::new(6);
        clock.tick_in_row = 1;
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, clock, SongPosition::default(), 120);
        let _ = processor.tick(&mut context);
        assert_eq!(processor.channel(0).map(|channel| channel.actual_period), Some(tick_period));
    }
}

#[test]
fn later_soundtracker_uses_vblank_fxx_and_large_samples_do_not_select_ultimate() {
    let bytes = fixture(0xF, 0x40);
    let module = Arc::new(soundtracker::load(&bytes).expect("later Soundtracker"));
    let mut processor = ModProcessor::new(module, 44_100);
    let mut voices = VoicePool::new(4);
    let mut channels = ChannelTable::new(4);
    let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
    let outcome = processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes[600..616] });
    assert_eq!((outcome.speed, outcome.tempo_bpm), (64, 125));

    let mut bytes = fixture(0, 0);
    bytes[42..44].copy_from_slice(&16384u16.to_be_bytes());
    bytes[48..50].copy_from_slice(&1u16.to_be_bytes());
    bytes.resize(600 + 2 * 1024 + 32768 + 4, 0);
    let module = soundtracker::load(&bytes).expect("Blood Money sized sample");
    assert_eq!((module.header().dialect, module.header().initial_tempo), (FormatDialect::Soundtracker, 125));
    assert_eq!(module.sample(SampleId(0)).map(|sample| sample.length_frames()), Some(32766));
}

#[test]
fn signatureless_detection_rejects_truncation_and_plausibility_false_positives() {
    let valid = fixture(0xF, 6);
    for length in 0..valid.len() {
        assert!(!soundtracker::probe(&valid[..length]), "truncated at {length}");
        assert!(!soundtracker::probe_reader(&CopyReader(&valid[..length])), "reader truncated at {length}");
    }
    for (offset, value) in [(0, 1), (20, 0xff), (44, 16), (45, 65), (470, 0), (470, 129), (472, 128), (600, 0x10), (601, 1), (602, 4)] {
        let mut bytes = valid.clone();
        bytes[offset] = value;
        assert!(!soundtracker::probe(&bytes), "invalid byte at {offset}: {value}");
    }
    assert!(!soundtracker::probe(&vec![0; valid.len()]));
    let mut no_notes = valid.clone();
    no_notes[600..600 + 2 * 1024].fill(0);
    assert!(!soundtracker::probe(&no_notes));
    let mut tagged_collision = valid.clone();
    tagged_collision[1080..1084].copy_from_slice(b"M.K.");
    assert!(!soundtracker::probe(&tagged_collision));
    let mut trailer = valid.clone();
    trailer.extend_from_slice(&[0x91; 24]);
    assert!(soundtracker::probe(&trailer), "Blood Money has a 24-byte trailer");
}

fn render_legacy(block_frames: usize) -> Vec<i16> {
    let module = Arc::new(soundtracker::load(&fixture(1, 0x37)).expect("legacy"));
    let mut engine: Engine<FixedPath, Linear, StereoI16, Arc<Module>> = Engine::with_settings(EngineSettings { sample_rate_hz: 44_100, channel_count: 4, voice_capacity: 4, ..EngineSettings::default() });
    engine.take_control().expect("control").load_module(Arc::clone(&module)).expect("load command");
    engine.set_source(Box::new(starplayer_mod::sequencer_for(module, 44_100, ExactFixedPoint)));
    let mut output = vec![0; 44_100 * 2];
    for block in output.chunks_mut(block_frames * 2) { engine.render(block); }
    assert!(!engine.warnings().any());
    output
}

#[test]
fn legacy_native_audio_is_byte_identical_at_every_required_host_block_size() {
    let expected = render_legacy(128);
    assert!(expected.iter().any(|sample| *sample != 0));
    for block_frames in [1, 3, 64, 128, 4096, 8191] { assert_eq!(render_legacy(block_frames), expected, "block size {block_frames}"); }
}

#[test]
fn legacy_image_validation_scans_pattern_bytes_within_the_step_budget() {
    let mut source = fixture(0xF, 6);
    source.splice(600 + 2 * 1024..600 + 2 * 1024, vec![0; 6 * 1024]);
    source[472 + 127] = 7;
    source[600 + 7 * 1024 + 2] = 4; // unsupported, in the second 4096-byte scan
    let mut destination = vec![0; 32_768];
    let mut workspace = [0; 4096];
    let mut decoder = starplayer_mod::ImageDecoder::new(&source, &mut destination, &mut workspace);
    let budget = DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 1 };
    assert_eq!(decoder.step(budget), Ok(ImageDecodeStatus::Pending), "header preflight does not scan patterns");
    assert_eq!(decoder.step(budget), Ok(ImageDecodeStatus::Pending), "the first pattern scan consumes at most4096 bytes");
    assert!(decoder.step(budget).is_err(), "the next bounded scan reaches the invalid cell");
}

#[test]
fn later_soundtracker_restart_is_retained_without_skipping_the_initial_orders() {
    use starplayer_engine::{ControlClock, EngineContext, EventSource};
    let mut bytes = fixture(0xF, 6);
    bytes[470] = 2;
    bytes[471] = 1;
    bytes[473] = 1;
    let module = Arc::new(soundtracker::load(&bytes).expect("later Soundtracker with restart"));
    assert_eq!(soundtracker::restart_order(&module), 1);
    let image = Module::from_image_copied(&module.to_image()).expect("image with restart");
    assert_eq!(soundtracker::restart_order(&image), 1);
    let mut sequencer = starplayer_mod::sequencer_for(module, 44_100, ExactFixedPoint);
    let mut voices = VoicePool::new(4);
    let mut channels = ChannelTable::new(4);
    let mut control = ControlClock::new(44_100, Frame::ZERO);
    let mut order_starts = Vec::new();
    assert_eq!(sequencer.position().order, 0, "a restart does not skip the beginning on first play");
    for event_index in 0..64 * 6 * 3 + 1 {
        let frame = sequencer.next_event_frame().expect("looping source");
        let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
        sequencer.dispatch(frame, &mut context);
        let position = sequencer.position();
        if event_index % (64 * 6) == 0 { order_starts.push(position.order); }
    }
    assert_eq!(&order_starts[..4], &[0, 1, 1, 1]);
}

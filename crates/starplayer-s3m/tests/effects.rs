//! Effect-processor integration and real-module rendering checks.

use starplayer_core::{ExactFixedPoint, Frame};
use starplayer_dsp::Linear;
use starplayer_engine::{ChannelTable, ControlClock, Engine, EngineContext, EngineSettings, EventSource, PatternSequencer, SongPosition};
use starplayer_mixer::{FixedPath, FloatPath, StereoF32, StereoI16};
use starplayer_mixer::VoicePool;
use starplayer_model::{InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec, ORDER_END};
use starplayer_rt::Arc;
use starplayer_s3m::{S3mCell, S3mPatternData, S3mProcessor, VOLUME_NONE};

const SAMPLE_RATE_HZ: u32 = 44_100;
const TEN_SECONDS: usize = SAMPLE_RATE_HZ as usize * 10;
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

type S3mEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;
type FloatS3mEngine = Engine<FloatPath, Linear, StereoF32, Arc<Module>>;
type TestSequencer = PatternSequencer<ExactFixedPoint, S3mProcessor, S3mPatternData>;

struct SequencerHarness {
    voices: VoicePool,
    channels: ChannelTable,
    control: ControlClock,
}

impl SequencerHarness {
    fn new(channels: usize) -> SequencerHarness {
        SequencerHarness { voices: VoicePool::new(channels.max(2)), channels: ChannelTable::new(channels), control: ControlClock::new(SAMPLE_RATE_HZ, Frame::ZERO) }
    }

    fn tick(&mut self, sequencer: &mut TestSequencer) -> Frame {
        let frame = sequencer.next_event_frame().expect("sequencer has another tick");
        let mut context = EngineContext::new(frame, &mut self.voices, &mut self.channels, &mut self.control);
        sequencer.dispatch(frame, &mut context);
        frame
    }
}

fn module_with_pattern(rows: u16, channels: u8, cells: &[(u16, u8, S3mCell)], with_sample: bool, speed: u8) -> Arc<Module> {
    let mut bytes = vec![0u8; rows as usize * channels as usize * 5];
    for cell in bytes.chunks_exact_mut(5) { cell.copy_from_slice(&S3mCell::EMPTY.to_bytes()); }
    for &(row, channel, cell) in cells {
        let start = (row as usize * channels as usize + channel as usize) * 5;
        bytes[start..start + 5].copy_from_slice(&cell.to_bytes());
    }

    let mut builder = ModuleBuilder::new();
    if with_sample {
        let sample = builder.add_sample(&[1000, 2000, -1000, -2000], SampleSpec::one_shot("test")).expect("test sample");
        builder.add_instrument(InstrumentDef::from_sample("test", sample, starplayer_core::U0F16::MAX)).expect("test instrument");
    }
    builder.add_pattern(&bytes, rows, channels).expect("fixed-stride S3M pattern");
    builder.set_orders(&[0, ORDER_END]);
    let mut header = ModuleHeader::new(ModuleFormat::S3m, channels);
    header.initial_speed = speed;
    header.default_pan = vec![starplayer_s3m::pan_nibble_to_bipolar(12); channels as usize].into_boxed_slice();
    builder.set_header(header);
    Arc::new(builder.build().expect("valid test module"))
}

fn sequencer(module: Arc<Module>) -> TestSequencer {
    starplayer_s3m::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint)
}

fn render(bytes: &[u8], block_frames: usize) -> (Vec<i16>, starplayer_engine::EngineWarnings) {
    let module = Arc::new(starplayer_s3m::load(bytes).expect("real S3M fixture loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count: module.header().channel_count as usize,
        voice_capacity: module.header().channel_count as usize,
        ..EngineSettings::default()
    };
    let mut engine: S3mEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("control handle is available once");
    control.load_module(Arc::clone(&module)).map_err(|_| "module command queued").expect("module command queued");
    engine.set_source(Box::new(starplayer_s3m::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint)));

    let mut output = vec![0i16; TEN_SECONDS * 2];
    let mut written = 0usize;
    while written < output.len() {
        let end = (written + block_frames * 2).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    (output, engine.warnings())
}

fn assert_real_module_is_deterministic(bytes: &[u8], name: &str) {
    let (reference, warnings) = render(bytes, 128);
    assert!(!warnings.any(), "{name}: reference render raises no warnings");
    assert!(reference.iter().any(|sample| *sample != 0), "{name}: ten seconds are not silent");

    for block_frames in BLOCK_SIZES {
        let (output, warnings) = render(bytes, block_frames);
        assert!(!warnings.any(), "{name}: block size {block_frames} raises no warnings");
        assert_eq!(output, reference, "{name}: block size {block_frames} changed the byte-exact render");
    }
}

#[test]
fn reflex_renders_ten_seconds_byte_identically_at_every_host_block_size() {
    assert_real_module_is_deterministic(include_bytes!("fixtures/REFLEX.S3M"), "REFLEX.S3M");
}

#[test]
fn movement_renders_ten_seconds_byte_identically_at_every_host_block_size() {
    assert_real_module_is_deterministic(include_bytes!("fixtures/MOVEMENT.S3M"), "MOVEMENT.S3M");
}

#[test]
fn reflex_float_render_is_finite_non_silent_and_warning_free() {
    let module = Arc::new(starplayer_s3m::load(include_bytes!("fixtures/REFLEX.S3M")).expect("REFLEX loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count: module.header().channel_count as usize,
        voice_capacity: module.header().channel_count as usize,
        ..EngineSettings::default()
    };
    let mut engine: FloatS3mEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("control handle");
    control.load_module(Arc::clone(&module)).map_err(|_| "module command queued").expect("module command queued");
    engine.set_source(Box::new(starplayer_s3m::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint)));
    let mut output = vec![0.0f32; TEN_SECONDS * 2];
    for block in output.chunks_mut(128 * 2) { engine.render(block); }
    assert!(output.iter().all(|sample| sample.is_finite()), "the float render contains no NaN or infinity");
    assert!(output.iter().any(|sample| *sample != 0.0), "the float render is not silent");
    assert!(!engine.warnings().any(), "the float render raises no engine warnings");
}

#[test]
fn sd3_releases_the_note_volume_and_initial_pan_on_tick_three() {
    let cell = S3mCell { note: 0x40, instrument: 1, volume: 23, command: 19, info: 0xD3 };
    let module = module_with_pattern(2, 1, &[(0, 0, cell)], true, 6);
    let mut sequencer = sequencer(module);
    let mut harness = SequencerHarness::new(1);

    harness.tick(&mut sequencer);
    assert!(!harness.channels.is_sounding(starplayer_core::ChannelId(0), &harness.voices), "SD3 suppresses the complete dirty byte on tick zero");
    harness.tick(&mut sequencer);
    harness.tick(&mut sequencer);
    assert!(!harness.channels.is_sounding(starplayer_core::ChannelId(0), &harness.voices), "the delayed note is still withheld through tick two");
    harness.tick(&mut sequencer);

    let voice_id = harness.channels.foreground(starplayer_core::ChannelId(0)).expect("the note sounds on tick three");
    let voice = harness.voices.get(voice_id).expect("foreground voice is live");
    assert_eq!(voice.params.volume, starplayer_core::fixed::unit_from_ratio(23, 64), "the row volume arrives with the delayed sample");
    assert_eq!(voice.params.pan, starplayer_s3m::pan_nibble_to_bipolar(12), "the initial pan dirty bit arrives with the delayed sample");
}

#[test]
fn se2_extends_the_row_to_three_lengths_without_refetching_its_note() {
    let cell = S3mCell { note: 0x40, instrument: 1, volume: VOLUME_NONE, command: 19, info: 0xE2 };
    let module = module_with_pattern(2, 1, &[(0, 0, cell)], true, 3);
    let mut sequencer = sequencer(module);
    let mut harness = SequencerHarness::new(1);

    harness.tick(&mut sequencer);
    assert_eq!(sequencer.row_clock().total_ticks(), 9, "SE2 is the original row plus two whole repeats");
    let original_voice = harness.channels.foreground(starplayer_core::ChannelId(0)).expect("tick zero starts the note");
    for _ in 1..9 {
        harness.tick(&mut sequencer);
        assert_eq!(harness.channels.foreground(starplayer_core::ChannelId(0)), Some(original_voice), "a delayed repeat runs ticks without fetching the note again");
    }
    assert_eq!(sequencer.position().row, 1, "the next row begins after all nine absolute ticks");
}

#[test]
fn sb1_plays_the_marked_range_exactly_twice() {
    let loop_start = S3mCell { command: 19, info: 0xB0, ..S3mCell::EMPTY };
    let loop_once = S3mCell { command: 19, info: 0xB1, ..S3mCell::EMPTY };
    let module = module_with_pattern(4, 1, &[(0, 0, loop_start), (2, 0, loop_once)], false, 1);
    let mut sequencer = sequencer(module);
    let mut harness = SequencerHarness::new(1);
    let mut rows = Vec::new();
    for _ in 0..7 {
        harness.tick(&mut sequencer);
        rows.push(sequencer.sounding_position().row);
    }
    assert_eq!(rows, vec![0, 1, 2, 0, 1, 2, 3], "SB1 adds one repeat, so the inclusive range plays exactly twice");
}

#[test]
fn bxx_and_cxx_combine_into_one_order_and_row_jump_in_either_channel_order() {
    for cells in [
        [(0, 0, S3mCell { command: 2, info: 1, ..S3mCell::EMPTY }), (0, 1, S3mCell { command: 3, info: 0x10, ..S3mCell::EMPTY })],
        [(0, 0, S3mCell { command: 3, info: 0x10, ..S3mCell::EMPTY }), (0, 1, S3mCell { command: 2, info: 1, ..S3mCell::EMPTY })],
    ] {
        let mut pattern_zero = vec![0u8; 64 * 2 * 5];
        let mut pattern_one = vec![0u8; 64 * 2 * 5];
        for cell in pattern_zero.chunks_exact_mut(5).chain(pattern_one.chunks_exact_mut(5)) { cell.copy_from_slice(&S3mCell::EMPTY.to_bytes()); }
        for (_, channel, cell) in cells {
            let start = channel as usize * 5;
            pattern_zero[start..start + 5].copy_from_slice(&cell.to_bytes());
        }
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&pattern_zero, 64, 2).expect("pattern zero");
        builder.add_pattern(&pattern_one, 64, 2).expect("pattern one");
        builder.set_orders(&[0, 1, ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::S3m, 2);
        header.initial_speed = 1;
        builder.set_header(header);
        let mut sequencer = sequencer(Arc::new(builder.build().expect("two-pattern module")));
        let mut harness = SequencerHarness::new(2);
        harness.tick(&mut sequencer);
        assert_eq!(sequencer.position(), SongPosition { order: 1, pattern: 1, row: 10 }, "Bxx names the order and Cxx supplies its decimal row");
    }
}

#[test]
fn txx_on_tick_n_moves_tick_n_plus_one() {
    let tempo = S3mCell { command: 20, info: 250, ..S3mCell::EMPTY };
    let module = module_with_pattern(2, 1, &[(0, 0, tempo)], false, 2);
    let mut sequencer = sequencer(module);
    let mut harness = SequencerHarness::new(1);
    let tick_zero = harness.tick(&mut sequencer);
    let tick_one = harness.tick(&mut sequencer);
    assert_eq!(tick_zero, Frame::ZERO);
    assert_eq!(tick_one, Frame(441), "the tick immediately after TFA uses 250 BPM");
}

#[test]
fn portamento_reuses_a_sounding_voice_but_retriggers_a_finished_one_shot() {
    let first = S3mCell { note: 0x40, instrument: 1, volume: 32, command: 0, info: 0 };
    let porta = S3mCell { note: 0x44, instrument: 1, volume: VOLUME_NONE, command: 7, info: 1 };
    for finish_before_porta in [false, true] {
        let module = module_with_pattern(3, 1, &[(0, 0, first), (1, 0, porta)], true, 1);
        let mut sequencer = sequencer(module);
        let mut harness = SequencerHarness::new(1);
        harness.tick(&mut sequencer);
        let original = harness.channels.foreground(starplayer_core::ChannelId(0)).expect("first note sounds");
        if finish_before_porta { harness.voices.release(original); }
        harness.tick(&mut sequencer);
        let after = harness.channels.foreground(starplayer_core::ChannelId(0)).expect("the channel has a voice after the porta row");
        if finish_before_porta {
            assert_ne!(after, original, "a porta onto a finished one-shot is a fresh trigger");
        } else {
            assert_eq!(after, original, "a porta onto a sounding voice changes only its target pitch");
        }
    }
}

#[test]
fn pattern_data_returns_exact_fixed_stride_rows() {
    let first = S3mCell { note: 0x40, ..S3mCell::EMPTY };
    let second = S3mCell { command: 24, info: 0x80, ..S3mCell::EMPTY };
    let module = module_with_pattern(2, 2, &[(0, 0, first), (1, 1, second)], false, 6);
    let data = S3mPatternData(module);
    let row_zero = starplayer_engine::PatternData::row_bytes(&data, 0, 0).expect("row zero");
    let row_one = starplayer_engine::PatternData::row_bytes(&data, 0, 1).expect("row one");
    assert_eq!(row_zero.len(), 10);
    assert_eq!(row_one.len(), 10);
    assert_eq!(S3mCell::from_bytes(&row_zero[..5]), Some(first));
    assert_eq!(S3mCell::from_bytes(&row_one[5..]), Some(second));
}

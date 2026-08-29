//! The M1-B6 verifications that need the engine: the `_MActual*` discipline, the effect
//! name table, the VU meter driven by real notes, and the reader coming out of a whole
//! `Engine`.
//!
//! Runs only under `--features telemetry`; `cargo xtask ci --job host-tests` runs the
//! engine's tests a second time with the feature on for exactly this file.

#![cfg(feature = "telemetry")]

use starplayer_core::{ChannelId, ExactFixedPoint, Frame, Note, Step, U0F16};
use starplayer_dsp::Linear;
use starplayer_engine::demo::{DEMO_SET_SPEED, DemoCell, DemoPatternData, DemoProcessor};
use starplayer_engine::{
    ChannelTable, ControlClock, Engine, EngineContext, EngineSettings, EventSource, PatternSequencer, RowRef,
    SequencerSettings, SongPosition, TickContext, TickOutcome, TrackerProcessor,
};
use starplayer_mixer::{FixedPath, LoopSpan, OutputFormat, SampleRegion, StereoI16, VoicePool, append_guarded_sample};
use starplayer_model::{EffectNames, s3m_command_code};
use starplayer_telemetry::{Snapshot, TelemetryPublisher, VuMeter, telemetry_channel};

const SAMPLE_RATE_HZ: u32 = 44_100;
/// 44100 Hz at 125 BPM is exactly 882 frames per tick.
const FRAMES_PER_TICK_AT_125: u64 = 882;
/// The default speed: six ticks to a row.
const TICKS_PER_ROW: usize = 6;

fn looping_blob() -> (Vec<i16>, SampleRegion) {
    let mut blob = Vec::new();
    let waveform: Vec<i16> = (0..64).map(|index: i16| 2_500 + index * 350).collect();
    let region = append_guarded_sample(&mut blob, &waveform, LoopSpan::new(0, 64));
    (blob, region)
}

// ── driving a sequencer directly, so a tick can be inspected one at a time ───────────

type TestSequencer<Processor> = PatternSequencer<ExactFixedPoint, Processor, DemoPatternData>;

/// The engine's side of a dispatch, plus a telemetry publisher, without an engine.
struct Harness {
    voices: VoicePool,
    channels: ChannelTable,
    control: ControlClock,
    publisher: TelemetryPublisher,
}

impl Harness {
    fn new(channel_count: usize, publisher: TelemetryPublisher) -> Harness {
        Harness {
            voices: VoicePool::new(8),
            channels: ChannelTable::new(channel_count),
            control: ControlClock::new(SAMPLE_RATE_HZ, Frame::ZERO),
            publisher,
        }
    }

    fn tick<Processor: TrackerProcessor>(&mut self, sequencer: &mut TestSequencer<Processor>) -> Option<Frame> {
        let frame = sequencer.next_event_frame()?;
        let mut context = EngineContext::new(frame, &mut self.voices, &mut self.channels, &mut self.control);
        context.set_telemetry(&mut self.publisher);
        sequencer.dispatch(frame, &mut context);
        Some(frame)
    }
}

fn demo_sequencer(data: DemoPatternData, region: SampleRegion) -> TestSequencer<DemoProcessor> {
    PatternSequencer::new(
        ExactFixedPoint,
        data,
        DemoProcessor::new(region, Step::ONE),
        SequencerSettings { sample_rate_hz: SAMPLE_RATE_HZ, ..SequencerSettings::default() },
    )
}

// ── the _MActual* discipline ────────────────────────────────────────────────────────

/// The verification: **the published row is the one that is sounding, not the one being
/// parsed.** The live cursor moves to row N+1 as soon as row N's last tick is committed;
/// the snapshot must not.
#[test]
fn the_published_row_is_the_row_that_is_sounding_not_the_one_being_parsed() {
    let (_blob, region) = looping_blob();
    let mut data = DemoPatternData::new(1, 4, 2);
    for row in 0..4u16 {
        data.set(0, row, 0, DemoCell::note(48));
    }
    let mut sequencer = demo_sequencer(data, region);
    let (publisher, mut reader) = telemetry_channel();
    let mut harness = Harness::new(2, publisher);

    for row in 0..4u16 {
        for tick_in_row in 0..TICKS_PER_ROW {
            harness.tick(&mut sequencer).expect("the song has four rows to play");

            let snapshot = reader.read();
            assert_eq!(snapshot.transport.row, row, "tick {tick_in_row} of row {row} must publish row {row}");
            assert_eq!(snapshot.transport.tick, tick_in_row as u16, "the tick index counts up within the row");
            assert_eq!(snapshot.transport.order, 0);
            assert_eq!(snapshot.transport.pattern, 0);
            assert_eq!(snapshot.transport.speed, 6);
            assert_eq!(snapshot.transport.tempo_bpm, 125);
        }

        // The whole point: between the last tick of this row and the first of the next,
        // the live cursor has already moved on and the snapshot has not.
        if row < 3 {
            assert_eq!(sequencer.position(), SongPosition { order: 0, pattern: 0, row: row + 1 }, "the live cursor runs ahead");
            assert_eq!(sequencer.sounding_position().row, row, "and the _MActual* latch does not");
            assert_eq!(reader.latest().transport.row, row, "so a UI polling right now still draws the sounding row");
        }
    }
}

/// A mid-row poll from a whole `Engine`, at a host block size that lands inside a row.
#[test]
fn an_engine_publishes_the_sounding_row_when_polled_mid_row() {
    let (blob, region) = looping_blob();
    let mut data = DemoPatternData::new(1, 8, 2);
    for row in 0..8u16 {
        data.set(0, row, 0, DemoCell::note(48));
    }

    let mut engine: Engine<FixedPath, Linear, StereoI16> =
        Engine::with_settings(EngineSettings { voice_capacity: 8, channel_count: 2, sample_rate_hz: SAMPLE_RATE_HZ, ..EngineSettings::default() });
    engine.set_pcm(blob);
    let mut reader = engine.telemetry_reader().expect("the reader is there to be claimed, once");
    assert!(engine.telemetry_reader().is_none(), "and only once");
    engine.set_source(Box::new(demo_sequencer(data, region)));

    let frames_per_row = FRAMES_PER_TICK_AT_125 * TICKS_PER_ROW as u64;
    let mut output = vec![0i16; 2 * StereoI16::CHANNELS];

    for row in 0..6u64 {
        // Render up to three ticks into row `row`, which is squarely mid-row.
        let target = row * frames_per_row + 3 * FRAMES_PER_TICK_AT_125;
        while engine.frame().0 < target {
            engine.render(&mut output);
        }
        let snapshot = reader.read();
        assert_eq!(snapshot.transport.row as u64, row, "the engine is {} frames in, which is row {row}", engine.frame().0);
        assert!(snapshot.voices_active >= 1, "a note is sounding on channel 0");
        assert_eq!(snapshot.channel_count, 2);
        assert!(snapshot.channels[0].active, "the _ActiveFlag follows the pool");
        assert!(!snapshot.channels[1].active, "channel 1 was never given a note");
    }

    assert!(!reader.latest().warnings.any(), "a well-behaved module raises nothing");
}

/// The transport follows an `Axx` speed change on the tick that applies it.
#[test]
fn a_speed_change_reaches_the_transport() {
    let (_blob, region) = looping_blob();
    let mut data = DemoPatternData::new(1, 4, 1);
    data.set(0, 1, 0, DemoCell::command(DEMO_SET_SPEED, 3));
    let mut sequencer = demo_sequencer(data, region);
    let (publisher, mut reader) = telemetry_channel();
    let mut harness = Harness::new(1, publisher);

    for _ in 0..TICKS_PER_ROW {
        harness.tick(&mut sequencer);
    }
    assert_eq!(reader.read().transport.speed, 6, "row 0 played at the initial speed");

    harness.tick(&mut sequencer);
    let snapshot = reader.read();
    assert_eq!(snapshot.transport.row, 1);
    assert_eq!(snapshot.transport.speed, 3, "row 1's Axx is in effect from the tick it appeared on");
}

// ── the VU meter, driven by real notes ──────────────────────────────────────────────

/// A note strikes the bar, and silence walks it down at the original's rate.
#[test]
fn a_note_strikes_the_vu_bar_and_it_decays_at_two_sixty_fourths_a_tick() {
    let (_blob, region) = looping_blob();
    // One note on row 0, nothing after it, and a long enough pattern to watch it fall.
    let mut data = DemoPatternData::new(1, 16, 1);
    data.set(0, 0, 0, DemoCell::note(48));
    let mut sequencer = demo_sequencer(data, region);
    let (publisher, mut reader) = telemetry_channel();
    let mut harness = Harness::new(1, publisher);

    harness.tick(&mut sequencer);
    assert_eq!(reader.read().channels[0].vu_level, U0F16::MAX, "a new note holds the bar at the channel volume");
    assert_eq!(reader.latest().channels[0].note, Some(Note::new(48)));
    assert_eq!(reader.latest().channels[0].instrument, 1);

    for tick in 1..VuMeter::TICKS_TO_SILENCE {
        harness.tick(&mut sequencer);
        let level = reader.read().channels[0].vu_level;
        assert_eq!(level.to_bits(), 65_535 - (tick as u16) * 2_048, "tick {tick} of the decay");
    }
    harness.tick(&mut sequencer);
    assert_eq!(reader.read().channels[0].vu_level, U0F16::ZERO, "silent after exactly {} ticks", VuMeter::TICKS_TO_SILENCE);

    harness.tick(&mut sequencer);
    assert_eq!(reader.read().channels[0].vu_level, U0F16::ZERO, "and it clamps rather than wrapping");
}

// ── the effect-name table ───────────────────────────────────────────────────────────

/// A processor that reports one fixed S3M command on channel 0 of every row, resolving its
/// name through `starplayer-model`'s table — which is the shape B4's ST3 processor will
/// have. `starplayer-telemetry` cannot see that table (its edges are core and rt only), so
/// the name arrives from the format side; this test stands in for the format side.
struct EffectReportingProcessor {
    code: u8,
    param: u8,
}

impl TrackerProcessor for EffectReportingProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, _row: RowRef<'_>) -> TickOutcome {
        let name = EffectNames::S3M.name(self.code, self.param).unwrap_or("");
        context.report_effect(ChannelId(0), self.code, self.param, name);
        context.outcome()
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome { context.outcome() }
}

fn published_effect(code: u8, param: u8) -> (u8, u8, &'static str) {
    let mut sequencer: TestSequencer<EffectReportingProcessor> = PatternSequencer::new(
        ExactFixedPoint,
        DemoPatternData::new(1, 4, 1),
        EffectReportingProcessor { code, param },
        SequencerSettings { sample_rate_hz: SAMPLE_RATE_HZ, ..SequencerSettings::default() },
    );
    let (publisher, mut reader) = telemetry_channel();
    let mut harness = Harness::new(1, publisher);
    harness.tick(&mut sequencer);
    let effect = reader.read().channels[0].effect;
    (effect.code, effect.param, effect.name)
}

/// The task's verification, and the original's single most charming feature: raw `A06` and
/// `S82` come out as English.
#[test]
fn the_effect_column_is_spelled_out_in_english() {
    assert_eq!(published_effect(s3m_command_code(b'A'), 0x06), (1, 0x06, "change speed"), "A06");
    assert_eq!(published_effect(s3m_command_code(b'S'), 0x82), (19, 0x82, "channel pan"), "S82");
    assert_eq!(published_effect(s3m_command_code(b'K'), 0x42), (11, 0x42, "vibrato & vol. slide"));
    assert_eq!(published_effect(s3m_command_code(b'M'), 0x00).2, "", "S3M has no M command, so it has no name");
}

/// The effect column is re-read per row: a row with no effect shows none.
#[test]
fn the_effect_column_is_cleared_at_the_top_of_each_row() {
    let (_blob, region) = looping_blob();
    let mut sequencer = demo_sequencer(DemoPatternData::new(1, 4, 1), region);
    let (mut publisher, mut reader) = telemetry_channel();

    // Stand in for a format that reported an effect on the previous row.
    publisher.report_effect(ChannelId(0), starplayer_telemetry::EffectDisplay::raw(1, 6).with_name("change speed"));
    let mut harness = Harness::new(1, publisher);

    harness.tick(&mut sequencer);
    assert!(reader.read().channels[0].effect.is_empty(), "the DemoProcessor reports nothing, so the row shows nothing");
}

// ── the reader's contract ───────────────────────────────────────────────────────────

#[test]
fn a_reader_polled_faster_than_the_tick_rate_keeps_the_last_snapshot() {
    let (_blob, region) = looping_blob();
    let mut sequencer = demo_sequencer(DemoPatternData::new(1, 4, 1), region);
    let (publisher, mut reader) = telemetry_channel();
    let mut harness = Harness::new(1, publisher);

    assert_eq!(reader.read(), &Snapshot::IDLE, "nothing has been published yet");
    harness.tick(&mut sequencer);

    let first = *reader.read();
    assert_eq!(first.sequence, 1);
    assert!(!reader.has_pending());
    assert_eq!(reader.read(), &first, "polling again without a tick redraws the same frame");

    harness.tick(&mut sequencer);
    assert!(reader.has_pending());
    assert_eq!(reader.read().sequence, 2);
}

/// The channel table is sized by the host — 32 lanes in the web player — but the display
/// wants the *song's* channels. A 3-channel song in a 32-lane engine reports 3.
#[test]
fn the_snapshot_reports_the_songs_channel_count_not_the_tables_lane_count() {
    let (blob, region) = looping_blob();
    let mut data = DemoPatternData::new(1, 4, 3);
    data.set(0, 0, 0, DemoCell::note(48));

    let mut engine: Engine<FixedPath, Linear, StereoI16> =
        Engine::with_settings(EngineSettings { voice_capacity: 8, channel_count: 32, sample_rate_hz: SAMPLE_RATE_HZ, ..EngineSettings::default() });
    engine.set_pcm(blob);
    let mut reader = engine.telemetry_reader().expect("the reader");
    engine.set_source(Box::new(demo_sequencer(data, region)));

    let mut output = vec![0i16; 2 * StereoI16::CHANNELS];
    while engine.frame().0 < 2 * FRAMES_PER_TICK_AT_125 {
        engine.render(&mut output);
    }
    let snapshot = reader.read();
    assert_eq!(snapshot.channel_count, 3, "the song has three channels, whatever the table's capacity");
    assert_eq!(snapshot.active_channels().len(), 3);
}

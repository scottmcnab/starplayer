//! **The musical path's block-size determinism invariant** (M4-task-E4 deliverable 5).
//!
//! `tests/block_size_determinism.rs` holds the line for the tracker path: the same
//! scenario rendered at host block sizes 1, 3, 64, 128, 4096 and 8191 must be
//! byte-identical. A [`MidiSource`] is the second kind of source there has ever been, and
//! it brings two new ways to break that rule — a control tick the source schedules itself,
//! and events that arrive from outside the engine — so it gets the same test rather than
//! the benefit of the doubt.
//!
//! Both instrument implementations are covered: a MOD-format module drives
//! [`SampleInstrument`](starplayer_engine::SampleInstrument), an IT-format one with note
//! maps drives [`MappedInstrument`](starplayer_engine::MappedInstrument).
//!
//! Samples are compared as **bit patterns**, never with `==` on floats, for the same
//! reason the tracker test does it: `==` accepts `0.0 == -0.0` and rejects two identical
//! NaNs, and byte identity is the actual claim.

use starplayer_core::fixed::{unit_from_midi7, unit_from_ratio};
use starplayer_core::{
    ChannelId, Event, Frame, I1F15, InstrumentId, Note, SampleId, Target, TimedEvent, U0F16, VoiceParams,
};
use starplayer_dsp::{Interpolate, Linear};
use starplayer_engine::instrument::{EventFeed, controller};
use starplayer_engine::{
    ChannelTable, ControlClock, Engine, EngineContext, EngineSettings, EventSource, InstrumentRack, MIDI_CHANNEL_BASE,
    MidiSource, RENDER_QUANTUM, external_event_channel, midi_channel,
};
use starplayer_mixer::{FixedPath, FloatPath, MixPath, MonoF32, MonoI16, OutputFormat, VoicePool};
use starplayer_model::{
    InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec, NOTE_MAP_LENGTH,
};
use starplayer_rt::Arc;

/// The host block sizes the invariant is stated over, in frames.
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

/// Frames every run produces: past 20,000, and not a whole number of quanta.
const TOTAL_FRAMES: usize = 20_000;

const SAMPLE_RATE_HZ: u32 = 44_100;

/// Every event frame is deliberately off both a quantum boundary and every block size, so
/// an implementation that applied events at buffer boundaries would be caught.
const _: () = assert!(!(EVENT_FRAMES[0] as usize).is_multiple_of(RENDER_QUANTUM));

const EVENT_FRAMES: [u64; 9] = [37, 1_003, 2_071, 3_119, 4_201, 5_303, 8_191 + 5, 12_007, 15_361];

// ── the fixtures ────────────────────────────────────────────────────────────────────

/// A short waveform with no zero-valued frames, so any mistimed event shows up in the
/// output rather than hiding in silence.
fn waveform(seed: i16) -> Vec<i16> { (0..64).map(|index: i16| seed + index * 400).collect() }

/// A MOD-format module: two instruments, one looping sample each, exactly the shape
/// `InstrumentDef::from_sample` describes and [`SampleInstrument`] plays.
fn sample_module() -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    let mut header = ModuleHeader::new(ModuleFormat::Mod, 4);
    header.title = "midi sample fixture".into();
    builder.set_header(header);

    for (index, seed) in [3_000i16, -2_500].into_iter().enumerate() {
        let specification = SampleSpec {
            loop_mode: LoopMode::Forward,
            loop_start: 16,
            loop_end: 64,
            reference_rate_hz: 8_363 + index as u32 * 1_000,
            ..SampleSpec::one_shot("voice")
        };
        let sample = builder.add_sample(&waveform(seed), specification).expect("the sample is well formed");
        let volume = unit_from_ratio(48 + index as u32 * 8, 64);
        builder.add_instrument(InstrumentDef::from_sample("lead", sample, volume)).expect("room for an instrument");
    }
    Arc::new(builder.build().expect("the fixture module validates"))
}

/// An IT-format module whose instrument selects its sample through the note maps, and
/// transposes the top half of the keyboard — the shape [`MappedInstrument`] plays.
fn mapped_module() -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    let mut header = ModuleHeader::new(ModuleFormat::It, 4);
    header.title = "midi mapped fixture".into();
    builder.set_header(header);

    let low = builder
        .add_sample(&waveform(2_200), SampleSpec { loop_mode: LoopMode::Forward, loop_start: 8, loop_end: 64, reference_rate_hz: 8_363, ..SampleSpec::one_shot("low") })
        .expect("the low sample is well formed");
    let high = builder
        .add_sample(&waveform(-1_900), SampleSpec { loop_mode: LoopMode::PingPong, loop_start: 0, loop_end: 48, reference_rate_hz: 16_726, relative_note: -12, finetune: 32, ..SampleSpec::one_shot("high") })
        .expect("the high sample is well formed");

    // Notes below middle C play the low sample; the rest play the high one, transposed
    // down an octave — both halves of what `MappedInstrument::select` has to do.
    let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
    let mut note_transpose_map: [u8; NOTE_MAP_LENGTH] = core::array::from_fn(|note| note as u8);
    for note in 0..NOTE_MAP_LENGTH {
        let use_low = note < 60;
        note_sample_map[note] = if use_low { low.0 + 1 } else { high.0 + 1 };
        if !use_low {
            note_transpose_map[note] = (note as u8).saturating_sub(12);
        }
    }
    let instrument = InstrumentDef {
        name: "mapped".into(),
        sample: Some(low),
        default_volume: unit_from_ratio(56, 64),
        global_volume: unit_from_ratio(96, 128),
        default_pan: Some(starplayer_core::fixed::bipolar_from_ratio(-8, 32)),
        note_sample_map,
        note_transpose_map,
        ..InstrumentDef::default()
    };
    builder.add_instrument(instrument).expect("room for an instrument");
    builder.add_instrument(InstrumentDef::from_sample("plain", high, U0F16::MAX)).expect("room for a second");
    Arc::new(builder.build().expect("the fixture module validates"))
}

// ── the feed ────────────────────────────────────────────────────────────────────────

/// A fixed list of events at known frames — the shape E5's `SmfSequencer` has, and the
/// second [`EventFeed`] implementation after [`ExternalEventQueue`].
struct ScriptedFeed {
    events: Vec<TimedEvent>,
    next: usize,
}

impl ScriptedFeed {
    fn new(events: Vec<TimedEvent>) -> ScriptedFeed {
        let mut events = events;
        events.sort_by_key(|event| event.frame);
        ScriptedFeed { events, next: 0 }
    }
}

impl EventFeed for ScriptedFeed {
    fn next_frame(&self) -> Option<Frame> { self.events.get(self.next).map(|event| event.frame) }

    fn pop_due(&mut self, frame: Frame) -> Option<TimedEvent> {
        let event = *self.events.get(self.next).filter(|event| event.frame <= frame)?;
        self.next += 1;
        Some(event)
    }
}

/// Note on, bend, sustain, program change, controller and all-sound-off, at frames that
/// are not multiples of the quantum or of any block size.
fn script() -> Vec<TimedEvent> {
    let channel_zero = midi_channel(0);
    let channel_one = midi_channel(1);
    let loud = Event::NoteOn { note: Note::new(60), velocity: U0F16::MAX };
    let quiet = Event::NoteOn { note: Note::new(67), velocity: unit_from_midi7(64) };
    vec![
        TimedEvent::on_channel(Frame(EVENT_FRAMES[0]), channel_zero, Event::Program(InstrumentId(0))),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[1]), channel_zero, loud),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[2]), channel_zero, Event::PitchBend(I1F15::from_bits(12_000))),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[3]), channel_zero, Event::Controller { number: controller::SUSTAIN, value: U0F16::MAX }),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[4]), channel_zero, Event::NoteOff { note: Note::new(60), velocity: U0F16::ZERO }),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[5]), channel_one, Event::Program(InstrumentId(1))),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[5] + 1), channel_one, quiet),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[6]), channel_zero, Event::Controller { number: controller::SUSTAIN, value: U0F16::ZERO }),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[7]), channel_one, Event::Controller { number: controller::PAN, value: unit_from_midi7(20) }),
        TimedEvent::on_channel(Frame(EVENT_FRAMES[8]), channel_one, Event::AllSoundOff),
    ]
}

// ── the engine under test ───────────────────────────────────────────────────────────

fn engine_settings() -> EngineSettings {
    EngineSettings {
        voice_capacity: 32,
        // The MIDI lanes sit above a module's, so the table has to be the full width.
        channel_count: ChannelTable::MAX_CHANNELS,
        sample_rate_hz: SAMPLE_RATE_HZ,
        ..EngineSettings::default()
    }
}

fn midi_source(module: &Arc<Module>) -> MidiSource<ScriptedFeed> {
    let rack = InstrumentRack::for_module(module, SAMPLE_RATE_HZ);
    MidiSource::new(ScriptedFeed::new(script()), rack, SAMPLE_RATE_HZ)
}

fn render_at_block_size<Path, Interp, Out>(module: &Arc<Module>, block_frames: usize) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut engine: Engine<Path, Interp, Out> = Engine::with_settings(engine_settings());
    engine.set_pcm(module.pcm().to_vec());
    engine.set_source(Box::new(midi_source(module)));

    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];
    let mut written = 0;
    while written < output.len() {
        let end = (written + block_frames * Out::CHANNELS).min(output.len());
        engine.render(output.get_mut(written..end).expect("the slice is in range"));
        written = end;
    }
    assert!(!engine.warnings().any(), "a clean script raises no warning: {:?}", engine.warnings());
    output
}

/// The fixed path compares as `i16` bit patterns; the float path as `f32` bits.
fn assert_identical_at_every_block_size(module: &Arc<Module>, label: &str) {
    let fixed_reference = render_at_block_size::<FixedPath, Linear, MonoI16>(module, BLOCK_SIZES[0]);
    assert!(fixed_reference.iter().any(|sample| *sample != 0), "{label}: the scenario must actually sound");
    for block_frames in BLOCK_SIZES {
        let rendered = render_at_block_size::<FixedPath, Linear, MonoI16>(module, block_frames);
        assert_eq!(rendered, fixed_reference, "{label}: the fixed path differed at block size {block_frames}");
    }

    let float_reference: Vec<u32> =
        render_at_block_size::<FloatPath, Linear, MonoF32>(module, BLOCK_SIZES[0]).iter().map(|sample| sample.to_bits()).collect();
    for block_frames in BLOCK_SIZES {
        let rendered: Vec<u32> =
            render_at_block_size::<FloatPath, Linear, MonoF32>(module, block_frames).iter().map(|sample| sample.to_bits()).collect();
        assert_eq!(rendered, float_reference, "{label}: the float path differed at block size {block_frames}");
    }
}

#[test]
fn a_sample_instrument_renders_identically_at_every_block_size() {
    assert_identical_at_every_block_size(&sample_module(), "SampleInstrument");
}

#[test]
fn a_mapped_instrument_renders_identically_at_every_block_size() {
    assert_identical_at_every_block_size(&mapped_module(), "MappedInstrument");
}

// ── behaviour, driven by hand ───────────────────────────────────────────────────────

/// Everything a bare [`EngineContext`] needs, so a rack can be driven without an engine.
struct Bench {
    voices: VoicePool,
    channels: ChannelTable,
    control: ControlClock,
}

impl Bench {
    fn new() -> Bench {
        Bench {
            voices: VoicePool::new(16),
            channels: ChannelTable::new(ChannelTable::MAX_CHANNELS),
            control: ControlClock::new(SAMPLE_RATE_HZ, Frame::ZERO),
        }
    }

    fn context(&mut self, frame: Frame) -> EngineContext<'_> {
        EngineContext::new(frame, &mut self.voices, &mut self.channels, &mut self.control)
    }

    fn sounding_note(&self, channel: u8) -> Option<u8> {
        let id = ChannelId(MIDI_CHANNEL_BASE + channel as u16);
        self.channels.foreground(id).and_then(|voice| self.voices.get(voice)).map(|state| state.tag.note)
    }
}

fn note_on(note: u8) -> Event { Event::NoteOn { note: Note::new(note), velocity: U0F16::MAX } }
fn note_off(note: u8) -> Event { Event::NoteOff { note: Note::new(note), velocity: U0F16::ZERO } }

#[test]
fn the_sustain_pedal_holds_a_note_off_until_it_is_released() {
    let module = sample_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();
    let channel = midi_channel(0);

    rack.apply(Target::Channel(channel), note_on(60), &mut bench.context(Frame(0)));
    assert_eq!(bench.sounding_note(0), Some(60));

    let pedal_down = Event::Controller { number: controller::SUSTAIN, value: U0F16::MAX };
    rack.apply(Target::Channel(channel), pedal_down, &mut bench.context(Frame(64)));
    assert!(rack.sustain(0));

    rack.apply(Target::Channel(channel), note_off(60), &mut bench.context(Frame(128)));
    assert_eq!(bench.sounding_note(0), Some(60), "the pedal is down, so the note keeps sounding");
    assert_eq!(rack.held_notes(0), &[60], "and it is still held, waiting to be released");

    let pedal_up = Event::Controller { number: controller::SUSTAIN, value: U0F16::ZERO };
    rack.apply(Target::Channel(channel), pedal_up, &mut bench.context(Frame(192)));
    assert!(!rack.sustain(0));
    assert!(rack.held_notes(0).is_empty(), "releasing the pedal releases what it was holding");
    let voice = bench.channels.foreground(channel);
    assert_eq!(voice, None, "the channel let its voice go");
}

#[test]
fn a_note_off_for_a_note_that_is_not_sounding_leaves_the_voice_alone() {
    let module = sample_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();
    let channel = midi_channel(0);

    rack.apply(Target::Channel(channel), note_on(60), &mut bench.context(Frame(0)));
    rack.apply(Target::Channel(channel), note_on(64), &mut bench.context(Frame(64)));
    assert_eq!(bench.sounding_note(0), Some(64), "a channel sounds its newest note");
    assert_eq!(rack.held_notes(0), &[60, 64]);

    rack.apply(Target::Channel(channel), note_off(60), &mut bench.context(Frame(128)));
    assert_eq!(bench.sounding_note(0), Some(64), "releasing the older note does not cut the newer one");
    assert_eq!(rack.held_notes(0), &[64]);

    rack.apply(Target::Channel(channel), note_off(64), &mut bench.context(Frame(192)));
    assert_eq!(bench.channels.foreground(channel), None);
}

#[test]
fn all_sound_off_empties_the_pool() {
    let module = sample_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();

    for channel in 0..4u8 {
        rack.apply(Target::Channel(midi_channel(channel)), note_on(60 + channel), &mut bench.context(Frame(0)));
    }
    assert_eq!(bench.voices.voices_active(), 4);

    rack.apply(Target::Global, Event::AllSoundOff, &mut bench.context(Frame(128)));
    for channel in 0..4u8 {
        assert_eq!(bench.channels.foreground(midi_channel(channel)), None, "channel {channel} let go");
        assert!(rack.held_notes(channel).is_empty());
    }

    // The voices are flagged; the pool reclaims them on the next accumulation pass, which
    // is the segment boundary the stop was scheduled for.
    let pcm = module.pcm().to_vec();
    let mut destination = [starplayer_mixer::FixedFrame::default(); 8];
    bench.voices.accumulate::<FixedPath, Linear>(&pcm, &mut destination, SAMPLE_RATE_HZ);
    assert_eq!(bench.voices.voices_active(), 0, "AllSoundOff empties the pool");
}

#[test]
fn the_tracker_vocabulary_is_reported_unsupported_rather_than_guessed_at() {
    let module = sample_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();
    let channel = midi_channel(0);

    let trigger = Event::Trigger(starplayer_core::TriggerSpec::new(SampleId(0)));
    assert_eq!(rack.apply(Target::Channel(channel), trigger, &mut bench.context(Frame(0))), starplayer_engine::EventOutcome::Unsupported);
    let param = Event::Param(starplayer_core::VoiceParam::Volume(U0F16::MAX));
    assert_eq!(rack.apply(Target::Channel(channel), param, &mut bench.context(Frame(0))), starplayer_engine::EventOutcome::Unsupported);
    // A tracker lane is not the rack's to write to, however the event is addressed.
    assert_eq!(rack.apply(Target::Channel(ChannelId(0)), note_on(60), &mut bench.context(Frame(0))), starplayer_engine::EventOutcome::Unsupported);
    assert_eq!(rack.unsupported_events(), 3);
    assert_eq!(bench.voices.voices_active(), 0, "and nothing sounded");
}

#[test]
fn a_bend_of_zero_leaves_the_step_exactly_where_the_note_put_it() {
    let module = sample_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();
    let channel = midi_channel(0);

    rack.apply(Target::Channel(channel), note_on(60), &mut bench.context(Frame(0)));
    let step_at_rest = bench
        .channels
        .foreground(channel)
        .and_then(|voice| bench.voices.get(voice))
        .map(|state| state.params.step)
        .expect("the note sounds");
    // The sample's reference rate, exactly: master-plan decision 4.
    let sample = module.sample(SampleId(0)).expect("the fixture has a sample");
    let expected = starplayer_core::Step::from_ratio(sample.reference_rate_hz() as u64, SAMPLE_RATE_HZ as u64);
    assert_eq!(step_at_rest, expected, "MIDI 60 plays the sample at its reference rate");

    rack.apply(Target::Channel(channel), Event::PitchBend(I1F15::ZERO), &mut bench.context(Frame(64)));
    let after = bench.channels.foreground(channel).and_then(|voice| bench.voices.get(voice)).map(|state| state.params.step);
    assert_eq!(after, Some(step_at_rest), "a centred bend changes nothing");

    rack.apply(Target::Channel(channel), Event::PitchBend(I1F15::MAX), &mut bench.context(Frame(128)));
    let bent = bench.channels.foreground(channel).and_then(|voice| bench.voices.get(voice)).map(|state| state.params.step);
    let two_semitones = starplayer_core::tables::scale_frequency(sample.reference_rate_hz(), 2 * 64);
    assert_eq!(bent, Some(starplayer_core::Step::from_ratio(two_semitones as u64, SAMPLE_RATE_HZ as u64)), "full scale is two semitones up");
}

// ── late events ─────────────────────────────────────────────────────────────────────

/// An event stamped for a frame that has already gone by is dispatched at the current
/// frame and counted, rather than dropped — a slightly-late keystroke must still sound.
#[test]
fn a_late_event_dispatches_at_the_current_frame_and_raises_a_warning() {
    let module = sample_module();
    let rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let (mut producer, queue) = external_event_channel(16);

    let mut engine: Engine<FixedPath, Linear, MonoI16> = Engine::with_settings(engine_settings());
    engine.set_pcm(module.pcm().to_vec());
    engine.set_source(Box::new(MidiSource::new(queue, rack, SAMPLE_RATE_HZ)));

    let mut block = vec![0i16; RENDER_QUANTUM * 4];
    engine.render(&mut block);
    assert!(engine.frame() >= Frame(RENDER_QUANTUM as u64 * 4), "the clock has moved past frame zero");
    assert!(!engine.warnings().late_events, "nothing has been late yet");

    // Stamped for a frame the engine passed several quanta ago.
    producer.send(TimedEvent::on_channel(Frame(1), midi_channel(0), note_on(60))).expect("room in the ring");
    engine.render(&mut block);

    assert!(engine.warnings().late_events, "the late event raised the warning the host reads");
    assert!(engine.channels().foreground(midi_channel(0)).is_some(), "and it still sounded");
    assert!(engine.voices().voices_active() > 0);
}

/// The source's own control tick runs at the engine's rate whether or not anything else
/// is playing (master-plan decision 2), and it does not disturb the engine's own clock.
#[test]
fn the_source_carries_its_own_control_tick() {
    let module = sample_module();
    let rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut source = MidiSource::new(ScriptedFeed::new(Vec::new()), rack, SAMPLE_RATE_HZ);
    assert_eq!(source.control_interval_frames(), 44, "1 ms at 44.1 kHz");
    assert_eq!(source.next_event_frame(), Some(Frame(0)), "an idle feed still owes a control tick");

    let mut bench = Bench::new();
    let mut frame = 0u64;
    for _ in 0..10 {
        let due = source.next_event_frame().expect("the control clock always has a next tick");
        assert_eq!(due, Frame(frame));
        source.dispatch(due, &mut bench.context(due));
        frame += source.control_interval_frames() as u64;
    }
    assert_eq!(source.control_ticks(), 10);
    assert_eq!(source.late_events(), 0);
    assert_eq!(bench.control.ticks(), 0, "the engine's own clock is untouched by the source's");
}

/// A voice a MIDI channel started carries the same tag a tracker voice does, so the
/// telemetry and the trace read both the same way.
#[test]
fn a_midi_voice_is_tagged_with_its_lane_note_instrument_and_sample() {
    let module = mapped_module();
    let mut rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
    let mut bench = Bench::new();
    let channel = midi_channel(3);

    rack.apply(Target::Channel(channel), note_on(72), &mut bench.context(Frame(0)));
    let voice = bench.channels.foreground(channel).expect("the note sounds");
    let state = bench.voices.get(voice).expect("the voice is live");
    assert_eq!(state.tag.channel, (MIDI_CHANNEL_BASE + 3) as u8, "MIDI channel 3 is engine lane 51");
    assert_eq!(state.tag.note, 60, "the note map transposed C-6 down an octave");
    assert_eq!(state.tag.instrument, 1, "one-based, as every format processor tags its voices");
    assert_eq!(state.tag.sample, 2, "and so is the sample: the high sample is number two");
    assert_ne!(state.params.pan, I1F15::ZERO, "the instrument's own pan applied");
    assert_ne!(state.params.volume, U0F16::ZERO);
    assert_ne!(state.params, VoiceParams::SILENT);
}

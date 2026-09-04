//! M4-E7's acceptance case: a real IT tracker sequencer and a scripted live MIDI feed
//! share one [`SourceMux`](starplayer::engine::SourceMux).
//!
//! The script lands at frames 0, 882, 1764 and 2646. At 44.1 kHz and the fixture's
//! initial tempo of 125 BPM, 882 frames is exactly one tracker tick, so every live event
//! collides with slot 0 rather than merely falling near it. The pool is deliberately four
//! voices wide: row zero asks for four tracker voices and the MIDI note asks for a fifth.
//! Whichever source owns the lower slot gets first claim, making the opposite slot order
//! audibly different and proving that the rule is stable rather than order-free.

use starplayer::core::quirks::QuirkSelection;
use starplayer::core::{Event, Frame, InstrumentId, Note, TimedEvent, U0F16};
use starplayer::dsp::{Interpolate, Linear};
use starplayer::engine::{
    ChannelTable, Engine, EngineSettings, EventFeed, InstrumentRack, MidiSource, SourceMux,
    midi_channel,
};
use starplayer::mixer::{FixedPath, FloatPath, MixPath, MonoF32, MonoI16, OutputFormat};
use starplayer::model::Module;
use starplayer::rt::Arc;
use starplayer::NativeSequencer;

const SAMPLE_RATE_HZ: u32 = 44_100;
const TICK_FRAMES: u64 = 882;
const TOTAL_FRAMES: usize = 6_000;
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

struct ScriptedFeed {
    events: Vec<TimedEvent>,
    next: usize,
}

impl ScriptedFeed {
    fn new() -> ScriptedFeed {
        let channel = midi_channel(0);
        let note_on = |frame, note| TimedEvent::on_channel(
            Frame(frame),
            channel,
            Event::NoteOn { note: Note::new(note), velocity: U0F16::MAX },
        );
        ScriptedFeed {
            events: vec![
                TimedEvent::on_channel(Frame::ZERO, channel, Event::Program(InstrumentId(0))),
                note_on(0, 72),
                TimedEvent::on_channel(
                    Frame(TICK_FRAMES),
                    channel,
                    Event::NoteOff { note: Note::new(72), velocity: U0F16::ZERO },
                ),
                note_on(TICK_FRAMES * 2, 67),
                TimedEvent::on_channel(Frame(TICK_FRAMES * 3), channel, Event::AllSoundOff),
            ],
            next: 0,
        }
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

fn jam_source(module: &Arc<Module>, tracker_first: bool) -> SourceMux {
    let tracker = NativeSequencer::new(Arc::clone(module), SAMPLE_RATE_HZ, QuirkSelection::FromDialect)
        .expect("the synthetic IT has a native sequencer");
    let rack = InstrumentRack::for_module(module, SAMPLE_RATE_HZ);
    let midi = MidiSource::new(ScriptedFeed::new(), rack, SAMPLE_RATE_HZ);
    let mut mux = SourceMux::new(2);
    if tracker_first {
        let tracker_slot = mux.insert(Box::new(tracker));
        assert!(tracker_slot.as_ref().is_ok_and(|slot| slot.index() == 0), "tracker takes slot 0");
        let midi_slot = mux.insert(Box::new(midi));
        assert!(midi_slot.as_ref().is_ok_and(|slot| slot.index() == 1), "MIDI takes slot 1");
    } else {
        let midi_slot = mux.insert(Box::new(midi));
        assert!(midi_slot.as_ref().is_ok_and(|slot| slot.index() == 0), "MIDI takes slot 0");
        let tracker_slot = mux.insert(Box::new(tracker));
        assert!(tracker_slot.as_ref().is_ok_and(|slot| slot.index() == 1), "tracker takes slot 1");
    }
    mux
}

fn render<Path, Interp, Out>(block_frames: usize, tracker_first: bool) -> Vec<Out::Sample>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let module = Arc::new(starplayer::it::load(&starplayer_offline::fixtures::synthetic_it()).expect("the synthetic IT loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count: ChannelTable::MAX_CHANNELS,
        // Deliberate contention is what makes the source-slot tie-break observable.
        voice_capacity: 4,
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Interp, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("a fresh engine owns its control handle");
    control.load_module(Arc::clone(&module)).expect("the command ring has room");
    engine.set_source(Box::new(jam_source(&module, tracker_first)));

    let mut output = vec![Out::Sample::default(); TOTAL_FRAMES * Out::CHANNELS];
    let mut written = 0usize;
    while written < output.len() {
        let end = (written + block_frames * Out::CHANNELS).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    assert!(!engine.warnings().any(), "the collision script raised an engine warning: {:?}", engine.warnings());
    output
}

#[test]
fn tracker_and_live_midi_collisions_are_block_size_independent_and_slot_ordered() {
    let fixed_reference = render::<FixedPath, Linear, MonoI16>(BLOCK_SIZES[0], true);
    assert!(fixed_reference.iter().any(|sample| *sample != 0), "the acceptance render must sound");
    for block_frames in BLOCK_SIZES {
        assert_eq!(
            render::<FixedPath, Linear, MonoI16>(block_frames, true),
            fixed_reference,
            "fixed path changed at block size {block_frames}",
        );
    }
    let fixed_opposite = render::<FixedPath, Linear, MonoI16>(BLOCK_SIZES[0], false);
    assert_ne!(fixed_opposite, fixed_reference, "reversing source slots must change the contended render");

    let float_reference: Vec<u32> = render::<FloatPath, Linear, MonoF32>(BLOCK_SIZES[0], true)
        .into_iter()
        .map(f32::to_bits)
        .collect();
    for block_frames in BLOCK_SIZES {
        let rendered: Vec<u32> = render::<FloatPath, Linear, MonoF32>(block_frames, true)
            .into_iter()
            .map(f32::to_bits)
            .collect();
        assert_eq!(rendered, float_reference, "float path changed at block size {block_frames}");
    }
    let float_opposite: Vec<u32> = render::<FloatPath, Linear, MonoF32>(BLOCK_SIZES[0], false)
        .into_iter()
        .map(f32::to_bits)
        .collect();
    assert_ne!(float_opposite, float_reference, "reversing source slots must change the contended float render");
}

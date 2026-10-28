use starplayer_core::{Command, InteractiveAction, InteractiveCommand};
use starplayer_engine::{Engine, EngineSettings};
use starplayer_mixer::{FixedPath, StereoI16};
fn main() {
    let settings = EngineSettings { channel_count: 4, voice_capacity: 4, ..EngineSettings::default() };
    assert!(settings.playback_configuration().is_none());
    let mut engine: Engine<FixedPath, starplayer_dsp::Linear, StereoI16, ()> = Engine::with_settings(settings);
    engine.take_control().unwrap().send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::Play })).unwrap();
    engine.render(&mut [0; 256]);
    assert!(engine.warnings().unsupported_command);
    assert_eq!(engine.channels().len(), 4); assert_eq!(engine.voices().capacity(), 4);
    assert_eq!(engine.channels().layout().live_base(), 48);
}

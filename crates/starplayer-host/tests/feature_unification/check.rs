//! Run with engine features enabled directly and the host features left disabled.
use starplayer::core::{AtEnd, Frame, PlaybackCapabilities, PlaybackIdentity};
use starplayer::engine::{EngineSettings, MixerMode, PlaybackConfiguration, VoiceCapacity};
use starplayer_host::{AudioSpec, HostEngine, ManualBackend, Player, SeekKind, SourceHandles};

fn main() {
    let compiled = starplayer::engine::compiled_playback_capabilities();
    for (capability, supported) in [
        (PlaybackCapabilities::NOTE_OVERLAP, false),
        (PlaybackCapabilities::INTERACTIVE_AUDITION, false),
    ] {
        if supported || !compiled.contains(capability) { continue; }
        let interactive = capability == PlaybackCapabilities::INTERACTIVE_AUDITION;
        let settings = EngineSettings::default().with_playback(PlaybackConfiguration {
            identity: PlaybackIdentity::default(), capabilities: capability, instrument_count: 32,
            capacity: VoiceCapacity { native: 64, custom: if interactive { 0 } else { 4 }, live: if interactive { 2 } else { 0 }, interactive_transport: interactive },
        }).unwrap();
        assert!(HostEngine::build_with_settings(MixerMode::DEFAULT, settings).is_err());
        let mut backend = ManualBackend::new();
        assert!(Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).is_err());
        assert!(backend.driver().is_none(), "reject before opening the audio callback");
        let module = starplayer::rt::Arc::new(starplayer::load(include_bytes!("../../../starplayer-s3m/tests/fixtures/PETRI.S3M")).unwrap());
        assert!(starplayer_host::build_source_with_settings(module, SeekKind::None, Frame::ZERO, &SourceHandles::new(AtEnd::Stop), None, 0, settings).is_err());
    }
    let mut backend = ManualBackend::new();
    let mut legacy = Player::open(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT).unwrap();
    legacy.load(include_bytes!("../../../starplayer-s3m/tests/fixtures/PETRI.S3M")).unwrap();
    legacy.play().unwrap(); backend.driver().unwrap().render(&mut [0.0; 256]);
    assert_eq!(legacy.interactive_transport(), None); assert!(legacy.is_playing());
}

//! Owner artifacts only: never rewrites committed fixtures or goldens.
use starplayer::core::{Command, InteractiveAction, InteractiveCommand};
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{Engine, EngineSettings};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::rt::Arc;
fn musical_mod(piano: bool) -> Vec<u8> {
    const FRAMES: usize = 32768;
    let mut bytes = vec![0; 1084 + 1024 + 3 * FRAMES];
    let title = if piano { b"NO synthetic piano  " } else { b"NO synthetic drums  " };
    bytes[..20].copy_from_slice(title);
    for instrument in 0..3 {
        let header = 20 + instrument * 30; bytes[header + 22..header + 24].copy_from_slice(&((FRAMES / 2) as u16).to_be_bytes()); bytes[header + 25] = 48;
        let mut random = 0x12345678u32;
        for frame in 0..FRAMES {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            let phase = frame % if instrument == 1 { 128 } else { 32 };
            let triangle = if phase < 16 { phase as i32 * 8 - 64 } else { 64 - (phase % 16) as i32 * 8 };
            let raw = if instrument == 0 { ((random >> 24) as i8) as i32 } else { triangle };
            bytes[1084 + 1024 + instrument * FRAMES + frame] = (raw * (FRAMES - frame) as i32 / FRAMES as i32).clamp(-127, 127) as i8 as u8;
        }
    }
    bytes[950] = 1; bytes[1080..1084].copy_from_slice(b"M.K.");
    for row in 0..12 {
        let period: u16 = if piano && row % 3 == 1 { 285 } else { 428 };
        let instrument = if piano { 3 } else { 1 + (row % 2) as u8 };
        let quiet = piano && (3..=4).contains(&row);
        let cell = starplayer::mod_file::ModCell { period, instrument, effect: if row == 0 { 15 } else if quiet { 12 } else { 0 }, param: if row == 0 { 3 } else if quiet { 23 } else { 0 } };
        bytes[1084 + row * 16..1088 + row * 16].copy_from_slice(&cell.to_bytes());
    }
    bytes
}
fn dense_it() -> Vec<u8> {
    let mut bytes = starplayer_offline::fixtures::synthetic_it();
    let orders = u16::from_le_bytes(bytes[32..34].try_into().unwrap()) as usize;
    let instruments = u16::from_le_bytes(bytes[34..36].try_into().unwrap()) as usize;
    let samples = u16::from_le_bytes(bytes[36..38].try_into().unwrap()) as usize;
    let sample_table = 192 + orders + instruments * 4;
    let pattern_table = sample_table + samples * 4;
    let pattern = u32::from_le_bytes(bytes[pattern_table..pattern_table + 4].try_into().unwrap()) as usize;
    let old_length = u16::from_le_bytes(bytes[pattern..pattern + 2].try_into().unwrap()) as usize;
    let mut packed = Vec::new();
    for row in 0..32 { for channel in 0..64 { packed.extend([0x80 | (channel + 1), 0x0f, 48 + row % 24, 1, 32, 0, 0]); } packed.push(0); }
    let delta = packed.len() as i64 - old_length as i64;
    for index in 0..samples {
        let offset = u32::from_le_bytes(bytes[sample_table + index * 4..sample_table + index * 4 + 4].try_into().unwrap()) as usize;
        let pointer = u32::from_le_bytes(bytes[offset + 72..offset + 76].try_into().unwrap()) as i64;
        bytes[offset + 72..offset + 76].copy_from_slice(&((pointer + delta) as u32).to_le_bytes());
    }
    bytes[pattern..pattern + 2].copy_from_slice(&(packed.len() as u16).to_le_bytes());
    bytes.splice(pattern + 8..pattern + 8 + old_length, packed); bytes
}
fn configured_golden<Kernel: starplayer::dsp::Interpolate>(module: &Arc<starplayer::model::Module>, quirks: starplayer::core::quirks::QuirkSet, state: usize) -> Vec<i16> {
    use starplayer::core::{InstrumentId, PlaybackCapabilities};
    use starplayer::engine::{PlaybackConfiguration, VoiceCapacity};
    let capacity = VoiceCapacity { native: starplayer::recommended_voice_capacity(module).max(1), custom: 64, live: 0, interactive_transport: false };
    let settings = EngineSettings { sample_rate_hz: 44100, channel_count: module.header().channel_count as usize, ..EngineSettings::default() }.with_playback(PlaybackConfiguration { identity: Default::default(), capabilities: PlaybackCapabilities::NOTE_OVERLAP, instrument_count: module.instruments().len(), capacity }).unwrap();
    let mut engine: Engine<FixedPath, Kernel, starplayer::mixer::MonoI16, Arc<starplayer::model::Module>> = Engine::with_settings(settings);
    engine.take_control().unwrap().load_module(Arc::clone(module)).ok().unwrap();
    engine.set_source(Box::new(starplayer::NativeSequencer::new_with_articulation(Arc::clone(module), 44100, QuirkSelection::Override(quirks), capacity.physical()).unwrap()));
    let selections: Vec<_> = (0..module.instruments().len()).map(|index| match state { 0 => false, 1 => true, _ => !starplayer::live_instrument_capability(module, InstrumentId(index as u16)).one_shot }).collect();
    engine.restore_selections(Default::default(), state != 1, &selections);
    engine.set_limiter(starplayer::mixer::Limiter::Clamp);
    let mut output = vec![0; starplayer_offline::GOLDEN_RENDER_FRAMES];
    for chunk in output.chunks_mut(128) { engine.render(chunk); }
    assert!(!engine.warnings().any()); output
}
fn check_configured_goldens() {
    use sha2::{Digest, Sha256};
    use starplayer::core::Interpolator;
    for fixture in starplayer_offline::golden_fixtures() {
        let module = Arc::new(starplayer::load(&fixture.bytes).unwrap());
        let quirks = starplayer_offline::scanned_song(&module, 44100).unwrap().quirks;
        let kernels: &[Interpolator] = if fixture.stem == "reflex" { &[Interpolator::Linear, Interpolator::Cubic, Interpolator::Sinc] } else { &[Interpolator::Linear] };
        for &kernel in kernels {
            let name = starplayer_offline::golden_filename_for_interpolator(fixture.stem, kernel);
            let expected = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../goldens").join(fixture.format.directory()).join(&name)).unwrap();
            for state in 0..3 {
                let samples = match kernel {
                    Interpolator::Linear => configured_golden::<starplayer::dsp::Linear>(&module, quirks, state),
                    Interpolator::Cubic => configured_golden::<starplayer::dsp::Cubic>(&module, quirks, state),
                    Interpolator::Sinc => configured_golden::<starplayer::dsp::Sinc>(&module, quirks, state),
                    Interpolator::None => unreachable!(),
                };
                let mut digest = Sha256::new(); for sample in samples { digest.update(sample.to_le_bytes()); }
                let actual: [u8; 32] = digest.finalize().into();
                assert_eq!(starplayer_offline::sha256_hex(actual), expected.trim(), "{} {name} state{state}", fixture.format);
                println!("configured golden {} {name} state{state}: unchanged", fixture.format);
            }
        }
    }
}
fn main() {
    check_configured_goldens();
    let destination = std::env::args_os().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp/no07-artifacts/owner".into());
    std::fs::create_dir_all(&destination).unwrap();
    for (name, bytes) in [("percussion.mod", musical_mod(false)), ("piano.mod", musical_mod(true)), ("mixed-map.xm", starplayer_offline::fixtures::synthetic_xm()), ("dense-native.it", dense_it())] {
        let module = Arc::new(starplayer::load(&bytes).unwrap());
        std::fs::write(destination.join(name), bytes).unwrap();
        for enabled in [false, true] {
            let mut settings = EngineSettings::interactive_web(Default::default(), module.instruments().len()).unwrap(); settings.sample_rate_hz = 48000;
            let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<starplayer::model::Module>> = Engine::with_settings(settings);
            let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).ok().unwrap();
            engine.set_source(Box::new(starplayer::NativeSequencer::new_with_articulation(Arc::clone(&module), 48000, QuirkSelection::FromDialect, 339).unwrap()));
            engine.restore_selections(Default::default(), enabled, &vec![true; module.instruments().len()]);
            control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::Play })).unwrap();
            let mut output = vec![0; 48000 * 8 * 2]; for chunk in output.chunks_mut(256) { engine.render(chunk); }
            assert!(output.iter().any(|sample| *sample != 0));
            starplayer_offline::wav::write_wav(destination.join(format!("{name}-{}.wav", if enabled { "overlap" } else { "native" })), 48000, 2, &output).unwrap();
        }
        if name.ends_with(".mod") { assert!(std::fs::read(destination.join(format!("{name}-native.wav"))).unwrap() != std::fs::read(destination.join(format!("{name}-overlap.wav"))).unwrap(), "musical fixtures must audibly exercise overlap"); }
        println!("{name}: native/overlap48kHz8-second WAV and browser module");
    }
}

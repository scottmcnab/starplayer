//! Native MOD engine integration and output determinism.

use starplayer_core::ExactFixedPoint;
use starplayer_dsp::{Cubic, Interpolate, Linear, Sinc};
use starplayer_engine::{Engine, EngineSettings};
use starplayer_mixer::{FixedPath, StereoI16};
use starplayer_model::Module;
use starplayer_mod::ModCell;
use starplayer_rt::Arc;

const SAMPLE_RATE_HZ: u32 = 44_100;
const OUTPUT_FRAMES: usize = SAMPLE_RATE_HZ as usize;
const BLOCK_SIZES: [usize; 6] = [1, 3, 64, 128, 4096, 8191];

type ModEngine<Interp> = Engine<FixedPath, Interp, StereoI16, Arc<Module>>;

fn looping_mod() -> Vec<u8> {
    const SAMPLE_FRAMES: usize = 256;
    const PATTERN_BYTES: usize = 64 * 4 * 4;
    let mut bytes = vec![0; 1084 + PATTERN_BYTES + SAMPLE_FRAMES];
    bytes[..18].copy_from_slice(b"deterministic MOD ");
    bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[45] = 64;
    bytes[48..50].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
    bytes[950] = 1;
    bytes[1080..1084].copy_from_slice(b"M.K.");
    bytes[1084..1088].copy_from_slice(&ModCell { period: 428, instrument: 1, effect: 4, param: 0x47 }.to_bytes());
    let sample_offset = 1084 + PATTERN_BYTES;
    for (index, byte) in bytes[sample_offset..].iter_mut().enumerate() {
        *byte = ((index as i16 * 5 - 128).clamp(-128, 127) as i8) as u8;
    }
    bytes
}

fn render<Interp: Interpolate>(block_frames: usize) -> (Vec<i16>, starplayer_engine::EngineWarnings) {
    let module = Arc::new(starplayer_mod::load(&looping_mod()).expect("native MOD loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count: module.header().channel_count as usize,
        voice_capacity: module.header().channel_count as usize,
        ..EngineSettings::default()
    };
    let mut engine: ModEngine<Interp> = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("control handle");
    control.load_module(Arc::clone(&module)).map_err(|_| "module command queued").expect("module command queued");
    engine.set_source(Box::new(starplayer_mod::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint)));

    let mut output = vec![0i16; OUTPUT_FRAMES * 2];
    let mut written = 0usize;
    while written < output.len() {
        let end = (written + block_frames * 2).min(output.len());
        engine.render(&mut output[written..end]);
        written = end;
    }
    (output, engine.warnings())
}

fn assert_block_size_independent<Interp: Interpolate>(kernel: &str) {
    let (reference, warnings) = render::<Interp>(128);
    assert!(!warnings.any(), "{kernel}: the reference render raises no engine warning");
    assert!(reference.iter().any(|sample| *sample != 0), "{kernel}: the native MOD render is audible");
    for block_frames in BLOCK_SIZES {
        let (output, warnings) = render::<Interp>(block_frames);
        assert!(!warnings.any(), "{kernel}: block size {block_frames} raises no engine warning");
        assert_eq!(output, reference, "{kernel}: block size {block_frames} changed the byte-exact MOD render");
    }
}

#[test]
fn native_mod_audio_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<Linear>("linear");
}

/// The wide kernels of M7-task-H5, on the same looping MOD: they read behind the
/// interpolation point and defer the loop wrap, and neither may depend on the block.
#[test]
fn native_mod_audio_on_the_wide_kernels_is_byte_identical_at_every_host_block_size() {
    assert_block_size_independent::<Cubic>("cubic");
    assert_block_size_independent::<Sinc>("sinc");
    assert_ne!(render::<Sinc>(128).0, render::<Linear>(128).0, "the kernels are not all rendering the same thing");
}

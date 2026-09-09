//! The module-rebuild half of M10-K5a's proof: an identity enhancer is invisible, a real
//! enhancement is deterministic, and neither moves a song's timeline.

use sha2::{Digest, Sha256};
use starplayer_enhance::{Chain, LoopSmoother, SampleEnhancer, SincUpsampler, UpsampleFactor, from_flags};
use starplayer_model::{EnhancedPcm, Module, SamplePcm};
use starplayer::rt::Arc;
use starplayer_offline::fixtures;

const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");
const PETRI: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");

/// The sample rate the timeline comparison runs at. Any rate does; this is the goldens'.
const SAMPLE_RATE_HZ: u32 = 44_100;

/// An enhancer that changes nothing at all.
struct Identity;

impl SampleEnhancer for Identity {
    fn name(&self) -> String { String::from("identity") }
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm { EnhancedPcm::unchanged(sample) }
}

/// Every format's fixture, as `(name, loaded module)`.
fn every_fixture() -> Vec<(&'static str, Module)> {
    vec![
        ("mod", starplayer::load(&fixtures::synthetic_mod()).expect("the synthetic MOD loads")),
        ("mtm", starplayer::load(&fixtures::synthetic_mtm()).expect("the synthetic MTM loads")),
        ("xm", starplayer::load(&fixtures::synthetic_xm()).expect("the synthetic XM loads")),
        ("it", starplayer::load(&fixtures::synthetic_it()).expect("the synthetic IT loads")),
        ("it-offset", starplayer::load(&fixtures::synthetic_it_with_offset()).expect("the offset IT loads")),
        ("s3m/reflex", starplayer::load(REFLEX).expect("REFLEX loads")),
        ("s3m/petri", starplayer::load(PETRI).expect("PETRI loads")),
    ]
}

#[test]
fn an_identity_enhancer_rebuilds_an_equal_module_for_every_format() {
    for (name, module) in every_fixture() {
        let rebuilt = module.enhanced(&Identity).expect("an identity rebuild");
        assert_eq!(rebuilt.pcm(), module.pcm(), "{name}: the PCM blob moved");
        assert_eq!(rebuilt.blob(), module.blob(), "{name}: the pattern blob moved");
        assert_eq!(rebuilt.samples(), module.samples(), "{name}: the sample table moved");
        assert_eq!(rebuilt, module, "{name}: a rebuild through an identity enhancer must be indistinguishable");
    }
}

#[test]
fn every_rebuilt_sample_records_its_factor_and_keeps_the_files_own_reference_rate() {
    let upsampler = SincUpsampler::new(UpsampleFactor::Four);
    for (name, module) in every_fixture() {
        let enhanced = module.enhanced(&upsampler).expect("a 4x rebuild");
        assert_eq!(enhanced.samples().len(), module.samples().len(), "{name}: the sample count moved");
        for (index, (before, after)) in module.samples().iter().zip(enhanced.samples().iter()).enumerate() {
            assert_eq!(after.reference_rate_hz(), before.reference_rate_hz(), "{name} sample {index}: the reference rate must stay the file's");
            assert_eq!(after.relative_note(), before.relative_note(), "{name} sample {index}: the tuning must not move");
            assert_eq!(after.finetune(), before.finetune(), "{name} sample {index}: the tuning must not move");
            assert_eq!(after.default_volume(), before.default_volume(), "{name} sample {index}: the volume must not move");
            assert_eq!(after.name(), before.name(), "{name} sample {index}: the name must not move");
            match before.length_frames() {
                0 => assert_eq!(after.rate_scale_log2(), 2, "{name} sample {index}: an empty sample still records the factor"),
                _ => assert_eq!(after.rate_scale_log2(), 2, "{name} sample {index}: the factor is not recorded"),
            }
        }
        assert_eq!(enhanced.header(), module.header(), "{name}: the header moved");
        assert_eq!(enhanced.orders(), module.orders(), "{name}: the order list moved");
        assert_eq!(enhanced.instruments(), module.instruments(), "{name}: the instrument table moved");
    }
}

#[test]
fn a_songs_timeline_is_the_same_before_and_after_enhancement() {
    let chain = from_flags(0b11, None).expect("sinc4x + loop");
    for (name, module) in every_fixture() {
        let plain = Arc::new(module.clone());
        let enhanced = Arc::new(module.enhanced(&chain).expect("a 4x + smoothed rebuild"));
        let before = starplayer_offline::song_timeline(&plain, SAMPLE_RATE_HZ).expect("the plain module scans");
        let after = starplayer_offline::song_timeline(&enhanced, SAMPLE_RATE_HZ).expect("the enhanced module scans");
        assert_eq!(after, before, "{name}: enhancement changed the song's timeline");
    }
}

/// The determinism contract: the SHA-256 of REFLEX's PCM after `sinc4x+loop`, little-endian.
///
/// This is the value M10-K5a pinned. It moves only if the polyphase table, the resampler's
/// arithmetic, the loop smoother or the rebuild's layout changes — each of which is a
/// deliberate change to what an enhanced module *is*, and each of which K5b's
/// `_enh-<name>` goldens would then also see.
const REFLEX_SINC4X_LOOP_SHA256: &str = "ec7385b9a30010da283a5b765309d0f821782faaf1b54e2cd87b07702a3c910a";

fn pcm_sha256(module: &Module) -> String {
    let mut hasher = Sha256::new();
    for frame in module.pcm() {
        hasher.update(frame.to_le_bytes());
    }
    starplayer_offline::sha256_hex(hasher.finalize().into())
}

#[test]
fn reflex_enhanced_with_sinc4x_and_loop_hashes_to_its_pinned_value() {
    let module = starplayer::load(REFLEX).expect("REFLEX loads");
    let chain = from_flags(0b11, None).expect("sinc4x + loop");
    assert_eq!(chain.name(), "sinc4x+loop=64", "the pinned hash names this configuration");

    let first = pcm_sha256(&module.enhanced(&chain).expect("a rebuild"));
    let second = pcm_sha256(&module.enhanced(&chain).expect("a second rebuild"));
    assert_eq!(first, second, "the same module enhanced the same way twice must hash the same");
    assert_eq!(first, REFLEX_SINC4X_LOOP_SHA256, "REFLEX's enhanced PCM is not what M10-K5a pinned");
}

#[test]
fn the_chain_and_its_two_stages_applied_by_hand_agree() {
    let module = starplayer::load(REFLEX).expect("REFLEX loads");
    let chain = from_flags(0b11, None).expect("sinc4x + loop");
    let through_chain = module.enhanced(&chain).expect("a chained rebuild");

    let by_hand = Chain::new()
        .then(Box::new(SincUpsampler::new(UpsampleFactor::Four)))
        .then(Box::new(LoopSmoother::new(64)));
    assert_eq!(module.enhanced(&by_hand).expect("a hand-built rebuild"), through_chain, "the catalogue's flag word and the hand-built chain must be the same thing");
}

#[test]
fn a_two_stage_rebuild_and_two_successive_rebuilds_agree_on_the_scale() {
    let module = starplayer::load(&fixtures::synthetic_mod()).expect("the synthetic MOD loads");
    let once = module.enhanced(&SincUpsampler::new(UpsampleFactor::Four)).expect("4x");
    let twice = module
        .enhanced(&SincUpsampler::new(UpsampleFactor::Two))
        .expect("2x")
        .enhanced(&SincUpsampler::new(UpsampleFactor::Two))
        .expect("2x again");
    for (index, (from_one, from_two)) in once.samples().iter().zip(twice.samples().iter()).enumerate() {
        assert_eq!(from_two.rate_scale_log2(), from_one.rate_scale_log2(), "sample {index}: two doublings must record the same scale as one quadrupling");
        assert_eq!(from_two.length_frames(), from_one.length_frames(), "sample {index}: and the same stored length");
    }
}

#[test]
fn a_rate_ceiling_leaves_an_already_fast_module_alone() {
    // Every REFLEX sample is well under 22 kHz, so a 22 kHz ceiling refuses every factor
    // and the rebuild is the identity.
    let module = starplayer::load(REFLEX).expect("REFLEX loads");
    let capped = module.enhanced(&SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(1)).expect("a capped rebuild");
    assert_eq!(capped, module, "a ceiling of 1 Hz can admit no factor at all, so nothing may change");
}

use starplayer_model::{DecodeBudget, ImageDecodeError, ImageDecodeStatus};
use starplayer_xm::ImageDecoder;

const SEEDS: &[(&str, &[u8])] = &[
    ("minimal", include_bytes!("../../../fuzz/seeds/xm/minimal.xm")),
    ("empty pattern", include_bytes!("../../../fuzz/seeds/xm/empty-pattern.xm")),
    ("version 1.02", include_bytes!("../../../fuzz/seeds/xm/version-1.02.xm")),
    ("ping-pong 16-bit", include_bytes!("../../../fuzz/seeds/xm/pingpong-16bit.xm")),
    ("zero-sample instrument", include_bytes!("../../../fuzz/seeds/xm/zero-sample-instrument.xm")),
    ("maximum counts", include_bytes!("../../../fuzz/seeds/xm/maximum-counts.xm")),
];

fn convert(source: &[u8], destination: &mut [u8]) -> Result<usize, ImageDecodeError> {
    let mut workspace = [0u8; 4096];
    let mut decoder = ImageDecoder::new(source, destination, &mut workspace);
    for _ in 0..1_000_000 {
        match decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 1024 })? {
            ImageDecodeStatus::Pending => {}
            ImageDecodeStatus::Complete { image_length } => return Ok(image_length),
        }
    }
    panic!("incremental XM conversion did not complete");
}

#[test]
fn every_xm_seed_produces_the_owned_loaders_exact_image() {
    for (name, source) in SEEDS {
        let expected = starplayer_xm::load(source).unwrap_or_else(|error| panic!("{name} eager load: {error}" )).to_image();
        let mut destination = vec![0u8; expected.len()];
        let length = convert(source, &mut destination).unwrap_or_else(|error| panic!("{name} incremental load: {error:?}"));
        assert_eq!(length, expected.len(), "{name}");
        assert_eq!(destination, expected, "{name}");
    }
}

#[test]
fn destination_capacity_is_preflighted_exactly() {
    let source = SEEDS[3].1;
    let required = starplayer_xm::load(source).expect("seed loads").to_image().len();
    let mut destination = vec![0u8; required - 1];
    assert_eq!(
        convert(source, &mut destination),
        Err(ImageDecodeError::DestinationTooSmall { required, available: required - 1 }),
    );
}

#[test]
fn public_steps_require_the_shared_bounded_budget_floor() {
    let mut destination = [];
    let mut workspace = [];
    let mut decoder = ImageDecoder::new(&[], &mut destination, &mut workspace);
    assert_eq!(
        decoder.step(DecodeBudget { max_input_bytes: 4095, max_pcm_frames: 1 }),
        Err(ImageDecodeError::BudgetTooSmall { minimum_input_bytes: 4096, minimum_pcm_frames: 1 }),
    );
    assert_eq!(
        decoder.step(DecodeBudget { max_input_bytes: 4096, max_pcm_frames: 0 }),
        Err(ImageDecodeError::BudgetTooSmall { minimum_input_bytes: 4096, minimum_pcm_frames: 1 }),
    );
}

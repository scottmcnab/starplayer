//! M10-K5c's enhancement report: every test instrument against every chain under test,
//! as the markdown table the task file's Research resolution carries verbatim.
//!
//! ```text
//! cargo run -p starplayer-offline --example enhance_report --release
//! ```
//!
//! The instruments, the degradation and the three scores live in
//! [`starplayer_offline::enhance_measure`] so that `tests/enhance_quality.rs` pins the
//! acceptance criteria against exactly the numbers printed here. What lives here is the
//! list of chains and the formatting.
//!
//! Every score compares a render of the **degraded** instrument, rebuilt through the
//! chain, against a render of the **ground truth**. Higher is better for the two SNRs;
//! lower is better for the log-spectral distance.

use starplayer_enhance::{Chain, SampleEnhancer, enhancer_for_id};
use starplayer_offline::enhance_measure::{Instrument, Metrics, TAIL_WINDOW_LOWER_DB, TAIL_WINDOW_UPPER_DB, attack_rms_db, degraded, ground_truth, measure, render};

/// The chains the task file asks for, in the order they are tabulated.
const CHAINS: [&[&str]; 7] = [
    &[],
    &["sinc4x"],
    &["denoise"],
    &["denoise", "sinc4x"],
    &["sinc4x", "sbr"],
    &["denoise", "sinc4x", "sbr"],
    &["denoise", "sinc4x", "sbr", "loop"],
];

fn build(ids: &[&str]) -> Option<Chain> {
    if ids.is_empty() {
        return None;
    }
    let mut chain = Chain::new();
    for id in ids {
        chain = chain.then(enhancer_for_id(id, None).unwrap_or_else(|| panic!("{id} is not in the catalogue")));
    }
    Some(chain)
}

fn main() {
    println!("# M10-K5c enhancement report\n");
    println!(
        "Every row renders the degraded instrument (band-limited, decimated to 8 363 Hz, \
         rounded to 8 bits) through the named chain and scores it against a render of the \
         16-bit 44 100 Hz ground truth. SNRs in dB, higher better; LSD in dB, lower better.\n"
    );
    println!(
        "The **log-spectral distance is floored 60 dB below the ground truth's loudest bin** \
         (`analysis::AUDIBLE_FLOOR_BELOW_PEAK_DB`) rather than at the −120 dB the libopenmpt \
         comparison uses: this harness asks whether something sounds closer, and a bin filled a \
         hundred decibels down is not a difference anybody can hear. The floor is anchored to \
         the signal rather than stated as an absolute magnitude because these magnitudes are \
         normalised so a full-scale *sine* puts 0.25 in one bin — an absolute −60 dBFS floor \
         left between 0.3 % and 7.2 % of the reference's bins above it and stopped the score \
         being a measurement at all.\n"
    );
    println!(
        "`tail` is the in-band SNR over the **decay window**: the frames where the ground \
         truth's own level lies between {TAIL_WINDOW_UPPER_DB:.0} dB and {TAIL_WINDOW_LOWER_DB:.0} dB \
         of its peak, which is where a decay crosses the 8-bit quantisation floor. `NaN` means \
         the instrument has no such window — a sustained tone never gets that quiet.\n"
    );
    println!(
        "Instruments (a) and (b) carry harmonics all the way to 20 kHz, so the band `sbr` \
         fills is a band the ground truth genuinely has. (d) is deliberately dark and (c) is \
         broadband noise.\n"
    );

    for instrument in Instrument::ALL {
        let reference = render(&ground_truth(instrument), None, instrument.render_frames()).expect("the ground truth renders");
        let degraded_sample = degraded(instrument);

        println!("## ({}) {}\n", instrument.letter(), instrument.label());
        println!("| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |");
        println!("|---|---:|---:|---:|---:|---:|");
        for ids in CHAINS {
            let chain = build(ids);
            let name = match &chain {
                Some(chain) => chain.name(),
                None => String::from("none"),
            };
            let candidate = render(&degraded_sample, chain.as_ref().map(|chain| chain as &dyn SampleEnhancer), instrument.render_frames())
                .expect("the degraded module renders");
            let Metrics { full_band_snr_db, in_band_snr_db, log_spectral_distance_db, tail_in_band_snr_db } = measure(&reference, &candidate);
            println!(
                "| `{name}` | {full_band_snr_db:.2} | {in_band_snr_db:.2} | {log_spectral_distance_db:.2} | {tail_in_band_snr_db:.2} | {:.2} |",
                attack_rms_db(&candidate, 10.0)
            );
        }
        println!("\nGround truth's own first 10 ms: {:.2} dB.\n", attack_rms_db(&reference, 10.0));
    }
}

//! The polyphase windowed-sinc coefficient table [`SincUpsampler`](crate::SincUpsampler)
//! resamples through.
//!
//! # Generated data, committed as source
//!
//! Exactly the pattern `starplayer_dsp::sinc_table` established, and for the same reason:
//! `sin`, `sqrt` and the Bessel series behind a Kaiser window differ in their last bits
//! between libm implementations, so a table built at run time would make an enhanced
//! module hash differently on x86, ARM and WASM. [`POLYPHASE_TABLE_BITS`] below is the
//! output of [`tests::generate`], which runs only under `cargo test`, and
//! [`tests::the_committed_table_is_what_the_generator_produces`] regenerates it in `f64`
//! and asserts every coefficient bit for bit. Editing the table by hand fails that test.
//!
//! The coefficients are stored as `u64` **bit patterns** rather than as decimal literals,
//! because a decimal literal is a lossy round trip through the parser and this table has
//! to be the generator's `f64` output exactly.
//!
//! # The filter
//!
//! Sixty-four taps per phase, four phases at quarter steps: phase `p` is the filter for an
//! output frame sitting `p/4` of a source frame after a source frame. A 4x upsample uses
//! all four; a 2x upsample uses phases 0 and 2, which are the half-step pair.
//!
//! The prototype is a sinc with its cutoff at [`UPSAMPLE_CUTOFF`] of the **input**
//! Nyquist — the band the source actually holds, not the band the output could hold —
//! windowed by a Kaiser window with β = [`UPSAMPLE_KAISER_BETA`]. β = 10 buys roughly
//! −100 dB of stopband rejection at 64 taps, which is what the image-rejection test in
//! [`crate::upsample`] measures. Each phase is then divided by its own sum, so every phase
//! has **unit DC gain in `f64`** and a constant input comes out constant rather than
//! rippling at the output rate.

/// Taps per phase. The kernel reads `x[-31] .. x[32]` around the interpolation point.
pub const UPSAMPLE_TAPS: usize = 64;

/// Phases the table holds: quarter steps, so a 4x upsample uses each one once.
pub const UPSAMPLE_PHASES: usize = 4;

/// Taps *before* the interpolation point. `UPSAMPLE_TAPS / 2 - 1`, matching
/// `starplayer_dsp::sinc_table`'s `leading = 3` for its eight taps.
pub const UPSAMPLE_LEADING_TAPS: usize = 31;

/// Cutoff as a fraction of the **input** Nyquist.
pub const UPSAMPLE_CUTOFF: f64 = 0.9;

/// Kaiser window shape parameter.
pub const UPSAMPLE_KAISER_BETA: f64 = 10.0;

/// One coefficient, decoded from its committed bit pattern.
///
/// Out-of-range indices answer `0.0` rather than panicking: this crate is
/// `deny(clippy::indexing_slicing)`-shaped even though it never runs on the audio thread,
/// because a load-time panic in a wasm host is just as fatal as one in `render()`.
#[inline]
pub fn coefficient(phase: usize, tap: usize) -> f64 {
    match POLYPHASE_TABLE_BITS.get(phase).and_then(|row| row.get(tap)) {
        Some(bits) => f64::from_bits(*bits),
        None => 0.0,
    }
}

/// `f64` bit patterns, phase-major. Generated — see the module documentation. Do not edit
/// by hand.
pub const POLYPHASE_TABLE_BITS: [[u64; UPSAMPLE_TAPS]; UPSAMPLE_PHASES] = [
    [
        0xbecea3014ffd9fd8, 0x3c0babc380c19e0b, 0x3ef163dc1879c0e7, 0xbf0d6b3dca7f28f9,
        0x3f20d12b0eac5d0c, 0xbf2f33bcf25e3354, 0x3f38dcd5ac1dfe6d, 0xbf4155c04cffe546,
        0x3f4506d7ca212d75, 0xbf45450bab64f16f, 0x3f3e7f7d3c0a0165, 0xbc547ff6e2ff906d,
        0xbf4ae7a876c2a6d2, 0x3f609e203f1bdeb4, 0xbf6d54bd3b4d9d3a, 0x3f75dc038a3810f9,
        0xbf7cda3408da57d8, 0x3f8112d72f41d894, 0xbf81f0b7c8a0fea4, 0x3f8001031c3bb185,
        0xbf749176f90902b2, 0x3c792b75296c5de0, 0x3f7e89e598fdff3e, 0xbf91b98ad1f0e42a,
        0x3f9df1904536ac54, 0xbfa5d5b2fa870d0a, 0x3fad02eba4758c15, 0xbfb1fc663b9fece3,
        0x3fb5136dd53a5eb6, 0xbfb78232af12098a, 0x3fb9108dd4cf316b, 0x3fecccd338e7aaeb,
        0x3fb9108dd4cf316b, 0xbfb78232af12098a, 0x3fb5136dd53a5eb6, 0xbfb1fc663b9fece3,
        0x3fad02eba4758c15, 0xbfa5d5b2fa870d0a, 0x3f9df1904536ac54, 0xbf91b98ad1f0e42a,
        0x3f7e89e598fdff3e, 0x3c792b75296c5de0, 0xbf749176f90902b2, 0x3f8001031c3bb185,
        0xbf81f0b7c8a0fea4, 0x3f8112d72f41d894, 0xbf7cda3408da57d8, 0x3f75dc038a3810f9,
        0xbf6d54bd3b4d9d3a, 0x3f609e203f1bdeb4, 0xbf4ae7a876c2a6d2, 0xbc547ff6e2ff906d,
        0x3f3e7f7d3c0a0165, 0xbf45450bab64f16f, 0x3f4506d7ca212d75, 0xbf4155c04cffe546,
        0x3f38dcd5ac1dfe6d, 0xbf2f33bcf25e3354, 0x3f20d12b0eac5d0c, 0xbf0d6b3dca7f28f9,
        0x3ef163dc1879c0e7, 0x3c0babc380c19e0b, 0xbecea3014ffd9fd8, 0x0000000000000000,
    ],
    [
        0x3ecd7ed506645be4, 0xbeeec7d75879ac43, 0x3f047d8900ac7ac8, 0xbf15398090ed472e,
        0x3f2259cbb6404564, 0xbf2b27964a1d8d74, 0x3f31189943238471, 0xbf315b93a79bc1db,
        0x3f264315db9d96cc, 0x3f14f3d365456e3d, 0xbf4181a95b635d9f, 0x3f53fa778440370e,
        0xbf6158c37ee9b871, 0x3f69c87c116ad866, 0xbf7100488967c6c2, 0x3f7407ede1cca77a,
        0xbf74beba20236481, 0x3f71c671649ed451, 0xbf63a75f3e0a67bf, 0xbf503e3618d4c3d6,
        0x3f783b9599c64731, 0xbf8913a7a13c64b1, 0x3f940cdd8f6b85ca, 0xbf9be2f4317ff808,
        0x3fa183aad654abad, 0xbfa41208033cbfd9, 0x3fa4c24880986067, 0xbfa26bf297b7e4ed,
        0x3f964acb064a35f3, 0x3f863466e3fc8833, 0xbfb8c46016046300, 0x3fea74052941cf82,
        0x3fd7197864831ecb, 0xbfc651d77514a343, 0x3fbc8594529ce0a6, 0xbfb2ce4db7d1fa1a,
        0x3fa77cefaa1118f1, 0xbf996210a63f9eba, 0x3f8236fe0c19bff3, 0x3f63e895231a1eff,
        0xbf83de81223158a7, 0x3f8bad17d0ad2a6d, 0xbf8dd229d177a866, 0x3f8bd8ff077b1123,
        0xbf874862d1705da2, 0x3f817f1520e4affa, 0xbf77309e9f223f17, 0x3f69722708b7dea6,
        0xbf51faee33ae650a, 0xbf32e7bd98154e2d, 0x3f51ceb9964e20b3, 0xbf5705a079e2fc31,
        0x3f56aac91fb14fed, 0xbf530bf4cd3d6a1b, 0x3f4c3454db06c95b, 0xbf42724c19f1bb4a,
        0x3f34dd1369eee672, 0xbf231605519f2d52, 0x3f05d99d1f0e78e3, 0x3ee1eeca2c3c2042,
        0xbef91044089692ff, 0x3ef64c4075de74d1, 0xbeeaa31cab5808a3, 0x3ed4bee1f4528bb6,
    ],
    [
        0x3eda0142ec47f9fb, 0xbef321a07b5d3180, 0x3f04230159401642, 0xbf10e2ebcbfde45f,
        0x3f16f96bd047de15, 0xbf17d86c85c0cd41, 0x3f096381f91c42f4, 0x3f12e8cccccd31bf,
        0xbf33d370172d399a, 0x3f45becb2dfd2638, 0xbf52df0f6935a7e9, 0x3f5c429bde51b743,
        0xbf62c3b29239e558, 0x3f6623e8f34b05ca, 0xbf66ac1e32d29017, 0x3f628eeed1c8ee0a,
        0xbf5021f2a38b3772, 0xbf5428fbfdc3d886, 0x3f7221adfa079733, 0xbf8162ee4565eddf,
        0x3f8ad559975048d7, 0xbf92262cb8bf2d14, 0x3f961bb84d83b0f2, 0xbf984f1f7b66ba70,
        0x3f9799a21b28ed1c, 0xbf92aedf337f8cae, 0x3f801a0057e7b871, 0x3f849fbd84ba62d3,
        0xbfa3f869e4ece044, 0x3fb663b393a8b2f4, 0xbfc7f368e1849e62, 0x3fe419024261b9ba,
        0x3fe419024261b9ba, 0xbfc7f368e1849e62, 0x3fb663b393a8b2f4, 0xbfa3f869e4ece044,
        0x3f849fbd84ba62d3, 0x3f801a0057e7b871, 0xbf92aedf337f8cae, 0x3f9799a21b28ed1c,
        0xbf984f1f7b66ba70, 0x3f961bb84d83b0f2, 0xbf92262cb8bf2d14, 0x3f8ad559975048d7,
        0xbf8162ee4565eddf, 0x3f7221adfa079733, 0xbf5428fbfdc3d886, 0xbf5021f2a38b3772,
        0x3f628eeed1c8ee0a, 0xbf66ac1e32d29017, 0x3f6623e8f34b05ca, 0xbf62c3b29239e558,
        0x3f5c429bde51b743, 0xbf52df0f6935a7e9, 0x3f45becb2dfd2638, 0xbf33d370172d399a,
        0x3f12e8cccccd31bf, 0x3f096381f91c42f4, 0xbf17d86c85c0cd41, 0x3f16f96bd047de15,
        0xbf10e2ebcbfde45f, 0x3f04230159401642, 0xbef321a07b5d3180, 0x3eda0142ec47f9fb,
    ],
    [
        0x3ed4bee1f4528bb9, 0xbeeaa31cab5808a6, 0x3ef64c4075de74d4, 0xbef9104408969302,
        0x3ee1eeca2c3c2044, 0x3f05d99d1f0e78e5, 0xbf231605519f2d54, 0x3f34dd1369eee675,
        0xbf42724c19f1bb4c, 0x3f4c3454db06c95e, 0xbf530bf4cd3d6a1d, 0x3f56aac91fb14ff0,
        0xbf5705a079e2fc33, 0x3f51ceb9964e20b5, 0xbf32e7bd98154e2f, 0xbf51faee33ae650c,
        0x3f69722708b7dea9, 0xbf77309e9f223f19, 0x3f817f1520e4affc, 0xbf874862d1705da5,
        0x3f8bd8ff077b1127, 0xbf8dd229d177a86a, 0x3f8bad17d0ad2a70, 0xbf83de81223158a9,
        0x3f63e895231a1f02, 0x3f8236fe0c19bff5, 0xbf996210a63f9ebd, 0x3fa77cefaa1118f4,
        0xbfb2ce4db7d1fa1c, 0x3fbc8594529ce0aa, 0xbfc651d77514a346, 0x3fd7197864831ecd,
        0x3fea74052941cf86, 0xbfb8c46016046303, 0x3f863466e3fc8836, 0x3f964acb064a35f5,
        0xbfa26bf297b7e4ef, 0x3fa4c2488098606a, 0xbfa41208033cbfdc, 0x3fa183aad654abaf,
        0xbf9be2f4317ff80b, 0x3f940cdd8f6b85cc, 0xbf8913a7a13c64b4, 0x3f783b9599c64734,
        0xbf503e3618d4c3d8, 0xbf63a75f3e0a67c2, 0x3f71c671649ed453, 0xbf74beba20236484,
        0x3f7407ede1cca77c, 0xbf7100488967c6c4, 0x3f69c87c116ad86a, 0xbf6158c37ee9b874,
        0x3f53fa7784403710, 0xbf4181a95b635da1, 0x3f14f3d365456e40, 0x3f264315db9d96ce,
        0xbf315b93a79bc1dd, 0x3f31189943238473, 0xbf2b27964a1d8d78, 0x3f2259cbb6404567,
        0xbf15398090ed4730, 0x3f047d8900ac7acb, 0xbeeec7d75879ac47, 0x3ecd7ed506645be8,
    ],
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Regenerate the table in `f64`, exactly as the committed constant was produced.
    fn generate() -> Vec<[f64; UPSAMPLE_TAPS]> {
        let leading = UPSAMPLE_LEADING_TAPS as f64;
        let mut table = Vec::with_capacity(UPSAMPLE_PHASES);
        for phase in 0..UPSAMPLE_PHASES {
            let fraction = phase as f64 / UPSAMPLE_PHASES as f64;
            let mut ideal = [0.0f64; UPSAMPLE_TAPS];
            let mut sum = 0.0;
            for (tap, coefficient) in ideal.iter_mut().enumerate() {
                // The tap sits `offset` source frames from the interpolation point, and
                // the window spans the sixty-four frames `[-32, 32)` around it.
                let offset = tap as f64 - leading - fraction;
                let position = (offset + leading + 1.0) / UPSAMPLE_TAPS as f64;
                *coefficient = UPSAMPLE_CUTOFF * sinc(UPSAMPLE_CUTOFF * offset) * kaiser(position, UPSAMPLE_KAISER_BETA);
                sum += *coefficient;
            }
            // Unit DC gain per phase, in `f64` — there is no quantisation step here, so
            // unlike the Q1.15 render table no residual has to be pushed anywhere.
            for coefficient in ideal.iter_mut() {
                *coefficient /= sum;
            }
            table.push(ideal);
        }
        table
    }

    fn sinc(x: f64) -> f64 {
        if x.abs() < 1.0e-12 { 1.0 } else { (core::f64::consts::PI * x).sin() / (core::f64::consts::PI * x) }
    }

    /// Kaiser window over `position` in `0..=1`.
    fn kaiser(position: f64, beta: f64) -> f64 {
        let normalised = 2.0 * position - 1.0;
        let inner = 1.0 - normalised * normalised;
        if inner <= 0.0 {
            return 0.0;
        }
        bessel_i0(beta * inner.sqrt()) / bessel_i0(beta)
    }

    /// Modified Bessel function of the first kind, order zero, by its power series.
    fn bessel_i0(x: f64) -> f64 {
        let mut sum = 1.0;
        let mut term = 1.0;
        for index in 1..60 {
            let ratio = x / 2.0 / index as f64;
            term *= ratio * ratio;
            sum += term;
            if term < 1.0e-18 * sum {
                break;
            }
        }
        sum
    }

    /// The regeneration gate. If this fails, either the table was edited by hand or the
    /// generator changed — and the second moves every enhanced module's hash.
    #[test]
    fn the_committed_table_is_what_the_generator_produces() {
        let generated = generate();
        assert_eq!(generated.len(), POLYPHASE_TABLE_BITS.len());
        for (phase, (expected, committed)) in generated.iter().zip(POLYPHASE_TABLE_BITS.iter()).enumerate() {
            for (tap, (expected, committed)) in expected.iter().zip(committed.iter()).enumerate() {
                assert_eq!(expected.to_bits(), *committed, "phase {phase} tap {tap} of the committed table is not what the generator produces");
            }
        }
    }

    #[test]
    fn every_phase_has_unit_dc_gain() {
        for phase in 0..UPSAMPLE_PHASES {
            let sum: f64 = (0..UPSAMPLE_TAPS).map(|tap| coefficient(phase, tap)).sum();
            assert!((sum - 1.0).abs() < 1.0e-12, "phase {phase} sums to {sum}, not to unity");
        }
    }

    #[test]
    fn phase_zero_is_the_identity_and_is_dominated_by_the_frame_it_sits_on() {
        let centre = coefficient(0, UPSAMPLE_LEADING_TAPS);
        assert!(centre > 0.85, "phase 0's centre tap is {centre}, far below the 0.9 cutoff's ~0.9");
        for tap in 0..UPSAMPLE_TAPS {
            if tap != UPSAMPLE_LEADING_TAPS {
                assert!(coefficient(0, tap) <= centre, "phase 0 tap {tap} outweighs the centre");
            }
        }
    }

    #[test]
    fn the_half_step_phase_is_symmetric_about_its_two_centre_taps() {
        // Phase 2 sits exactly halfway between two source frames, so its taps mirror
        // about the pair `x[0]`/`x[1]` — the property that makes 2x upsampling reuse it.
        for tap in 0..UPSAMPLE_TAPS / 2 {
            let left = coefficient(2, UPSAMPLE_LEADING_TAPS - tap);
            let right = coefficient(2, UPSAMPLE_LEADING_TAPS + 1 + tap);
            assert!((left - right).abs() < 1.0e-12, "phase 2 tap {tap} is not symmetric: {left} vs {right}");
        }
    }

    /// Emit the committed constant. Ignored by default; run it with
    /// `cargo test -p starplayer-enhance -- --ignored --nocapture emit_table` and paste
    /// the output over [`POLYPHASE_TABLE_BITS`] after changing the generator.
    #[test]
    #[ignore = "a generator, not a check"]
    fn emit_table() {
        let table = generate();
        std::println!("pub const POLYPHASE_TABLE_BITS: [[u64; UPSAMPLE_TAPS]; UPSAMPLE_PHASES] = [");
        for row in &table {
            std::println!("    [");
            for chunk in row.chunks(4) {
                let cells: Vec<std::string::String> = chunk.iter().map(|coefficient| std::format!("0x{:016x}", coefficient.to_bits())).collect();
                std::println!("        {},", cells.join(", "));
            }
            std::println!("    ],");
        }
        std::println!("];");
    }
}

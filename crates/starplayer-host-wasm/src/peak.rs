//! The VU side of the telemetry path.
//!
//! `plans/product/01-technical-architecture.md` §9 classifies peak levels as a *lossy
//! audio tap*: the reader may see a torn or stale value and it does not matter, so the
//! transport can be a plain relaxed write into shared memory rather than a seqlock. The
//! decay follows the original — `__UpdateTracker` walked `_VUBarLevel` down by a fixed
//! amount every tick — so the meter falls smoothly instead of flickering at quantum
//! rate.

/// Fraction of the held level that survives one quantum. 0.94 over a 2.7 ms quantum at
/// 48 kHz is roughly a 200 ms fall from full scale to silence, which reads well on a bar.
const HOLD_DECAY_PER_QUANTUM: f32 = 0.94;

/// A peak-hold meter over one quantum at a time.
pub struct PeakMeter {
    held: f32,
}

impl PeakMeter {
    pub fn new() -> Self {
        Self { held: 0.0 }
    }

    /// Folds one rendered block into the meter and returns the level to publish.
    ///
    /// Called once per quantum from the audio path: no allocation, one pass, and `held`
    /// stays finite because a non-finite sample is skipped rather than propagated.
    pub fn observe(&mut self, samples: &[f32]) -> f32 {
        let mut block_peak = 0.0f32;
        for sample in samples {
            let magnitude = sample.abs();
            if magnitude > block_peak && magnitude.is_finite() {
                block_peak = magnitude;
            }
        }
        self.held *= HOLD_DECAY_PER_QUANTUM;
        if block_peak > self.held {
            self.held = block_peak;
        }
        self.held
    }

    pub fn level(&self) -> f32 {
        self.held
    }
}

impl Default for PeakMeter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loud_block_pins_the_meter_and_then_it_falls() {
        let mut meter = PeakMeter::new();
        assert!((meter.observe(&[0.0, 0.8, -0.9, 0.1]) - 0.9).abs() < 1e-6);
        let after_silence = meter.observe(&[0.0; 4]);
        assert!(after_silence < 0.9 && after_silence > 0.0, "decayed but not gone: {after_silence}");
    }

    #[test]
    fn silence_eventually_reads_as_silence() {
        let mut meter = PeakMeter::new();
        meter.observe(&[1.0]);
        for _ in 0..1000 { meter.observe(&[0.0]); }
        assert!(meter.level() < 1e-6, "meter fell to zero: {}", meter.level());
    }

    #[test]
    fn a_non_finite_sample_cannot_poison_the_meter() {
        let mut meter = PeakMeter::new();
        meter.observe(&[f32::NAN, f32::INFINITY, 0.25]);
        assert!(meter.level().is_finite());
        assert!((meter.level() - 0.25).abs() < 1e-6);
    }
}

//! A band-limited-enough sine oscillator built from a lookup table.
//!
//! The architecture bans transcendental functions from the real-time path
//! (`plans/product/01-technical-architecture.md` §7.3): `sin()` is compiled differently
//! on x86, ARM and WASM, so an engine that calls it cannot be bit-identical across
//! targets. The table is therefore built **once, at init**, where a call to `f64::sin`
//! is merely ordinary code, and the audio path only ever indexes it.
//!
//! Phase is a 32-bit fixed-point turn count (Q0.32): the whole `u32` range is one cycle,
//! so wrapping is free and exact and there is no accumulating floating-point drift. The
//! top 10 bits select a table entry and the remaining 22 bits interpolate between it and
//! its successor.

/// Entries in one cycle. A power of two so the phase split is a shift and a mask.
pub const SINE_TABLE_LENGTH: usize = 1024;

/// Bits of phase consumed by the table index.
const INDEX_BITS: u32 = 10;

/// Bits of phase left over for interpolation between two entries.
const FRACTION_BITS: u32 = 32 - INDEX_BITS;

const FRACTION_MASK: u32 = (1 << FRACTION_BITS) - 1;

/// Reciprocal of the fraction range, so the fraction becomes a multiply rather than a
/// divide.
const FRACTION_SCALE: f32 = 1.0 / (1u64 << FRACTION_BITS) as f32;

/// One cycle of a sine, plus a guard entry equal to the first so that interpolation at
/// the very top of the table never has to wrap its index.
pub struct SineTable {
    samples: Vec<f32>,
}

impl SineTable {
    /// Builds the table. Calls `f64::sin`, which is why this may only be done at init.
    pub fn new() -> Self {
        let mut samples = Vec::with_capacity(SINE_TABLE_LENGTH + 1);
        for index in 0..SINE_TABLE_LENGTH {
            let turns = index as f64 / SINE_TABLE_LENGTH as f64;
            samples.push((turns * core::f64::consts::TAU).sin() as f32);
        }
        let first = samples[0];
        samples.push(first);
        Self { samples }
    }

    /// Linearly interpolated lookup. Cannot panic: `phase >> 22` is at most 1023 and the
    /// table holds 1025 entries, so both indices are always in range.
    #[inline]
    pub fn lookup(&self, phase: u32) -> f32 {
        let index = (phase >> FRACTION_BITS) as usize;
        let fraction = (phase & FRACTION_MASK) as f32 * FRACTION_SCALE;
        let lower = self.samples[index];
        let upper = self.samples[index + 1];
        lower + (upper - lower) * fraction
    }
}

impl Default for SineTable {
    fn default() -> Self {
        Self::new()
    }
}

/// A phase accumulator whose frequency is slewed rather than stepped.
///
/// Stepping the phase increment on a slider move produces an audible zipper; the whole
/// point of the spike is to prove the command path is glitch-free, so the increment is
/// approached exponentially in integer arithmetic — no floats, no transcendentals, and
/// the same result on every target.
pub struct SineOscillator {
    sample_rate: f32,
    phase: u32,
    current_increment: u32,
    target_increment: u32,
    current_gain: i32,
    target_gain: i32,
}

/// Shift applied to the increment error each frame: the increment closes ~1/1024 of the
/// remaining distance per sample, a time constant of about 21 ms at 48 kHz.
const SLEW_SHIFT: u32 = 10;

/// Gain is carried as Q16.16 so the same integer slew works for it.
const GAIN_ONE: i32 = 1 << 16;

impl SineOscillator {
    pub fn new(sample_rate: f32, frequency: f32, gain: f32) -> Self {
        let increment = Self::increment_for(sample_rate, frequency);
        let gain = (gain.clamp(0.0, 1.0) * GAIN_ONE as f32) as i32;
        Self {
            sample_rate,
            phase: 0,
            // Start silent and at the right pitch, then fade in: the fade is what keeps
            // the very first quantum after `resume()` from clicking.
            current_increment: increment,
            target_increment: increment,
            current_gain: 0,
            target_gain: gain,
        }
    }

    /// Phase increment per frame, as Q0.32 turns.
    fn increment_for(sample_rate: f32, frequency: f32) -> u32 {
        // NaN has to be excluded explicitly: it fails every comparison, and `clamp`
        // panics on it.
        let usable = frequency.is_finite() && frequency > 0.0 && sample_rate.is_finite() && sample_rate > 0.0;
        if !usable {
            return 0;
        }
        let turns_per_frame = (frequency as f64) / (sample_rate as f64);
        // Anything at or above Nyquist is pointless; clamp rather than alias.
        let turns_per_frame = turns_per_frame.clamp(0.0, 0.5);
        (turns_per_frame * (1u64 << 32) as f64) as u32
    }

    pub fn set_frequency(&mut self, frequency: f32) {
        self.target_increment = Self::increment_for(self.sample_rate, frequency);
    }

    /// The frequency the oscillator is actually running at right now, in Hz — the page
    /// reads this back to show the slew arriving.
    pub fn current_frequency(&self) -> f32 {
        (self.current_increment as f64 / (1u64 << 32) as f64 * self.sample_rate as f64) as f32
    }

    /// Produces one frame and advances. No branches on data, no allocation, no float
    /// transcendentals.
    #[inline]
    pub fn next_sample(&mut self, table: &SineTable) -> f32 {
        self.current_increment = slew(self.current_increment as i64, self.target_increment as i64) as u32;
        self.current_gain = slew(self.current_gain as i64, self.target_gain as i64) as i32;
        let sample = table.lookup(self.phase);
        self.phase = self.phase.wrapping_add(self.current_increment);
        sample * (self.current_gain as f32 / GAIN_ONE as f32)
    }
}

/// Move `current` one slew step towards `target`, snapping once the remaining error is
/// smaller than a single step so the two actually meet.
#[inline]
fn slew(current: i64, target: i64) -> i64 {
    let error = target - current;
    let step = error >> SLEW_SHIFT;
    if step == 0 { target } else { current + step }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_one_cycle() {
        let table = SineTable::new();
        assert!(table.lookup(0).abs() < 1e-6, "sine starts at zero");
        assert!((table.lookup(1 << 30) - 1.0).abs() < 1e-5, "quarter turn is the positive peak");
        assert!(table.lookup(1 << 31).abs() < 1e-5, "half turn is back at zero");
        assert!((table.lookup(3 << 30) + 1.0).abs() < 1e-5, "three-quarter turn is the negative peak");
    }

    #[test]
    fn lookup_never_leaves_the_unit_range() {
        let table = SineTable::new();
        for step in 0..4096u32 {
            let phase = step.wrapping_mul(1_048_573);
            let value = table.lookup(phase);
            assert!((-1.0..=1.0).contains(&value), "phase {phase} produced {value}");
        }
    }

    #[test]
    fn frequency_slews_towards_its_target_without_jumping() {
        let table = SineTable::new();
        let mut oscillator = SineOscillator::new(48_000.0, 440.0, 1.0);
        oscillator.set_frequency(880.0);
        let before = oscillator.current_frequency();
        oscillator.next_sample(&table);
        let after = oscillator.current_frequency();
        assert!(after > before, "the slew moved: {before} -> {after}");
        assert!(after < 500.0, "one frame does not jump the whole way: {after}");

        for _ in 0..48_000 { oscillator.next_sample(&table); }
        assert!((oscillator.current_frequency() - 880.0).abs() < 0.1, "the slew arrives exactly");
    }

    #[test]
    fn gain_fades_in_rather_than_starting_at_full_scale() {
        let table = SineTable::new();
        let mut oscillator = SineOscillator::new(48_000.0, 440.0, 1.0);
        assert_eq!(oscillator.next_sample(&table), 0.0, "the first frame is silent");
        let mut peak = 0.0f32;
        for _ in 0..64 { peak = peak.max(oscillator.next_sample(&table).abs()); }
        assert!(peak < 0.5, "the fade is still climbing after 64 frames: {peak}");
    }

    #[test]
    fn frequency_above_nyquist_is_clamped_rather_than_aliased() {
        let mut oscillator = SineOscillator::new(48_000.0, 440.0, 1.0);
        oscillator.set_frequency(1.0e9);
        for _ in 0..200_000 { oscillator.current_increment = slew(oscillator.current_increment as i64, oscillator.target_increment as i64) as u32; }
        assert!(oscillator.current_frequency() <= 24_000.0 + 1.0, "clamped to Nyquist");
    }
}

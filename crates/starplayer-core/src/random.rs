//! [`Xorshift32`] — the fixed-seed integer LFO stream behind MOD's random waveform
//! (accuracy policy D11) and, from IT (G3), its per-note-on random volume and pan
//! variation.

/// A 32-bit xorshift PRNG, stepped once per call to [`Xorshift32::next_u32`].
///
/// No floating point, no external entropy, no wall-clock seed: a stream started at the
/// same seed always produces the same sequence, which is what keeps fixed-point output
/// bit-identical across x86, ARM and WASM (architecture §7.3) and across repeated runs of
/// the same module.
///
/// A zero seed is folded to `1` at construction, because the all-zero state is a fixed
/// point of this xorshift and would produce nothing but zeroes forever. Every other seed
/// has period `2^32 - 1` and never reaches zero, so the fold-up needs to happen only once,
/// not on every step.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Xorshift32 {
    state: u32,
}

impl Xorshift32 {
    /// A stream seeded at `seed`, or at `1` if `seed` is zero.
    pub const fn new(seed: u32) -> Xorshift32 { Xorshift32 { state: if seed == 0 { 1 } else { seed } } }

    /// Advance the stream by one step and return the new state.
    pub const fn next_u32(&mut self) -> u32 {
        let mut state = self.state;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        self.state = state;
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first eight raw states MOD's waveform selector 3 produced before this type
    /// existed, captured from `starplayer_mod::processor::ModProcessor::waveform_value`
    /// (seed `0x6D2B_79F5`, the same xorshift32 body inlined there) by a throwaway test
    /// run against the pre-move code. `starplayer-mod`'s own test
    /// `random_waveform_is_deterministic_but_not_the_square_alias` and the MOD goldens are
    /// the proof that moving the code here changed nothing.
    #[test]
    fn reproduces_the_mod_processors_first_eight_values_before_the_move() {
        let mut stream = Xorshift32::new(0x6D2B_79F5);
        let values: alloc::vec::Vec<u32> = (0..8).map(|_| stream.next_u32()).collect();
        assert_eq!(
            values,
            alloc::vec![
                1_085_196_063,
                2_447_379_481,
                2_618_286_376,
                1_701_901_981,
                265_159_372,
                1_030_440_423,
                4_012_273_292,
                2_080_899_351,
            ]
        );
    }

    #[test]
    fn a_zero_seed_is_folded_to_one() {
        assert_eq!(Xorshift32::new(0), Xorshift32::new(1));
    }
}

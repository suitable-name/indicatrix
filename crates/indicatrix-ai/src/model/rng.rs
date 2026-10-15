//! Small deterministic pseudo-random generator used for weight initialisation, action sampling and
//! mini-batch shuffling.
//!
//! The generator is `SplitMix64`: every `u64` seed (including 0) yields a full-period, well-mixed
//! stream, and neighbouring seeds produce uncorrelated streams. It is not cryptographic.

/// Deterministic `SplitMix64` pseudo-random generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prng {
    state: u64,
}

impl Prng {
    /// Create a generator from `seed`. Every seed value, including 0, is valid.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Create a generator for sub-stream `stream` of `seed` (for example one stream per tier or per worker).
    #[must_use]
    pub const fn from_stream(seed: u64, stream: u64) -> Self {
        let mut mixer = Self::new(seed ^ stream.wrapping_mul(0xD1B5_4A32_D192_ED03));
        // Discard one output so streams of adjacent `stream` values start decorrelated.
        let first = mixer.next_u64();
        Self::new(first)
    }

    /// Next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_9B9B);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `f32` in `[0, 1)` with 24 bits of resolution.
    pub const fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 16_777_216.0
    }

    /// Uniform `f32` in `[-1, 1)`.
    pub const fn next_signed_f32(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Standard normal sample (Box-Muller, one value per call).
    pub fn standard_normal(&mut self) -> f32 {
        // u1 in (0, 1] so that ln(u1) is finite.
        let u1 = 1.0 - self.next_f32();
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }

    /// Uniform index in `[0, bound)`. `bound` must be non-zero; a zero bound returns 0.
    pub fn next_index(&mut self, bound: usize) -> usize {
        if bound <= 1 {
            return 0;
        }
        // Lemire's multiply-shift reduction; the bias is below 2^-64 * bound, irrelevant here.
        ((u128::from(self.next_u64()) * bound as u128) >> 64) as usize
    }

    /// In-place Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.next_index(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Prng;

    #[test]
    fn equal_seeds_agree_and_neighbouring_seeds_differ() {
        let mut a = Prng::new(7);
        let mut b = Prng::new(7);
        let mut c = Prng::new(8);
        for _ in 0..16 {
            let x = a.next_u64();
            assert_eq!(x, b.next_u64());
            assert_ne!(x, c.next_u64());
        }
    }

    #[test]
    fn zero_seed_is_not_degenerate() {
        let mut rng = Prng::new(0);
        let first = rng.next_u64();
        let second = rng.next_u64();
        assert_ne!(first, 0);
        assert_ne!(first, second);
    }

    #[test]
    fn unit_samples_stay_in_range_and_normal_is_finite() {
        let mut rng = Prng::new(1);
        for _ in 0..10_000 {
            let u = rng.next_f32();
            assert!((0.0..1.0).contains(&u));
            assert!(rng.standard_normal().is_finite());
        }
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut items: Vec<usize> = (0..50).collect();
        Prng::new(3).shuffle(&mut items);
        let mut sorted = items.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(items, sorted);
    }
}

//! Counter-based random numbers for the forward tracer.
//!
//! Every path owns a generator keyed by `(seed, view, pixel, sample, bin)`; its draws are
//! numbered (the draw counter is the path depth in effect: the tracer takes a fixed number of
//! draws per event, in event order). The numbers are `SplitMix64` hashes of the key and the
//! counter, so a path's randomness depends on nothing else: not the thread, the chunk, the
//! order in which pixels are processed, or the other paths. That makes the trace bitwise
//! independent of the number of threads.

const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// The `SplitMix64` output mix.
const fn finalize(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hashes a seed and four keys into one 64-bit value.
fn hash_keys(seed: u64, keys: [u64; 4]) -> u64 {
    let mut h = finalize(seed ^ 0xA076_1D64_78BD_642F);
    for key in keys {
        h = finalize(
            h.wrapping_add(GAMMA)
                .wrapping_add(key.wrapping_mul(0xE703_7ED1_A0B4_28DB)),
        );
    }
    h
}

/// A generator for one path.
#[derive(Debug, Clone, Copy)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// The generator of path `(view, pixel, sample, bin)`.
    #[must_use]
    pub fn keyed(seed: u64, view: u64, pixel: u64, sample: u64, bin: u64) -> Self {
        Self {
            state: hash_keys(seed, [view, pixel, sample, bin]),
        }
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GAMMA);
        finalize(self.state)
    }

    /// A uniform number in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// The low-discrepancy sample positions of one pixel: the R3 additive-recurrence sequence with
/// a per-pixel shift. Any prefix is well spread, so the adaptive passes simply continue the
/// sample index.
#[derive(Debug, Clone, Copy)]
pub struct PixelSequence {
    shift: [f64; 3],
}

impl PixelSequence {
    /// The sequence of `pixel` in `view`.
    #[must_use]
    pub fn new(seed: u64, view: u64, pixel: u64) -> Self {
        let mut rng = Rng::keyed(seed, view, pixel, u64::MAX, u64::MAX);
        Self {
            shift: [rng.next_f64(), rng.next_f64(), rng.next_f64()],
        }
    }

    /// Sample `index`: two coordinates over the footprint and one over the wavelength bin, each
    /// in `[0, 1)`.
    #[must_use]
    pub fn point(&self, index: u64) -> [f64; 3] {
        // The plastic-constant recurrence (Roberts, "The Unreasonable Effectiveness of
        // Quasirandom Sequences", R3).
        const STEP: [f64; 3] = [
            0.819_172_513_396_164_4,
            0.671_043_606_703_789_2,
            0.549_700_477_901_970_4,
        ];
        let n = (index + 1) as f64;
        let mut out = [0.0; 3];
        for i in 0..3 {
            let x = STEP[i].mul_add(n, self.shift[i]);
            out[i] = x - x.floor();
        }
        out
    }
}

//! Deterministic synthetic radiance frames (three little-endian `f32` XYZ values per pixel).
//!
//! Every generator is seeded with a fixed constant and uses no clock and no external file,
//! so two runs produce byte-identical frames. The statistics are shaped like what the
//! emitter puts on the wire, not copies of real renders:
//!
//! - [`DataSet::Sparse`]: a gem-shaped convex blob of noisy radiance with a smooth gradient
//!   on a mostly exactly-zero background (a few percent of background pixels hold tiny
//!   stray values).
//! - [`DataSet::Dense`]: path-tracing noise everywhere (log-normal per-channel noise around
//!   a smooth base, plus rare fireflies).
//! - [`DataSet::Converged`]: a high-sample-count frame, i.e. the smooth base with
//!   half-percent noise on the mantissas.
//! - [`DataSet::Delta`]: what a per-chunk `FRAME` delta holds: the SUM of [`DELTA_SPP`]
//!   independent samples per pixel, most of which hit nothing (the emitter sends deltas,
//!   not running totals, and the coordinator adds them).

/// Samples summed into one pixel of a [`DataSet::Delta`] frame.
pub const DELTA_SPP: u32 = 8;

/// Bytes per pixel on the wire (three `f32`).
pub const BYTES_PER_PIXEL: usize = 12;

/// The synthetic frame families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataSet {
    /// Gem blob on an (almost) empty background.
    Sparse,
    /// Heavy noise everywhere.
    Dense,
    /// Smooth, low-noise, converged.
    Converged,
    /// A coalesced per-chunk delta (few samples per pixel).
    Delta,
}

impl DataSet {
    /// Every data set, in report order.
    pub const ALL: [Self; 4] = [Self::Sparse, Self::Dense, Self::Converged, Self::Delta];

    /// Short stable name used in the table and the CSV.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Dense => "dense",
            Self::Converged => "converged",
            Self::Delta => "delta",
        }
    }

    /// The fixed RNG seed of this data set.
    const fn seed(self) -> u64 {
        match self {
            Self::Sparse => 0x5eed_0001_9e37_79b9,
            Self::Dense => 0x5eed_0002_9e37_79b9,
            Self::Converged => 0x5eed_0003_9e37_79b9,
            Self::Delta => 0x5eed_0004_9e37_79b9,
        }
    }
}

/// A small splitmix64 generator: fast, seedable, identical on every platform.
struct Rng(u64);

impl Rng {
    /// The next 64 random bits.
    const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn uniform(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Approximately standard normal (sum of four uniforms, rescaled).
    fn gauss(&mut self) -> f32 {
        let s = self.uniform() + self.uniform() + self.uniform() + self.uniform();
        (s - 2.0) * 1.732_050_8
    }
}

/// Number of sides of the convex gem outline.
const FACETS: usize = 8;

/// Geometry of the convex blob inside a `w x h` frame.
struct Blob {
    centre: (f32, f32),
    radius: (f32, f32),
    normals: [(f32, f32); FACETS],
}

impl Blob {
    /// A regular octagon-ish outline filling about a third of the frame.
    fn new(w: usize, h: usize) -> Self {
        let mut normals = [(0.0, 0.0); FACETS];
        for (k, n) in normals.iter_mut().enumerate() {
            let a = (k as f32 + 0.5) * std::f32::consts::TAU / FACETS as f32;
            *n = (a.cos(), a.sin());
        }
        Self {
            centre: (w as f32 * 0.5, h as f32 * 0.5),
            radius: (w as f32 * 0.34, h as f32 * 0.38),
            normals,
        }
    }

    /// `(normalised distance, facet)` of pixel `p`; distance below 1 is inside.
    fn locate(&self, p: Px) -> (f32, usize) {
        let dx = (p.x as f32 + 0.5 - self.centre.0) / self.radius.0;
        let dy = (p.y as f32 + 0.5 - self.centre.1) / self.radius.1;
        let mut best = (f32::MIN, 0);
        for (k, n) in self.normals.iter().enumerate() {
            let d = dy.mul_add(n.1, dx * n.0);
            if d > best.0 {
                best = (d, k);
            }
        }
        best
    }
}

/// A pixel position inside a frame of `w x h` pixels.
#[derive(Clone, Copy)]
struct Px {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Px {
    /// Horizontal position in `[0, 1)`.
    fn fx(self) -> f32 {
        self.x as f32 / self.w as f32
    }

    /// Vertical position in `[0, 1)`.
    fn fy(self) -> f32 {
        self.y as f32 / self.h as f32
    }
}

/// The smooth gem radiance at a pixel inside the blob: a gradient across the frame with a
/// per-facet brightness offset and a falloff towards the rim.
fn gem_base(blob: &Blob, p: Px) -> Option<[f32; 3]> {
    let (d, facet) = blob.locate(p);
    if d >= 1.0 {
        return None;
    }
    let facet_gain = 0.45_f32.mul_add(((facet * 3) % FACETS) as f32 / FACETS as f32, 0.55);
    let fall = (-0.5 * d).mul_add(d, 1.0);
    let gain = fall * facet_gain;
    Some([
        0.9_f32.mul_add(p.fx(), 0.25) * gain,
        0.8_f32.mul_add(p.fy(), 0.30) * gain,
        0.6_f32.mul_add(1.0 - p.fx(), 0.20) * gain,
    ])
}

/// A smooth full-frame base (a soft sky gradient with a gentle ripple).
fn smooth_base(p: Px) -> [f32; 3] {
    let ripple = 0.05 * (p.fx() * 9.0).sin() * (p.fy() * 7.0).cos();
    [
        0.4_f32.mul_add(p.fx(), 0.35) + ripple,
        0.3_f32.mul_add(p.fy(), 0.40) + ripple,
        (0.5 * (1.0 - p.fx())).mul_add(p.fy(), 0.30) + ripple,
    ]
}

/// Appends three `f32` values as little-endian bytes.
fn push_pixel(out: &mut Vec<u8>, v: [f32; 3]) {
    for c in v {
        out.extend_from_slice(&c.to_le_bytes());
    }
}

/// One pixel of the sparse frame.
fn sparse_pixel(rng: &mut Rng, blob: &Blob, p: Px) -> [f32; 3] {
    if let Some(base) = gem_base(blob, p) {
        base.map(|b| (b * 0.25_f32.mul_add(rng.gauss(), 1.0)).max(0.0))
    } else if rng.uniform() < 0.03 {
        [0.0; 3].map(|_| 1.0e-3 * rng.uniform())
    } else {
        [0.0; 3]
    }
}

/// One pixel of the dense noisy frame.
fn dense_pixel(rng: &mut Rng, p: Px) -> [f32; 3] {
    let firefly = if rng.uniform() < 1.0 / 2000.0 {
        50.0
    } else {
        1.0
    };
    smooth_base(p).map(|b| b * (0.8 * rng.gauss()).exp() * firefly)
}

/// One pixel of the converged smooth frame.
fn converged_pixel(rng: &mut Rng, p: Px) -> [f32; 3] {
    smooth_base(p).map(|b| b * 0.002_f32.mul_add(rng.gauss(), 1.0))
}

/// One pixel of the delta frame: the sum of [`DELTA_SPP`] samples, each a hit with a
/// probability that is high inside the gem and tiny outside.
fn delta_pixel(rng: &mut Rng, blob: &Blob, p: Px) -> [f32; 3] {
    let (base, hit) = gem_base(blob, p).map_or(([0.4, 0.45, 0.35], 0.01), |b| (b, 0.35));
    let mut sum = [0.0_f32; 3];
    for _ in 0..DELTA_SPP {
        if rng.uniform() < hit {
            let gain = (0.6 * rng.gauss()).exp();
            for (s, b) in sum.iter_mut().zip(base) {
                let jitter = 0.1_f32.mul_add(rng.gauss(), 1.0).max(0.0);
                *s = (b * gain).mul_add(jitter, *s);
            }
        }
    }
    sum
}

/// Generates a `w x h` frame of `set` as `w * h * 12` little-endian `f32` bytes.
pub fn generate(set: DataSet, w: usize, h: usize) -> Vec<u8> {
    let mut rng = Rng(set.seed());
    let blob = Blob::new(w, h);
    let mut out = Vec::with_capacity(w * h * BYTES_PER_PIXEL);
    for y in 0..h {
        for x in 0..w {
            let p = Px { x, y, w, h };
            let v = match set {
                DataSet::Sparse => sparse_pixel(&mut rng, &blob, p),
                DataSet::Dense => dense_pixel(&mut rng, p),
                DataSet::Converged => converged_pixel(&mut rng, p),
                DataSet::Delta => delta_pixel(&mut rng, &blob, p),
            };
            push_pixel(&mut out, v);
        }
    }
    out
}

/// Fraction of `f32` values in `raw` that are exactly +0.0.
pub fn zero_fraction(raw: &[u8]) -> f64 {
    let zeros = raw
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|v| **v == [0, 0, 0, 0])
        .count();
    zeros as f64 / (raw.len() / 4) as f64
}

/// Tone-maps a radiance frame to an opaque RGBA8 display frame (the XYZ triplet is used as
/// RGB; the exact color does not matter, only that the 8-bit image has the same spatial
/// structure and noise as the radiance it came from).
pub fn to_rgba8(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 3 * 4);
    for px in raw.as_chunks::<BYTES_PER_PIXEL>().0 {
        for c in px.as_chunks::<4>().0 {
            let v = f32::from_le_bytes(*c).max(0.0);
            let mapped = (1.0 - (-1.5 * v).exp()).powf(1.0 / 2.2);
            out.push(mapped.mul_add(255.0, 0.5) as u8);
        }
        out.push(255);
    }
    out
}

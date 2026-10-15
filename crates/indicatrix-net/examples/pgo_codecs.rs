//! PGO training workload for the `FRAME`/`PREVIEW` payload codecs of `indicatrix-net`.
//!
//! `indicatrix-net` depends on `indicatrix`, so `indicatrix`'s own `pgo_train` example cannot
//! link it, and `scene_roundtrip` only exercises the postcard scene codec. This example
//! covers the rest of the wire hot path under the same instrumentation: the byte shuffle,
//! zstd and LZ4 encode, and the decode both as a fresh buffer (`PREVIEW`) and summed onto an
//! accumulator (`FRAME` delta), at 1080p on a sparse gem-shaped frame and a dense noisy
//! frame, plus one 4K sparse frame. Each encoder and decoder is reused for two rounds so the
//! cached zstd contexts are trained too.
//!
//! The data is deterministic (fixed seeds, no files, no clock). Every round trip is checked
//! bit for bit; a mismatch exits non-zero so the PGO script stops instead of training on a
//! broken codec.
//!
//! ```text
//! cargo build --release -p indicatrix-net --features render --example pgo_codecs
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "synthetic frame generator: pixel indices and 24-bit random draws into f32"
)]

use glam::Vec3;
use indicatrix_net::{
    messages::PayloadEncoding,
    radiance::{self, PayloadDecoder, PayloadEncoder},
};
use std::process::ExitCode;

/// Encode/decode rounds per encoder, so the reused compression contexts are exercised.
const ROUNDS: usize = 2;

/// The encodings trained at 1080p (`Raw` covers the zero-copy path and the fallback).
const ENCODINGS_1080P: [PayloadEncoding; 4] = [
    PayloadEncoding::Raw,
    PayloadEncoding::ShuffleZstd { level: 1 },
    PayloadEncoding::ShuffleZstd { level: 3 },
    PayloadEncoding::ShuffleLz4,
];

/// The encodings trained once at 4K (the shipped default and the LAN opt-in).
const ENCODINGS_4K: [PayloadEncoding; 2] = [
    PayloadEncoding::ShuffleZstd { level: 1 },
    PayloadEncoding::ShuffleLz4,
];

/// A small splitmix64 generator: seedable and identical on every platform.
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
        (self.next_u64() >> 40) as f32 / 16_777_216.0
    }

    /// Approximately standard normal (sum of four uniforms, rescaled).
    fn gauss(&mut self) -> f32 {
        let s = self.uniform() + self.uniform() + self.uniform() + self.uniform();
        (s - 2.0) * 1.732_050_8
    }
}

/// Which synthetic frame family to generate.
#[derive(Clone, Copy)]
enum Kind {
    /// Gem-shaped noisy blob on a mostly exactly-zero background.
    Sparse,
    /// Log-normal path-tracing noise everywhere.
    Dense,
}

/// A smooth full-frame base colour at normalised position `(fx, fy)`.
fn base_color(fx: f32, fy: f32) -> Vec3 {
    Vec3::new(
        0.4f32.mul_add(fx, 0.35),
        0.3f32.mul_add(fy, 0.40),
        (0.5 * (1.0 - fx)).mul_add(fy, 0.30),
    )
}

/// One pixel of a frame of `kind`.
fn pixel(rng: &mut Rng, kind: Kind, (x, y): (usize, usize), (w, h): (usize, usize)) -> Vec3 {
    let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
    let base = base_color(fx, fy);
    match kind {
        Kind::Dense => base * (0.8 * rng.gauss()).exp(),
        Kind::Sparse => {
            let dx = (fx - 0.5) / 0.34;
            let dy = (fy - 0.5) / 0.38;
            if f32::mul_add(dy, dy, dx * dx) < 1.0 {
                base * 0.25f32.mul_add(rng.gauss(), 1.0).max(0.0)
            } else if rng.uniform() < 0.03 {
                Vec3::splat(1.0e-3 * rng.uniform())
            } else {
                Vec3::ZERO
            }
        }
    }
}

/// A deterministic `w x h` radiance frame.
fn frame(kind: Kind, seed: u64, w: usize, h: usize) -> Vec<Vec3> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(pixel(&mut rng, kind, (x, y), (w, h)));
        }
    }
    out
}

/// Encodes `pixels` with `encoding` for [`ROUNDS`] rounds and decodes each both ways,
/// checking bit-identical results. Returns the total encoded bytes as a checksum.
fn round_trip(
    pixels: &[Vec3],
    (w, h): (u32, u32),
    encoding: PayloadEncoding,
) -> Result<u64, String> {
    let raw = radiance::as_bytes(pixels);
    let mut encoder = PayloadEncoder::new(encoding);
    let mut decoder = PayloadDecoder::new();
    let mut total = 0u64;
    for _ in 0..ROUNDS {
        let payload = encoder.encode(raw);
        total += payload.bytes.len() as u64;
        let preview = decoder
            .decode_to_vec(payload.encoding, payload.raw_len, payload.bytes, w, h)
            .map_err(|e| format!("{encoding:?} decode_to_vec: {e:?}"))?;
        if preview.as_slice() != pixels {
            return Err(format!("{encoding:?}: decode_to_vec is not bit-identical"));
        }
        let mut acc = vec![Vec3::ZERO; pixels.len()];
        let dropped = decoder
            .decode_and_add(
                payload.encoding,
                payload.raw_len,
                payload.bytes,
                w,
                h,
                &mut acc,
            )
            .map_err(|e| format!("{encoding:?} decode_and_add: {e:?}"))?;
        if dropped != 0 || acc.as_slice() != pixels {
            return Err(format!("{encoding:?}: decode_and_add is not bit-identical"));
        }
    }
    Ok(total)
}

/// Runs every encoding in `encodings` over one generated frame.
fn train(
    kind: Kind,
    seed: u64,
    (w, h): (u32, u32),
    encodings: &[PayloadEncoding],
) -> Result<u64, String> {
    let pixels = frame(kind, seed, w as usize, h as usize);
    let mut sum = 0u64;
    for &encoding in encodings {
        sum = sum.wrapping_add(round_trip(&pixels, (w, h), encoding)?);
    }
    Ok(sum)
}

/// Trains 1080p sparse and dense, then 4K sparse once.
fn run() -> Result<u64, String> {
    let mut checksum = train(Kind::Sparse, 1, (1920, 1080), &ENCODINGS_1080P)?;
    checksum = checksum.wrapping_add(train(Kind::Dense, 2, (1920, 1080), &ENCODINGS_1080P)?);
    checksum = checksum.wrapping_add(train(Kind::Sparse, 3, (3840, 2160), &ENCODINGS_4K)?);
    Ok(checksum)
}

fn main() -> ExitCode {
    match run() {
        Ok(checksum) => {
            println!("pgo_codecs: all round trips bit-identical (encoded bytes {checksum})");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pgo_codecs: {e}");
            ExitCode::FAILURE
        }
    }
}

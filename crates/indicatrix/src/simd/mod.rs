//! Runtime-dispatched SIMD kernels (AVX2 / AVX-512, with a scalar fallback).
//!
//! Covers the two hot loops that dominate CPU time: the meet-solver's
//! candidate-vertex enumeration (`f64`) and the spectral raytracer's
//! plane-intersection and per-channel absorption math (`f32`).
//!
//! # Determinism contract
//!
//! Every kernel is **bit-identical across dispatch levels** (scalar, AVX2,
//! AVX-512) and to the scalar code it replaces, with one exception:
//! [`exp_f32x8`] is a polynomial exponential, bit-identical across its own
//! levels but deliberately **not** identical to `f32::exp` (libm) -- callers
//! switching to it change results by a couple of ULP, once, uniformly.
//!
//! The `f64`/`f32` geometry kernels replicate the exact operation order of the
//! `glam` scalar code (left-associated dot products, separate multiply and
//! add rather than FMA, per-lane decisions replayed in ascending
//! plane/triple order), so identical operation sequences give identical bits
//! at any vector width. [`exp_f32x8`] is the one FMA user, which is why the
//! dispatch levels also require the `fma` feature.
//!
//! No cross-lane floating-point reductions feed a decision: horizontal steps
//! only *select* existing lane values, ties broken toward the lowest
//! plane/triple index, matching the sequential scans they replace.

mod exp_poly;
mod feasibility;
mod slab;
mod triple_solve;

pub use exp_poly::exp_f32x8;
pub use feasibility::{BLANK_OWNER, Feasibility, PlanesSoA64, any_violation, classify_feasibility};
pub use slab::{PlanesSoA32, SlabScan, slab_scan};
pub use triple_solve::{TRIPLE_LANES, TripleBatch, TripleSolution, solve_triple_batch};

use std::sync::OnceLock;

/// Which instruction set the kernels dispatch to on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdLevel {
    Scalar,
    Avx2,
    Avx512,
}

static LEVEL: OnceLock<SimdLevel> = OnceLock::new();

/// Detected dispatch level, cached for the process lifetime.
///
/// `INDICATRIX_SIMD` (read once, at first call) can cap the level below what
/// the CPU supports: `scalar`, `avx2` or `avx512`; never raises it. Since
/// every kernel is bit-identical across levels, capping changes only speed --
/// it exists so PGO training (`scripts/pgo-build.ps1`) can target the scalar
/// kernels on an AVX-capable machine, and so a level can be A/B-timed in place.
pub fn simd_level() -> SimdLevel {
    *LEVEL.get_or_init(|| {
        let detected = detect_level();
        let cap = match std::env::var("INDICATRIX_SIMD").as_deref() {
            Ok("scalar") => SimdLevel::Scalar,
            Ok("avx2") => SimdLevel::Avx2,
            _ => SimdLevel::Avx512,
        };
        if rank(cap) < rank(detected) {
            cap
        } else {
            detected
        }
    })
}

const fn rank(level: SimdLevel) -> u8 {
    match level {
        SimdLevel::Scalar => 0,
        SimdLevel::Avx2 => 1,
        SimdLevel::Avx512 => 2,
    }
}

// Split rather than one function with an internal `#[cfg]`: `is_x86_feature_detected!`
// is a runtime CPUID check, not `const`-compatible, so only the non-x86_64 arm can be `const fn`.
#[cfg(target_arch = "x86_64")]
fn detect_level() -> SimdLevel {
    if std::arch::is_x86_feature_detected!("avx512f") && std::arch::is_x86_feature_detected!("fma")
    {
        return SimdLevel::Avx512;
    }
    if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
        return SimdLevel::Avx2;
    }
    SimdLevel::Scalar
}

#[cfg(not(target_arch = "x86_64"))]
const fn detect_level() -> SimdLevel {
    SimdLevel::Scalar
}

//! `f64` structure-of-arrays plane arena and per-plane feasibility
//! classification: the meet solver's owner-tracking candidate-vertex scan
//! ([`classify_feasibility`]) and `stone_metrics`'s order-independent
//! violation test ([`any_violation`]), each with its scalar reference and
//! AVX2/AVX-512 kernels.

use glam::DVec3;

// `SimdLevel` itself is named only inside this file's `#[cfg(target_arch = "x86_64")]`
// match arms (`SimdLevel::Avx512`/`SimdLevel::Avx2`) -- the scalar fallback arm below
// matches on it via `_`, never by name. `simd_level()` (the function that returns one)
// stays imported unconditionally since that fallback `match` calls it on every target.
// Splitting the import in two, rather than one `#[cfg]` on the whole `use` line, is
// what keeps this a targeted fix for the one name that goes unused elsewhere, instead
// of also hiding a real "you forgot to call `simd_level` on this target" mistake behind
// the same attribute.
#[cfg(target_arch = "x86_64")]
use super::SimdLevel;
use super::simd_level;

/// Owner value marking a bounding-blank plane in [`PlanesSoA64`].
pub const BLANK_OWNER: u32 = u32::MAX;

/// Structure-of-arrays mirror of a plane list (`n . x <= m`), built once per
/// arrangement and scanned millions of times by the feasibility kernels.
#[derive(Default)]
pub struct PlanesSoA64 {
    nx: Vec<f64>,
    ny: Vec<f64>,
    nz: Vec<f64>,
    m: Vec<f64>,
    owner: Vec<u32>,
}

impl PlanesSoA64 {
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            nx: Vec::with_capacity(cap),
            ny: Vec::with_capacity(cap),
            nz: Vec::with_capacity(cap),
            m: Vec::with_capacity(cap),
            owner: Vec::with_capacity(cap),
        }
    }

    pub fn push(&mut self, n: DVec3, m: f64, owner: u32) {
        self.nx.push(n.x);
        self.ny.push(n.y);
        self.nz.push(n.z);
        self.m.push(m);
        self.owner.push(owner);
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.nx.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nx.is_empty()
    }
}

/// Outcome of scanning one candidate point against every plane of an
/// arrangement, matching the meet solver's violated-owner semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feasibility {
    /// The point violates a blank plane, or planes of two distinct tiers.
    Dead,
    /// Feasible against everything (`None`) or violating exactly one tier.
    Ok(Option<u32>),
}

/// Scalar owner-tracking replay over one already-computed violation: shared
/// by every dispatch level so the decision sequence is identical everywhere.
#[inline]
const fn note_violation(owner: u32, violated: &mut Option<u32>) -> bool {
    if owner == BLANK_OWNER {
        return false;
    }
    if violated.is_none() {
        *violated = Some(owner);
        return true;
    }
    matches!(*violated, Some(prev) if prev == owner)
}

fn classify_feasibility_scalar(planes: &PlanesSoA64, v: DVec3, eps: f64) -> Feasibility {
    let mut violated: Option<u32> = None;
    for i in 0..planes.len() {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        if dot - planes.m[i] > eps && !note_violation(planes.owner[i], &mut violated) {
            return Feasibility::Dead;
        }
    }
    Feasibility::Ok(violated)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn classify_feasibility_avx2(planes: &PlanesSoA64, v: DVec3, eps: f64) -> Feasibility {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _mm256_add_pd, _mm256_cmp_pd, _mm256_loadu_pd, _mm256_movemask_pd,
        _mm256_mul_pd, _mm256_set1_pd, _mm256_sub_pd,
    };
    let n = planes.len();
    let vx = _mm256_set1_pd(v.x);
    let vy = _mm256_set1_pd(v.y);
    let vz = _mm256_set1_pd(v.z);
    let eps_v = _mm256_set1_pd(eps);
    let mut violated: Option<u32> = None;
    let mut i = 0usize;
    while i + 4 <= n {
        // SAFETY: i + 4 <= len for every array; loads stay in bounds.
        let (diff_mask, base) = unsafe {
            let nx = _mm256_loadu_pd(planes.nx.as_ptr().add(i));
            let ny = _mm256_loadu_pd(planes.ny.as_ptr().add(i));
            let nz = _mm256_loadu_pd(planes.nz.as_ptr().add(i));
            let m = _mm256_loadu_pd(planes.m.as_ptr().add(i));
            // ((nx*vx + ny*vy) + nz*vz) - m, matching glam's scalar dot order.
            let dot = _mm256_add_pd(
                _mm256_add_pd(_mm256_mul_pd(nx, vx), _mm256_mul_pd(ny, vy)),
                _mm256_mul_pd(nz, vz),
            );
            let diff = _mm256_sub_pd(dot, m);
            (
                _mm256_movemask_pd(_mm256_cmp_pd::<_CMP_GT_OQ>(diff, eps_v)),
                i,
            )
        };
        let mut bits = diff_mask as u32;
        while bits != 0 {
            let lane = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if !note_violation(planes.owner[base + lane], &mut violated) {
                return Feasibility::Dead;
            }
        }
        i += 4;
    }
    while i < n {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        if dot - planes.m[i] > eps && !note_violation(planes.owner[i], &mut violated) {
            return Feasibility::Dead;
        }
        i += 1;
    }
    Feasibility::Ok(violated)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn classify_feasibility_avx512(planes: &PlanesSoA64, v: DVec3, eps: f64) -> Feasibility {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _mm512_add_pd, _mm512_cmp_pd_mask, _mm512_loadu_pd, _mm512_mul_pd,
        _mm512_set1_pd, _mm512_sub_pd,
    };
    let n = planes.len();
    let vx = _mm512_set1_pd(v.x);
    let vy = _mm512_set1_pd(v.y);
    let vz = _mm512_set1_pd(v.z);
    let eps_v = _mm512_set1_pd(eps);
    let mut violated: Option<u32> = None;
    let mut i = 0usize;
    while i + 8 <= n {
        // SAFETY: i + 8 <= len for every array; loads stay in bounds.
        let mask = unsafe {
            let nx = _mm512_loadu_pd(planes.nx.as_ptr().add(i));
            let ny = _mm512_loadu_pd(planes.ny.as_ptr().add(i));
            let nz = _mm512_loadu_pd(planes.nz.as_ptr().add(i));
            let m = _mm512_loadu_pd(planes.m.as_ptr().add(i));
            let dot = _mm512_add_pd(
                _mm512_add_pd(_mm512_mul_pd(nx, vx), _mm512_mul_pd(ny, vy)),
                _mm512_mul_pd(nz, vz),
            );
            _mm512_cmp_pd_mask::<_CMP_GT_OQ>(_mm512_sub_pd(dot, m), eps_v)
        };
        let mut bits = u32::from(mask);
        while bits != 0 {
            let lane = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if !note_violation(planes.owner[i + lane], &mut violated) {
                return Feasibility::Dead;
            }
        }
        i += 8;
    }
    while i < n {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        if dot - planes.m[i] > eps && !note_violation(planes.owner[i], &mut violated) {
            return Feasibility::Dead;
        }
        i += 1;
    }
    Feasibility::Ok(violated)
}

/// Scans `v` against every plane in ascending index order, tracking which
/// tier(s) it violates -- the meet solver's candidate-vertex feasibility test.
///
/// Bit-identical to the scalar plane loop it replaces at every dispatch level.
#[must_use]
pub fn classify_feasibility(planes: &PlanesSoA64, v: DVec3, eps: f64) -> Feasibility {
    match simd_level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx512 => unsafe { classify_feasibility_avx512(planes, v, eps) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx2 => unsafe { classify_feasibility_avx2(planes, v, eps) },
        _ => classify_feasibility_scalar(planes, v, eps),
    }
}

/// `true` iff `v` violates any plane by more than `eps` -- the owner-free
/// variant used by `stone_metrics` (order-independent, a pure OR).
#[must_use]
pub fn any_violation(planes: &PlanesSoA64, v: DVec3, eps: f64) -> bool {
    // A pure disjunction: reuse the owner-tracking kernel by treating every
    // owner as distinct would change semantics, so scan directly.
    match simd_level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx512 => unsafe { any_violation_avx512(planes, v, eps) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx2 => unsafe { any_violation_avx2(planes, v, eps) },
        _ => any_violation_scalar(planes, v, eps),
    }
}

fn any_violation_scalar(planes: &PlanesSoA64, v: DVec3, eps: f64) -> bool {
    (0..planes.len()).any(|i| {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        dot - planes.m[i] > eps
    })
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn any_violation_avx2(planes: &PlanesSoA64, v: DVec3, eps: f64) -> bool {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _mm256_add_pd, _mm256_cmp_pd, _mm256_loadu_pd, _mm256_movemask_pd,
        _mm256_mul_pd, _mm256_set1_pd, _mm256_sub_pd,
    };
    let n = planes.len();
    let vx = _mm256_set1_pd(v.x);
    let vy = _mm256_set1_pd(v.y);
    let vz = _mm256_set1_pd(v.z);
    let eps_v = _mm256_set1_pd(eps);
    let mut i = 0usize;
    while i + 4 <= n {
        // SAFETY: i + 4 <= len for every array; loads stay in bounds.
        let mask = unsafe {
            let nx = _mm256_loadu_pd(planes.nx.as_ptr().add(i));
            let ny = _mm256_loadu_pd(planes.ny.as_ptr().add(i));
            let nz = _mm256_loadu_pd(planes.nz.as_ptr().add(i));
            let m = _mm256_loadu_pd(planes.m.as_ptr().add(i));
            let dot = _mm256_add_pd(
                _mm256_add_pd(_mm256_mul_pd(nx, vx), _mm256_mul_pd(ny, vy)),
                _mm256_mul_pd(nz, vz),
            );
            _mm256_movemask_pd(_mm256_cmp_pd::<_CMP_GT_OQ>(_mm256_sub_pd(dot, m), eps_v))
        };
        if mask != 0 {
            return true;
        }
        i += 4;
    }
    while i < n {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        if dot - planes.m[i] > eps {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn any_violation_avx512(planes: &PlanesSoA64, v: DVec3, eps: f64) -> bool {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _mm512_add_pd, _mm512_cmp_pd_mask, _mm512_loadu_pd, _mm512_mul_pd,
        _mm512_set1_pd, _mm512_sub_pd,
    };
    let n = planes.len();
    let vx = _mm512_set1_pd(v.x);
    let vy = _mm512_set1_pd(v.y);
    let vz = _mm512_set1_pd(v.z);
    let eps_v = _mm512_set1_pd(eps);
    let mut i = 0usize;
    while i + 8 <= n {
        // SAFETY: i + 8 <= len for every array; loads stay in bounds.
        let mask = unsafe {
            let nx = _mm512_loadu_pd(planes.nx.as_ptr().add(i));
            let ny = _mm512_loadu_pd(planes.ny.as_ptr().add(i));
            let nz = _mm512_loadu_pd(planes.nz.as_ptr().add(i));
            let m = _mm512_loadu_pd(planes.m.as_ptr().add(i));
            let dot = _mm512_add_pd(
                _mm512_add_pd(_mm512_mul_pd(nx, vx), _mm512_mul_pd(ny, vy)),
                _mm512_mul_pd(nz, vz),
            );
            _mm512_cmp_pd_mask::<_CMP_GT_OQ>(_mm512_sub_pd(dot, m), eps_v)
        };
        if mask != 0 {
            return true;
        }
        i += 8;
    }
    while i < n {
        let dot = DVec3::new(planes.nx[i], planes.ny[i], planes.nz[i]).dot(v);
        if dot - planes.m[i] > eps {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_planes64(seed: u64, count: usize) -> PlanesSoA64 {
        // Small deterministic LCG; no external RNG dependency.
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (((state >> 33) as f64) / f64::from(u32::MAX)).mul_add(2.0, -1.0)
        };
        let mut soa = PlanesSoA64::with_capacity(count);
        for i in 0..count {
            let n = DVec3::new(next(), next(), next()).normalize();
            let owner = match i % 7 {
                0 => BLANK_OWNER,
                k => (k as u32) % 3,
            };
            soa.push(n, next().abs() + 0.2, owner);
        }
        soa
    }

    /// Every dispatch level available on this machine must agree bit-for-bit
    /// with the scalar reference on the feasibility kernels.
    #[test]
    fn feasibility_kernels_match_scalar_bitwise() {
        for seed in 1..12u64 {
            for count in [1usize, 3, 4, 5, 7, 8, 9, 31, 64, 130] {
                let planes = test_planes64(seed, count);
                let v = DVec3::new(
                    (seed as f64).sin(),
                    (seed as f64).cos() * 0.5,
                    ((seed * 31) as f64).sin() * 0.8,
                );
                let eps = 1e-5;
                let want = classify_feasibility_scalar(&planes, v, eps);
                assert_eq!(classify_feasibility(&planes, v, eps), want);
                let want_any = any_violation_scalar(&planes, v, eps);
                assert_eq!(any_violation(&planes, v, eps), want_any);
                #[cfg(target_arch = "x86_64")]
                {
                    if std::arch::is_x86_feature_detected!("avx2") {
                        // SAFETY: guarded by runtime detection above.
                        unsafe { assert_eq!(classify_feasibility_avx2(&planes, v, eps), want) };
                        unsafe { assert_eq!(any_violation_avx2(&planes, v, eps), want_any) };
                    }
                    if std::arch::is_x86_feature_detected!("avx512f") {
                        // SAFETY: guarded by runtime detection above.
                        unsafe { assert_eq!(classify_feasibility_avx512(&planes, v, eps), want) };
                        unsafe { assert_eq!(any_violation_avx512(&planes, v, eps), want_any) };
                    }
                }
            }
        }
    }
}

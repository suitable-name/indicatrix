//! `f32` structure-of-arrays plane arena and slab-method ray/polyhedron
//! intersection: [`PlanesSoA32`] and [`slab_scan`], the spectral raytracer's
//! `intersect_polyhedron_soa` inner loop, with its scalar reference and
//! AVX2/AVX-512 kernels.

use glam::Vec3;

// See `feasibility.rs`'s identical split for why `SimdLevel` (named only inside this
// file's `#[cfg(target_arch = "x86_64")]` match arms) and `simd_level` (called
// unconditionally by the scalar-fallback `match` itself) need different visibility.
#[cfg(target_arch = "x86_64")]
use super::SimdLevel;
use super::simd_level;

/// Structure-of-arrays mirror of a `GpuFacetPlane` list for the slab kernel,
/// built once per traced ray and scanned once per bounce.
///
/// One backing allocation holds four consecutive `count`-long sections
/// (`nx | ny | nz | d`) -- a traced sample rebuilds this arena, so build cost
/// is one allocation, not four.
pub struct PlanesSoA32 {
    buf: Vec<f32>,
    count: usize,
}

impl PlanesSoA32 {
    /// Builds the arena from `(normal, d)` plane data.
    #[must_use]
    pub fn from_planes(planes: &[([f32; 3], f32)]) -> Self {
        Self::from_normals_d(planes.iter().copied(), planes.len())
    }

    /// Builds the arena from an iterator of `(normal, d)` pairs whose length
    /// is `count` (extra items are ignored, missing items stay zero).
    #[must_use]
    pub fn from_normals_d(iter: impl Iterator<Item = ([f32; 3], f32)>, count: usize) -> Self {
        let mut buf = vec![0f32; count * 4];
        for (i, (n, d)) in iter.take(count).enumerate() {
            buf[i] = n[0];
            buf[count + i] = n[1];
            buf[2 * count + i] = n[2];
            buf[3 * count + i] = d;
        }
        Self { buf, count }
    }

    fn nx(&self) -> &[f32] {
        &self.buf[..self.count]
    }

    fn ny(&self) -> &[f32] {
        &self.buf[self.count..2 * self.count]
    }

    fn nz(&self) -> &[f32] {
        &self.buf[2 * self.count..3 * self.count]
    }

    fn dvals(&self) -> &[f32] {
        &self.buf[3 * self.count..]
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The `i`-th plane's normal, bit-identical to the `AoS` original.
    #[must_use]
    pub fn normal(&self, i: usize) -> Vec3 {
        Vec3::new(self.nx()[i], self.ny()[i], self.nz()[i])
    }
}

/// Result of the slab scan over every plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlabScan {
    /// The ray travels (near-)parallel to some plane with its origin outside
    /// that half-space: the polyhedron intersection is empty.
    Outside,
    /// Running slab state after scanning every plane. Indices are `-1` when
    /// never updated (mirroring the scalar `Option`s being `None`).
    Slab {
        t_near: f32,
        near_idx: i32,
        t_far: f32,
        far_idx: i32,
    },
}

/// The scalar per-plane decision replay, shared verbatim by every dispatch
/// level: the vector paths precompute `denom`/`side`/`t` lanes and feed them
/// through this exact sequence in ascending plane order.
struct SlabState {
    t_near: f32,
    near_idx: i32,
    t_far: f32,
    far_idx: i32,
}

impl SlabState {
    const fn new() -> Self {
        Self {
            t_near: -1e30,
            near_idx: -1,
            t_far: 1e30,
            far_idx: -1,
        }
    }

    /// Returns `false` on the parallel-outside early exit.
    #[inline]
    fn step(&mut self, i: usize, denom: f32, side: f32, t: f32) -> bool {
        if denom.abs() > 1e-7 {
            if denom < 0.0 {
                if t > self.t_near {
                    self.t_near = t;
                    self.near_idx = i as i32;
                }
            } else if t < self.t_far {
                self.t_far = t;
                self.far_idx = i as i32;
            }
        } else if side > 0.0 {
            return false;
        }
        true
    }

    const fn finish(self) -> SlabScan {
        SlabScan::Slab {
            t_near: self.t_near,
            near_idx: self.near_idx,
            t_far: self.t_far,
            far_idx: self.far_idx,
        }
    }
}

fn slab_scan_scalar(planes: &PlanesSoA32, origin: Vec3, dir: Vec3) -> SlabScan {
    let mut st = SlabState::new();
    for i in 0..planes.len() {
        let normal = planes.normal(i);
        let denom = normal.dot(dir);
        let side = planes.dvals()[i] + normal.dot(origin);
        let t = (-side) / denom;
        if !st.step(i, denom, side, t) {
            return SlabScan::Outside;
        }
    }
    st.finish()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn hmax_ps256(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::{
        _mm_cvtss_f32, _mm_max_ps, _mm_movehl_ps, _mm_shuffle_ps, _mm256_castps256_ps128,
        _mm256_extractf128_ps,
    };
    let lo = _mm256_castps256_ps128(v);
    let hi = _mm256_extractf128_ps::<1>(v);
    let m = _mm_max_ps(lo, hi);
    let m = _mm_max_ps(m, _mm_movehl_ps(m, m));
    let m = _mm_max_ps(m, _mm_shuffle_ps::<0b01>(m, m));
    _mm_cvtss_f32(m)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn hmin_ps256(v: std::arch::x86_64::__m256) -> f32 {
    use std::arch::x86_64::{
        _mm_cvtss_f32, _mm_min_ps, _mm_movehl_ps, _mm_shuffle_ps, _mm256_castps256_ps128,
        _mm256_extractf128_ps,
    };
    let lo = _mm256_castps256_ps128(v);
    let hi = _mm256_extractf128_ps::<1>(v);
    let m = _mm_min_ps(lo, hi);
    let m = _mm_min_ps(m, _mm_movehl_ps(m, m));
    let m = _mm_min_ps(m, _mm_shuffle_ps::<0b01>(m, m));
    _mm_cvtss_f32(m)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn slab_scan_avx2(planes: &PlanesSoA32, origin: Vec3, dir: Vec3) -> SlabScan {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _CMP_LE_OQ, _CMP_LT_OQ, _mm256_add_ps, _mm256_and_ps, _mm256_andnot_ps,
        _mm256_blendv_ps, _mm256_cmp_ps, _mm256_div_ps, _mm256_loadu_ps, _mm256_movemask_ps,
        _mm256_mul_ps, _mm256_set1_ps, _mm256_setzero_ps, _mm256_storeu_ps, _mm256_sub_ps,
    };
    let n = planes.len();
    let dx = _mm256_set1_ps(dir.x);
    let dy = _mm256_set1_ps(dir.y);
    let dz = _mm256_set1_ps(dir.z);
    let ox = _mm256_set1_ps(origin.x);
    let oy = _mm256_set1_ps(origin.y);
    let oz = _mm256_set1_ps(origin.z);
    let thr = _mm256_set1_ps(1e-7);
    let sign_mask = _mm256_set1_ps(-0.0);
    let neg_inf = _mm256_set1_ps(f32::NEG_INFINITY);
    let pos_inf = _mm256_set1_ps(f32::INFINITY);
    let mut st = SlabState::new();
    let mut denom_l = [0f32; 8];
    let mut side_l = [0f32; 8];
    let mut t_l = [0f32; 8];
    let mut i = 0usize;
    let (nx_a, ny_a, nz_a, d_a) = (planes.nx(), planes.ny(), planes.nz(), planes.dvals());
    while i + 8 <= n {
        // SAFETY: i + 8 <= len for every section; loads/stores stay in bounds.
        let interesting = unsafe {
            let nx = _mm256_loadu_ps(nx_a.as_ptr().add(i));
            let ny = _mm256_loadu_ps(ny_a.as_ptr().add(i));
            let nz = _mm256_loadu_ps(nz_a.as_ptr().add(i));
            let d = _mm256_loadu_ps(d_a.as_ptr().add(i));
            let denom = _mm256_add_ps(
                _mm256_add_ps(_mm256_mul_ps(nx, dx), _mm256_mul_ps(ny, dy)),
                _mm256_mul_ps(nz, dz),
            );
            let dot_o = _mm256_add_ps(
                _mm256_add_ps(_mm256_mul_ps(nx, ox), _mm256_mul_ps(ny, oy)),
                _mm256_mul_ps(nz, oz),
            );
            let side = _mm256_add_ps(d, dot_o);
            let t = _mm256_div_ps(_mm256_sub_ps(_mm256_setzero_ps(), side), denom);
            // Fast-path classification: a block only needs the exact scalar
            // replay when some lane could change the slab state. Skipped
            // blocks change nothing, so bit-identity is untouched.
            let abs_denom = _mm256_andnot_ps(sign_mask, denom);
            let valid = _mm256_cmp_ps::<_CMP_GT_OQ>(abs_denom, thr);
            let invalid = _mm256_cmp_ps::<_CMP_LE_OQ>(abs_denom, thr);
            let par_bad = _mm256_and_ps(
                invalid,
                _mm256_cmp_ps::<_CMP_GT_OQ>(side, _mm256_setzero_ps()),
            );
            if _mm256_movemask_ps(par_bad) != 0 {
                // A near-parallel plane with the origin outside its half-space
                // empties the intersection regardless of scan position.
                return SlabScan::Outside;
            }
            let entering = _mm256_and_ps(
                valid,
                _mm256_cmp_ps::<_CMP_LT_OQ>(denom, _mm256_setzero_ps()),
            );
            let exiting = _mm256_andnot_ps(entering, valid);
            let ent_max = hmax_ps256(_mm256_blendv_ps(neg_inf, t, entering));
            let ex_min = hmin_ps256(_mm256_blendv_ps(pos_inf, t, exiting));
            let interesting = ent_max > st.t_near || ex_min < st.t_far;
            if interesting {
                _mm256_storeu_ps(denom_l.as_mut_ptr(), denom);
                _mm256_storeu_ps(side_l.as_mut_ptr(), side);
                _mm256_storeu_ps(t_l.as_mut_ptr(), t);
            }
            interesting
        };
        if interesting {
            for lane in 0..8 {
                if !st.step(i + lane, denom_l[lane], side_l[lane], t_l[lane]) {
                    return SlabScan::Outside;
                }
            }
        }
        i += 8;
    }
    while i < n {
        let normal = planes.normal(i);
        let denom = normal.dot(dir);
        let side = planes.dvals()[i] + normal.dot(origin);
        let t = (-side) / denom;
        if !st.step(i, denom, side, t) {
            return SlabScan::Outside;
        }
        i += 1;
    }
    st.finish()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn slab_scan_avx512(planes: &PlanesSoA32, origin: Vec3, dir: Vec3) -> SlabScan {
    use std::arch::x86_64::{
        _CMP_GT_OQ, _CMP_LE_OQ, _CMP_LT_OQ, _mm512_add_ps, _mm512_and_si512, _mm512_castps_si512,
        _mm512_castsi512_ps, _mm512_cmp_ps_mask, _mm512_div_ps, _mm512_loadu_ps,
        _mm512_mask_blend_ps, _mm512_mul_ps, _mm512_reduce_max_ps, _mm512_reduce_min_ps,
        _mm512_set1_epi32, _mm512_set1_ps, _mm512_setzero_ps, _mm512_storeu_ps, _mm512_sub_ps,
    };
    let n = planes.len();
    let dx = _mm512_set1_ps(dir.x);
    let dy = _mm512_set1_ps(dir.y);
    let dz = _mm512_set1_ps(dir.z);
    let ox = _mm512_set1_ps(origin.x);
    let oy = _mm512_set1_ps(origin.y);
    let oz = _mm512_set1_ps(origin.z);
    let thr = _mm512_set1_ps(1e-7);
    let abs_mask = _mm512_set1_epi32(0x7FFF_FFFF);
    let neg_inf = _mm512_set1_ps(f32::NEG_INFINITY);
    let pos_inf = _mm512_set1_ps(f32::INFINITY);
    let mut st = SlabState::new();
    let mut denom_l = [0f32; 16];
    let mut side_l = [0f32; 16];
    let mut t_l = [0f32; 16];
    let mut i = 0usize;
    let (nx_a, ny_a, nz_a, d_a) = (planes.nx(), planes.ny(), planes.nz(), planes.dvals());
    while i + 16 <= n {
        // SAFETY: i + 16 <= len for every section; loads/stores stay in bounds.
        let interesting = unsafe {
            let nx = _mm512_loadu_ps(nx_a.as_ptr().add(i));
            let ny = _mm512_loadu_ps(ny_a.as_ptr().add(i));
            let nz = _mm512_loadu_ps(nz_a.as_ptr().add(i));
            let d = _mm512_loadu_ps(d_a.as_ptr().add(i));
            let denom = _mm512_add_ps(
                _mm512_add_ps(_mm512_mul_ps(nx, dx), _mm512_mul_ps(ny, dy)),
                _mm512_mul_ps(nz, dz),
            );
            let dot_o = _mm512_add_ps(
                _mm512_add_ps(_mm512_mul_ps(nx, ox), _mm512_mul_ps(ny, oy)),
                _mm512_mul_ps(nz, oz),
            );
            let side = _mm512_add_ps(d, dot_o);
            let t = _mm512_div_ps(_mm512_sub_ps(_mm512_setzero_ps(), side), denom);
            // Fast-path classification, mirroring the AVX2 body (see there).
            let abs_denom =
                _mm512_castsi512_ps(_mm512_and_si512(_mm512_castps_si512(denom), abs_mask));
            let valid = _mm512_cmp_ps_mask::<_CMP_GT_OQ>(abs_denom, thr);
            let invalid = _mm512_cmp_ps_mask::<_CMP_LE_OQ>(abs_denom, thr);
            let side_pos = _mm512_cmp_ps_mask::<_CMP_GT_OQ>(side, _mm512_setzero_ps());
            if invalid & side_pos != 0 {
                return SlabScan::Outside;
            }
            let entering = valid & _mm512_cmp_ps_mask::<_CMP_LT_OQ>(denom, _mm512_setzero_ps());
            let exiting = valid & !entering;
            let ent_max = _mm512_reduce_max_ps(_mm512_mask_blend_ps(entering, neg_inf, t));
            let ex_min = _mm512_reduce_min_ps(_mm512_mask_blend_ps(exiting, pos_inf, t));
            let interesting = ent_max > st.t_near || ex_min < st.t_far;
            if interesting {
                _mm512_storeu_ps(denom_l.as_mut_ptr(), denom);
                _mm512_storeu_ps(side_l.as_mut_ptr(), side);
                _mm512_storeu_ps(t_l.as_mut_ptr(), t);
            }
            interesting
        };
        if interesting {
            for lane in 0..16 {
                if !st.step(i + lane, denom_l[lane], side_l[lane], t_l[lane]) {
                    return SlabScan::Outside;
                }
            }
        }
        i += 16;
    }
    while i < n {
        let normal = planes.normal(i);
        let denom = normal.dot(dir);
        let side = planes.dvals()[i] + normal.dot(origin);
        let t = (-side) / denom;
        if !st.step(i, denom, side, t) {
            return SlabScan::Outside;
        }
        i += 1;
    }
    st.finish()
}

/// Slab-method scan of a ray against every plane in ascending index order --
/// the raytracer's `intersect_polyhedron` inner loop.
///
/// The vector paths
/// compute `denom`/`side`/`t` lanes with the exact scalar operation order and
/// replay the per-plane decisions sequentially, so the result is
/// bit-identical to the scalar loop at every dispatch level.
#[must_use]
pub fn slab_scan(planes: &PlanesSoA32, origin: Vec3, dir: Vec3) -> SlabScan {
    match simd_level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx512 => unsafe { slab_scan_avx512(planes, origin, dir) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx2 => unsafe { slab_scan_avx2(planes, origin, dir) },
        _ => slab_scan_scalar(planes, origin, dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slab scan must agree bit-for-bit with the scalar replay at every
    /// available dispatch level, including the parallel-outside early exit
    /// and first-strict-improver tie semantics.
    #[test]
    fn slab_scan_matches_scalar_bitwise() {
        let mut state = 7u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (((state >> 33) as f32) / (u32::MAX as f32)).mul_add(2.0, -1.0)
        };
        for count in [1usize, 5, 8, 9, 16, 17, 40, 100] {
            for _round in 0..20 {
                let mut raw: Vec<([f32; 3], f32)> = Vec::new();
                for i in 0..count {
                    // Sprinkle in exactly-parallel planes to exercise the
                    // parallel-reject branch.
                    let n = if i % 9 == 3 {
                        [0.0, 1.0, 0.0]
                    } else {
                        [next(), next(), next()]
                    };
                    let len = f32::mul_add(n[2], n[2], f32::mul_add(n[1], n[1], n[0] * n[0]))
                        .sqrt()
                        .max(1e-3);
                    raw.push(([n[0] / len, n[1] / len, n[2] / len], next()));
                }
                let soa = PlanesSoA32::from_planes(&raw);
                let origin = Vec3::new(next() * 3.0, next() * 3.0, next() * 3.0);
                let dir = Vec3::new(0.0, -1.0, 0.0);
                let want = slab_scan_scalar(&soa, origin, dir);
                let got = slab_scan(&soa, origin, dir);
                assert_eq!(bits_of(got), bits_of(want));
                #[cfg(target_arch = "x86_64")]
                {
                    if std::arch::is_x86_feature_detected!("avx2") {
                        // SAFETY: guarded by runtime detection above.
                        unsafe {
                            assert_eq!(bits_of(slab_scan_avx2(&soa, origin, dir)), bits_of(want));
                        };
                    }
                    if std::arch::is_x86_feature_detected!("avx512f") {
                        // SAFETY: guarded by runtime detection above.
                        unsafe {
                            assert_eq!(bits_of(slab_scan_avx512(&soa, origin, dir)), bits_of(want));
                        };
                    }
                }
            }
        }
    }

    fn bits_of(scan: SlabScan) -> (u32, i32, u32, i32, bool) {
        match scan {
            SlabScan::Outside => (0, 0, 0, 0, true),
            SlabScan::Slab {
                t_near,
                near_idx,
                t_far,
                far_idx,
            } => (t_near.to_bits(), near_idx, t_far.to_bits(), far_idx, false),
        }
    }
}

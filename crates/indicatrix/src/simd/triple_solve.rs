//! Batched `f64` plane-triple solves: [`TripleBatch`]/[`TripleSolution`] and
//! [`solve_triple_batch`], with the `glam`-sequence scalar reference the
//! AVX2/AVX-512 lane program is tested bit-identical against.

use glam::DVec3;

// See `feasibility.rs`'s identical split for why `SimdLevel` (named only inside this
// file's `#[cfg(target_arch = "x86_64")]` match arms) and `simd_level` (called
// unconditionally by the scalar-fallback `match` itself) need different visibility.
#[cfg(target_arch = "x86_64")]
use super::SimdLevel;
use super::simd_level;

/// Lanes per triple batch (the AVX-512 width; AVX2 runs it as two half-width
/// passes, scalar as a loop).
pub const TRIPLE_LANES: usize = 8;

/// Up to [`TRIPLE_LANES`] plane triples in structure-of-arrays layout:
/// `a`/`b`/`c` are the three planes' unit normals, `ma`/`mb`/`mc` their
/// offsets.
#[derive(Default, Clone)]
pub struct TripleBatch {
    pub ax: [f64; TRIPLE_LANES],
    pub ay: [f64; TRIPLE_LANES],
    pub az: [f64; TRIPLE_LANES],
    pub bx: [f64; TRIPLE_LANES],
    pub by: [f64; TRIPLE_LANES],
    pub bz: [f64; TRIPLE_LANES],
    pub cx: [f64; TRIPLE_LANES],
    pub cy: [f64; TRIPLE_LANES],
    pub cz: [f64; TRIPLE_LANES],
    pub ma: [f64; TRIPLE_LANES],
    pub mb: [f64; TRIPLE_LANES],
    pub mc: [f64; TRIPLE_LANES],
    pub len: usize,
}

impl TripleBatch {
    /// Adds one triple; returns `true` when the batch is full.
    pub const fn push(&mut self, a: (DVec3, f64), b: (DVec3, f64), c: (DVec3, f64)) -> bool {
        let i = self.len;
        self.ax[i] = a.0.x;
        self.ay[i] = a.0.y;
        self.az[i] = a.0.z;
        self.bx[i] = b.0.x;
        self.by[i] = b.0.y;
        self.bz[i] = b.0.z;
        self.cx[i] = c.0.x;
        self.cy[i] = c.0.y;
        self.cz[i] = c.0.z;
        self.ma[i] = a.1;
        self.mb[i] = b.1;
        self.mc[i] = c.1;
        self.len = i + 1;
        self.len == TRIPLE_LANES
    }
}

/// Per-lane determinant and intersection point of a solved [`TripleBatch`].
/// Lanes past `len`, and lanes whose `|det|` the caller rejects, hold garbage.
pub struct TripleSolution {
    pub det: [f64; TRIPLE_LANES],
    pub vx: [f64; TRIPLE_LANES],
    pub vy: [f64; TRIPLE_LANES],
    pub vz: [f64; TRIPLE_LANES],
}

/// One lane of the triple solve, replicating the exact `glam` sequence the
/// solver's scalar code performs: `DMat3::from_cols(a, b, c).transpose()`,
/// `.determinant()`, `.inverse() * DVec3::new(ma, mb, mc)`. This IS the
/// scalar fallback and the reference the vector paths are tested against.
fn solve_triple_lane_reference(a: DVec3, b: DVec3, c: DVec3, rhs: DVec3) -> (f64, DVec3) {
    use glam::DMat3;
    let m = DMat3::from_cols(a, b, c).transpose();
    let det = m.determinant();
    (det, m.inverse() * rhs)
}

/// The expanded lane program equivalent to [`solve_triple_lane_reference`],
/// used by the vector paths (each intrinsic op mirrors one scalar op below,
/// so per-lane results are bit-identical to the `glam` sequence).
///
/// With `u = (ax,bx,cx)`, `v = (ay,by,cy)`, `w = (az,bz,cz)` (the transposed
/// matrix's axes), glam computes: `tmp2 = u x v`, `det = w . tmp2`,
/// `tmp0 = v x w`, `tmp1 = w x u`, `inv_det = 1/det`, and the solution's
/// components are `res.i = ((tmpI.x*inv_det)*ma + (tmpI.y*inv_det)*mb) +
/// (tmpI.z*inv_det)*mc`.
///
/// `#[cfg(target_arch = "x86_64")]`: both of this macro's call sites (the AVX2 and
/// AVX-512 lane kernels below) are themselves gated to that architecture -- there is
/// no scalar or other-architecture use of it, so on any other target the macro
/// definition itself would otherwise be a genuinely unused item.
#[cfg(target_arch = "x86_64")]
macro_rules! triple_lane_program {
    ($t:ty, $mul:expr, $add:expr, $sub:expr, $div1:expr,
     $ax:expr, $ay:expr, $az:expr, $bx:expr, $by:expr, $bz:expr,
     $cx:expr, $cy:expr, $cz:expr, $ma:expr, $mb:expr, $mc:expr) => {{
        let mul = $mul;
        let add = $add;
        let sub = $sub;
        let div1 = $div1;
        // u = (ax,bx,cx), v = (ay,by,cy), w = (az,bz,cz)
        // tmp2 = u x v  (glam cross: (u.y*v.z - v.y*u.z, u.z*v.x - v.z*u.x, u.x*v.y - v.x*u.y))
        let t2x = sub(mul($bx, $cy), mul($by, $cx));
        let t2y = sub(mul($cx, $ay), mul($cy, $ax));
        let t2z = sub(mul($ax, $by), mul($ay, $bx));
        // det = w . tmp2 = ((w.x*t2x) + (w.y*t2y)) + (w.z*t2z)
        let det = add(add(mul($az, t2x), mul($bz, t2y)), mul($cz, t2z));
        // tmp0 = v x w
        let t0x = sub(mul($by, $cz), mul($bz, $cy));
        let t0y = sub(mul($cy, $az), mul($cz, $ay));
        let t0z = sub(mul($ay, $bz), mul($az, $by));
        // tmp1 = w x u
        let t1x = sub(mul($bz, $cx), mul($bx, $cz));
        let t1y = sub(mul($cz, $ax), mul($cx, $az));
        let t1z = sub(mul($az, $bx), mul($ax, $bz));
        let inv_det = div1(det);
        let vx = add(
            add(mul(mul(t0x, inv_det), $ma), mul(mul(t0y, inv_det), $mb)),
            mul(mul(t0z, inv_det), $mc),
        );
        let vy = add(
            add(mul(mul(t1x, inv_det), $ma), mul(mul(t1y, inv_det), $mb)),
            mul(mul(t1z, inv_det), $mc),
        );
        let vz = add(
            add(mul(mul(t2x, inv_det), $ma), mul(mul(t2y, inv_det), $mb)),
            mul(mul(t2z, inv_det), $mc),
        );
        (det, vx, vy, vz)
    }};
}

fn solve_triple_batch_scalar(batch: &TripleBatch) -> TripleSolution {
    let mut out = TripleSolution {
        det: [0.0; TRIPLE_LANES],
        vx: [0.0; TRIPLE_LANES],
        vy: [0.0; TRIPLE_LANES],
        vz: [0.0; TRIPLE_LANES],
    };
    for i in 0..batch.len {
        let (det, v) = solve_triple_lane_reference(
            DVec3::new(batch.ax[i], batch.ay[i], batch.az[i]),
            DVec3::new(batch.bx[i], batch.by[i], batch.bz[i]),
            DVec3::new(batch.cx[i], batch.cy[i], batch.cz[i]),
            DVec3::new(batch.ma[i], batch.mb[i], batch.mc[i]),
        );
        out.det[i] = det;
        out.vx[i] = v.x;
        out.vy[i] = v.y;
        out.vz[i] = v.z;
    }
    out
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn solve_triple_batch_avx2(batch: &TripleBatch) -> TripleSolution {
    use std::arch::x86_64::{
        _mm256_add_pd, _mm256_div_pd, _mm256_loadu_pd, _mm256_mul_pd, _mm256_set1_pd,
        _mm256_storeu_pd, _mm256_sub_pd,
    };
    let mut out = TripleSolution {
        det: [0.0; TRIPLE_LANES],
        vx: [0.0; TRIPLE_LANES],
        vy: [0.0; TRIPLE_LANES],
        vz: [0.0; TRIPLE_LANES],
    };
    for half in 0..2usize {
        let o = half * 4;
        if o >= batch.len {
            break;
        }
        // SAFETY: every array is TRIPLE_LANES (8) long; o is 0 or 4, so all
        // 4-wide loads/stores at offset o stay in bounds.
        unsafe {
            let ld = |arr: &[f64; TRIPLE_LANES]| _mm256_loadu_pd(arr.as_ptr().add(o));
            let (ax, ay, az) = (ld(&batch.ax), ld(&batch.ay), ld(&batch.az));
            let (bx, by, bz) = (ld(&batch.bx), ld(&batch.by), ld(&batch.bz));
            let (cx, cy, cz) = (ld(&batch.cx), ld(&batch.cy), ld(&batch.cz));
            let (ma, mb, mc) = (ld(&batch.ma), ld(&batch.mb), ld(&batch.mc));
            let one = _mm256_set1_pd(1.0);
            let (det, vx, vy, vz) = triple_lane_program!(
                __m256d,
                |a, b| _mm256_mul_pd(a, b),
                |a, b| _mm256_add_pd(a, b),
                |a, b| _mm256_sub_pd(a, b),
                |d| _mm256_div_pd(one, d),
                ax,
                ay,
                az,
                bx,
                by,
                bz,
                cx,
                cy,
                cz,
                ma,
                mb,
                mc
            );
            _mm256_storeu_pd(out.det.as_mut_ptr().add(o), det);
            _mm256_storeu_pd(out.vx.as_mut_ptr().add(o), vx);
            _mm256_storeu_pd(out.vy.as_mut_ptr().add(o), vy);
            _mm256_storeu_pd(out.vz.as_mut_ptr().add(o), vz);
        }
    }
    out
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn solve_triple_batch_avx512(batch: &TripleBatch) -> TripleSolution {
    use std::arch::x86_64::{
        _mm512_add_pd, _mm512_div_pd, _mm512_loadu_pd, _mm512_mul_pd, _mm512_set1_pd,
        _mm512_storeu_pd, _mm512_sub_pd,
    };
    let mut out = TripleSolution {
        det: [0.0; TRIPLE_LANES],
        vx: [0.0; TRIPLE_LANES],
        vy: [0.0; TRIPLE_LANES],
        vz: [0.0; TRIPLE_LANES],
    };
    // SAFETY: every array is exactly TRIPLE_LANES (8) f64s -- one full zmm load.
    unsafe {
        let ld = |arr: &[f64; TRIPLE_LANES]| _mm512_loadu_pd(arr.as_ptr());
        let (ax, ay, az) = (ld(&batch.ax), ld(&batch.ay), ld(&batch.az));
        let (bx, by, bz) = (ld(&batch.bx), ld(&batch.by), ld(&batch.bz));
        let (cx, cy, cz) = (ld(&batch.cx), ld(&batch.cy), ld(&batch.cz));
        let (ma, mb, mc) = (ld(&batch.ma), ld(&batch.mb), ld(&batch.mc));
        let one = _mm512_set1_pd(1.0);
        let (det, vx, vy, vz) = triple_lane_program!(
            __m512d,
            |a, b| _mm512_mul_pd(a, b),
            |a, b| _mm512_add_pd(a, b),
            |a, b| _mm512_sub_pd(a, b),
            |d| _mm512_div_pd(one, d),
            ax,
            ay,
            az,
            bx,
            by,
            bz,
            cx,
            cy,
            cz,
            ma,
            mb,
            mc
        );
        _mm512_storeu_pd(out.det.as_mut_ptr(), det);
        _mm512_storeu_pd(out.vx.as_mut_ptr(), vx);
        _mm512_storeu_pd(out.vy.as_mut_ptr(), vy);
        _mm512_storeu_pd(out.vz.as_mut_ptr(), vz);
    }
    out
}

/// Solves every lane of `batch` (determinant + intersection point),
/// bit-identical per lane to the `glam` `DMat3` sequence the solver's scalar
/// code performs.
///
/// The caller applies its own determinant threshold and bounds checks per
/// lane, in ascending lane order, exactly as the scalar loop did.
#[must_use]
pub fn solve_triple_batch(batch: &TripleBatch) -> TripleSolution {
    match simd_level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx512 => unsafe { solve_triple_batch_avx512(batch) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: dispatch reached only after runtime feature detection.
        SimdLevel::Avx2 => unsafe { solve_triple_batch_avx2(batch) },
        _ => solve_triple_batch_scalar(batch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The batched triple solve must reproduce the `glam` `DMat3` sequence
    /// bit-for-bit on every lane, at every available dispatch level.
    #[test]
    fn triple_solve_matches_glam_bitwise() {
        let mut state = 42u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            (((state >> 33) as f64) / f64::from(u32::MAX)).mul_add(2.0, -1.0)
        };
        for _round in 0..50 {
            let mut batch = TripleBatch::default();
            let mut refs: Vec<(f64, DVec3)> = Vec::new();
            for _lane in 0..TRIPLE_LANES {
                let a = (DVec3::new(next(), next(), next()).normalize(), next());
                let b = (DVec3::new(next(), next(), next()).normalize(), next());
                let c = (DVec3::new(next(), next(), next()).normalize(), next());
                refs.push(solve_triple_lane_reference(
                    a.0,
                    b.0,
                    c.0,
                    DVec3::new(a.1, b.1, c.1),
                ));
                batch.push(a, b, c);
            }
            let check = |sol: &TripleSolution| {
                for (lane, (det, v)) in refs.iter().enumerate() {
                    assert_eq!(sol.det[lane].to_bits(), det.to_bits(), "det lane {lane}");
                    assert_eq!(sol.vx[lane].to_bits(), v.x.to_bits(), "vx lane {lane}");
                    assert_eq!(sol.vy[lane].to_bits(), v.y.to_bits(), "vy lane {lane}");
                    assert_eq!(sol.vz[lane].to_bits(), v.z.to_bits(), "vz lane {lane}");
                }
            };
            check(&solve_triple_batch_scalar(&batch));
            check(&solve_triple_batch(&batch));
            #[cfg(target_arch = "x86_64")]
            {
                if std::arch::is_x86_feature_detected!("avx2") {
                    // SAFETY: guarded by runtime detection above.
                    unsafe { check(&solve_triple_batch_avx2(&batch)) };
                }
                if std::arch::is_x86_feature_detected!("avx512f") {
                    // SAFETY: guarded by runtime detection above.
                    unsafe { check(&solve_triple_batch_avx512(&batch)) };
                }
            }
        }
    }
}

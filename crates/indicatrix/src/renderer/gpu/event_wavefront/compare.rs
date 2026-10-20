//! Pixel-wise comparison of two accumulated frames under the §7.2 golden rule (G-D10),
//! used by the event wavefront vs megakernel equivalence check.
//!
//! Pure CPU code: testable without a GPU. NaN or Inf in either frame is always a hard
//! failure (`NaN > tol` is false, so a naive threshold test would let it through).

use glam::Vec3;

/// Outcome of comparing two frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDiff {
    /// Pixels with at least one bitwise-different channel.
    pub differing_pixels: usize,
    /// Largest absolute channel difference; `INFINITY` when any value is non-finite.
    pub max_abs_diff: f32,
    /// Pixels with a channel beyond `1e-5 * max(|a|, |b|) + 1e-6 * spp`, or non-finite.
    pub tol_failing_pixels: usize,
    /// Pixels where either frame holds a NaN or infinite channel.
    pub non_finite_pixels: usize,
}

impl FrameDiff {
    /// Pixels allowed to fail the tolerance: `max(2, 0.1 %)` of `total_pixels`.
    #[must_use]
    pub fn allowed_tol_failing(total_pixels: usize) -> usize {
        2.max(total_pixels / 1000)
    }

    /// Whether the frames pass the TOL tier: no non-finite value anywhere and at most
    /// `max(2, 0.1 %)` pixels beyond tolerance.
    #[must_use]
    pub fn tol_passed(&self, total_pixels: usize) -> bool {
        self.non_finite_pixels == 0
            && self.tol_failing_pixels <= Self::allowed_tol_failing(total_pixels)
    }
}

/// Compares `a` against `b` (same length) for a pass of `spp` samples per pixel.
///
/// # Panics
///
/// Panics if the slices differ in length.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the §7.2 golden tolerance is defined with separately rounded operations; a fused form could flip a borderline pixel"
)]
pub fn compare_frames(a: &[Vec3], b: &[Vec3], spp: u32) -> FrameDiff {
    assert_eq!(a.len(), b.len(), "frames must have the same pixel count");
    let spp_tol = 1e-6 * spp as f32;
    let mut diff = FrameDiff {
        differing_pixels: 0,
        max_abs_diff: 0.0,
        tol_failing_pixels: 0,
        non_finite_pixels: 0,
    };
    for (pa, pb) in a.iter().zip(b) {
        if !pa.is_finite() || !pb.is_finite() {
            diff.non_finite_pixels += 1;
            diff.differing_pixels += 1;
            diff.tol_failing_pixels += 1;
            diff.max_abs_diff = f32::INFINITY;
            continue;
        }
        let mut bit_diff = false;
        let mut tol_fail = false;
        for c in 0..3 {
            let (va, vb) = (pa[c], pb[c]);
            if va.to_bits() != vb.to_bits() {
                bit_diff = true;
                let d = (va - vb).abs();
                diff.max_abs_diff = diff.max_abs_diff.max(d);
                if d > 1e-5 * va.abs().max(vb.abs()) + spp_tol {
                    tol_fail = true;
                }
            }
        }
        diff.differing_pixels += usize::from(bit_diff);
        diff.tol_failing_pixels += usize::from(tol_fail);
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, v: f32) -> Vec<Vec3> {
        vec![Vec3::splat(v); n]
    }

    #[test]
    fn identical_frames_are_exact() {
        let a = flat(100, 2.0);
        let d = compare_frames(&a, &a.clone(), 4);
        assert_eq!(d.differing_pixels, 0);
        assert!(d.tol_passed(100));
        assert_eq!(d.max_abs_diff, 0.0);
    }

    #[test]
    fn one_ulp_is_a_tol_pass() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        b[7].x = f32::from_bits(b[7].x.to_bits() + 1);
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.differing_pixels, 1);
        assert_eq!(d.tol_failing_pixels, 0);
        assert!(d.tol_passed(100));
    }

    #[test]
    fn many_large_differences_fail_the_tol_tier() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        for p in b.iter_mut().take(10) {
            *p = Vec3::splat(3.0);
        }
        let d = compare_frames(&a, &b, 4);
        assert_eq!(d.tol_failing_pixels, 10);
        assert!(!d.tol_passed(100));
    }

    #[test]
    fn a_couple_of_flips_still_pass() {
        let a = flat(100, 2.0);
        let mut b = a.clone();
        b[0] = Vec3::splat(3.0);
        b[1] = Vec3::splat(3.0);
        assert!(compare_frames(&a, &b, 4).tol_passed(100));
    }

    #[test]
    fn nan_is_never_tol() {
        let a = flat(1_000_000, 1.0);
        let mut b = a.clone();
        b[10].y = f32::NAN;
        let d = compare_frames(&a, &b, 2);
        assert_eq!(d.non_finite_pixels, 1);
        assert!(d.max_abs_diff.is_infinite());
        assert!(!d.tol_passed(1_000_000));
        // NaN in the reference frame is just as fatal.
        assert!(!compare_frames(&b, &a, 2).tol_passed(1_000_000));
    }

    #[test]
    fn infinity_is_never_tol() {
        let a = flat(64, 1.0);
        let mut b = a.clone();
        b[3].z = f32::INFINITY;
        assert!(!compare_frames(&a, &b, 2).tol_passed(64));
    }

    #[test]
    fn identical_nan_frames_still_fail() {
        let mut a = flat(64, 1.0);
        a[0].x = f32::NAN;
        let d = compare_frames(&a, &a.clone(), 2);
        assert!(!d.tol_passed(64));
    }
}

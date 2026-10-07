//! Closed-form entry and exit parameters of a ray inside the convex pieces of a tool
//! (ball, cone frustum, wedge). Split from the parent module so the file stays readable;
//! the arithmetic is unchanged.

use crate::optics::raytracer::camera::Ray;
use glam::Vec3;

/// Disc/quadratic guard: a ray whose discriminant is within this (relative)
/// distance of zero is tangent to the tool and reports no interval. Same guard
/// class as the slab loop's `denom`; the missed boundary is a measure-zero set
/// and the choice is deterministic.
const TANGENT_EPS: f32 = 1e-7;

/// `|denom|` below this treats a ray as parallel to a slab or plane.
const PARALLEL_EPS: f32 = 1e-7;

/// Roots of `a t^2 + 2 b t + c = 0` as `(lo, hi)`, or `None` when the ray is
/// tangent (relative discriminant within [`TANGENT_EPS`]).
///
/// Uses the cancellation-free form so a small tool hit from far away keeps its
/// precision. Requires `a != 0`.
fn quadratic_roots(a: f32, b: f32, c: f32) -> Option<(f32, f32)> {
    let disc = b.mul_add(b, -(a * c));
    if disc <= TANGENT_EPS * b.mul_add(b, (a * c).abs()) {
        return None;
    }
    let sq = disc.sqrt();
    let q = -(b + sq.copysign(b));
    let r1 = q / a;
    let r2 = c / q;
    Some(if r1 <= r2 { (r1, r2) } else { (r2, r1) })
}

pub(super) fn ball_interval(ray: Ray, c: Vec3, r: f32) -> Option<(f32, f32)> {
    let o = ray.origin - c;
    let a = ray.dir.dot(ray.dir);
    let b = o.dot(ray.dir);
    let cc = r.mul_add(-r, o.dot(o));
    quadratic_roots(a, b, cc)
}

/// Parameter range of `ray` inside the slab `|axial| <= hl`, `None` when the
/// ray is parallel to the caps and outside them.
fn cap_range(z0: f32, dz: f32, hl: f32) -> Option<(f32, f32)> {
    if dz.abs() > PARALLEL_EPS {
        let t1 = (-hl - z0) / dz;
        let t2 = (hl - z0) / dz;
        Some(if t1 <= t2 { (t1, t2) } else { (t2, t1) })
    } else if z0.abs() <= hl {
        Some((-1e30, 1e30))
    } else {
        None
    }
}

fn clip(lo: f32, hi: f32, s0: f32, s1: f32) -> Option<(f32, f32)> {
    let l = lo.max(s0);
    let h = hi.min(s1);
    (l <= h).then_some((l, h))
}

#[expect(
    clippy::manual_midpoint,
    reason = "pinned reference arithmetic: plain f32 `0.5 * (r0 + r1)`; `f32::midpoint` widens through f64"
)]
pub(super) fn frustum_interval(
    ray: Ray,
    c: Vec3,
    a: Vec3,
    hl: f32,
    r0: f32,
    r1: f32,
) -> Option<(f32, f32)> {
    let o = ray.origin - c;
    let z0 = o.dot(a);
    let dz = ray.dir.dot(a);
    let (s0, s1) = cap_range(z0, dz, hl)?;
    let rm = 0.5 * (r0 + r1);
    let k = (r1 - r0) / (2.0 * hl);
    let p = o - a * z0;
    let q = ray.dir - a * dz;
    let rz0 = k.mul_add(z0, rm);
    let rate = k * dz;
    let qa = rate.mul_add(-rate, q.dot(q));
    let qb = (-rate).mul_add(rz0, p.dot(q));
    let qc = rz0.mul_add(-rz0, p.dot(p));
    if qa.abs() <= PARALLEL_EPS {
        // Ray parallel to a generator: the radius condition is linear in t.
        if qb.abs() <= PARALLEL_EPS {
            return (qc <= 0.0).then_some((s0, s1));
        }
        let t = -qc / (2.0 * qb);
        return if qb > 0.0 {
            clip(-1e30, t, s0, s1)
        } else {
            clip(t, 1e30, s0, s1)
        };
    }
    let (lo, hi) = quadratic_roots(qa, qb, qc)?;
    if qa > 0.0 {
        clip(lo, hi, s0, s1)
    } else {
        // Steeper than the cone: `Q <= 0` outside the roots. The mirror nappe
        // lies beyond the caps, so at most one side survives clipping; the hull
        // keeps the result an interval if rounding lets both through.
        match (clip(-1e30, lo, s0, s1), clip(hi, 1e30, s0, s1)) {
            (Some(x), Some(y)) => Some((x.0.min(y.0), x.1.max(y.1))),
            (x, y) => x.or(y),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a wedge is fully described by its frame and two radii; a struct would only rename them"
)]
#[expect(
    clippy::manual_midpoint,
    reason = "pinned reference arithmetic: plain f32 `0.5 * (r0 + r1)`; `f32::midpoint` widens through f64"
)]
pub(super) fn wedge_interval(
    ray: Ray,
    c: Vec3,
    a: Vec3,
    d: Vec3,
    hl: f32,
    s: f32,
    r0: f32,
    r1: f32,
) -> Option<(f32, f32)> {
    let w = a.cross(d);
    let k = (r1 - r0) / (2.0 * hl);
    let rm = 0.5 * (r0 + r1);
    // `(normal, offset)` with `normal . (x - c) + offset <= 0` inside; normals
    // are deliberately not normalised, the slab method only needs ratios.
    let faces = [
        (a, -hl),
        (-a, -hl),
        (d, -s),
        (-d, -s),
        (w - a * k, -rm),
        (-w - a * k, -rm),
    ];
    let o = ray.origin - c;
    let mut t_near = -1e30f32;
    let mut t_far = 1e30f32;
    for (n, e) in faces {
        let denom = n.dot(ray.dir);
        let side = n.dot(o) + e;
        if denom.abs() > PARALLEL_EPS {
            let t = -side / denom;
            if denom < 0.0 {
                t_near = t_near.max(t);
            } else {
                t_far = t_far.min(t);
            }
        } else if side > 0.0 {
            return None;
        }
    }
    (t_near <= t_far).then_some((t_near, t_far))
}

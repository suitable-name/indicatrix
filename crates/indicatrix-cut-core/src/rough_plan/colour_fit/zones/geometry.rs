//! Small deterministic numerics shared by the zone modules.
//!
//! A Jacobi eigen solver for symmetric 3 x 3 matrices, a dense linear solver, a damped
//! Gauss-Newton driver with finite differences, and the axis reference frame of the zone shapes.
//!
//! Everything is a fixed sequence of arithmetic on `f64`: no randomness, no hashed collections,
//! fixed sweep and iteration caps.

use std::f64::consts::TAU;

use glam::DVec3;

/// The eigen decomposition of a symmetric 3 x 3 matrix by cyclic Jacobi rotations.
///
/// Returns the eigenvalues in ascending order and the matching unit eigenvectors, each with its
/// largest component made positive (a deterministic sign). Converges quadratically; the cap of
/// 32 sweeps is never reached for a finite matrix.
pub(super) fn sym_eigen3(m: [[f64; 3]; 3]) -> ([f64; 3], [DVec3; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let scale = (a[0][0].abs() + a[1][1].abs() + a[2][2].abs()).max(1e-300);
    for _sweep in 0..32 {
        let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
        if off <= 1e-30 * scale * scale {
            break;
        }
        for (p, q) in [(0_usize, 1_usize), (0, 2), (1, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let sign = if theta >= 0.0 { 1.0 } else { -1.0 };
            let t = sign / (theta.abs() + theta.mul_add(theta, 1.0).sqrt());
            let c = 1.0 / t.mul_add(t, 1.0).sqrt();
            let s = t * c;
            let app = a[p][p];
            let aqq = a[q][q];
            let apq = a[p][q];
            a[p][p] = app - t * apq;
            a[q][q] = aqq + t * apq;
            a[p][q] = 0.0;
            a[q][p] = 0.0;
            for k in 0..3 {
                if k != p && k != q {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[p][k] = a[k][p];
                    a[k][q] = s * akp + c * akq;
                    a[q][k] = a[k][q];
                }
            }
            for row in &mut v {
                let vp = row[p];
                let vq = row[q];
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let values = [a[0][0], a[1][1], a[2][2]];
    let mut order = [0_usize, 1, 2];
    order.sort_by(|&i, &j| values[i].total_cmp(&values[j]).then(i.cmp(&j)));
    let vectors = order.map(|i| canonical_dir(DVec3::new(v[0][i], v[1][i], v[2][i])));
    (order.map(|i| values[i]), vectors)
}

/// `v` or `-v`, whichever has its largest-magnitude component positive (ties: the first axis).
pub(super) fn canonical_dir(v: DVec3) -> DVec3 {
    let a = v.abs();
    let k = if a.x >= a.y && a.x >= a.z {
        0
    } else if a.y >= a.z {
        1
    } else {
        2
    };
    if v[k] < 0.0 { -v } else { v }
}

/// The mean of the points (in order).
pub(super) fn centroid(points: &[DVec3]) -> DVec3 {
    let mut sum = DVec3::ZERO;
    for p in points {
        sum += *p;
    }
    sum / points.len().max(1) as f64
}

/// The scatter matrix `sum (p - c)(p - c)^T` of the points about `c`.
pub(super) fn scatter(points: &[DVec3], c: DVec3) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for p in points {
        let d = (*p - c).to_array();
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += d[i] * d[j];
            }
        }
    }
    m
}

/// The principal axes of a point set.
pub(super) struct Pca {
    /// The centroid.
    pub centroid: DVec3,
    /// The eigenvalues of the scatter matrix, ascending.
    pub values: [f64; 3],
    /// The matching unit eigenvectors.
    pub vectors: [DVec3; 3],
}

/// The principal axes of `points` (at least one point).
pub(super) fn pca(points: &[DVec3]) -> Pca {
    let c = centroid(points);
    let (values, vectors) = sym_eigen3(scatter(points, c));
    Pca {
        centroid: c,
        values,
        vectors,
    }
}

/// The diagonal of the bounding box of the points.
pub(super) fn extent(points: &[DVec3]) -> f64 {
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for p in points {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    if lo.is_finite() && hi.is_finite() {
        (hi - lo).length()
    } else {
        0.0
    }
}

/// The root mean square of the values (0 for none).
pub(super) fn rms(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
}

/// Solves the `n x n` system `a x = b` (`a` row-major) in place by Gaussian elimination with
/// partial pivoting; the solution replaces `b`. Returns `false` for a singular system.
pub(super) fn solve_linear(n: usize, a: &mut [f64], b: &mut [f64]) -> bool {
    for col in 0..n {
        let mut pivot = col;
        let mut best = a[col * n + col].abs();
        for row in (col + 1)..n {
            let v = a[row * n + col].abs();
            if v > best {
                best = v;
                pivot = row;
            }
        }
        if best <= 1e-300 || !best.is_finite() {
            return false;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(col * n + k, pivot * n + k);
            }
            b.swap(col, pivot);
        }
        for row in (col + 1)..n {
            let f = a[row * n + col] / a[col * n + col];
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[row * n + k] -= f * a[col * n + k];
            }
            b[row] -= f * b[col];
        }
    }
    for col in (0..n).rev() {
        let mut s = b[col];
        for k in (col + 1)..n {
            s -= a[col * n + k] * b[k];
        }
        b[col] = s / a[col * n + col];
    }
    true
}

/// The result of [`least_squares`].
#[allow(dead_code, reason = "diagnostic fields, read by the tests")]
pub(super) struct Lsq {
    /// The final parameters.
    pub x: Vec<f64>,
    /// The final sum of squared residuals.
    pub cost: f64,
    /// Iterations used.
    pub iterations: usize,
    /// Whether the iteration stopped on its tolerance (as opposed to the iteration cap).
    pub converged: bool,
}

fn sum_sq(values: &[f64]) -> f64 {
    values.iter().map(|v| v * v).sum()
}

/// Damped Gauss-Newton (Levenberg-Marquardt with a multiplicative damping update) on the
/// residual function `residuals(x, out)`, which must clear `out` and push exactly `n_res`
/// values. The Jacobian is built by central differences with the per-parameter `steps`.
///
/// A step is accepted only when it lowers the sum of squares; the damping starts at 1e-3 of the
/// diagonal of `J^T J`, shrinks tenfold per accepted step and grows tenfold per rejected one.
/// Stops when a step is tiny against the difference steps, the relative gain falls below 1e-13,
/// no damping level helps, or after `max_iter` iterations. Fully deterministic.
pub(super) fn least_squares(
    x0: &[f64],
    n_res: usize,
    steps: &[f64],
    max_iter: usize,
    residuals: &dyn Fn(&[f64], &mut Vec<f64>),
) -> Lsq {
    let n = x0.len();
    let mut x = x0.to_vec();
    let mut r = Vec::with_capacity(n_res);
    residuals(&x, &mut r);
    let mut cost = sum_sq(&r);
    let mut lambda = 1e-3;
    let mut converged = false;
    let mut iterations = 0;
    let mut r_plus = Vec::with_capacity(n_res);
    let mut r_minus = Vec::with_capacity(n_res);
    let mut jac = vec![0.0; n_res * n];
    while iterations < max_iter {
        iterations += 1;
        for j in 0..n {
            let mut xp = x.clone();
            xp[j] += steps[j];
            let mut xm = x.clone();
            xm[j] -= steps[j];
            residuals(&xp, &mut r_plus);
            residuals(&xm, &mut r_minus);
            for i in 0..n_res {
                jac[i * n + j] = (r_plus[i] - r_minus[i]) / (2.0 * steps[j]);
            }
        }
        let mut jtj = vec![0.0; n * n];
        let mut jtr = vec![0.0; n];
        for i in 0..n_res {
            let row = &jac[i * n..(i + 1) * n];
            for a in 0..n {
                jtr[a] += row[a] * r[i];
                for b in 0..n {
                    jtj[a * n + b] += row[a] * row[b];
                }
            }
        }
        let mut improved = false;
        for _ in 0..12 {
            let mut m = jtj.clone();
            for a in 0..n {
                m[a * n + a] += lambda * jtj[a * n + a].max(1e-12);
            }
            let mut delta: Vec<f64> = jtr.iter().map(|v| -v).collect();
            if !solve_linear(n, &mut m, &mut delta) {
                lambda *= 10.0;
                continue;
            }
            let trial: Vec<f64> = x.iter().zip(&delta).map(|(a, d)| a + d).collect();
            residuals(&trial, &mut r_plus);
            let trial_cost = sum_sq(&r_plus);
            if trial_cost.is_finite() && trial_cost < cost {
                let tiny = delta.iter().zip(steps).all(|(d, s)| d.abs() <= 1e-4 * s);
                let gain = (cost - trial_cost) / cost.max(1e-300);
                x = trial;
                r.clone_from(&r_plus);
                cost = trial_cost;
                lambda = (lambda * 0.1).max(1e-12);
                improved = true;
                if tiny || gain < 1e-13 {
                    converged = true;
                }
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            converged = true;
            break;
        }
        if converged {
            break;
        }
    }
    Lsq {
        x,
        cost,
        iterations,
        converged,
    }
}

/// The reference directions `(u, v)` of an axis, exactly the rule of the zone kernels: the world
/// X axis (Y when `|axis.x| > 0.9`) minus its component along the axis, normalised, then
/// `v = axis x u`. Angles and prism phases are measured from `u` towards `v`.
pub(super) fn axis_reference(axis_dir: DVec3) -> (DVec3, DVec3) {
    let seed = if axis_dir.x.abs() > 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let u = (seed - axis_dir * seed.dot(axis_dir)).normalize();
    (u, axis_dir.cross(u))
}

/// The side normals of a regular polygon: `cos(a_k) e1 + sin(a_k) e2` with
/// `a_k = phase + 2 pi k / sides`.
pub(super) fn polygon_normals(sides: u32, phase: f64, e1: DVec3, e2: DVec3) -> Vec<DVec3> {
    (0..sides)
        .map(|k| {
            let angle = phase + TAU * f64::from(k) / f64::from(sides);
            e1 * angle.cos() + e2 * angle.sin()
        })
        .collect()
}

/// The largest projection of `w` on any of `normals` (the polygon gauge).
pub(super) fn gauge(normals: &[DVec3], w: DVec3) -> f64 {
    normals
        .iter()
        .map(|n| w.dot(*n))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The angle of `direction` in the reference frame `(u, v)`, in `[0, 2 pi)`.
pub(super) fn angle_in(direction: DVec3, u: DVec3, v: DVec3) -> f64 {
    direction.dot(v).atan2(direction.dot(u)).rem_euclid(TAU)
}

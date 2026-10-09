//! Small dense linear algebra for the calibration: Cholesky, non-negative least squares on the
//! normal equations (Lawson-Hanson active set), and a condition estimate.
//!
//! Row-major flat
//! matrices, deterministic.

#![allow(
    clippy::many_single_char_names,
    reason = "matrix notation: L, A, H, n, b, x, y, z, s as in the Cholesky and Lawson-Hanson texts"
)]

/// Cholesky factor `L` (lower, row-major `n x n`) of a symmetric positive definite matrix, or
/// `None` if a pivot is not positive.
pub(super) fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0_f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum = l[i * n + k].mul_add(-l[j * n + k], sum);
            }
            if i == j {
                if sum <= 0.0 || !sum.is_finite() {
                    return None;
                }
                l[i * n + i] = sum.sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j];
            }
        }
    }
    Some(l)
}

/// Solve `L L^T x = b`.
pub(super) fn cholesky_solve(l: &[f64], n: usize, b: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum = l[i * n + k].mul_add(-y[k], sum);
        }
        y[i] = sum / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum = l[k * n + i].mul_add(-x[k], sum);
        }
        x[i] = sum / l[i * n + i];
    }
    x
}

/// `y = A x` for a row-major `n x n` matrix.
pub(super) fn mat_vec(a: &[f64], n: usize, x: &[f64]) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let mut sum = 0.0;
            for j in 0..n {
                sum = a[i * n + j].mul_add(x[j], sum);
            }
            sum
        })
        .collect()
}

/// Minimise `0.5 s^T H s - g^T s` over `s >= 0` for symmetric positive definite `H`
/// (Lawson-Hanson active set on the normal equations). Deterministic; iteration-capped.
pub(super) fn nnls_normal(h: &[f64], g: &[f64], n: usize) -> Vec<f64> {
    let g_max = g.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    let tolerance = 1e-12 * g_max.max(f64::MIN_POSITIVE);
    let mut s = vec![0.0; n];
    let mut passive = vec![false; n];
    for _ in 0..(10 * n + 10) {
        let hs = mat_vec(h, n, &s);
        let mut best: Option<usize> = None;
        let mut best_w = tolerance;
        for j in 0..n {
            if passive[j] {
                continue;
            }
            let w = g[j] - hs[j];
            if w > best_w {
                best_w = w;
                best = Some(j);
            }
        }
        let Some(entering) = best else { break };
        passive[entering] = true;
        for _ in 0..(n + 5) {
            let idx: Vec<usize> = (0..n).filter(|&i| passive[i]).collect();
            let m = idx.len();
            let mut sub = vec![0.0; m * m];
            let mut rhs = vec![0.0; m];
            for (a, &i) in idx.iter().enumerate() {
                rhs[a] = g[i];
                for (b, &j) in idx.iter().enumerate() {
                    sub[a * m + b] = h[i * n + j];
                }
            }
            let Some(factor) = cholesky(&sub, m) else {
                return s;
            };
            let solved = cholesky_solve(&factor, m, &rhs);
            let mut z = vec![0.0; n];
            for (a, &i) in idx.iter().enumerate() {
                z[i] = solved[a];
            }
            if idx.iter().all(|&i| z[i] > 0.0) {
                s = z;
                break;
            }
            // Some index has z <= 0 here (the `all` above failed), so `blocking` is set.
            let mut alpha = f64::INFINITY;
            let mut blocking = idx[0];
            for &i in &idx {
                if z[i] <= 0.0 {
                    let d = s[i] - z[i];
                    let a = if d > 0.0 { s[i] / d } else { 0.0 };
                    if a < alpha {
                        alpha = a;
                        blocking = i;
                    }
                }
            }
            for i in 0..n {
                s[i] = f64::mul_add(alpha, z[i] - s[i], s[i]);
            }
            s[blocking] = 0.0;
            for &i in &idx {
                if s[i] <= 1e-14 {
                    s[i] = 0.0;
                    passive[i] = false;
                }
            }
        }
    }
    s
}

/// Estimate `lambda_max / lambda_min` of a symmetric positive definite matrix by power
/// iteration and inverse iteration (200 steps each, fixed start vector). Underestimates when the
/// start vector is nearly orthogonal to an extreme eigenvector; fine as a report number.
pub(super) fn condition_estimate(a: &[f64], n: usize) -> f64 {
    let start = |i: usize| 0.37f64.mul_add((i * 7 % 11) as f64, 1.0);
    let mut v: Vec<f64> = (0..n).map(start).collect();
    let mut lambda_max = 0.0;
    for _ in 0..200 {
        let w = mat_vec(a, n, &v);
        let norm = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm == 0.0 {
            return f64::INFINITY;
        }
        lambda_max = norm / v.iter().map(|x| x * x).sum::<f64>().sqrt();
        v = w.iter().map(|x| x / norm).collect();
    }
    let Some(factor) = cholesky(a, n) else {
        return f64::INFINITY;
    };
    let mut v: Vec<f64> = (0..n).map(start).collect();
    let mut inverse_norm = 0.0;
    for _ in 0..200 {
        let w = cholesky_solve(&factor, n, &v);
        let norm = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return f64::INFINITY;
        }
        inverse_norm = norm / v.iter().map(|x| x * x).sum::<f64>().sqrt();
        v = w.iter().map(|x| x / norm).collect();
    }
    lambda_max * inverse_norm
}

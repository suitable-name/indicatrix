//! Small dense linear algebra for the normal equations: row-major `n x n` matrices stored in
//! flat slices.
//!
//! The sizes are at most a few dozen, so nothing here is blocked or vectorised, and
//! every loop has a fixed order (the results are bitwise reproducible).

/// The lower Cholesky factor `L` (`a = L L^T`) of the symmetric positive definite `a`, or `None`
/// when `a` is not positive definite or holds a non-finite value.
pub(super) fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if sum.is_nan() || sum <= 0.0 || !sum.is_finite() {
                    return None;
                }
                l[i * n + i] = sum.sqrt();
            } else {
                let value = sum / l[j * n + j];
                if !value.is_finite() {
                    return None;
                }
                l[i * n + j] = value;
            }
        }
    }
    Some(l)
}

/// Solves `L L^T x = b` for the factor of [`cholesky`].
pub(super) fn cholesky_solve(l: &[f64], n: usize, b: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum -= l[k * n + i] * x[k];
        }
        x[i] = sum / l[i * n + i];
    }
    x
}

/// Solves `a x = b` for the symmetric positive definite `a`.
pub(super) fn solve_spd(a: &[f64], n: usize, b: &[f64]) -> Option<Vec<f64>> {
    let l = cholesky(a, n)?;
    let x = cholesky_solve(&l, n, b);
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The inverse of the symmetric positive definite `a`.
pub(super) fn inverse_spd(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let l = cholesky(a, n)?;
    let mut inverse = vec![0.0; n * n];
    let mut unit = vec![0.0; n];
    for col in 0..n {
        unit.fill(0.0);
        unit[col] = 1.0;
        let x = cholesky_solve(&l, n, &unit);
        for row in 0..n {
            inverse[row * n + col] = x[row];
        }
    }
    // Symmetrise: the two triangles differ by rounding only.
    for i in 0..n {
        for j in (i + 1)..n {
            let mean = f64::midpoint(inverse[i * n + j], inverse[j * n + i]);
            inverse[i * n + j] = mean;
            inverse[j * n + i] = mean;
        }
    }
    inverse.iter().all(|v| v.is_finite()).then_some(inverse)
}

/// The inverse of `a`, adding a growing ridge to the diagonal when `a` is singular (a parameter
/// the data and the priors do not determine). Returns the inverse and the ridge used (0 when
/// none was needed).
pub(super) fn inverse_spd_ridged(a: &[f64], n: usize) -> (Vec<f64>, f64) {
    if let Some(inverse) = inverse_spd(a, n) {
        return (inverse, 0.0);
    }
    let scale = (0..n).map(|i| a[i * n + i].abs()).sum::<f64>() / n.max(1) as f64;
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let mut ridge = 1e-12 * scale;
    for _ in 0..24 {
        let mut shifted = a.to_vec();
        for i in 0..n {
            shifted[i * n + i] += ridge;
        }
        if let Some(inverse) = inverse_spd(&shifted, n) {
            return (inverse, ridge);
        }
        ridge *= 10.0;
    }
    // Last resort: a diagonal inverse (never reached for a matrix with a positive diagonal).
    let mut inverse = vec![0.0; n * n];
    for i in 0..n {
        let d = a[i * n + i];
        inverse[i * n + i] = if d.is_finite() && d > 0.0 {
            1.0 / d
        } else {
            0.0
        };
    }
    (inverse, f64::INFINITY)
}

/// The eigen decomposition of the symmetric `a` by the cyclic Jacobi method (fixed sweep order,
/// so the result is bitwise reproducible). Returns the eigenvalues in ascending order (ties by
/// original index) and the matching unit eigenvectors, row `k` (`n` entries at `k * n`) being the
/// vector of eigenvalue `k`.
pub(super) fn symmetric_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    // Symmetrise: the caller's matrix differs from its transpose by rounding only.
    for i in 0..n {
        for j in (i + 1)..n {
            let mean = f64::midpoint(m[i * n + j], m[j * n + i]);
            m[i * n + j] = mean;
            m[j * n + i] = mean;
        }
    }
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..64 {
        let mut off = 0.0;
        let mut diag = 0.0;
        for i in 0..n {
            diag += m[i * n + i] * m[i * n + i];
            for j in (i + 1)..n {
                off += m[i * n + j] * m[i * n + j];
            }
        }
        if off <= 1e-28 * diag.max(1e-300) {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = m[p * n + q];
                if apq == 0.0 {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + theta.mul_add(theta, 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / t.mul_add(t, 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| m[x * n + x].total_cmp(&m[y * n + y]).then(x.cmp(&y)));
    let values: Vec<f64> = order.iter().map(|&i| m[i * n + i]).collect();
    let mut vectors = vec![0.0; n * n];
    for (row, &col) in order.iter().enumerate() {
        for k in 0..n {
            vectors[row * n + k] = v[k * n + col];
        }
    }
    (values, vectors)
}

/// `trace(a b)` of two `n x n` matrices.
pub(super) fn trace_product(a: &[f64], b: &[f64], n: usize) -> f64 {
    let mut sum = 0.0;
    for i in 0..n {
        for k in 0..n {
            sum += a[i * n + k] * b[k * n + i];
        }
    }
    sum
}

/// `j c j^T` for a `3 x n` matrix `j` (row-major) and the `n x n` matrix `c`.
pub(super) fn sandwich3(j: &[[f64; 3]], c: &[f64], n: usize, stride: usize) -> [[f64; 3]; 3] {
    // `j` is given column-wise: `j[p]` is the derivative of the three outputs with respect to
    // parameter `p`. `c` has row stride `stride` and the leading `n x n` block is used.
    let mut out = [[0.0; 3]; 3];
    for p in 0..n {
        for q in 0..n {
            let cpq = c[p * stride + q];
            if cpq == 0.0 {
                continue;
            }
            for a in 0..3 {
                for b in 0..3 {
                    out[a][b] += j[p][a] * cpq * j[q][b];
                }
            }
        }
    }
    out
}

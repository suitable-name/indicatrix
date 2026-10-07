//! Small numerical routines: a dense linear solve and a Levenberg-Marquardt fit.
//!
//! Everything is deterministic: fixed loop orders, central-difference Jacobians with fixed
//! steps, no randomness. The matrices are tiny (at most a few dozen unknowns), so plain
//! Gaussian elimination is enough.

/// The fit [`levenberg_marquardt`] returns.
#[derive(Debug, Clone)]
pub(super) struct LmFit {
    /// The parameters at the lowest cost found.
    pub params: Vec<f64>,
    /// Half the sum of squared residuals there.
    pub cost: f64,
    /// How many iterations were run.
    pub iterations: usize,
}

/// Solves `matrix * x = rhs` for a dense row-major square matrix by Gaussian elimination
/// with partial pivoting; `None` when the matrix is singular.
pub(super) fn solve_dense(matrix: &[f64], rhs: &[f64]) -> Option<Vec<f64>> {
    let size = rhs.len();
    let mut work = matrix.to_vec();
    let mut right = rhs.to_vec();
    for col in 0..size {
        let pivot_row = (col..size).max_by(|&row1, &row2| {
            work[row1 * size + col]
                .abs()
                .total_cmp(&work[row2 * size + col].abs())
        })?;
        let pivot = work[pivot_row * size + col];
        if pivot.is_nan() || pivot.abs() <= 1e-300 {
            return None;
        }
        if pivot_row != col {
            for k in 0..size {
                work.swap(col * size + k, pivot_row * size + k);
            }
            right.swap(col, pivot_row);
        }
        for row in col + 1..size {
            let factor = work[row * size + col] / pivot;
            for k in col..size {
                let sub = factor * work[col * size + k];
                work[row * size + k] -= sub;
            }
            let sub = factor * right[col];
            right[row] -= sub;
        }
    }
    let mut solution = vec![0.0; size];
    for row in (0..size).rev() {
        let tail: f64 = (row + 1..size)
            .map(|k| work[row * size + k] * solution[k])
            .sum();
        solution[row] = (right[row] - tail) / work[row * size + row];
    }
    Some(solution)
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right).map(|(l, r)| l * r).sum()
}

fn half_sum_squares(values: &[f64]) -> f64 {
    0.5 * values.iter().map(|v| v * v).sum::<f64>()
}

/// The Jacobian of `residuals` at `params` by central differences, one column per
/// parameter.
fn jacobian(
    params: &[f64],
    steps: &[f64],
    residuals: &dyn Fn(&[f64]) -> Vec<f64>,
) -> Vec<Vec<f64>> {
    let mut columns = Vec::with_capacity(params.len());
    for (k, &step) in steps.iter().enumerate() {
        let mut plus = params.to_vec();
        let mut minus = params.to_vec();
        plus[k] += step;
        minus[k] -= step;
        let (res_plus, res_minus) = (residuals(&plus), residuals(&minus));
        let inv = 0.5 / step;
        columns.push(
            res_plus
                .iter()
                .zip(&res_minus)
                .map(|(hi, lo)| (hi - lo) * inv)
                .collect(),
        );
    }
    columns
}

/// The normal equations `J^T J` (row-major) and `-J^T r` for the Jacobian `columns` and the
/// residuals `current`.
fn normal_equations(columns: &[Vec<f64>], current: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let count = columns.len();
    let mut normal = vec![0.0; count * count];
    let mut rhs = vec![0.0; count];
    for (i, col_i) in columns.iter().enumerate() {
        rhs[i] = -dot(col_i, current);
        for (j, col_j) in columns.iter().enumerate().skip(i) {
            let value = dot(col_i, col_j);
            normal[i * count + j] = value;
            normal[j * count + i] = value;
        }
    }
    (normal, rhs)
}

/// Minimises half the sum of squares of `residuals(params)` from `start`.
///
/// It is a Levenberg-Marquardt iteration (Marquardt's diagonal scaling) and a central-difference
/// Jacobian taken with `steps` (one per parameter). The residual vector must have the same
/// length for every parameter vector.
pub(super) fn levenberg_marquardt(
    start: &[f64],
    steps: &[f64],
    max_iterations: usize,
    residuals: &dyn Fn(&[f64]) -> Vec<f64>,
) -> LmFit {
    let count = start.len();
    let mut params = start.to_vec();
    let mut current = residuals(&params);
    let mut cost = half_sum_squares(&current);
    let mut lambda = 1e-3;
    let mut iterations = 0;
    for _ in 0..max_iterations {
        if cost < 1e-24 {
            break;
        }
        iterations += 1;
        let columns = jacobian(&params, steps, residuals);
        let (normal, rhs) = normal_equations(&columns, &current);
        let mut damped = normal.clone();
        for i in 0..count {
            let diag = normal[i * count + i];
            let bump = lambda * diag.max(1e-12);
            damped[i * count + i] = diag + bump;
        }
        let Some(step) = solve_dense(&damped, &rhs) else {
            lambda = (lambda * 10.0).min(1e12);
            continue;
        };
        let trial: Vec<f64> = params.iter().zip(&step).map(|(p, d)| p + d).collect();
        let trial_residuals = residuals(&trial);
        let trial_cost = half_sum_squares(&trial_residuals);
        if trial_cost < cost {
            let gain = (cost - trial_cost) / cost.max(1e-300);
            let step_size = step.iter().map(|d| d * d).sum::<f64>().sqrt();
            params = trial;
            current = trial_residuals;
            cost = trial_cost;
            lambda = (lambda / 3.0).max(1e-12);
            if gain < 1e-13 || step_size < 1e-12 {
                break;
            }
        } else {
            lambda *= 5.0;
            if lambda > 1e12 {
                break;
            }
        }
    }
    LmFit {
        params,
        cost,
        iterations,
    }
}

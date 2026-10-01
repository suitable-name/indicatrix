//! Dense two-phase simplex solver with Bland's rule for scaling stones into halfspace regions.
//!
//! Solves:
//! ```text
//! maximize   k
//! subject to n_j · t + k * h_j <= m_j   for j = 1..J
//!            k >= 0,  t in R^3 free
//! ```
//! where `t` is the stone's position, `k` its scale, and `h_j >= 0` the support of the
//! stone in direction `n_j` at unit scale.
//!
//! The module is private to `rough_plan`: [`ScaleRow`], [`LpScratch`] and the solvers are
//! reachable only inside the crate.

/// One constraint `normal · t + scale * support <= offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleRow {
    /// Outward unit normal of the halfspace constraint.
    pub normal: [f64; 3],
    /// Support of the stone in direction `normal` at unit scale (`h_j >= 0`).
    pub support: f64,
    /// Offset of the halfspace plane (`m_j`).
    pub offset: f64,
}

/// Reusable tableau and workspace for the LP solver to eliminate per-call allocations.
#[derive(Default, Debug)]
pub struct LpScratch {
    pub(crate) tableau: Vec<f64>,
    pub(crate) basic_vars: Vec<usize>,
}

impl LpScratch {
    /// Creates a new empty scratch workspace.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Solves for the maximum scale `k` and stone position `t` using an ephemeral scratchpad.
///
/// Returns `None` if the region is empty / infeasible even at `k = 0` or if the iteration cap is hit.
/// Only tests solve one-off problems; production callers reuse a scratch through
/// [`max_scale_with_scratch`].
#[cfg(test)]
#[must_use]
pub fn max_scale(rows: &[ScaleRow]) -> Option<(f64, [f64; 3])> {
    let mut scratch = LpScratch::new();
    max_scale_with_scratch(rows, &mut scratch)
}

/// Simplex iterations allowed for `j_count` rows (phase 1 and phase 2 together).
const fn iteration_cap(j_count: usize) -> usize {
    50 * (j_count + 8)
}

/// Solves for the maximum scale `k` and stone position `t` using the provided reusable workspace.
///
/// Returns `None` if the region is empty / infeasible even at `k = 0`, if `k` is unbounded,
/// if a row is not finite (normal, support or offset) or has a negative support, or if the
/// iteration cap is hit.
pub fn max_scale_with_scratch(
    rows: &[ScaleRow],
    scratch: &mut LpScratch,
) -> Option<(f64, [f64; 3])> {
    solve(rows, scratch, iteration_cap(rows.len()))
}

/// [`max_scale_with_scratch`] with an explicit simplex iteration cap.
pub(super) fn solve(
    rows: &[ScaleRow],
    scratch: &mut LpScratch,
    max_iterations: usize,
) -> Option<(f64, [f64; 3])> {
    if rows.is_empty() {
        return None;
    }

    let j_count = rows.len();
    let mut artificial_count = 0;
    for row in rows {
        if !row.normal.iter().all(|c| c.is_finite())
            || !row.offset.is_finite()
            || !row.support.is_finite()
            || row.support < 0.0
        {
            return None;
        }
        if row.offset < 0.0 {
            artificial_count += 1;
        }
    }

    let rhs_col = 7 + j_count + artificial_count;
    let num_cols = rhs_col + 1;
    let num_rows = j_count + 1;
    let total_cells = num_rows * num_cols;

    // `clear` then `resize` zero-fills every cell (a bare `resize` would keep the pivoted
    // values of the previous solve in the first `len` cells) and keeps the allocation.
    scratch.tableau.clear();
    scratch.tableau.resize(total_cells, 0.0);
    scratch.basic_vars.clear();
    scratch.basic_vars.resize(j_count, 0);

    setup_tableau(rows, scratch, num_cols, rhs_col);

    let mut iterations = 0;

    if artificial_count > 0 {
        run_phase1(
            rows,
            scratch,
            num_cols,
            artificial_count,
            max_iterations,
            &mut iterations,
        )?;
    }

    run_phase2(scratch, j_count, num_cols, max_iterations, &mut iterations)?;

    let (k, t) = extract_solution(scratch, j_count, num_cols, rhs_col);
    post_check_feasibility(rows, t, k)
}

fn setup_tableau(rows: &[ScaleRow], scratch: &mut LpScratch, num_cols: usize, rhs_col: usize) {
    let j_count = rows.len();
    let mut next_art = 0;
    for (j, row) in rows.iter().enumerate() {
        let r_offset = j * num_cols;
        if row.offset >= 0.0 {
            scratch.tableau[r_offset] = row.normal[0];
            scratch.tableau[r_offset + 1] = row.normal[1];
            scratch.tableau[r_offset + 2] = row.normal[2];
            scratch.tableau[r_offset + 3] = -row.normal[0];
            scratch.tableau[r_offset + 4] = -row.normal[1];
            scratch.tableau[r_offset + 5] = -row.normal[2];
            scratch.tableau[r_offset + 6] = row.support;
            scratch.tableau[r_offset + 7 + j] = 1.0;
            scratch.tableau[r_offset + rhs_col] = row.offset;
            scratch.basic_vars[j] = 7 + j;
        } else {
            scratch.tableau[r_offset] = -row.normal[0];
            scratch.tableau[r_offset + 1] = -row.normal[1];
            scratch.tableau[r_offset + 2] = -row.normal[2];
            scratch.tableau[r_offset + 3] = row.normal[0];
            scratch.tableau[r_offset + 4] = row.normal[1];
            scratch.tableau[r_offset + 5] = row.normal[2];
            scratch.tableau[r_offset + 6] = -row.support;
            scratch.tableau[r_offset + 7 + j] = -1.0;
            let art_col = 7 + j_count + next_art;
            scratch.tableau[r_offset + art_col] = 1.0;
            scratch.tableau[r_offset + rhs_col] = -row.offset;
            scratch.basic_vars[j] = art_col;
            next_art += 1;
        }
    }
}

fn run_phase1(
    rows: &[ScaleRow],
    scratch: &mut LpScratch,
    num_cols: usize,
    artificial_count: usize,
    max_iterations: usize,
    iterations: &mut usize,
) -> Option<()> {
    let j_count = rows.len();
    let num_rows = j_count + 1;
    let obj_row = j_count;
    let obj_offset = obj_row * num_cols;
    // Maximise `-sum(artificials)`: the objective row starts with `+1` on every artificial
    // column, and each artificial's constraint row is then subtracted so the basic
    // artificial columns end at a reduced cost of exactly zero.
    for a in 0..artificial_count {
        scratch.tableau[obj_offset + 7 + j_count + a] = 1.0;
    }
    for (j, row) in rows.iter().enumerate() {
        if row.offset < 0.0 {
            let r_offset = j * num_cols;
            for c in 0..num_cols {
                scratch.tableau[obj_offset + c] -= scratch.tableau[r_offset + c];
            }
        }
    }

    loop {
        *iterations += 1;
        if *iterations > max_iterations {
            return None;
        }

        // A nonbasic artificial never re-enters: its constraint row is already satisfied
        // with the artificial at zero, so the restricted problem is exact.
        let Some(enter_col) = find_entering_col(&scratch.tableau, num_cols, obj_row, 7 + j_count)
        else {
            break;
        };

        let leave_row = find_leaving_row(
            &scratch.tableau,
            num_cols,
            j_count,
            &scratch.basic_vars,
            enter_col,
        )?;

        pivot(
            &mut scratch.tableau,
            num_cols,
            num_rows,
            &mut scratch.basic_vars,
            leave_row,
            enter_col,
        );
    }

    let rhs_col = 7 + j_count + artificial_count;
    let phase1_val = scratch.tableau[obj_row * num_cols + rhs_col];
    // The rounding error left in the phase-1 objective grows with the size of the offsets, so
    // the infeasibility tolerance scales with the largest one (never below 1e-9).
    let offset_scale = rows.iter().fold(1.0_f64, |m, row| m.max(row.offset.abs()));
    if phase1_val < -1e-9 * offset_scale {
        return None;
    }

    drive_out_artificials(scratch, j_count, num_cols);
    Some(())
}

/// Pivots every artificial variable still basic after phase 1 out of the basis, on the real
/// column (structural or slack) with the largest `|entry|` of its row. A row whose real
/// entries are all negligible is redundant: its artificial stays basic at zero.
fn drive_out_artificials(scratch: &mut LpScratch, j_count: usize, num_cols: usize) {
    let real_cols = 7 + j_count;
    let num_rows = j_count + 1;
    for r in 0..j_count {
        if scratch.basic_vars[r] < real_cols {
            continue;
        }
        let r_offset = r * num_cols;
        let row = &scratch.tableau[r_offset..r_offset + real_cols];
        let max_abs = row.iter().fold(1.0_f64, |m, &x| m.max(x.abs()));
        let (best_col, best_abs) =
            row.iter()
                .enumerate()
                .fold((0, 0.0_f64), |(col, abs), (c, &x)| {
                    if x.abs() > abs {
                        (c, x.abs())
                    } else {
                        (col, abs)
                    }
                });
        if best_abs > 1e-9 * max_abs {
            pivot(
                &mut scratch.tableau,
                num_cols,
                num_rows,
                &mut scratch.basic_vars,
                r,
                best_col,
            );
        }
    }
}

fn run_phase2(
    scratch: &mut LpScratch,
    j_count: usize,
    num_cols: usize,
    max_iterations: usize,
    iterations: &mut usize,
) -> Option<()> {
    let num_rows = j_count + 1;
    let obj_row = j_count;
    let obj_offset = obj_row * num_cols;
    scratch.tableau[obj_offset..obj_offset + num_cols].fill(0.0);
    scratch.tableau[obj_offset + 6] = -1.0;

    for r in 0..j_count {
        if scratch.basic_vars[r] == 6 {
            let r_offset = r * num_cols;
            for c in 0..num_cols {
                scratch.tableau[obj_offset + c] += scratch.tableau[r_offset + c];
            }
            break;
        }
    }

    loop {
        *iterations += 1;
        if *iterations > max_iterations {
            return None;
        }

        let Some(enter_col) = find_entering_col(&scratch.tableau, num_cols, obj_row, 7 + j_count)
        else {
            break;
        };

        let leave_row = find_leaving_row(
            &scratch.tableau,
            num_cols,
            j_count,
            &scratch.basic_vars,
            enter_col,
        )?;

        pivot(
            &mut scratch.tableau,
            num_cols,
            num_rows,
            &mut scratch.basic_vars,
            leave_row,
            enter_col,
        );
    }

    Some(())
}

fn extract_solution(
    scratch: &LpScratch,
    j_count: usize,
    num_cols: usize,
    rhs_col: usize,
) -> (f64, [f64; 3]) {
    let mut t_pos = [0.0; 3];
    let mut t_neg = [0.0; 3];
    let mut k = 0.0;

    for j in 0..j_count {
        let b = scratch.basic_vars[j];
        let val = scratch.tableau[j * num_cols + rhs_col];
        if b == 6 {
            k = val;
        } else if b < 3 {
            t_pos[b] = val;
        } else if (3..6).contains(&b) {
            t_neg[b - 3] = val;
        }
    }

    let t = [
        t_pos[0] - t_neg[0],
        t_pos[1] - t_neg[1],
        t_pos[2] - t_neg[2],
    ];

    (k, t)
}

fn post_check_feasibility(rows: &[ScaleRow], t: [f64; 3], mut k: f64) -> Option<(f64, [f64; 3])> {
    let mut max_shrink: f64 = 0.0;
    for row in rows {
        let dot_t = row.normal[2].mul_add(t[2], row.normal[0].mul_add(t[0], row.normal[1] * t[1]));
        let lhs = k.mul_add(row.support, dot_t);
        let violation = lhs - row.offset;
        let tol = 1e-9 * (1.0 + row.offset.abs());
        if violation > tol {
            if row.support > 1e-12 {
                let shrink = violation / row.support;
                if shrink > max_shrink {
                    max_shrink = shrink;
                }
            } else {
                return None;
            }
        }
    }

    if max_shrink > 0.0 {
        k -= max_shrink;
    }
    if k < 0.0 {
        if k >= -1e-9 {
            k = 0.0;
        } else {
            return None;
        }
    }

    Some((k, t))
}

fn find_entering_col(
    tableau: &[f64],
    num_cols: usize,
    obj_row: usize,
    candidate_cols: usize,
) -> Option<usize> {
    let obj_offset = obj_row * num_cols;
    let max_abs = tableau[obj_offset..obj_offset + candidate_cols]
        .iter()
        .fold(1.0_f64, |m, &x| m.max(x.abs()));
    let tol = 1e-12 * max_abs;

    (0..candidate_cols).find(|&c| tableau[obj_offset + c] < -tol)
}

fn find_leaving_row(
    tableau: &[f64],
    num_cols: usize,
    j_count: usize,
    basic_vars: &[usize],
    enter_col: usize,
) -> Option<usize> {
    let rhs_col = num_cols - 1;
    // The best row so far as `(row, ratio, basic variable)`.
    let mut best: Option<(usize, f64, usize)> = None;

    for (r, &basic_var) in basic_vars.iter().enumerate().take(j_count) {
        let r_offset = r * num_cols;
        let coeff = tableau[r_offset + enter_col];
        // The pivot tolerance is at least 1e-12 (its row scale is at least 1), so a
        // coefficient at or below that can never qualify and the row scan is skipped.
        if coeff.is_nan() || coeff <= 1e-12 {
            continue;
        }
        let max_abs = tableau[r_offset..r_offset + num_cols - 1]
            .iter()
            .fold(1.0_f64, |m, &x| m.max(x.abs()));
        if coeff <= 1e-12 * max_abs {
            continue;
        }
        let ratio = (tableau[r_offset + rhs_col] / coeff).max(0.0);
        let better = match best {
            None => true,
            Some((_, min_ratio, best_basic_var)) => {
                let tol = 1e-12 * (1.0 + min_ratio.abs());
                ratio < min_ratio - tol
                    || ((ratio - min_ratio).abs() <= tol && basic_var < best_basic_var)
            }
        };
        if better {
            best = Some((r, ratio, basic_var));
        }
    }
    best.map(|(row, _, _)| row)
}

fn pivot(
    tableau: &mut [f64],
    num_cols: usize,
    num_rows: usize,
    basic_vars: &mut [usize],
    pivot_row: usize,
    enter_col: usize,
) {
    let p_offset = pivot_row * num_cols;
    let pivot_val = tableau[p_offset + enter_col];
    let inv_pivot = 1.0 / pivot_val;

    for c in 0..num_cols {
        tableau[p_offset + c] *= inv_pivot;
    }
    tableau[p_offset + enter_col] = 1.0;

    for r in 0..num_rows {
        if r != pivot_row {
            let r_offset = r * num_cols;
            let factor = tableau[r_offset + enter_col];
            if factor.abs() > 1e-15 {
                for c in 0..num_cols {
                    tableau[r_offset + c] =
                        factor.mul_add(-tableau[p_offset + c], tableau[r_offset + c]);
                }
                tableau[r_offset + enter_col] = 0.0;
            }
        }
    }
    basic_vars[pivot_row] = enter_col;
}

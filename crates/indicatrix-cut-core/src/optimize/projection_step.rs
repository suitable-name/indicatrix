//! A bounded correction for a nonlinear meet system, not a feasibility certificate.
//!
//! Redundant contacts and production-plane rounding make a finite-difference
//! Jacobian inconsistent away from the solution. Solve its least-squares problem
//! instead of rejecting such a linearization as an impossible authored design.

/// Damped QR in physical trust-region units, without forming J^T J. Derivatives
/// are never normalized by their own magnitude: doing so amplifies rounding-only
/// directions. Final geometry validation remains the caller's responsibility.
pub(super) fn correction(
    columns: &[Vec<f64>],
    residual: &[f64],
    parameter_scales: &[f64],
) -> Option<Vec<f64>> {
    const DAMPING: f64 = 1e-8;
    let n = columns.len();
    let m = residual.len();
    if n == 0 || parameter_scales.len() != n || residual.iter().any(|r| !r.is_finite()) {
        return None;
    }
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(n);
    let mut triangular = vec![vec![0.0; n]; n];
    let mut rhs = Vec::with_capacity(n);
    for (j, column) in columns.iter().enumerate() {
        if column.len() != m || column.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let scale = parameter_scales[j];
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let mut q = vec![0.0; m + n];
        for (dst, value) in q.iter_mut().zip(column) {
            *dst = value * scale;
        }
        q[m + j] = DAMPING.sqrt();
        // Reorthogonalization avoids losing small independent directions.
        for _ in 0..2 {
            for (i, previous) in basis.iter().enumerate() {
                let dot: f64 = q.iter().zip(previous).map(|(a, b)| a * b).sum();
                triangular[i][j] += dot;
                for (value, component) in q.iter_mut().zip(previous) {
                    *value -= dot * component;
                }
            }
        }
        let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return None;
        }
        triangular[j][j] = norm;
        for value in &mut q {
            *value /= norm;
        }
        rhs.push(-q.iter().zip(residual).map(|(a, b)| a * b).sum::<f64>());
        basis.push(q);
    }
    let mut step = vec![0.0; n];
    for i in (0..n).rev() {
        let later: f64 = ((i + 1)..n).map(|j| triangular[i][j] * step[j]).sum();
        step[i] = (rhs[i] - later) / triangular[i][i];
    }
    for (value, scale) in step.iter_mut().zip(parameter_scales) {
        *value *= scale;
    }
    step.iter().all(|v| v.is_finite()).then_some(step)
}

/// Shorten the whole direction to fit the remaining trust box, keeping its coupled
/// angle/depth ratios intact. The caller still checks authored bounds and line search.
pub(super) fn bound_correction(
    step: &mut [f64],
    current: &[f64],
    origin: &[f64],
    limits: &[f64],
) -> Option<()> {
    if [current.len(), origin.len(), limits.len()]
        .into_iter()
        .any(|n| n != step.len())
    {
        return None;
    }
    let mut fraction = 1.0_f64;
    for (((delta, value), start), limit) in step.iter().zip(current).zip(origin).zip(limits) {
        if !delta.is_finite()
            || !value.is_finite()
            || !start.is_finite()
            || !limit.is_finite()
            || *limit <= 0.0
        {
            return None;
        }
        if *delta != 0.0 {
            let room = if *delta > 0.0 {
                start + limit - value
            } else {
                value - (start - limit)
            };
            fraction = fraction.min(room.max(0.0) / delta.abs());
        }
    }
    if fraction <= 0.0 {
        return None;
    }
    for value in step {
        *value *= fraction;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::{bound_correction, correction};

    #[test]
    fn dependent_contacts_with_rounding_disagreement_still_have_a_correction() {
        let step = correction(
            &[vec![1.0, 1.0], vec![1.0, 1.0]],
            &[-1.0, -1.00001],
            &[1.0, 1.0],
        )
        .expect("an inconsistent linearization is not an infeasibility verdict");
        assert!((step[0] - 0.500_002_5).abs() < 1e-5);
        assert!((step[1] - 0.500_002_5).abs() < 1e-5);
        let remaining: f64 = [-1.0, -1.00001]
            .iter()
            .map(|r| (r + step[0] + step[1]).powi(2))
            .sum();
        assert!(remaining < 1e-9);
    }

    #[test]
    fn locked_columns_stay_zero_and_parameter_units_do_not_lose_rank() {
        let step = correction(
            &[vec![0.0, 0.0], vec![0.1, 0.0], vec![0.0, 10.0]],
            &[-0.2, -0.5],
            &[5.0, 5.0, 0.1],
        )
        .expect("a locked variable does not prevent other variables moving");
        assert_eq!(step[0], 0.0);
        assert!((step[1] - 2.0).abs() < 1e-5);
        assert!((step[2] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn rounding_only_direction_does_not_overwhelm_a_useful_correction() {
        let step = correction(
            &[vec![1e-9, 0.0], vec![0.0, 1.0]],
            &[-1e-6, -0.02],
            &[5.0, 0.1],
        )
        .unwrap();
        assert!(
            step[0].abs() < 1e-3,
            "rounding noise must not request a huge angle move"
        );
        assert!((step[1] - 0.02).abs() < 1e-6);
    }

    #[test]
    fn correction_uses_remaining_trust_room_without_changing_its_direction() {
        let mut step = [20.0, -0.2];
        bound_correction(&mut step, &[3.0, 0.0], &[0.0, 0.0], &[5.0, 0.1]).unwrap();
        assert!((step[0] - 2.0).abs() < 1e-12);
        assert!((step[1] + 0.02).abs() < 1e-12);
    }
}

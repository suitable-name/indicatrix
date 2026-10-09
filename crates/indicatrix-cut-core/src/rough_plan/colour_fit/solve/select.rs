//! Model selection (plan section 6.2): an information criterion and a likelihood ratio test.
//!
//! * `chi2` is the plain sum of squared whitened residuals of the data term; `k` the effective
//!   number of parameters `trace(H^-1 H_data)` (a smoothness prior makes a model use fewer than
//!   its raw count).
//! * The noise may be larger than the noise model says (a model that cannot fit leaves residual
//!   structure): the common scale `s2 = max(1, min over models of chi2 / (n - k))`, and
//!   `AIC = chi2 / s2 + 2 k`.
//! * The chromophore model A is "not significantly worse" than the smooth basis B when
//!   `(chi2_A - chi2_B) / s2` stays below the 99 % chi-square quantile for `max(1, k_B - k_A)`
//!   degrees of freedom (Wilson-Hilferty approximation of the quantile, accurate to a few
//!   percent from one degree of freedom up).

use super::output::{ChoiceReason, LikelihoodRatio, ModelComparison, ModelKind, ModelScore};

/// The standard normal quantile of 0.99.
const Z_99: f64 = 2.326_347_874_040_841;

/// The chi-square quantile for `df` degrees of freedom at the normal quantile `z`
/// (Wilson-Hilferty).
#[must_use]
pub(super) fn chi_square_quantile(z: f64, df: f64) -> f64 {
    let df = df.max(1.0);
    let t = 2.0 / (9.0 * df);
    df * (1.0 - t + z * t.sqrt()).powi(3)
}

/// The inputs of the comparison for one model.
pub(super) struct Candidate {
    pub kind: ModelKind,
    pub chi2: f64,
    pub effective_params: f64,
}

/// Compares the candidates (the chromophore model first when present) and chooses one.
///
/// `n_data` is the number of residuals; `preferred` overrides the rule when that model was
/// fitted. Returns the comparison and the index of the chosen candidate.
pub(super) fn compare(
    candidates: &[Candidate],
    n_data: usize,
    preferred: Option<ModelKind>,
) -> (ModelComparison, usize) {
    let scale2 = candidates
        .iter()
        .map(|c| {
            let dof = (n_data as f64 - c.effective_params).max(1.0);
            c.chi2 / dof
        })
        .fold(f64::INFINITY, f64::min)
        .max(1.0);
    let scores: Vec<ModelScore> = candidates
        .iter()
        .map(|c| ModelScore {
            kind: c.kind,
            chi2: c.chi2,
            effective_params: c.effective_params,
            aic: c.chi2 / scale2 + 2.0 * c.effective_params,
        })
        .collect();
    let a = candidates
        .iter()
        .position(|c| c.kind == ModelKind::Chromophore);
    let b = candidates
        .iter()
        .position(|c| c.kind == ModelKind::SmoothBasis);

    let likelihood_ratio = match (a, b) {
        (Some(a), Some(b)) => {
            let statistic = (candidates[a].chi2 - candidates[b].chi2) / scale2;
            let dof = (candidates[b].effective_params - candidates[a].effective_params).max(1.0);
            let critical = chi_square_quantile(Z_99, dof);
            Some(LikelihoodRatio {
                statistic,
                dof,
                critical,
                chromophore_significantly_worse: statistic > critical,
            })
        }
        _ => None,
    };

    let (chosen_index, reason) = if let Some(kind) = preferred
        && let Some(i) = candidates.iter().position(|c| c.kind == kind)
    {
        (i, ChoiceReason::UserPreference)
    } else if let (Some(a), Some(b), Some(lr)) = (a, b, likelihood_ratio) {
        if lr.chromophore_significantly_worse {
            (b, ChoiceReason::ChromophoreSignificantlyWorse)
        } else {
            (a, ChoiceReason::ChromophoreAcceptable)
        }
    } else {
        (0, ChoiceReason::OnlyOneModel)
    };

    (
        ModelComparison {
            scores,
            noise_scale2: scale2,
            likelihood_ratio,
            chosen: candidates[chosen_index].kind,
            reason,
        },
        chosen_index,
    )
}

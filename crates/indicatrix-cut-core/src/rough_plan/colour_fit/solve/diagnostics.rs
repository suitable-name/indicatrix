//! Validation inside the fit (plan section 6.5): leave-one-view-out cross-validation and the
//! residual maps with their spatial-structure score.
//!
//! # Cross-validation
//!
//! For every view, the model is refitted on the other views (warm start from the full fit, at
//! most `lovo_iterations` Levenberg-Marquardt iterations) and used to predict the held-out view.
//! The comparison is the CIEDE2000 between the held-out view's mean colour and the prediction's
//! mean colour over the same pixels, both read as linear sRGB relative to white (a transmittance
//! colour). The held-out view's gain is not known, so it is estimated from that view alone in
//! closed form (`sum s^2 p o / sum s^2 p^2`, bounded to `e^-1 .. e`); the table also carries the
//! value with the gain of the full fit. Exposure differences between views are a nuisance, so
//! the first number is the honest colour error and the second shows how much the gain absorbed.
//!
//! # Structure
//!
//! Moran's I of the whitened residual of each channel on the working grid (rook neighbours,
//! valid pixels only) measures spatial autocorrelation: about 0 for noise, towards 1 for smooth
//! blobs of the same sign, which is what an unmodelled zone looks like. A view scores its worst
//! channel; the fit scores the pixel-weighted mean. Zoning is suggested when the score exceeds
//! [`SUGGEST_MORAN`] and the whitened residual is not noise-sized (rms above [`SUGGEST_RMS`]).

use std::sync::atomic::AtomicBool;

use super::{
    FitConfig, FitError,
    lm::{LmSettings, run_lm},
    models::{FitModel, PriorContext, SmoothBasisModel},
    output::{LovoEntry, LovoReport, ResidualMap},
    par::par_map,
    problem::{GAIN_LOG_LIMIT, Problem, rgb_delta_e},
};

/// Moran's I above which the residual counts as structured.
pub const SUGGEST_MORAN: f64 = 0.25;
/// The whitened rms above which a structured residual is worth a question to the user.
pub const SUGGEST_RMS: f64 = 1.3;

/// Leave-one-view-out cross-validation of the model at `x` (`[params, log-gains]`) with the
/// L1 weights `l1` of the final fit. `None` when fewer than two views have usable pixels.
pub(super) fn lovo(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x: &[f64],
    l1: &[f64],
    config: &FitConfig,
    cancel: &AtomicBool,
) -> Result<Option<LovoReport>, FitError> {
    let np = model.n_params();
    let nv = problem.view_count();
    let slots: Vec<usize> = (0..nv)
        .filter(|&s| problem.views[s].used_pixels > 0)
        .collect();
    if slots.len() < 2 {
        return Ok(None);
    }
    let settings = LmSettings::new(config.lovo_iterations, config.rel_cost_tol);
    let work = |i: usize| -> Result<LovoEntry, FitError> {
        let slot = slots[i];
        let copy = model.duplicate();
        let mut active = vec![true; nv];
        active[slot] = false;
        let ctx = PriorContext {
            config,
            l1_weights: l1,
            pin_common_gain: false,
        };
        let outcome = run_lm(problem, &*copy, x, &active, &ctx, &settings, cancel)?;
        let stats = problem.view_stats(&*copy, &outcome.x[..np], slot)?;
        let own_gain = if stats.s_pp > 0.0 {
            (stats.s_po / stats.s_pp).clamp((-GAIN_LOG_LIMIT).exp(), GAIN_LOG_LIMIT.exp())
        } else {
            1.0
        };
        let scaled = |gain: f64| stats.mean_pred.map(|v| v * gain);
        let full_gain = x[np + slot].exp();
        Ok(LovoEntry {
            view: problem.views[slot].view,
            pixels: stats.pixels,
            delta_e: rgb_delta_e(stats.mean_obs, scaled(own_gain)),
            delta_e_full_fit_gain: rgb_delta_e(stats.mean_obs, scaled(full_gain)),
            gain: own_gain,
        })
    };
    let results = par_map(slots.len(), config.threads, cancel, &work);
    let mut entries = Vec::with_capacity(slots.len());
    for result in results {
        match result {
            None => return Err(FitError::Cancelled),
            Some(Err(e)) => return Err(e),
            Some(Ok(entry)) => entries.push(entry),
        }
    }
    let mut sorted: Vec<f64> = entries.iter().map(|e| e.delta_e).collect();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    let median = if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    };
    let max = sorted.last().copied().unwrap_or(0.0);
    Ok(Some(LovoReport {
        entries,
        median_delta_e: median,
        max_delta_e: max,
    }))
}

/// Moran's I of one channel on a `width x height` grid; 0 when it is undefined.
pub(super) fn moran_i(values: &[f64], valid: &[bool], width: usize, height: usize) -> f64 {
    let count = valid.iter().filter(|v| **v).count();
    if count < 4 {
        return 0.0;
    }
    let mean = values
        .iter()
        .zip(valid)
        .filter(|(_, ok)| **ok)
        .map(|(v, _)| *v)
        .sum::<f64>()
        / count as f64;
    let mut denominator = 0.0;
    for (v, ok) in values.iter().zip(valid) {
        if *ok {
            denominator += (v - mean) * (v - mean);
        }
    }
    if denominator <= 0.0 {
        return 0.0;
    }
    let (mut numerator, mut pairs) = (0.0, 0_usize);
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !valid[i] {
                continue;
            }
            let di = values[i] - mean;
            if x + 1 < width && valid[i + 1] {
                numerator += di * (values[i + 1] - mean);
                pairs += 1;
            }
            if y + 1 < height && valid[i + width] {
                numerator += di * (values[i + width] - mean);
                pairs += 1;
            }
        }
    }
    if pairs == 0 {
        return 0.0;
    }
    count as f64 / pairs as f64 * numerator / denominator
}

/// The residual maps with their scores.
pub(super) struct Structure {
    pub maps: Vec<ResidualMap>,
    pub score: f64,
    pub rms: f64,
}

/// Scores the structure of the residual at `x`.
pub(super) fn structure(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x: &[f64],
) -> Result<Structure, FitError> {
    let maps = problem.residual_maps(model, x)?;
    Ok(score_maps(maps))
}

/// The scores of residual maps.
fn score_maps(mut maps: Vec<ResidualMap>) -> Structure {
    let (mut score_sum, mut square_sum, mut weight) = (0.0, 0.0, 0.0);
    for map in &mut maps {
        let count = map.valid.iter().filter(|v| **v).count();
        let mut worst = f64::NEG_INFINITY;
        for c in 0..3 {
            let channel: Vec<f64> = map.whitened.iter().map(|u| f64::from(u[c])).collect();
            worst = worst.max(moran_i(&channel, &map.valid, map.width, map.height));
        }
        map.moran = if worst.is_finite() { worst } else { 0.0 };
        let squares: f64 = map
            .whitened
            .iter()
            .zip(&map.valid)
            .filter(|(_, ok)| **ok)
            .map(|(u, _)| u.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>())
            .sum();
        map.rms = if count > 0 {
            (squares / (3 * count) as f64).sqrt()
        } else {
            0.0
        };
        if count >= 8 {
            score_sum += count as f64 * map.moran;
            square_sum += squares;
            weight += count as f64;
        }
    }
    let (score, rms) = if weight > 0.0 {
        (score_sum / weight, (square_sum / (3.0 * weight)).sqrt())
    } else {
        (0.0, 0.0)
    };
    Structure { maps, score, rms }
}

/// The data term of the fit evaluated at a GIVEN absorption instead of a fitted one (round 2,
/// C2): what the objective is at the truth.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveAt {
    /// `sum u^2` over the whitened residuals, the fit's own `chi2` for the same weights.
    pub chi2: f64,
    /// The residuals in the sum (channels with a usable weight).
    pub n_data: usize,
    /// The pixel-weighted Moran's I of the whitened residual (as `ColourFit::structured_score`).
    pub moran: f64,
    /// The pixel-weighted root mean square whitened residual.
    pub rms: f64,
    /// The robust (Huber) data term, the part of the fit's `cost` that the data make.
    pub data_cost: f64,
    /// The prior terms at the truth (round 3, D1.3), `(name, value)`: the gain prior at the given
    /// gains, and the smoothness and magnitude priors of model B at the least-squares projection
    /// of the truth onto the basis (the truth is not exactly in the basis, see
    /// `projection_max_error`).
    pub prior_terms: Vec<(String, f64)>,
    /// The sum of `prior_terms`.
    pub prior_cost: f64,
    /// `data_cost + prior_cost`: comparable with `ModelFit::cost` of a fit of the same photos.
    pub total: f64,
    /// The largest absolute difference, per mm, between the truth and its projection on the
    /// 400 to 700 nm grid of 5 nm.
    pub projection_max_error: f64,
}

/// Evaluates the data term at the absorption `alpha(zone, lambda_nm)` (1/mm).
///
/// Uses the per-view `log_gains` (one per view in the order of the records, so `gamma_v = ln`
/// of the exposure factor of the photo) and exactly the weights `config` gives the fit.
///
/// A fit whose `chi2` is not clearly below this value has not reached the noise floor of the
/// model that made the photos. `chi2 / n_data` near 1 at the truth and well above 1 at the fit
/// means the optimiser or a degeneracy lost the truth; `chi2 / n_data` far above 1 at the truth
/// itself means the photos and the records disagree (a generator or tracer mismatch).
///
/// # Errors
///
/// As [`Problem::new`](super::problem::Problem::new).
pub fn objective_at(
    records: &crate::rough_plan::colour_fit::forward::ForwardRecords,
    observed: &[super::ObservedView],
    config: &FitConfig,
    alpha: &(dyn Fn(usize, f64) -> f64 + Sync),
    log_gains: &[f64],
) -> Result<ObjectiveAt, FitError> {
    config.validate()?;
    let problem = Problem::new(records, observed, config)?;
    let model = FixedAlpha { alpha };
    let all = problem.whitened_with(&model, log_gains)?;
    let mut chi2 = 0.0;
    let mut data_cost = 0.0;
    let k = config.huber_k;
    let mut n_data = 0;
    let mut maps = Vec::with_capacity(all.len());
    for (view, pixels) in problem.views.iter().zip(all) {
        for (p, u) in pixels.iter().enumerate() {
            if let Some(u) = u {
                for c in 0..3 {
                    if view.inv_sigma[p][c] != 0.0 {
                        chi2 += u[c] * u[c];
                        let a = u[c].abs();
                        data_cost += if a <= k {
                            0.5 * a * a
                        } else {
                            k * a - 0.5 * k * k
                        };
                        n_data += 1;
                    }
                }
            }
        }
        maps.push(ResidualMap {
            view: view.view,
            width: view.width,
            height: view.height,
            whitened: pixels
                .iter()
                .map(|p| p.map_or([0.0; 3], |u| [u[0] as f32, u[1] as f32, u[2] as f32]))
                .collect(),
            valid: pixels.iter().map(Option::is_some).collect(),
            moran: 0.0,
            rms: 0.0,
        });
    }
    let scored = score_maps(maps);
    // The priors at the truth: those of the least-squares projection onto the basis.
    let basis_model = SmoothBasisModel::new(records.n_zones);
    let (theta, projection_max_error) = SmoothBasisModel::project(records.n_zones, alpha);
    let ones = vec![1.0; theta.len()];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
    };
    let terms = problem.prior_terms_at(&basis_model, &theta, log_gains, &ctx);
    let prior_cost = terms.total();
    Ok(ObjectiveAt {
        chi2,
        n_data,
        moran: scored.score,
        rms: scored.rms,
        data_cost,
        prior_terms: terms.named(),
        prior_cost,
        total: data_cost + prior_cost,
        projection_max_error,
    })
}

/// A spectral model with no parameters: a fixed absorption.
struct FixedAlpha<'a> {
    alpha: &'a (dyn Fn(usize, f64) -> f64 + Sync),
}

impl crate::rough_plan::colour_fit::forward::SpectralModel for FixedAlpha<'_> {
    fn n_params(&self) -> usize {
        0
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, _params: &[f64]) -> f64 {
        (self.alpha)(zone, lambda_nm)
    }

    fn dalpha(&self, _zone: usize, _lambda_nm: f64, _params: &[f64], _out: &mut [f64]) {}
}

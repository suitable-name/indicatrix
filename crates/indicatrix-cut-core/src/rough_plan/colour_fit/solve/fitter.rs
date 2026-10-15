//! Fitting a model to the records.
//!
//! Deterministic multi-start, the reweighted L1 rounds of the chromophore model, the posterior
//! covariance, and [`fit_records`], which fits every available model, compares them and
//! validates the chosen one.
//!
//! # Multi-start and stages (round 3, D1)
//!
//! `config.seeds` (8) deterministic starts run Levenberg-Marquardt for `stage1_iterations` (12)
//! each with ALL gains held at 1, in parallel when `config.threads` allows (every start is
//! independent, so the result does not depend on the thread count). The best `finalists` (2) by
//! objective, ties by seed order, continue through three stages ([`run_staged`]): the
//! absorption with the gains at 1, then the per-view deviations, then the common mode. Two
//! structured starts from the best finalist (the flat component removed, one band at the most
//! absorbed channel) are staged too, and the lowest total objective wins. A warm start is one
//! plain run with every parameter free.

use std::sync::atomic::AtomicBool;

use indicatrix::{
    color::body_color::{body_color, delta_e_2000},
    optics::{
        absorption::{AbsorptionTensor, BODY_COLOR_BASIS_NM},
        zoning::ZoneAbsorption,
    },
};

use super::{
    FitConfig, FitError, FitProgress, FitStage, ObservedView,
    diagnostics::{SUGGEST_MORAN, SUGGEST_RMS, lovo, structure},
    extremes::{ExtremeInput, constrained_extremes},
    linalg::{inverse_spd_ridged, trace_product},
    lm::{LmOutcome, LmSettings, bounds, run_lm},
    models::{ChromophoreModel, FitModel, PriorContext, SmoothBasisModel},
    output::{
        BIRGE_THRESHOLD, ColourFit, FIT_VERSION, FitWarning, ModelFit, ModelKind, WarningKind,
    },
    par::{inner_threads, par_map_progress},
    predict::{candidate_point, face_up_path_mm, metamer_steps, predictions, zone_lab},
    problem::{Normal, Problem},
    select::{Candidate, compare},
};
use crate::rough_plan::colour_fit::forward::{ForwardRecords, status};

/// A converged fit of one model.
pub(super) struct Fitted {
    pub x: Vec<f64>,
    pub normal: Normal,
    pub iterations: usize,
    /// The iterations of each stage on its own: the multi-start's first stage (absent when a
    /// structured start won), the three staged runs of the winner and one entry per reweighted L1
    /// round (one entry for a warm start).
    pub iterations_per_stage: Vec<usize>,
    pub converged: bool,
    pub seed_costs: Vec<f64>,
    pub l1: Vec<f64>,
}

fn collect_ok<T>(results: Vec<Option<Result<T, FitError>>>) -> Result<Vec<T>, FitError> {
    let mut out = Vec::with_capacity(results.len());
    for result in results {
        match result {
            None => return Err(FitError::Cancelled),
            Some(Err(e)) => return Err(e),
            Some(Ok(value)) => out.push(value),
        }
    }
    Ok(out)
}

/// Runs the three stages of round 3, D1 from `x0` (its gains are reset to 1):
///
/// 1. the absorption with ALL gains held at 1 (`stage_one_budget` iterations);
/// 2. the per-view deviations freed, the common mode pinned;
/// 3. the common mode freed too (skipped when `config.fix_common_gain` already pins it).
///
/// Each stage starts from the previous result and gets `config.max_iterations` (the first one
/// `stage_one_budget`). The returned outcome is the last stage's, with the iterations summed;
/// `stages` holds the iterations of every stage on its own (so that a stage that hit its cap is
/// not hidden in the sum).
struct Staged {
    out: LmOutcome,
    stages: Vec<usize>,
}

/// The first-stage iteration budget of [`run_staged`] and the threads of its pixel passes.
#[derive(Clone, Copy)]
struct StageLimits {
    /// Iterations of stage 1.
    stage_one_budget: usize,
    /// Threads per normal-equation pixel pass (`par::inner_threads` of the running phase).
    inner_threads: usize,
}

fn run_staged(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    x0: &[f64],
    ctx: &PriorContext<'_>,
    config: &FitConfig,
    cancel: &AtomicBool,
    limits: StageLimits,
) -> Result<Staged, FitError> {
    let np = model.n_params();
    let active = vec![true; problem.view_count()];
    let StageLimits {
        stage_one_budget,
        inner_threads,
    } = limits;
    let settings = |max_iterations: usize, freeze_gains: bool, pin_common_gain: bool| LmSettings {
        max_iterations,
        rel_tol: config.rel_cost_tol,
        freeze_gains,
        pin_common_gain,
        inner_threads,
    };
    let mut start = x0.to_vec();
    for gain in &mut start[np..] {
        *gain = 0.0;
    }
    let mut out = run_lm(
        problem,
        model,
        &start,
        &active,
        ctx,
        &settings(stage_one_budget.max(1), true, false),
        cancel,
    )?;
    let mut stages = vec![out.iterations];
    out = run_lm(
        problem,
        model,
        &out.x,
        &active,
        ctx,
        &settings(config.max_iterations, false, true),
        cancel,
    )?;
    stages.push(out.iterations);
    if !config.fix_common_gain {
        out = run_lm(
            problem,
            model,
            &out.x,
            &active,
            ctx,
            &settings(config.max_iterations, false, false),
            cancel,
        )?;
        stages.push(out.iterations);
    }
    out.iterations = stages.iter().sum();
    Ok(Staged { out, stages })
}

/// The deterministic multi-start of a cold fit (round 3, D1):
///
/// * `config.seeds` starts run the first stage (gains held at 1, `stage1_iterations`), in
///   parallel; the best `finalists` by objective (ties by seed order) go through the three
///   stages of [`run_staged`];
/// * two structured starts from the best finalist (`FitModel::structured_starts`: the flat
///   component of the absorption removed, one band at the most absorbed channel) go through the
///   same stages from gains of 1;
/// * the lowest TOTAL objective (data plus priors) wins, ties by start order.
///
/// Returns the winner, the seeds' objectives after the first stage (in seed order) and the
/// iterations of each of the winner's stages. `progress` gets the finished share in `0.0..=1.0`
/// (first stage 30 %, the finalists 40 %, the structured starts 30 %).
fn multi_start(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    config: &FitConfig,
    cancel: &AtomicBool,
    ctx: &PriorContext<'_>,
    progress: &mut dyn FnMut(f32),
) -> Result<(LmOutcome, Vec<f64>, Vec<usize>), FitError> {
    let share = |from: f32, to: f32, finished: usize, count: usize| -> f32 {
        from + (to - from) * (finished as f32 / count.max(1) as f32)
    };
    let nv = problem.view_count();
    let np = model.n_params();
    let active = vec![true; nv];
    let starts: Vec<Vec<f64>> = (0..config.seeds.max(1))
        .map(|s| {
            let mut x = model.seed(s);
            x.extend(std::iter::repeat_n(0.0, nv));
            x
        })
        .collect();
    let first_stage = LmSettings {
        max_iterations: config.stage1_iterations.min(config.max_iterations),
        rel_tol: config.rel_cost_tol,
        freeze_gains: true,
        pin_common_gain: false,
        inner_threads: inner_threads(config.threads, starts.len()),
    };
    let stage_one = {
        let work = |i: usize| -> Result<LmOutcome, FitError> {
            let copy = model.duplicate();
            run_lm(
                problem,
                &*copy,
                &starts[i],
                &active,
                ctx,
                &first_stage,
                cancel,
            )
        };
        let count = starts.len();
        collect_ok(par_map_progress(
            count,
            config.threads,
            cancel,
            &work,
            &mut |finished| progress(share(0.0, 0.3, finished, count)),
        ))?
    };
    let seed_costs: Vec<f64> = stage_one.iter().map(|o| o.normal.cost).collect();

    let mut order: Vec<usize> = (0..stage_one.len()).collect();
    order.sort_by(|&a, &b| seed_costs[a].total_cmp(&seed_costs[b]).then(a.cmp(&b)));
    let finalists: Vec<usize> = order.into_iter().take(config.finalists.max(1)).collect();

    let staged = {
        let finalist_inner = inner_threads(config.threads, finalists.len());
        let work = |i: usize| -> Result<Staged, FitError> {
            let copy = model.duplicate();
            let start = &stage_one[finalists[i]];
            let done = start.iterations;
            let budget = config.max_iterations.saturating_sub(done).max(1);
            let limits = StageLimits {
                stage_one_budget: budget,
                inner_threads: finalist_inner,
            };
            let mut staged = run_staged(problem, &*copy, &start.x, ctx, config, cancel, limits)?;
            staged.out.iterations += done;
            // The multi-start's own first stage comes first.
            staged.stages.insert(0, done);
            Ok(staged)
        };
        let count = finalists.len();
        collect_ok(par_map_progress(
            count,
            config.threads,
            cancel,
            &work,
            &mut |finished| progress(share(0.3, 0.7, finished, count)),
        ))?
    };

    // Structured starts from the best finalist.
    let leader = staged
        .iter()
        .enumerate()
        .min_by(|(ia, a), (ib, b)| {
            a.out
                .normal
                .cost
                .total_cmp(&b.out.normal.cost)
                .then(ia.cmp(ib))
        })
        .map(|(i, _)| i)
        .ok_or(FitError::Numerical("no start produced a fit"))?;
    let extra_starts: Vec<Vec<f64>> = model
        .structured_starts(&staged[leader].out.x[..np])
        .into_iter()
        .map(|mut x| {
            x.extend(std::iter::repeat_n(0.0, nv));
            x
        })
        .collect();
    let extras = {
        let limits = StageLimits {
            stage_one_budget: config.max_iterations,
            inner_threads: inner_threads(config.threads, extra_starts.len()),
        };
        let work = |i: usize| -> Result<Staged, FitError> {
            let copy = model.duplicate();
            run_staged(
                problem,
                &*copy,
                &extra_starts[i],
                ctx,
                config,
                cancel,
                limits,
            )
        };
        let count = extra_starts.len();
        collect_ok(par_map_progress(
            count,
            config.threads,
            cancel,
            &work,
            &mut |finished| progress(share(0.7, 1.0, finished, count)),
        ))?
    };

    let best = staged
        .into_iter()
        .chain(extras)
        .enumerate()
        .min_by(|(ia, a), (ib, b)| {
            a.out
                .normal
                .cost
                .total_cmp(&b.out.normal.cost)
                .then(ia.cmp(ib))
        })
        .map(|(_, s)| s)
        .ok_or(FitError::Numerical("no start produced a fit"))?;
    progress(1.0);
    Ok((best.out, seed_costs, best.stages))
}

/// Fits `model`: the staged multi-start of [`multi_start`] (or one warm start with every
/// parameter free), then the reweighted L1 rounds when the model has an L1 prior.
pub(super) fn fit_model(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    config: &FitConfig,
    cancel: &AtomicBool,
    warm: Option<&[f64]>,
) -> Result<Fitted, FitError> {
    fit_model_reporting(problem, model, config, cancel, warm, &mut |_| {})
}

/// [`fit_model`] with a progress hook: `progress` gets the finished share of the fit in
/// `0.0..=1.0`, non-decreasing (the multi-start reports as its starts finish).
pub(super) fn fit_model_reporting(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    config: &FitConfig,
    cancel: &AtomicBool,
    warm: Option<&[f64]>,
    progress: &mut dyn FnMut(f32),
) -> Result<Fitted, FitError> {
    let nv = problem.view_count();
    let np = model.n_params();
    let active = vec![true; nv];
    let ones = vec![1.0; np];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: true,
    };
    // One run at a time here (warm start, L1 rounds): the pixel pass may use the whole width.
    let full = LmSettings::new(config.max_iterations, config.rel_cost_tol)
        .with_inner_threads(inner_threads(config.threads, 1));

    let (mut best, seed_costs, mut stages) = match warm {
        Some(x) => {
            let outcome = run_lm(problem, model, x, &active, &ctx, &full, cancel)?;
            let cost = outcome.normal.cost;
            progress(1.0);
            let stages = vec![outcome.iterations];
            (outcome, vec![cost], stages)
        }
        None => multi_start(problem, model, config, cancel, &ctx, progress)?,
    };

    let mut l1 = vec![1.0; np];
    if model.kind() == ModelKind::Chromophore && config.l1_weight > 0.0 {
        for _ in 0..config.l1_rounds {
            let weights = model.l1_weights(&best.x[..np]);
            let round_ctx = PriorContext {
                config,
                l1_weights: &weights,
                pin_common_gain: false,
                model_priors: true,
            };
            best = run_lm(problem, model, &best.x, &active, &round_ctx, &full, cancel)?;
            stages.push(best.iterations);
            l1 = weights;
        }
    }
    Ok(Fitted {
        x: best.x,
        normal: best.normal,
        iterations: best.iterations,
        iterations_per_stage: stages,
        converged: best.converged,
        seed_costs,
        l1,
    })
}

/// The face-up colour difference (CIEDE2000) from which the refit of the exported spectrum is
/// reported as a warning.
const EXPORT_DELTA_E_NOTICE: f64 = 0.5;

/// `erf(x)` for `x >= 0` (Abramowitz and Stegun 7.1.26, absolute error below 1.5e-7).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    1.0 - poly * (-x * x).exp()
}

/// The expectation of the Huber-weighted square `min(u^2, k |u|)` of a standard normal `u`:
/// `erf(k / sqrt 2)`. The Birge ratio of a Huber fit divides by it, so that it is 1 for honest
/// noise.
fn huber_expectation(k: f64) -> f64 {
    erf(k / std::f64::consts::SQRT_2)
}

/// The largest absorption difference (per mm, 5 nm grid from 400 to 700 nm) and the largest
/// face-up CIEDE2000 (every zone, size and illuminant of the prediction settings) between the
/// fitted spectra of `model` at `params` and the exported `tensors`.
fn export_misfit(
    model: &dyn FitModel,
    params: &[f64],
    tensors: &[AbsorptionTensor],
    config: &FitConfig,
) -> (f64, f64) {
    let (mut worst_alpha, mut worst_de) = (0.0_f64, 0.0_f64);
    for (zone, tensor) in tensors.iter().enumerate() {
        let exported = ZoneAbsorption::per_mm(tensor.clone());
        for i in 0..=60 {
            let lambda = 400.0 + 5.0 * f64::from(i);
            let difference =
                (model.alpha(zone, lambda, params) - exported.alpha(lambda, None)).abs();
            worst_alpha = worst_alpha.max(difference);
        }
        for size_mm in config.prediction.sizes() {
            let path_mm = face_up_path_mm(size_mm, config.prediction.model_width_units);
            for &illuminant in &config.prediction.illuminants {
                let fitted = zone_lab(model.as_spectral(), zone, params, path_mm, illuminant);
                let refit = body_color(
                    |lambda| exported.alpha(lambda, None).max(0.0),
                    path_mm,
                    illuminant,
                )
                .lab;
                let de = delta_e_2000(fitted, refit);
                if de.is_finite() {
                    worst_de = worst_de.max(de);
                }
            }
        }
    }
    (worst_alpha, worst_de)
}

/// The report of one fitted model (the information criterion is filled in by the comparison),
/// and the warnings that belong to it.
///
/// * The Birge ratio `R = chi2_robust / (erf(k / sqrt 2) (n_data - effective_params))` (round 5,
///   ZC-18: the Huber-weighted chi-square, so that a few outliers the loss already down-weights
///   do not widen the covariance) inflates the covariance by `R` when it exceeds
///   [`BIRGE_THRESHOLD`] (the noise model was too small; protects real photos too).
/// * The metamer spread comes from the constrained extremisation of `extremes` with
///   `Delta = max(1, R)`.
/// * The exported tensors may differ from the fitted spectrum (the smooth basis is refitted onto
///   seven bands); a face-up colour change of [`EXPORT_DELTA_E_NOTICE`] or more is a
///   [`WarningKind::ExportRefit`] warning.
///
/// `progress` gets the finished share of the constrained search in `0.0..=1.0` (the search is
/// the long part of this function).
///
/// # Errors
///
/// [`FitError::Cancelled`] and the forward evaluator's errors (the constrained search evaluates
/// the data term).
pub(super) fn build_model_fit(
    problem: &Problem<'_>,
    model: &dyn FitModel,
    fitted: &Fitted,
    config: &FitConfig,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<(ModelFit, Vec<FitWarning>), FitError> {
    let np = model.n_params();
    let nv = problem.view_count();
    let n = np + nv;
    let (mut covariance, _ridge) = inverse_spd_ridged(&fitted.normal.h, n);
    let data_h: Vec<f64> = fitted
        .normal
        .h
        .iter()
        .zip(&fitted.normal.h_prior)
        .map(|(h, p)| h - p)
        .collect();
    let effective_params = trace_product(&covariance, &data_h, n);
    let dof = (fitted.normal.n_data as f64 - effective_params).max(1.0);
    let birge_ratio = fitted.normal.chi2_robust / (huber_expectation(config.huber_k) * dof);
    let inflate = birge_ratio.is_finite() && birge_ratio > BIRGE_THRESHOLD;
    if inflate {
        for value in &mut covariance {
            *value *= birge_ratio;
        }
    }
    let threshold = if inflate { birge_ratio } else { 1.0 };
    // The metric of the unverified (ellipsoid) figure: the data alone plus the gain prior
    // (calibration knowledge), without the model's smoothness and magnitude priors.
    let gain_h = problem.gain_prior_hessian(np);
    let metric: Vec<f64> = data_h.iter().zip(&gain_h).map(|(d, g)| d + g).collect();
    let names = model.param_names();
    let (lo, hi) = bounds(model, nv);
    let at_bound: Vec<String> = (0..np)
        // Only the upper bound is a finding: the lower one is "absent" for a chromophore and
        // "no absorption in this band" for the basis.
        .filter(|&i| fitted.x[i] >= hi[i] - 1e-9)
        .map(|i| names[i].clone())
        .collect();
    let steps = metamer_steps(&metric, n, np, threshold);
    let unverified: Vec<Vec<f64>> = steps
        .iter()
        .map(|step| candidate_point(&fitted.x, step, 1.0, &lo, &hi))
        .collect();
    let verified = constrained_extremes(
        &ExtremeInput {
            problem,
            model,
            fit: &fitted.x,
            delta: threshold,
            // The semi-axes of the data ellipsoid point along the weakest data directions,
            // which is where the metamers are (round 6, R3).
            extra_starts: &unverified,
            config,
            cancel,
        },
        progress,
    )?;
    let zone_tensors = model.tensors(&fitted.x[..np]);
    let mut warnings = Vec::new();
    let (alpha_error, delta_e) = export_misfit(model, &fitted.x[..np], &zone_tensors, config);
    if delta_e >= EXPORT_DELTA_E_NOTICE {
        warnings.push(FitWarning {
            kind: WarningKind::ExportRefit,
            message: format!(
                "the {} model's spectrum was refitted onto the {}-band renderer basis: the \
                 refit changes a face-up color by up to {delta_e:.2} CIEDE2000 (absorption by up \
                 to {alpha_error:.4} per mm)",
                model.kind().name(),
                BODY_COLOR_BASIS_NM.len(),
            ),
        });
    }
    let fit = ModelFit {
        kind: model.kind(),
        host_id: model.host().map(str::to_owned),
        param_names: names,
        params: fitted.x[..np].to_vec(),
        view_ids: problem.views.iter().map(|v| v.view).collect(),
        log_gains: fitted.x[np..].to_vec(),
        zone_tensors,
        cost: fitted.normal.cost,
        data_cost: fitted.normal.data_cost,
        prior_terms: fitted.normal.prior.named(),
        birge_ratio,
        chi2: fitted.normal.chi2,
        n_data: fitted.normal.n_data,
        effective_params,
        aic: 0.0,
        iterations: fitted.iterations,
        iterations_per_stage: fitted.iterations_per_stage.clone(),
        converged: fitted.converged,
        seed_costs: fitted.seed_costs.clone(),
        at_bound,
        predictions: predictions(
            model,
            &fitted.x[..np],
            &covariance,
            n,
            config,
            &verified,
            &unverified,
        ),
        covariance,
    };
    Ok((fit, warnings))
}

/// Fits every available model to the records and the observations, compares them, and validates
/// the chosen one (cross-validation, residual maps, zoning suggestion).
///
/// This does not trace: use [`fit_colour`](super::fit_colour) for the full pipeline with the
/// roughness search and the alignment refinement.
///
/// # Errors
///
/// [`FitError`] for unusable input, [`FitError::Cancelled`] when `cancel` is raised.
pub fn fit_records(
    records: &ForwardRecords,
    observed: &[ObservedView],
    config: &FitConfig,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<ColourFit, FitError> {
    config.validate()?;
    let problem = Problem::new(records, observed, config)?;
    if problem.used_pixels() == 0 {
        return Err(FitError::NoData);
    }
    let zones = records.n_zones;
    let mut models: Vec<Box<dyn FitModel>> = Vec::new();
    if let Some(host) = &config.host_id {
        models.push(Box::new(ChromophoreModel::new(
            host,
            &config.treatments,
            zones,
        )?));
    }
    models.push(Box::new(SmoothBasisModel::new(zones)));

    let mut fits: Vec<ModelFit> = Vec::new();
    let mut fit_warnings: Vec<FitWarning> = Vec::new();
    let mut fitted_all: Vec<Fitted> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        // Within the model's slot: the multi-start is the first half, the constrained spread
        // search the second.
        let count = models.len() as f32;
        let mut report = |within: f32| {
            progress(FitProgress {
                stage: FitStage::Fitting,
                fraction: (i as f32 + within.clamp(0.0, 1.0)) / count,
            });
        };
        report(0.0);
        let fitted = fit_model_reporting(&problem, &**model, config, cancel, None, &mut |p| {
            report(0.5 * p);
        })?;
        let (fit, warnings) =
            build_model_fit(&problem, &**model, &fitted, config, cancel, &mut |q| {
                report(0.5 + 0.5 * q);
            })?;
        fits.push(fit);
        fit_warnings.extend(warnings);
        fitted_all.push(fitted);
    }

    progress(FitProgress {
        stage: FitStage::Selection,
        fraction: 0.0,
    });
    let candidates: Vec<Candidate> = fits
        .iter()
        .map(|f| Candidate {
            kind: f.kind,
            chi2: f.chi2,
            effective_params: f.effective_params,
        })
        .collect();
    let n_data = fits.first().map_or(0, |f| f.n_data);
    let (comparison, chosen_index) = compare(&candidates, n_data, config.preferred_model);
    for (fit, score) in fits.iter_mut().zip(&comparison.scores) {
        fit.aic = score.aic;
    }

    let mut warnings = fit_warnings;
    if let Some(kind) = config.preferred_model
        && !fits.iter().any(|f| f.kind == kind)
    {
        warnings.push(FitWarning {
            kind: WarningKind::PreferenceIgnored,
            message: format!(
                "the {} model was asked for but no host was set; the smooth basis was used",
                kind.name()
            ),
        });
    }

    let chosen_model = &*models[chosen_index];
    let chosen_fitted = &fitted_all[chosen_index];

    progress(FitProgress {
        stage: FitStage::CrossValidation,
        fraction: 0.0,
    });
    let lovo_report = if config.lovo {
        let report = lovo(
            &problem,
            chosen_model,
            &chosen_fitted.x,
            &chosen_fitted.l1,
            config,
            cancel,
        )?;
        if report.is_none() {
            warnings.push(FitWarning {
                kind: WarningKind::LovoSkipped,
                message: "fewer than two views have usable pixels; no cross-validation".to_owned(),
            });
        }
        report
    } else {
        None
    };

    let scored = structure(&problem, chosen_model, &chosen_fitted.x)?;
    let suggest_zoning = scored.score > SUGGEST_MORAN && scored.rms > SUGGEST_RMS;

    collect_warnings(records, &problem, &fits, &mut warnings);
    progress(FitProgress {
        stage: FitStage::Done,
        fraction: 1.0,
    });
    Ok(ColourFit {
        version: FIT_VERSION,
        n_zones: zones,
        fits,
        chosen_index,
        comparison,
        roughness: None,
        alignment: None,
        lovo: lovo_report,
        residuals: scored.maps,
        structured_score: scored.score,
        residual_rms: scored.rms,
        suggest_zoning,
        warnings,
    })
}

fn collect_warnings(
    records: &ForwardRecords,
    problem: &Problem<'_>,
    fits: &[ModelFit],
    warnings: &mut Vec<FitWarning>,
) {
    let total: usize = records
        .views
        .iter()
        .map(crate::rough_plan::colour_fit::forward::ViewRecords::pixel_count)
        .sum();
    let valid = records.valid_pixels();
    let skipped_by_tracer: usize = records
        .views
        .iter()
        .map(|v| {
            v.status
                .iter()
                .filter(|s| **s & (status::FLAGGED_LIGHT | status::INCLUSION) != 0)
                .count()
        })
        .sum();
    if total > 0 && skipped_by_tracer as f64 > 0.2 * (valid + skipped_by_tracer) as f64 {
        warnings.push(FitWarning {
            kind: WarningKind::PixelsDropped,
            message: format!(
                "{skipped_by_tracer} pixels were dropped (unseen backlight or inclusions), \
                 more than a fifth of the traced pixels"
            ),
        });
    }
    let depth = records.lost_depth_fraction();
    if depth > 0.05 {
        warnings.push(FitWarning {
            kind: WarningKind::DepthLoss,
            message: format!(
                "paths cut off at the depth limit carry {:.1} % of the weight",
                100.0 * depth
            ),
        });
    }
    if records.stats.max_cluster_range_mm > 0.5 {
        warnings.push(FitWarning {
            kind: WarningKind::CompressionCoarse,
            message: format!(
                "the record compression merged paths up to {:.2} mm apart; strongly absorbing \
                 stones lose accuracy (raise max_records)",
                records.stats.max_cluster_range_mm
            ),
        });
    }
    for (slot, view) in problem.views.iter().enumerate() {
        if view.used_pixels == 0 {
            warnings.push(FitWarning {
                kind: WarningKind::EmptyView,
                message: format!("view {} has no usable pixel", view.view + 1),
            });
        }
        for fit in fits {
            if let Some(g) = fit.log_gains.get(slot)
                && g.abs() > 0.3
            {
                warnings.push(FitWarning {
                    kind: WarningKind::LargeGain,
                    message: format!(
                        "the {} model needs a gain of {:.2} for view {}",
                        fit.kind.name(),
                        g.exp(),
                        view.view + 1
                    ),
                });
            }
        }
    }
    for fit in fits {
        if !fit.converged {
            // The last stage is the one that stopped on its cap; the sum over the stages is
            // not a cap.
            let last = fit
                .iterations_per_stage
                .last()
                .copied()
                .unwrap_or(fit.iterations);
            warnings.push(FitWarning {
                kind: WarningKind::NotConverged,
                message: format!(
                    "the {} model's last stage stopped at the iteration limit after {last} \
                     iterations without converging ({} in all stages); the colors may not be \
                     the best fit",
                    fit.kind.name(),
                    fit.iterations
                ),
            });
        }
        if fit.birge_ratio > BIRGE_THRESHOLD {
            warnings.push(FitWarning {
                kind: WarningKind::NoiseUnderestimated,
                message: format!(
                    "the photos scatter {:.1} times more (in variance) than the noise model \
                     expects; the uncertainties of the {} model were widened by that factor",
                    fit.birge_ratio,
                    fit.kind.name()
                ),
            });
        }
        if !fit.at_bound.is_empty() {
            warnings.push(FitWarning {
                kind: WarningKind::ParameterAtBound,
                message: format!(
                    "{} parameters of the {} model sit on a bound (for example {})",
                    fit.at_bound.len(),
                    fit.kind.name(),
                    fit.at_bound[0]
                ),
            });
        }
    }
}

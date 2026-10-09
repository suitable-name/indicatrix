//! The outer loops (plan section 6.4) and the full pipeline [`fit_colour`]: trace, optional skin
//! roughness search, optional alignment refinement, then [`fit_records`].
//!
//! # Skin roughness
//!
//! Golden-section search over `[min, max]` (default 0.05 to 0.6) with `evaluations` (5) traces.
//! Each evaluation traces the rig with the surface map `surfaces_for(roughness)` (the tracer's
//! own disk cache, when a cache directory is given, makes a repeated value free) and fits the
//! search model (the chromophore model when a host is set, else the smooth basis) from a warm
//! start of the previous evaluation, except the first, which runs the full multi-start. The
//! objective compared is the total fit objective. The best of the evaluated values wins (not
//! necessarily the last bracket); only the best records and one working set are held.
//!
//! # Alignment
//!
//! Optional (off by default). Six rigid parameters (a rotation vector and a translation, added to
//! the given alignment), two Levenberg-Marquardt steps: each step traces the rig at the base
//! alignment and six forward-difference shifts (`rotation_step_rad`, `translation_step_mm`; the
//! tracer's seed is the same in every trace, so the Monte-Carlo noise is common and cancels in
//! the differences), builds the Jacobian of the whitened residual on the pixels valid in all
//! seven traces at the fitted model, solves `(J^T J + mu diag) d = -J^T r` for two values of
//! `mu`, re-traces at the candidate and accepts the first that lowers the summed squared
//! residual on the pixels valid in both traces. The model parameters of the Jacobian are held
//! fixed within a step; the final [`fit_records`] refits everything at the refined alignment.

use std::sync::atomic::AtomicBool;

use super::{
    AlignmentRefine, FitConfig, FitError, FitInputs, FitProgress, FitStage, RoughnessSearch,
    fitter::{fit_model, fit_records},
    linalg::solve_spd,
    models::{ChromophoreModel, FitModel, SmoothBasisModel},
    output::{AlignmentReport, ColourFit, RoughnessReport},
    problem::Problem,
};
use crate::rough_plan::{
    colour_fit::forward::{ForwardInput, ForwardRecords, SurfaceMap, trace_rig},
    locate::Rigid,
};

/// The model the outer loops fit: the chromophore model when a host is set.
fn search_model(config: &FitConfig, zones: usize) -> Result<Box<dyn FitModel>, FitError> {
    match &config.host_id {
        Some(host) => Ok(Box::new(ChromophoreModel::new(
            host,
            &config.treatments,
            zones,
        )?)),
        None => Ok(Box::new(SmoothBasisModel::new(zones))),
    }
}

fn trace(
    forward: &ForwardInput<'_>,
    surfaces: &SurfaceMap,
    alignment: Rigid,
    cancel: &AtomicBool,
) -> Result<ForwardRecords, FitError> {
    let mut input = *forward;
    input.surfaces = surfaces;
    input.alignment = alignment;
    trace_rig(&input, cancel, &mut |_| {}).map_err(FitError::from)
}

/// One evaluated roughness.
struct Point {
    cost: f64,
    x: Vec<f64>,
}

/// The best evaluation so far.
struct Best {
    roughness: f32,
    cost: f64,
    records: ForwardRecords,
    surfaces: SurfaceMap,
    x: Vec<f64>,
}

fn golden_roughness(
    inputs: &FitInputs<'_>,
    search: &RoughnessSearch,
    surfaces_for: &(dyn Fn(f32) -> SurfaceMap + Sync),
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<(ForwardRecords, SurfaceMap, Vec<f64>, RoughnessReport), FitError> {
    let config = inputs.config;
    let alignment = inputs.forward.alignment;
    let total = search.evaluations.max(2);
    let mut evaluations: Vec<(f32, f64)> = Vec::new();
    let mut best: Option<Best> = None;
    let mut evaluate = |roughness: f64, warm: Option<Vec<f64>>| -> Result<Point, FitError> {
        progress(FitProgress {
            stage: FitStage::Roughness,
            fraction: evaluations.len() as f32 / total as f32,
        });
        let alpha = roughness as f32;
        let surfaces = surfaces_for(alpha);
        let records = trace(&inputs.forward, &surfaces, alignment, cancel)?;
        let model = search_model(config, records.n_zones)?;
        let (cost, x) = {
            let problem = Problem::new(&records, inputs.observed, config)?;
            let fitted = fit_model(&problem, &*model, config, cancel, warm.as_deref())?;
            (fitted.normal.cost, fitted.x)
        };
        evaluations.push((alpha, cost));
        if best.as_ref().is_none_or(|b| cost < b.cost) {
            best = Some(Best {
                roughness: alpha,
                cost,
                records,
                surfaces,
                x: x.clone(),
            });
        }
        Ok(Point { cost, x })
    };

    let phi = 0.618_033_988_749_894_9_f64;
    let (mut a, mut b) = (f64::from(search.min), f64::from(search.max));
    let mut c = b - phi * (b - a);
    let mut d = a + phi * (b - a);
    let mut fc = evaluate(c, None)?;
    let mut fd = evaluate(d, Some(fc.x.clone()))?;
    for _ in 2..total {
        if fc.cost < fd.cost {
            b = d;
            d = c;
            fd = fc;
            c = b - phi * (b - a);
            fc = evaluate(c, Some(fd.x.clone()))?;
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + phi * (b - a);
            fd = evaluate(d, Some(fc.x.clone()))?;
        }
    }
    let Some(best) = best else {
        return Err(FitError::Numerical(
            "the roughness search evaluated nothing",
        ));
    };
    Ok((
        best.records,
        best.surfaces,
        best.x,
        RoughnessReport {
            roughness: best.roughness,
            evaluations,
        },
    ))
}

/// The alignment shifted by `amount` in rigid parameter `k` (0 to 2 rotation vector, 3 to 5
/// translation).
fn displaced(alignment: Rigid, k: usize, amount: f64) -> Rigid {
    let mut out = alignment;
    if k < 3 {
        out.rotation[k] += amount;
    } else {
        out.translation[k - 3] += amount;
    }
    out
}

struct AlignmentOutcome {
    records: ForwardRecords,
    report: AlignmentReport,
}

fn refine_alignment(
    inputs: &FitInputs<'_>,
    refine: &AlignmentRefine,
    surfaces: &SurfaceMap,
    model: &dyn FitModel,
    mut records: ForwardRecords,
    x: &[f64],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<AlignmentOutcome, FitError> {
    let config = inputs.config;
    let initial = inputs.forward.alignment;
    let mut alignment = initial;
    let steps = [
        refine.rotation_step_rad,
        refine.rotation_step_rad,
        refine.rotation_step_rad,
        refine.translation_step_mm,
        refine.translation_step_mm,
        refine.translation_step_mm,
    ];
    let (mut chi_before, mut chi_after, mut accepted_steps) = (None, 0.0, 0);
    for step in 0..refine.steps {
        progress(FitProgress {
            stage: FitStage::Alignment,
            fraction: step as f32 / refine.steps.max(1) as f32,
        });
        let base = {
            let problem = Problem::new(&records, inputs.observed, config)?;
            problem.whitened_all(model, x)?
        };
        let mut shifted = Vec::with_capacity(6);
        for k in 0..6 {
            let rigid = displaced(alignment, k, steps[k]);
            let moved = trace(&inputs.forward, surfaces, rigid, cancel)?;
            let problem = Problem::new(&moved, inputs.observed, config)?;
            shifted.push(problem.whitened_all(model, x)?);
        }
        let mut common: Vec<(usize, usize)> = Vec::new();
        for (slot, view) in base.iter().enumerate() {
            for (p, u) in view.iter().enumerate() {
                if u.is_some() && shifted.iter().all(|s| s[slot][p].is_some()) {
                    common.push((slot, p));
                }
            }
        }
        if common.is_empty() {
            break;
        }
        let value =
            |map: &[Vec<Option<[f64; 3]>>], slot: usize, p: usize| map[slot][p].unwrap_or([0.0; 3]);
        let mut jtj = [0.0_f64; 36];
        let mut jtr = [0.0_f64; 6];
        for &(slot, p) in &common {
            let r0 = value(&base, slot, p);
            for c in 0..3 {
                let mut col = [0.0_f64; 6];
                for k in 0..6 {
                    col[k] = (value(&shifted[k], slot, p)[c] - r0[c]) / steps[k];
                }
                for i in 0..6 {
                    jtr[i] += col[i] * r0[c];
                    for j in 0..6 {
                        jtj[i * 6 + j] += col[i] * col[j];
                    }
                }
            }
        }
        let mut accepted = false;
        for mu in [1e-2, 1.0] {
            let mut a = jtj;
            for i in 0..6 {
                a[i * 6 + i] += mu * jtj[i * 6 + i].max(1e-12) + 1e-12;
            }
            let rhs: Vec<f64> = jtr.iter().map(|v| -v).collect();
            let Some(delta) = solve_spd(&a, 6, &rhs) else {
                continue;
            };
            let mut candidate = alignment;
            for k in 0..6 {
                candidate = displaced(candidate, k, delta[k]);
            }
            let moved = trace(&inputs.forward, surfaces, candidate, cancel)?;
            let after = {
                let problem = Problem::new(&moved, inputs.observed, config)?;
                problem.whitened_all(model, x)?
            };
            let (mut old, mut new) = (0.0, 0.0);
            for &(slot, p) in &common {
                if after[slot][p].is_none() {
                    continue;
                }
                for c in 0..3 {
                    let u0 = value(&base, slot, p)[c];
                    let u1 = value(&after, slot, p)[c];
                    old += u0 * u0;
                    new += u1 * u1;
                }
            }
            if chi_before.is_none() {
                chi_before = Some(old);
            }
            if new < old {
                alignment = candidate;
                records = moved;
                chi_after = new;
                accepted_steps += 1;
                accepted = true;
                break;
            }
        }
        if !accepted {
            break;
        }
    }
    let before = chi_before.unwrap_or(0.0);
    Ok(AlignmentOutcome {
        records,
        report: AlignmentReport {
            initial,
            refined: alignment,
            chi2_before: before,
            chi2_after: if accepted_steps > 0 {
                chi_after
            } else {
                before
            },
            steps: accepted_steps,
        },
    })
}

/// The full pipeline: traces the rig (searching the skin roughness when configured), refines the
/// alignment when configured, and fits.
///
/// # Errors
///
/// [`FitError`] for unusable input, [`FitError::Cancelled`] when `cancel` is raised, and the
/// tracer's errors.
pub fn fit_colour(
    inputs: &FitInputs<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(FitProgress),
) -> Result<ColourFit, FitError> {
    let config = inputs.config;
    config.validate()?;
    let mut surfaces = inputs.forward.surfaces.clone();
    let alignment = inputs.forward.alignment;

    let (mut records, mut warm, roughness) =
        match (&config.roughness, inputs.surfaces_for_roughness) {
            (Some(search), Some(surfaces_for)) => {
                let (records, chosen, x, report) =
                    golden_roughness(inputs, search, surfaces_for, cancel, progress)?;
                surfaces = chosen;
                (records, Some(x), Some(report))
            }
            (Some(_), None) => {
                return Err(FitError::BadConfig(
                    "a roughness search needs surfaces_for_roughness",
                ));
            }
            (None, _) => {
                progress(FitProgress {
                    stage: FitStage::Tracing,
                    fraction: 0.0,
                });
                (
                    trace(&inputs.forward, &surfaces, alignment, cancel)?,
                    None,
                    None,
                )
            }
        };

    let alignment_report = if let Some(refine) = &config.alignment {
        let model = search_model(config, records.n_zones)?;
        let x0 = if let Some(x) = warm.take() {
            x
        } else {
            let problem = Problem::new(&records, inputs.observed, config)?;
            fit_model(&problem, &*model, config, cancel, None)?.x
        };
        let outcome = refine_alignment(
            inputs, refine, &surfaces, &*model, records, &x0, cancel, progress,
        )?;
        records = outcome.records;
        Some(outcome.report)
    } else {
        None
    };

    let mut fit = fit_records(&records, inputs.observed, config, cancel, progress)?;
    fit.roughness = roughness;
    fit.alignment = alignment_report;
    Ok(fit)
}

//! Joint refinement of the zone boundaries (plan section 7.3, step 4): with the colours fitted by
//! the solver, the boundary offsets and radii are moved to lower the photo residual.
//!
//! # Method
//!
//! The unknowns are the unlocked boundary parameters of the zones: the offset of a half space, the
//! two offsets of a slab, the outer (and a non-zero inner) radius of a cylinder or prism. Angles,
//! axes, phases and mesh shells are not refined.
//!
//! For a candidate geometry the forward tracer is run again (F1's disk cache makes a repeat of
//! the same geometry free) and the colours of all zones are refitted ([`fit_records`]); the
//! residual vector is the whitened residual of the fit, pixel by pixel and channel by channel
//! ("variable projection": the colours are always at their optimum for the geometry).
//!
//! Each iteration then does one Gauss-Newton step on that vector: the Jacobian by forward
//! differences with [`RefineOptions::step_mm`] (a backward difference when the forward step would
//! make the zoning invalid), normal equations with a Levenberg damping of 1e-3 of the diagonal,
//! each component of the step limited to [`RefineOptions::max_move_mm`]. The step is accepted only
//! when the sum of squared residuals over the pixels valid in both evaluations falls by at least
//! `min_gain` (relative); otherwise the damping is raised tenfold, twice, and the iteration then
//! stops. [`RefineOptions::max_iterations`] iterations run at most (default
//! [`DEFAULT_REFINE_ITERATIONS`] = 3, configurable up to [`MAX_REFINE_ITERATIONS`]); the loop also
//! stops early when a step brings less than `min_gain`. A full iteration costs
//! `parameters + 1` traces and fits, so the whole refinement costs up to
//! `max_iterations * (parameters + 1)` plus the base evaluation.
//!
//! The generic [`refine_with`] takes the evaluation as a closure (the unit tests drive it with an
//! analytic residual); [`refine_zone_geometry`] is the production wrapper around F1 and S1.

use std::sync::atomic::{AtomicBool, Ordering};

use indicatrix::optics::zoning::{ZoneShape, ZonedAbsorption, ZoningError};

use super::edit::{ZoneLocks, ZoneParameter, parameter_value, set_parameter_value};
use crate::rough_plan::colour_fit::{
    forward::{ForwardInput, trace_rig},
    solve::{ColourFit, FitConfig, FitError, FitInputs, fit_records},
};

/// The default number of refinement iterations.
pub const DEFAULT_REFINE_ITERATIONS: usize = 3;

/// The largest accepted [`RefineOptions::max_iterations`] (a sanity bound: every iteration costs
/// `parameters + 1` full traces and fits).
pub const MAX_REFINE_ITERATIONS: usize = 25;

/// Settings of the refinement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineOptions {
    /// The iterations to run at most, from 1 up to [`MAX_REFINE_ITERATIONS`] (default
    /// [`DEFAULT_REFINE_ITERATIONS`]). The loop stops earlier once a step gains less than
    /// `min_gain`.
    pub max_iterations: usize,
    /// The finite-difference step of an offset or radius, mm (default 0.15).
    pub step_mm: f64,
    /// The Levenberg damping relative to the diagonal (default 1e-3).
    pub damping: f64,
    /// The largest change of one parameter in one step, mm (default 1).
    pub max_move_mm: f64,
    /// The relative fall of the sum of squares a step must bring (default 1e-4).
    pub min_gain: f64,
}

impl Default for RefineOptions {
    fn default() -> Self {
        Self {
            max_iterations: DEFAULT_REFINE_ITERATIONS,
            step_mm: 0.15,
            damping: 1e-3,
            max_move_mm: 1.0,
            min_gain: 1e-4,
        }
    }
}

impl RefineOptions {
    fn validate(&self) -> Result<(), RefineError> {
        let ok = |v: f64| v.is_finite() && v > 0.0;
        if self.max_iterations == 0
            || self.max_iterations > MAX_REFINE_ITERATIONS
            || !(ok(self.step_mm) && ok(self.damping) && ok(self.max_move_mm))
            || !(self.min_gain.is_finite() && self.min_gain >= 0.0)
        {
            return Err(RefineError::BadOptions);
        }
        Ok(())
    }
}

/// Why a refinement could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefineError {
    /// The trace or the fit failed.
    Fit(FitError),
    /// Every boundary parameter is locked, or the zoning has none to refine.
    NoFreeParameters,
    /// The starting zoning does not validate.
    BadZoning(ZoningError),
    /// A setting is out of range.
    BadOptions,
    /// The cancel flag was raised.
    Cancelled,
}

impl std::fmt::Display for RefineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fit(e) => write!(f, "{e}"),
            Self::NoFreeParameters => write!(f, "there is no unlocked boundary to refine"),
            Self::BadZoning(e) => write!(f, "{e}"),
            Self::BadOptions => write!(f, "the refinement settings are out of range"),
            Self::Cancelled => write!(f, "the refinement was cancelled"),
        }
    }
}

impl std::error::Error for RefineError {}

impl From<FitError> for RefineError {
    fn from(e: FitError) -> Self {
        match e {
            FitError::Cancelled => Self::Cancelled,
            other => Self::Fit(other),
        }
    }
}

/// What one geometry gives: the whitened residual vector and which entries are valid.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// The residuals, three channels per pixel, view after view; zero where invalid.
    pub residuals: Vec<f64>,
    /// Whether each entry entered the fit.
    pub valid: Vec<bool>,
    /// The fit behind it (absent for a synthetic evaluation).
    pub fit: Option<ColourFit>,
}

impl Evaluation {
    /// The residual vector of a fit.
    #[must_use]
    pub fn from_fit(fit: ColourFit) -> Self {
        let mut residuals = Vec::new();
        let mut valid = Vec::new();
        for map in &fit.residuals {
            for p in 0..map.width * map.height {
                let ok = map.valid[p];
                for c in 0..3 {
                    residuals.push(if ok {
                        f64::from(map.whitened[p][c])
                    } else {
                        0.0
                    });
                    valid.push(ok);
                }
            }
        }
        Self {
            residuals,
            valid,
            fit: Some(fit),
        }
    }
}

/// One parameter that moved.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterMove {
    /// The zone index (1..).
    pub zone: usize,
    /// The parameter.
    pub parameter: ZoneParameter,
    /// The value before, mm.
    pub before: f64,
    /// The value after, mm.
    pub after: f64,
}

/// The outcome of a refinement.
#[derive(Debug, Clone, PartialEq)]
pub struct RefineResult {
    /// The refined zoning (the input when no step helped).
    pub zoned: ZonedAbsorption,
    /// The fit at the refined geometry (absent for a synthetic evaluation).
    pub fit: Option<ColourFit>,
    /// The accepted steps.
    pub iterations: usize,
    /// The geometries evaluated (traces and fits).
    pub evaluations: usize,
    /// How every free parameter moved.
    pub moves: Vec<ParameterMove>,
    /// The summed squared whitened residual of the starting geometry.
    pub chi2_before: f64,
    /// And of the refined one.
    pub chi2_after: f64,
}

/// A progress report: the geometries evaluated so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefineProgress {
    /// The iteration under way (0-based).
    pub iteration: usize,
    /// The evaluations finished.
    pub evaluations: usize,
}

fn boundary_parameters(shape: &ZoneShape) -> Vec<ZoneParameter> {
    match shape {
        ZoneShape::HalfSpace { .. } => vec![ZoneParameter::Offset],
        ZoneShape::Slab { .. } => vec![ZoneParameter::OffsetMin, ZoneParameter::OffsetMax],
        ZoneShape::CoaxialCylinder { r_in, .. } | ZoneShape::CoaxialPrism { r_in, .. } => {
            if *r_in > 0.0 {
                vec![ZoneParameter::RIn, ZoneParameter::ROut]
            } else {
                vec![ZoneParameter::ROut]
            }
        }
        ZoneShape::Sector { .. } | ZoneShape::MeshShell { .. } => Vec::new(),
    }
}

/// The unlocked boundary parameters, zone by zone.
#[must_use]
pub fn free_parameters(zoned: &ZonedAbsorption, locks: &ZoneLocks) -> Vec<(usize, ZoneParameter)> {
    let mut out = Vec::new();
    for (i, zone) in zoned.zones.iter().enumerate() {
        for parameter in boundary_parameters(&zone.shape) {
            if !locks.is_locked(i + 1, parameter) {
                out.push((i + 1, parameter));
            }
        }
    }
    out
}

fn read(zoned: &ZonedAbsorption, zone: usize, parameter: ZoneParameter) -> Option<f64> {
    zoned
        .zones
        .get(zone - 1)
        .and_then(|z| parameter_value(&z.shape, parameter))
}

/// `zoned` with each free parameter moved by `delta` (same order as `free`); `None` when the
/// result does not validate.
fn moved(
    zoned: &ZonedAbsorption,
    free: &[(usize, ZoneParameter)],
    delta: &[f64],
) -> Option<ZonedAbsorption> {
    let mut out = zoned.clone();
    for (&(zone, parameter), d) in free.iter().zip(delta) {
        let value = read(zoned, zone, parameter)? + d;
        if !set_parameter_value(&mut out.zones.get_mut(zone - 1)?.shape, parameter, value) {
            return None;
        }
    }
    out.validate().ok()?;
    Some(out)
}

fn chi2(e: &Evaluation) -> f64 {
    e.residuals
        .iter()
        .zip(&e.valid)
        .filter(|(_, v)| **v)
        .map(|(r, _)| r * r)
        .sum()
}

/// The sums of squares of two evaluations over the entries valid in both.
fn paired_chi2(a: &Evaluation, b: &Evaluation) -> (f64, f64) {
    let mut sa = 0.0;
    let mut sb = 0.0;
    for i in 0..a.residuals.len() {
        if a.valid[i] && b.valid[i] {
            sa += a.residuals[i] * a.residuals[i];
            sb += b.residuals[i] * b.residuals[i];
        }
    }
    (sa, sb)
}

const fn check_layout(base: &Evaluation, other: &Evaluation) -> Result<(), RefineError> {
    if base.residuals.len() == other.residuals.len() && base.valid.len() == other.valid.len() {
        Ok(())
    } else {
        Err(RefineError::Fit(FitError::Numerical(
            "the residual layout changed between geometries",
        )))
    }
}

/// The refinement loop around an evaluation of a geometry (module docs).
///
/// `evaluate` returns the residual vector for a candidate zoning; it is called once for the
/// start, `parameters` times per iteration for the Jacobian and once or twice per iteration for
/// the step. `on_progress` is called after every evaluation.
///
/// # Errors
///
/// [`RefineError`]: bad settings or zoning, no free parameter, cancellation, or a failure of
/// `evaluate`.
pub fn refine_with(
    zoned: &ZonedAbsorption,
    locks: &ZoneLocks,
    options: &RefineOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(RefineProgress),
    evaluate: &mut dyn FnMut(&ZonedAbsorption) -> Result<Evaluation, RefineError>,
) -> Result<RefineResult, RefineError> {
    options.validate()?;
    zoned.validate().map_err(RefineError::BadZoning)?;
    let free = free_parameters(zoned, locks);
    if free.is_empty() {
        return Err(RefineError::NoFreeParameters);
    }
    let p = free.len();
    let start_values: Vec<f64> = free
        .iter()
        .filter_map(|&(zone, parameter)| read(zoned, zone, parameter))
        .collect();
    let mut evaluations = 0_usize;
    let mut current = zoned.clone();
    let mut base = evaluate(&current)?;
    evaluations += 1;
    on_progress(RefineProgress {
        iteration: 0,
        evaluations,
    });
    let chi2_before = chi2(&base);
    let mut accepted_steps = 0_usize;

    for iteration in 0..options.max_iterations.min(MAX_REFINE_ITERATIONS) {
        if cancel.load(Ordering::Relaxed) {
            return Err(RefineError::Cancelled);
        }
        let n = base.residuals.len();
        let mut jac = vec![0.0; n * p];
        for j in 0..p {
            let mut forward_step = vec![0.0; p];
            forward_step[j] = options.step_mm;
            let (trial, sign) = if let Some(t) = moved(&current, &free, &forward_step) {
                (t, 1.0)
            } else {
                let mut backward = vec![0.0; p];
                backward[j] = -options.step_mm;
                match moved(&current, &free, &backward) {
                    Some(t) => (t, -1.0),
                    None => continue,
                }
            };
            let ev = evaluate(&trial)?;
            evaluations += 1;
            on_progress(RefineProgress {
                iteration,
                evaluations,
            });
            check_layout(&base, &ev)?;
            for i in 0..n {
                if base.valid[i] && ev.valid[i] {
                    jac[i * p + j] =
                        (ev.residuals[i] - base.residuals[i]) / (sign * options.step_mm);
                }
            }
        }
        let mut jtj = vec![0.0; p * p];
        let mut jtr = vec![0.0; p];
        for i in 0..n {
            if !base.valid[i] {
                continue;
            }
            let row = &jac[i * p..(i + 1) * p];
            for a in 0..p {
                jtr[a] += row[a] * base.residuals[i];
                for b in 0..p {
                    jtj[a * p + b] += row[a] * row[b];
                }
            }
        }
        let mut accepted = false;
        let mut lambda = options.damping;
        for _attempt in 0..3 {
            let mut m = jtj.clone();
            for a in 0..p {
                m[a * p + a] += lambda * jtj[a * p + a].max(1e-12);
            }
            let mut delta: Vec<f64> = jtr.iter().map(|v| -v).collect();
            if !super::geometry::solve_linear(p, &mut m, &mut delta) {
                lambda *= 10.0;
                continue;
            }
            for d in &mut delta {
                *d = d.clamp(-options.max_move_mm, options.max_move_mm);
            }
            // Halve the step until the zoning stays valid.
            let mut trial = moved(&current, &free, &delta);
            let mut halvings = 0;
            while trial.is_none() && halvings < 4 {
                for d in &mut delta {
                    *d *= 0.5;
                }
                trial = moved(&current, &free, &delta);
                halvings += 1;
            }
            let Some(trial) = trial else {
                lambda *= 10.0;
                continue;
            };
            let ev = evaluate(&trial)?;
            evaluations += 1;
            on_progress(RefineProgress {
                iteration,
                evaluations,
            });
            check_layout(&base, &ev)?;
            let (before, after) = paired_chi2(&base, &ev);
            if after < before * (1.0 - options.min_gain) {
                current = trial;
                base = ev;
                accepted = true;
                break;
            }
            lambda *= 10.0;
        }
        if !accepted {
            break;
        }
        accepted_steps += 1;
    }

    let moves = free
        .iter()
        .zip(&start_values)
        .map(|(&(zone, parameter), &before)| ParameterMove {
            zone,
            parameter,
            before,
            after: read(&current, zone, parameter).unwrap_or(before),
        })
        .collect();
    let chi2_after = chi2(&base);
    Ok(RefineResult {
        zoned: current,
        fit: base.fit.take(),
        iterations: accepted_steps,
        evaluations,
        moves,
        chi2_before,
        chi2_after,
    })
}

/// Refines the zone boundaries against the photos with the real forward tracer and solver.
///
/// `inputs.forward.zones` is ignored (each candidate geometry is traced). The fit settings of
/// `inputs.config` are used with the cross-validation, the roughness search and the alignment
/// refinement switched off (they are not part of the inner loop); pass an
/// `inputs.forward.cache_dir` to reuse traces of geometries that were seen before.
///
/// # Errors
///
/// [`RefineError`] as [`refine_with`].
pub fn refine_zone_geometry(
    inputs: &FitInputs<'_>,
    zoned: &ZonedAbsorption,
    locks: &ZoneLocks,
    options: &RefineOptions,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(RefineProgress),
) -> Result<RefineResult, RefineError> {
    let config = FitConfig {
        lovo: false,
        roughness: None,
        alignment: None,
        ..inputs.config.clone()
    };
    let mut evaluate = |candidate: &ZonedAbsorption| -> Result<Evaluation, RefineError> {
        let forward = ForwardInput {
            zones: Some(candidate),
            ..inputs.forward
        };
        let records = trace_rig(&forward, cancel, &mut |_| {})
            .map_err(|e| RefineError::from(FitError::from(e)))?;
        let fit = fit_records(&records, inputs.observed, &config, cancel, &mut |_| {})?;
        Ok(Evaluation::from_fit(fit))
    };
    refine_with(zoned, locks, options, cancel, on_progress, &mut evaluate)
}

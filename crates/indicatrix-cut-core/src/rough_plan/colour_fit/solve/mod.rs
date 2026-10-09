//! The inverse solver of the colour fit (plan 2026-10-09, section 6).
//!
//! From the stored path records of lane F1 and the rig photos of lane P1 to the absorption
//! spectrum of every zone, with its uncertainty, a validation and a model comparison. Only built
//! with the `zoning` feature.
//!
//! # Entry points
//!
//! * [`fit_colour`]: the whole pipeline. Traces the rig through [`forward`](super::forward), runs
//!   the optional skin roughness search and alignment refinement, then [`fit_records`].
//! * [`fit_records`]: fits records that are already traced. Fits every available model, compares
//!   them, cross-validates and scores the residual of the chosen one.
//! * [`ColourFit`]: the result, serialisable and versioned ([`FIT_VERSION`]).
//!
//! # Model
//!
//! The prediction for view `v` and pixel `p` is the camera RGB relative to the empty backlit rig,
//! times a per-view gain `g_v = exp(gamma_v)` (a nuisance that absorbs exposure drift). The
//! gains are split into a common mode `m = mean(gamma_v)` (prior `N(0, 0.02^2)`, or pinned) and
//! the deviations `gamma_v - m` (prior `N(0, 0.03^2)` each), see `FitConfig::gain_mean_sigma`. The observed value is the transmittance image of lane P1 on the
//! same working grid. Residuals are whitened by the photo variance plus the Monte-Carlo variance of
//! the records (see `problem`), with the Huber loss at `delta = 2 sigma`; the objective is the
//! robust data term plus the priors, minimised by Levenberg-Marquardt on log-parameters with
//! dense normal equations and a fixed iteration order. CIEDE2000 is only ever a reported metric.
//!
//! Two spectral models (plan section 6.2), one parameter block per zone of the records:
//!
//! * [`ChromophoreModel`] (A): log-amounts of the host's catalogue chromophores, through the
//!   catalogue resolver.
//! * [`SmoothBasisModel`] (B): seven log-amplitudes on the body-colour basis, with a
//!   second-difference prior.
//!
//! The default choice follows the plan: A when a host is set and A is not significantly worse
//! than B (likelihood ratio test at 1 %), else B; [`FitConfig::preferred_model`] overrides.
//!
//! # Staged fit (round 3, D1)
//!
//! A common gain is degenerate with a wavelength-flat absorption, and a start with a high gain
//! and a grey floor is a local minimum. Every start therefore runs in three stages, each from the
//! previous result: (1) the absorption with ALL gains held at 1 (the white-frame normalisation
//! makes that the physical expectation), (2) the per-view deviations freed with the common mode
//! pinned, (3) the common mode freed too (skipped with `fix_common_gain`). Besides the
//! `seeds` starts, two structured starts from the best result are staged as well (the flat
//! component of the absorption removed, and one band at the most absorbed channel of each zone;
//! `FitModel::structured_starts`). The lowest TOTAL objective (data plus priors) wins.
//!
//! # Honest noise (round 3, D3 and D4)
//!
//! After the fit the Birge ratio `R = chi2 / (n - p_eff)` is computed; above
//! [`BIRGE_THRESHOLD`] the covariance (and with it the radii) is inflated by `R` and the
//! metamer search accepts `Delta chi2 <= R`. Metamer candidates are verified with the full
//! nonlinear data chi-square and shrunk by bisection until they pass.
//!
//! # Cost and determinism
//!
//! One normal-equation pass visits every valid pixel serially (`for_each_pixel_jacobian`): about
//! `pixels * 3 * (np^2 / 2 + np)` multiply-adds plus the record sums, a second or so for 480 000
//! pixels with `np` of about 10. The independent starts and the held-out refits run in parallel;
//! every task is a complete computation and the results are gathered in task order, so the output
//! is bitwise the same for any `FitConfig::threads` (and for any thread count of the tracer).
//!
//! # Not done here
//!
//! The zone geometry is an input (the records were traced with it); estimating boundaries is a
//! later lane. Nothing here runs without the records of lane F1.

#![allow(
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::suboptimal_flops,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    reason = "index-parallel normal equations and small dense matrices read better as written"
)]

mod diagnostics;
mod fitter;
mod linalg;
mod lm;
mod models;
mod outer;
mod output;
mod par;
mod predict;
mod problem;
mod select;
#[cfg(test)]
mod tests;

use std::fmt;

use indicatrix::color::body_color::Illuminant;

pub use diagnostics::{ObjectiveAt, SUGGEST_MORAN, SUGGEST_RMS, objective_at};
pub use fitter::fit_records;
pub use models::{
    CHROMOPHORE_FD_STEP, CHROMOPHORE_FLOOR_FRACTION, ChromophoreModel, SmoothBasisModel,
};
pub use outer::fit_colour;
pub use output::{
    AlignmentReport, BIRGE_THRESHOLD, ChoiceReason, ColourFit, FIT_VERSION, FitWarning,
    LikelihoodRatio, LovoEntry, LovoReport, ModelComparison, ModelFit, ModelKind, ModelScore,
    ResidualMap, RoughnessReport, WarningKind, ZonePrediction,
};
pub use predict::{METAMER_DIRECTIONS, METAMER_STEP_CAP};
pub use problem::ObservedView;

use super::forward::{ForwardError, ForwardInput, SurfaceMap};

/// Why a fit could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FitError {
    /// The cancel flag was raised.
    Cancelled,
    /// The forward tracer or evaluator failed.
    Forward(ForwardError),
    /// The chromophore catalogue has no such host.
    UnknownHost(String),
    /// The host offers no element to fit.
    NoChromophores(String),
    /// No observation was given for this rig view.
    MissingObservation(usize),
    /// The observation of this rig view does not match the working grid of its records.
    ObservationSize(usize),
    /// No pixel is usable.
    NoData,
    /// A setting is out of range.
    BadConfig(&'static str),
    /// A numerical problem.
    Numerical(&'static str),
}

impl fmt::Display for FitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "the fit was cancelled"),
            Self::Forward(e) => write!(f, "{e}"),
            Self::UnknownHost(h) => write!(f, "the chromophore catalogue has no host '{h}'"),
            Self::NoChromophores(h) => write!(f, "the host '{h}' offers no element to fit"),
            Self::MissingObservation(v) => write!(f, "no photo was given for view {}", v + 1),
            Self::ObservationSize(v) => write!(
                f,
                "the photo of view {} does not match its working grid",
                v + 1
            ),
            Self::NoData => write!(f, "no pixel is usable for the fit"),
            Self::BadConfig(what) => write!(f, "invalid setting: {what}"),
            Self::Numerical(what) => write!(f, "numerical problem: {what}"),
        }
    }
}

impl std::error::Error for FitError {}

impl From<ForwardError> for FitError {
    fn from(e: ForwardError) -> Self {
        match e {
            ForwardError::Cancelled => Self::Cancelled,
            other => Self::Forward(other),
        }
    }
}

/// The stage a [`FitProgress`] report belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitStage {
    /// Tracing the rig.
    Tracing,
    /// The skin roughness search.
    Roughness,
    /// The alignment refinement.
    Alignment,
    /// Fitting the models.
    Fitting,
    /// Comparing the models.
    Selection,
    /// The leave-one-view-out cross-validation and the residual maps.
    CrossValidation,
    /// Finished.
    Done,
}

/// A progress report: the stage and the finished fraction of the stage in `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitProgress {
    /// The stage.
    pub stage: FitStage,
    /// The finished fraction of the stage.
    pub fraction: f32,
}

/// The skin roughness search (golden section, plan section 6.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoughnessSearch {
    /// The smallest GGX roughness (alpha), default 0.05.
    pub min: f32,
    /// The largest, default 0.6.
    pub max: f32,
    /// The traces (and fits) spent, at least 2, default 5.
    pub evaluations: usize,
}

impl Default for RoughnessSearch {
    fn default() -> Self {
        Self {
            min: 0.05,
            max: 0.6,
            evaluations: 5,
        }
    }
}

/// The alignment refinement (6 rigid parameters).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignmentRefine {
    /// Gauss-Newton steps, default 2.
    pub steps: usize,
    /// The finite-difference step of a rotation-vector component, rad, default 0.003.
    pub rotation_step_rad: f64,
    /// The finite-difference step of a translation component, mm, default 0.05.
    pub translation_step_mm: f64,
}

impl Default for AlignmentRefine {
    fn default() -> Self {
        Self {
            steps: 2,
            rotation_step_rad: 0.003,
            translation_step_mm: 0.05,
        }
    }
}

/// Which predicted colours the fit reports (plan section 6.5).
#[derive(Debug, Clone, PartialEq)]
pub struct PredictionConfig {
    /// The reference stone width, mm, default 7.
    pub reference_mm: f64,
    /// The caller's stone width, mm (default 12); not reported when not positive.
    pub caller_mm: f64,
    /// The girdle width of the design in model units: the face-up path is
    /// `MODEL_UNIT_FACE_UP_PATH * width / model_width_units` (default 1).
    pub model_width_units: f64,
    /// The illuminants, default D65 and A (`Planckian(2856)`).
    pub illuminants: Vec<Illuminant>,
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self {
            reference_mm: 7.0,
            caller_mm: 12.0,
            model_width_units: 1.0,
            illuminants: vec![Illuminant::D65, Illuminant::Planckian(2856.0)],
        }
    }
}

impl PredictionConfig {
    /// The distinct positive widths, reference first.
    pub(super) fn sizes(&self) -> Vec<f64> {
        let mut sizes = vec![self.reference_mm];
        if self.caller_mm > 0.0 && (self.caller_mm - self.reference_mm).abs() > 1e-9 {
            sizes.push(self.caller_mm);
        }
        sizes
    }
}

/// Default sigma of the per-view deviation of the log-gains from their mean (round 2, C1).
pub const GAIN_DEVIATION_SIGMA: f64 = 0.03;
/// Default sigma of the mean log-gain (the common mode, round 2, C1).
pub const GAIN_MEAN_SIGMA: f64 = 0.02;
/// The sigma the common mode gets when [`FitConfig::fix_common_gain`] is set: a numerical pin.
pub const GAIN_FIXED_SIGMA: f64 = 1e-4;

/// The settings of a fit. The defaults are the plan's.
#[derive(Debug, Clone, PartialEq)]
pub struct FitConfig {
    /// The catalogue host (`"tourmaline"`, `"quartz"`, ...) for model A; `None` fits model B only.
    pub host_id: Option<String>,
    /// Catalogue treatments applied to every zone of model A (not fitted).
    pub treatments: Vec<String>,
    /// The user's choice of model; `None` follows the plan's rule.
    pub preferred_model: Option<ModelKind>,
    /// Sigma of the deviation of a view's log-gain from the mean log-gain of the views
    /// ([`GAIN_DEVIATION_SIGMA`], 0.03). P1 normalises every photo to its own white frame, so a gain
    /// only models exposure drift between the stone and white frames of that view.
    pub gain_sigma: f64,
    /// Sigma of the prior on the common mode of the log-gains, their mean over the views
    /// ([`GAIN_MEAN_SIGMA`], 0.02). The common gain is degenerate with a wavelength-flat absorption
    /// whenever the path length hardly varies across the pixels (a cube), so it must be pinned
    /// by the calibration knowledge that the white-frame normalisation leaves it near 1.
    pub gain_mean_sigma: f64,
    /// Pin the common mode of the log-gains at 0 (sigma [`GAIN_FIXED_SIGMA`]), for rigs with locked
    /// exposure; default off.
    pub fix_common_gain: bool,
    /// The Huber threshold in sigma, default 2.
    pub huber_k: f64,
    /// Sigma of the second difference of the log-amplitudes of model B, default 0.5.
    pub smoothness_sigma: f64,
    /// Sigma of the weak magnitude prior of model B around 0.1 per mm, default 3.
    pub magnitude_sigma: f64,
    /// Weight of the reweighted L1 prior on the relative amounts of model A, default 0.05
    /// (0 turns it off).
    pub l1_weight: f64,
    /// Reweighting rounds of the L1 prior, default 3.
    pub l1_rounds: usize,
    /// Sigma of a pull of every zone's log parameters towards the base zone ("same family");
    /// `None` (default) leaves the zones independent.
    pub zone_pull_sigma: Option<f64>,
    /// Deterministic starts, default 8.
    pub seeds: usize,
    /// Iterations of the first stage of every start, default 12.
    pub stage1_iterations: usize,
    /// Starts that continue after the first stage, default 2.
    pub finalists: usize,
    /// Iteration limit of every stage of a run, default 500 (round 3, D1: it was 200, and the
    /// watermelon cube hit it).
    pub max_iterations: usize,
    /// Stop when an accepted step lowers the objective by less than this fraction, default 1e-9.
    pub rel_cost_tol: f64,
    /// Whether to run the leave-one-view-out cross-validation, default true.
    pub lovo: bool,
    /// Iteration limit of a held-out refit, default 25.
    pub lovo_iterations: usize,
    /// Factor on the Monte-Carlo variance of the records, default 1.
    pub mc_variance_scale: f64,
    /// The smallest pixel variance, default 1e-8.
    pub variance_floor: f64,
    /// Worker threads for the starts and held-out refits; 0 means all cores. Does not change the
    /// result.
    pub threads: usize,
    /// The skin roughness search (needs `FitInputs::surfaces_for_roughness`); `None` skips it.
    pub roughness: Option<RoughnessSearch>,
    /// The alignment refinement; `None` (default) skips it.
    pub alignment: Option<AlignmentRefine>,
    /// The reported colours.
    pub prediction: PredictionConfig,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self {
            host_id: None,
            treatments: Vec::new(),
            preferred_model: None,
            gain_sigma: GAIN_DEVIATION_SIGMA,
            gain_mean_sigma: GAIN_MEAN_SIGMA,
            fix_common_gain: false,
            huber_k: 2.0,
            smoothness_sigma: 0.5,
            magnitude_sigma: 3.0,
            l1_weight: 0.05,
            l1_rounds: 3,
            zone_pull_sigma: None,
            seeds: 8,
            stage1_iterations: 12,
            finalists: 2,
            max_iterations: 500,
            rel_cost_tol: 1e-9,
            lovo: true,
            lovo_iterations: 25,
            mc_variance_scale: 1.0,
            variance_floor: 1e-8,
            threads: 0,
            roughness: None,
            alignment: None,
            prediction: PredictionConfig::default(),
        }
    }
}

impl FitConfig {
    pub(super) fn validate(&self) -> Result<(), FitError> {
        let bad = FitError::BadConfig;
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if !(positive(self.gain_sigma)
            && positive(self.gain_mean_sigma)
            && positive(self.huber_k)
            && positive(self.smoothness_sigma)
            && positive(self.magnitude_sigma)
            && positive(self.variance_floor))
        {
            return Err(bad(
                "gain_sigma, gain_mean_sigma, huber_k, smoothness_sigma, magnitude_sigma and variance_floor \
                 must be positive",
            ));
        }
        if !(self.l1_weight.is_finite() && self.l1_weight >= 0.0) {
            return Err(bad("l1_weight must not be negative"));
        }
        if !(self.rel_cost_tol.is_finite() && self.rel_cost_tol >= 0.0) {
            return Err(bad("rel_cost_tol must not be negative"));
        }
        if !(self.mc_variance_scale.is_finite() && self.mc_variance_scale >= 0.0) {
            return Err(bad("mc_variance_scale must not be negative"));
        }
        if self.seeds == 0 || self.finalists == 0 || self.max_iterations == 0 {
            return Err(bad(
                "seeds, finalists and max_iterations must be at least 1",
            ));
        }
        if let Some(sigma) = self.zone_pull_sigma
            && !positive(sigma)
        {
            return Err(bad("zone_pull_sigma must be positive"));
        }
        if let Some(r) = &self.roughness
            && !(r.min.is_finite() && r.max.is_finite() && 0.0 < r.min && r.min < r.max)
        {
            return Err(bad("the roughness range must be positive and increasing"));
        }
        if let Some(a) = &self.alignment
            && !(a.steps > 0 && positive(a.rotation_step_rad) && positive(a.translation_step_mm))
        {
            return Err(bad("the alignment steps must be positive"));
        }
        if !(positive(self.prediction.reference_mm) && positive(self.prediction.model_width_units))
        {
            return Err(bad(
                "the prediction widths and the model width must be positive",
            ));
        }
        Ok(())
    }
}

/// Everything [`fit_colour`] needs.
#[derive(Clone, Copy)]
pub struct FitInputs<'a> {
    /// The trace: mesh, alignment, rig, zones, camera and the rest (see
    /// [`ForwardInput`]). Its `surfaces` are used unless a roughness search is configured.
    pub forward: ForwardInput<'a>,
    /// The photos on the working grids of `forward.views`, matched by rig view index.
    pub observed: &'a [ObservedView],
    /// The settings.
    pub config: &'a FitConfig,
    /// The surface map for a skin roughness (GGX alpha), for the roughness search.
    pub surfaces_for_roughness: Option<&'a (dyn Fn(f32) -> SurfaceMap + Sync)>,
}

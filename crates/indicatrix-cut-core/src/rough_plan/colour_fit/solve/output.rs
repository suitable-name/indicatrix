//! The result types of the fit. Everything is plain data and serialisable; [`ColourFit::version`]
//! is [`FIT_VERSION`], bumped whenever a field changes meaning.

use indicatrix::{
    color::body_color::Illuminant,
    optics::{absorption::AbsorptionTensor, zoning::ZoneAbsorption},
};
use serde::{Deserialize, Serialize};

use crate::rough_plan::locate::Rigid;

/// The version of the serialised [`ColourFit`].
pub const FIT_VERSION: u32 = 1;

/// The Birge ratio above which the covariance and the radii are inflated (round 3, D4.1).
pub const BIRGE_THRESHOLD: f64 = 1.2;

/// The spectral model of a fit (plan section 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelKind {
    /// A: log-amounts of the host's catalogue chromophores.
    Chromophore,
    /// B: the seven-band body-colour basis with a smoothness prior.
    SmoothBasis,
}

impl ModelKind {
    /// A short name for reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Chromophore => "chromophore",
            Self::SmoothBasis => "smooth basis",
        }
    }
}

/// The predicted face-up colour of a stone made of one zone, with its 1 sigma uncertainty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZonePrediction {
    /// The zone (0 is the base zone).
    pub zone: usize,
    /// The stone width the colour is predicted for, mm.
    pub size_mm: f64,
    /// The mean face-up light path used, mm.
    pub path_mm: f64,
    /// The illuminant.
    pub illuminant: Illuminant,
    /// The predicted CIELAB colour (relative to the illuminant's own white).
    pub lab: [f64; 3],
    /// The 1 sigma standard deviation of each Lab component.
    pub lab_sigma: [f64; 3],
    /// The 1 sigma radius in CIEDE2000 units: the root mean square CIEDE2000 distance from the
    /// prediction over the posterior.
    pub delta_e_radius: f64,
    /// The metamer spread: the largest CIEDE2000 from `lab` among parameter vectors with
    /// `Delta chi2 <= 1` relative to the optimum, computed WITHOUT the smoothness and magnitude
    /// priors (round 2, C3). Unlike `delta_e_radius` it does not hide directions the camera data
    /// cannot see; a spread above the accuracy target means the target is a metamer limit of the
    /// data, not an error of the fit. An estimate from a deterministic search, not a bound.
    ///
    /// Round 3, D3: every candidate is VERIFIED with the full nonlinear data chi-square
    /// (`Delta chi2_data <= 1`, or the Birge ratio when that is above 1.2), has a non-negative
    /// absorption at the basis centres and on a 5 nm grid from 400 to 700 nm, and is shrunk by
    /// bisection until it passes instead of being dropped.
    #[serde(default)]
    pub metamer_spread: f64,
    /// The same search before the verification of round 3, D3 (the quadratic-model candidates
    /// as they were): shown next to `metamer_spread` so that the effect of the verification is
    /// visible.
    #[serde(default)]
    pub metamer_spread_unverified: f64,
}

const fn one() -> f64 {
    1.0
}

/// One fitted model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelFit {
    /// The model.
    pub kind: ModelKind,
    /// The host id of a chromophore model.
    pub host_id: Option<String>,
    /// The name of every model parameter, zone-major.
    pub param_names: Vec<String>,
    /// The model parameters (natural logarithms of amounts or amplitudes), zone-major.
    pub params: Vec<f64>,
    /// The rig view index of every gain, in the order of the records.
    pub view_ids: Vec<usize>,
    /// The natural logarithm of every view's gain.
    pub log_gains: Vec<f64>,
    /// The posterior covariance of `[params, log_gains]`, row-major, `(np + views)^2` entries:
    /// the inverse of the Gauss-Newton matrix with the priors included.
    pub covariance: Vec<f64>,
    /// The fitted absorption of every zone as band sets in 1/mm (build [`ZoneAbsorption`] with
    /// [`ModelFit::zone_absorptions`]).
    pub zone_tensors: Vec<AbsorptionTensor>,
    /// The total objective (half the robust chi-square plus the priors).
    pub cost: f64,
    /// The robust (Huber) data term alone; `cost - data_cost` is the prior part (round 3, D1.3).
    #[serde(default)]
    pub data_cost: f64,
    /// The prior part of `cost` by term, `(name, value)`: gain, smoothness, magnitude, l1, zone
    /// pull (round 3, D1.3).
    #[serde(default)]
    pub prior_terms: Vec<(String, f64)>,
    /// The Birge ratio `chi2 / (n_data - effective_params)` (round 3, D4.1). When it exceeds
    /// [`BIRGE_THRESHOLD`] the covariance and the predictions' radii are inflated by it: the
    /// noise model was too small. 1 for a fit that was never scored.
    #[serde(default = "one")]
    pub birge_ratio: f64,
    /// The plain chi-square of the data term, `sum u^2` over the whitened residuals.
    pub chi2: f64,
    /// The number of residuals in the data term.
    pub n_data: usize,
    /// The effective number of parameters, `trace(H^-1 H_data)`.
    pub effective_params: f64,
    /// Akaike information criterion with the common noise scale of the comparison.
    pub aic: f64,
    /// Levenberg-Marquardt iterations of the winning start.
    pub iterations: usize,
    /// Whether the iteration stopped on its tolerance.
    pub converged: bool,
    /// The objective of every start after the first stage, in seed order.
    pub seed_costs: Vec<f64>,
    /// Names of parameters sitting on a bound.
    pub at_bound: Vec<String>,
    /// The predicted face-up colours: every zone, every size, every illuminant.
    pub predictions: Vec<ZonePrediction>,
}

impl ModelFit {
    /// The fitted zones as per-millimetre absorptions, ready for a `ZonedAbsorption`.
    #[must_use]
    pub fn zone_absorptions(&self) -> Vec<ZoneAbsorption> {
        self.zone_tensors
            .iter()
            .map(|tensor| ZoneAbsorption::per_mm(tensor.clone()))
            .collect()
    }

    /// The gain of every view (`exp` of the log-gain).
    #[must_use]
    pub fn gains(&self) -> Vec<f64> {
        self.log_gains.iter().map(|g| g.exp()).collect()
    }
}

/// Why a model was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChoiceReason {
    /// The caller named the model.
    UserPreference,
    /// Only one model was fitted (no host set).
    OnlyOneModel,
    /// The chromophore model is not significantly worse than the smooth basis.
    ChromophoreAcceptable,
    /// The chromophore model is significantly worse (likelihood ratio test), so the smooth basis
    /// was chosen.
    ChromophoreSignificantlyWorse,
}

/// The scores of one model in the comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelScore {
    /// The model.
    pub kind: ModelKind,
    /// Plain chi-square of the data term.
    pub chi2: f64,
    /// Effective parameters.
    pub effective_params: f64,
    /// Akaike information criterion.
    pub aic: f64,
}

/// The likelihood ratio test of the chromophore model against the smooth basis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LikelihoodRatio {
    /// `(chi2_A - chi2_B) / noise_scale2`.
    pub statistic: f64,
    /// Degrees of freedom: the difference of the effective parameters, at least 1.
    pub dof: f64,
    /// The chi-square quantile at the test level.
    pub critical: f64,
    /// Whether the statistic exceeds the quantile.
    pub chromophore_significantly_worse: bool,
}

/// The comparison of the fitted models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelComparison {
    /// The scores, in the order of [`ColourFit::fits`].
    pub scores: Vec<ModelScore>,
    /// The common noise scale (a variance factor, at least 1): the data may be noisier than the
    /// noise model says.
    pub noise_scale2: f64,
    /// The likelihood ratio test, when both models were fitted.
    pub likelihood_ratio: Option<LikelihoodRatio>,
    /// The chosen model.
    pub chosen: ModelKind,
    /// Why.
    pub reason: ChoiceReason,
}

/// The result of the skin roughness search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoughnessReport {
    /// The chosen GGX roughness (alpha).
    pub roughness: f32,
    /// Every evaluated roughness with its objective, in evaluation order.
    pub evaluations: Vec<(f32, f64)>,
}

/// The result of the alignment refinement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignmentReport {
    /// The alignment given.
    pub initial: Rigid,
    /// The alignment used for the final fit.
    pub refined: Rigid,
    /// The summed squared whitened residual before, on the pixels common to all traces.
    pub chi2_before: f64,
    /// And after.
    pub chi2_after: f64,
    /// The Gauss-Newton steps taken (accepted).
    pub steps: usize,
}

/// One held-out view of the leave-one-view-out cross-validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LovoEntry {
    /// The rig view index.
    pub view: usize,
    /// The pixels the comparison used.
    pub pixels: usize,
    /// CIEDE2000 between the held-out view's mean colour and the prediction of the 7-view fit,
    /// with the view's gain estimated from that view alone.
    pub delta_e: f64,
    /// The same with the gain the full fit found for the view.
    pub delta_e_full_fit_gain: f64,
    /// The gain used for `delta_e`.
    pub gain: f64,
}

/// The leave-one-view-out cross-validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LovoReport {
    /// One entry per held-out view.
    pub entries: Vec<LovoEntry>,
    /// The median of `delta_e`.
    pub median_delta_e: f64,
    /// The maximum of `delta_e`.
    pub max_delta_e: f64,
}

/// The whitened residual of one view on its working grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResidualMap {
    /// The rig view index.
    pub view: usize,
    /// Working pixels across.
    pub width: usize,
    /// Working pixels down.
    pub height: usize,
    /// `(observed - gain * predicted) / sigma` per pixel and channel; zero where `valid` is
    /// false.
    pub whitened: Vec<[f32; 3]>,
    /// Whether the pixel entered the fit.
    pub valid: Vec<bool>,
    /// Moran's I of the worst channel (spatial autocorrelation of the residual).
    pub moran: f64,
    /// Root mean square of the whitened residual over the valid pixels and channels.
    pub rms: f64,
}

/// The kind of a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningKind {
    /// Many pixels were dropped by the tracer.
    PixelsDropped,
    /// The tracer lost throughput at the depth limit.
    DepthLoss,
    /// The record compression was coarse.
    CompressionCoarse,
    /// A parameter sits on a bound.
    ParameterAtBound,
    /// A model did not reach its tolerance.
    NotConverged,
    /// A gain is far from 1.
    LargeGain,
    /// The preferred model was not available.
    PreferenceIgnored,
    /// Too few valid pixels in a view for the cross-validation.
    LovoSkipped,
    /// A view has no usable pixels.
    EmptyView,
    /// The fit's Birge ratio is above [`BIRGE_THRESHOLD`]: the photos scatter more than the
    /// noise model says, and the uncertainties were inflated accordingly (round 3, D4.1).
    NoiseUnderestimated,
}

/// A warning with its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FitWarning {
    /// The kind.
    pub kind: WarningKind,
    /// A sentence for the report.
    pub message: String,
}

/// The result of a colour fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColourFit {
    /// [`FIT_VERSION`].
    pub version: u32,
    /// The zones: 1 (base) plus the shaped zones of the records.
    pub n_zones: usize,
    /// The fitted models: the chromophore model first (when a host was set), then the smooth
    /// basis.
    pub fits: Vec<ModelFit>,
    /// The index of the chosen model in `fits`.
    pub chosen_index: usize,
    /// The comparison.
    pub comparison: ModelComparison,
    /// The roughness search (only from `fit_colour` with a search configured).
    pub roughness: Option<RoughnessReport>,
    /// The alignment refinement (only from `fit_colour` with it enabled).
    pub alignment: Option<AlignmentReport>,
    /// The cross-validation of the chosen model.
    pub lovo: Option<LovoReport>,
    /// The residual maps of the chosen model, one per view.
    pub residuals: Vec<ResidualMap>,
    /// The pixel-weighted mean of the views' Moran's I.
    pub structured_score: f64,
    /// The pixel-weighted root mean square whitened residual.
    pub residual_rms: f64,
    /// Whether the residual is structured enough to ask the user whether the stone is zoned.
    pub suggest_zoning: bool,
    /// Warnings.
    pub warnings: Vec<FitWarning>,
}

impl ColourFit {
    /// The chosen model's fit.
    #[must_use]
    pub fn chosen_fit(&self) -> &ModelFit {
        &self.fits[self.chosen_index]
    }

    /// The chosen model's zones as per-millimetre absorptions.
    #[must_use]
    pub fn zone_absorptions(&self) -> Vec<ZoneAbsorption> {
        self.chosen_fit().zone_absorptions()
    }

    /// The chosen model's prediction for `zone`, `size_mm` (within 1e-9) and `illuminant`.
    #[must_use]
    pub fn prediction(
        &self,
        zone: usize,
        size_mm: f64,
        illuminant: Illuminant,
    ) -> Option<&ZonePrediction> {
        self.chosen_fit().predictions.iter().find(|p| {
            p.zone == zone && (p.size_mm - size_mm).abs() < 1e-9 && p.illuminant == illuminant
        })
    }
}

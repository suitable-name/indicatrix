//! The spectral models of the fit (plan section 6.2), as [`SpectralModel`]s for the forward
//! evaluator plus what the solver needs on top (bounds, seeds, priors).
//!
//! * [`ChromophoreModel`] (A): per zone, the log-amounts of the selectable elements of a catalogue
//!   host. The absorption is the catalogue resolver's own: the amounts go into a `ColorRecipe`,
//!   `resolve` returns the band sets, and `ZoneAbsorption::alpha` evaluates them (the
//!   orientation mean for a pleochroic host, since the rig does not know the crystal axis). The
//!   resolver is not linear in the amounts (pair laws, valence splits, the 8 band budget), so the
//!   derivative is a central difference in the log-amount with a fixed step
//!   ([`CHROMOPHORE_FD_STEP`]), clamped at the bounds. Resolution is cached per parameter vector.
//! * [`SmoothBasisModel`] (B): per zone, the log-amplitudes of the fourteen bands of
//!   [`FIT_BASIS_NM`]; the derivative is analytic. The fitted spectrum is exported to the
//!   renderer (and the GPU, which carries at most eight bands per material) by a least-squares
//!   refit onto the seven-band body-colour basis, see [`SmoothBasisModel`].
//!
//! Parameters are zone-major: zone `z` owns `params[z * per_zone .. (z + 1) * per_zone]`.
//!
//! # The priors of model B (round 5, ZC-1)
//!
//! The camera sees three numbers per pixel and path length, so a fourteen-band spectrum has a
//! large null space: the priors only have to bound it, and must never decide the answer. Two
//! rules follow, and the first version of this model broke both:
//!
//! 1. **The prior acts on the linear amplitudes `a_k = exp(theta_k)`, not on their logarithms.**
//!    A second difference of `theta` charges a band that sits next to an empty neighbour (the
//!    log of an empty band is `-13.8`) about 1800 per band at sigma 0.5, four times the data term
//!    of a saturated two-band stone, so the optimiser escaped into a wavelength-flat floor and
//!    paid for it with the common gain (the +12 % of the round-4 run).
//! 2. **"Zero absorption" must be free.** A physical stone is transparent over most of the
//!    visible, so the magnitude prior is an exponential (L1) prior on `a_k`, whose cost vanishes
//!    at zero, not a Gaussian centred on 0.1 per mm.
//!
//! The smoothness prior is a Gaussian-process prior on the amplitude vector,
//! `cost = 1/2 a^T K^-1 a`, with the covariance `K_kl = sigma_a^2 k(|c_k - c_l| / ell)` over the
//! basis centres `c` (nm). The kernel is the **Matern 5/2** kernel
//! `k(r) = (1 + sqrt5 r + 5 r^2 / 3) exp(-sqrt5 r)`, not the squared exponential the review
//! proposed. Both were measured on the thirteen centres 420 to 720 nm (25 nm apart, `ell = 60 nm`,
//! projected truth spectra of the validation suite, non-negative least squares); the 395 nm band
//! added later extends the same grid by one point and was not re-measured:
//!
//! | kernel | condition of `K` | cost, saturated truth | cost, one band of 0.2 per mm |
//! |---|---|---|---|
//! | squared exponential | 8.7e7 | 9.5e3 | 7.8e4 |
//! | Matern 5/2 (`sigma_a` 1 per mm, measured) | 7.5e2 | 0.15 | 0.80 |
//! | Matern 5/2 (`sigma_a` 0.3 per mm, scaled by 1/0.09) | 7.5e2 | 1.7 | 8.9 |
//!
//! The scales matter as much as the kernel (round 6, R2). `sigma_a` is the prior standard
//! deviation of an amplitude and body-colour amplitudes are 0.01 to 0.3 per mm, so the first
//! `sigma_a = 1` bounded nothing: the whole prior budget (1 to 3) was below the 25 the data term
//! fluctuates by, and the 11 null directions of the camera per zone were set by noise. With
//! `sigma_a = 0.3` per mm and the L1 scale `b = 0.05` per mm the saturated truth costs 1.7 + 7.5
//! = 9.2, one isolated 0.2 per mm band 8.9 + 4.0 = 12.9 (both under the bound of `2 BANDS`
//! = 28), and a camera metamer with a red hump of 0.38 per mm costs 29, about 20 more than the
//! truth, against the 2 the data can offer for it. The bias of a measured band is
//! `(1/b) / curvature = 20 / 7e7` per mm: nothing.
//!
//! A third, scale-free "shape" prior was tried on 2026-10-12 (a Matern 5/2 process on the
//! detrended softened log-amplitudes `ln(a_k + eps)`, so that a log-linear spectrum, any overall
//! scale and the clear stone cost nothing) and taken out again. The camera leaves the spectrum
//! free along about eleven directions per zone, and along them the magnitude prior picks the
//! spectrum with the least absorption: one broad hump with dead edges (for a log-linear truth of
//! 0.12 per mm the fit has 0 at 395 nm and 0.06 instead of 0.20 at 670 nm, a data term 4 BELOW
//! the truth's, and a magnitude cost 12.5 below it). To pull such a fit back onto the truth a
//! shape prior must charge the hump more than those 12.5 + 4, and no smoothness measure does
//! that without charging the two-band saturated validation truth or an isolated band far more
//! (measured: every setting that beats the hump costs the isolated band 75 to 6000 and the
//! projected saturated truth 25 to 400 against the budget of 28). A hump is smoother than a
//! two-band stone by any measure; the null space is a metamer statement, which the spread
//! reports (see `predict`), not something a prior can settle.
//!
//! The squared exponential forbids, by construction, every feature narrower than `ell / sqrt2`
//! (42 nm), and a Gaussian band of 33 nm (the 2600 cm^-1 bands of the validation truths) is one
//! of them: its cost is finite on the grid points only because the matrix is regularised by
//! rounding. Matern 5/2 is twice differentiable, keeps `K` well conditioned, and still keeps
//! the null space bounded.
//!
//! With `L L^T = K^-1` (Cholesky) the cost is `1/2 |L^T a|^2`, so the prior rows are
//! `r_i = sum_{k >= i} L_ki a_k` with `dr_i / dtheta_k = L_ki a_k`. The exponential prior
//! `cost_k = a_k / b` has the row `r_k = sqrt(2 a_k / b)`, `dr_k / dtheta_k = r_k / 2`.

use std::sync::{Arc, Mutex, PoisonError};

use indicatrix::optics::{
    absorption::{AbsorptionTensor, BODY_COLOR_BASIS_NM, body_color_bands},
    chromophore::{ChromophoreCatalogue, ColorRecipe, resolve},
    zoning::ZoneAbsorption,
};

use super::{
    FitConfig, FitError, ModelKind,
    linalg::{cholesky, inverse_spd, solve_spd},
};
use crate::rough_plan::colour_fit::forward::SpectralModel;

/// The step in the log-amount of the central difference of [`ChromophoreModel`].
pub const CHROMOPHORE_FD_STEP: f64 = 0.02;

/// The smallest amount of a chromophore element, relative to its catalogue maximum. The
/// log-amount is bounded below by this, so "absent" is a tiny positive amount.
pub const CHROMOPHORE_FLOOR_FRACTION: f64 = 1e-4;

/// Bounds of a basis log-amplitude of the smooth model: 1e-6 to 30 per mm.
const BASIS_LOG_MIN: f64 = -13.815_510_557_964_274;
const BASIS_LOG_MAX: f64 = 3.401_197_381_662_155_4;
/// The level of the tilted seeds of the smooth model, 0.1 per mm (`ln 0.1 = -ln 10`).
const SEED_LOG_LEVEL: f64 = -std::f64::consts::LN_10;

/// The largest amplitude of the amplitude-space view of model B, per mm (`exp(BASIS_LOG_MAX)`).
const AMPLITUDE_MAX: f64 = 30.0;

/// The basis of the smooth-basis fit (round 5, ZC-6): `(centre nm, width nm)` of fourteen
/// Gaussians.
///
/// The Gaussians are 25 nm apart from 395 to 720 nm, each with a standard deviation of 15 nm
/// (FWHM 35 nm). The band at 395 nm was added after the first thirteen-band version (420 to
/// 720 nm) could not follow the 390 nm absorber tail of the saturated validation truth (0.09
/// per mm short at the 400 nm edge). The width is the wavelength-domain standard deviation, converted to the
/// wavenumber standard deviation `width * 1e7 / centre^2` of an energy-domain band exactly as
/// `body_color_bands` does for the body-colour basis.
///
/// The width is narrower than the spacing on purpose (ZC-6 round 6, R1). With 25 nm the 395 nm
/// band had a wavenumber sigma of 1602 cm^-1, wider than the 1486 cm^-1 of the 3500 cm^-1
/// wide 390 nm absorber of the saturated validation truth, and a non-negative sum of Gaussians
/// cannot follow a feature narrower than its widest member: the projection overshot the
/// 400 to 470 nm wing (1.75 to 2.96 dE2000 lost to the basis alone). At 15 nm the 395 nm band is
/// 961 cm^-1 wide (720 nm: 289 cm^-1). The price is the ripple of a flat non-negative sum at
/// the spacing `h = 25 nm`: `2 exp(-2 pi^2 (15 / 25)^2) = 0.17 %`. At 12.5 nm it would be
/// 1.5 %, so 15 nm is the widest value under 0.2 % that resolves the absorber.
///
/// The fit needs its own table: the seven bands of `BODY_COLOR_BASIS_NM` (40 nm apart, 45 to
/// 55 nm wide) belong to the body-colour slider and the GPU (at most eight bands per material),
/// and a non-negative sum of 45 nm Gaussians cannot be narrower than 45 nm, which costs 2 to 3
/// L* on a crystal-field band of 25 to 35 nm. The GPU export (`project_onto` the seven bands)
/// refits a narrower spectrum less well; the `ExportRefit` warning (0.5 dE and more) reports it.
/// Fourteen bands times four zones plus eight gains is 64 parameters, just over the plan's
/// budget of 60 (section 6.3); the priors bound the null space.
pub const FIT_BASIS_NM: [(f32, f32); 14] = [
    (395.0, 15.0),
    (420.0, 15.0),
    (445.0, 15.0),
    (470.0, 15.0),
    (495.0, 15.0),
    (520.0, 15.0),
    (545.0, 15.0),
    (570.0, 15.0),
    (595.0, 15.0),
    (620.0, 15.0),
    (645.0, 15.0),
    (670.0, 15.0),
    (695.0, 15.0),
    (720.0, 15.0),
];

/// The value of band `k` of `basis` at `lambda_nm` (unit peak), Gaussian in wavenumber.
fn basis_value(basis: &[(f32, f32)], k: usize, lambda_nm: f64) -> f64 {
    let (centre, width) = basis[k];
    let sigma_nu = f64::from(width) * 1.0e7 / (f64::from(centre) * f64::from(centre));
    let delta_nm = f64::from(centre) - lambda_nm;
    let nu_diff = 1.0e7 * delta_nm / (lambda_nm * f64::from(centre));
    let t = nu_diff / sigma_nu;
    (-0.5 * t * t).exp()
}

/// The Matern 5/2 kernel at the scaled distance `r = |d| / ell`.
fn matern52(r: f64) -> f64 {
    let s = 5.0_f64.sqrt() * r;
    (1.0 + s + s * s / 3.0) * (-s).exp()
}

/// The lower Cholesky factor `L` (row-major, `BANDS x BANDS`) of the precision matrix
/// `K^-1` of the smoothness prior, `K_kl = sigma_a^2 matern52(|c_k - c_l| / ell_nm)` over the
/// centres of [`FIT_BASIS_NM`]. `None` when the parameters are not positive finite numbers or
/// `K` is numerically singular (a length of many band spacings); `FitConfig::validate` rejects
/// such a configuration, so the model never meets `None` in a running fit.
pub(super) fn smoothness_factor(sigma_a: f64, ell_nm: f64) -> Option<Vec<f64>> {
    if !(sigma_a.is_finite() && sigma_a > 0.0 && ell_nm.is_finite() && ell_nm > 0.0) {
        return None;
    }
    let n = FIT_BASIS_NM.len();
    let mut k = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let d = f64::from(FIT_BASIS_NM[i].0 - FIT_BASIS_NM[j].0).abs();
            k[i * n + j] = sigma_a * sigma_a * matern52(d / ell_nm);
        }
    }
    let precision = inverse_spd(&k, n)?;
    cholesky(&precision, n)
}

/// Which prior a [`PriorRow`] belongs to (round 3, D1.3: the report names the dominant term).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PriorTag {
    /// The Gaussian-process smoothness of the amplitudes of model B.
    Smoothness,
    /// The exponential (L1) magnitude prior of model B.
    Magnitude,
    /// The reweighted L1 prior of model A.
    L1,
    /// The pull of the zones towards the base zone.
    ZonePull,
}

/// One prior residual row: `value` and its derivative with respect to some parameters.
#[derive(Debug, Clone)]
pub(super) struct PriorRow {
    pub value: f64,
    pub entries: Vec<(usize, f64)>,
    pub tag: PriorTag,
}

/// What the priors depend on besides the parameters.
pub(super) struct PriorContext<'a> {
    pub config: &'a FitConfig,
    /// The reweighted L1 weights (one per parameter; only the chromophore model reads them).
    pub l1_weights: &'a [f64],
    /// Pin the common mode of the log-gains (stage 2 of the staged fit, round 3, D1).
    pub pin_common_gain: bool,
    /// Whether the model's own priors (smoothness, magnitude, L1, zone pull) are in the
    /// objective. `false` leaves the data term and the gain prior alone: the data-only optimum
    /// of the constrained colour search (round 5, ZC-4).
    pub model_priors: bool,
}

/// A spectral model together with what the solver needs.
pub(super) trait FitModel: SpectralModel {
    /// `self` as the forward evaluator's trait object.
    fn as_spectral(&self) -> &dyn SpectralModel;
    /// Which model.
    fn kind(&self) -> ModelKind;
    /// The number of zones.
    fn zone_count(&self) -> usize;
    /// Parameters per zone.
    fn per_zone(&self) -> usize;
    /// Names of all parameters.
    fn param_names(&self) -> Vec<String>;
    /// Lower and upper bounds of all parameters.
    fn bounds(&self) -> (Vec<f64>, Vec<f64>);
    /// The `index`-th deterministic start.
    fn seed(&self, index: usize) -> Vec<f64>;
    /// Appends the prior residual rows at `params`.
    fn prior_rows(&self, params: &[f64], ctx: &PriorContext<'_>, out: &mut Vec<PriorRow>);
    /// The reweighted L1 weights for the next round (all 1 for a model without L1).
    fn l1_weights(&self, params: &[f64]) -> Vec<f64>;
    /// The band sets of every zone at `params`.
    fn tensors(&self, params: &[f64]) -> Vec<AbsorptionTensor>;
    /// The step of a central difference in a parameter (for the derivative of derived
    /// quantities such as the colour).
    fn fd_step(&self) -> f64;
    /// The host id, for a chromophore model.
    fn host(&self) -> Option<&str> {
        None
    }
    /// An independent copy (its own caches; the smooth model shares its configuration-keyed
    /// factor cache on purpose), for use on another thread.
    fn duplicate(&self) -> Box<dyn FitModel>;
    /// Extra deterministic starts derived from a fitted parameter vector (round 3, D1.2), each a
    /// full model parameter vector. Empty for a model that has none.
    fn structured_starts(&self, _params: &[f64]) -> Vec<Vec<f64>> {
        Vec::new()
    }
    /// A parametrisation of the same absorption in which the data term is much closer to a
    /// quadratic, for the constrained colour search (round 5, ZC-4); `None` when the model's own
    /// parameters are already the right ones. The returned model has the same zones and no
    /// priors, and `to_search_space`/`from_search_space` convert parameter vectors (without
    /// gains) between the two.
    fn search_space(&self) -> Option<Box<dyn FitModel>> {
        None
    }
    /// The parameters of [`search_space`](Self::search_space) for the model parameters `params`.
    fn to_search_space(&self, params: &[f64]) -> Vec<f64> {
        params.to_vec()
    }
    /// The model parameters for the search-space parameters `search`.
    #[allow(
        clippy::wrong_self_convention,
        reason = "the converting pair to_search_space/from_search_space is symmetric and reads the model's layout"
    )]
    fn from_search_space(&self, search: &[f64]) -> Vec<f64> {
        search.to_vec()
    }
}

/// Pull of every zone towards the base zone ("same family"): one row per parameter of each
/// later zone.
fn zone_pull_rows(
    per_zone: usize,
    zones: usize,
    params: &[f64],
    sigma: Option<f64>,
    out: &mut Vec<PriorRow>,
) {
    let Some(sigma) = sigma.filter(|s| s.is_finite() && *s > 0.0) else {
        return;
    };
    for z in 1..zones {
        for k in 0..per_zone {
            let (a, b) = (z * per_zone + k, k);
            out.push(PriorRow {
                value: (params[a] - params[b]) / sigma,
                entries: vec![(a, 1.0 / sigma), (b, -1.0 / sigma)],
                tag: PriorTag::ZonePull,
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// B: the smooth basis
// ---------------------------------------------------------------------------------------------

/// Model B: fourteen log-amplitudes per zone on [`FIT_BASIS_NM`].
///
/// The fitted absorption of a zone is `sum_k exp(theta_k) b_k(lambda)`. The renderer and the GPU
/// carry at most eight bands per material, so [`FitModel::tensors`] does not hand the fourteen
/// bands over: it refits the fitted spectrum onto the seven bands of the body-colour basis
/// (`BODY_COLOR_BASIS_NM`) by non-negative least squares on a 5 nm grid from 400 to 700 nm, and
/// the fitter reports the face-up colour difference of that refit as a
/// [`WarningKind::ExportRefit`](super::WarningKind::ExportRefit) when it is noticeable.
#[derive(Debug, Clone)]
pub struct SmoothBasisModel {
    zones: usize,
    /// The smoothness factor of the last `(sigma, length)` asked for, shared by the duplicates of
    /// the model: the 14 x 14 inverse and Cholesky are done once, not per normal pass.
    factor: Arc<Mutex<Option<CachedFactor>>>,
}

/// The smoothness factor with the two parameters (as bit patterns) it was computed for.
#[derive(Debug)]
struct CachedFactor {
    key: (u64, u64),
    factor: Option<Arc<Vec<f64>>>,
}

/// The number of bands of the body-colour basis, which is what the GPU export carries.
const EXPORT_BANDS: usize = BODY_COLOR_BASIS_NM.len();

impl SmoothBasisModel {
    /// The basis bands per zone.
    pub const BANDS: usize = FIT_BASIS_NM.len();

    /// A model with `zones` zones (at least 1).
    #[must_use]
    pub fn new(zones: usize) -> Self {
        Self {
            zones: zones.max(1),
            factor: Arc::new(Mutex::new(None)),
        }
    }

    /// [`smoothness_factor`] for the two parameters, computed once per distinct pair.
    fn cached_smoothness_factor(&self, sigma_a: f64, ell_nm: f64) -> Option<Arc<Vec<f64>>> {
        let key = (sigma_a.to_bits(), ell_nm.to_bits());
        let mut guard = self.factor.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = guard.as_ref()
            && entry.key == key
        {
            return entry.factor.clone();
        }
        let factor = smoothness_factor(sigma_a, ell_nm).map(Arc::new);
        *guard = Some(CachedFactor {
            key,
            factor: factor.clone(),
        });
        factor
    }

    /// The value of basis band `k` at `lambda_nm` (unit peak).
    #[must_use]
    pub fn basis(k: usize, lambda_nm: f64) -> f64 {
        basis_value(&FIT_BASIS_NM, k, lambda_nm)
    }
}

impl SpectralModel for SmoothBasisModel {
    fn n_params(&self) -> usize {
        self.zones * Self::BANDS
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, params: &[f64]) -> f64 {
        let block = &params[zone * Self::BANDS..(zone + 1) * Self::BANDS];
        let mut sum = 0.0;
        for (k, theta) in block.iter().enumerate() {
            sum += theta.exp() * Self::basis(k, lambda_nm);
        }
        sum
    }

    fn dalpha(&self, zone: usize, lambda_nm: f64, params: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        for k in 0..Self::BANDS {
            let at = zone * Self::BANDS + k;
            out[at] = params[at].exp() * Self::basis(k, lambda_nm);
        }
    }
}

impl FitModel for SmoothBasisModel {
    fn as_spectral(&self) -> &dyn SpectralModel {
        self
    }

    fn kind(&self) -> ModelKind {
        ModelKind::SmoothBasis
    }

    fn zone_count(&self) -> usize {
        self.zones
    }

    fn per_zone(&self) -> usize {
        Self::BANDS
    }

    fn param_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.n_params());
        for z in 0..self.zones {
            for &(centre, _) in &FIT_BASIS_NM {
                names.push(format!("zone {z} band {centre:.0} nm"));
            }
        }
        names
    }

    fn bounds(&self) -> (Vec<f64>, Vec<f64>) {
        (
            vec![BASIS_LOG_MIN; self.n_params()],
            vec![BASIS_LOG_MAX; self.n_params()],
        )
    }

    fn seed(&self, index: usize) -> Vec<f64> {
        // Four flat spectra of different strength, then four tilted ones (red-heavy and
        // blue-heavy absorbers at two strengths). A tilt is a log-linear slope per 40 nm about
        // 540 nm, so it means the same on any basis.
        const LEVELS: [f64; 4] = [0.05, 0.2, 0.01, 0.5];
        const TILTS: [f64; 4] = [0.6, -0.6, 0.3, -0.3];
        let mut block = [0.0_f64; Self::BANDS];
        for (k, value) in block.iter_mut().enumerate() {
            *value = if index % 8 < 4 {
                LEVELS[index % 4].ln()
            } else {
                SEED_LOG_LEVEL + TILTS[index % 4] * (f64::from(FIT_BASIS_NM[k].0) - 540.0) / 40.0
            };
        }
        let mut out = Vec::with_capacity(self.n_params());
        for _ in 0..self.zones {
            out.extend_from_slice(&block);
        }
        out
    }

    /// The priors of the module notes: per zone, the rows `r_i = sum_{k >= i} L_ki a_k` of the
    /// Gaussian-process smoothness prior on the amplitudes `a = exp(theta)`, the rows
    /// `sqrt(2 a_k / b)` of the exponential magnitude prior (`b = magnitude_scale`), plus the
    /// optional zone pull.
    fn prior_rows(&self, params: &[f64], ctx: &PriorContext<'_>, out: &mut Vec<PriorRow>) {
        if !ctx.model_priors {
            return;
        }
        let n = Self::BANDS;
        // `FitConfig::validate` has checked that the factor exists.
        let factor = self
            .cached_smoothness_factor(ctx.config.smoothness_sigma, ctx.config.smoothness_length_nm);
        debug_assert!(factor.is_some(), "FitConfig::validate admits this prior");
        let scale = ctx.config.magnitude_scale;
        for z in 0..self.zones {
            let base = z * n;
            let amplitude: Vec<f64> = (0..n).map(|k| params[base + k].exp()).collect();
            if let Some(l) = &factor {
                for i in 0..n {
                    let mut value = 0.0;
                    let mut entries = Vec::with_capacity(n - i);
                    for k in i..n {
                        let coefficient = l[k * n + i];
                        value += coefficient * amplitude[k];
                        entries.push((base + k, coefficient * amplitude[k]));
                    }
                    out.push(PriorRow {
                        value,
                        entries,
                        tag: PriorTag::Smoothness,
                    });
                }
            }
            for k in 0..n {
                let value = (2.0 * amplitude[k] / scale).sqrt();
                out.push(PriorRow {
                    value,
                    entries: vec![(base + k, 0.5 * value)],
                    tag: PriorTag::Magnitude,
                });
            }
        }
        zone_pull_rows(
            Self::BANDS,
            self.zones,
            params,
            ctx.config.zone_pull_sigma,
            out,
        );
    }

    fn l1_weights(&self, params: &[f64]) -> Vec<f64> {
        vec![1.0; params.len()]
    }

    /// The fitted spectra on the seven bands of the body-colour basis (the most a material can
    /// carry on the GPU): the least-squares refit of [`SmoothBasisModel`]'s notes. A refit
    /// amplitude at the lower bound of the fit (1e-6 per mm) is "no band" and is left out.
    fn tensors(&self, params: &[f64]) -> Vec<AbsorptionTensor> {
        let (theta, _) = project_onto(&BODY_COLOR_BASIS_NM, self.zones, &|zone, lambda| {
            self.alpha(zone, lambda, params)
        });
        (0..self.zones)
            .map(|z| {
                let mut amplitudes = [0.0_f32; EXPORT_BANDS];
                for (k, a) in amplitudes.iter_mut().enumerate() {
                    let t = theta[z * EXPORT_BANDS + k];
                    if t > BASIS_LOG_MIN + 1e-9 {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "band amplitude per mm is small and positive; the export bands are f32"
                        )]
                        let amplitude = t.exp() as f32;
                        *a = amplitude;
                    }
                }
                AbsorptionTensor::isotropic(body_color_bands(amplitudes))
            })
            .collect()
    }

    fn fd_step(&self) -> f64 {
        1e-3
    }

    fn duplicate(&self) -> Box<dyn FitModel> {
        Box::new(self.clone())
    }

    /// Round 3, D1.2. Two starts from a fitted vector:
    ///
    /// * (a) the wavelength-flat component of every zone's absorption removed: the absorption at
    ///   the basis centres minus its minimum over the centres, floored at [`START_FLOOR`];
    /// * (b) one band per zone, centred on the zone's most absorbed camera channel (the one of
    ///   [`CHANNEL_NM`] where the fitted absorption is largest), with a Gaussian profile of the
    ///   basis width and the fitted level there (at least [`START_LEVEL`]).
    fn structured_starts(&self, params: &[f64]) -> Vec<Vec<f64>> {
        let mut flat_removed = params.to_vec();
        let mut single_band = params.to_vec();
        for zone in 0..self.zones {
            let at_centres: Vec<f64> = FIT_BASIS_NM
                .iter()
                .map(|&(centre, _)| self.alpha(zone, f64::from(centre), params))
                .collect();
            let flat = at_centres.iter().copied().fold(f64::INFINITY, f64::min);
            for (k, a) in at_centres.iter().enumerate() {
                flat_removed[zone * Self::BANDS + k] = (a - flat)
                    .max(START_FLOOR)
                    .ln()
                    .clamp(BASIS_LOG_MIN, BASIS_LOG_MAX);
            }
            let mut channel = CHANNEL_NM[0];
            for &candidate in &CHANNEL_NM[1..] {
                if self.alpha(zone, candidate, params) > self.alpha(zone, channel, params) {
                    channel = candidate;
                }
            }
            let level = self.alpha(zone, channel, params).max(START_LEVEL);
            for (k, &(centre, width)) in FIT_BASIS_NM.iter().enumerate() {
                let t = (channel - f64::from(centre)) / f64::from(width);
                single_band[zone * Self::BANDS + k] = (level * (-0.5 * t * t).exp())
                    .max(START_FLOOR)
                    .ln()
                    .clamp(BASIS_LOG_MIN, BASIS_LOG_MAX);
            }
        }
        vec![flat_removed, single_band]
    }

    /// Model B in amplitude space ([`AmplitudeModel`]): the absorption is linear in the
    /// amplitudes, so the data term (a sum of `exp(-tau)` with `tau` linear in them) is close to
    /// quadratic there, and "zero absorption" is a point of the box instead of `-13.8` in a
    /// logarithm.
    fn search_space(&self) -> Option<Box<dyn FitModel>> {
        Some(Box::new(AmplitudeModel { zones: self.zones }))
    }

    fn to_search_space(&self, params: &[f64]) -> Vec<f64> {
        params.iter().map(|theta| theta.exp()).collect()
    }

    fn from_search_space(&self, search: &[f64]) -> Vec<f64> {
        search
            .iter()
            .map(|a| a.max(0.0).ln().clamp(BASIS_LOG_MIN, BASIS_LOG_MAX))
            .collect()
    }
}

/// Model B with the amplitudes `a_k` (per mm, `0..=30`) as its parameters, no priors: the search
/// space of the constrained colour search (see [`FitModel::search_space`]).
#[derive(Debug, Clone)]
struct AmplitudeModel {
    zones: usize,
}

impl SpectralModel for AmplitudeModel {
    fn n_params(&self) -> usize {
        self.zones * SmoothBasisModel::BANDS
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, params: &[f64]) -> f64 {
        let n = SmoothBasisModel::BANDS;
        let block = &params[zone * n..(zone + 1) * n];
        let mut sum = 0.0;
        for (k, a) in block.iter().enumerate() {
            sum += a * SmoothBasisModel::basis(k, lambda_nm);
        }
        sum
    }

    fn dalpha(&self, zone: usize, lambda_nm: f64, _params: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        for k in 0..SmoothBasisModel::BANDS {
            out[zone * SmoothBasisModel::BANDS + k] = SmoothBasisModel::basis(k, lambda_nm);
        }
    }
}

impl FitModel for AmplitudeModel {
    fn as_spectral(&self) -> &dyn SpectralModel {
        self
    }

    fn kind(&self) -> ModelKind {
        ModelKind::SmoothBasis
    }

    fn zone_count(&self) -> usize {
        self.zones
    }

    fn per_zone(&self) -> usize {
        SmoothBasisModel::BANDS
    }

    fn param_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.n_params());
        for z in 0..self.zones {
            for &(centre, _) in &FIT_BASIS_NM {
                names.push(format!("zone {z} amplitude {centre:.0} nm"));
            }
        }
        names
    }

    fn bounds(&self) -> (Vec<f64>, Vec<f64>) {
        (
            vec![0.0; self.n_params()],
            vec![AMPLITUDE_MAX; self.n_params()],
        )
    }

    fn seed(&self, _index: usize) -> Vec<f64> {
        vec![0.05; self.n_params()]
    }

    fn prior_rows(&self, _params: &[f64], _ctx: &PriorContext<'_>, _out: &mut Vec<PriorRow>) {}

    fn l1_weights(&self, params: &[f64]) -> Vec<f64> {
        vec![1.0; params.len()]
    }

    fn tensors(&self, params: &[f64]) -> Vec<AbsorptionTensor> {
        let theta: Vec<f64> = params
            .iter()
            .map(|a| a.max(0.0).ln().clamp(BASIS_LOG_MIN, BASIS_LOG_MAX))
            .collect();
        SmoothBasisModel::new(self.zones).tensors(&theta)
    }

    /// The step of the finite difference of a colour in an amplitude, per mm.
    fn fd_step(&self) -> f64 {
        2e-4
    }

    fn duplicate(&self) -> Box<dyn FitModel> {
        Box::new(self.clone())
    }
}

/// The smallest amplitude of a structured start, per mm.
const START_FLOOR: f64 = 1e-4;
/// The smallest level of the single-band start, per mm.
const START_LEVEL: f64 = 0.02;
/// Representative wavelengths of the red, green and blue camera channels, nm.
const CHANNEL_NM: [f64; 3] = [610.0, 540.0, 460.0];

impl SmoothBasisModel {
    /// The log-amplitudes of the fit basis ([`FIT_BASIS_NM`]) that best represent the absorption
    /// `alpha(zone, lambda)` (per mm) in the least-squares sense on a 5 nm grid from 400 to
    /// 700 nm, and the largest absolute misfit on that grid (per mm). The truth of a synthetic
    /// photo is not exactly in the basis, so the priors "at the truth" (round 3, D1.3) are those
    /// of this projection.
    pub(super) fn project(zones: usize, alpha: &dyn Fn(usize, f64) -> f64) -> (Vec<f64>, f64) {
        project_onto(&FIT_BASIS_NM, zones, alpha)
    }
}

/// The log-amplitudes of `basis` that best represent `alpha(zone, lambda)` per zone, in the
/// least-squares sense on a 5 nm grid from 400 to 700 nm (Levenberg-Marquardt on the logarithms
/// from the absorption at the band centres), and the largest absolute misfit on that grid.
/// `theta` is zone-major with `basis.len()` entries per zone.
fn project_onto(
    basis: &[(f32, f32)],
    zones: usize,
    alpha: &dyn Fn(usize, f64) -> f64,
) -> (Vec<f64>, f64) {
    let grid: Vec<f64> = (0..=60).map(|i| 400.0 + 5.0 * f64::from(i)).collect();
    let bands = basis.len();
    // The basis on the grid, once: `table[i * bands + k]`.
    let table: Vec<f64> = grid
        .iter()
        .flat_map(|&l| (0..bands).map(move |k| basis_value(basis, k, l)))
        .collect();
    let mut out = Vec::with_capacity(zones * bands);
    let mut worst = 0.0_f64;
    for zone in 0..zones {
        let target: Vec<f64> = grid.iter().map(|&l| alpha(zone, l)).collect();
        let model_at = |theta: &[f64]| -> Vec<f64> {
            (0..grid.len())
                .map(|i| {
                    (0..bands)
                        .map(|k| theta[k].exp() * table[i * bands + k])
                        .sum::<f64>()
                })
                .collect()
        };
        let sse = |theta: &[f64]| -> f64 {
            model_at(theta)
                .iter()
                .zip(&target)
                .map(|(m, t)| (m - t) * (m - t))
                .sum()
        };
        let mut theta: Vec<f64> = basis
            .iter()
            .map(|&(c, _)| {
                alpha(zone, f64::from(c))
                    .max(START_FLOOR)
                    .ln()
                    .clamp(BASIS_LOG_MIN, BASIS_LOG_MAX)
            })
            .collect();
        let mut cost = sse(&theta);
        let mut mu = 1e-3_f64;
        for _ in 0..80 {
            let predicted = model_at(&theta);
            let mut jtj = vec![0.0; bands * bands];
            let mut jtr = vec![0.0; bands];
            for i in 0..grid.len() {
                let r = predicted[i] - target[i];
                let row: Vec<f64> = (0..bands)
                    .map(|k| theta[k].exp() * table[i * bands + k])
                    .collect();
                for a in 0..bands {
                    jtr[a] += row[a] * r;
                    for b in 0..bands {
                        jtj[a * bands + b] += row[a] * row[b];
                    }
                }
            }
            let mut accepted = false;
            for _ in 0..12 {
                let mut a = jtj.clone();
                for d in 0..bands {
                    a[d * bands + d] += mu * jtj[d * bands + d].max(1e-12) + 1e-12;
                }
                let rhs: Vec<f64> = jtr.iter().map(|v| -v).collect();
                if let Some(delta) = solve_spd(&a, bands, &rhs) {
                    let trial: Vec<f64> = theta
                        .iter()
                        .zip(&delta)
                        .map(|(t, d)| (t + d).clamp(BASIS_LOG_MIN, BASIS_LOG_MAX))
                        .collect();
                    let trial_cost = sse(&trial);
                    if trial_cost <= cost {
                        theta = trial;
                        cost = trial_cost;
                        mu = (mu / 3.0).max(1e-9);
                        accepted = true;
                        break;
                    }
                }
                mu *= 4.0;
            }
            if !accepted {
                break;
            }
        }
        let fitted = model_at(&theta);
        for (m, t) in fitted.iter().zip(&target) {
            worst = worst.max((m - t).abs());
        }
        out.extend_from_slice(&theta);
    }
    (out, worst)
}

// ---------------------------------------------------------------------------------------------
// A: the chromophore recipe
// ---------------------------------------------------------------------------------------------

/// The central-difference partner tensors of one parameter.
struct Derivative {
    high: ZoneAbsorption,
    low: ZoneAbsorption,
    span: f64,
}

/// The resolved tensors for one parameter vector.
#[derive(Default)]
struct ResolveCache {
    key: Vec<u64>,
    base: Vec<Option<ZoneAbsorption>>,
    derivatives: Vec<Option<Vec<Derivative>>>,
}

/// Model A: log-amounts of the selectable elements of a catalogue host, per zone.
pub struct ChromophoreModel {
    catalogue: &'static ChromophoreCatalogue,
    host_id: String,
    elements: Vec<String>,
    max_amount: Vec<f64>,
    end_member: Vec<bool>,
    treatments: Vec<String>,
    zones: usize,
    cache: Mutex<ResolveCache>,
}

impl ChromophoreModel {
    /// A model for `host_id` (`"tourmaline"`, `"quartz"`, ...) with `zones` zones. `treatments`
    /// are catalogue treatment ids applied to every zone and not fitted.
    ///
    /// # Errors
    ///
    /// [`FitError::UnknownHost`] when the catalogue has no such host, and
    /// [`FitError::NoChromophores`] when it offers no fittable element.
    pub fn new(host_id: &str, treatments: &[String], zones: usize) -> Result<Self, FitError> {
        let catalogue = ChromophoreCatalogue::global();
        let host = catalogue
            .host(host_id)
            .ok_or_else(|| FitError::UnknownHost(host_id.to_owned()))?;
        let mut elements = Vec::new();
        let mut max_amount = Vec::new();
        let mut end_member = Vec::new();
        // The fit's box bounds follow the fixed treatments, every selectable element presumed
        // present; the resolver still caps the sums of multi-element chromophores.
        let ids = catalogue.selectable_elements(host_id);
        let split = host.treated_split(treatments, &|x| ids.iter().any(|i| i == x));
        for id in ids {
            let max = host.element_conc_max_split(&id, &split);
            if max.is_finite() && max > 0.0 {
                end_member.push(host.end_members.iter().any(|m| m.id == id));
                max_amount.push(max);
                elements.push(id);
            }
        }
        if elements.is_empty() {
            return Err(FitError::NoChromophores(host_id.to_owned()));
        }
        Ok(Self {
            catalogue,
            host_id: host_id.to_owned(),
            elements,
            max_amount,
            end_member,
            treatments: treatments.to_vec(),
            zones: zones.max(1),
            cache: Mutex::new(ResolveCache::default()),
        })
    }

    /// The fitted element ids of one zone, in parameter order.
    #[must_use]
    pub fn elements(&self) -> &[String] {
        &self.elements
    }

    /// The upper bound of each element's amount (the catalogue's `conc_max` under the fixed
    /// treatments), in the element's input unit.
    #[must_use]
    pub fn max_amounts(&self) -> &[f64] {
        &self.max_amount
    }

    /// The amount (input unit) that log-parameter `theta` stands for element `i`.
    #[must_use]
    pub fn amount(&self, i: usize, theta: f64) -> f64 {
        theta.exp().min(self.max_amount[i])
    }

    /// The log-parameter of `amount` of element `i`, clamped to the bounds.
    #[must_use]
    pub fn log_amount(&self, i: usize, amount: f64) -> f64 {
        let lo = (self.max_amount[i] * CHROMOPHORE_FLOOR_FRACTION).ln();
        let hi = self.max_amount[i].ln();
        amount.max(f64::MIN_POSITIVE).ln().clamp(lo, hi)
    }

    fn zone_params<'a>(&self, params: &'a [f64], zone: usize) -> &'a [f64] {
        let n = self.elements.len();
        &params[zone * n..(zone + 1) * n]
    }

    /// The recipe of one zone's log-amounts. End-member amounts are scaled to sum to at most 1
    /// (the resolver refuses more).
    fn recipe(&self, zone_params: &[f64]) -> ColorRecipe {
        let mut recipe = ColorRecipe::new(self.host_id.clone(), self.catalogue.data_version);
        let mut amounts: Vec<f64> = (0..self.elements.len())
            .map(|i| self.amount(i, zone_params[i]))
            .collect();
        let end_sum: f64 = amounts
            .iter()
            .zip(&self.end_member)
            .filter(|(_, e)| **e)
            .map(|(a, _)| *a)
            .sum();
        if end_sum > 1.0 {
            let scale = (1.0 - 1e-9) / end_sum;
            for (a, e) in amounts.iter_mut().zip(&self.end_member) {
                if *e {
                    *a *= scale;
                }
            }
        }
        for (id, amount) in self.elements.iter().zip(&amounts) {
            recipe.set_amount(id, *amount);
        }
        recipe.treatments.clone_from(&self.treatments);
        recipe
    }

    fn resolve_zone(&self, zone_params: &[f64]) -> ZoneAbsorption {
        let recipe = self.recipe(zone_params);
        match resolve(&recipe, self.catalogue) {
            Ok((tensor, _)) => ZoneAbsorption::per_mm(tensor),
            Err(_) => ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
        }
    }

    /// The cache entry for `params`, reset when the vector differs from the cached one.
    fn locked(&self, params: &[f64]) -> std::sync::MutexGuard<'_, ResolveCache> {
        let mut guard = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let same = guard.key.len() == params.len()
            && guard.key.iter().zip(params).all(|(k, p)| *k == p.to_bits());
        if !same {
            guard.key = params.iter().map(|p| p.to_bits()).collect();
            guard.base = vec![None; self.zones];
            guard.derivatives = (0..self.zones).map(|_| None).collect();
        }
        guard
    }

    fn zone_bounds(&self) -> (Vec<f64>, Vec<f64>) {
        let lo = (0..self.elements.len())
            .map(|i| (self.max_amount[i] * CHROMOPHORE_FLOOR_FRACTION).ln())
            .collect();
        let hi = (0..self.elements.len())
            .map(|i| self.max_amount[i].ln())
            .collect();
        (lo, hi)
    }
}

impl SpectralModel for ChromophoreModel {
    fn n_params(&self) -> usize {
        self.zones * self.elements.len()
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, params: &[f64]) -> f64 {
        let mut guard = self.locked(params);
        if guard.base[zone].is_none() {
            let resolved = self.resolve_zone(self.zone_params(params, zone));
            guard.base[zone] = Some(resolved);
        }
        guard.base[zone]
            .as_ref()
            .map_or(0.0, |za| za.alpha(lambda_nm, None))
    }

    fn dalpha(&self, zone: usize, lambda_nm: f64, params: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        let n = self.elements.len();
        let mut guard = self.locked(params);
        if guard.derivatives[zone].is_none() {
            let zp = self.zone_params(params, zone);
            let (lo, hi) = self.zone_bounds();
            let mut list = Vec::with_capacity(n);
            let mut moved = zp.to_vec();
            for i in 0..n {
                let up = (zp[i] + CHROMOPHORE_FD_STEP).min(hi[i]);
                let down = (zp[i] - CHROMOPHORE_FD_STEP).max(lo[i]);
                moved[i] = up;
                let high = self.resolve_zone(&moved);
                moved[i] = down;
                let low = self.resolve_zone(&moved);
                moved[i] = zp[i];
                list.push(Derivative {
                    high,
                    low,
                    span: up - down,
                });
            }
            guard.derivatives[zone] = Some(list);
        }
        if let Some(list) = &guard.derivatives[zone] {
            for (i, d) in list.iter().enumerate() {
                if d.span > 1e-12 {
                    out[zone * n + i] =
                        (d.high.alpha(lambda_nm, None) - d.low.alpha(lambda_nm, None)) / d.span;
                }
            }
        }
    }
}

/// The radical inverse of `n` in `base` (the Halton sequence).
fn radical_inverse(mut n: u64, base: u64) -> f64 {
    let (mut factor, mut result) = (1.0, 0.0);
    while n > 0 {
        factor /= base as f64;
        result += factor * (n % base) as f64;
        n /= base;
    }
    result
}

/// Primes for the Halton starts, one per element (cycled).
const HALTON_BASES: [u64; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

impl FitModel for ChromophoreModel {
    fn as_spectral(&self) -> &dyn SpectralModel {
        self
    }

    fn kind(&self) -> ModelKind {
        ModelKind::Chromophore
    }

    fn zone_count(&self) -> usize {
        self.zones
    }

    fn per_zone(&self) -> usize {
        self.elements.len()
    }

    fn param_names(&self) -> Vec<String> {
        let mut names = Vec::with_capacity(self.n_params());
        for z in 0..self.zones {
            for id in &self.elements {
                names.push(format!("zone {z} {id}"));
            }
        }
        names
    }

    fn bounds(&self) -> (Vec<f64>, Vec<f64>) {
        let (lo, hi) = self.zone_bounds();
        (
            (0..self.zones).flat_map(|_| lo.clone()).collect(),
            (0..self.zones).flat_map(|_| hi.clone()).collect(),
        )
    }

    /// The `index`-th start places every element at a Halton fraction of its log range
    /// `[ln(1e-4 max), ln(max)]`. The recipe solver
    /// (`indicatrix::optics::chromophore::solver`) seeds from a coarse log grid over
    /// `[1e-3 max, max]`; its helpers are private, so this replicates the rule with a
    /// low-discrepancy sequence, which gives the same coverage for any number of starts.
    fn seed(&self, index: usize) -> Vec<f64> {
        let (lo, hi) = self.zone_bounds();
        let mut zone = Vec::with_capacity(self.elements.len());
        for i in 0..self.elements.len() {
            let u = radical_inverse(index as u64 + 1, HALTON_BASES[i % HALTON_BASES.len()]);
            zone.push(lo[i] + u * (hi[i] - lo[i]));
        }
        (0..self.zones).flat_map(|_| zone.clone()).collect()
    }

    fn prior_rows(&self, params: &[f64], ctx: &PriorContext<'_>, out: &mut Vec<PriorRow>) {
        if !ctx.model_priors {
            return;
        }
        let l1 = ctx.config.l1_weight;
        if l1 > 0.0 && l1.is_finite() {
            for (p, &theta) in params.iter().enumerate() {
                let i = p % self.elements.len();
                let x = self.amount(i, theta) / self.max_amount[i];
                let scale = (l1 * ctx.l1_weights[p]).sqrt();
                out.push(PriorRow {
                    value: scale * x,
                    entries: vec![(p, scale * x)],
                    tag: PriorTag::L1,
                });
            }
        }
        zone_pull_rows(
            self.elements.len(),
            self.zones,
            params,
            ctx.config.zone_pull_sigma,
            out,
        );
    }

    fn l1_weights(&self, params: &[f64]) -> Vec<f64> {
        params
            .iter()
            .enumerate()
            .map(|(p, &theta)| {
                let i = p % self.elements.len();
                let x = self.amount(i, theta) / self.max_amount[i];
                1.0 / (x + 1e-3)
            })
            .collect()
    }

    fn tensors(&self, params: &[f64]) -> Vec<AbsorptionTensor> {
        (0..self.zones)
            .map(|z| self.resolve_zone(self.zone_params(params, z)).tensor)
            .collect()
    }

    fn fd_step(&self) -> f64 {
        CHROMOPHORE_FD_STEP
    }

    fn host(&self) -> Option<&str> {
        Some(&self.host_id)
    }

    fn duplicate(&self) -> Box<dyn FitModel> {
        Box::new(Self {
            catalogue: self.catalogue,
            host_id: self.host_id.clone(),
            elements: self.elements.clone(),
            max_amount: self.max_amount.clone(),
            end_member: self.end_member.clone(),
            treatments: self.treatments.clone(),
            zones: self.zones,
            cache: Mutex::new(ResolveCache::default()),
        })
    }
}

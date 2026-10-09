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
//! * [`SmoothBasisModel`] (B): per zone, the log-amplitudes of the seven body-colour basis bands
//!   (`BODY_COLOR_BASIS_NM`); the derivative is analytic.
//!
//! Parameters are zone-major: zone `z` owns `params[z * per_zone .. (z + 1) * per_zone]`.

use std::sync::{Mutex, PoisonError};

use indicatrix::optics::{
    absorption::{AbsorptionTensor, BODY_COLOR_BASIS_NM, body_color_bands},
    chromophore::{ChromophoreCatalogue, ColorRecipe, resolve},
    zoning::ZoneAbsorption,
};

use super::{FitConfig, FitError, ModelKind, linalg::solve_spd};
use crate::rough_plan::colour_fit::forward::SpectralModel;

/// The step in the log-amount of the central difference of [`ChromophoreModel`].
pub const CHROMOPHORE_FD_STEP: f64 = 0.02;

/// The smallest amount of a chromophore element, relative to its catalogue maximum. The
/// log-amount is bounded below by this, so "absent" is a tiny positive amount.
pub const CHROMOPHORE_FLOOR_FRACTION: f64 = 1e-4;

/// Bounds of a basis log-amplitude of the smooth model: 1e-6 to 30 per mm.
const BASIS_LOG_MIN: f64 = -13.815_510_557_964_274;
const BASIS_LOG_MAX: f64 = 3.401_197_381_662_155_4;
/// The weak magnitude prior of the smooth model is centred on 0.1 per mm (`ln 0.1 = -ln 10`).
const BASIS_LOG_CENTRE: f64 = -std::f64::consts::LN_10;

/// Which prior a [`PriorRow`] belongs to (round 3, D1.3: the report names the dominant term).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PriorTag {
    /// The second-difference smoothness of model B.
    Smoothness,
    /// The weak magnitude prior of model B.
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
    /// An independent copy (its own caches), for use on another thread.
    fn duplicate(&self) -> Box<dyn FitModel>;
    /// Extra deterministic starts derived from a fitted parameter vector (round 3, D1.2), each a
    /// full model parameter vector. Empty for a model that has none.
    fn structured_starts(&self, _params: &[f64]) -> Vec<Vec<f64>> {
        Vec::new()
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

/// Model B: seven log-amplitudes per zone on the body-colour basis.
#[derive(Debug, Clone)]
pub struct SmoothBasisModel {
    zones: usize,
}

impl SmoothBasisModel {
    /// The basis bands per zone.
    pub const BANDS: usize = 7;

    /// A model with `zones` zones (at least 1).
    #[must_use]
    pub fn new(zones: usize) -> Self {
        Self {
            zones: zones.max(1),
        }
    }

    /// The value of basis band `k` at `lambda_nm` (unit peak).
    #[must_use]
    pub fn basis(k: usize, lambda_nm: f64) -> f64 {
        let (centre, width) = BODY_COLOR_BASIS_NM[k];
        let t = (lambda_nm - f64::from(centre)) / f64::from(width);
        (-0.5 * t * t).exp()
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
            for &(centre, _) in &BODY_COLOR_BASIS_NM {
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
        // blue-heavy absorbers at two strengths).
        const LEVELS: [f64; 4] = [0.05, 0.2, 0.01, 0.5];
        const TILTS: [f64; 4] = [0.6, -0.6, 0.3, -0.3];
        let mut block = [0.0_f64; Self::BANDS];
        for (k, value) in block.iter_mut().enumerate() {
            *value = if index % 8 < 4 {
                LEVELS[index % 4].ln()
            } else {
                BASIS_LOG_CENTRE + TILTS[index % 4] * (k as f64 - 3.0)
            };
        }
        let mut out = Vec::with_capacity(self.n_params());
        for _ in 0..self.zones {
            out.extend_from_slice(&block);
        }
        out
    }

    fn prior_rows(&self, params: &[f64], ctx: &PriorContext<'_>, out: &mut Vec<PriorRow>) {
        let smooth = ctx.config.smoothness_sigma;
        let magnitude = ctx.config.magnitude_sigma;
        for z in 0..self.zones {
            let base = z * Self::BANDS;
            // Second-difference smoothness of the log-amplitudes.
            for k in 1..Self::BANDS - 1 {
                let (a, b, c) = (base + k - 1, base + k, base + k + 1);
                out.push(PriorRow {
                    value: (params[a] - 2.0 * params[b] + params[c]) / smooth,
                    entries: vec![(a, 1.0 / smooth), (b, -2.0 / smooth), (c, 1.0 / smooth)],
                    tag: PriorTag::Smoothness,
                });
            }
            // A weak pull of every log-amplitude towards 0.1 per mm keeps the directions the
            // three camera channels cannot see bounded.
            for k in 0..Self::BANDS {
                let a = base + k;
                out.push(PriorRow {
                    value: (params[a] - BASIS_LOG_CENTRE) / magnitude,
                    entries: vec![(a, 1.0 / magnitude)],
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

    fn tensors(&self, params: &[f64]) -> Vec<AbsorptionTensor> {
        (0..self.zones)
            .map(|z| {
                let mut amplitudes = [0.0_f32; Self::BANDS];
                for (k, a) in amplitudes.iter_mut().enumerate() {
                    *a = params[z * Self::BANDS + k].exp() as f32;
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
            let at_centres: Vec<f64> = BODY_COLOR_BASIS_NM
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
            for (k, &(centre, width)) in BODY_COLOR_BASIS_NM.iter().enumerate() {
                let t = (channel - f64::from(centre)) / f64::from(width);
                single_band[zone * Self::BANDS + k] = (level * (-0.5 * t * t).exp())
                    .max(START_FLOOR)
                    .ln()
                    .clamp(BASIS_LOG_MIN, BASIS_LOG_MAX);
            }
        }
        vec![flat_removed, single_band]
    }
}

/// The smallest amplitude of a structured start, per mm.
const START_FLOOR: f64 = 1e-4;
/// The smallest level of the single-band start, per mm.
const START_LEVEL: f64 = 0.02;
/// Representative wavelengths of the red, green and blue camera channels, nm.
const CHANNEL_NM: [f64; 3] = [610.0, 540.0, 460.0];

impl SmoothBasisModel {
    /// The log-amplitudes of the basis that best represent the absorption `alpha(zone, lambda)`
    /// (per mm) in the least-squares sense on a 5 nm grid from 400 to 700 nm, and the largest
    /// absolute misfit on that grid (per mm). The truth of a synthetic photo is not exactly in
    /// the basis, so the priors "at the truth" (round 3, D1.3) are those of this projection.
    pub(super) fn project(zones: usize, alpha: &dyn Fn(usize, f64) -> f64) -> (Vec<f64>, f64) {
        let grid: Vec<f64> = (0..=60).map(|i| 400.0 + 5.0 * f64::from(i)).collect();
        let bands = Self::BANDS;
        let mut out = Vec::with_capacity(zones * bands);
        let mut worst = 0.0_f64;
        for zone in 0..zones {
            let target: Vec<f64> = grid.iter().map(|&l| alpha(zone, l)).collect();
            let model_at = |theta: &[f64]| -> Vec<f64> {
                grid.iter()
                    .map(|&l| {
                        (0..bands)
                            .map(|k| theta[k].exp() * Self::basis(k, l))
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
            let mut theta: Vec<f64> = BODY_COLOR_BASIS_NM
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
                for (i, &l) in grid.iter().enumerate() {
                    let r = predicted[i] - target[i];
                    let row: Vec<f64> = (0..bands)
                        .map(|k| theta[k].exp() * Self::basis(k, l))
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
        for id in catalogue.selectable_elements(host_id) {
            let max = host.element_conc_max(&id);
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

    /// The upper bound of each element's amount (the catalogue's `conc_max`), in the element's
    /// input unit.
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

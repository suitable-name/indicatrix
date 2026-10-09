//! The least-squares problem: the observed photos against the stored records.
//!
//! # Residuals
//!
//! For view `v`, pixel `p` and channel `c`, with the model prediction `r_c` (camera RGB relative
//! to the empty rig, from the records) and the view's gain `g_v = exp(gamma_v)`:
//!
//! ```text
//! u = (obs_c - g_v r_c) / sigma,   sigma^2 = photo variance + (obs_c * rel_mc)^2 + floor
//! ```
//!
//! `rel_mc = sqrt(mc_variance) / mc_mean` is the relative Monte-Carlo error of the pixel's
//! records (the tracer's estimate at the reference absorption). The Monte-Carlo term is scaled by
//! the observation, not the prediction, so the weights do not move during the iteration and the
//! objective stays comparable between steps.
//!
//! The objective is `sum rho(u) + priors` with the Huber function `rho` (`rho = u^2 / 2` up to
//! `|u| = k`, then linear; `k = 2` by default). The Gauss-Newton matrix uses the Huber weights
//! `min(1, k / |u|)`.
//!
//! # Parameters
//!
//! `x = [model parameters (np), log-gains (one per view)]`.

use indicatrix::color::body_color::{delta_e_2000, xyz_to_lab};

use super::{
    FitConfig, FitError, GAIN_FIXED_SIGMA,
    models::{FitModel, PriorContext, PriorRow, PriorTag},
    output::ResidualMap,
};
use crate::rough_plan::{
    colour_fit::forward::{ForwardRecords, PixelJacobian, SpectralModel, for_each_pixel_jacobian},
    photometry::ResampledImage,
};

/// The bound on a log-gain: gains between `e^-1` and `e`.
pub(super) const GAIN_LOG_LIMIT: f64 = 1.0;

/// The photo of one view on the working grid of its records: the transmittance image of
/// lane P1, area-averaged.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedView {
    /// The rig view index.
    pub view: usize,
    /// Working pixels across.
    pub width: usize,
    /// Working pixels down.
    pub height: usize,
    /// Linear camera RGB relative to the empty rig, per working pixel.
    pub values: Vec<[f32; 3]>,
    /// The variance of each value; non-finite entries mark unusable pixels.
    pub variance: Vec<[f32; 3]>,
}

impl ObservedView {
    /// The observation of rig view `view` from a resampled transmittance image.
    #[must_use]
    pub fn from_resampled(view: usize, image: &ResampledImage) -> Self {
        Self {
            view,
            width: image.grid.width,
            height: image.grid.height,
            values: image.values.clone(),
            variance: image.variance.clone(),
        }
    }
}

/// The weights of one view, aligned with its records.
pub(super) struct ViewData {
    pub view: usize,
    pub width: usize,
    pub height: usize,
    pub obs: Vec<[f32; 3]>,
    /// `1 / sigma` per pixel and channel; 0 for a pixel or channel that is not used.
    pub inv_sigma: Vec<[f32; 3]>,
    /// Pixels with at least one used channel.
    pub used_pixels: usize,
}

/// The prior part of the objective, split by term (round 3, D1.3), so a report can name the term
/// that dominates when the fit sits above the truth.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct PriorTerms {
    /// The gain prior (common mode and deviations).
    pub gain: f64,
    /// The second-difference smoothness of model B.
    pub smoothness: f64,
    /// The weak magnitude prior of model B.
    pub magnitude: f64,
    /// The reweighted L1 prior of model A.
    pub l1: f64,
    /// The zone pull.
    pub zone_pull: f64,
}

impl PriorTerms {
    /// The sum of all terms.
    pub(super) const fn total(&self) -> f64 {
        self.gain + self.smoothness + self.magnitude + self.l1 + self.zone_pull
    }

    fn add(&mut self, tag: PriorTag, value: f64) {
        match tag {
            PriorTag::Smoothness => self.smoothness += value,
            PriorTag::Magnitude => self.magnitude += value,
            PriorTag::L1 => self.l1 += value,
            PriorTag::ZonePull => self.zone_pull += value,
        }
    }

    /// `(name, value)` pairs in a fixed order.
    pub(super) fn named(&self) -> Vec<(String, f64)> {
        vec![
            ("gain".to_owned(), self.gain),
            ("smoothness".to_owned(), self.smoothness),
            ("magnitude".to_owned(), self.magnitude),
            ("l1".to_owned(), self.l1),
            ("zone pull".to_owned(), self.zone_pull),
        ]
    }
}

/// The gradient, the Gauss-Newton matrix and the objective at a point.
pub(super) struct Normal {
    /// `n x n`, data and priors.
    pub h: Vec<f64>,
    /// `n x n`, priors only.
    pub h_prior: Vec<f64>,
    /// Gradient of the objective.
    pub g: Vec<f64>,
    /// The total objective: robust data term plus priors.
    pub cost: f64,
    /// The robust (Huber) data term alone.
    pub data_cost: f64,
    /// The prior part of `cost`, by term.
    pub prior: PriorTerms,
    /// `sum u^2` of the data.
    pub chi2: f64,
    /// The residuals in the data term.
    pub n_data: usize,
}

/// Held-out statistics of one view.
pub(super) struct ViewStats {
    pub pixels: usize,
    pub mean_obs: [f64; 3],
    /// The mean prediction at gain 1.
    pub mean_pred: [f64; 3],
    pub s_po: f64,
    pub s_pp: f64,
}

/// A model with its parameters frozen: no parameters, so the forward evaluator skips the
/// Jacobian and returns the `f64` prediction only.
struct Frozen<'a> {
    inner: &'a dyn SpectralModel,
    params: &'a [f64],
}

impl SpectralModel for Frozen<'_> {
    fn n_params(&self) -> usize {
        0
    }

    fn alpha(&self, zone: usize, lambda_nm: f64, _params: &[f64]) -> f64 {
        self.inner.alpha(zone, lambda_nm, self.params)
    }

    fn dalpha(&self, _zone: usize, _lambda_nm: f64, _params: &[f64], _out: &mut [f64]) {}
}

/// The observed photos prepared against a set of records.
pub(super) struct Problem<'a> {
    pub records: &'a ForwardRecords,
    pub views: Vec<ViewData>,
    pub config: &'a FitConfig,
}

impl<'a> Problem<'a> {
    /// Matches the observations to the records by view index and builds the weights.
    pub(super) fn new(
        records: &'a ForwardRecords,
        observed: &[ObservedView],
        config: &'a FitConfig,
    ) -> Result<Self, FitError> {
        let mut views = Vec::with_capacity(records.views.len());
        for vr in &records.views {
            let obs = observed
                .iter()
                .find(|o| o.view == vr.view)
                .ok_or(FitError::MissingObservation(vr.view))?;
            let n = vr.pixel_count();
            if obs.width != vr.grid.width
                || obs.height != vr.grid.height
                || obs.values.len() != n
                || obs.variance.len() != n
            {
                return Err(FitError::ObservationSize(vr.view));
            }
            let mut inv_sigma = vec![[0.0_f32; 3]; n];
            let mut used_pixels = 0;
            for p in 0..n {
                if !vr.is_valid(p) {
                    continue;
                }
                let mean = f64::from(vr.mc_mean[p]);
                let rel2 = (if mean > 1e-6 {
                    f64::from(vr.mc_variance[p]) / (mean * mean)
                } else {
                    0.0
                }) * config.mc_variance_scale;
                let mut any = false;
                for c in 0..3 {
                    let value = f64::from(obs.values[p][c]);
                    let photo = f64::from(obs.variance[p][c]);
                    if !(value.is_finite() && photo.is_finite()) {
                        continue;
                    }
                    let variance = (photo + value * value * rel2).max(config.variance_floor);
                    inv_sigma[p][c] = (1.0 / variance.sqrt()) as f32;
                    any = true;
                }
                if any {
                    used_pixels += 1;
                }
            }
            views.push(ViewData {
                view: vr.view,
                width: vr.grid.width,
                height: vr.grid.height,
                obs: obs.values.clone(),
                inv_sigma,
                used_pixels,
            });
        }
        Ok(Self {
            records,
            views,
            config,
        })
    }

    /// The number of views.
    pub(super) const fn view_count(&self) -> usize {
        self.views.len()
    }

    /// The pixels with at least one used channel, over all views.
    pub(super) fn used_pixels(&self) -> usize {
        self.views.iter().map(|v| v.used_pixels).sum()
    }

    /// The gradient and Gauss-Newton matrix at `x`, over the views marked `active`.
    pub(super) fn normal(
        &self,
        model: &dyn FitModel,
        x: &[f64],
        active: &[bool],
        ctx: &PriorContext<'_>,
    ) -> Result<Normal, FitError> {
        let np = model.n_params();
        let n = x.len();
        let params = &x[..np];
        let gains: Vec<f64> = x[np..].iter().map(|g| g.exp()).collect();
        let k = self.config.huber_k;
        let mut h = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        let (mut data_cost, mut chi2, mut count) = (0.0, 0.0, 0_usize);
        let mut row = vec![0.0; np];
        for_each_pixel_jacobian(
            self.records,
            params,
            model.as_spectral(),
            &mut |px: PixelJacobian<'_>| {
                let slot = px.view_slot;
                if !active[slot] {
                    return;
                }
                let view = &self.views[slot];
                let s = view.inv_sigma[px.pixel];
                let o = view.obs[px.pixel];
                let gain = gains[slot];
                let q = np + slot;
                for c in 0..3 {
                    let sc = f64::from(s[c]);
                    if sc == 0.0 {
                        continue;
                    }
                    let u = sc * (f64::from(o[c]) - gain * px.rgb[c]);
                    let a = u.abs();
                    let w = if a <= k { 1.0 } else { k / a };
                    data_cost += if a <= k {
                        0.5 * u * u
                    } else {
                        k * a - 0.5 * k * k
                    };
                    chi2 += u * u;
                    count += 1;
                    let factor = -sc * gain;
                    let jac = &px.jacobian[c * np..(c + 1) * np];
                    for j in 0..np {
                        row[j] = factor * jac[j];
                    }
                    let row_gain = factor * px.rgb[c];
                    for j in 0..np {
                        let wj = w * row[j];
                        g[j] += wj * u;
                        for l in j..np {
                            h[j * n + l] += wj * row[l];
                        }
                        h[j * n + q] += wj * row_gain;
                    }
                    g[q] += w * u * row_gain;
                    h[q * n + q] += w * row_gain * row_gain;
                }
            },
        )?;
        // Mirror the upper triangle.
        for j in 0..n {
            for l in (j + 1)..n {
                h[l * n + j] = h[j * n + l];
            }
        }
        let mut h_prior = vec![0.0; n * n];
        let mut prior = PriorTerms::default();
        self.add_gain_prior(
            x,
            np,
            active,
            ctx.pin_common_gain,
            &mut h_prior,
            &mut g,
            &mut prior.gain,
        );
        let mut rows: Vec<PriorRow> = Vec::new();
        model.prior_rows(params, ctx, &mut rows);
        for r in &rows {
            prior.add(r.tag, 0.5 * r.value * r.value);
            for &(i, vi) in &r.entries {
                g[i] += r.value * vi;
                for &(j, vj) in &r.entries {
                    h_prior[i * n + j] += vi * vj;
                }
            }
        }
        for i in 0..n * n {
            h[i] += h_prior[i];
        }
        Ok(Normal {
            h,
            h_prior,
            g,
            cost: data_cost + prior.total(),
            data_cost,
            prior,
            chi2,
            n_data: count,
        })
    }

    /// The prior terms alone at `params` and `log_gains` (all views active, the config's own gain
    /// prior), for a report of the objective at a given point.
    pub(super) fn prior_terms_at(
        &self,
        model: &dyn FitModel,
        params: &[f64],
        log_gains: &[f64],
        ctx: &PriorContext<'_>,
    ) -> PriorTerms {
        let np = params.len();
        let n = np + log_gains.len();
        let mut x = params.to_vec();
        x.extend_from_slice(log_gains);
        let mut terms = PriorTerms::default();
        let active = vec![true; self.view_count()];
        let mut h = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        self.add_gain_prior(
            &x,
            np,
            &active,
            ctx.pin_common_gain,
            &mut h,
            &mut g,
            &mut terms.gain,
        );
        let mut rows: Vec<PriorRow> = Vec::new();
        model.prior_rows(params, ctx, &mut rows);
        for r in &rows {
            terms.add(r.tag, 0.5 * r.value * r.value);
        }
        terms
    }

    /// The plain chi-square of the data term, `sum u^2`, at `x` over all views: the evaluation of
    /// a candidate without any derivative (the metamer verification, round 3, D3).
    pub(super) fn data_chi2(&self, model: &dyn FitModel, x: &[f64]) -> Result<f64, FitError> {
        let np = model.n_params();
        let frozen = Frozen {
            inner: model.as_spectral(),
            params: &x[..np],
        };
        let gains: Vec<f64> = x[np..].iter().map(|g| g.exp()).collect();
        let mut chi2 = 0.0;
        for_each_pixel_jacobian(self.records, &[], &frozen, &mut |px: PixelJacobian<'_>| {
            let view = &self.views[px.view_slot];
            let s = view.inv_sigma[px.pixel];
            let o = view.obs[px.pixel];
            let gain = gains[px.view_slot];
            for c in 0..3 {
                let sc = f64::from(s[c]);
                if sc != 0.0 {
                    let u = sc * (f64::from(o[c]) - gain * px.rgb[c]);
                    chi2 += u * u;
                }
            }
        })?;
        Ok(chi2)
    }

    /// The sigma of the common mode of the log-gains: [`FitConfig::gain_mean_sigma`], or the
    /// numerical pin [`GAIN_FIXED_SIGMA`] when [`FitConfig::fix_common_gain`] is set or `pin`
    /// asks for it (stage 2 of the staged fit).
    const fn common_gain_sigma(&self, pin: bool) -> f64 {
        if pin || self.config.fix_common_gain {
            GAIN_FIXED_SIGMA
        } else {
            self.config.gain_mean_sigma
        }
    }

    /// Adds the prior of the log-gains `x[np..]` to the Hessian `h`, the gradient `g` and the
    /// cost.
    ///
    /// The gains of the `active` views split into a common mode `m = mean(gamma)` with the prior
    /// `N(0, sigma_m^2)` and the deviations `gamma_v - m` with `N(0, sigma_d^2)` each (round 2,
    /// C1): `0.5 (m^2 / sigma_m^2 + sum (gamma_v - m)^2 / sigma_d^2)`. The common gain is
    /// degenerate with a wavelength-flat absorption whenever the path length hardly varies across
    /// the pixels, and P1 normalises every photo to its own white frame, so its common mode is 1
    /// by construction. A view that is not active (held out) only keeps `N(0, sigma_d^2)` so the
    /// matrix stays regular.
    fn add_gain_prior(
        &self,
        x: &[f64],
        np: usize,
        active: &[bool],
        pin_common: bool,
        h: &mut [f64],
        g: &mut [f64],
        cost: &mut f64,
    ) {
        let n = x.len();
        let sd2 = 1.0 / (self.config.gain_sigma * self.config.gain_sigma);
        let sm = self.common_gain_sigma(pin_common);
        let sm2 = 1.0 / (sm * sm);
        let slots: Vec<usize> = (0..self.view_count()).filter(|&s| active[s]).collect();
        for slot in 0..self.view_count() {
            if !active[slot] {
                let q = np + slot;
                h[q * n + q] += sd2;
                g[q] += x[q] * sd2;
                *cost += 0.5 * x[q] * x[q] * sd2;
            }
        }
        if slots.is_empty() {
            return;
        }
        let count = slots.len() as f64;
        let mean = slots.iter().map(|&s| x[np + s]).sum::<f64>() / count;
        *cost += 0.5 * mean * mean * sm2;
        for &s in &slots {
            let dev = x[np + s] - mean;
            *cost += 0.5 * dev * dev * sd2;
            g[np + s] += dev * sd2 + mean * sm2 / count;
        }
        let common = sm2 / (count * count) - sd2 / count;
        for &s in &slots {
            for &t in &slots {
                let diagonal = if s == t { sd2 } else { 0.0 };
                h[(np + s) * n + np + t] += diagonal + common;
            }
        }
    }

    /// The Hessian of the gain prior alone at `n = np + views` parameters, all views active: the
    /// calibration knowledge about the gains that the metamer spread keeps (the model priors it
    /// leaves out).
    pub(super) fn gain_prior_hessian(&self, np: usize) -> Vec<f64> {
        let n = np + self.view_count();
        let mut h = vec![0.0; n * n];
        let mut g = vec![0.0; n];
        let mut cost = 0.0;
        let active = vec![true; self.view_count()];
        self.add_gain_prior(&vec![0.0; n], np, &active, false, &mut h, &mut g, &mut cost);
        h
    }

    /// The held-out statistics of the view in `slot` for the model parameters `params`.
    pub(super) fn view_stats(
        &self,
        model: &dyn FitModel,
        params: &[f64],
        slot: usize,
    ) -> Result<ViewStats, FitError> {
        let frozen = Frozen {
            inner: model.as_spectral(),
            params,
        };
        let mut stats = ViewStats {
            pixels: 0,
            mean_obs: [0.0; 3],
            mean_pred: [0.0; 3],
            s_po: 0.0,
            s_pp: 0.0,
        };
        let view = &self.views[slot];
        for_each_pixel_jacobian(self.records, &[], &frozen, &mut |px: PixelJacobian<'_>| {
            if px.view_slot != slot {
                return;
            }
            let s = view.inv_sigma[px.pixel];
            if s.contains(&0.0) {
                return;
            }
            let o = view.obs[px.pixel];
            stats.pixels += 1;
            for c in 0..3 {
                let sc2 = f64::from(s[c]) * f64::from(s[c]);
                stats.mean_obs[c] += f64::from(o[c]);
                stats.mean_pred[c] += px.rgb[c];
                stats.s_po += sc2 * px.rgb[c] * f64::from(o[c]);
                stats.s_pp += sc2 * px.rgb[c] * px.rgb[c];
            }
        })?;
        if stats.pixels > 0 {
            for c in 0..3 {
                stats.mean_obs[c] /= stats.pixels as f64;
                stats.mean_pred[c] /= stats.pixels as f64;
            }
        }
        Ok(stats)
    }

    /// The whitened residual of every pixel, `None` where the pixel is not used.
    pub(super) fn whitened_all(
        &self,
        model: &dyn FitModel,
        x: &[f64],
    ) -> Result<Vec<Vec<Option<[f64; 3]>>>, FitError> {
        let np = model.n_params();
        let frozen = Frozen {
            inner: model.as_spectral(),
            params: &x[..np],
        };
        self.whitened_with(&frozen, &x[np..])
    }

    /// The whitened residual of every pixel for a spectral model without free parameters and the
    /// log-gains `log_gains` (one per view, in the order of the records).
    pub(super) fn whitened_with(
        &self,
        spectral: &dyn SpectralModel,
        log_gains: &[f64],
    ) -> Result<Vec<Vec<Option<[f64; 3]>>>, FitError> {
        let gains: Vec<f64> = log_gains.iter().map(|g| g.exp()).collect();
        let mut out: Vec<Vec<Option<[f64; 3]>>> = self
            .views
            .iter()
            .map(|v| vec![None; v.width * v.height])
            .collect();
        for_each_pixel_jacobian(self.records, &[], spectral, &mut |px: PixelJacobian<'_>| {
            let view = &self.views[px.view_slot];
            let s = view.inv_sigma[px.pixel];
            if s.iter().all(|v| *v == 0.0) {
                return;
            }
            let o = view.obs[px.pixel];
            let gain = gains[px.view_slot];
            let mut u = [0.0; 3];
            for c in 0..3 {
                u[c] = f64::from(s[c]) * (f64::from(o[c]) - gain * px.rgb[c]);
            }
            out[px.view_slot][px.pixel] = Some(u);
        })?;
        Ok(out)
    }

    /// The residual maps at `x` (without the structure scores, see `diagnostics`).
    pub(super) fn residual_maps(
        &self,
        model: &dyn FitModel,
        x: &[f64],
    ) -> Result<Vec<ResidualMap>, FitError> {
        let all = self.whitened_all(model, x)?;
        Ok(self
            .views
            .iter()
            .zip(all)
            .map(|(view, pixels)| ResidualMap {
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
            })
            .collect())
    }
}

/// CIELAB of a linear camera RGB read as linear sRGB, relative to the white `(1, 1, 1)`: the
/// colour of a transmittance image, for the cross-validation and its reports.
pub(super) fn rgb_to_lab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|v| v.max(0.0));
    let xyz = [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b,
        0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b,
    ];
    xyz_to_lab(xyz, [0.950_47, 1.0, 1.088_83])
}

/// CIEDE2000 between two linear camera RGB colours read as linear sRGB.
pub(super) fn rgb_delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    delta_e_2000(rgb_to_lab(a), rgb_to_lab(b))
}

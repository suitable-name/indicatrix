//! Evaluating candidate absorptions on the stored records, with analytic derivatives.
//!
//! For view `v`, pixel `p` and colour channel `c`,
//!
//! ```text
//! rgb_c = sum_pt w_c(pt) * sum_r weight_r * exp(-sum_z alpha_z(lambda_pt) * lengths_rz)
//! ```
//!
//! over the evaluation points `pt` of the spectral setup and the records `r` of the point's bin.
//! The weights `w_c` fold the camera sensitivity, the backlight spectrum and the wavelength step
//! and sum to 1 per channel, so the result is the camera RGB of the stone relative to the empty
//! backlit rig, which is what the transmittance images hold.
//!
//! The derivative with respect to a model parameter `j` is
//! `-sum_pt w_c(pt) sum_z (sum_r e_r lengths_rz) dalpha_z(lambda_pt)/dp_j` with
//! `e_r = weight_r exp(-tau_r)`: one pass over the records collects the per-zone sums, then
//! the parameters are contracted once per pixel.

use std::sync::atomic::AtomicBool;

use super::{
    ForwardError,
    parallel::run_indexed,
    records::{ForwardRecords, LENGTHS, ViewRecords},
};
use crate::rough_plan::photometry::WorkingGrid;

/// A spectral absorption model with parameters: what the solver fits.
///
/// Zones are in length-array order (0 is the base zone). Wavelengths are in nm, absorption in
/// 1/mm.
pub trait SpectralModel: Sync {
    /// The number of parameters.
    fn n_params(&self) -> usize;

    /// The absorption coefficient of `zone` at `lambda_nm` for `params` (`n_params()` values).
    fn alpha(&self, zone: usize, lambda_nm: f64, params: &[f64]) -> f64;

    /// The derivative of [`alpha`](Self::alpha) with respect to every parameter. `out` has
    /// `n_params()` entries and must be fully overwritten (zero for parameters the zone does not
    /// depend on).
    fn dalpha(&self, zone: usize, lambda_nm: f64, params: &[f64], out: &mut [f64]);
}

/// The predicted camera RGB of one view on its working grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewPrediction {
    /// The rig view index.
    pub view: usize,
    /// The working grid.
    pub grid: WorkingGrid,
    /// Linear camera RGB relative to the empty rig, per working pixel; zero where invalid.
    pub rgb: Vec<[f32; 3]>,
    /// Whether the pixel has records (not masked, not dropped).
    pub valid: Vec<bool>,
}

/// The derivatives of one view's prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewJacobian {
    /// The rig view index.
    pub view: usize,
    /// Parameters per derivative row.
    pub n_params: usize,
    /// `data[(pixel * 3 + channel) * n_params + param]`; zero for invalid pixels.
    pub data: Vec<f32>,
}

/// Prediction and derivatives for all views.
#[derive(Debug, Clone, PartialEq)]
pub struct JacobianEvaluation {
    /// As [`evaluate`].
    pub predictions: Vec<ViewPrediction>,
    /// One per view, in the order of `predictions`.
    pub jacobians: Vec<ViewJacobian>,
}

/// One pixel as seen by [`for_each_pixel_jacobian`].
#[derive(Debug, Clone, Copy)]
pub struct PixelJacobian<'a> {
    /// The index of the view in the records.
    pub view_slot: usize,
    /// The rig view index.
    pub view: usize,
    /// The working pixel index in the view's grid.
    pub pixel: usize,
    /// The predicted camera RGB.
    pub rgb: [f64; 3],
    /// `jacobian[channel * n_params + param]`.
    pub jacobian: &'a [f64],
    /// The Monte-Carlo variance of the pixel's mean transmittance (see
    /// [`ViewRecords::mc_variance`]).
    pub mc_variance: f32,
    /// The pixel's status bits.
    pub status: u8,
}

/// The tables of one evaluation: `alpha[pt * nz + z]`, `dalpha[(pt * nz + z) * n_params + j]`.
struct Tables {
    alpha: Vec<f64>,
    dalpha: Vec<f64>,
    n_params: usize,
}

struct Scratch {
    grad: Vec<f64>,
    jac: Vec<f64>,
}

impl Scratch {
    fn new(records: &ForwardRecords, tables: &Tables) -> Self {
        Self {
            grad: vec![0.0; records.spectral.eval_points.len() * records.n_zones],
            jac: vec![0.0; 3 * tables.n_params],
        }
    }
}

fn eval_pixel(
    records: &ForwardRecords,
    view: &ViewRecords,
    p: usize,
    tables: &Tables,
    scratch: &mut Scratch,
) -> [f64; 3] {
    let nz = records.n_zones;
    let points = &records.spectral.eval_points;
    let traced = records.spectral.traced_bins;
    let np = tables.n_params;
    let with_jac = np > 0;
    if with_jac {
        scratch.grad.fill(0.0);
    }
    let mut rgb = [0.0_f64; 3];
    for (k, point) in points.iter().enumerate() {
        let alpha = &tables.alpha[k * nz..(k + 1) * nz];
        let mut sum = 0.0_f64;
        for record in view.pixel_records(p, point.bin, traced) {
            let mut tau = 0.0_f64;
            for (&a, &length) in alpha.iter().zip(&record.lengths[..nz]) {
                tau = a.mul_add(f64::from(length), tau);
            }
            let e = f64::from(record.weight) * (-tau).exp();
            sum += e;
            if with_jac {
                for z in 0..nz {
                    scratch.grad[k * nz + z] =
                        e.mul_add(f64::from(record.lengths[z]), scratch.grad[k * nz + z]);
                }
            }
        }
        for (channel, &weight) in rgb.iter_mut().zip(&point.weight) {
            *channel = weight.mul_add(sum, *channel);
        }
    }
    if with_jac {
        scratch.jac.fill(0.0);
        for (k, point) in points.iter().enumerate() {
            for z in 0..nz {
                let g = scratch.grad[k * nz + z];
                if g == 0.0 {
                    continue;
                }
                let d = &tables.dalpha[(k * nz + z) * np..(k * nz + z + 1) * np];
                for c in 0..3 {
                    let factor = -point.weight[c] * g;
                    for (j, &dj) in d.iter().enumerate() {
                        scratch.jac[c * np + j] = factor.mul_add(dj, scratch.jac[c * np + j]);
                    }
                }
            }
        }
    }
    rgb
}

fn plain_tables(records: &ForwardRecords, alpha: &dyn Fn(usize, f64) -> f64) -> Tables {
    let nz = records.n_zones.min(LENGTHS);
    let mut table = Vec::with_capacity(records.spectral.eval_points.len() * nz);
    for point in &records.spectral.eval_points {
        for z in 0..nz {
            table.push(alpha(z, point.lambda_nm));
        }
    }
    Tables {
        alpha: table,
        dalpha: Vec::new(),
        n_params: 0,
    }
}

fn model_tables(
    records: &ForwardRecords,
    params: &[f64],
    model: &dyn SpectralModel,
) -> Result<Tables, ForwardError> {
    let np = model.n_params();
    if params.len() != np {
        return Err(ForwardError::BadOptions(
            "the parameter vector does not match the model",
        ));
    }
    let nz = records.n_zones.min(LENGTHS);
    let points = &records.spectral.eval_points;
    let mut alpha = Vec::with_capacity(points.len() * nz);
    let mut dalpha = vec![0.0; points.len() * nz * np];
    for (k, point) in points.iter().enumerate() {
        for z in 0..nz {
            alpha.push(model.alpha(z, point.lambda_nm, params));
            let at = (k * nz + z) * np;
            model.dalpha(z, point.lambda_nm, params, &mut dalpha[at..at + np]);
        }
    }
    Ok(Tables {
        alpha,
        dalpha,
        n_params: np,
    })
}

/// Pixels per parallel task.
const CHUNK: usize = 256;

fn run_view(
    records: &ForwardRecords,
    slot: usize,
    tables: &Tables,
) -> (ViewPrediction, Option<ViewJacobian>) {
    let view = &records.views[slot];
    let n = view.pixel_count();
    let np = tables.n_params;
    let chunks = n.div_ceil(CHUNK);
    let cancel = AtomicBool::new(false);
    let work = |chunk: usize| {
        let (first, last) = (chunk * CHUNK, ((chunk + 1) * CHUNK).min(n));
        let mut scratch = Scratch::new(records, tables);
        let mut rgb = Vec::with_capacity(last - first);
        let mut jac: Vec<f32> = Vec::with_capacity((last - first) * 3 * np);
        for p in first..last {
            if view.is_valid(p) {
                let value = eval_pixel(records, view, p, tables, &mut scratch);
                rgb.push([value[0] as f32, value[1] as f32, value[2] as f32]);
                jac.extend(scratch.jac.iter().map(|&x| x as f32));
            } else {
                rgb.push([0.0; 3]);
                jac.extend(std::iter::repeat_n(0.0_f32, 3 * np));
            }
        }
        (rgb, jac)
    };
    let parts = run_indexed(chunks, 0, &cancel, &work, &mut |_| {});
    let mut rgb = Vec::with_capacity(n);
    let mut data: Vec<f32> = Vec::with_capacity(n * 3 * np);
    for part in parts.into_iter().flatten() {
        rgb.extend(part.0);
        data.extend(part.1);
    }
    let valid = (0..n).map(|p| view.is_valid(p)).collect();
    let prediction = ViewPrediction {
        view: view.view,
        grid: view.grid,
        rgb,
        valid,
    };
    let jacobian = (np > 0).then_some(ViewJacobian {
        view: view.view,
        n_params: np,
        data,
    });
    (prediction, jacobian)
}

/// The camera RGB of every view for the absorption `alpha(zone, lambda_nm)` in 1/mm (zone 0 is
/// the base zone), computed on the stored records without tracing.
#[must_use]
pub fn evaluate(
    records: &ForwardRecords,
    alpha: &dyn Fn(usize, f64) -> f64,
) -> Vec<ViewPrediction> {
    let tables = plain_tables(records, alpha);
    (0..records.views.len())
        .map(|slot| run_view(records, slot, &tables).0)
        .collect()
}

/// The prediction and its derivatives with respect to the parameters of `model`, for every view.
///
/// The Jacobian is materialised in `f32`: `pixels * 3 * n_params * 4` bytes per view. Use
/// [`for_each_pixel_jacobian`] to accumulate normal equations without storing it.
///
/// # Errors
///
/// [`ForwardError::BadOptions`] when `params` does not have `model.n_params()` entries.
pub fn evaluate_with_jacobian(
    records: &ForwardRecords,
    params: &[f64],
    model: &dyn SpectralModel,
) -> Result<JacobianEvaluation, ForwardError> {
    let tables = model_tables(records, params, model)?;
    let mut predictions = Vec::with_capacity(records.views.len());
    let mut jacobians = Vec::with_capacity(records.views.len());
    for slot in 0..records.views.len() {
        let (prediction, jacobian) = run_view(records, slot, &tables);
        predictions.push(prediction);
        if let Some(jacobian) = jacobian {
            jacobians.push(jacobian);
        }
    }
    Ok(JacobianEvaluation {
        predictions,
        jacobians,
    })
}

/// Calls `visit` for every valid pixel of every view, in view then pixel order, with the
/// prediction and its derivatives in `f64`. Serial and allocation-free per pixel.
///
/// # Errors
///
/// [`ForwardError::BadOptions`] when `params` does not have `model.n_params()` entries.
pub fn for_each_pixel_jacobian(
    records: &ForwardRecords,
    params: &[f64],
    model: &dyn SpectralModel,
    visit: &mut dyn FnMut(PixelJacobian<'_>),
) -> Result<(), ForwardError> {
    let tables = model_tables(records, params, model)?;
    let mut scratch = Scratch::new(records, &tables);
    for (slot, view) in records.views.iter().enumerate() {
        for p in 0..view.pixel_count() {
            if !view.is_valid(p) {
                continue;
            }
            let rgb = eval_pixel(records, view, p, &tables, &mut scratch);
            visit(PixelJacobian {
                view_slot: slot,
                view: view.view,
                pixel: p,
                rgb,
                jacobian: &scratch.jac,
                mc_variance: view.mc_variance[p],
                status: view.status[p],
            });
        }
    }
    Ok(())
}

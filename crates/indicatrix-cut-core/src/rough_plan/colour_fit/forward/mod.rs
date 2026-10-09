//! The forward path tracer of the rig (plan 2026-10-09, sections 3 and 5).
//!
//! For every working pixel of every view it traces paths through the scanned mesh once and
//! stores them as absorption-free records, so any candidate absorption is evaluated afterwards
//! as a sum of exponentials ([`evaluate`], [`evaluate_with_jacobian`]) without tracing again.
//!
//! # Pipeline
//!
//! 1. [`trace_rig`] takes a [`ForwardInput`]: the mesh and its alignment in the rig, the
//!    [`ColourRig`](super::ColourRig) (cameras, indices, [`RigLighting`]), the [`SurfaceMap`]
//!    (polished or frosted per triangle), the [`StoneIndex`], optional zones, inclusion shells,
//!    the camera response and backlight spectrum (lane P2), and per view a [`ViewTraceInput`]
//!    (the working grid of lane P1 and its pixel mask).
//! 2. Per pixel, `samples` camera rays over the pixel's footprint x `bins` wavelength bins are
//!    traced (low-discrepancy positions, counter-based random numbers, see `rng`). Adaptive
//!    passes add samples to pixels whose Monte-Carlo error exceeds the target.
//! 3. The result is compressed per (pixel, bin) to at most `max_records` records and stored in
//!    [`ForwardRecords`]; optionally in a disk cache keyed by everything the records depend on.
//!
//! # Conventions
//!
//! - The mesh frame is millimetres, as in `locate`; zone geometry is in the mesh frame.
//! - A pixel's prediction is the camera RGB of the stone **relative to the empty backlit rig**
//!   (the transmittance images of lane P1): the light at an exit is the white-frame level there
//!   divided by the level at the pixel the camera ray started from.
//! - Paths whose throughput is lost (depth limit, invalid microfacet sample, damaged mesh) are
//!   not renormalised; their share is in [`PixelLoss`]. Paths removed because they pass an
//!   inclusion are renormalised away, and the pixel is dropped above `inclusion_limit`.
//! - Birefringence is ignored (ordinary index); radiance scaling by `n^2` at the interfaces is
//!   omitted because paths start and end in the same medium.
//!
//! # Memory
//!
//! Per view: `pixels * (1 + 2 + 4 + 4 + 16 + 4 * traced_bins)` bytes of per-pixel data plus 24
//! bytes per record (`[f32; 5]` lengths and an `f32` weight). The number of records is at most
//! `pixels * traced_bins * max_records`; polished pixels need 1 to 3 records per bin, frosted
//! ones use the budget. For 8 views of 60 000 pixels, 8 bins, 16 records, that is at most
//! 1.5 GB (worst case, all pixels frosted and wide); a polished rough or immersion stays below
//! 100 MB, and `traced_bins = 1` (no dispersion) divides everything by 8. Records are kept per
//! view, so a caller short of memory traces a subset of views per call and keeps what it needs.
//!
//! # Cost estimate (not measured)
//!
//! About `views * pixels * samples * traced_bins` paths, each 2 to 6 BVH queries (about 0.3 to
//! 1 microsecond each on a mesh of some 10 000 triangles) plus the Fresnel and microfacet
//! arithmetic: 8 views x 60 000 pixels x 256 samples x 8 bins is 1e9 paths, about 15 to 45
//! core-minutes, i.e. 1 to 3 minutes on 16 cores for a frosted rough; polished or with a constant
//! index (`traced_bins = 1`) 8 times less. Compression adds roughly 10 to 30 percent.

pub mod cache;
mod compress;
mod evaluate;
mod lighting;
mod parallel;
mod records;
mod rng;
mod surface;
mod trace;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_more;

use std::{fmt, path::Path, sync::atomic::AtomicBool};

use indicatrix::optics::{
    dispersion::DispersionModel,
    materials::GemMaterial,
    zoning::{MAX_ZONES, ZoneKernel, ZonedAbsorption, ZoningError},
};

pub use evaluate::{
    JacobianEvaluation, PixelJacobian, SpectralModel, ViewJacobian, ViewPrediction, evaluate,
    evaluate_with_jacobian, for_each_pixel_jacobian,
};
pub use lighting::{ExitLight, LightModel, PanelGeom, RigLighting, WhiteFrame};
pub use records::{
    CacheStatus, EvalPoint, ForwardRecords, ForwardStats, LENGTHS, PathRecord, PixelLoss,
    SpectralSetup, ViewRecords, status,
};
pub use surface::{MIN_ROUGHNESS, SurfaceClass, SurfaceMap};

use super::ColourRig;
use crate::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse, GRID_FIRST_NM, GRID_LEN, GRID_STEP_NM},
    locate::{InclusionShell, RigError, Rigid, Scene},
    photometry::{PixelMask, WorkingGrid, flag},
    shape::{MeshError, RoughMesh},
};
use parallel::{run_indexed, thread_count};
use trace::{PixelScratch, TraceCtx, trace_working_pixel};

/// Why a trace could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardError {
    /// The cancel flag was raised.
    Cancelled,
    /// No view was asked for.
    NoViews,
    /// The rig profile is unusable.
    BadRig(RigError),
    /// This view (index into the request) does not exist in the rig, or its grid or mask does
    /// not fit.
    BadView(usize),
    /// An option is out of range.
    BadOptions(&'static str),
    /// The zones do not validate.
    BadZoning(ZoningError),
    /// This inclusion shell (0-based) is not a closed mesh.
    BadInclusion(usize, MeshError),
    /// The light model is unusable.
    BadLighting(String),
    /// The camera response or backlight has no signal in the wavelength range.
    BadSpectrum(String),
    /// A photometry error while building a white frame.
    Photometry(String),
}

impl fmt::Display for ForwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "the trace was cancelled"),
            Self::NoViews => write!(f, "no view to trace"),
            Self::BadRig(e) => write!(f, "{e}"),
            Self::BadView(i) => write!(
                f,
                "view {} of the request does not fit the rig, its grid or its mask",
                i + 1
            ),
            Self::BadOptions(what) => write!(f, "invalid option: {what}"),
            Self::BadZoning(e) => write!(f, "{e}"),
            Self::BadInclusion(i, e) => write!(f, "inclusion {}: {e}", i + 1),
            Self::BadLighting(what) | Self::BadSpectrum(what) | Self::Photometry(what) => {
                write!(f, "{what}")
            }
        }
    }
}

impl std::error::Error for ForwardError {}

/// The refractive index of the stone as a function of wavelength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StoneIndex {
    /// The rig profile's stone index at every wavelength (no dispersion): the paths do not
    /// depend on the wavelength, so one set of records serves all bins.
    Rig,
    /// This constant index (the `n_d` option, for previews).
    Constant(f64),
    /// A dispersion curve, traced at every wavelength bin: the rough's chosen host material
    /// (see [`StoneIndex::from_material`]). The ordinary index of a birefringent host.
    Dispersion(DispersionModel),
}

impl StoneIndex {
    /// The dispersion of `material`.
    #[must_use]
    pub const fn from_material(material: &GemMaterial) -> Self {
        Self::Dispersion(material.dispersion)
    }

    /// Whether the paths depend on the wavelength.
    #[must_use]
    pub const fn is_dispersive(&self) -> bool {
        matches!(self, Self::Dispersion(_))
    }

    /// The index at `lambda_nm`; `rig_n` is the profile's stone index.
    #[must_use]
    pub fn n_at(&self, rig_n: f64, lambda_nm: f64) -> f64 {
        match self {
            Self::Rig => rig_n,
            Self::Constant(n) => *n,
            Self::Dispersion(model) => f64::from(model.evaluate(lambda_nm as f32)).max(1.0),
        }
    }
}

/// Settings of a trace. The fields that change the records are part of the cache key.
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardOptions {
    /// Camera-ray samples per pixel in the first pass (default 256).
    pub samples: usize,
    /// Wavelength bins (default 8).
    pub bins: usize,
    /// The wavelength range the bins cover, nm (default 400 to 700).
    pub lambda_range_nm: [f64; 2],
    /// Wavelengths per bin where the absorption is evaluated (default 2): the paths are traced
    /// per bin, the spectral integral is finer.
    pub spectral_sub: usize,
    /// The record budget per (pixel, bin) (default 16, at most 255).
    pub max_records: usize,
    /// Surface events per path (default 16).
    pub max_depth: usize,
    /// Adaptive sampling: a pixel gets up to this many times `samples` (default 4; 1 turns it
    /// off).
    pub max_sample_factor: usize,
    /// The relative standard error a pixel must reach (default 0.01); per-pixel targets in
    /// [`ViewTraceInput`] override it.
    pub target_rel_error: f64,
    /// A pixel whose standard error is below this absolute value is converged (default 1e-4).
    pub abs_error: f64,
    /// The absorption (1/mm) at which the Monte-Carlo error is judged, and which sets the
    /// compression tolerance `0.1 / alpha` mm (default 0.3).
    pub reference_alpha_per_mm: f64,
    /// A pixel is dropped when more than this share of its exit light is the Lambertian mean
    /// (default 0.05).
    pub flagged_limit: f64,
    /// A pixel is dropped when more than this share of its paths ran through an inclusion
    /// (default 0.3).
    pub inclusion_limit: f64,
    /// Mask flags that exclude a pixel from tracing (default: outside the outline, inclusion,
    /// ghost, user, saturated, below noise).
    pub skip_flags: u8,
    /// The random seed (default 0).
    pub seed: u64,
    /// Worker threads; 0 means all cores. Does not change the result.
    pub threads: usize,
    /// Pixels per scheduling chunk. Does not change the result.
    pub chunk_pixels: usize,
}

impl Default for ForwardOptions {
    fn default() -> Self {
        Self {
            samples: 256,
            bins: 8,
            lambda_range_nm: [400.0, 700.0],
            spectral_sub: 2,
            max_records: 16,
            max_depth: 16,
            max_sample_factor: 4,
            target_rel_error: 0.01,
            abs_error: 1e-4,
            reference_alpha_per_mm: 0.3,
            flagged_limit: 0.05,
            inclusion_limit: 0.3,
            skip_flags: flag::OUTSIDE_OUTLINE
                | flag::INCLUSION
                | flag::GHOST
                | flag::USER
                | flag::SATURATED
                | flag::BELOW_NOISE,
            seed: 0,
            threads: 0,
            chunk_pixels: 16,
        }
    }
}

impl ForwardOptions {
    fn validate(&self) -> Result<(), ForwardError> {
        let bad = ForwardError::BadOptions;
        if self.samples < 2 {
            return Err(bad("samples must be at least 2"));
        }
        if self.max_sample_factor == 0
            || self.samples.saturating_mul(self.max_sample_factor) > usize::from(u16::MAX)
        {
            return Err(bad("samples times max_sample_factor must be in 1..=65535"));
        }
        if self.bins == 0 || self.bins > 256 {
            return Err(bad("bins must be in 1..=256"));
        }
        if self.spectral_sub == 0 || self.spectral_sub > 16 {
            return Err(bad("spectral_sub must be in 1..=16"));
        }
        if self.max_records == 0 || self.max_records > 255 {
            return Err(bad("max_records must be in 1..=255"));
        }
        if self.max_depth == 0 {
            return Err(bad("max_depth must be at least 1"));
        }
        let [lo, hi] = self.lambda_range_nm;
        if !(lo.is_finite() && hi.is_finite() && lo > 0.0 && hi > lo) {
            return Err(bad("the wavelength range must be increasing and positive"));
        }
        if !(self.reference_alpha_per_mm.is_finite() && self.reference_alpha_per_mm > 0.0) {
            return Err(bad("reference_alpha_per_mm must be positive"));
        }
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(unit(self.flagged_limit) && unit(self.inclusion_limit)) {
            return Err(bad("the drop limits must be in 0..=1"));
        }
        if !(self.target_rel_error.is_finite()
            && self.target_rel_error >= 0.0
            && self.abs_error.is_finite()
            && self.abs_error >= 0.0)
        {
            return Err(bad("the error targets must be non-negative"));
        }
        Ok(())
    }
}

/// One view to trace: which rig view, its working grid, and which pixels to skip.
#[derive(Debug, Clone, Copy)]
pub struct ViewTraceInput<'a> {
    /// The index of the view in the rig profile.
    pub view: usize,
    /// The working grid (lane P1's `WorkingGrid::fit`).
    pub grid: WorkingGrid,
    /// The pixel mask on the working grid; pixels with any of
    /// [`ForwardOptions::skip_flags`] are not traced.
    pub mask: Option<&'a PixelMask>,
    /// Per working pixel, the relative standard error to reach (for example the photo noise
    /// over the pixel value); non-finite or non-positive entries use the global target.
    pub rel_error_targets: Option<&'a [f32]>,
}

impl<'a> ViewTraceInput<'a> {
    /// A view with no mask and the global error target.
    #[must_use]
    pub const fn new(view: usize, grid: WorkingGrid) -> Self {
        Self {
            view,
            grid,
            mask: None,
            rel_error_targets: None,
        }
    }

    /// This view with a pixel mask.
    #[must_use]
    pub const fn with_mask(mut self, mask: &'a PixelMask) -> Self {
        self.mask = Some(mask);
        self
    }
}

/// Everything [`trace_rig`] needs.
#[derive(Clone, Copy)]
pub struct ForwardInput<'a> {
    /// The scanned rough, in its own frame (mm).
    pub mesh: &'a RoughMesh,
    /// The transform from the mesh frame to the rig frame (lane locate's alignment).
    pub alignment: Rigid,
    /// The cameras, indices and light model.
    pub rig: &'a ColourRig,
    /// Polished or frosted per triangle.
    pub surfaces: &'a SurfaceMap,
    /// The stone's refractive index model.
    pub index: &'a StoneIndex,
    /// The zone geometry in the mesh frame (mm); `None` is a single zone.
    pub zones: Option<&'a ZonedAbsorption>,
    /// Located inclusion shells (mesh frame); paths through them are removed.
    pub inclusions: &'a [InclusionShell],
    /// The camera's spectral sensitivities.
    pub camera: &'a CameraResponse,
    /// The backlight spectrum.
    pub backlight: &'a BacklightSpectrum,
    /// The views to trace.
    pub views: &'a [ViewTraceInput<'a>],
    /// The settings.
    pub options: &'a ForwardOptions,
    /// Where to keep the disk cache; `None` for no cache.
    pub cache_dir: Option<&'a Path>,
}

/// Linear interpolation of a curve on the 380 to 780 nm, 5 nm grid; 0 outside.
fn interpolate(values: &[f64; GRID_LEN], lambda_nm: f64) -> f64 {
    let position = (lambda_nm - GRID_FIRST_NM) / GRID_STEP_NM;
    if !(0.0..=(GRID_LEN - 1) as f64).contains(&position) {
        return 0.0;
    }
    let lo = (position.floor() as usize).min(GRID_LEN - 2);
    let t = position - lo as f64;
    (values[lo + 1] - values[lo]).mul_add(t, values[lo])
}

fn build_spectral(input: &ForwardInput<'_>) -> Result<SpectralSetup, ForwardError> {
    let opts = input.options;
    let traced_bins = if input.index.is_dispersive() {
        opts.bins
    } else {
        1
    };
    let [lo, hi] = opts.lambda_range_nm;
    let bin_width = (hi - lo) / opts.bins as f64;
    let step = bin_width / opts.spectral_sub as f64;
    let mut points = Vec::with_capacity(opts.bins * opts.spectral_sub);
    let mut reference = [0.0_f64; 3];
    for b in 0..opts.bins {
        for j in 0..opts.spectral_sub {
            let lambda_nm = step.mul_add(j as f64 + 0.5, bin_width.mul_add(b as f64, lo));
            let source = input.backlight.spectral_power(lambda_nm);
            let mut weight = [0.0_f64; 3];
            for c in 0..3 {
                weight[c] = interpolate(&input.camera.sensitivity()[c], lambda_nm) * source * step;
                reference[c] += weight[c];
            }
            points.push(EvalPoint {
                bin: if traced_bins == 1 { 0 } else { b },
                lambda_nm,
                weight,
            });
        }
    }
    if reference.iter().any(|r| !r.is_finite() || *r <= 0.0) {
        return Err(ForwardError::BadSpectrum(
            "the camera and backlight give no signal in the wavelength range".to_owned(),
        ));
    }
    for point in &mut points {
        for (weight, &reference_value) in point.weight.iter_mut().zip(&reference) {
            *weight /= reference_value;
        }
    }
    Ok(SpectralSetup {
        lambda_min_nm: lo,
        lambda_max_nm: hi,
        bins: opts.bins,
        traced_bins,
        eval_points: points,
    })
}

fn validate(input: &ForwardInput<'_>) -> Result<(), ForwardError> {
    input.options.validate()?;
    if input.views.is_empty() {
        return Err(ForwardError::NoViews);
    }
    input.rig.validate()?;
    if let Some(zones) = input.zones {
        zones.validate().map_err(ForwardError::BadZoning)?;
    }
    if let StoneIndex::Constant(n) = input.index
        && !(n.is_finite() && *n >= 1.0)
    {
        return Err(ForwardError::BadOptions(
            "the constant index must be at least 1",
        ));
    }
    let views = input.rig.rig.views.len();
    for (i, v) in input.views.iter().enumerate() {
        let mask_fits = v
            .mask
            .is_none_or(|m| m.width() == v.grid.width && m.height() == v.grid.height);
        let targets_fit = v
            .rel_error_targets
            .is_none_or(|t| t.len() == v.grid.width * v.grid.height);
        let grid_ok =
            v.grid.width > 0 && v.grid.height > 0 && v.grid.scale.is_finite() && v.grid.scale > 0.0;
        if v.view >= views || !(mask_fits && targets_fit && grid_ok) {
            return Err(ForwardError::BadView(i));
        }
    }
    if let LightModel::Backlight { per_view, .. } = &input.rig.lighting.model
        && let Some(i) = input.views.iter().position(|v| v.view >= per_view.len())
    {
        return Err(ForwardError::BadLighting(format!(
            "view {} has no backlight panel",
            input.views[i].view + 1
        )));
    }
    Ok(())
}

/// A run of pixels of one view that one task traces.
struct Chunk {
    slot: usize,
    first: usize,
    last: usize,
}

/// What a task returns: its pixels' results in pixel order.
#[derive(Default)]
struct ChunkOut {
    pixels: Vec<u32>,
    status: Vec<u8>,
    samples: Vec<u16>,
    mc_mean: Vec<f32>,
    mc_variance: Vec<f32>,
    loss: Vec<PixelLoss>,
    counts: Vec<u8>,
    records: Vec<PathRecord>,
    total_samples: u64,
    max_range_mm: f32,
}

/// The pixels to trace in every view (row-major indices, masked pixels left out) and the tasks
/// that share them out.
fn plan_chunks(input: &ForwardInput<'_>) -> (Vec<Vec<u32>>, Vec<Chunk>) {
    let opts = input.options;
    let mut actives: Vec<Vec<u32>> = Vec::with_capacity(input.views.len());
    let mut chunks: Vec<Chunk> = Vec::new();
    let chunk_pixels = opts.chunk_pixels.max(1);
    for (slot, v) in input.views.iter().enumerate() {
        let (w, h) = (v.grid.width, v.grid.height);
        let list: Vec<u32> = (0..w * h)
            .filter(|&p| {
                v.mask
                    .is_none_or(|m| m.get(p % w, p / w) & opts.skip_flags == 0)
            })
            .map(|p| p as u32)
            .collect();
        for first in (0..list.len()).step_by(chunk_pixels) {
            chunks.push(Chunk {
                slot,
                first,
                last: (first + chunk_pixels).min(list.len()),
            });
        }
        actives.push(list);
    }
    (actives, chunks)
}

/// Traces the pixels `pixels` (row-major indices) of one view: one task of [`trace_rig`].
fn trace_chunk(ctx: &TraceCtx<'_>, view: &ViewTraceInput<'_>, pixels: &[u32]) -> ChunkOut {
    let width = view.grid.width;
    let mut scratch = PixelScratch::new(ctx.traced_bins);
    let mut out = ChunkOut::default();
    for &p in pixels {
        let p = p as usize;
        let target = view
            .rel_error_targets
            .and_then(|t| t.get(p))
            .copied()
            .filter(|t| t.is_finite() && *t > 0.0)
            .map_or(ctx.opts.target_rel_error, f64::from);
        let px = trace_working_pixel(
            ctx,
            view.view,
            &view.grid,
            p % width,
            p / width,
            target,
            &mut scratch,
            &mut out.records,
            &mut out.counts,
        );
        out.pixels.push(p as u32);
        out.status.push(px.status);
        out.samples.push(px.samples);
        out.mc_mean.push(px.mc_mean);
        out.mc_variance.push(px.mc_variance);
        out.loss.push(px.loss);
        out.total_samples += u64::from(px.samples);
        out.max_range_mm = out.max_range_mm.max(px.max_range_mm);
    }
    out
}

/// Assembles the task outputs in view, then pixel order: the records of every view, the total
/// sample count and the largest cluster range.
fn assemble_views(
    input: &ForwardInput<'_>,
    chunks: &[Chunk],
    outputs: &mut [Option<ChunkOut>],
    traced_bins: usize,
) -> Result<(Vec<ViewRecords>, u64, f32), ForwardError> {
    let mut views = Vec::with_capacity(input.views.len());
    let mut total_samples = 0_u64;
    let mut max_range = 0.0_f32;
    for (slot, v) in input.views.iter().enumerate() {
        let n = v.grid.width * v.grid.height;
        let mut view = ViewRecords {
            view: v.view,
            grid: v.grid,
            status: vec![status::SKIPPED; n],
            samples: vec![0; n],
            mc_mean: vec![0.0; n],
            mc_variance: vec![0.0; n],
            loss: vec![PixelLoss::default(); n],
            offsets: Vec::with_capacity(n * traced_bins + 1),
            records: Vec::new(),
        };
        view.offsets.push(0);
        let mut running = 0_u32;
        let mut next_pixel = 0_usize;
        for (task, chunk) in chunks.iter().enumerate() {
            if chunk.slot != slot {
                continue;
            }
            let Some(out) = outputs[task].take() else {
                return Err(ForwardError::Cancelled);
            };
            for (k, &p) in out.pixels.iter().enumerate() {
                let p = p as usize;
                while next_pixel < p {
                    for _ in 0..traced_bins {
                        view.offsets.push(running);
                    }
                    next_pixel += 1;
                }
                view.status[p] = out.status[k];
                view.samples[p] = out.samples[k];
                view.mc_mean[p] = out.mc_mean[k];
                view.mc_variance[p] = out.mc_variance[k];
                view.loss[p] = out.loss[k];
                for b in 0..traced_bins {
                    running = running
                        .checked_add(u32::from(out.counts[k * traced_bins + b]))
                        .ok_or(ForwardError::BadOptions("too many records for one view"))?;
                    view.offsets.push(running);
                }
                next_pixel = p + 1;
            }
            view.records.extend_from_slice(&out.records);
            total_samples += out.total_samples;
            max_range = max_range.max(out.max_range_mm);
        }
        while next_pixel < n {
            for _ in 0..traced_bins {
                view.offsets.push(running);
            }
            next_pixel += 1;
        }
        views.push(view);
    }
    Ok((views, total_samples, max_range))
}

/// Traces the rig: path records for every requested view.
///
/// `cancel` is polled between chunks of pixels; `progress` receives the finished fraction in
/// `0..=1` on the calling thread. The result does not depend on the thread count. With a cache
/// directory in the input a matching file is read instead of tracing, and a fresh trace is
/// written to it (see [`cache`]).
///
/// # Errors
///
/// [`ForwardError`] for an unusable input, or [`ForwardError::Cancelled`] when `cancel` was
/// raised (nothing is cached then).
pub fn trace_rig(
    input: &ForwardInput<'_>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<ForwardRecords, ForwardError> {
    validate(input)?;
    let opts = input.options;
    let key = cache::cache_key(input);
    if let Some(dir) = input.cache_dir
        && let Some(hit) = cache::load(dir, key)
    {
        progress(1.0);
        return Ok(hit);
    }
    let spectral = build_spectral(input)?;
    let traced_bins = spectral.traced_bins;
    let mut inclusions = Vec::with_capacity(input.inclusions.len());
    for (i, shell) in input.inclusions.iter().enumerate() {
        inclusions.push(shell.mesh().map_err(|e| ForwardError::BadInclusion(i, e))?);
    }
    let scene = Scene::new(input.mesh, &input.rig.rig, input.alignment);
    let n_zones = 1 + input.zones.map_or(0, |z| z.zones.len().min(MAX_ZONES));
    let ctx = TraceCtx {
        scene,
        lighting: &input.rig.lighting,
        surfaces: input.surfaces,
        index: input.index,
        kernel: input.zones.map(ZoneKernel::new),
        inclusions,
        opts,
        eps: 1e-7 * scene.mesh_scale(),
        traced_bins,
        n_zones,
    };

    // The pixels to trace and the tasks that share them out.
    let (actives, chunks) = plan_chunks(input);

    let work = |task: usize| -> ChunkOut {
        let chunk = &chunks[task];
        trace_chunk(
            &ctx,
            &input.views[chunk.slot],
            &actives[chunk.slot][chunk.first..chunk.last],
        )
    };

    let total = chunks.len().max(1);
    let threads = thread_count(opts.threads, chunks.len());
    let mut report = |done: usize| progress(done as f32 / total as f32);
    let mut outputs = run_indexed(chunks.len(), threads, cancel, &work, &mut report);
    if outputs.iter().any(Option::is_none) {
        return Err(ForwardError::Cancelled);
    }

    let (views, total_samples, max_range) =
        assemble_views(input, &chunks, &mut outputs, traced_bins)?;

    let mut result = ForwardRecords {
        spectral,
        n_zones,
        views,
        stats: ForwardStats {
            cache: CacheStatus::NotUsed,
            samples: total_samples,
            max_cluster_range_mm: max_range,
        },
    };
    if let Some(dir) = input.cache_dir {
        result.stats.cache = if cache::store(dir, key, &result) {
            CacheStatus::Stored
        } else {
            CacheStatus::StoreFailed
        };
    }
    progress(1.0);
    Ok(result)
}

/// The straight chord of a line through a convex box (helper shared by tests): the parameter
/// range `(t_enter, t_exit)` of the ray `origin + t dir` inside `[lo, hi]`.
#[cfg(test)]
pub(crate) fn box_chord(
    origin: glam::DVec3,
    dir: glam::DVec3,
    lo: glam::DVec3,
    hi: glam::DVec3,
) -> Option<(f64, f64)> {
    use glam::DVec3;
    let inv = DVec3::ONE / dir;
    let near = (lo - origin) * inv;
    let far = (hi - origin) * inv;
    let enter = near.min(far).max_element();
    let exit = near.max(far).min_element();
    (exit > enter).then_some((enter, exit))
}

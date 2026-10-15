//! Tracing the paths of one camera ray, and of all the samples of one working pixel.
//!
//! A path starts at the camera, in the surroundings. At every surface it meets (first hit on the
//! mesh BVH) it reflects or refracts: polished triangles follow Fresnel with Russian roulette,
//! frosted ones sample GGX microfacets (see `surface`). Inside the stone the legs are measured
//! per zone with the K1 kernels and added up. A path ends when it escapes: the light model gives
//! the radiance at its exit, relative to the level at the pixel it started from. Radiance scaling
//! by `n^2` at the interfaces is omitted because every path starts and ends in the same medium,
//! where it cancels exactly.

use glam::{DVec2, DVec3};
use indicatrix::optics::zoning::ZoneKernel;

use super::{
    ForwardOptions, StoneIndex,
    compress::compress,
    lighting::RigLighting,
    records::{
        CHROMA_LIMIT, LENGTHS, PathRecord, PixelLoss, chroma_encode, chroma_in_range, status,
    },
    rng::{PixelSequence, Rng},
    surface::{Scatter, SurfaceMap, scatter},
};
use crate::rough_plan::{locate::Scene, photometry::WorkingGrid, shape::RoughMesh};

/// Everything a trace needs, shared by all threads.
pub struct TraceCtx<'a> {
    pub scene: Scene<'a>,
    pub lighting: &'a RigLighting,
    pub surfaces: &'a SurfaceMap,
    pub index: &'a StoneIndex,
    pub kernel: Option<ZoneKernel>,
    pub inclusions: Vec<RoughMesh>,
    pub opts: &'a ForwardOptions,
    /// The shortest distance a ray must travel from a surface to count another hit.
    pub eps: f64,
    /// Bins traced: the spectral bins for a dispersive stone, else 1.
    pub traced_bins: usize,
    /// The zones in use (base plus shaped), for distances in the compression.
    pub n_zones: usize,
}

/// The wavelength of the sodium d line, nm: the index at which a dispersive stone is traced when
/// only one bin is traced (see [`ForwardOptions::trace_dispersion`](super::ForwardOptions)).
const D_LINE_NM: f64 = 587.56;

impl TraceCtx<'_> {
    /// The wavelength of bin `bin` at the in-bin position `fraction`.
    fn sample_lambda(&self, bin: usize, fraction: f64) -> f64 {
        let [lo, hi] = self.opts.lambda_range_nm;
        if self.traced_bins == 1 {
            // One shared bin: a dispersive stone is traced at its d-line index, any other index
            // does not depend on the wavelength.
            return if self.index.is_dispersive() {
                D_LINE_NM
            } else {
                f64::midpoint(lo, hi)
            };
        }
        let width = (hi - lo) / self.traced_bins as f64;
        width.mul_add(bin as f64 + fraction, lo)
    }
}

/// How one sample path ended.
#[derive(Debug, Clone, Copy)]
pub enum Terminal {
    /// Escaped into the light: lengths per zone, the weight (throughput times the channel-mean
    /// light ratio) and the factor by which each channel's light ratio differs from that mean.
    Record {
        lengths: [f32; LENGTHS],
        weight: f64,
        chroma: [f32; 3],
        flagged: bool,
    },
    /// Escaped into darkness (holder, or off the panel): no contribution.
    Dark,
    /// Cut off at the maximum depth with this weight.
    LostDepth(f64),
    /// Dropped (invalid microfacet sample, damaged mesh) with this weight.
    LostInvalid(f64),
    /// Passed through an inclusion with this weight.
    Inclusion(f64),
}

/// Traces one path of view `view` starting at the full-resolution photo position `pixel`, at
/// wavelength `lambda_nm`.
pub fn trace_sample(
    ctx: &TraceCtx<'_>,
    view: usize,
    pixel: DVec2,
    own_level: [f64; 3],
    lambda_nm: f64,
    rng: &mut Rng,
) -> Terminal {
    let Some((mut pos, mut dir)) = ctx.scene.camera_ray(view, pixel) else {
        return Terminal::LostInvalid(1.0);
    };
    let n_surround = ctx.scene.rig.surround_n;
    let n_stone = ctx.index.n_at(ctx.scene.rig.stone_n, lambda_nm);
    let mut weight = 1.0_f64;
    let mut inside = false;
    let mut lengths = [0.0_f64; LENGTHS];

    for _depth in 0..ctx.opts.max_depth {
        let Some(hit) = ctx.scene.mesh.first_hit(pos, dir, ctx.eps) else {
            if inside {
                return Terminal::LostInvalid(weight);
            }
            return escape(ctx, view, pos, dir, weight, &lengths, own_level);
        };
        let point = pos + dir * hit.t;
        let entering = dir.dot(hit.normal) < 0.0;
        if inside {
            if entering {
                return Terminal::LostInvalid(weight);
            }
            let leg = ctx.kernel.as_ref().map_or_else(
                || {
                    let mut single = [0.0_f64; LENGTHS];
                    single[0] = hit.t;
                    single
                },
                |kernel| kernel.lengths(pos, point),
            );
            for (total, part) in lengths.iter_mut().zip(leg) {
                *total += part;
            }
            let through = ctx.inclusions.iter().any(|shell| {
                shell
                    .first_hit(pos, dir, 0.0)
                    .is_some_and(|inner| inner.t < hit.t)
            });
            if through {
                return Terminal::Inclusion(weight);
            }
        } else if !entering {
            return Terminal::LostInvalid(weight);
        }
        let (n_from, n_to) = if entering {
            (n_surround, n_stone)
        } else {
            (n_stone, n_surround)
        };
        let facing = if entering { hit.normal } else { -hit.normal };
        let class = ctx.surfaces.class_of(hit.triangle);
        match scatter(class, dir, facing, n_from, n_to, rng) {
            Scatter::Invalid => return Terminal::LostInvalid(weight),
            Scatter::Out {
                dir: next,
                factor,
                transmitted,
            } => {
                weight *= factor;
                dir = next;
                pos = point;
                if transmitted {
                    inside = entering;
                }
            }
        }
    }
    Terminal::LostDepth(weight)
}

/// The path left the mesh for good at `pos` along `dir` (mesh frame).
fn escape(
    ctx: &TraceCtx<'_>,
    view: usize,
    pos: DVec3,
    dir: DVec3,
    weight: f64,
    lengths: &[f64; LENGTHS],
    own_level: [f64; 3],
) -> Terminal {
    let alignment = &ctx.scene.alignment;
    let light = ctx.lighting.exit_light(
        ctx.scene.rig,
        view,
        alignment.to_rig(pos),
        alignment.dir_to_rig(dir).normalize(),
        own_level,
    );
    // The scalar weight carries the channel mean of the light ratio (the quantity the adaptive
    // sampling and the Monte-Carlo statistics are about); the colour is the factor of each channel
    // against that mean (ZC-13). A grey ratio gives the mean itself and the factors exactly 1.
    let ratio = light.ratio;
    #[expect(
        clippy::float_cmp,
        reason = "a grey ratio is detected by bit-equal channels, which makes the mean and the factors exactly 1"
    )]
    let grey_ratio = ratio[0] == ratio[1] && ratio[1] == ratio[2];
    let mean = if grey_ratio {
        ratio[0]
    } else {
        (ratio[0] + ratio[1] + ratio[2]) / 3.0
    };
    let total = weight * mean;
    if total <= 0.0 || !total.is_finite() {
        return Terminal::Dark;
    }
    // A channel that differs from the mean by more than the stored range is clamped to it and the
    // exit is flagged, like light from a place the camera never saw: it counts towards dropping
    // the pixel (`flagged_limit`) instead of silently biasing it.
    let mut flagged = light.flagged;
    let mut chroma = [1.0_f32; 3];
    for (slot, &r) in chroma.iter_mut().zip(&ratio) {
        let factor = r / mean;
        flagged |= !chroma_in_range(factor);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the record chroma precision; the factor is clamped to 1 / 1.5 ..= 1.5"
        )]
        let clamped = factor.clamp(1.0 / CHROMA_LIMIT, CHROMA_LIMIT) as f32;
        *slot = clamped;
    }
    let mut out = [0.0_f32; LENGTHS];
    for (slot, &l) in out.iter_mut().zip(lengths) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the record storage precision; path lengths in mm are in the normal float range"
        )]
        let length = l as f32;
        *slot = length;
    }
    Terminal::Record {
        lengths: out,
        weight: total,
        chroma,
        flagged,
    }
}

/// Per-thread scratch of the pixel loop.
pub struct PixelScratch {
    raw: Vec<Vec<PathRecord>>,
    /// The compressed records of the bin being finished.
    compressed: Vec<PathRecord>,
    /// `(weight, total length)` of the exit paths of the sample being traced, all bins.
    sample_paths: Vec<(f64, f64)>,
    /// The same for all samples of the first batch, and where each sample's paths end: the
    /// reference absorption of the pixel is only known once the first batch is done.
    first_paths: Vec<(f64, f64)>,
    first_ends: Vec<usize>,
}

impl PixelScratch {
    /// Empty scratch for a pixel traced in `traced_bins` bins: one raw record list per bin,
    /// reused across the pixel's samples so tracing a pixel does not reallocate.
    pub fn new(traced_bins: usize) -> Self {
        Self {
            raw: vec![Vec::new(); traced_bins],
            compressed: Vec::new(),
            sample_paths: Vec::new(),
            first_paths: Vec::new(),
            first_ends: Vec::new(),
        }
    }
}

/// The scalar results of one pixel.
#[derive(Debug, Clone, Copy)]
pub struct PixelOutput {
    pub status: u8,
    pub samples: u16,
    /// The mean transmittance at the pixel's reference absorption and its variance.
    pub mc_mean: f32,
    pub mc_variance: f32,
    /// The mean transmittance of the clear stone and its variance.
    pub mc_mean_clear: f32,
    pub mc_variance_clear: f32,
    pub loss: PixelLoss,
    pub max_range_mm: f32,
}

/// Welford's running mean and sum of squared deviations of a stream of per-sample estimates.
#[derive(Debug, Clone, Copy, Default)]
struct Welford {
    count: f64,
    mean: f64,
    m2: f64,
}

impl Welford {
    fn push(&mut self, x: f64) {
        self.count += 1.0;
        let delta = x - self.mean;
        self.mean += delta / self.count;
        self.m2 = delta.mul_add(x - self.mean, self.m2);
    }

    /// The variance of the mean (not of one sample); 0 below two samples.
    fn variance_of_mean(&self) -> f64 {
        if self.count > 1.0 {
            (self.m2 / (self.count - 1.0) / self.count).max(0.0)
        } else {
            0.0
        }
    }

    /// The standard error of the mean; infinite below two samples.
    fn standard_error(&self) -> f64 {
        if self.count > 1.0 {
            self.variance_of_mean().sqrt()
        } else {
            f64::INFINITY
        }
    }
}

/// The estimate of one sample at absorption `alpha`: the transmitted weight of its paths,
/// averaged over the traced bins.
fn sample_estimate(paths: &[(f64, f64)], alpha: f64, bins: usize) -> f64 {
    let sum: f64 = paths
        .iter()
        .map(|&(weight, total)| weight * (-alpha * total).exp())
        .sum();
    sum / bins as f64
}

/// The absorption at which the pixel's mean path has optical depth 1: the reciprocal of the
/// weight-averaged path length of `paths`; `None` when no weight reached the light.
fn reference_alpha(paths: &[(f64, f64)]) -> Option<f64> {
    let (weight, moment) = paths
        .iter()
        .fold((0.0_f64, 0.0_f64), |(w, m), &(weight, total)| {
            (w + weight, weight.mul_add(total, m))
        });
    let mean_path = moment / weight;
    (weight > 0.0 && mean_path.is_finite() && mean_path > 0.0).then_some(1.0 / mean_path)
}

/// Traces all samples of working pixel `(x, y)` of `view` and appends its compressed records
/// (bin after bin, `n_zones` lengths and the weight each) to `records`, their three chroma codes
/// each to `chroma`, and one count per traced bin to `counts`.
///
/// `target` is the relative standard error the pixel must reach before sampling stops. The
/// error is judged on the clear-stone transmittance (the sum of the path weights, absorption 0),
/// which stays well above any floor on a large stone; the variance is kept at two absorptions,
/// 0 and the pixel's own reference absorption (optical depth 1 at its mean path), so the solver
/// can weight a pale and a saturated prediction alike (ZC-7).
#[allow(
    clippy::too_many_arguments,
    reason = "the pixel loop needs the context, the pixel, its target, the scratch and both outputs"
)]
pub fn trace_working_pixel(
    ctx: &TraceCtx<'_>,
    view: usize,
    grid: &WorkingGrid,
    x: usize,
    y: usize,
    target: f64,
    scratch: &mut PixelScratch,
    records: &mut Vec<f32>,
    chroma: &mut Vec<u8>,
    counts: &mut Vec<u8>,
) -> PixelOutput {
    let opts = ctx.opts;
    let bins = ctx.traced_bins;
    let pixel_index = y * grid.width + x;
    let [u0, v0, u1, v1] = grid.footprint(x, y);
    let sequence = PixelSequence::new(opts.seed, view as u64, pixel_index as u64);
    for raw in &mut scratch.raw {
        raw.clear();
    }
    scratch.first_paths.clear();
    scratch.first_ends.clear();

    let cap = opts.samples * opts.max_sample_factor.max(1);
    let mut acc = PixelAccum::default();
    let mut batch = opts.samples;
    let mut alpha_ref: Option<f64> = None;

    while acc.done < cap {
        let take = batch.min(cap - acc.done);
        for s in acc.done..acc.done + take {
            let [fx, fy, fl] = sequence.point(s as u64);
            let pixel = DVec2::new(fx.mul_add(u1 - u0, u0), fy.mul_add(v1 - v0, v0));
            let own_level = ctx.lighting.own_level(view, pixel);
            scratch.sample_paths.clear();
            for (b, raw) in scratch.raw.iter_mut().enumerate() {
                let lambda = ctx.sample_lambda(b, fl);
                let mut rng = Rng::keyed(
                    opts.seed,
                    view as u64,
                    pixel_index as u64,
                    s as u64,
                    b as u64,
                );
                match trace_sample(ctx, view, pixel, own_level, lambda, &mut rng) {
                    Terminal::Record {
                        lengths,
                        weight,
                        chroma,
                        flagged,
                    } => {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "f32 is the record weight precision; the path weight is in the normal float range"
                        )]
                        let weight32 = weight as f32;
                        raw.push(PathRecord {
                            lengths,
                            weight: weight32,
                            chroma,
                        });
                        let total: f64 = lengths.iter().map(|&l| f64::from(l)).sum();
                        scratch.sample_paths.push((weight, total));
                        acc.exit_weight += weight;
                        if flagged {
                            acc.flagged_weight += weight;
                        }
                    }
                    Terminal::Dark => {}
                    Terminal::LostDepth(w) => acc.lost_depth += w,
                    Terminal::LostInvalid(w) => acc.lost_invalid += w,
                    Terminal::Inclusion(w) => acc.lost_inclusion += w,
                }
            }
            acc.clear
                .push(sample_estimate(&scratch.sample_paths, 0.0, bins));
            if let Some(alpha) = alpha_ref {
                acc.reference
                    .push(sample_estimate(&scratch.sample_paths, alpha, bins));
            } else {
                scratch.first_paths.extend_from_slice(&scratch.sample_paths);
                scratch.first_ends.push(scratch.first_paths.len());
            }
        }
        acc.done += take;
        if alpha_ref.is_none() {
            // The first batch is done: fix the reference absorption from its mean path and
            // replay its samples at that absorption, in sample order.
            let alpha =
                reference_alpha(&scratch.first_paths).unwrap_or(opts.reference_alpha_per_mm);
            alpha_ref = Some(alpha);
            let mut start = 0;
            for &end in &scratch.first_ends {
                acc.reference.push(sample_estimate(
                    &scratch.first_paths[start..end],
                    alpha,
                    bins,
                ));
                start = end;
            }
            scratch.first_paths.clear();
            scratch.first_ends.clear();
        }
        let standard_error = acc.clear.standard_error();
        if standard_error <= target * acc.clear.mean.abs() || standard_error <= opts.abs_error {
            acc.converged = true;
            break;
        }
        batch = acc.done;
    }

    finish_pixel(ctx, &acc, scratch, records, chroma, counts)
}

/// The running sums of one pixel's sampling loop in [`trace_working_pixel`].
#[derive(Default)]
struct PixelAccum {
    /// Camera-ray samples taken so far.
    done: usize,
    /// The per-sample estimate at absorption 0 (the convergence criterion) and at the pixel's
    /// reference absorption.
    clear: Welford,
    reference: Welford,
    /// Path weight lost to the depth limit, to invalid paths and to inclusions.
    lost_depth: f64,
    lost_invalid: f64,
    lost_inclusion: f64,
    /// Weight of all exit paths and of those that exit through flagged light.
    exit_weight: f64,
    flagged_weight: f64,
    converged: bool,
}

/// The status bits of a finished pixel: not converged, too much inclusion loss, too much flagged
/// light.
fn status_code(
    opts: &ForwardOptions,
    converged: bool,
    inclusion_fraction: f64,
    flagged_fraction: f64,
) -> u8 {
    let mut code = 0_u8;
    if !converged {
        code |= status::NOT_CONVERGED;
    }
    if inclusion_fraction > opts.inclusion_limit {
        code |= status::INCLUSION;
    }
    if flagged_fraction > opts.flagged_limit {
        code |= status::FLAGGED_LIGHT;
    }
    code
}

/// Compresses the pixel's raw records into `records` and `counts` and builds its result from the
/// sums of the sampling loop.
fn finish_pixel(
    ctx: &TraceCtx<'_>,
    acc: &PixelAccum,
    scratch: &mut PixelScratch,
    records: &mut Vec<f32>,
    chroma: &mut Vec<u8>,
    counts: &mut Vec<u8>,
) -> PixelOutput {
    let opts = ctx.opts;
    let bins = ctx.traced_bins;
    let &PixelAccum {
        done,
        clear,
        reference,
        lost_depth,
        lost_invalid,
        lost_inclusion,
        exit_weight,
        flagged_weight,
        converged,
    } = acc;
    let n = done as f64;
    let per_path = n * bins as f64;
    let inclusion_fraction = lost_inclusion / per_path;
    let flagged_fraction = if exit_weight > 0.0 {
        flagged_weight / exit_weight
    } else {
        0.0
    };
    let code = status_code(opts, converged, inclusion_fraction, flagged_fraction);
    let dropped = code & status::DROPPED != 0;
    let mut max_range = 0.0_f32;
    if dropped {
        counts.extend(std::iter::repeat_n(0_u8, bins));
    } else {
        // Re-normalise by the surviving share when an inclusion removed some paths.
        let scale = (1.0 / n) / (1.0 - inclusion_fraction).max(1e-9);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the compression tolerance precision; a path length in mm is in the normal float range"
        )]
        let tolerance = (0.1 / opts.reference_alpha_per_mm) as f32;
        let zones = ctx.n_zones;
        for raw in &mut scratch.raw {
            for record in &mut *raw {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "f32 is the record weight precision; the renormalised weight is in the normal float range"
                )]
                let weight = (f64::from(record.weight) * scale) as f32;
                record.weight = weight;
            }
            scratch.compressed.clear();
            let range = compress(
                raw,
                opts.max_records,
                tolerance,
                zones,
                &mut scratch.compressed,
            );
            max_range = max_range.max(range);
            // Flat layout of `ViewRecords`: the `n_zones` lengths, then the weight.
            for record in &scratch.compressed {
                records.extend_from_slice(&record.lengths[..zones]);
                records.push(record.weight);
                chroma.extend(record.chroma.map(|c| chroma_encode(f64::from(c))));
            }
            counts.push(
                u8::try_from(scratch.compressed.len())
                    .expect("max_records is validated to 1..=255"),
            );
        }
    }
    let renormalise = if dropped {
        1.0
    } else {
        1.0 / (1.0 - inclusion_fraction).max(1e-9)
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the per-pixel storage precision; means, variances and loss fractions are in the normal float range"
    )]
    let (mc_mean, mc_variance, mc_mean_clear, mc_variance_clear, loss) = (
        (reference.mean * renormalise) as f32,
        (reference.variance_of_mean() * renormalise * renormalise) as f32,
        (clear.mean * renormalise) as f32,
        (clear.variance_of_mean() * renormalise * renormalise) as f32,
        PixelLoss {
            depth: (lost_depth / per_path) as f32,
            invalid: (lost_invalid / per_path) as f32,
            inclusion: inclusion_fraction as f32,
            flagged: flagged_fraction as f32,
        },
    );
    PixelOutput {
        status: code,
        samples: u16::try_from(done).expect("samples * max_sample_factor is validated to fit u16"),
        mc_mean,
        mc_variance,
        mc_mean_clear,
        mc_variance_clear,
        loss,
        max_range_mm: max_range,
    }
}

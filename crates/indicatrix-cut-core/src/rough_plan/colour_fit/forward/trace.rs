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
    records::{LENGTHS, PathRecord, PixelLoss, status},
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

impl TraceCtx<'_> {
    /// The wavelength of bin `bin` at the in-bin position `fraction`.
    fn sample_lambda(&self, bin: usize, fraction: f64) -> f64 {
        let [lo, hi] = self.opts.lambda_range_nm;
        if self.traced_bins == 1 {
            return f64::midpoint(lo, hi);
        }
        let width = (hi - lo) / self.traced_bins as f64;
        width.mul_add(bin as f64 + fraction, lo)
    }
}

/// How one sample path ended.
#[derive(Debug, Clone, Copy)]
pub enum Terminal {
    /// Escaped into the light: lengths per zone and the weight (throughput times light ratio).
    Record {
        lengths: [f32; LENGTHS],
        weight: f64,
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
    own_level: f64,
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
    own_level: f64,
) -> Terminal {
    let alignment = &ctx.scene.alignment;
    let light = ctx.lighting.exit_light(
        ctx.scene.rig,
        view,
        alignment.to_rig(pos),
        alignment.dir_to_rig(dir).normalize(),
        own_level,
    );
    let total = weight * light.ratio;
    if total <= 0.0 || !total.is_finite() {
        return Terminal::Dark;
    }
    let mut out = [0.0_f32; LENGTHS];
    for (slot, &l) in out.iter_mut().zip(lengths) {
        *slot = l as f32;
    }
    Terminal::Record {
        lengths: out,
        weight: total,
        flagged: light.flagged,
    }
}

/// Per-thread scratch of the pixel loop.
pub struct PixelScratch {
    raw: Vec<Vec<PathRecord>>,
}

impl PixelScratch {
    pub fn new(traced_bins: usize) -> Self {
        Self {
            raw: vec![Vec::new(); traced_bins],
        }
    }
}

/// The scalar results of one pixel.
#[derive(Debug, Clone, Copy)]
pub struct PixelOutput {
    pub status: u8,
    pub samples: u16,
    pub mc_mean: f32,
    pub mc_variance: f32,
    pub loss: PixelLoss,
    pub max_range_mm: f32,
}

/// Traces all samples of working pixel `(x, y)` of `view` and appends its compressed records
/// (bin after bin) to `records`, and one count per traced bin to `counts`.
///
/// `target` is the relative standard error the pixel must reach before sampling stops.
#[allow(clippy::too_many_arguments)]
pub fn trace_working_pixel(
    ctx: &TraceCtx<'_>,
    view: usize,
    grid: &WorkingGrid,
    x: usize,
    y: usize,
    target: f64,
    scratch: &mut PixelScratch,
    records: &mut Vec<PathRecord>,
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

    let cap = opts.samples * opts.max_sample_factor.max(1);
    let mut acc = PixelAccum::default();
    let mut batch = opts.samples;

    while acc.done < cap {
        let take = batch.min(cap - acc.done);
        for s in acc.done..acc.done + take {
            let [fx, fy, fl] = sequence.point(s as u64);
            let pixel = DVec2::new(fx.mul_add(u1 - u0, u0), fy.mul_add(v1 - v0, v0));
            let own_level = ctx.lighting.own_level(view, pixel);
            let mut estimate = 0.0_f64;
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
                        flagged,
                    } => {
                        raw.push(PathRecord {
                            lengths,
                            weight: weight as f32,
                        });
                        let total: f64 = lengths.iter().map(|&l| f64::from(l)).sum();
                        estimate = f64::mul_add(
                            weight,
                            (-opts.reference_alpha_per_mm * total).exp(),
                            estimate,
                        );
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
            estimate /= bins as f64;
            let count = (s + 1) as f64;
            let delta = estimate - acc.mean;
            acc.mean += delta / count;
            acc.m2 = delta.mul_add(estimate - acc.mean, acc.m2);
        }
        acc.done += take;
        let done = acc.done;
        let standard_error = if done > 1 {
            (acc.m2 / (done - 1) as f64 / done as f64).max(0.0).sqrt()
        } else {
            f64::INFINITY
        };
        if standard_error <= target * acc.mean.abs() || standard_error <= opts.abs_error {
            acc.converged = true;
            break;
        }
        batch = done;
    }

    finish_pixel(ctx, &acc, scratch, records, counts)
}

/// The running sums of one pixel's sampling loop in [`trace_working_pixel`].
#[derive(Default)]
struct PixelAccum {
    /// Camera-ray samples taken so far.
    done: usize,
    /// Welford running mean and sum of squared deviations of the per-sample estimate.
    mean: f64,
    m2: f64,
    /// Path weight lost to the depth limit, to invalid paths and to inclusions.
    lost_depth: f64,
    lost_invalid: f64,
    lost_inclusion: f64,
    /// Weight of all exit paths and of those that exit through flagged light.
    exit_weight: f64,
    flagged_weight: f64,
    converged: bool,
}

/// Compresses the pixel's raw records into `records` and `counts` and builds its result from the
/// sums of the sampling loop.
fn finish_pixel(
    ctx: &TraceCtx<'_>,
    acc: &PixelAccum,
    scratch: &mut PixelScratch,
    records: &mut Vec<PathRecord>,
    counts: &mut Vec<u8>,
) -> PixelOutput {
    let opts = ctx.opts;
    let bins = ctx.traced_bins;
    let PixelAccum {
        done,
        mean,
        m2,
        lost_depth,
        lost_invalid,
        lost_inclusion,
        exit_weight,
        flagged_weight,
        converged,
    } = *acc;
    let n = done as f64;
    let per_path = n * bins as f64;
    let variance_of_mean = if done > 1 { m2 / (n - 1.0) / n } else { 0.0 };
    let inclusion_fraction = lost_inclusion / per_path;
    let flagged_fraction = if exit_weight > 0.0 {
        flagged_weight / exit_weight
    } else {
        0.0
    };
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
    let dropped = code & status::DROPPED != 0;
    let mut max_range = 0.0_f32;
    if dropped {
        counts.extend(std::iter::repeat_n(0_u8, bins));
    } else {
        // Re-normalise by the surviving share when an inclusion removed some paths.
        let scale = (1.0 / n) / (1.0 - inclusion_fraction).max(1e-9);
        let tolerance = (0.1 / opts.reference_alpha_per_mm) as f32;
        for raw in &mut scratch.raw {
            for record in &mut *raw {
                record.weight = (f64::from(record.weight) * scale) as f32;
            }
            let before = records.len();
            let range = compress(raw, opts.max_records, tolerance, ctx.n_zones, records);
            max_range = max_range.max(range);
            counts.push((records.len() - before) as u8);
        }
    }
    let renormalise = if dropped {
        1.0
    } else {
        1.0 / (1.0 - inclusion_fraction).max(1e-9)
    };
    PixelOutput {
        status: code,
        samples: done as u16,
        mc_mean: (mean * renormalise) as f32,
        mc_variance: (variance_of_mean * renormalise * renormalise) as f32,
        loss: PixelLoss {
            depth: (lost_depth / per_path) as f32,
            invalid: (lost_invalid / per_path) as f32,
            inclusion: inclusion_fraction as f32,
            flagged: flagged_fraction as f32,
        },
        max_range_mm: max_range,
    }
}

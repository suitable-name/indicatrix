//! Scintillation in the image domain: a per-pixel temporal term (the pixel's centre ray
//! under five small yaw offsets) and a per-pixel spatial term (contrast of the brilliance
//! fraction over a 5x5 window of map pixels), combined with the metrics module's own
//! functions and weights.
//!
//! The tracer only ever fills `OpticalPixel::scint_returned` and `scint_poses`
//! ([`TemporalSampler`]); the spatial term is a pure post-pass over the finished map
//! ([`scintillation_spatial`]). Neither touches a base class count or a Fire sum.
use super::{
    classify::{ApertureSampleContext, RayClassification, classify_ray},
    image::offset,
    image_types::{OpticalImage, OpticalImageError, OpticalImageRequest, OpticalPixel},
    prepared::PreparedEval,
    scintillation::{
        SCINT_TEMPORAL_YAW_OFFSETS_DEG, combine_scintillation_pct, spatial_scintillation_pct,
        temporal_scintillation_pct,
    },
};
use crate::optics::raytracer::{Camera, DEFAULT_FOV_DEG};

/// Number of yaw poses of the temporal term (the grid's own offsets).
const POSES: u8 = 5;
/// The pose whose offset is 0 degrees: the base camera of the map.
const BASE_POSE: usize = 2;
/// Half width of the spatial window, in map pixels (a 5x5 window). An export that cuts the
/// picture into tiles traces this many extra rows on each side of a tile.
pub const SCINTILLATION_WINDOW_RADIUS: usize = 2;

/// The five offset cameras of the temporal term for the orbit pose `[yaw, pitch, distance]`.
///
/// Each one is `Camera::new(yaw + offset, pitch, distance, ..)` with the very yaw
/// expression the grid's `TemporalPoseContext` uses, so pose `i` of the image domain is pose
/// `i` of the grid. The field of view is the request camera's own (`fov_tan`), so the base
/// pose reproduces the map's rays exactly.
pub(super) fn temporal_cameras(pose: [f32; 3], fov_tan: f32) -> [Camera; 5] {
    SCINT_TEMPORAL_YAW_OFFSETS_DEG.map(|offset_deg| {
        let mut camera = Camera::new(
            pose[0] + offset_deg.to_radians(),
            pose[1],
            pose[2],
            DEFAULT_FOV_DEG,
        );
        camera.fov_tan = fov_tan;
        camera
    })
}

/// Everything the temporal term reads that is the same for every pixel of one evaluation.
pub(super) struct TemporalSampler<'a> {
    /// A Fire-less classification context: the companion traces must not run for the
    /// offset rays.
    ctx: ApertureSampleContext<'a>,
    cameras: [Camera; 5],
    projection: [u32; 2],
    origin: [u32; 2],
    scale: [f32; 2],
}

impl<'a> TemporalSampler<'a> {
    /// A sampler for `request` (whose `pose` the caller has already validated) against the
    /// prepared stone; `projection`, `origin` and `scale` are the image sampler's own.
    pub(super) fn new(
        request: &OpticalImageRequest<'_>,
        prepared: &'a PreparedEval<'a>,
        pose: [f32; 3],
        projection: [u32; 2],
        origin: [u32; 2],
        scale: [f32; 2],
    ) -> Self {
        let c = request.camera;
        Self {
            ctx: ApertureSampleContext {
                stone: prepared.stone(),
                nd: prepared.nd,
                n_f: prepared.n_f,
                n_c: prepared.n_c,
                cam_forward: c.forward,
                cam_right: c.right,
                cam_up: c.up,
                fan: prepared.fan,
                lighting: &prepared.lighting,
                want_fire: false,
            },
            cameras: temporal_cameras(pose, c.fov_tan),
            projection,
            origin,
            scale,
        }
    }

    /// Whether the centre ray of pixel `(x, y)` under pose `index` is visibly returned;
    /// `None` when it misses the stone.
    fn returned_at(&self, index: usize, x: u32, y: u32) -> Option<bool> {
        // The pixel's centre sample, at the same screen coordinate as sample 0 of the map.
        let (jx, jy) = offset(0);
        let ray = self.cameras[index].generate_ray(
            (x as f32 + jx).mul_add(self.scale[0], self.origin[0] as f32 - 0.5),
            (y as f32 + jy).mul_add(self.scale[1], self.origin[1] as f32 - 0.5),
            self.projection[0] as f32,
            self.projection[1] as f32,
            0.0,
            0.0,
        );
        classify_ray(&self.ctx, ray, -ray.dir, None)
            .map(|class| matches!(class, RayClassification::Returned(_)))
    }

    /// Fills `pixel.scint_returned` and `pixel.scint_poses` for pixel `(x, y)`.
    ///
    /// A pixel whose centre ray misses the stone at the base pose stays at zero poses (not
    /// computed). A miss at another pose is a pose at which no light returns. Cancellation
    /// is checked before every offset.
    pub(super) fn apply(
        &self,
        x: u32,
        y: u32,
        pixel: &mut OpticalPixel,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), OpticalImageError> {
        if cancelled() {
            return Err(OpticalImageError::Cancelled);
        }
        let Some(base) = self.returned_at(BASE_POSE, x, y) else {
            return Ok(());
        };
        let mut returned = u8::from(base);
        for index in (0..self.cameras.len()).filter(|i| *i != BASE_POSE) {
            if cancelled() {
                return Err(OpticalImageError::Cancelled);
            }
            returned += u8::from(self.returned_at(index, x, y) == Some(true));
        }
        pixel.scint_returned = returned;
        pixel.scint_poses = POSES;
        Ok(())
    }
}

/// The brilliance fraction of every hit pixel; `None` where the pixel has no hits.
fn brilliance_fractions(image: &OpticalImage) -> Vec<Option<f32>> {
    image
        .pixels
        .iter()
        .map(|p| (p.hits > 0).then(|| p.fractions()[2]))
        .collect()
}

/// The spatial term of the pixel at `(x, y)` over its 5x5 window of `fractions` (fewer at
/// the silhouette and the frame edge): the module's own squashed coefficient of variation.
fn window_spatial(
    fractions: &[Option<f32>],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> f32 {
    let mut sum = 0.0_f32;
    let mut sum_sq = 0.0_f32;
    let mut count = 0_u32;
    for wy in y.saturating_sub(SCINTILLATION_WINDOW_RADIUS)
        ..(y + SCINTILLATION_WINDOW_RADIUS + 1).min(height)
    {
        for wx in x.saturating_sub(SCINTILLATION_WINDOW_RADIUS)
            ..(x + SCINTILLATION_WINDOW_RADIUS + 1).min(width)
        {
            if let Some(f) = fractions[wy * width + wx] {
                sum += f;
                sum_sq = f.mul_add(f, sum_sq);
                count += 1;
            }
        }
    }
    spatial_scintillation_pct(sum, sum_sq, count)
}

/// The spatial scintillation term of every map pixel, 0 to 100; 0 where `hits == 0`.
///
/// The coefficient of variation of the brilliance fraction (`classes[2] / hits`) over the
/// 5x5 window of map pixels that have hits, squashed through `cv / (1 + cv)` exactly as
/// the grid's `spatial_scintillation_pct`. A map whose size does not match its pixel count
/// reads all zero.
#[must_use]
pub fn scintillation_spatial(image: &OpticalImage) -> Vec<f32> {
    let (width, height) = (image.width as usize, image.height as usize);
    let mut out = vec![0.0; image.pixels.len()];
    if width.checked_mul(height) != Some(image.pixels.len()) {
        return out;
    }
    let fractions = brilliance_fractions(image);
    for y in 0..height {
        for x in 0..width {
            if fractions[y * width + x].is_some() {
                out[y * width + x] = window_spatial(&fractions, width, height, x, y);
            }
        }
    }
    out
}

/// [`scintillation_spatial`] for the single pixel `i` (the hover read-out), without
/// building the whole map. Reads the same window, so it equals the full pass bit for bit.
#[must_use]
pub fn scintillation_spatial_at(image: &OpticalImage, i: usize) -> f32 {
    let (width, height) = (image.width as usize, image.height as usize);
    if width.checked_mul(height) != Some(image.pixels.len()) || i >= image.pixels.len() {
        return 0.0;
    }
    let (col, row) = (i % width, i / width);
    let (x0, x1) = (
        col.saturating_sub(SCINTILLATION_WINDOW_RADIUS),
        (col + SCINTILLATION_WINDOW_RADIUS + 1).min(width),
    );
    let (y0, y1) = (
        row.saturating_sub(SCINTILLATION_WINDOW_RADIUS),
        (row + SCINTILLATION_WINDOW_RADIUS + 1).min(height),
    );
    // Only the window's fractions are needed; lay them out as a small frame so the shared
    // window reader sees the same neighbours in the same order.
    let (frame_w, frame_h) = (x1 - x0, y1 - y0);
    let mut local = Vec::with_capacity(frame_w * frame_h);
    for wy in y0..y1 {
        for wx in x0..x1 {
            let pixel = &image.pixels[wy * width + wx];
            local.push((pixel.hits > 0).then(|| pixel.fractions()[2]));
        }
    }
    if local[(row - y0) * frame_w + (col - x0)].is_none() {
        return 0.0;
    }
    window_spatial(&local, frame_w, frame_h, col - x0, row - y0)
}

/// The scintillation index of pixel `i`, 0 to 100.
///
/// It is the module's 60/40 combination of the pixel's spatial term (`spatial[i]`, from [`scintillation_spatial`]) and its temporal
/// term (the Bernoulli variance of the five poses over the achievable 0.24). `None` where
/// the temporal term was not computed.
#[must_use]
pub fn scintillation_index(image: &OpticalImage, spatial: &[f32], i: usize) -> Option<f32> {
    let variance = image.temporal_variance(i)?;
    let spatial_pct = spatial.get(i).copied().unwrap_or(0.0);
    Some(combine_scintillation_pct(
        spatial_pct,
        temporal_scintillation_pct(variance, 1),
    ))
}

/// [`scintillation_index`] for the single pixel `i` (the hover read-out), without building
/// the whole spatial map; equal to the full pass bit for bit.
#[must_use]
pub fn scintillation_index_at(image: &OpticalImage, i: usize) -> Option<f32> {
    let variance = image.temporal_variance(i)?;
    Some(combine_scintillation_pct(
        scintillation_spatial_at(image, i),
        temporal_scintillation_pct(variance, 1),
    ))
}

/// The image aggregate behind [`OpticalImage::scintillation_pct`]: the spatial term over
/// all hit pixels' brilliance fractions, the temporal term over the pixels that have one.
pub(super) fn image_scintillation_pct(image: &OpticalImage) -> f32 {
    let mut sum = 0.0_f32;
    let mut sum_sq = 0.0_f32;
    let mut count = 0_u32;
    for p in image.pixels.iter().filter(|p| p.hits > 0) {
        let f = p.fractions()[2];
        sum += f;
        sum_sq = f.mul_add(f, sum_sq);
        count += 1;
    }
    let mut variance_sum = 0.0_f32;
    let mut poses_pixels = 0_u32;
    for i in 0..image.pixels.len() {
        if let Some(variance) = image.temporal_variance(i) {
            variance_sum += variance;
            poses_pixels += 1;
        }
    }
    combine_scintillation_pct(
        spatial_scintillation_pct(sum, sum_sq, count),
        temporal_scintillation_pct(variance_sum, poses_pixels),
    )
}

#[cfg(test)]
#[path = "image_scint_tests.rs"]
mod tests;

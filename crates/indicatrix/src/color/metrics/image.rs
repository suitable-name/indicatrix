//! Deterministic perspective sampling using exactly the renderer camera convention.
use super::{
    classify::{ApertureSampleContext, RayClassification, classify_ray},
    image_scint::TemporalSampler,
    image_types::{
        MAX_DIAGNOSTIC_PIXELS, OpticalImage, OpticalImageError, OpticalImageRequest, OpticalPixel,
        RayDetail,
    },
    prepared::PreparedEval,
};

/// Centre first, then a fixed low-discrepancy prefix, independent of scheduling.
///
/// The digit sum stays a plain product added to `y`, never a fused multiply-add: that would
/// re-round it and could move the jitter offsets the optical-image goldens were recorded with.
/// The product is its own binding so no clippy version suggests fusing it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a sub-pixel offset in [0, 1) needs only f32 precision"
)]
pub(super) fn offset(sample: u32) -> (f32, f32) {
    if sample == 0 {
        return (0.5, 0.5);
    }
    let radical = f64::from(sample.reverse_bits()) / 4_294_967_296.0;
    let mut n = sample;
    let mut scale = 1.0 / 3.0;
    let mut y = 0.0;
    while n > 0 {
        let digit = f64::from(n % 3) * scale;
        y += digit;
        n /= 3;
        scale /= 3.0;
    }
    (radical as f32, y as f32)
}

/// Rejects requests the sampler cannot honour, returning the number of pixels to allocate.
fn validate_request(r: &OpticalImageRequest<'_>) -> Result<usize, OpticalImageError> {
    let count = (r.width as usize)
        .checked_mul(r.height as usize)
        .ok_or(OpticalImageError::ResourceLimit)?;
    if count == 0 || count > MAX_DIAGNOSTIC_PIXELS || r.samples == 0 || r.samples > 64 {
        return Err(OpticalImageError::ResourceLimit);
    }
    let projection = r.projection_size.unwrap_or([r.width, r.height]);
    // The offset cameras are rebuilt from the orbit pose, so scintillation cannot run
    // without one (and a degenerate pose cannot build a camera).
    if r.want_scintillation
        && !r
            .pose
            .is_some_and(|pose| pose.iter().all(|v| v.is_finite()) && pose[2] > 0.0)
    {
        return Err(OpticalImageError::InvalidInput);
    }
    if projection.contains(&0)
        || r.geometry.planes.iter().any(|p| {
            !p.d.is_finite()
                || p.normal.iter().any(|n| !n.is_finite())
                || (p.normal.iter().map(|n| n * n).sum::<f32>() - 1.0).abs() > 0.01
        })
        || r.geometry.tools.iter().any(|t| t.validate().is_err())
    {
        return Err(OpticalImageError::InvalidInput);
    }
    if let Some([left, top]) = r.tile_origin
        && (left.checked_add(r.width).is_none_or(|v| v > projection[0])
            || top.checked_add(r.height).is_none_or(|v| v > projection[1]))
    {
        return Err(OpticalImageError::InvalidInput);
    }
    let c = r.camera;
    if ![c.origin, c.forward, c.right, c.up]
        .iter()
        .all(|v| v.is_finite())
        || c.forward.dot(c.right).abs() > 0.001
        || c.forward.dot(c.up).abs() > 0.001
        || c.right.dot(c.up).abs() > 0.001
        || !c.fov_tan.is_finite()
        || c.fov_tan <= 0.0
        || [c.forward, c.right, c.up]
            .iter()
            .any(|v| (v.length_squared() - 1.0).abs() > 0.01)
        || [486.1, 589.3, 656.3].iter().any(|w| {
            let n = r.material.dispersion.evaluate(*w);
            !n.is_finite() || n < 1.0
        })
    {
        return Err(OpticalImageError::InvalidInput);
    }
    Ok(count)
}

/// Everything [`PixelSampler::sample_pixel`] reads that is the same for every pixel of one
/// evaluation.
struct PixelSampler<'a> {
    request: &'a OpticalImageRequest<'a>,
    ctx: &'a ApertureSampleContext<'a>,
    /// Beauty dimensions the camera rays are generated against.
    projection: [u32; 2],
    /// Projection pixel of this image's top-left corner (the tile origin, or zero).
    origin: [u32; 2],
    /// Projection pixels per image pixel: 1 for a tile, the draft upscale otherwise.
    scale: [f32; 2],
    zero_dispersion: bool,
}

impl PixelSampler<'_> {
    /// Classifies every sample of pixel `(x, y)`; `Err` on cancellation.
    fn sample_pixel(
        &self,
        x: u32,
        y: u32,
        cancelled: &impl Fn() -> bool,
    ) -> Result<OpticalPixel, OpticalImageError> {
        let r = self.request;
        let mut pixel = OpticalPixel::default();
        let mut first_facet = None;
        for sample in 0..r.samples {
            if cancelled() {
                return Err(OpticalImageError::Cancelled);
            }
            let (jx, jy) = offset(sample);
            // The renderer centres beauty pixel `x` at screen coordinate `x` (the guide pass
            // and the un-jittered sample generate their rays there), so this pixel's
            // `[x, x + 1)` extent, in edge coordinates, is shifted back by half a pixel.
            let ray = r.camera.generate_ray(
                (x as f32 + jx).mul_add(self.scale[0], self.origin[0] as f32 - 0.5),
                (y as f32 + jy).mul_add(self.scale[1], self.origin[1] as f32 - 0.5),
                self.projection[0] as f32,
                self.projection[1] as f32,
                0.0,
                0.0,
            );
            let mut detail = RayDetail::default();
            // The beauty render centres the head-shadow cone on this pixel's own eye
            // direction, not on the camera axis.
            let Some(class) = classify_ray(self.ctx, ray, -ray.dir, Some(&mut detail)) else {
                continue;
            };
            pixel.hits += 1;
            if sample == 0 {
                pixel.centre = detail;
            }
            if let Some(id) = first_facet {
                pixel.mixed_facets |= detail.entry_facet != Some(id);
            } else {
                first_facet = detail.entry_facet;
            }
            let bucket = match class {
                RayClassification::Windowed => 0,
                RayClassification::Extinct => 1,
                RayClassification::Returned(returned) => {
                    if r.want_fire {
                        if let Some((angle, weight)) = returned.fire {
                            if angle.is_finite() && weight.is_finite() {
                                pixel.fire_sum_deg =
                                    (if self.zero_dispersion { 0.0 } else { angle })
                                        .mul_add(weight, pixel.fire_sum_deg);
                                pixel.fire_qualified += 1;
                            } else {
                                pixel.fire_rejected += 1;
                            }
                        } else {
                            pixel.fire_rejected += 1;
                        }
                    }
                    2
                }
                RayClassification::EntryBlocked | RayClassification::Invalid => 3,
            };
            pixel.classes[bucket] += 1;
        }
        Ok(pixel)
    }
}

/// Computes all base classes together; optional Fire companions only for returned rays.
///
/// Cancellation is checked per pixel and per sample (each path is bounded to ten bounces).
/// The caller chooses a sampling resolution and must label it when exporting an upscale.
///
/// # Errors
///
/// Returns an [`OpticalImageError`] when the request is invalid or exceeds the resource
/// limits, when the geometry is empty, or when `cancelled` reports cancellation.
pub fn evaluate_optical_image(
    request: &OpticalImageRequest<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<OpticalImage, OpticalImageError> {
    let r = request;
    let count = validate_request(r)?;
    let projection = r.projection_size.unwrap_or([r.width, r.height]);
    let c = r.camera;
    if cancelled() {
        return Err(OpticalImageError::Cancelled);
    }
    let prepared = PreparedEval::new(r.geometry, r.material, r.environment)
        .ok_or(OpticalImageError::EmptyGeometry)?;
    let ctx = ApertureSampleContext {
        stone: prepared.stone(),
        nd: prepared.nd,
        n_f: prepared.n_f,
        n_c: prepared.n_c,
        cam_forward: c.forward,
        cam_right: c.right,
        cam_up: c.up,
        fan: prepared.fan,
        lighting: &prepared.lighting,
        want_fire: r.want_fire,
    };
    let mut image = OpticalImage {
        width: r.width,
        height: r.height,
        samples: r.samples,
        fire_computed: r.want_fire,
        scint_computed: r.want_scintillation,
        origin: r.tile_origin.unwrap_or([0, 0]),
        pixels: Vec::with_capacity(count),
        total: OpticalPixel::default(),
    };
    let scale = if r.tile_origin.is_some() {
        [1.0, 1.0]
    } else {
        [
            projection[0] as f32 / r.width as f32,
            projection[1] as f32 / r.height as f32,
        ]
    };
    let sampler = PixelSampler {
        request: r,
        ctx: &ctx,
        projection,
        origin: image.origin,
        scale,
        zero_dispersion: prepared.n_f.to_bits() == prepared.n_c.to_bits(),
    };
    // Validation guarantees a pose whenever scintillation is requested.
    let temporal = r
        .pose
        .filter(|_| r.want_scintillation)
        .map(|pose| TemporalSampler::new(r, &prepared, pose, projection, image.origin, scale));
    for y in 0..r.height {
        for x in 0..r.width {
            let mut pixel = sampler.sample_pixel(x, y, &cancelled)?;
            if pixel.hits > 0
                && let Some(temporal) = &temporal
            {
                temporal.apply(x, y, &mut pixel, &cancelled)?;
            }
            image.total.hits += pixel.hits;
            for (sum, n) in image.total.classes.iter_mut().zip(pixel.classes) {
                *sum += n;
            }
            image.total.fire_sum_deg += pixel.fire_sum_deg;
            image.total.fire_qualified += pixel.fire_qualified;
            image.total.fire_rejected += pixel.fire_rejected;
            image.pixels.push(pixel);
        }
    }
    Ok(image)
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;

//! The working resolution: area-averaged resampling of the stone's region.
//!
//! The fit does not run on 12 million pixels. The stone's bounding region is cut into a grid of
//! at most `N` pixels across (default [`DEFAULT_WORKING_PX`]); every working pixel is the
//! exact box average of the full-resolution pixels under its footprint (fractional overlaps
//! are weighted by area). The forward model later samples rays over the same footprint, so the
//! footprint is kept: [`WorkingGrid::footprint`].
//!
//! Noise variances are propagated through the averaging for independent pixels:
//! `var(mean) = sum(w_k^2 var_k) / (sum w_k)^2`.

use glam::DVec2;

use super::{
    linear::PhotometryError,
    mask::{PixelMask, flag},
};

/// The default longest side of the working grid, in pixels.
pub const DEFAULT_WORKING_PX: usize = 384;

/// The working grid of one view: where each working pixel sits in the full-resolution photo.
///
/// Coordinates are those of the camera model: `u` right, `v` down, the full-resolution
/// pixel `(i, j)` covering `[i, i + 1) x [j, j + 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkingGrid {
    /// Working pixels across.
    pub width: usize,
    /// Working pixels down.
    pub height: usize,
    /// The full-resolution position of the top-left corner of working pixel `(0, 0)`.
    pub origin: [f64; 2],
    /// Full-resolution pixels per working pixel (1 when the region is small enough to keep).
    pub scale: f64,
}

impl WorkingGrid {
    /// The grid for the full-resolution region `[x0, y0, x1, y1)` with at most `max_px` pixels
    /// along its longer side (never upsampled).
    ///
    /// # Errors
    ///
    /// [`PhotometryError::Invalid`] for an empty region or `max_px == 0`.
    pub fn fit(region: [usize; 4], max_px: usize) -> Result<Self, PhotometryError> {
        let [x0, y0, x1, y1] = region;
        if x1 <= x0 || y1 <= y0 || max_px == 0 {
            return Err(PhotometryError::Invalid(
                "empty region or zero working size".to_owned(),
            ));
        }
        let (w, h) = ((x1 - x0) as f64, (y1 - y0) as f64);
        let scale = (w.max(h) / max_px as f64).max(1.0);
        Ok(Self {
            width: ((w / scale) - 1e-9).ceil().max(1.0) as usize,
            height: ((h / scale) - 1e-9).ceil().max(1.0) as usize,
            origin: [x0 as f64, y0 as f64],
            scale,
        })
    }

    /// The full-resolution rectangle `[u0, v0, u1, v1)` that working pixel `(x, y)` covers.
    /// The last row and column may reach past the region.
    #[must_use]
    pub fn footprint(&self, x: usize, y: usize) -> [f64; 4] {
        let u0 = (x as f64).mul_add(self.scale, self.origin[0]);
        let v0 = (y as f64).mul_add(self.scale, self.origin[1]);
        [u0, v0, u0 + self.scale, v0 + self.scale]
    }

    /// The full-resolution position of the centre of working pixel `(x, y)`.
    #[must_use]
    pub fn centre(&self, x: usize, y: usize) -> DVec2 {
        let [u0, v0, u1, v1] = self.footprint(x, y);
        DVec2::new(f64::midpoint(u0, u1), f64::midpoint(v0, v1))
    }

    /// The working-grid coordinates (fractional, pixel `(i, j)` covering `[i, i + 1)`) of a
    /// full-resolution position; `None` when outside the grid.
    #[must_use]
    pub fn working_coordinates(&self, at: DVec2) -> Option<(f64, f64)> {
        let x = (at.x - self.origin[0]) / self.scale;
        let y = (at.y - self.origin[1]) / self.scale;
        (x >= 0.0 && y >= 0.0 && x < self.width as f64 && y < self.height as f64).then_some((x, y))
    }
}

/// Settings of [`resample`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResampleOptions {
    /// Source pixels carrying any of these flags do not enter the average.
    pub exclude: u8,
    /// A working pixel whose valid share of the footprint is below this is flagged with the
    /// flags of the excluded pixels under it (default 0.5).
    pub min_coverage: f32,
}

impl Default for ResampleOptions {
    fn default() -> Self {
        Self {
            exclude: flag::SATURATED | flag::BELOW_NOISE | flag::USER,
            min_coverage: 0.5,
        }
    }
}

/// A full-resolution image to resample.
#[derive(Debug, Clone, Copy)]
pub struct ResampleSource<'a> {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The values, row-major.
    pub values: &'a [[f32; 3]],
    /// The per-channel variances, row-major; zero when `None`.
    pub variance: Option<&'a [[f32; 3]]>,
    /// The exclusion mask, when there is one.
    pub mask: Option<&'a PixelMask>,
}

/// A resampled image on a [`WorkingGrid`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResampledImage {
    /// The grid.
    pub grid: WorkingGrid,
    /// The averaged values. Zero where nothing valid lay under the footprint.
    pub values: Vec<[f32; 3]>,
    /// The variance of each average; infinite where nothing valid lay under the footprint.
    pub variance: Vec<[f32; 3]>,
    /// The valid share of each footprint, in `[0, 1]`.
    pub coverage: Vec<f32>,
    /// The flags: those of the excluded source pixels where coverage fell below the minimum.
    pub mask: PixelMask,
}

impl ResampledImage {
    /// The index of working pixel `(x, y)`.
    #[must_use]
    pub const fn index(&self, x: usize, y: usize) -> usize {
        y * self.grid.width + x
    }
}

/// Area-averages `source` onto `grid`.
///
/// # Errors
///
/// [`PhotometryError::Invalid`] when the arrays do not match the stated size;
/// [`PhotometryError::SizeMismatch`] when the mask does not.
pub fn resample(
    source: &ResampleSource<'_>,
    grid: &WorkingGrid,
    options: &ResampleOptions,
) -> Result<ResampledImage, PhotometryError> {
    let (w, h) = (source.width, source.height);
    if w == 0
        || h == 0
        || source.values.len() != w * h
        || source.variance.is_some_and(|v| v.len() != w * h)
    {
        return Err(PhotometryError::Invalid(
            "resample source arrays do not match its size".to_owned(),
        ));
    }
    if let Some(mask) = source.mask
        && (mask.width() != w || mask.height() != h)
    {
        return Err(PhotometryError::SizeMismatch {
            expected: [w, h],
            found: [mask.width(), mask.height()],
        });
    }
    let count = grid.width * grid.height;
    let mut values = Vec::with_capacity(count);
    let mut variance = Vec::with_capacity(count);
    let mut coverage = Vec::with_capacity(count);
    let mut mask = PixelMask::new(grid.width, grid.height);
    for gy in 0..grid.height {
        for gx in 0..grid.width {
            let [u0, v0, u1, v1] = grid.footprint(gx, gy);
            let (u0, u1) = (u0.max(0.0), u1.min(w as f64));
            let (v0, v1) = (v0.max(0.0), v1.min(h as f64));
            let mut total = 0.0_f64;
            let mut valid = 0.0_f64;
            let mut sum = [0.0_f64; 3];
            let mut var_sum = [0.0_f64; 3];
            let mut excluded_flags = 0_u8;
            if u1 > u0 && v1 > v0 {
                for j in v0.floor() as usize..(v1.ceil() as usize).min(h) {
                    let wy = (v1.min(j as f64 + 1.0) - v0.max(j as f64)).max(0.0);
                    for i in u0.floor() as usize..(u1.ceil() as usize).min(w) {
                        let wx = (u1.min(i as f64 + 1.0) - u0.max(i as f64)).max(0.0);
                        let weight = wx * wy;
                        if weight <= 0.0 {
                            continue;
                        }
                        total += weight;
                        let at = j * w + i;
                        let bits = source.mask.map_or(0, |m| m.bits()[at]);
                        if bits & options.exclude != 0 {
                            excluded_flags |= bits & options.exclude;
                            continue;
                        }
                        valid += weight;
                        for c in 0..3 {
                            sum[c] = f64::mul_add(weight, f64::from(source.values[at][c]), sum[c]);
                            if let Some(var) = source.variance {
                                var_sum[c] = f64::mul_add(
                                    weight * weight,
                                    f64::from(var[at][c]),
                                    var_sum[c],
                                );
                            }
                        }
                    }
                }
            }
            let share = if total > 0.0 { valid / total } else { 0.0 };
            coverage.push(share as f32);
            if valid > 0.0 {
                values.push([
                    (sum[0] / valid) as f32,
                    (sum[1] / valid) as f32,
                    (sum[2] / valid) as f32,
                ]);
                let scale = 1.0 / (valid * valid);
                variance.push([
                    (var_sum[0] * scale) as f32,
                    (var_sum[1] * scale) as f32,
                    (var_sum[2] * scale) as f32,
                ]);
            } else {
                values.push([0.0; 3]);
                variance.push([f32::INFINITY; 3]);
            }
            if share < f64::from(options.min_coverage) {
                let bits = if total <= 0.0 {
                    flag::OUTSIDE_OUTLINE
                } else {
                    excluded_flags
                };
                mask.set(gx, gy, bits);
            }
        }
    }
    Ok(ResampledImage {
        grid: *grid,
        values,
        variance,
        coverage,
        mask,
    })
}

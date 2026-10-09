//! Per-pixel exclusion masks.
//!
//! A [`PixelMask`] holds one byte of [`flag`] bits per pixel. A pixel with no bit set is used by
//! the fit; any bit excludes it, and the bit says why (the report counts them).
//!
//! The geometric masks (outside the outline, the edge band, inclusions, ghost images) are
//! computed in the **working grid** ([`WorkingGrid`]) of a view from the aligned mesh, using the
//! locate module's `Scene` and `trace_pixel`: a pixel is outside when its camera ray misses the
//! stone, an inclusion when any of its traced interior legs passes within the inclusion's
//! radius, and a ghost when it lies within a ghost radius of a pixel that
//! `predict_ghosts` lists for a located inclusion.

use glam::{DVec2, DVec3};

use super::{
    linear::{LinearImage, PhotometryError},
    resample::WorkingGrid,
};
use crate::rough_plan::locate::{InclusionShell, Scene, predict_ghosts, trace_pixel};

/// The bit flags of a [`PixelMask`].
pub mod flag {
    /// At or above the saturation level in some channel.
    pub const SATURATED: u8 = 1;
    /// The backlight minus the dark level is below the noise floor.
    pub const BELOW_NOISE: u8 = 1 << 1;
    /// The camera ray misses the stone (or cannot enter it).
    pub const OUTSIDE_OUTLINE: u8 = 1 << 2;
    /// Within `k` pixels of the outline.
    pub const EDGE_BAND: u8 = 1 << 3;
    /// The ray passes through a located inclusion.
    pub const INCLUSION: u8 = 1 << 4;
    /// A predicted ghost image of an inclusion.
    pub const GHOST: u8 = 1 << 5;
    /// Painted by the user.
    pub const USER: u8 = 1 << 6;
    /// Every flag.
    pub const ALL: u8 =
        SATURATED | BELOW_NOISE | OUTSIDE_OUTLINE | EDGE_BAND | INCLUSION | GHOST | USER;
}

/// One byte of [`flag`] bits per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelMask {
    width: usize,
    height: usize,
    bits: Vec<u8>,
}

impl PixelMask {
    /// A clear mask.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            bits: vec![0; width * height],
        }
    }

    /// The width in pixels.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// The height in pixels.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// The raw bytes, row-major.
    #[must_use]
    pub fn bits(&self) -> &[u8] {
        &self.bits
    }

    /// The flags of pixel `(x, y)`.
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.bits[y * self.width + x]
    }

    /// Sets `flags` on pixel `(x, y)`.
    pub fn set(&mut self, x: usize, y: usize, flags: u8) {
        self.bits[y * self.width + x] |= flags;
    }

    /// Whether any of `flags` is set on pixel `(x, y)`.
    #[must_use]
    pub fn has(&self, x: usize, y: usize, flags: u8) -> bool {
        self.get(x, y) & flags != 0
    }

    /// Whether the pixel is free of every flag, so the fit may use it.
    #[must_use]
    pub fn is_clear(&self, x: usize, y: usize) -> bool {
        self.get(x, y) == 0
    }

    /// How many pixels carry any of `flags`.
    #[must_use]
    pub fn count(&self, flags: u8) -> usize {
        self.bits.iter().filter(|&&b| b & flags != 0).count()
    }

    /// How many pixels are free of every flag.
    #[must_use]
    pub fn clear_count(&self) -> usize {
        self.bits.iter().map(|&b| usize::from(b == 0)).sum()
    }

    /// Paints or erases the user flag on a pixel.
    pub fn set_user(&mut self, x: usize, y: usize, on: bool) {
        let bits = &mut self.bits[y * self.width + x];
        if on {
            *bits |= flag::USER;
        } else {
            *bits &= !flag::USER;
        }
    }

    /// ORs another mask of the same size into this one.
    ///
    /// # Errors
    ///
    /// [`PhotometryError::SizeMismatch`].
    pub fn merge(&mut self, other: &Self) -> Result<(), PhotometryError> {
        if self.width != other.width || self.height != other.height {
            return Err(PhotometryError::SizeMismatch {
                expected: [self.width, self.height],
                found: [other.width, other.height],
            });
        }
        for (mine, theirs) in self.bits.iter_mut().zip(&other.bits) {
            *mine |= *theirs;
        }
        Ok(())
    }

    /// The saturated pixels of an image: any channel at or above `fraction` of its full scale.
    #[must_use]
    pub fn from_saturation(image: &LinearImage, fraction: f32) -> Self {
        let limit = image.full_scale * fraction;
        let mut mask = Self::new(image.width, image.height);
        for (bits, pixel) in mask.bits.iter_mut().zip(&image.pixels) {
            if pixel.iter().any(|&v| v >= limit) {
                *bits |= flag::SATURATED;
            }
        }
        mask
    }
}

/// An inclusion as the masks need it: a sphere in the MESH frame (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InclusionMarker {
    /// The centre in the mesh frame.
    pub centre: DVec3,
    /// The radius to keep clear, in mm (the shell's reach plus its margin).
    pub radius_mm: f64,
}

impl InclusionMarker {
    /// The marker of a located inclusion's shell: the centroid of its corners, and the largest
    /// corner distance plus the shell's margin as the radius.
    ///
    /// `None` for a shell without corners.
    #[must_use]
    pub fn from_shell(shell: &InclusionShell) -> Option<Self> {
        if shell.points.is_empty() {
            return None;
        }
        let centre = shell.points.iter().copied().sum::<DVec3>() / shell.points.len() as f64;
        let reach = shell
            .points
            .iter()
            .map(|p| (*p - centre).length())
            .fold(0.0, f64::max);
        Some(Self {
            centre,
            radius_mm: reach + shell.margin_mm,
        })
    }
}

/// Settings of [`mesh_masks`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshMaskOptions {
    /// Width of the edge band in working pixels (default 3).
    pub edge_band_px: usize,
    /// Most internal reflections the inclusion test follows (default 2).
    pub max_bounces: u8,
    /// A ghost counts when its ray misses the inclusion centre by less than this, mm
    /// (default 0.5).
    pub ghost_max_miss_mm: f64,
    /// Radius around a ghost pixel that is masked, in FULL-resolution pixels (default 4).
    pub ghost_radius_px: f64,
}

impl Default for MeshMaskOptions {
    fn default() -> Self {
        Self {
            edge_band_px: 3,
            max_bounces: 2,
            ghost_max_miss_mm: 0.5,
            ghost_radius_px: 4.0,
        }
    }
}

/// The pixel rectangle `[x0, y0, x1, y1)` (full resolution) that the stone covers in `view`:
/// the projected corners of the mesh's bounding box, grown by `margin` of their extent, clipped to
/// the image.
///
/// `None` when a corner is behind the camera, the view does not exist, or the rectangle is
/// empty.
#[must_use]
pub fn stone_region(scene: &Scene<'_>, view: usize, margin: f64) -> Option<[usize; 4]> {
    let pose = scene.rig.views.get(view)?;
    let (lo, hi) = scene.mesh.bounds();
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for corner in 0..8_u32 {
        let pick = |bit: u32, low: f64, high: f64| if bit == 0 { low } else { high };
        let point = DVec3::new(
            pick(corner & 1, lo.x, hi.x),
            pick((corner >> 1) & 1, lo.y, hi.y),
            pick((corner >> 2) & 1, lo.z, hi.z),
        );
        let pixel = pose.project(scene.alignment.to_rig(point))?;
        min = min.min(pixel);
        max = max.max(pixel);
    }
    let grow = (max - min) * margin;
    let (min, max) = (min - grow, max + grow);
    let clip = |v: f64, limit: u32| v.clamp(0.0, f64::from(limit));
    let x0 = clip(min.x, pose.image_size[0]).floor() as usize;
    let y0 = clip(min.y, pose.image_size[1]).floor() as usize;
    let x1 = clip(max.x, pose.image_size[0]).ceil() as usize;
    let y1 = clip(max.y, pose.image_size[1]).ceil() as usize;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

/// Computes the outline, edge-band, inclusion and ghost flags of `view` on `grid`.
///
/// Every working pixel is sampled at the centre of its footprint. A pixel whose camera ray
/// cannot enter the stone (a miss, a hit from behind or a total reflection at entry) gets
/// [`flag::OUTSIDE_OUTLINE`]; inside pixels within `edge_band_px` (Euclidean) of such a pixel get
/// [`flag::EDGE_BAND`]; the rest are tested against each marker.
#[must_use]
pub fn mesh_masks(
    scene: &Scene<'_>,
    view: usize,
    grid: &WorkingGrid,
    inclusions: &[InclusionMarker],
    options: &MeshMaskOptions,
) -> PixelMask {
    let (width, height) = (grid.width, grid.height);
    let mut mask = PixelMask::new(width, height);
    let mut inside = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let centre = grid.centre(x, y);
            let Ok(mut best) = trace_pixel(scene, view, centre, 0) else {
                mask.set(x, y, flag::OUTSIDE_OUTLINE);
                continue;
            };
            inside[y * width + x] = true;
            for bounces in 1..=options.max_bounces {
                match trace_pixel(scene, view, centre, bounces) {
                    Ok(path) => best = path,
                    Err(_) => break,
                }
            }
            let hit = inclusions.iter().any(|marker| {
                best.legs
                    .iter()
                    .any(|leg| leg.distance_to(marker.centre) <= marker.radius_mm)
            });
            if hit {
                mask.set(x, y, flag::INCLUSION);
            }
        }
    }
    mark_edge_band(&mut mask, &inside, options.edge_band_px);
    mark_ghosts(&mut mask, scene, view, grid, inclusions, options);
    mask
}

/// Flags the inside pixels within `radius` working pixels of an outside pixel.
fn mark_edge_band(mask: &mut PixelMask, inside: &[bool], radius: usize) {
    if radius == 0 {
        return;
    }
    let (width, height) = (mask.width, mask.height);
    let limit = (radius * radius) as isize;
    let reach = radius as isize;
    for y in 0..height {
        for x in 0..width {
            if !inside[y * width + x] {
                continue;
            }
            let mut near_outside = false;
            'search: for dy in -reach..=reach {
                for dx in -reach..=reach {
                    if dx * dx + dy * dy > limit {
                        continue;
                    }
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    if !inside[ny as usize * width + nx as usize] {
                        near_outside = true;
                        break 'search;
                    }
                }
            }
            if near_outside {
                mask.set(x, y, flag::EDGE_BAND);
            }
        }
    }
}

/// Flags the pixels near each predicted ghost of each inclusion.
fn mark_ghosts(
    mask: &mut PixelMask,
    scene: &Scene<'_>,
    view: usize,
    grid: &WorkingGrid,
    inclusions: &[InclusionMarker],
    options: &MeshMaskOptions,
) {
    let radius_work = (options.ghost_radius_px / grid.scale).max(0.5);
    for marker in inclusions {
        for ghost in predict_ghosts(
            scene,
            view,
            marker.centre,
            options.max_bounces,
            options.ghost_max_miss_mm,
        ) {
            let at = DVec2::from_array(ghost.pixel);
            let Some((gx, gy)) = grid.working_coordinates(at) else {
                continue;
            };
            let reach = radius_work.ceil() as isize + 1;
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (x, y) = (gx.floor() as isize + dx, gy.floor() as isize + dy);
                    if x < 0 || y < 0 || x >= mask.width as isize || y >= mask.height as isize {
                        continue;
                    }
                    let centre = (x as f64 + 0.5 - gx, y as f64 + 0.5 - gy);
                    if centre.0.hypot(centre.1) <= radius_work {
                        mask.set(x as usize, y as usize, flag::GHOST);
                    }
                }
            }
        }
    }
}

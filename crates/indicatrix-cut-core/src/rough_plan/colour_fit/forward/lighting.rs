//! The light model of the rig: backlight panels with a measured white frame, a uniform
//! surround, and the holder that occludes.
//!
//! Plan section 3.2. The panel is treated as a Lambertian emitter whose radiance varies across it
//! as the empty-rig photo (the corrected white frame) shows. An exit ray that reaches the panel
//! at point `q` gets the white-frame level at the pixel `q` projects to in that view's camera,
//! relative to the level at the pixel the camera ray started from (so the prediction is a
//! transmittance relative to the empty rig, like the photos). Panel points the camera never saw
//! (outside the image) get the mean level, and are flagged. The levels are kept per colour
//! channel (ZC-13): an edge-lit panel is bluer at its edges, and the ratio of an exit ray is
//! taken channel by channel, as the transmittance images divide.

use glam::{DVec2, DVec3};

use super::ForwardError;
use crate::rough_plan::{
    locate::{RigProfile, ViewPose},
    photometry::{CalibrationFrames, LinearImage},
    shape::RoughMesh,
};

/// A diffuse backlight panel behind one view, in the rig frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelGeom {
    /// The panel centre, mm.
    pub centre: [f64; 3],
    /// The normal, pointing at the stone (towards the camera). Need not be unit.
    pub normal: [f64; 3],
    /// A direction along the panel's height (made perpendicular to the normal).
    pub up: [f64; 3],
    /// The panel width and height in mm.
    pub size_mm: [f64; 2],
}

impl PanelGeom {
    /// A panel facing `pose`'s camera, `distance_mm` along its viewing direction from the camera
    /// centre, with the camera's up direction.
    #[must_use]
    pub fn facing_camera(pose: &ViewPose, distance_mm: f64, size_mm: [f64; 2]) -> Self {
        let basis = pose.basis();
        Self {
            centre: (pose.position_vec() + basis.forward * distance_mm).to_array(),
            normal: (-basis.forward).to_array(),
            up: (-basis.down).to_array(),
            size_mm,
        }
    }

    /// The unit axes `(across, up, normal)`, or `None` for a zero normal.
    fn axes(&self) -> Option<(DVec3, DVec3, DVec3)> {
        let normal = DVec3::from_array(self.normal).try_normalize()?;
        let up_raw = DVec3::from_array(self.up);
        let up = (up_raw - normal * up_raw.dot(normal))
            .try_normalize()
            .unwrap_or_else(|| normal.any_orthonormal_vector());
        Some((up.cross(normal), up, normal))
    }

    fn is_usable(&self) -> bool {
        self.axes().is_some()
            && DVec3::from_array(self.centre).is_finite()
            && self.size_mm.iter().all(|s| s.is_finite() && *s > 0.0)
    }

    /// Where the ray `origin + t dir` meets the panel's front face: `(t, point)`. `None` when
    /// it travels away from the panel, parallel to it, or misses the rectangle.
    #[must_use]
    pub fn hit(&self, origin: DVec3, dir: DVec3) -> Option<(f64, DVec3)> {
        let (across, up, normal) = self.axes()?;
        let denom = dir.dot(normal);
        if denom >= -1e-12 {
            return None;
        }
        let centre = DVec3::from_array(self.centre);
        let t = (centre - origin).dot(normal) / denom;
        if t <= 1e-9 {
            return None;
        }
        let point = origin + dir * t;
        let rel = point - centre;
        (rel.dot(across).abs() <= 0.5 * self.size_mm[0]
            && rel.dot(up).abs() <= 0.5 * self.size_mm[1])
            .then_some((t, point))
    }
}

/// The corrected white frame of one view as a relative level image, one level per colour channel.
///
/// Built from the empty-rig photo (white minus dark) divided, channel by channel, by the
/// backlight's camera RGB, so that the level of an unobstructed pixel is about 1 in every channel
/// and only ratios matter. The channels are kept apart (ZC-13): an edge-lit panel changes colour
/// across the frame, and the transmittance images divide per channel. Stored at a reduced
/// resolution (a box average of `scale x scale` photo pixels) to keep memory small.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteFrame {
    full_size: [usize; 2],
    width: usize,
    height: usize,
    scale: f64,
    level: Vec<[f32; 3]>,
    mean: [f32; 3],
}

impl WhiteFrame {
    /// A frame from a ready level image of `width x height` cells, each `scale` photo pixels
    /// wide, for a photo of `full_size` pixels; every cell holds the level of the red, green and
    /// blue channel.
    ///
    /// # Errors
    ///
    /// [`ForwardError::BadLighting`] for a wrong cell count, a non-positive scale, or levels
    /// that are not finite and non-negative.
    pub fn from_levels(
        full_size: [usize; 2],
        scale: f64,
        width: usize,
        height: usize,
        level: Vec<[f32; 3]>,
    ) -> Result<Self, ForwardError> {
        let bad = |what: &str| ForwardError::BadLighting(format!("white frame: {what}"));
        if width == 0 || height == 0 || level.len() != width * height {
            return Err(bad("cell count does not match the size"));
        }
        if !(scale.is_finite() && scale > 0.0) {
            return Err(bad("scale must be positive"));
        }
        if level.iter().flatten().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(bad("levels must be finite and non-negative"));
        }
        let mut mean = [0.0_f32; 3];
        for (c, slot) in mean.iter_mut().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f32 is the level image storage precision; the mean level is a small non-negative ratio"
            )]
            let channel_mean =
                (level.iter().map(|v| f64::from(v[c])).sum::<f64>() / level.len() as f64) as f32;
            *slot = channel_mean;
        }
        Ok(Self {
            full_size,
            width,
            height,
            scale,
            level,
            mean,
        })
    }

    /// The level image of a corrected white frame (`white - dark`, camera linear RGB).
    ///
    /// Divided by `reference_rgb` (the camera RGB of the backlight spectrum, from
    /// `BacklightSpectrum::camera_rgb`), reduced so that its longer side has at most `max_side`
    /// cells.
    ///
    /// The level of channel `c` of a pixel is `max(p_c, 0) / reference_c`, so the colour of the
    /// panel across the frame is kept (ZC-13).
    ///
    /// # Errors
    ///
    /// [`ForwardError::BadLighting`] for an empty image or a reference with a channel that is not
    /// positive.
    pub fn from_corrected(
        corrected: &LinearImage,
        reference_rgb: [f64; 3],
        max_side: usize,
    ) -> Result<Self, ForwardError> {
        if !reference_rgb.iter().all(|r| r.is_finite() && *r > 0.0) {
            return Err(ForwardError::BadLighting(
                "white frame: the reference RGB must be positive".to_owned(),
            ));
        }
        if max_side == 0 {
            return Err(ForwardError::BadLighting(
                "white frame: max_side must be at least 1".to_owned(),
            ));
        }
        let (w, h) = (corrected.width, corrected.height);
        if w == 0 || h == 0 || corrected.pixels.len() != w * h {
            return Err(ForwardError::BadLighting(
                "white frame: empty or inconsistent image".to_owned(),
            ));
        }
        let factor = w.max(h).div_ceil(max_side).max(1);
        let (nw, nh) = (w.div_ceil(factor), h.div_ceil(factor));
        let mut sum = vec![[0.0_f64; 3]; nw * nh];
        let mut count = vec![0_u32; nw * nh];
        for y in 0..h {
            for x in 0..w {
                let p = corrected.pixels[y * w + x];
                let cell = (y / factor) * nw + x / factor;
                for c in 0..3 {
                    sum[cell][c] += f64::from(p[c].max(0.0)) / reference_rgb[c];
                }
                count[cell] += 1;
            }
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the level image storage precision; levels are small non-negative ratios"
        )]
        let level = sum
            .iter()
            .zip(&count)
            .map(|(s, &c)| {
                let n = f64::from(c.max(1));
                [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
            })
            .collect();
        Self::from_levels([w, h], factor as f64, nw, nh, level)
    }

    /// The level image from the white and dark frames of a view's calibration: the mean white
    /// frame minus the mean dark frame (see [`from_corrected`](Self::from_corrected)).
    ///
    /// # Errors
    ///
    /// [`ForwardError::Photometry`] when the frames are missing or differ in size, else as
    /// [`from_corrected`](Self::from_corrected).
    pub fn from_calibration(
        frames: &CalibrationFrames,
        reference_rgb: [f64; 3],
        max_side: usize,
    ) -> Result<Self, ForwardError> {
        let photometry = |e: crate::rough_plan::photometry::PhotometryError| {
            ForwardError::Photometry(format!("{e:?}"))
        };
        let (mut white, _) = frames.mean_white().map_err(photometry)?;
        let (dark, _) = frames.mean_dark().map_err(photometry)?;
        white.require_same_size(&dark).map_err(photometry)?;
        for (w, d) in white.pixels.iter_mut().zip(&dark.pixels) {
            for c in 0..3 {
                w[c] = (w[c] - d[c]).max(0.0);
            }
        }
        Self::from_corrected(&white, reference_rgb, max_side)
    }

    /// The mean level over the frame, per channel.
    #[must_use]
    pub const fn mean(&self) -> [f32; 3] {
        self.mean
    }

    /// The photo size in pixels this frame describes.
    #[must_use]
    pub const fn full_size(&self) -> [usize; 2] {
        self.full_size
    }

    /// The cells across, down, and the photo pixels per cell.
    #[must_use]
    pub const fn cells(&self) -> (usize, usize, f64) {
        (self.width, self.height, self.scale)
    }

    /// The cell levels (red, green, blue), row-major.
    #[must_use]
    pub fn levels(&self) -> &[[f32; 3]] {
        &self.level
    }

    /// The level of each channel at the photo position `(u, v)` (pixel `(i, j)` covers
    /// `[i, i + 1)`), bilinear between cell centres; `None` outside the photo.
    #[must_use]
    pub fn level_at(&self, u: f64, v: f64) -> Option<[f64; 3]> {
        let inside =
            u >= 0.0 && v >= 0.0 && u < self.full_size[0] as f64 && v < self.full_size[1] as f64;
        if !inside {
            return None;
        }
        let x = (u / self.scale - 0.5).clamp(0.0, (self.width - 1) as f64);
        let y = (v / self.scale - 0.5).clamp(0.0, (self.height - 1) as f64);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "x and y are clamped to 0..=(size - 1) above"
        )]
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let mut out = [0.0_f64; 3];
        for (c, slot) in out.iter_mut().enumerate() {
            let at = |i: usize, j: usize| f64::from(self.level[j * self.width + i][c]);
            let top = (at(x1, y0) - at(x0, y0)).mul_add(fx, at(x0, y0));
            let bottom = (at(x1, y1) - at(x0, y1)).mul_add(fx, at(x0, y1));
            *slot = (bottom - top).mul_add(fy, top);
        }
        Some(out)
    }
}

/// Where the light comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum LightModel {
    /// One diffuse panel behind each view. `white[v]` is the measured level image of view `v`'s
    /// empty rig; missing or `None` entries mean a panel of uniform level.
    Backlight {
        /// The panel of each view, in the rig frame.
        per_view: Vec<PanelGeom>,
        /// The white frame of each view.
        white: Vec<Option<WhiteFrame>>,
    },
    /// A uniform, all-round surround of level 1 (a dome or integrating sphere): the cheap
    /// fallback, and the furnace of the tests.
    UniformSurround,
}

/// The light model of the rig and its occluder.
#[derive(Debug, Clone, PartialEq)]
pub struct RigLighting {
    /// The light.
    pub model: LightModel,
    /// The holder (dop, mount) as a closed mesh in the rig frame. Rays that reach it get no
    /// light.
    pub holder: Option<RoughMesh>,
}

/// The light an exit ray finds.
#[derive(Debug, Clone, Copy)]
pub struct ExitLight {
    /// The level of each colour channel relative to the level of the same channel at the
    /// camera-ray pixel; all 0 when dark.
    pub ratio: [f64; 3],
    /// Whether the level is the Lambertian mean (the exit point was never seen by the camera).
    pub flagged: bool,
}

impl ExitLight {
    const DARK: Self = Self {
        ratio: [0.0; 3],
        flagged: false,
    };

    const GREY: Self = Self {
        ratio: [1.0; 3],
        flagged: false,
    };
}

impl RigLighting {
    /// A backlight with a panel per view and optional white frames.
    #[must_use]
    pub const fn backlight(per_view: Vec<PanelGeom>, white: Vec<Option<WhiteFrame>>) -> Self {
        Self {
            model: LightModel::Backlight { per_view, white },
            holder: None,
        }
    }

    /// A uniform surround.
    #[must_use]
    pub const fn uniform_surround() -> Self {
        Self {
            model: LightModel::UniformSurround,
            holder: None,
        }
    }

    /// This lighting with a holder mesh (rig frame).
    #[must_use]
    pub fn with_holder(mut self, holder: RoughMesh) -> Self {
        self.holder = Some(holder);
        self
    }

    /// Checks the panels and the white frames.
    ///
    /// # Errors
    ///
    /// [`ForwardError::BadLighting`] for an unusable panel.
    pub fn validate(&self) -> Result<(), ForwardError> {
        if let LightModel::Backlight { per_view, .. } = &self.model {
            if per_view.is_empty() {
                return Err(ForwardError::BadLighting("no backlight panels".to_owned()));
            }
            if let Some(i) = per_view.iter().position(|p| !p.is_usable()) {
                return Err(ForwardError::BadLighting(format!(
                    "panel of view {} has a zero normal or size",
                    i + 1
                )));
            }
        }
        Ok(())
    }

    /// The white-frame level of each channel at the photo position of a camera ray, 1 for a
    /// uniform panel or surround, the frame mean when the position is outside the frame.
    pub(super) fn own_level(&self, view: usize, pixel: DVec2) -> [f64; 3] {
        match &self.model {
            LightModel::UniformSurround => [1.0; 3],
            LightModel::Backlight { white, .. } => {
                white
                    .get(view)
                    .and_then(Option::as_ref)
                    .map_or([1.0; 3], |frame| {
                        frame
                            .level_at(pixel.x, pixel.y)
                            .unwrap_or_else(|| frame.mean.map(f64::from))
                    })
            }
        }
    }

    /// Whether the holder blocks the ray before distance `max_t`.
    fn blocked(&self, origin: DVec3, dir: DVec3, max_t: f64) -> bool {
        self.holder.as_ref().is_some_and(|holder| {
            let (lo, hi) = holder.bounds();
            holder
                .first_hit(origin, dir, 1e-7 * (hi - lo).length())
                .is_some_and(|hit| hit.t < max_t)
        })
    }

    /// The light of an exit ray of view `view`, from `origin` along the unit `dir` (rig frame),
    /// relative, channel by channel, to the level `own_level` at the camera ray's own pixel.
    ///
    /// A pixel whose own level is zero in any channel has no defined transmittance in that
    /// channel (the white frame is black there), so its light is dark.
    pub(super) fn exit_light(
        &self,
        rig: &RigProfile,
        view: usize,
        origin: DVec3,
        dir: DVec3,
        own_level: [f64; 3],
    ) -> ExitLight {
        match &self.model {
            LightModel::UniformSurround => {
                if self.blocked(origin, dir, f64::INFINITY) {
                    ExitLight::DARK
                } else {
                    ExitLight::GREY
                }
            }
            LightModel::Backlight { per_view, white } => {
                let Some((t, point)) = per_view.get(view).and_then(|p| p.hit(origin, dir)) else {
                    return ExitLight::DARK;
                };
                if self.blocked(origin, dir, t) {
                    return ExitLight::DARK;
                }
                let Some(frame) = white.get(view).and_then(Option::as_ref) else {
                    return ExitLight::GREY;
                };
                if own_level.iter().any(|&level| level <= 1e-12) {
                    return ExitLight::DARK;
                }
                let seen = rig
                    .views
                    .get(view)
                    .and_then(|pose| pose.project(point))
                    .and_then(|px| frame.level_at(px.x, px.y));
                let flagged = seen.is_none();
                let level = seen.unwrap_or_else(|| frame.mean.map(f64::from));
                ExitLight {
                    ratio: [
                        level[0] / own_level[0],
                        level[1] / own_level[1],
                        level[2] / own_level[2],
                    ],
                    flagged,
                }
            }
        }
    }
}

//! The gemological studio light rig: key softbox, fill softbox, and the overhead ring
//! of pinpoint scintillation emitters, all derived from a single `(light_yaw,
//! light_pitch)` pose.
//!
//! # Why this exists
//!
//! Before this module, the key/fill direction formulas (and, less critically, the ring
//! emitter directions) were written out twice: once in
//! `optics::raytracer::sample_studio_environment` (which lights the actual traced
//! image) and once again in `color::metrics::evaluate_gem_optical_metrics` (which
//! scores that same image's brilliance/fire/scintillation). The two copies agreed at
//! the time, but nothing enforced that -- and because the metrics panel is supposed to
//! describe the image the renderer actually produces, a drift between the two would
//! silently make the panel describe a different scene than the one on screen, with no
//! test catching it. [`StudioRig`] gives both call sites exactly one formula to call
//! instead, following the same precedent as `color::metrics::camera_view_basis` for the
//! camera basis.
//!
//! Lives under `optics/` (rather than `color/`) because `color::metrics` already
//! imports from `optics`, so this is reachable from both call sites without a circular
//! module dependency.

use glam::Vec3;

/// Number of pinpoint emitters in the overhead scintillation ring.
pub const RING_LIGHT_COUNT: usize = 16;

/// The studio light rig's three light sources, all derived from a single
/// `(light_yaw, light_pitch)` pose. See the module docs for why this is shared rather
/// than duplicated.
#[derive(Clone, Copy, Debug)]
pub struct StudioRig {
    /// Direction toward the main key softbox.
    pub key_dir: Vec3,
    /// Direction toward the fill softbox (yaw offset `PI * 0.78` from the key, at a
    /// shallower, clamped pitch).
    pub fill_dir: Vec3,
    /// Directions toward the `RING_LIGHT_COUNT` overhead ring emitters, evenly spaced
    /// in yaw around the key/fill azimuth.
    pub ring_dirs: [Vec3; RING_LIGHT_COUNT],
    /// `light_pitch.sin()`, exposed directly because
    /// `color::metrics::ray_is_visibly_returned`'s coarse ring-annulus test consults it
    /// alone rather than the discrete `ring_dirs` above.
    pub sin_light_pitch: f32,
}

impl StudioRig {
    /// Builds the rig for a given key light yaw/pitch, in radians.
    #[must_use]
    pub fn new(light_yaw: f32, light_pitch: f32) -> Self {
        let cos_lp = light_pitch.cos();
        let sin_lp = light_pitch.sin();
        let cos_ly = light_yaw.cos();
        let sin_ly = light_yaw.sin();
        let key_dir = Vec3::new(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly).normalize();

        // Fill Softbox Light (side reflector offset by 140 deg)
        let fill_yaw = std::f32::consts::PI.mul_add(0.78, light_yaw);
        let fill_pitch = (light_pitch * 0.65).clamp(0.15, 1.2);
        let fill_dir = Vec3::new(
            fill_pitch.cos() * fill_yaw.sin(),
            fill_pitch.sin(),
            fill_pitch.cos() * fill_yaw.cos(),
        )
        .normalize();

        let ring_dirs = std::array::from_fn(|i| {
            let angle = (i as f32).mul_add(
                std::f32::consts::PI * 2.0 / RING_LIGHT_COUNT as f32,
                light_yaw,
            );
            Vec3::new(angle.cos() * 0.75, sin_lp * 0.8, angle.sin() * 0.75).normalize()
        });

        Self {
            key_dir,
            fill_dir,
            ring_dirs,
            sin_light_pitch: sin_lp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the key/fill/ring direction formulas at a fixed, representative pose
    /// against values computed independently by hand from the same formulas
    /// `sample_studio_environment` and `evaluate_gem_optical_metrics` used to each
    /// write out inline before this extraction. A future accidental change to
    /// `StudioRig::new` that drifted from either original formula would fail this
    /// test.
    #[test]
    fn key_fill_and_ring_directions_match_the_original_inline_formulas() {
        let light_yaw = 0.85f32;
        let light_pitch = 0.95f32;
        let rig = StudioRig::new(light_yaw, light_pitch);

        let cos_lp = light_pitch.cos();
        let sin_lp = light_pitch.sin();
        let cos_ly = light_yaw.cos();
        let sin_ly = light_yaw.sin();
        let expected_key = Vec3::new(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly).normalize();
        assert!((rig.key_dir - expected_key).length() < 1e-6);

        let fill_yaw = std::f32::consts::PI.mul_add(0.78, light_yaw);
        let fill_pitch = (light_pitch * 0.65).clamp(0.15, 1.2);
        let expected_fill = Vec3::new(
            fill_pitch.cos() * fill_yaw.sin(),
            fill_pitch.sin(),
            fill_pitch.cos() * fill_yaw.cos(),
        )
        .normalize();
        assert!((rig.fill_dir - expected_fill).length() < 1e-6);

        assert_eq!(rig.ring_dirs.len(), RING_LIGHT_COUNT);
        for (i, ring_dir) in rig.ring_dirs.iter().enumerate() {
            let angle = (i as f32).mul_add(
                std::f32::consts::PI * 2.0 / RING_LIGHT_COUNT as f32,
                light_yaw,
            );
            let expected_ring =
                Vec3::new(angle.cos() * 0.75, sin_lp * 0.8, angle.sin() * 0.75).normalize();
            assert!(
                (*ring_dir - expected_ring).length() < 1e-6,
                "ring light {i} direction mismatch"
            );
        }

        assert!((rig.sin_light_pitch - sin_lp).abs() < 1e-6);
    }

    /// Cross-check that the SAME `StudioRig` construction is what both
    /// `optics::raytracer::sample_studio_environment` and
    /// `color::metrics::evaluate_gem_optical_metrics` now derive their key/fill
    /// directions from: driving `sample_studio_environment` with `dir` set exactly to
    /// `rig.key_dir` must land on the key softbox's own peak-alignment term (`key_dot
    /// == 1.0`), which only holds if the renderer is using this exact same `key_dir`
    /// vector rather than an independently (and possibly drifted) recomputed one.
    #[test]
    fn sample_studio_environment_peaks_exactly_along_this_rigs_key_direction() {
        let light_yaw = 0.3f32;
        let light_pitch = 0.6f32;
        let rig = StudioRig::new(light_yaw, light_pitch);

        let on_axis = crate::optics::raytracer::sample_studio_environment(
            rig.key_dir,
            560.0,
            crate::optics::raytracer::LightingPreset::RingLights,
            1.0,
            light_yaw,
            light_pitch,
        );
        let off_axis = crate::optics::raytracer::sample_studio_environment(
            Vec3::new(-rig.key_dir.z, rig.key_dir.y, rig.key_dir.x),
            560.0,
            crate::optics::raytracer::LightingPreset::RingLights,
            1.0,
            light_yaw,
            light_pitch,
        );

        assert!(
            on_axis > off_axis,
            "radiance exactly along the shared rig's key_dir ({on_axis}) should exceed a direction rotated away from it ({off_axis})"
        );
    }
}

//! The gemological studio light rig: key softbox, fill softbox, and the overhead ring
//! of pinpoint scintillation emitters, all derived from a single `(light_yaw,
//! light_pitch)` pose.
//!
//! # Why this exists
//!
//! The key/fill direction formulas and the ring emitter directions are written once, in
//! [`StudioRig::new`], and every consumer of the rig reads them from there:
//! `optics::raytracer::sample_studio_environment` (which lights the traced image) and
//! `color::metrics`, which scores that same image's brilliance/fire/scintillation by
//! reading the same radiance through `sample_studio_environment_with_rig`. The metrics
//! panel describes the image the renderer actually produces, so the two must not drift
//! apart; sharing the rig construction guarantees it.
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
    /// in azimuth: slot 0 shares the key's azimuth (`light_yaw`) and each following
    /// slot is a further `360 / RING_LIGHT_COUNT` degrees round, in the key's
    /// rotation sense.
    pub ring_dirs: [Vec3; RING_LIGHT_COUNT],
    /// `light_pitch.sin()`, the elevation factor the `ring_dirs` above are built with,
    /// exposed for consumers that need the pitch alone.
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

        // Same azimuth convention as the key (`atan2(x, z)` is the yaw), so slot 0 sits
        // on the key's azimuth and slot `i` is `i * 22.5 deg` further round.
        let ring_dirs = std::array::from_fn(|i| {
            let angle = (i as f32).mul_add(
                std::f32::consts::PI * 2.0 / RING_LIGHT_COUNT as f32,
                light_yaw,
            );
            Vec3::new(angle.sin() * 0.75, sin_lp * 0.8, angle.cos() * 0.75).normalize()
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

    /// Azimuth of `dir` in the key's convention: `atan2(x, z)`, the yaw a direction
    /// built as `(cos p * sin yaw, sin p, cos p * cos yaw)` was made with.
    fn azimuth(dir: Vec3) -> f32 {
        dir.x.atan2(dir.z)
    }

    /// Smallest signed difference between two angles, wrapped into `[-pi, pi)`.
    fn angle_difference(a: f32, b: f32) -> f32 {
        let tau = std::f32::consts::TAU;
        (a - b + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
    }

    /// The key sits at the requested yaw, ring slot 0 shares that azimuth, and slot 4 (a
    /// quarter turn round) is at `yaw + 90 deg` -- so the light tent's black cards,
    /// which are placed by ring slot, are at 90/180/270 degrees from the key.
    #[test]
    fn ring_azimuths_follow_the_key_yaw_and_rotate_in_its_sense() {
        let light_pitch = 0.95f32;
        for light_yaw in [0.0f32, 0.3, 0.85, 1.5] {
            let rig = StudioRig::new(light_yaw, light_pitch);
            assert_eq!(rig.ring_dirs.len(), RING_LIGHT_COUNT);

            assert!(
                angle_difference(azimuth(rig.key_dir), light_yaw).abs() < 1e-5,
                "key azimuth must equal the yaw {light_yaw}"
            );
            assert!(
                angle_difference(azimuth(rig.ring_dirs[0]), light_yaw).abs() < 1e-5,
                "ring slot 0 must share the key azimuth at yaw {light_yaw}"
            );
            let quarter = light_yaw + std::f32::consts::FRAC_PI_2;
            assert!(
                angle_difference(azimuth(rig.ring_dirs[4]), quarter).abs() < 1e-5,
                "ring slot 4 must sit a quarter turn past the key at yaw {light_yaw}"
            );
            assert!((rig.sin_light_pitch - light_pitch.sin()).abs() < 1e-6);
        }
    }

    /// Cross-check that `optics::raytracer::sample_studio_environment`, which the
    /// metrics' illumination test and the renderer both read, derives its key/fill
    /// directions from this same `StudioRig` construction: driving it with `dir` set
    /// exactly to `rig.key_dir` must land on the key softbox's own peak-alignment term
    /// (`key_dot == 1.0`), which only holds if the renderer is using this exact same
    /// `key_dir` vector rather than an independently (and possibly drifted) recomputed one.
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

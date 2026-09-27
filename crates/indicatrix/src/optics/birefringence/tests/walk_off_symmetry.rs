//! Walk-off symmetry tests: the optic axis is a director, so
//! [`BirefringenceParams::extraordinary_poynting_dir`] must agree for `c_axis`
//! and `-c_axis`, and must match the analytic walk-off angle.

use crate::optics::birefringence::BirefringenceParams;
use glam::Vec3;

#[cfg(test)]
mod walk_off_symmetry_tests {
    use super::*;

    /// The optic axis is a director, not a directed vector -- `c_axis` and
    /// `-c_axis` describe the identical physical crystal, so
    /// `extraordinary_poynting_dir` must return the same Poynting direction for both.
    /// Swept across many incidence angles, deliberately crossing both sides of the
    /// `wave_normal.dot(c_axis) >= 0` branch, for calcite-like ordinary/extraordinary
    /// indices and an off-axis `c_axis`.
    #[test]
    fn extraordinary_poynting_dir_is_axis_direction_symmetric() {
        let n_o = 1.658f32;
        let n_e = 1.486f32;
        let c_axis = Vec3::new(0.2, 0.9, -0.35).normalize(); // deliberately off-axis
        let perp = {
            let p = c_axis.cross(Vec3::X);
            if p.length_squared() < 1e-6 {
                c_axis.cross(Vec3::Y).normalize()
            } else {
                p.normalize()
            }
        };

        for i in 0..37 {
            let theta = (i as f32) / 36.0 * std::f32::consts::PI; // sweep 0..=180 deg
            let wave_normal = (theta.cos() * c_axis + theta.sin() * perp).normalize();

            let s_pos =
                BirefringenceParams::extraordinary_poynting_dir(wave_normal, c_axis, n_o, n_e);
            let s_neg =
                BirefringenceParams::extraordinary_poynting_dir(wave_normal, -c_axis, n_o, n_e);

            assert!(
                (s_pos - s_neg).length() < 1e-4,
                "S(c) must equal S(-c) at theta={} deg (got s_pos={s_pos:?}, s_neg={s_neg:?})",
                theta.to_degrees()
            );
        }
    }

    /// Pins the exact measured regression: a wave normal at 45
    /// degrees from the optic axis in a calcite-like negative uniaxial material
    /// (`n_o`=1.658, `n_e`=1.486) must walk off to 51.23 degrees from the axis -- not 38.77
    /// degrees (the pre-fix answer on the `wave_normal.dot(c_axis) > 0` branch, off by
    /// exactly `2*delta` -- and this must hold whichever of `c_axis`/`-c_axis` is passed
    /// in, since both must agree per the symmetry test above.
    #[test]
    fn extraordinary_poynting_dir_matches_analytic_walk_off_both_hemispheres() {
        let n_o = 1.658f32;
        let n_e = 1.486f32;
        let c_axis = Vec3::Y;
        let theta = 45.0f32.to_radians();
        let wave_normal = Vec3::new(theta.sin(), theta.cos(), 0.0); // 45 deg from +Y

        for c in [c_axis, -c_axis] {
            let s = BirefringenceParams::extraordinary_poynting_dir(wave_normal, c, n_o, n_e);
            let angle_from_axis = s.dot(c_axis).clamp(-1.0, 1.0).abs().acos().to_degrees();
            assert!(
                (angle_from_axis - 51.23).abs() < 0.05,
                "expected walk-off to land at 51.23 deg from the axis for c={c:?}, got {angle_from_axis}"
            );
        }
    }
}

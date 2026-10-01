//! Uniaxial birefringence parameters: the closed-form effective extraordinary
//! index, walk-off angle, and the ordinary/extraordinary eigen-polarization
//! directions.

use super::helpers::stable_orthonormal_basis;
use glam::Vec3;

/// A uniaxial material's birefringence: the ordinary/extraordinary index difference
/// and the crystal's optic (c) axis direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BirefringenceParams {
    /// Extraordinary minus ordinary index.
    pub delta_n: f32,
    /// Optic (c) axis direction.
    pub c_axis: [f32; 3],
}

impl BirefringenceParams {
    /// Builds from a delta and a (not-necessarily-normalized) axis, normalizing
    /// `c_axis` first (the zero vector normalizes to zero).
    #[must_use]
    pub fn new(delta_n: f32, c_axis: Vec3) -> Self {
        let norm_c = c_axis.normalize_or_zero();
        Self {
            delta_n,
            c_axis: [norm_c.x, norm_c.y, norm_c.z],
        }
    }

    /// [`Self::c_axis`] as a [`Vec3`].
    #[must_use]
    pub const fn c_axis_vec3(&self) -> Vec3 {
        Vec3::from_array(self.c_axis)
    }

    /// Evaluates direction-dependent extraordinary refractive index `n_e(theta)`.
    #[must_use]
    pub fn effective_extraordinary_index(n_o: f32, n_e: f32, theta: f32) -> f32 {
        if (n_o - n_e).abs() < 1e-5 {
            return n_o;
        }
        let sin_t = theta.sin();
        let cos_t = theta.cos();
        let denom_sq = (n_e * cos_t).mul_add(n_e * cos_t, (n_o * sin_t).powi(2));
        if denom_sq <= 1e-8 {
            return n_o;
        }
        (n_o * n_e) / denom_sq.sqrt()
    }

    /// Walk-off angle estimation for extraordinary ray Poynting vector.
    #[must_use]
    pub fn walk_off_angle(n_o: f32, n_e: f32, theta: f32) -> f32 {
        if (n_o - n_e).abs() < 1e-5 {
            return 0.0;
        }
        let n_o2 = n_o * n_o;
        let n_e2 = n_e * n_e;
        let sin_t = theta.sin();
        let cos_t = theta.cos();
        let tan_rho = ((n_o2 - n_e2) * sin_t * cos_t)
            / (n_e2 * cos_t)
                .mul_add(cos_t, n_o2 * sin_t * sin_t)
                .max(1e-6);
        tan_rho.atan()
    }

    /// Calculates the deviated Poynting energy direction for the extraordinary ray.
    ///
    /// The optic axis is a DIRECTOR, not a directed vector -- a crystal's `c_axis` and
    /// `-c_axis` describe the same physical axis, so this must satisfy `S(c) == S(-c)`
    /// for every `wave_normal`. `c_axis` is folded onto the wave normal's own
    /// hemisphere first (via `sign`), with the tilt negated on that branch to
    /// compensate, so both branches agree -- matching the always-correct general
    /// `poynting_direction`/biaxial `mode_poynting_dir` construction this uniaxial fast
    /// path approximates.
    #[must_use]
    pub fn extraordinary_poynting_dir(wave_normal: Vec3, c_axis: Vec3, n_o: f32, n_e: f32) -> Vec3 {
        let cos_theta = wave_normal.dot(c_axis).clamp(-1.0, 1.0);
        let theta = cos_theta.abs().acos();
        let delta = Self::walk_off_angle(n_o, n_e, theta);

        if delta.abs() < 1e-5 {
            return wave_normal;
        }

        // Walk-off occurs in the plane containing wave normal and c-axis
        let c_proj = (c_axis - cos_theta * wave_normal).normalize_or_zero();
        if c_proj.length_squared() < 1e-6 {
            return wave_normal;
        }
        let sign = if cos_theta >= 0.0 { 1.0 } else { -1.0 };

        (wave_normal * delta.cos() - sign * c_proj * delta.sin()).normalize()
    }

    /// The ordinary eigenmode's electric-field direction for a wave travelling along
    /// `wave_normal` in a uniaxial crystal with optical axis `c_axis`: perpendicular to
    /// the plane containing the wave normal and the c-axis, so it always has zero
    /// component along `c_axis` (purely "ordinary") regardless of propagation angle.
    /// Degenerates to an arbitrary direction perpendicular to `wave_normal` when
    /// `wave_normal` is parallel to `c_axis` (propagation along the optic axis, where
    /// the two eigenmodes are physically degenerate anyway -- no birefringence there).
    #[must_use]
    pub fn ordinary_eigen_polarization(wave_normal: Vec3, c_axis: Vec3) -> Vec3 {
        let cross = wave_normal.cross(c_axis);
        if cross.length_squared() > 1e-8 {
            cross.normalize()
        } else {
            stable_orthonormal_basis(wave_normal.normalize_or_zero()).0
        }
    }

    /// The extraordinary eigenmode's electric-field direction: perpendicular to both
    /// the wave normal and the ordinary eigenmode above, i.e. lying in the plane
    /// containing the wave normal and the c-axis (the plane the walk-off displacement
    /// itself occurs in -- see `extraordinary_poynting_dir`).
    #[must_use]
    pub fn extraordinary_eigen_polarization(wave_normal: Vec3, c_axis: Vec3) -> Vec3 {
        let o_hat = Self::ordinary_eigen_polarization(wave_normal, c_axis);
        wave_normal.cross(o_hat).normalize_or_zero()
    }
}

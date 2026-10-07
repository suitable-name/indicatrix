//! Snell refraction and specular reflection at a surface, with total internal reflection.
//!
//! The spectral tracer in `indicatrix` keeps its refraction inside the renderer and exports no
//! helper, so this is a small pure function of its own: a unit direction in, a unit direction
//! out, the normal in either orientation.

use glam::DVec3;

/// Refracts the direction `incident` at a surface with `normal`, going from a medium of index
/// `n_from` into one of index `n_to`.
///
/// `incident` and `normal` need not be unit vectors, and the normal may face either way (it is
/// turned against the incident ray). Returns the unit direction of the transmitted ray, or
/// `None` for total internal reflection (only possible when `n_from > n_to`). Equal indices
/// return the incident direction unchanged, bit for bit after normalising.
#[must_use]
pub fn refract(incident: DVec3, normal: DVec3, n_from: f64, n_to: f64) -> Option<DVec3> {
    let dir = incident.normalize();
    let mut facing = normal.normalize();
    let mut cos_i = -dir.dot(facing);
    if cos_i < 0.0 {
        facing = -facing;
        cos_i = -cos_i;
    }
    if n_from == n_to {
        return Some(dir);
    }
    let eta = n_from / n_to;
    let sin_i = dir.cross(facing).length();
    let sin_t = eta * sin_i;
    if sin_t > 1.0 {
        return None;
    }
    let cos_t = (-sin_t).mul_add(sin_t, 1.0).max(0.0).sqrt();
    let along_normal = eta.mul_add(cos_i, -cos_t);
    Some((dir * eta + facing * along_normal).normalize())
}

/// The mirror image of `incident` at a surface with `normal`: the direction after a specular
/// reflection. Both are normalised first.
#[must_use]
pub fn reflect(incident: DVec3, normal: DVec3) -> DVec3 {
    let dir = incident.normalize();
    let facing = normal.normalize();
    (dir - facing * (2.0 * dir.dot(facing))).normalize()
}

/// The critical angle in degrees for light going from `n_from` into `n_to`, or `None` when
/// there is none (`n_from <= n_to`: light always gets through).
#[must_use]
pub fn critical_angle_deg(n_from: f64, n_to: f64) -> Option<f64> {
    (n_from > n_to).then(|| (n_to / n_from).asin().to_degrees())
}

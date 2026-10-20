//! Scalar f32 operations kept in the same order as the small WGSL kernel.

use glam::Vec3;

/// Unpolarized power reflectance; `eta` is the incident/transmitted index ratio.
pub fn fresnel_unpolarised_f32(cos_i: f32, eta: f32) -> f32 {
    if eta == 1.0 {
        return 0.0;
    }
    let cos_i = cos_i.abs().clamp(0.0, 1.0);
    let sin2_i = (-cos_i).mul_add(cos_i, 1.0).max(0.0);
    let cos2_t = (-(eta * eta)).mul_add(sin2_i, 1.0);
    if cos2_t <= 0.0 {
        return 1.0;
    }
    let cos_trans = cos2_t.sqrt();
    let eta_cos_i = eta * cos_i;
    let eta_cos_t = eta * cos_trans;
    let rs = (eta_cos_i - cos_trans) / (eta_cos_i + cos_trans);
    let rp = (cos_i - eta_cos_t) / (cos_i + eta_cos_t);
    (0.5 * rs.mul_add(rs, rp * rp)).clamp(0.0, 1.0)
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a.x.mul_add(b.x, a.y.mul_add(b.y, a.z * b.z))
}

#[expect(
    clippy::missing_const_for_fn,
    reason = "f32::mul_add is not const; clippy's const suggestion would not compile"
)]
pub fn scale_add(v: Vec3, scale: f32, add: Vec3) -> Vec3 {
    Vec3::new(
        v.x.mul_add(scale, add.x),
        v.y.mul_add(scale, add.y),
        v.z.mul_add(scale, add.z),
    )
}

pub fn reflect(dir: Vec3, normal: Vec3) -> Vec3 {
    scale_add(normal, -2.0 * dot(dir, normal), dir)
}

/// `normal` faces the incoming ray. A negative Snell discriminant means TIR.
pub fn refract(dir: Vec3, normal: Vec3, eta: f32) -> Option<Vec3> {
    let cos_i = (-dot(dir, normal)).clamp(0.0, 1.0);
    let sin2_i = (-cos_i).mul_add(cos_i, 1.0).max(0.0);
    let discriminant = (-(eta * eta)).mul_add(sin2_i, 1.0);
    if discriminant < 0.0 {
        return None;
    }
    Some(scale_add(
        normal,
        eta.mul_add(cos_i, -discriminant.sqrt()),
        dir * eta,
    ))
}

//! Flat Lambert-plus-rim shading and the color-blend helper used for the
//! selected-facet tint and the preform tint.

use super::SolidStyle;
use glam::Vec3;
use indicatrix::optics::raytracer::Camera;

/// Linearly blends `base` toward `target` by `amount` (`0.0` = `base` unchanged,
/// `1.0` = `target` exactly) -- the selected-facet tint.
pub(super) fn blend_toward(base: [u8; 3], target: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        let b = f32::from(base[i]);
        let t = f32::from(target[i]);
        amount.mul_add(t - b, b).round().clamp(0.0, 255.0) as u8
    })
}

/// Flat Lambert shading from a fixed key light, plus a small Fresnel-style rim
/// term so near-edge-on facets (`N . V` close to zero) stay distinguishable from
/// background instead of going nearly black. Computed once per triangle since
/// facet normals are flat.
pub(super) fn shade(
    normal: Vec3,
    world_point: Vec3,
    camera: &Camera,
    style: &SolidStyle,
    base_color: [u8; 3],
) -> [u8; 3] {
    let n = normal.normalize();
    let light_dir = style.key_light_dir.normalize();
    let view_dir = (camera.origin - world_point).normalize();
    let n_dot_l = n.dot(light_dir).max(0.0);
    let n_dot_v = n.dot(view_dir).max(0.0);
    let rim = (1.0 - n_dot_v).powi(2) * style.rim_strength;
    let intensity = style
        .diffuse
        .mul_add(n_dot_l, style.ambient + rim)
        .clamp(0.0, 1.0);
    [
        (f32::from(base_color[0]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(base_color[1]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(base_color[2]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
    ]
}

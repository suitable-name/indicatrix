//! Fixed spherical quadrature for the deliberately approximate frosted finish.

use glam::Vec3;

use crate::{optics::raytracer::FacetFinish, renderer::frame_scene::FrameScene};

use super::env::WhittedEnv;

const DIRECTIONS: u16 = 256;
const GOLDEN_ANGLE: f32 = 2.399_963_1;

pub(super) fn prepare(scene: &FrameScene<'_>, env: &WhittedEnv<'_>) -> Vec<Vec3> {
    if !scene.facet_finishes.contains(&FacetFinish::Frosted) {
        return Vec::new();
    }
    scene
        .planes
        .iter()
        .enumerate()
        .map(|(facet, plane)| {
            if scene.facet_finishes.get(facet) != Some(&FacetFinish::Frosted) {
                return Vec3::ZERO;
            }
            let normal = Vec3::from_array(plane.normal).normalize_or_zero();
            mean(env, normal, -scene.camera.forward)
        })
        .collect()
}

pub(super) fn mean(env: &WhittedEnv<'_>, normal: Vec3, observer: Vec3) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut weights = 0.0;
    for k in 0..DIRECTIONS {
        let y = 1.0 - 2.0 * (f32::from(k) + 0.5) / f32::from(DIRECTIONS);
        // Operation order mirrors the WGSL twin whitted/00_bindings.wgsl: the square is its
        // own binding so no clippy version suggests a fused multiply-add.
        let square = y * y;
        let radius = (1.0 - square).max(0.0).sqrt();
        let angle = f32::from(k) * GOLDEN_ANGLE;
        let (sin, cos) = angle.sin_cos();
        let direction = Vec3::new(radius * cos, y, radius * sin);
        let weight = normal.dot(direction).max(0.0);
        sum += env.leaf(direction, observer) * weight;
        weights += weight;
    }
    if weights > 0.0 {
        sum / weights
    } else {
        Vec3::ZERO
    }
}

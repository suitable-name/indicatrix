//! Thread-free XYZ tap tracing with the shared interleaved-pixel contract.

use glam::Vec3;

use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::{HitRecord, Ray, add_finite_sample, color::apply_von_kries_white_balance},
    renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

use super::{WhittedPrep, aa_offset, chain::trace_chain};

/// First geometric hit for picking/guide buffers, reusing the scene's plane arena.
/// Empty image geometry is an unobstructed environment.
#[must_use]
pub fn whitted_primary_hit(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    ray: Ray,
) -> Option<HitRecord> {
    if scene.planes.is_empty() {
        None
    } else {
        crate::optics::raytracer::intersect_stone::intersect_stone_soa(
            ray,
            plane_soa,
            scene.planes.len(),
            tools,
        )
    }
}

/// One absolute tap of one pixel. The first tap is the pixel centre. All color
/// channels share the first geometric hit even when Fire traces three indices.
#[must_use]
pub fn whitted_tap_xyz(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    pixel: u32,
    tap: u32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    if scene.width == 0
        || scene.height == 0
        || u64::from(pixel) >= u64::from(scene.width) * u64::from(scene.height)
    {
        if let Some(out) = primary_hit_out {
            *out = None;
        }
        return Vec3::ZERO;
    }
    let (jx, jy) = aa_offset(prep.taps, tap);
    let ray = scene.camera.generate_ray(
        (pixel % scene.width) as f32,
        (pixel / scene.width) as f32,
        scene.width as f32,
        scene.height as f32,
        jx,
        jy,
    );
    let observer = -ray.dir;
    let mut rgb = Vec3::ZERO;
    let mut primary_hit_out = primary_hit_out;
    for chain in prep
        .chains
        .iter()
        .take(usize::from(prep.chain_count.min(3)))
    {
        rgb += chain.mask
            * trace_chain(
                scene,
                tools,
                plane_soa,
                prep,
                ray,
                chain.n,
                observer,
                primary_hit_out.take(),
            );
    }
    if let Some(out) = primary_hit_out {
        *out = None;
    }
    let mut xyz = prep.rgb_to_xyz * rgb;
    if let Some(scale) = prep.white_balance {
        xyz = apply_von_kries_white_balance(xyz, scale);
    }
    xyz.max(Vec3::ZERO)
}

/// Sum absolute taps `[sample_offset, sample_offset+spp)` for ascending pixels
/// `first_pixel + k*stride`.
///
/// Disjoint partitions reconstruct the same per-pixel sums bit for bit,
/// independent of thread/worker count. As in the path tracer, a non-finite
/// sample is discarded while still counting towards the target.
#[expect(
    clippy::too_many_arguments,
    reason = "scene, prepared state and the pixel/tap partition stay flat, as in the path tracer's twin"
)]
#[must_use]
pub fn whitted_pixels_interleaved_geom(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if stride == 0 || pixels == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pixel = u64::from(first_pixel);
    while pixel < pixels
        && let Ok(index) = u32::try_from(pixel)
    {
        let mut sum = Vec3::ZERO;
        for sample in 0..spp {
            add_finite_sample(
                &mut sum,
                whitted_tap_xyz(
                    scene,
                    tools,
                    plane_soa,
                    prep,
                    index,
                    sample_offset.wrapping_add(sample),
                    None,
                ),
            );
        }
        out.push(sum);
        pixel += u64::from(stride);
    }
    out
}

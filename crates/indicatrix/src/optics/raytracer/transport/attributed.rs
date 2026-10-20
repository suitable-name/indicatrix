//! The attributed trace entry point: the plain stone trace plus a [`PathEvents`] record.

use super::{PathEvents, PathTermination, inner};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            camera::{FacetFinish, HitRecord, Ray},
            environment::{EnvironmentSource, environment_supports_nee},
        },
    },
};
use glam::Vec3;

/// [`trace_spectral_ray_with_finish_soa_geom`](super::trace_spectral_ray_with_finish_soa_geom)
/// that also returns what the path did.
///
/// The `Vec3` is that function's result bit for bit (the same switches, the same loop; the
/// recorder is only written to), and the [`PathEvents`] carry the labelling data of the
/// hero path: its first facet event, first exit, flags and the reason the loop stopped.
#[expect(
    clippy::too_many_arguments,
    reason = "see trace_spectral_ray_with_finish_soa_geom's own reason"
)]
#[must_use]
pub fn trace_spectral_ray_attributed_soa_geom(
    initial_ray: Ray,
    geom: StoneGeometry<'_>,
    plane_soa: &crate::simd::PlanesSoA32,
    facet_finishes: &[FacetFinish],
    material: &GemMaterial,
    fluorescence: &Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'_>,
    rng_seed: u32,
    hero_rand: f32,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> (Vec3, PathEvents) {
    let mut events = PathEvents::default();
    let mut termination = (max_bounces, PathTermination::HitCap);
    let xyz = inner::trace_spectral_ray_inner_geom_events(
        initial_ray,
        geom.planes,
        geom.tools,
        plane_soa,
        facet_finishes,
        material,
        fluorescence,
        max_bounces,
        environment,
        rng_seed,
        hero_rand,
        primary_hit_out,
        true,
        true,
        environment_supports_nee(environment),
        (Some(&mut termination), Some(&mut events)),
    );
    events.terminated(termination.1);
    (xyz, events)
}

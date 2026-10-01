//! [`FrameScene`]: everything a frame tracer needs to render a scene, independent of
//! which backend traces it.
//!
//! Lives outside the `gpu` feature (unlike the code that dispatches it on the GPU)
//! because the wasm-safe CPU tracer in [`super::cpu_frame`] needs this exact struct
//! with no `gpu` feature enabled -- see that module's doc comment. The GPU megakernel
//! path (`renderer::gpu::frame`) re-exports this same type as `GpuFrameScene` so every
//! existing `GpuFrameScene { .. }` construction and import keeps compiling unchanged.

use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
};

/// Everything the frame tracer needs to render a frame, GPU or CPU.
///
/// Bundled so a caller assembles the scene once rather than per chunk (the GPU side)
/// or per worker (the CPU side).
pub struct FrameScene<'a> {
    /// Camera the frame is rendered from.
    pub camera: &'a Camera,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Facet planes of the stone.
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane finish, indexed in step with `planes`. A shorter slice is padded with
    /// [`FacetFinish::default`], matching [`crate::renderer::buffers::encode_facet_finishes`].
    pub facet_finishes: &'a [FacetFinish],
    /// Gem material.
    pub material: &'a GemMaterial,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Lighting environment.
    pub environment: EnvironmentSource<'a>,
}

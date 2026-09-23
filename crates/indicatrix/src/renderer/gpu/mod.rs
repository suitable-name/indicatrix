//! GPU compute infrastructure for `indicatrix` (behind the `gpu` feature).
//!
//! Adapter/device acquisition, a minimal compute-pipeline harness, buffer
//! upload/readback helpers, and the struct-layout/RNG/self-determinism
//! self-tests any physics port must pass before its output can be trusted --
//! plus [`frame`], a REAL, production GPU port of
//! `optics::raytracer::trace_spectral_ray` (the `transport_main` megakernel
//! and the `Wavefront` pipeline alternative) that
//! `renderer::gpu_backend::GpuBackend` dispatches scenes through today.
//!
//! `renderer::pipeline::IndicatrixRaytracerPipeline` is a SEPARATE,
//! unrelated rasterized/hybrid preview path that still panics
//! unconditionally (its shader is quarantined) and has never been wired to
//! anything in this workspace -- unlike [`frame`], which every self-test
//! below and `renderer::gpu_backend` exercise directly.
//!
//! # Modules
//!
//! - [`context`]: adapter/device acquisition ([`GpuContext`]).
//! - [`compute`]: generic compute-pipeline/buffer helpers.
//! - [`layout_check`]: struct-layout GPU echo test.
//! - [`rng_check`]: RNG/integer bit-exactness test against the CPU.
//! - [`determinism_check`]: GPU self-determinism test.
//!
//! `examples/gpu_equivalence_harness.rs` (gated on `gpu`, needs a real
//! adapter so it isn't a `cargo test` target) runs every self-test in this
//! module and its siblings (`camera_check`/`environment_check`/`furnace_check`/
//! `polyhedron_check`/`estimator_check`/`transport_check`/`shading_normal_check`,
//! plus [`frame`]'s own chunk/wavefront/specialisation equivalence checks).

pub mod compute;
pub mod context;
pub mod determinism_check;
pub mod layout_check;
pub mod rng_check;

// Geometry/environment self-tests against analytic truth: camera rays, the
// `intersect_polyhedron` slab test, studio environment sampling, CIE 1931 CMF
// integration, von Kries white balance. No transport physics.
pub mod camera_check;
pub mod environment_check;
pub mod furnace_check;
pub mod polyhedron_check;
pub(crate) mod ulp;

// Full isotropic spectral estimator self-test (Fresnel/TIR, Stokes-Mueller
// transport, pleochroic Beer-Lambert absorption, Russian roulette, spectral
// MIS) -- cubic materials only; birefringence not covered here.
pub mod estimator_check;
pub mod transport_check;

// Self-test for `shading_normal_near_edge`; kept separate (own `planes`
// binding) from `transport_check`/`transport_functions.wgsl`.
pub mod shading_normal_check;

// Not a self-test: renders an arbitrary scene with the megakernel every
// module above verifies, via the dispatch routine `estimator_check` uses.
pub mod frame;
#[cfg(test)]
mod shader_validation_tests;

// CPU+GPU hybrid rendering: GPU and every CPU core trace disjoint sample
// ranges of one frame concurrently, merging into one image.
//
// Excluded on wasm32: `std::thread::scope` needs real OS threads, and the
// `webgpu` backend's `wgpu::Device` holds `Rc`-based handles that aren't
// `Send` -- a hard compile error there regardless. `apps/indicatrix-web`
// never calls this module.
#[cfg(not(target_arch = "wasm32"))]
pub mod hybrid;

pub use context::{
    GpuAcquireError, GpuContext, MEGAKERNEL_STORAGE_BUFFERS, WAVEFRONT_BOUNCE_TOTAL_BUFFERS,
    WAVEFRONT_STORAGE_BUFFERS,
};
pub use frame::{AccumulateOutcome, GpuFrameError, GpuFrameRenderer, GpuFrameScene};

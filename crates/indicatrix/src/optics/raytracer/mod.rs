//! The spectral path tracer.
//!
//! Camera/ray generation, polyhedron intersection, Fresnel/TIR refraction,
//! Henyey-Greenstein scattering, pleochroic absorption, environment sampling,
//! low-discrepancy sampling, and spectral-to-tristimulus colour conversion.
//!
//! Split from a single `raytracer.rs` into this module tree by seam (see each
//! submodule's own doc comment for what it owns). This file is a pure re-export hub:
//! every path that was reachable as `indicatrix::optics::raytracer::X` before the split is
//! still reachable at exactly that path via the `pub use`/`pub(crate) use` lines below,
//! and every submodule can see every other submodule's `pub(super)` items directly.

// `pub`, not private: a `pub(crate)` item inside a private module is only reachable
// via this file's re-export, which clippy's `redundant_pub_crate` (nursery) then flags
// as suspicious. Making the submodule `pub` resolves it without widening any item's
// own effective visibility, still capped at its declared visibility.
pub mod absorption;
pub mod camera;
pub mod color;
pub mod environment;
pub mod intersect;
pub mod refraction;
pub mod sampling;
pub mod scattering;
pub mod transport;
pub mod uniaxial_fresnel;

/// Number of spectral channels `trace_spectral_ray` traces per ray (8-channel
/// stratified hero-wavelength sampling). Module-level, not a `const` local to
/// `trace_spectral_ray`, so the per-bounce helper functions extracted from that
/// function can also reference it.
const NUM_CHANNELS: usize = 8;

// Re-exports below preserve paths reachable directly off `raytracer` before the module
// split, at their original visibility. `pub(crate)` ones carry #[allow(unused_imports)]:
// sibling submodules reach each other directly, never through this hub, but the path
// must still resolve for consumers this exact build may not compile (renderer::gpu's
// feature="gpu"-gated Tier 2 harnesses, crates/indicatrix/tests/).

// camera.rs
pub use camera::{Camera, FacetFinish, HitRecord, Ray};

// intersect.rs
pub use intersect::intersect_polyhedron;
// `pub`, not `pub(crate)`: a batch/frame caller (e.g. `examples/pgo_train.rs`) needs
// this to build the `PlanesSoA32` arena once and drive
// `trace_spectral_ray_with_finish_soa` with it rather than rebuilding it per sample.
pub use intersect::build_plane_soa;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::transport_check's \
              Tier 2 harness (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use intersect::{intersect_polyhedron_soa, shading_normal_near_edge};

// sampling.rs
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::rng_check's Phase 0 \
              self-test (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use sampling::{
    BIREFRINGENT_SPLIT_STREAM, DISTANCE_SAMPLE_STREAM, FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM,
    FROSTED_DIR_V_STREAM, MODE_COUPLING_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM,
    RUSSIAN_ROULETTE_STREAM,
};
pub use sampling::{
    HERO_WAVELENGTH_ROTATION_STREAM, PIXEL_JITTER_X_ROTATION_STREAM,
    PIXEL_JITTER_Y_ROTATION_STREAM, PixelRotations, SampleDraws, cranley_patterson_rotate,
    hash_u32, low_discrepancy_base2, pixel_rotations, radical_inverse_base, sample_draws,
};

// environment.rs
pub use environment::{
    BACKDROP_GREY, BACKDROP_WHITE, EnvironmentSource, LightingModel, LightingPreset,
    LightingRigParams, blackbody_spectrum, sample_studio_environment,
    sample_studio_environment_observed,
};

// color.rs
pub use color::{aces_tonemap, cie_1931_cmf, xyz_to_rgb_in_space, xyz_to_srgb_gamma};
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for crates/indicatrix/tests/ and same-crate \
              callers outside this module tree"
)]
pub(crate) use color::{
    apply_von_kries_white_balance, compute_illuminant_white_balance, illuminant_temperature_k,
    integrate_channels_to_xyz, integrate_channels_to_xyz_families, spectral_mis_weight,
};

// absorption.rs
pub use absorption::spectral_absorption;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for same-crate callers outside this \
              module tree"
)]
pub(crate) use absorption::{channel_absorption_alphas_assigned, signed_frame_rotation_psi};
// `channel_absorption_alphas` (the OLD DOP-blended function -- see its own doc comment)
// is only referenced by this crate's own tests and `raytracer::scattering`'s tests now
// that `channel_absorption_alphas_assigned` has replaced it on the interior transport
// path -- the Tier 2 GPU harness exercises `birefringence::pleochroic_channel_alpha`
// directly instead, so (unlike `henyey_greenstein_phase` just below) this one does NOT
// need a `feature = "gpu"` gate.
#[cfg(test)]
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::channel_absorption_alphas path; only \
              named by this crate's own tests"
)]
pub(crate) use absorption::channel_absorption_alphas;

// refraction.rs
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for same-crate callers outside this \
              module tree"
)]
pub(crate) use refraction::{
    BounceRefractionGeometry, RayMaterialContext, per_channel_uniaxial_indices, theta_c_for_bounce,
    tir_phase_delta,
};

// scattering.rs
#[cfg(any(test, feature = "gpu"))]
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::henyey_greenstein_phase path; only named by \
              this crate's own tests or the gpu-feature Tier 2 harness, not always both"
)]
pub(crate) use scattering::henyey_greenstein_phase;
#[allow(
    unused_imports,
    reason = "preserves the pre-split raytracer::X path for renderer::gpu::transport_check's \
              Tier 2 harness (feature = \"gpu\"), which this build may not compile"
)]
pub(crate) use scattering::{
    apply_frosted_bounce, cosine_weighted_hemisphere, maybe_scatter_or_extinguish,
    sample_henyey_greenstein_direction,
};

// Pure re-export shim of `scattering::balance_heuristic` for the pre-split
// `raytracer::balance_heuristic` path -- only named by `renderer::gpu::transport_check`'s
// Tier 2 harness (`nee.rs`), which this build may not compile without `feature = "gpu"`.
#[cfg(feature = "gpu")]
#[inline]
#[must_use]
pub(crate) fn balance_heuristic(pdf_a: f32, pdf_b: f32) -> f32 {
    scattering::balance_heuristic(pdf_a, pdf_b)
}

// transport.rs
pub use transport::{
    PathTermination, trace_spectral_ray, trace_spectral_ray_with_finish,
    trace_spectral_ray_with_finish_instrumented, trace_spectral_ray_with_finish_soa,
    wrapped_hero_wavelengths,
};

//! Production GPU frame rendering: the general entry point to the `transport_main`
//! megakernel that `renderer::gpu`'s self-tests verify.
//!
//! Every other public entry point in `renderer::gpu` is a *check*: it renders one
//! hardcoded scene and reports a verdict. This module is the callable counterpart --
//! hand it a scene, camera and resolution and get pixels back -- and introduces no
//! physics of its own. [`encode_and_dispatch`] is the single buffer-binding/dispatch
//! routine, shared with `estimator_check::dispatch_transport`, so the Tier 2/Tier 3
//! equivalence checks exercise exactly the code this module ships.
//!
//! # Chunking
//!
//! `transport_main` writes up to four output buffers per (pixel, sample) thread: final
//! XYZ plus three 8-channel debug arrays (radiance, lambdas, `path_pdf`), 27 floats
//! total. Self-tests read the debug arrays; this module's production dispatch never
//! does, and `GpuTransportParams::write_debug_buffers` (default "on") lets it skip
//! those three writes entirely -- [`TransportOutputs::new_production`] backs the
//! skipped writes with tiny fixed-size buffers instead of chunk-sized ones. A chunk's
//! byte budget is thus spent on 3 floats/tuple, not 27, roughly 9x more samples per
//! [`CHUNK_BUDGET_BYTES`] dispatch.
//!
//! A frame is split into pixel chunks that fit [`CHUNK_BUDGET_BYTES`], dispatched in
//! sequence. `GpuTransportParams::pixel_offset` tells the shader where a chunk starts,
//! so camera-ray generation and the per-pixel Cranley-Patterson rotations stay a
//! function of the pixel's true place in the frame while output slots stay
//! chunk-local. Chunking is over pixels, never samples: a pixel's samples all land in
//! one dispatch, since each thread's output depends only on its own `(pixel,
//! sample_num)` and splitting would only re-upload the scene per sample for nothing.
//!
//! # Time-budgeted chunk sizing
//!
//! [`CHUNK_BUDGET_BYTES`] alone sizes a chunk by output bytes, with no notion of how
//! long that takes a given adapter -- a chunk sized for a fast discrete GPU could take
//! 2-3s on a slow integrated one, inside Windows' ~2s TDR watchdog window.
//! [`GpuFrameRenderer::next_chunk_pixels`] sizes each chunk after the first toward
//! [`TARGET_CHUNK_MS`], from an exponential moving average of ns-per-tuple measured
//! dispatch-submission to readback-completion (see
//! [`GpuFrameRenderer::drain_pending_chunk`]). `chunk_budget_bytes` stays a hard
//! ceiling this can only shrink below, never exceed. Before any measurement exists,
//! dispatches use [`FIRST_DISPATCH_MAX_TUPLES`] instead, so a cold adapter's first
//! chunk alone cannot trip a TDR.
//!
//! # Cancellation
//!
//! [`GpuFrameRenderer::accumulate_cancellable`] checks a caller-supplied `AtomicBool`
//! between chunks and can stop early -- see [`AccumulateOutcome`] for the
//! drain-then-discard guarantee this makes about `accum`. [`GpuFrameRenderer::accumulate`]
//! delegates to the same loop with `cancel: None`, which can never fire.
//!
//! # Overlapped chunk pipeline
//!
//! `accumulate` alternates between TWO [`TransportOutputs`] (`self.outputs[chunk_index %
//! 2]`), each paired with its own persistent [`StagingSlot`]: chunk i's dispatch and
//! readback-copy are submitted (non-blocking) BEFORE chunk i-1's result is
//! mapped/read/summed, so chunk i's GPU work is already queued while the CPU blocks on
//! chunk i-1's map. Reusing a slot two chunks later is safe because a chunk's own
//! result is always drained before its slot is reused -- the pending queue is exactly
//! one chunk deep. This changes only WHEN a result is read back, never WHAT is
//! computed, so results stay bit-identical ([`run_chunk_equivalence`] checks this).
//!
//! # Per-frame uploads, persistent staging
//!
//! Of `transport_main`'s five scene-input buffers, only [`GpuTransportParams`]
//! (`pixel_offset`, chunk pixel count) differs between chunks of one frame; the other
//! four (camera, material, facet geometry, facet finishes) are PERSISTENT across every
//! `accumulate` call, not merely uploaded once per call -- see
//! [`FrameSceneBuffers::ensure`], which either creates them (first call) or updates them
//! in place via `queue.write_buffer` (every later call), avoiding a four-buffer
//! reallocation on every call. [`GpuTransportParams`] itself is likewise a persistent
//! per-output-slot buffer (see [`GpuFrameRenderer::params_buffers`]), written rather than
//! recreated each chunk. Likewise [`GpuFrameRenderer::staging`] holds two persistent
//! [`StagingSlot`]s, grown -- never shrunk -- only when
//! [`GpuFrameRenderer::ensure_staging_capacity`] finds one too small (see
//! [`staging_needs_growth`]), mirroring [`GpuFrameRenderer::ensure_capacity`]'s policy
//! for `outputs`. [`dispatch_chunk`] therefore uploads nothing at all via
//! [`build_chunk_bind_group`] -- only `queue.write_buffer`s into already-allocated
//! buffers. The wasm32 [`GpuFrameRenderer::accumulate_async`] path has no such overlap
//! and keeps uploading through [`build_bind_group`] unchanged.
//!
//! # Material-class kernel specialisation
//!
//! `transport_main` handles three material classes (isotropic/cubic, uniaxial,
//! biaxial) with runtime branches in one megakernel; a plain isotropic dispatch
//! otherwise still carries every class's per-ray register state, since
//! `is_anisotropic`/`is_biaxial` are ordinary runtime booleans no compiler can prove
//! false ahead of time. [`compute::create_compute_pipeline_with_constants`] fixes
//! `spectral_transport.wgsl`'s `MATERIAL_CLASS` pipeline-overridable constant at
//! pipeline-creation time; [`GpuFrameRenderer`] builds one specialised pipeline per
//! class LAZILY, on first use of that class, alongside the GENERIC pipeline every
//! self-test still compiles -- lazy so a session that only ever renders one or two
//! classes never pays every class's shader-compile cost.
//!
//! [`classify_material`] is the single place that decision is made, and MIRRORS (never
//! duplicates) `renderer::buffers::GpuGemMaterial::encode`'s own
//! `is_anisotropic`/`has_biaxial_delta` derivation: biaxial takes priority, then
//! uniaxial (non-cubic with birefringence magnitude above the same 1e-4 threshold
//! `encode` uses), else isotropic. `accumulate_via_pipeline` lets
//! [`run_chunk_equivalence`]/[`run_specialisation_equivalence`] force a specific
//! pipeline for the same material, which is what proves the two agree.
//!
//! **The GENERIC and specialised pipelines are NOT guaranteed byte-identical on the
//! same input, and this module does not require it.** Removing the
//! anisotropic/biaxial branches changes the compiled kernel's register pressure and
//! scheduling enough that a stochastic branch comparison (Fresnel reflect-vs-refract,
//! Russian roulette) can round 1 ULP differently between pipelines, flipping which
//! branch a handful of (pixel, sample) tuples take -- isolated, large-delta-per-pixel,
//! the signature of a flipped branch rather than a bug. Tolerated the same way the
//! CPU/GPU estimators already are (every stochastic decision divides by its own
//! locally recomputed probability, so either branch stays unbiased): verified via
//! [`run_specialisation_equivalence`] gating on GPU dispatch DETERMINISM (the same
//! specialised pipeline, dispatched twice, must be byte-identical) plus a diagnostic
//! diff count, while `estimator_check::run_specialisation_image_comparison` is the
//! rigorous GENERIC-vs-specialised correctness gate (Tier 3 statistical image
//! comparison).
//!
//! # What this does NOT do
//!
//! - **No guide buffers.** The megakernel returns radiance only, with no first-hit
//!   depth/normal/facet-id for the A-Trous denoiser to key on. `apps/indicatrix-cut`'s
//!   `bridge::guide_pass` regenerates them locally from one un-jittered primary ray per
//!   pixel instead, reused unchanged from the remote path.
//! - **Material routing is a contract, not a restriction today.**
//!   `GemMaterial::gpu_supported` is this crate's routing predicate and
//!   [`GpuFrameRenderer::accumulate`] enforces it ([`GpuFrameError::UnsupportedMaterial`]).
//!   Currently unconditionally `true` -- every built-in material, including the
//!   biaxial ones, is ported and verified against this module's Tier 2 per-function ULP
//!   budgets -- but stays enforced because a future material kind the megakernel cannot
//!   handle would flip it. Every
//!   Beer-Lambert/extinction-transmittance site whose CPU twin calls
//!   `crate::simd::exp_f32x8` -- `apply_absorption` (`absorption.rs`), the achromatic
//!   scatter/survive branches of `maybe_scatter_or_extinguish`, and
//!   `nee_contribution_hg_scatter`'s medium transmittance (all in `scattering.rs`) --
//!   has a WGSL twin (`exp_poly`, `transport_physics.wgsl`) that ports `exp_f32x8`'s own
//!   scalar reference op-for-op (same Cody-Waite range reduction, same `mul_add`/`fma`
//!   placement, same polynomial coefficients), not the `exp()` builtin, at every one of
//!   those call sites in `transport_physics.wgsl`/`transport_bounce.wgsl`/
//!   `transport_functions.wgsl`. `spectral_absorption`'s Gaussian band-shape evaluation
//!   (`optics::absorption::AbsorptionBand::evaluate`) is a DIFFERENT case: the CPU
//!   genuinely calls `f32::exp()` there (not `exp_f32x8`), so its WGSL twin keeps the
//!   plain `exp()` builtin too -- op-for-op identical source on both sides, the residual
//!   ULP gap there is this adapter's hardware-transcendental-vs-`libm` rounding
//!   difference, not a twin divergence, and stays within its own ULP budget, never 0. No
//!   CPU-vs-GPU comparison in this crate should ever be tightened to bit-exact on that
//!   basis -- but the Beer-Lambert-family sites above ARE
//!   bit-exact (0 genuine ULP), since both sides run the identical polynomial.
//! - **HDR environment maps.** `EnvironmentSource::HdrMap` renders on the
//!   GPU: `env_mode == transport_env_mode::HDR_MAP` routes the megakernel's miss-branch
//!   and exit-splitting environment lookups through `hdr_env_radiance_at`
//!   (`spectral_transport.wgsl`), which ports [`crate::renderer::env_map::EnvironmentMap`]'s
//!   `direction_to_uv`/`sample_bilinear`/`radiance_at` -- pure trig and bilinear-blend
//!   arithmetic, no `exp()` in this part of the path -- verified against
//!   [`crate::renderer::gpu::environment_check`]'s `run_hdr_env_radiance` ULP check (see
//!   `renderer::env_map_gpu::HdrEnvGpuData`'s doc comment for the texel-buffer layout),
//!   **not** claimed bit-exact: `run_hdr_env_radiance` gates on a ULP budget, not `== 0`,
//!   the same way every other Tier 2 check in this module does, for any material whose
//!   absorption/scattering feeds into what gets looked up.
//!   The CPU (`optics::raytracer::transport::trace_spectral_ray_inner`'s own
//!   white-balance step) and this GPU path both skip the Von-Kries white-balance round
//!   trip for `HdrMap` --
//!   `env_mode == 1u` (`Studio`) gates it on the GPU, `EnvironmentSource::HdrMap(_) => xyz`
//!   (no-op) on the CPU -- so a hybrid CPU/GPU HDR frame never disagrees by that
//!   transform's own non-exact-inverse floor (`max|B*A - I| ~= 5.2e-7`), the way it would
//!   if only one side applied the transform. [`GpuFrameError::UnsupportedEnvironment`] is
//!   reachable, not merely unreachable-but-enforced future-proofing (mirroring
//!   [`GpuFrameError::UnsupportedMaterial`]'s own status): [`build_hdr_env`] returns it
//!   for an HDR map whose texel buffer would exceed the
//!   device's `max_storage_buffer_binding_size` (see
//!   [`crate::renderer::env_map_gpu::HdrEnvGpuData::fits_storage_binding`]).
//!
//! # Wavefront pipeline
//!
//! [`GpuPipelineKind::Wavefront`] (selected via [`GpuFrameRenderer::set_pipeline_kind`]/
//! [`crate::renderer::gpu_backend::GpuBackend::set_pipeline_kind`], `Megakernel` is
//! still the default for every existing caller) dispatches the same per-ray physics
//! through a different kernel SHAPE: ray state in storage buffers instead of per-thread
//! locals, one bounce of every still-alive ray as its own dispatch, so a divergent
//! material no longer keeps a whole warp/wavefront resident through the entire bounce
//! loop the way the megakernel's single mega-dispatch does. See
//! `shaders/wavefront_transport.wgsl`'s own header comment for the full kernel sequence
//! and determinism argument; this section covers the HOST side.
//!
//! ## Buffer layout
//!
//! [`WavefrontRayBuffers`] is one flat struct-of-arrays buffer per ray-state field
//! (`vec3<f32>` fields stored as `vec4<f32>`, `w` unused), sized to `chunk_rays =
//! pixels_this_chunk * spp` -- the same `tuples` the megakernel path's
//! [`TransportOutputs`] is sized to, so both pipelines share
//! `outputs`/`pixel_output`/the reduce/staging/readback tail unchanged (see
//! [`GpuFrameRenderer::dispatch_chunk`]'s own comment at its pipeline-kind branch).
//! Deliberately NOT persistent/grown-never-shrunk like `outputs`: allocated FRESH every
//! `dispatch_chunk_wavefront` call, sized exactly to that chunk -- a documented
//! simplification for this first cut (see [`GpuFrameRenderer::dispatch_chunk_wavefront`]'s
//! own doc comment for why). Also deliberately NOT storing the megakernel's per-ray
//! "hoisted" material-derived constants (dispersion/absorption arrays, the biaxial axis
//! frame, hero indices, studio rig directions): `wavefront_bounce` recomputes them fresh
//! every bounce round from each ray's own stored `lambdas` instead (see
//! `transport_hoist_ray_constants`'s doc comment in `shaders/transport_bounce.wgsl`).
//!
//! ## Per-bounce traffic estimate
//!
//! Each bounce round reads and writes essentially the full per-ray state for every
//! still-alive ray -- roughly 360 bytes: four `vec4<f32>` fields (`origin`, `dir`, `k`,
//! `prev_plane_normal`, 64 bytes), one `u32` flags word, six eight-channel arrays
//! (`stokes`, `radiance`, `path_pdf`, `split_radiance`, `compat`, `lambdas`, about 288
//! bytes total), plus `pending_light_mis`/`seed` (8 bytes). At `max_bounces` rounds with
//! no compaction shrinkage this is `max_bounces` times the megakernel's total
//! register/cache traffic for the same rays, paid as actual VRAM bandwidth instead
//! of registers -- the trade this pipeline makes is register-pressure
//! relief in exchange for this bandwidth and the compaction round's
//! own synchronous host stall (see [`GpuFrameRenderer::run_wavefront_bounce_rounds`]'s
//! doc comment).
//!
//! ## When to prefer which
//!
//! Unmeasured as of this writing -- [`GpuPipelineKind::Megakernel`] stays the default
//! for every caller until the owner profiles both on real scenes. The wavefront
//! pipeline's theoretical advantage is scenes with high bounce-to-bounce divergence
//! (mixed material classes, frequent early ray termination via Russian roulette or
//! misses) where the megakernel's whole-warp occupancy is dominated by its slowest
//! rays; its theoretical cost is the per-bounce struct-of-arrays read/write traffic
//! above plus this implementation's synchronous compaction stall, both of which the
//! megakernel avoids entirely by keeping everything in registers for one dispatch's
//! whole lifetime.
//!
//! ## Equivalence
//!
//! `renderer::gpu::transport_check::run_pipeline_equivalence` renders the same chunk
//! through both pipelines and asserts bit-identical `out_xyz` per pixel; a companion
//! determinism check runs the wavefront pipeline twice and asserts the same.
//!
//! # Module layout
//!
//! Split from one file into this folder along real seams: [`scene_buffers`] owns the
//! persistent scene/output GPU buffers, [`bind_groups`] builds pipelines and bind
//! groups (including the wavefront kernel's five pipelines), [`specialisation`] is the
//! material-class routing decision, [`renderer`] is [`GpuFrameRenderer`] itself plus its
//! construction/capacity-management methods, [`accumulate`]/[`dispatch`] split the
//! per-frame accumulate loop from the per-chunk GPU dispatch, [`readback`] is the
//! GPU-side reduce and CPU readback, [`async_impl`] is the wasm32-only async
//! counterpart, and [`equivalence`] holds the bit-exactness self-tests every other
//! module's doc comments reference.

use std::{sync::atomic::AtomicBool, time::Instant};

use glam::Vec3;

use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
    renderer::gpu::GpuAcquireError,
};

mod accumulate;
#[cfg(target_arch = "wasm32")]
mod async_impl;
mod bind_groups;
mod dispatch;
mod equivalence;
mod readback;
mod renderer;
mod scene_buffers;
mod specialisation;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "gpu"))]
mod gpu_hardware_tests;

// `GpuPipelineKind` itself is defined in `renderer::gpu_backend`, NOT
// here, and re-exported -- see that module's own doc comment on why `GpuBackend` exists
// in both build configurations (with and without `feature = "gpu"`) and its types must
// therefore be reachable without the feature too. Re-exporting it here (rather than every
// caller reaching into `gpu_backend` directly) keeps `GpuFrameRenderer::set_pipeline_kind`'s
// own signature reading naturally as `renderer::gpu::frame::GpuPipelineKind`.
pub use crate::renderer::gpu_backend::GpuPipelineKind;

pub(crate) use bind_groups::TransportDispatchArgs;
pub(crate) use dispatch::encode_and_dispatch;
pub use equivalence::{
    ChunkEquivalenceResult, PipelineEquivalenceResult, SpecialisationCaseResult,
    SpecialisationEquivalenceResult, run_chunk_equivalence, run_pipeline_equivalence,
    run_specialisation_equivalence, run_wavefront_determinism,
};
pub(crate) use readback::GpuReduceParams;
pub use renderer::GpuFrameRenderer;
pub(crate) use scene_buffers::TransportOutputs;
pub(crate) use specialisation::{classify_material, material_class};

/// `spectral_transport.wgsl` alone is not valid WGSL -- it assumes
/// `shaders/transport_physics.wgsl`'s functions are already in scope, and `build.rs`
/// concatenates the two. Shared with `estimator_check` so both compile the identical
/// source text.
pub(crate) const SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/spectral_transport.generated.wgsl"
));

/// `reduce_xyz.wgsl`'s source -- a standalone kernel, unlike [`SHADER_SRC`] never
/// concatenated with `transport_physics.wgsl` by `build.rs` (see that file's own doc
/// comment: it only concatenates `spectral_transport.wgsl` and
/// `transport_functions.wgsl`). Pulled in directly via `include_str!` instead. See
/// [`GpuFrameRenderer::dispatch_chunk`]'s doc comment for why this kernel exists.
const REDUCE_SHADER_SRC: &str = include_str!("../../shaders/reduce_xyz.wgsl");

/// `shaders/wavefront_transport.wgsl`'s generated source --
/// `shaders/transport_physics.wgsl` + `shaders/transport_bounce.wgsl` +
/// `shaders/wavefront_transport.wgsl`, concatenated by `build.rs` the same way
/// [`SHADER_SRC`] is (see that constant's own doc comment and `build.rs`'s own doc
/// comment on `generate_transport_shaders`). Used only when
/// [`GpuPipelineKind::Wavefront`] is selected -- see [`GpuFrameRenderer::set_pipeline_kind`].
const WAVEFRONT_SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/wavefront_transport.generated.wgsl"
));

/// Floats this module's production dispatch writes and reads back per (pixel, sample)
/// thread: XYZ only. The chunk budget is sized against this, not the shader's full
/// 27-floats-per-tuple capacity -- see the module doc comment's "Chunking" section.
const FLOATS_PER_TUPLE: usize = 3;

/// Fixed, deliberately tiny float count backing each of a production
/// [`TransportOutputs`]'s three debug buffers (`radiance`/`lambdas`/`path_pdf`) -- with
/// `write_debug_buffers = 0` the shader never writes them, so they only need to satisfy
/// the megakernel's static bind-group layout, never hold real per-chunk data. One tuple's
/// worth is an arbitrary safe size that never needs to grow with chunk size.
const DEBUG_BUFFER_FLOATS: usize = 8;

/// Output-buffer budget for a single dispatch, in bytes.
///
/// 64 MiB is well inside what an integrated GPU will allocate without complaint, and
/// keeps a chunk short enough that a progressive frame stays responsive. It bounds only
/// how many chunks a frame takes, never what a frame can contain.
pub const CHUNK_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Target wall-clock duration for one dispatch-to-drain cycle, in milliseconds.
///
/// [`GpuFrameRenderer::next_chunk_pixels`] sizes every chunk after the first toward this,
/// from an EMA of measured ns/tuple (see [`GpuFrameRenderer::ns_per_tuple_ema`]) -- a slow
/// integrated GPU converges to small (TDR-safe) chunks, a fast discrete one to large
/// (fewer dispatches) ones, without a hand-tuned [`CHUNK_BUDGET_BYTES`] per machine.
/// `chunk_budget_bytes` stays the hard ceiling this can never exceed.
const TARGET_CHUNK_MS: f64 = 150.0;

/// Hard floor on a time-budgeted chunk: [`MIN_CHUNK_BYTES`] worth of tuples, so a
/// pessimistic EMA can never shrink a chunk small enough that fixed per-dispatch overhead
/// (encoder creation, submission, polling) dominates its actual GPU work.
const MIN_CHUNK_BYTES: usize = 64 * 1024;

/// Tuples the very first dispatch(es) of a freshly constructed [`GpuFrameRenderer`] use,
/// regardless of `chunk_budget_bytes` -- with no measured per-tuple cost yet, a
/// byte-budget-only guess could take 2-3s on a cold integrated GPU, inside Windows' ~2s
/// TDR window. Every dispatch after the EMA has a measurement is sized from it instead.
const FIRST_DISPATCH_MAX_TUPLES: usize = 1_000_000;

/// Smoothing factor for [`GpuFrameRenderer::ns_per_tuple_ema`]'s exponential moving
/// average -- low enough that one unusual dispatch (cold cache, driver hiccup) doesn't
/// whipsaw the next chunk's size, high enough to track a real throughput change quickly.
const CHUNK_TIMING_EMA_ALPHA: f64 = 0.3;

/// `transport_main`'s declared `@workgroup_size(64)` -- a dispatch of `n` workgroups
/// covers `n * WORKGROUP_SIZE` (pixel, sample) tuples.
const WORKGROUP_SIZE: usize = 64;

/// The lowest cross-backend guarantee for `maxComputeWorkgroupsPerDimension`
/// (WebGPU/Vulkan/D3D12's shared floor) -- `dispatch_workgroups(x, 1, 1)` is only valid
/// for `x <=` this. `transport_main` indexes purely off `global_invocation_id.x`, so every
/// dispatch here is one-dimensional and this bounds how many tuples ONE dispatch may
/// cover, independent of [`CHUNK_BUDGET_BYTES`] -- an 800x600 frame at 32 spp can exceed
/// it in a single chunk if not capped. [`dispatch::chunk_pixels_for`] caps against this in
/// addition to the byte budget so a chunk is never too large to dispatch.
const MAX_WORKGROUPS_PER_DIMENSION: usize = 65_535;

/// Why a GPU frame could not be produced. Every variant is a condition the caller is
/// expected to handle by falling back to the CPU tracer -- none is a bug.
#[derive(Debug)]
pub enum GpuFrameError {
    /// No usable adapter or device on this machine. Expected on plenty of systems; see
    /// [`GpuContext::acquire`](crate::renderer::gpu::GpuContext::acquire).
    Acquire(GpuAcquireError),
    /// The scene's environment cannot be dispatched as-is.
    ///
    /// Every [`EnvironmentSource`] variant has an `env_mode` (including `HdrMap`,
    /// see [`dispatch::environment_params`]), so this is not an unreachable-by-variant
    /// case. It IS reachable:
    /// [`scene_buffers::build_hdr_env`] returns it when
    /// [`HdrEnvGpuData::fits_storage_binding`](crate::renderer::env_map_gpu::HdrEnvGpuData::fits_storage_binding)
    /// says an HDR map's texel buffer would exceed the device's
    /// `max_storage_buffer_binding_size`. A caller sees this exactly like any other
    /// decline (fall back to the CPU tracer, which has no such buffer-size ceiling) --
    /// see `renderer::gpu_backend::GpuBackend::try_accumulate_cancellable`'s non-`DeviceLost`
    /// `Err` arm.
    UnsupportedEnvironment,
    /// The device cannot bind enough storage buffers in one compute stage for the
    /// megakernel (`needed` is `MEGAKERNEL_STORAGE_BUFFERS`). WebGPU's baseline is 8;
    /// the adapter offered `available`. The backend declines to the CPU tracer.
    DeviceLimits { needed: u32, available: u32 },
    /// The scene's material is one [`GemMaterial::gpu_supported`] rejects. Currently
    /// unreachable (the predicate is unconditionally `true`); kept as defensive
    /// future-proofing since `gpu_supported` is the crate's routing contract. Carries the
    /// material's name, for a log line that says which stone.
    UnsupportedMaterial(String),
    /// The device stopped making forward progress -- a `Device::poll` timeout/error, a
    /// buffer-mapping callback failure, or an unreadable mapped range. Carries a
    /// human-readable message for the caller's log line. Unlike every other variant here,
    /// this is NOT a per-scene routing decision: `renderer::gpu_backend::GpuBackend`
    /// treats it as a signal to permanently stop using this renderer for the rest of the
    /// process, not merely to fall this one call back to the CPU.
    DeviceLost(String),
}

impl std::fmt::Display for GpuFrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Acquire(e) => write!(f, "no usable GPU adapter or device: {e:?}"),
            Self::DeviceLimits { needed, available } => write!(
                f,
                "the GPU device allows {available} storage buffers per compute stage; the transport megakernel needs {needed}"
            ),
            Self::UnsupportedEnvironment => {
                write!(f, "the GPU megakernel has no env_mode for this environment")
            }
            Self::UnsupportedMaterial(name) => write!(
                f,
                "{name} is not GPU-supported (GemMaterial::gpu_supported), so it renders on the CPU"
            ),
            Self::DeviceLost(why) => write!(
                f,
                "GPU device lost or unresponsive, disabling GPU rendering for the rest of this process: {why}"
            ),
        }
    }
}

impl std::error::Error for GpuFrameError {}

/// Everything the megakernel needs to render a frame.
///
/// Bundled so [`GpuFrameRenderer::accumulate`] stays within clippy's argument-count limit and so
/// the caller assembles the scene once rather than per chunk.
pub struct GpuFrameScene<'a> {
    pub camera: &'a Camera,
    pub width: u32,
    pub height: u32,
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane finish, indexed in step with `planes`. A shorter slice is padded with
    /// [`FacetFinish::default`], matching [`crate::renderer::buffers::encode_facet_finishes`].
    pub facet_finishes: &'a [FacetFinish],
    pub material: &'a GemMaterial,
    pub max_bounces: u32,
    pub environment: EnvironmentSource<'a>,
}

/// Outcome of [`GpuFrameRenderer::accumulate_cancellable`].
///
/// Lets a caller tell "every requested sample landed in `accum`" apart from "cancelled,
/// `accum` untouched" -- which a plain `bool` cannot, since [`GpuFrameRenderer::accumulate`]'s
/// `Ok(())`/`Err(GpuFrameError)` shape already means something else (decline reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccumulateOutcome {
    /// Every requested sample was traced and summed into `accum`.
    Done,
    /// `cancel` was observed set before every chunk finished.
    ///
    /// `accum` is GUARANTEED untouched: drain-then-discard semantics mean any chunk
    /// already in flight when cancellation was observed is still waited on and read off
    /// the GPU -- so double-buffered chunk-output state stays consistent for the next
    /// call -- but that chunk's samples are thrown away rather than summed. A caller
    /// never has to reason about a partial sample count: `accum`/`sample_offset`
    /// bookkeeping stay exactly as if this call had never been made.
    Cancelled,
}

/// Resumable progress through one [`GpuFrameRenderer::accumulate_turn`] request, owned
/// by the CALLER (`renderer::gpu_backend::GpuBackend`), never by [`GpuFrameRenderer`]
/// itself -- so several concurrent requests sharing one renderer each keep their OWN
/// cursor across turns, letting them interleave chunk-batch by chunk-batch. See
/// `gpu_backend`'s module doc comment for the fairness model this exists for.
///
/// `Default` is the start-of-request state: nothing dispatched yet.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ChunkCursor {
    pub(crate) first_pixel: usize,
    pub(crate) chunk_index: usize,
}

/// Outcome of one [`GpuFrameRenderer::accumulate_turn`] call.
///
/// Like [`AccumulateOutcome`], but with a third possibility: a turn's chunk budget
/// (`max_chunks`) can run out with pixels of the SAME request still left to render,
/// which "done vs. cancelled" cannot express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkTurnOutcome {
    /// Every requested sample was traced and summed into `accum` -- the whole request
    /// is finished. Matches [`AccumulateOutcome::Done`].
    Done,
    /// `cancel` fired during this turn. Matches [`AccumulateOutcome::Cancelled`]'s
    /// drain-then-discard guarantee about `accum`.
    Cancelled,
    /// This turn's `max_chunks` budget was spent, but pixels remain. Every chunk
    /// dispatched THIS turn is already summed into `accum` (nothing here is discarded,
    /// unlike `Cancelled`), and the renderer's one-deep double-buffered pipeline is
    /// fully drained -- nothing is left in flight, so it is safe for a completely
    /// different request to take the next turn on this same renderer before this
    /// request's `cursor` is passed to another [`GpuFrameRenderer::accumulate_turn`]
    /// call to resume it.
    MoreWork,
}

/// Bundles [`GpuFrameRenderer::accumulate_turn`]'s per-REQUEST inputs -- the values that
/// stay fixed across however many turns it takes to resume one request, as opposed to
/// `accum`/`cursor`, which a caller mutates turn by turn. Exists purely to bring
/// `accumulate_turn`'s argument count within clippy's `too_many_arguments` limit, the
/// same reason [`ChunkFrameState`]/[`bind_groups::TransportDispatchArgs`] exist.
///
/// `renderer::gpu_backend::GpuBackend::try_accumulate_cancellable` builds one of these
/// once per request (outside its turn-taking loop) and reuses it, unchanged, for every
/// turn; [`GpuFrameRenderer::accumulate_via_pipeline`] does the same with a fixed
/// `max_chunks: usize::MAX`.
pub(crate) struct TurnRequest<'a> {
    pub(crate) scene: &'a GpuFrameScene<'a>,
    /// One of [`material_class`]'s values -- see `scene.material` and
    /// [`classify_material`].
    pub(crate) pipeline_class: u32,
    pub(crate) sample_offset: u32,
    pub(crate) spp: u32,
    /// `None` means "never cancel" (every self-test, [`GpuFrameRenderer::accumulate`]).
    pub(crate) cancel: Option<&'a AtomicBool>,
    /// How many chunks ONE call to [`GpuFrameRenderer::accumulate_turn`] may dispatch
    /// before returning control to the caller, regardless of how many pixels remain --
    /// see [`ChunkTurnOutcome::MoreWork`]. `usize::MAX` (every self-test,
    /// [`GpuFrameRenderer::accumulate_via_pipeline`]) means "run the whole request in one
    /// turn"; `renderer::gpu_backend`'s fairness-driven caller uses a small fixed bound
    /// (`CHUNKS_PER_TURN`) instead.
    pub(crate) max_chunks: usize,
}

/// The per-frame values [`GpuFrameRenderer::dispatch_chunk`] needs but that never change
/// between chunks of the SAME [`GpuFrameRenderer::accumulate_via_pipeline`] call --
/// computed once and threaded through by reference, purely so both functions stay under
/// clippy's argument-count and function-length limits.
///
/// `gpu_material`/`gpu_finishes` are uploaded ONCE into a [`scene_buffers::FrameSceneBuffers`]
/// alongside this state instead of living here, since their bytes never change between
/// chunks either -- see [`GpuFrameRenderer::accumulate_via_pipeline`].
pub(crate) struct ChunkFrameState<'a> {
    pub(crate) scene: &'a GpuFrameScene<'a>,
    pub(crate) pipeline_class: u32,
    pub(crate) sample_offset: u32,
    pub(crate) spp: u32,
    pub(crate) env_mode: u32,
    pub(crate) temp_k: f32,
    pub(crate) spot_mult: f32,
    pub(crate) exposure: f32,
    pub(crate) light_yaw: f32,
    pub(crate) light_pitch: f32,
    pub(crate) white_balance: Vec3,
    /// See [`dispatch::environment_params`]'s own doc comment.
    pub(crate) use_d65: bool,
    pub(crate) studio_model: u32,
    pub(crate) backdrop: f32,
}

/// The GPU-side preparation [`GpuFrameRenderer::accumulate_turn`] must redo on EVERY
/// turn -- pipeline selection, buffer-capacity growth, and refreshing the four
/// persistent [`scene_buffers::FrameSceneBuffers`] -- before it can dispatch a single
/// chunk. Bundles [`GpuFrameRenderer::prepare_turn`]'s three outputs purely to keep
/// `accumulate_turn` under clippy's function-length limit; see that function's own doc
/// comment for why this setup cannot be skipped on a resumed turn.
pub(crate) struct TurnSetup<'a> {
    pub(crate) state: ChunkFrameState<'a>,
    pub(crate) frame_buffers: scene_buffers::FrameSceneBuffers,
    /// [`dispatch::chunk_pixels_for`]'s hard ceiling for this request, computed once
    /// here rather than once per chunk since it depends only on values fixed for the
    /// whole request.
    pub(crate) byte_budget_pixels: usize,
}

/// The four per-slot GPU resources [`GpuFrameRenderer::dispatch_chunk`] reads for one
/// chunk, bundled purely to keep that method under clippy's function-length limit.
pub(crate) struct ChunkSlotResources<'a> {
    pub(crate) outputs: &'a scene_buffers::TransportOutputs,
    pub(crate) pixel_output: &'a readback::PixelXyzOutput,
    pub(crate) staging_buffer: &'a wgpu::Buffer,
    pub(crate) params_buf: &'a wgpu::Buffer,
}

/// One in-flight chunk's readback state, between "dispatch and copy submitted" and
/// "mapped, read, and summed into `accum`" -- see [`GpuFrameRenderer::accumulate`]'s
/// pipeline.
pub(crate) struct PendingChunk {
    /// Index into [`GpuFrameRenderer::staging`] holding this chunk's persistent staging
    /// buffer -- always `chunk_index % 2`, matching `outputs`. The buffer itself lives in
    /// `self.staging` for [`GpuFrameRenderer::drain_pending_chunk`] to look up, so it
    /// survives being reused two chunks later.
    pub(crate) staging_slot: usize,
    pub(crate) copy_index: wgpu::SubmissionIndex,
    pub(crate) rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    pub(crate) first_pixel: usize,
    pub(crate) pixels_this_chunk: usize,
    /// Wall-clock time this chunk's dispatch was submitted -- the per-tuple timing
    /// measurement is `Instant::now()` at [`GpuFrameRenderer::drain_pending_chunk`] minus
    /// this, covering submit-to-map-completion.
    pub(crate) submitted_at: Instant,
}

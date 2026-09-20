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
//! in place via `queue.write_buffer` (every later call), eliminating the four-buffer
//! churn finding G3 flagged. [`GpuTransportParams`] itself is likewise now a persistent
//! per-output-slot buffer (see [`GpuFrameRenderer::params_buffers`]), written rather than
//! recreated each chunk. Likewise [`GpuFrameRenderer::staging`] holds two persistent
//! [`StagingSlot`]s, grown -- never shrunk -- only when
//! [`GpuFrameRenderer::ensure_staging_capacity`] finds one too small (see
//! [`staging_needs_growth`]), mirroring [`GpuFrameRenderer::ensure_capacity`]'s policy
//! for `outputs`. [`dispatch_chunk`] therefore uploads nothing at all via
//! [`build_chunk_bind_group`] -- only `queue.write_buffer`s into already-allocated
//! buffers. The wasm32 [`GpuFrameRenderer::accumulate_async`] path never had this overlap
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
//!   biaxial ones, is ported and verified at 0 ULP -- but stays enforced because a
//!   future material kind the megakernel cannot handle would flip it.
//! - **HDR environment maps (finding G6).** `EnvironmentSource::HdrMap` now renders on the
//!   GPU: `env_mode == transport_env_mode::HDR_MAP` routes the megakernel's miss-branch
//!   and exit-splitting environment lookups through `hdr_env_radiance_at`
//!   (`spectral_transport.wgsl`), which mirrors [`crate::renderer::env_map::EnvironmentMap`]'s
//!   `direction_to_uv`/`sample_bilinear`/`radiance_at` bit-for-bit (see
//!   `renderer::env_map_gpu::HdrEnvGpuData`'s doc comment for the texel-buffer layout and
//!   [`crate::renderer::gpu::environment_check`]'s `run_hdr_env_radiance` for the ULP
//!   check). [`GpuFrameError::UnsupportedEnvironment`] is kept for a genuinely
//!   unsupported future environment source, mirroring [`GpuFrameError::UnsupportedMaterial`]'s
//!   own currently-unreachable-but-enforced status -- no [`EnvironmentSource`] variant
//!   produces it today.
//!
//! # Wavefront pipeline (finding G5 Part B)
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
//! register/cache traffic for the same rays, now paid as actual VRAM bandwidth instead
//! of registers -- the trade this pipeline makes is exactly the register-pressure
//! relief finding G5 targets, in exchange for this bandwidth and the compaction round's
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

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use glam::Vec3;
use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::{CrystalSystem, GemMaterial},
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, LightingPreset,
            environment::environment_white_balance, illuminant_temperature_k,
        },
    },
    renderer::{
        buffers::{
            GpuCameraParams, GpuGemMaterial, GpuTransportParams, GpuWavefrontParams,
            encode_facet_finishes, transport_env_mode,
        },
        env_map_gpu::HdrEnvGpuData,
        gpu::{
            GpuAcquireError, GpuContext, MEGAKERNEL_STORAGE_BUFFERS, WAVEFRONT_STORAGE_BUFFERS,
            compute,
        },
    },
};

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
const REDUCE_SHADER_SRC: &str = include_str!("../shaders/reduce_xyz.wgsl");

/// Finding G5 Part B: `shaders/wavefront_transport.wgsl`'s generated source --
/// `shaders/transport_physics.wgsl` + `shaders/transport_bounce.wgsl` +
/// `shaders/wavefront_transport.wgsl`, concatenated by `build.rs` the same way
/// [`SHADER_SRC`] is (see that constant's own doc comment and `build.rs`'s own doc
/// comment on `generate_transport_shaders`). Used only when
/// [`GpuPipelineKind::Wavefront`] is selected -- see [`GpuFrameRenderer::set_pipeline_kind`].
const WAVEFRONT_SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/wavefront_transport.generated.wgsl"
));

/// `reduce_xyz.wgsl`'s uniform param struct (binding 0 there). `_pad0`/`_pad1` reproduce
/// WGSL's implicit padding of a two-`u32` uniform struct up to a 16-byte block, mirroring
/// `renderer::buffers`' own layout convention (see that module's doc comment) rather than
/// relying on `wgpu`'s own struct-size rounding for a uniform buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuReduceParams {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Floats this module's production dispatch writes and reads back per (pixel, sample)
/// thread: XYZ only. The chunk budget is sized against this, not the shader's full
/// 27-floats-per-tuple capacity -- see the module doc comment's "Chunking" section.
const FLOATS_PER_TUPLE: usize = 3;

// Finding G5 Part B: `GpuPipelineKind` itself is defined in `renderer::gpu_backend`, NOT
// here, and re-exported -- see that module's own doc comment on why `GpuBackend` exists
// in both build configurations (with and without `feature = "gpu"`) and its types must
// therefore be reachable without the feature too. Re-exporting it here (rather than every
// caller reaching into `gpu_backend` directly) keeps `GpuFrameRenderer::set_pipeline_kind`'s
// own signature reading naturally as `renderer::gpu::frame::GpuPipelineKind`.
pub use crate::renderer::gpu_backend::GpuPipelineKind;

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

/// Why a GPU frame could not be produced. Every variant is a condition the caller is
/// expected to handle by falling back to the CPU tracer -- none is a bug.
#[derive(Debug)]
pub enum GpuFrameError {
    /// No usable adapter or device on this machine. Expected on plenty of systems; see
    /// [`GpuContext::acquire`].
    Acquire(GpuAcquireError),
    /// The scene uses an environment the megakernel has no `env_mode` for.
    ///
    /// Currently unreachable: every [`EnvironmentSource`] variant (including `HdrMap`,
    /// since finding G6) has an `env_mode` -- see [`environment_params`]. Kept as
    /// defensive future-proofing, mirroring [`Self::UnsupportedMaterial`]'s own
    /// currently-unreachable-but-enforced status, since a future environment source this
    /// megakernel cannot handle should produce this rather than a plausible-looking wrong
    /// image.
    UnsupportedEnvironment,
    /// The device cannot bind enough storage buffers in one compute stage for the
    /// megakernel (`needed` is [`MEGAKERNEL_STORAGE_BUFFERS`]). WebGPU's baseline is 8;
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
                "the GPU device allows {available} storage buffers per compute stage; the                  transport megakernel needs {needed}"
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
    /// [`FacetFinish::default`], matching [`encode_facet_finishes`].
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
    first_pixel: usize,
    chunk_index: usize,
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

/// The four `transport_main` output buffers, sized for `capacity` (pixel, sample)
/// tuples.
///
/// Held across dispatches by [`GpuFrameRenderer`] and reallocated only when a frame
/// needs more capacity than the last one did -- the buffers are large (see the module
/// doc comment) and a progressive render re-dispatches the same geometry many times per
/// second, so recreating them every frame would dominate the frame's own cost.
pub(crate) struct TransportOutputs {
    xyz: wgpu::Buffer,
    radiance: wgpu::Buffer,
    lambdas: wgpu::Buffer,
    path_pdf: wgpu::Buffer,
    /// `out_compat`'s backing buffer -- see that binding's own doc comment in
    /// `spectral_transport.wgsl`. Same `write_debug_buffers`-gated, self-test-only
    /// contract as `radiance`/`lambdas`/`path_pdf` above.
    compat: wgpu::Buffer,
    capacity: usize,
}

impl TransportOutputs {
    pub(crate) fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = BufferUsages::STORAGE | BufferUsages::COPY_SRC;
        Self {
            xyz: compute::zeroed_buffer::<f32>(device, "transport out xyz", capacity * 3, usage),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "transport out radiance",
                capacity * 8,
                usage,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "transport out lambdas",
                capacity * 8,
                usage,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "transport out path_pdf",
                capacity * 8,
                usage,
            ),
            compat: compute::zeroed_buffer::<u32>(
                device,
                "transport out compat",
                capacity * 8,
                usage,
            ),
            capacity,
        }
    }

    /// Like [`Self::new`], but for `renderer::gpu::frame`'s production dispatch only --
    /// `xyz` is sized for `capacity` tuples, while the three debug buffers are fixed at
    /// [`DEBUG_BUFFER_FLOATS`] regardless of `capacity`, since a production dispatch's
    /// `write_debug_buffers` is always off and the shader never writes them.
    pub(crate) fn new_production(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = BufferUsages::STORAGE | BufferUsages::COPY_SRC;
        Self {
            xyz: compute::zeroed_buffer::<f32>(
                device,
                "transport out xyz (production)",
                capacity * 3,
                usage,
            ),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "transport out radiance (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "transport out lambdas (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "transport out path_pdf (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            compat: compute::zeroed_buffer::<u32>(
                device,
                "transport out compat (production, write_debug_buffers=0, unused)",
                DEBUG_BUFFER_FLOATS,
                usage,
            ),
            capacity,
        }
    }

    pub(crate) const fn xyz(&self) -> &wgpu::Buffer {
        &self.xyz
    }

    pub(crate) const fn radiance(&self) -> &wgpu::Buffer {
        &self.radiance
    }

    pub(crate) const fn lambdas(&self) -> &wgpu::Buffer {
        &self.lambdas
    }

    pub(crate) const fn path_pdf(&self) -> &wgpu::Buffer {
        &self.path_pdf
    }

    pub(crate) const fn compat(&self) -> &wgpu::Buffer {
        &self.compat
    }
}

/// One double-buffered slot's GPU-reduced per-pixel XYZ output -- `reduce_xyz_main`'s
/// destination buffer (see `reduce_xyz.wgsl`'s header comment and finding G2). Alternated
/// in lockstep with [`TransportOutputs`]/[`StagingSlot`] by
/// [`GpuFrameRenderer::dispatch_chunk`]: `reduce_xyz_main` sums a chunk's `out_xyz`
/// (`pixels * spp * 3` floats) down to `pixels * 3` floats here, and THIS buffer -- not
/// `out_xyz` -- is what the chunk's readback copy actually reads from.
struct PixelXyzOutput {
    buffer: wgpu::Buffer,
    /// Capacity in PIXELS (3 floats each) -- distinct from [`TransportOutputs::capacity`],
    /// which counts (pixel, sample) tuples.
    capacity: usize,
}

impl PixelXyzOutput {
    fn new(device: &wgpu::Device, capacity: usize) -> Self {
        let usage = BufferUsages::STORAGE | BufferUsages::COPY_SRC;
        Self {
            buffer: compute::zeroed_buffer::<f32>(
                device,
                "transport out pixel xyz (GPU-reduced)",
                capacity * 3,
                usage,
            ),
            capacity,
        }
    }
}

/// Bundles the material/scene/output-buffer inputs shared by [`build_bind_group`] and
/// [`encode_and_dispatch`] (and, on wasm32, [`GpuFrameRenderer::accumulate_async`]'s
/// inlined dispatch) -- purely to keep argument counts within clippy's
/// `too_many_arguments` limit. `total_tuples` stays its own parameter on the two dispatch
/// functions rather than joining this struct: it drives the workgroup count and the
/// output-capacity assert, and differs from `outputs.capacity` whenever a chunk is
/// smaller than its buffers' full capacity.
pub(crate) struct TransportDispatchArgs<'a> {
    pub(crate) ctx: &'a GpuContext,
    pub(crate) pipeline: &'a wgpu::ComputePipeline,
    pub(crate) camera_params: &'a GpuCameraParams,
    pub(crate) params: &'a GpuTransportParams,
    pub(crate) material: &'a GpuGemMaterial,
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) facet_finishes: &'a [u32],
    pub(crate) outputs: &'a TransportOutputs,
    /// Finding G6/G7: bindings 10-14 (`hdr_texels`/`hdr_env_dims`/`dist_func`/`dist_cdf`/`dist_dims`) -- ALWAYS bound, even for
    /// a non-HDR dispatch (pass [`HdrEnvGpuData::dummy`]); see that type's own doc comment
    /// for why the megakernel's bind group layout always includes these two.
    pub(crate) hdr_env: &'a HdrEnvGpuData,
}

/// Uploads the scene inputs and binds all eight buffers, WITHOUT dispatching.
///
/// Shared by every self-test's blocking path ([`encode_and_dispatch`]) and wasm32's
/// [`GpuFrameRenderer::accumulate_async`] inlined dispatch. Native's pipelined production
/// path (`GpuFrameRenderer::dispatch_chunk`) does NOT call this -- it would re-upload all
/// five buffers every chunk, which [`FrameSceneBuffers`]/[`build_chunk_bind_group`] avoid;
/// see the module doc comment's "Per-frame uploads, persistent staging" section. The local
/// upload buffers are safely dropped when this function returns even before the GPU has
/// consumed them: `wgpu` keeps a resource alive internally as long as any submitted (but
/// not completed) command buffer references it.
fn build_bind_group(args: &TransportDispatchArgs<'_>) -> wgpu::BindGroup {
    let camera_buf = compute::upload(
        &args.ctx.device,
        "transport camera",
        std::slice::from_ref(args.camera_params),
        BufferUsages::UNIFORM,
    );
    let params_buf = compute::upload(
        &args.ctx.device,
        "transport params",
        std::slice::from_ref(args.params),
        BufferUsages::UNIFORM,
    );
    let material_buf = compute::upload(
        &args.ctx.device,
        "transport material",
        std::slice::from_ref(args.material),
        BufferUsages::STORAGE,
    );
    let planes_buf = compute::upload(
        &args.ctx.device,
        "transport planes",
        args.planes,
        BufferUsages::STORAGE,
    );
    // A SEPARATE storage buffer, parallel to `planes_buf`, never merged into
    // `GpuFacetPlane` itself -- see `renderer::buffers::facet_finish`'s module doc
    // comment for why.
    let facet_finishes_buf = compute::upload(
        &args.ctx.device,
        "transport facet finishes",
        args.facet_finishes,
        BufferUsages::STORAGE,
    );

    compute::bind_buffers(
        &args.ctx.device,
        "transport bind group",
        args.pipeline,
        &[
            (0, &camera_buf),
            (1, &params_buf),
            (2, &material_buf),
            (3, &planes_buf),
            (4, &args.outputs.xyz),
            (5, &args.outputs.radiance),
            (6, &args.outputs.lambdas),
            (7, &args.outputs.path_pdf),
            (8, &facet_finishes_buf),
            (9, &args.outputs.compat),
            (10, &args.hdr_env.texels),
            (11, &args.hdr_env.dims),
            (12, &args.hdr_env.dist_func),
            (13, &args.hdr_env.dist_cdf),
            (14, &args.hdr_env.dist_dims),
        ],
    )
}

/// Uploads the scene inputs, binds all eight buffers, and dispatches `transport_main`
/// over `total_tuples` threads -- then BLOCKS until it finishes.
///
/// The single blocking dispatch routine for the megakernel: `estimator_check::dispatch_transport`
/// and every other self-test in `renderer::gpu` go through here, so the Tier 2/Tier 3
/// equivalence checks verify the exact binding code the renderer ships. `GpuFrameRenderer::dispatch_chunk`'s
/// pipelined production dispatch uses [`build_chunk_bind_group`] instead -- see the module
/// doc comment's "Per-frame uploads, persistent staging" section.
pub(crate) fn encode_and_dispatch(args: &TransportDispatchArgs<'_>, total_tuples: usize) {
    assert!(
        total_tuples <= args.outputs.capacity,
        "dispatch of {total_tuples} tuples exceeds output capacity {}",
        args.outputs.capacity
    );
    let bind_group = build_bind_group(args);
    let workgroups = (total_tuples as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &args.ctx.device,
        &args.ctx.queue,
        args.pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
}

/// The four scene-input buffers (camera, material, facet geometry, facet finishes),
/// PERSISTENT across every [`GpuFrameRenderer::accumulate`]/[`accumulate_cancellable`]
/// call, not merely within one -- see [`Self::ensure`]. Held as
/// [`GpuFrameRenderer::scene_buffers`] and updated in place via `queue.write_buffer`
/// (camera/material every call; `planes`/`facet_finishes` too, UNLESS the new scene needs
/// more capacity than the buffer already has, in which case it is recreated -- grown,
/// never shrunk, mirroring [`GpuFrameRenderer::ensure_capacity`]'s policy for `outputs`).
/// This replaces the four-fresh-buffers-per-call behaviour finding G3 flagged: a desktop
/// viewport calling `accumulate` every ~16ms was recreating all four wgpu buffers that
/// often. None of these four differ between CHUNKS of one frame either -- only
/// [`GpuTransportParams`] does, itself now also a persistent per-slot buffer (see
/// [`GpuFrameRenderer::params_buffers`]) rather than a fresh upload in
/// [`build_chunk_bind_group`]. See the module doc comment's "Per-frame uploads, persistent
/// staging" section.
struct FrameSceneBuffers {
    camera: wgpu::Buffer,
    material: wgpu::Buffer,
    planes: wgpu::Buffer,
    /// `planes`'s allocated capacity, in [`GpuFacetPlane`]s -- may exceed the CURRENT
    /// scene's live plane count when a smaller scene follows a larger one. The live
    /// count is never stored here (it is passed fresh into [`build_chunk_bind_group`]
    /// from `state.scene.planes.len()`), only the buffer's own high-water capacity.
    planes_capacity: usize,
    facet_finishes: wgpu::Buffer,
    facet_finishes_capacity: usize,
    /// Finding G6/G7: bindings 10-14's backing data -- see [`HdrEnvGpuData`]'s doc comment.
    /// Unlike `camera`/`material`/`planes`/`facet_finishes` above, this is REBUILT (never
    /// `queue.write_buffer`d in place) whenever it changes, since a texel buffer's very
    /// SIZE depends on the map's resolution -- there is no fixed-capacity "grow, never
    /// shrink" policy to apply here, just "rebuild on identity change" (see
    /// [`Self::update`]).
    hdr_env: HdrEnvGpuData,
    /// The `EnvironmentMap` [`Self::hdr_env`] was built from, identified by its `&self`
    /// pointer address (a scene's loaded HDR map is not `Clone`/`PartialEq`, and a caller
    /// that keeps rendering the same loaded map passes the same `&EnvironmentMap` every
    /// call -- see `EnvironmentSource::HdrMap`'s doc comment). `None` means "currently
    /// bound to [`HdrEnvGpuData::dummy`]", distinct from any real map's address, which
    /// [`Self::update`] uses to detect "no longer HDR" and rebuild back to the dummy.
    hdr_env_identity: Option<usize>,
}

/// Builds a fresh [`HdrEnvGpuData`] for `environment`: the real upload for `HdrMap`, or
/// [`HdrEnvGpuData::dummy`] for `Studio` -- see that type's own doc comment for why a
/// non-HDR scene still needs something bound at bindings 10-14. Returns the identity
/// [`FrameSceneBuffers::hdr_env_identity`] should record alongside it.
fn build_hdr_env(
    device: &wgpu::Device,
    environment: EnvironmentSource<'_>,
) -> (HdrEnvGpuData, Option<usize>) {
    match environment {
        EnvironmentSource::HdrMap(map) => {
            let identity = std::ptr::from_ref(map) as usize;
            (HdrEnvGpuData::upload(device, map), Some(identity))
        }
        EnvironmentSource::Studio { .. } => (HdrEnvGpuData::dummy(device), None),
    }
}

/// Bundles the per-call scene inputs [`FrameSceneBuffers::new`]/[`FrameSceneBuffers::update`]/
/// [`FrameSceneBuffers::ensure`] need, keeping their argument counts within clippy's
/// `too_many_arguments` limit -- the same reason [`ChunkFrameState`]/[`TransportDispatchArgs`]/
/// [`TurnSetup`] exist in this file.
#[derive(Clone, Copy)]
struct SceneBufferInputs<'a> {
    camera_params: &'a GpuCameraParams,
    material: &'a GpuGemMaterial,
    planes: &'a [GpuFacetPlane],
    facet_finishes: &'a [u32],
    environment: EnvironmentSource<'a>,
}

impl FrameSceneBuffers {
    /// Creates all four persistent scene buffers plus the HDR environment buffers for the
    /// very first time, sized exactly to the live data. `COPY_DST` is added to every
    /// scene-buffer usage (unlike the self-test-only [`build_bind_group`]'s one-shot
    /// uploads) since [`Self::update`] writes into these same buffers on every later call
    /// -- `hdr_env` has no such in-place-write path (see its own field doc comment).
    fn new(device: &wgpu::Device, inputs: &SceneBufferInputs<'_>) -> Self {
        let (hdr_env, hdr_env_identity) = build_hdr_env(device, inputs.environment);
        Self {
            camera: compute::upload(
                device,
                "transport camera (persistent)",
                std::slice::from_ref(inputs.camera_params),
                BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            ),
            material: compute::upload(
                device,
                "transport material (persistent)",
                std::slice::from_ref(inputs.material),
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes: compute::upload(
                device,
                "transport planes (persistent)",
                inputs.planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            planes_capacity: inputs.planes.len(),
            facet_finishes: compute::upload(
                device,
                "transport facet finishes (persistent)",
                inputs.facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            facet_finishes_capacity: inputs.facet_finishes.len(),
            hdr_env,
            hdr_env_identity,
        }
    }

    /// Updates all four persistent scene buffers in place for a new `accumulate` call, in
    /// lieu of recreating them -- see this struct's own doc comment. `hdr_env` is instead
    /// rebuilt from scratch whenever `inputs.environment`'s identity has changed since the
    /// last call (a different `EnvironmentMap`, or a switch to/from `HdrMap` entirely) --
    /// see [`build_hdr_env`].
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: &SceneBufferInputs<'_>,
    ) {
        let SceneBufferInputs {
            camera_params,
            material,
            planes,
            facet_finishes,
            environment,
        } = *inputs;

        // Fixed-size every call -- always a write, never a resize.
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(camera_params));
        queue.write_buffer(&self.material, 0, bytemuck::bytes_of(material));

        if planes.len() > self.planes_capacity {
            self.planes = compute::upload(
                device,
                "transport planes (persistent, grown)",
                planes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.planes_capacity = planes.len();
        } else {
            queue.write_buffer(&self.planes, 0, bytemuck::cast_slice(planes));
        }

        if facet_finishes.len() > self.facet_finishes_capacity {
            self.facet_finishes = compute::upload(
                device,
                "transport facet finishes (persistent, grown)",
                facet_finishes,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            );
            self.facet_finishes_capacity = facet_finishes.len();
        } else {
            queue.write_buffer(
                &self.facet_finishes,
                0,
                bytemuck::cast_slice(facet_finishes),
            );
        }

        let new_identity = match environment {
            EnvironmentSource::HdrMap(map) => Some(std::ptr::from_ref(map) as usize),
            EnvironmentSource::Studio { .. } => None,
        };
        if new_identity != self.hdr_env_identity {
            let (hdr_env, hdr_env_identity) = build_hdr_env(device, environment);
            self.hdr_env = hdr_env;
            self.hdr_env_identity = hdr_env_identity;
        }
    }

    /// Create-or-update entry point every caller in this module uses: `existing` comes
    /// from `self.scene_buffers.take()`, `None` only before this renderer's very first
    /// `accumulate` call. Taking `self.scene_buffers` out and returning an OWNED `Self`
    /// (rather than binding a `&FrameSceneBuffers` borrowed from `self`) deliberately
    /// keeps the returned value independent of `self` for the rest of the caller's
    /// dispatch loop, which still needs `self.dispatch_chunk`/`self.drain_pending_chunk`
    /// (`&self`/`&mut self`) alongside it -- the caller is responsible for putting it
    /// back via `self.scene_buffers = Some(..)` once done.
    fn ensure(existing: Option<Self>, ctx: &GpuContext, inputs: &SceneBufferInputs<'_>) -> Self {
        existing.map_or_else(
            || Self::new(&ctx.device, inputs),
            |mut existing| {
                existing.update(&ctx.device, &ctx.queue, inputs);
                existing
            },
        )
    }
}

/// Like [`build_bind_group`], but for [`GpuFrameRenderer::dispatch_chunk`]'s pipelined
/// per-chunk dispatch: `frame_buffers`' four scene buffers and `params_buf` are all
/// PERSISTENT (see [`FrameSceneBuffers`] and [`GpuFrameRenderer::params_buffers`]) and
/// already written by the caller -- this function only builds the bind group, uploading
/// nothing itself. `planes_len` is `state.scene.planes.len()` for the CURRENT scene, which
/// can be smaller than `frame_buffers.planes`'s allocated capacity (grown-never-shrunk
/// across frames), so `planes`/`facet_finishes` bind an explicit byte range via
/// [`compute::bind_buffers_sized`] rather than [`compute::bind_buffers`]'s
/// whole-buffer `as_entire_binding` -- otherwise the shader's `arrayLength(&planes)`
/// would see the buffer's stale larger capacity instead of this frame's true plane count.
/// See the module doc comment's "Per-frame uploads, persistent staging" section.
fn build_chunk_bind_group(
    ctx: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    compute::bind_buffers_sized(
        &ctx.device,
        "transport bind group (pipelined production)",
        pipeline,
        &[
            (0, &frame_buffers.camera, None),
            (1, params_buf, None),
            (2, &frame_buffers.material, None),
            (3, &frame_buffers.planes, Some(planes_bytes)),
            (4, outputs.xyz(), None),
            (5, outputs.radiance(), None),
            (6, outputs.lambdas(), None),
            (7, outputs.path_pdf(), None),
            (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
            (9, outputs.compat(), None),
            (10, &frame_buffers.hdr_env.texels, None),
            (11, &frame_buffers.hdr_env.dims, None),
            (12, &frame_buffers.hdr_env.dist_func, None),
            (13, &frame_buffers.hdr_env.dist_cdf, None),
            (14, &frame_buffers.hdr_env.dist_dims, None),
        ],
    )
}

/// One persistent staging buffer backing one of [`GpuFrameRenderer::staging`]'s two
/// double-buffered slots -- reused across every chunk of every frame, and reallocated
/// only when a chunk needs more capacity than it currently has (see
/// [`GpuFrameRenderer::ensure_staging_capacity`]), mirroring how [`TransportOutputs`] is
/// grown-never-shrunk across frames rather than reallocated per chunk.
struct StagingSlot {
    buffer: wgpu::Buffer,
    /// Capacity in PIXELS, not (pixel, sample) tuples -- `capacity * FLOATS_PER_TUPLE` is
    /// the buffer's actual float count. Since [`GpuFrameRenderer::dispatch_chunk`]'s
    /// readback copy now reads from [`PixelXyzOutput`] (already GPU-reduced to one XYZ
    /// triple per pixel, see finding G2) rather than [`TransportOutputs::xyz`] (one
    /// triple per (pixel, sample) tuple), this shrank from a `spp`-scaled quantity down
    /// to pixel-scale -- matching [`PixelXyzOutput::capacity`]'s own convention, no
    /// longer [`TransportOutputs::capacity`]'s.
    capacity: usize,
}

impl StagingSlot {
    fn new(device: &wgpu::Device, label: &str, capacity: usize) -> Self {
        let byte_len = (capacity * FLOATS_PER_TUPLE * size_of::<f32>()) as wgpu::BufferAddress;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: byte_len,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { buffer, capacity }
    }
}

/// Whether a persistent [`StagingSlot`] currently sized for `current_capacity` tuples
/// must be reallocated to serve a chunk needing `required` tuples.
///
/// A pure predicate (no GPU access) so the "grow, never shrink" growth policy is directly
/// unit-testable. A slot already big enough (even strictly larger than `required`) is
/// deliberately left alone, matching [`GpuFrameRenderer::ensure_capacity`]'s identical
/// policy for `outputs`.
const fn staging_needs_growth(current_capacity: usize, required: usize) -> bool {
    current_capacity < required
}

/// Like [`compute::copy_to_staging`], but copies into an ALREADY-ALLOCATED `staging`
/// buffer instead of creating a fresh one every call.
///
/// [`GpuFrameRenderer::dispatch_chunk`] reuses one of its two persistent [`StagingSlot`]s
/// across every chunk of a frame rather than allocating a fresh buffer per chunk.
/// `staging` must already be sized for at least `count` `T`s -- guaranteed by
/// [`GpuFrameRenderer::ensure_staging_capacity`], called once per `accumulate` call before
/// any chunk dispatches -- and must carry `COPY_DST | MAP_READ`.
fn copy_to_existing_staging<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    staging: &wgpu::Buffer,
    count: usize,
) -> wgpu::SubmissionIndex {
    let byte_len = (count * size_of::<T>()) as wgpu::BufferAddress;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("indicatrix gpu pipelined readback copy encoder (persistent staging)"),
    });
    encoder.copy_buffer_to_buffer(source, 0, staging, 0, byte_len);
    queue.submit(Some(encoder.finish()))
}

/// The values `spectral_transport.wgsl`'s `MATERIAL_CLASS` pipeline-overridable
/// constant accepts -- see that override's own doc comment. `pub(crate)` rather than
/// private: `estimator_check::dispatch_transport_for_class` (Tier 3 statistical image
/// comparisons) also needs these to dispatch through the same specialised pipelines
/// [`GpuFrameRenderer::accumulate`] does -- see the module doc comment's "Material-class
/// kernel specialisation" section.
pub(crate) mod material_class {
    /// Every class, runtime-dispatched inside the kernel -- the override's own declared
    /// default, so a dispatch that never sets it (every self-test) is unaffected.
    pub const GENERIC: u32 = 0;
    pub const ISOTROPIC: u32 = 1;
    pub const UNIAXIAL: u32 = 2;
    pub const BIAXIAL: u32 = 3;
}

/// Which of [`material_class`]'s values a real render of `material` should dispatch
/// through.
///
/// MIRRORS `renderer::buffers::GpuGemMaterial::encode`'s own `is_anisotropic`/
/// `has_biaxial_delta` derivation exactly -- never a second, independently-maintained
/// definition: biaxial takes priority (`biaxial_delta_beta_alpha.is_some()`), then
/// uniaxial (`crystal_system != Cubic && |birefringence_delta| > 1e-4`, matching the
/// kernel's own `is_anisotropic`), else isotropic. If `encode`'s formula ever changes,
/// this must change with it, or [`GpuFrameRenderer::accumulate`] would pick a specialised
/// pipeline that forces off state the material's own encoded flags say it needs --
/// silently wrong output, not a crash. `estimator_check::run_specialisation_image_comparison`
/// (statistical, not bit-exact) is the check that would catch such a drift.
#[must_use]
pub(crate) fn classify_material(material: &GemMaterial) -> u32 {
    if material.biaxial_delta_beta_alpha.is_some() {
        material_class::BIAXIAL
    } else if material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        material_class::UNIAXIAL
    } else {
        material_class::ISOTROPIC
    }
}

/// A GPU-backed renderer for arbitrary scenes, owning its device and pipeline across
/// frames.
///
/// Construct once and keep it: [`GpuFrameRenderer::new`] acquires an adapter and
/// compiles the megakernel, both of which take long enough to be worth doing off the
/// frame loop.
pub struct GpuFrameRenderer {
    ctx: GpuContext,
    /// The GENERIC (`MATERIAL_CLASS = 0`) pipeline -- built eagerly since every self-test
    /// in `renderer::gpu` dispatches it, and the equivalence checks need it available
    /// without any prior `accumulate` call. See the module doc comment's "Material-class
    /// kernel specialisation" section.
    pipeline: wgpu::ComputePipeline,
    /// The three per-class specialised pipelines, built LAZILY (see
    /// [`Self::ensure_specialized_pipeline`]) on first use of that class -- `None` until a
    /// scene of that class is dispatched, so a session that only renders one class never
    /// pays the others' shader-compile cost.
    pipeline_isotropic: Option<wgpu::ComputePipeline>,
    pipeline_uniaxial: Option<wgpu::ComputePipeline>,
    pipeline_biaxial: Option<wgpu::ComputePipeline>,
    /// `reduce_xyz_main`'s pipeline (see `reduce_xyz.wgsl` and finding G2) -- built
    /// eagerly in [`Self::new`]/[`Self::new_async`] alongside the GENERIC transport
    /// pipeline, since every production dispatch (`dispatch_chunk` and wasm32's
    /// `accumulate_async`) uses it unconditionally.
    reduce_pipeline: wgpu::ComputePipeline,
    adapter_label: String,
    /// TWO chunk-output buffer sets, alternated by chunk index so one chunk's dispatch
    /// can be queued into the other slot while the previous chunk's readback is still in
    /// flight -- see the module doc comment's "Overlapped chunk pipeline" section.
    outputs: [Option<TransportOutputs>; 2],
    /// TWO double-buffered [`PixelXyzOutput`]s, alternated in lockstep with `outputs` --
    /// `reduce_xyz_main`'s destination, and what [`Self::dispatch_chunk`]'s readback copy
    /// reads from instead of `outputs[..].xyz()` directly. See finding G2.
    pixel_outputs: [Option<PixelXyzOutput>; 2],
    /// TWO persistent staging buffers, alternated by chunk index in lockstep with
    /// `outputs` -- reused across every chunk of every frame and grown (never shrunk)
    /// only when [`Self::ensure_staging_capacity`] finds one too small. See the module
    /// doc comment's "Per-frame uploads, persistent staging" section.
    staging: [Option<StagingSlot>; 2],
    /// TWO persistent per-output-slot [`GpuTransportParams`] uniform buffers, written via
    /// `queue.write_buffer` immediately before that slot's dispatch rather than recreated
    /// every chunk (see [`Self::dispatch_chunk`] and finding G3). Safe to overwrite a
    /// slot's buffer two chunks later because `wgpu` executes queue operations in
    /// submission order: the write is submitted strictly after the earlier chunk that
    /// read the same slot, matching `outputs`/`staging`'s own double-buffering depth.
    params_buffers: [Option<wgpu::Buffer>; 2],
    /// TWO persistent per-output-slot [`GpuReduceParams`] uniform buffers for
    /// `reduce_xyz_main`, mirroring `params_buffers`' same write-before-dispatch,
    /// two-deep-safe reuse policy.
    reduce_params_buffers: [Option<wgpu::Buffer>; 2],
    /// The four scene-input buffers, persistent across every `accumulate` call (not just
    /// within one) -- see [`FrameSceneBuffers::ensure`]. `None` only before this
    /// renderer's very first `accumulate` call.
    scene_buffers: Option<FrameSceneBuffers>,
    /// Reusable scratch buffer for [`Self::drain_pending_chunk`]'s per-chunk GPU-reduced
    /// XYZ readback -- avoids a fresh `Vec` allocation every chunk (see
    /// [`compute::finish_map_read_into`]). Safe to reuse across chunks: the overlapped
    /// pipeline's pending queue is exactly one chunk deep (see the module doc comment's
    /// "Overlapped chunk pipeline" section), so at most one `drain_pending_chunk` call is
    /// ever using it at a time.
    xyz_scratch: Vec<f32>,
    chunk_budget_bytes: usize,
    /// Running exponential moving average of nanoseconds-per-(pixel,sample)-tuple,
    /// measured from dispatch submission to readback completion in
    /// [`Self::drain_pending_chunk`] -- `None` until the first chunk has drained.
    /// [`Self::next_chunk_pixels`] sizes every later chunk toward [`TARGET_CHUNK_MS`]
    /// from this estimate.
    ns_per_tuple_ema: Option<f64>,
    /// Finding G5 Part B: which kernel [`Self::dispatch_chunk`] dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. `Default::default()` (`Megakernel`)
    /// until [`Self::set_pipeline_kind`] is called.
    pipeline_kind: GpuPipelineKind,
    /// The five wavefront-pipeline pipelines, built LAZILY on first use of
    /// [`GpuPipelineKind::Wavefront`] -- mirrors [`Self::pipeline_isotropic`]/etc.'s own
    /// "never pay a shader-compile cost a session doesn't use" reasoning, at the whole
    /// pipeline granularity rather than per material class.
    wavefront_pipelines: Option<WavefrontPipelines>,
}

/// Finding G5 Part B: the five `wavefront_transport.wgsl` entry points -- see that
/// file's own header comment for the kernel sequence and
/// [`GpuFrameRenderer::dispatch_chunk_wavefront`] for how they're driven.
struct WavefrontPipelines {
    generate: wgpu::ComputePipeline,
    bounce: wgpu::ComputePipeline,
    compact_scan: wgpu::ComputePipeline,
    compact_scatter: wgpu::ComputePipeline,
    finalize_survivors: wgpu::ComputePipeline,
}

impl WavefrontPipelines {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            generate: compute::create_compute_pipeline(
                device,
                "wavefront_generate",
                WAVEFRONT_SHADER_SRC,
                "wavefront_generate",
            ),
            bounce: compute::create_compute_pipeline(
                device,
                "wavefront_bounce",
                WAVEFRONT_SHADER_SRC,
                "wavefront_bounce",
            ),
            compact_scan: compute::create_compute_pipeline(
                device,
                "wavefront_compact_scan",
                WAVEFRONT_SHADER_SRC,
                "wavefront_compact_scan",
            ),
            compact_scatter: compute::create_compute_pipeline(
                device,
                "wavefront_compact_scatter",
                WAVEFRONT_SHADER_SRC,
                "wavefront_compact_scatter",
            ),
            finalize_survivors: compute::create_compute_pipeline(
                device,
                "wavefront_finalize_survivors",
                WAVEFRONT_SHADER_SRC,
                "wavefront_finalize_survivors",
            ),
        }
    }
}

/// Finding G5 Part B: one chunk's wavefront ray-state struct-of-arrays buffers -- mirrors
/// `wavefront_transport.wgsl`'s bindings 16-33 exactly (binding 15,
/// [`GpuWavefrontParams`], is uploaded separately since it's rewritten every bounce
/// round, unlike these, which are allocated once per chunk -- see
/// [`GpuFrameRenderer::dispatch_chunk_wavefront`]). Every per-ray array is flat, sized
/// to `tuples` rays (an 8-channel field to `tuples * 8`); `block_alive_count`/
/// `block_offset` are sized to `tuples.div_ceil(64)` workgroups. See
/// `wavefront_transport.wgsl`'s own module doc comment for what each binding holds.
struct WavefrontRayBuffers {
    origin: wgpu::Buffer,
    dir: wgpu::Buffer,
    k: wgpu::Buffer,
    prev_plane_normal: wgpu::Buffer,
    flags: wgpu::Buffer,
    stokes: wgpu::Buffer,
    radiance: wgpu::Buffer,
    path_pdf: wgpu::Buffer,
    split_radiance: wgpu::Buffer,
    compat: wgpu::Buffer,
    pending_light_mis: wgpu::Buffer,
    lambdas: wgpu::Buffer,
    seed: wgpu::Buffer,
    /// `active_ray_indices` (binding 29): the CURRENT bounce round's live ray indices.
    active_in: wgpu::Buffer,
    /// `active_ray_indices_next` (binding 30): `wavefront_compact_scatter`'s
    /// destination, copied back into `active_in` before the next round -- see
    /// `dispatch_chunk_wavefront`'s own comment on why a copy, not a bind-group swap.
    active_out: wgpu::Buffer,
    /// `compact_local_offset` (binding 31).
    local_offset: wgpu::Buffer,
    /// `compact_block_alive_count` (binding 32) -- read back to the host every round.
    block_alive_count: wgpu::Buffer,
    /// `compact_block_offset` (binding 33) -- written by the host every round, from the
    /// CPU-side exclusive prefix sum of `block_alive_count`'s readback.
    block_offset: wgpu::Buffer,
}

impl WavefrontRayBuffers {
    fn new(device: &wgpu::Device, tuples: usize) -> Self {
        let storage_rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        // `block_alive_count` additionally needs `COPY_SRC`: the host reads it back
        // every bounce round (see `dispatch_chunk_wavefront`).
        let storage_rw_src = storage_rw | wgpu::BufferUsages::COPY_SRC;

        let workgroups = tuples.div_ceil(WORKGROUP_SIZE);
        Self {
            origin: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_origin", tuples, storage_rw),
            dir: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_dir", tuples, storage_rw),
            k: compute::zeroed_buffer::<[f32; 4]>(device, "wf ray_k", tuples, storage_rw),
            prev_plane_normal: compute::zeroed_buffer::<[f32; 4]>(
                device,
                "wf ray_prev_plane_normal",
                tuples,
                storage_rw,
            ),
            flags: compute::zeroed_buffer::<u32>(device, "wf ray_flags", tuples, storage_rw),
            stokes: compute::zeroed_buffer::<[f32; 4]>(
                device,
                "wf ray_stokes",
                tuples * 8,
                storage_rw,
            ),
            radiance: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_radiance",
                tuples * 8,
                storage_rw,
            ),
            path_pdf: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_path_pdf",
                tuples * 8,
                storage_rw,
            ),
            split_radiance: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_split_radiance",
                tuples * 8,
                storage_rw,
            ),
            compat: compute::zeroed_buffer::<u32>(device, "wf ray_compat", tuples * 8, storage_rw),
            pending_light_mis: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_pending_light_mis",
                tuples,
                storage_rw,
            ),
            lambdas: compute::zeroed_buffer::<f32>(
                device,
                "wf ray_lambdas",
                tuples * 8,
                storage_rw,
            ),
            seed: compute::zeroed_buffer::<u32>(device, "wf ray_seed", tuples, storage_rw),
            active_in: compute::zeroed_buffer::<u32>(
                device,
                "wf active_ray_indices",
                tuples,
                storage_rw,
            ),
            active_out: compute::zeroed_buffer::<u32>(
                device,
                "wf active_ray_indices_next",
                tuples,
                storage_rw,
            ),
            local_offset: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_local_offset",
                tuples,
                storage_rw,
            ),
            block_alive_count: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_block_alive_count",
                workgroups.max(1),
                storage_rw_src,
            ),
            block_offset: compute::zeroed_buffer::<u32>(
                device,
                "wf compact_block_offset",
                workgroups.max(1),
                storage_rw,
            ),
        }
    }
}

/// Deterministic exclusive prefix sum over `counts` (one entry per compaction
/// workgroup), plus the total. A trivial sequential CPU loop -- see
/// `wavefront_compact_scan`'s own doc comment (`wavefront_transport.wgsl`) for why this
/// stays on the host rather than a GPU-side atomic counter.
fn exclusive_prefix_sum(counts: &[u32]) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(counts.len());
    let mut running = 0u32;
    for &c in counts {
        offsets.push(running);
        running += c;
    }
    (offsets, running)
}

/// `wavefront_generate`'s binding list -- see `wavefront_transport.wgsl`'s own module
/// doc comment for why this entry point reaches `camera`/`params` (binding 0/1, camera
/// ray generation) but none of the other shared scene bindings.
const fn wavefront_generate_bindings<'a>(
    camera: &'a wgpu::Buffer,
    params_buf: &'a wgpu::Buffer,
    wf_params_buf: &'a wgpu::Buffer,
    rays: &'a WavefrontRayBuffers,
) -> [(u32, &'a wgpu::Buffer); 17] {
    [
        (0, camera),
        (1, params_buf),
        (15, wf_params_buf),
        (16, &rays.origin),
        (17, &rays.dir),
        (18, &rays.k),
        (19, &rays.prev_plane_normal),
        (20, &rays.flags),
        (21, &rays.stokes),
        (22, &rays.radiance),
        (23, &rays.path_pdf),
        (24, &rays.split_radiance),
        (25, &rays.compat),
        (26, &rays.pending_light_mis),
        (27, &rays.lambdas),
        (28, &rays.seed),
        (29, &rays.active_in),
    ]
}

/// The four bind groups every bounce round (plus the final survivors pass) dispatches
/// through, built ONCE per chunk since the buffers they reference never change across
/// rounds -- see [`GpuFrameRenderer::run_wavefront_bounce_rounds`]'s own comment on why
/// `active_ray_indices`/`active_ray_indices_next` are copied between rounds instead of
/// the bind groups being rebuilt.
struct WavefrontRoundBindGroups {
    bounce: wgpu::BindGroup,
    compact_scan: wgpu::BindGroup,
    compact_scatter: wgpu::BindGroup,
    finalize: wgpu::BindGroup,
}

/// Builds [`WavefrontRoundBindGroups`] -- split out of
/// [`GpuFrameRenderer::dispatch_chunk_wavefront`] purely to keep that function under
/// clippy's function-length limit (mirrors [`build_chunk_bind_group`]'s own reason for
/// being a free function rather than inlined into its one call site), and split again
/// into one function per bind group (below) for the same reason.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per distinct buffer source this chunk's four bind groups draw \
              from (device, pipelines, scene buffers, params, geometry length, transport \
              outputs, wavefront params, ray state) -- bundling them into a context struct \
              here would just re-introduce this exact parameter list one level removed, the \
              same reasoning ChunkFrameState/TransportDispatchArgs/TurnSetup already \
              document elsewhere in this file"
)]
fn build_wavefront_round_bind_groups(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> WavefrontRoundBindGroups {
    WavefrontRoundBindGroups {
        bounce: wavefront_bounce_bind_group(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            wf_params_buf,
            rays,
        ),
        compact_scan: wavefront_compact_scan_bind_group(device, pipelines, wf_params_buf, rays),
        compact_scatter: wavefront_compact_scatter_bind_group(
            device,
            pipelines,
            wf_params_buf,
            rays,
        ),
        finalize: wavefront_finalize_bind_group(
            device,
            pipelines,
            params_buf,
            outputs,
            wf_params_buf,
            rays,
        ),
    }
}

/// `wavefront_bounce`'s bind group -- everything `transport_bounce_step`/
/// `transport_finalize_ray` can reach EXCEPT `camera` (never touched once a ray
/// exists) plus this pipeline's own per-round state.
#[expect(
    clippy::too_many_arguments,
    reason = "see build_wavefront_round_bind_groups' own #[expect] just above"
)]
fn wavefront_bounce_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    frame_buffers: &FrameSceneBuffers,
    params_buf: &wgpu::Buffer,
    planes_len: usize,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    let planes_bytes = (planes_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (planes_len * size_of::<u32>()) as wgpu::BufferAddress;
    compute::bind_buffers_sized(
        device,
        "wavefront bounce bind group",
        &pipelines.bounce,
        &[
            (1, params_buf, None),
            (2, &frame_buffers.material, None),
            (3, &frame_buffers.planes, Some(planes_bytes)),
            (4, outputs.xyz(), None),
            (5, outputs.radiance(), None),
            (6, outputs.lambdas(), None),
            (7, outputs.path_pdf(), None),
            (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
            (9, outputs.compat(), None),
            (10, &frame_buffers.hdr_env.texels, None),
            (11, &frame_buffers.hdr_env.dims, None),
            (12, &frame_buffers.hdr_env.dist_func, None),
            (13, &frame_buffers.hdr_env.dist_cdf, None),
            (14, &frame_buffers.hdr_env.dist_dims, None),
            (15, wf_params_buf, None),
            (16, &rays.origin, None),
            (17, &rays.dir, None),
            (18, &rays.k, None),
            (19, &rays.prev_plane_normal, None),
            (20, &rays.flags, None),
            (21, &rays.stokes, None),
            (22, &rays.radiance, None),
            (23, &rays.path_pdf, None),
            (24, &rays.split_radiance, None),
            (25, &rays.compat, None),
            (26, &rays.pending_light_mis, None),
            (27, &rays.lambdas, None),
            (28, &rays.seed, None),
            (29, &rays.active_in, None),
        ],
    )
}

/// `wavefront_compact_scan`'s bind group -- see `wavefront_transport.wgsl`'s own doc
/// comment for why it touches only `ray_flags`/`active_ray_indices` plus its own
/// per-workgroup scratch, none of the scene bindings.
fn wavefront_compact_scan_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scan bind group",
        &pipelines.compact_scan,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (31, &rays.local_offset),
            (32, &rays.block_alive_count),
        ],
    )
}

/// `wavefront_compact_scatter`'s bind group -- see `wavefront_compact_scan_bind_group`'s
/// own doc comment.
fn wavefront_compact_scatter_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront compact scatter bind group",
        &pipelines.compact_scatter,
        &[
            (15, wf_params_buf),
            (20, &rays.flags),
            (29, &rays.active_in),
            (30, &rays.active_out),
            (31, &rays.local_offset),
            (33, &rays.block_offset),
        ],
    )
}

/// `wavefront_finalize_survivors`'s bind group -- reaches `params`/the four output
/// buffers (via `transport_finalize_ray`) plus its own per-ray scratch, but none of
/// `camera`/`material`/`planes`/`facet_finishes`/HDR environment (a survivor's own
/// physics already ran inside `wavefront_bounce`; finalizing just integrates and
/// writes out what it already accumulated).
fn wavefront_finalize_bind_group(
    device: &wgpu::Device,
    pipelines: &WavefrontPipelines,
    params_buf: &wgpu::Buffer,
    outputs: &TransportOutputs,
    wf_params_buf: &wgpu::Buffer,
    rays: &WavefrontRayBuffers,
) -> wgpu::BindGroup {
    compute::bind_buffers(
        device,
        "wavefront finalize survivors bind group",
        &pipelines.finalize_survivors,
        &[
            (1, params_buf),
            (4, outputs.xyz()),
            (5, outputs.radiance()),
            (6, outputs.lambdas()),
            (7, outputs.path_pdf()),
            (9, outputs.compat()),
            (15, wf_params_buf),
            (20, &rays.flags),
            (22, &rays.radiance),
            (23, &rays.path_pdf),
            (24, &rays.split_radiance),
            (25, &rays.compat),
            (27, &rays.lambdas),
            (29, &rays.active_in),
        ],
    )
}

/// One in-flight chunk's readback state, between "dispatch and copy submitted" and
/// "mapped, read, and summed into `accum`" -- see [`GpuFrameRenderer::accumulate`]'s
/// pipeline.
struct PendingChunk {
    /// Index into [`GpuFrameRenderer::staging`] holding this chunk's persistent staging
    /// buffer -- always `chunk_index % 2`, matching `outputs`. The buffer itself lives in
    /// `self.staging` for [`GpuFrameRenderer::drain_pending_chunk`] to look up, so it
    /// survives being reused two chunks later.
    staging_slot: usize,
    copy_index: wgpu::SubmissionIndex,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    first_pixel: usize,
    pixels_this_chunk: usize,
    /// Wall-clock time this chunk's dispatch was submitted -- the per-tuple timing
    /// measurement is `Instant::now()` at [`Self::drain_pending_chunk`] minus this,
    /// covering submit-to-map-completion.
    submitted_at: Instant,
}

/// The per-frame values [`GpuFrameRenderer::dispatch_chunk`] needs but that never change
/// between chunks of the SAME [`GpuFrameRenderer::accumulate_via_pipeline`] call --
/// computed once and threaded through by reference, purely so both functions stay under
/// clippy's argument-count and function-length limits.
///
/// `gpu_material`/`gpu_finishes` are uploaded ONCE into a [`FrameSceneBuffers`] alongside
/// this state instead of living here, since their bytes never change between chunks
/// either -- see [`GpuFrameRenderer::accumulate_via_pipeline`].
struct ChunkFrameState<'a> {
    scene: &'a GpuFrameScene<'a>,
    pipeline_class: u32,
    sample_offset: u32,
    spp: u32,
    env_mode: u32,
    temp_k: f32,
    spot_mult: f32,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    white_balance: Vec3,
    /// See [`environment_params`]'s own doc comment.
    use_d65: bool,
    studio_model: u32,
    backdrop: f32,
}

/// Bundles [`GpuFrameRenderer::accumulate_turn`]'s per-REQUEST inputs -- the values that
/// stay fixed across however many turns it takes to resume one request, as opposed to
/// `accum`/`cursor`, which a caller mutates turn by turn. Exists purely to bring
/// `accumulate_turn`'s argument count within clippy's `too_many_arguments` limit, the
/// same reason [`ChunkFrameState`]/[`TransportDispatchArgs`] exist.
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

/// The GPU-side preparation [`GpuFrameRenderer::accumulate_turn`] must redo on EVERY
/// turn -- pipeline selection, buffer-capacity growth, and refreshing the four
/// persistent [`FrameSceneBuffers`] -- before it can dispatch a single chunk. Bundles
/// [`GpuFrameRenderer::prepare_turn`]'s three outputs purely to keep `accumulate_turn`
/// under clippy's function-length limit; see that function's own doc comment for why
/// this setup cannot be skipped on a resumed turn.
struct TurnSetup<'a> {
    state: ChunkFrameState<'a>,
    frame_buffers: FrameSceneBuffers,
    /// [`chunk_pixels_for`]'s hard ceiling for this request, computed once here rather
    /// than once per chunk since it depends only on values fixed for the whole request.
    byte_budget_pixels: usize,
}

/// Builds one chunk's `GpuTransportParams`, split out of
/// [`GpuFrameRenderer::dispatch_chunk`] purely to keep that method under clippy's
/// function-length limit. `params` is written into this chunk's slot's persistent
/// uniform buffer by the caller instead of being uploaded fresh here -- see finding G3.
const fn build_chunk_params(
    state: &ChunkFrameState<'_>,
    first_pixel: usize,
    pixels_this_chunk: usize,
) -> GpuTransportParams {
    GpuTransportParams::new(
        pixels_this_chunk as u32,
        state.scene.max_bounces,
        state.sample_offset,
        state.env_mode,
        0.0,
        state.temp_k,
        state.spot_mult,
        state.exposure,
        state.light_yaw,
        state.light_pitch,
        state.white_balance.to_array(),
    )
    .with_pixel_offset(first_pixel as u32)
    .with_debug_buffers_disabled()
    .with_studio_use_d65(state.use_d65)
    .with_studio_model(state.studio_model)
    .with_backdrop(state.backdrop)
}

/// The four per-slot GPU resources [`GpuFrameRenderer::dispatch_chunk`] reads for one
/// chunk, bundled purely to keep that method under clippy's function-length limit.
struct ChunkSlotResources<'a> {
    outputs: &'a TransportOutputs,
    pixel_output: &'a PixelXyzOutput,
    staging_buffer: &'a wgpu::Buffer,
    params_buf: &'a wgpu::Buffer,
}

impl GpuFrameRenderer {
    /// Acquires a GPU device and compiles `transport_main`.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::Acquire`] if this machine has no usable adapter or device --
    /// an expected outcome, not a bug; the caller should fall back to the CPU tracer.
    pub fn new() -> Result<Self, GpuFrameError> {
        let ctx = GpuContext::acquire().map_err(GpuFrameError::Acquire)?;
        let available = ctx.device.limits().max_storage_buffers_per_shader_stage;
        if available < MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS,
                available,
            });
        }
        let info = ctx.adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "transport_main",
            SHADER_SRC,
            "transport_main",
        );
        let reduce_pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
        );
        Ok(Self {
            ctx,
            pipeline,
            pipeline_isotropic: None,
            pipeline_uniaxial: None,
            pipeline_biaxial: None,
            reduce_pipeline,
            adapter_label,
            outputs: [None, None],
            pixel_outputs: [None, None],
            staging: [None, None],
            params_buffers: [None, None],
            reduce_params_buffers: [None, None],
            scene_buffers: None,
            xyz_scratch: Vec::new(),
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
            ns_per_tuple_ema: None,
            pipeline_kind: GpuPipelineKind::default(),
            wavefront_pipelines: None,
        })
    }

    /// Overrides the per-dispatch output-buffer budget (default [`CHUNK_BUDGET_BYTES`]).
    ///
    /// Exists so a check can force a frame through many small chunks and confirm the
    /// result is identical to the single-chunk one -- see [`run_chunk_equivalence`].
    /// Lowering it trades more dispatches for less peak VRAM; it never changes what a
    /// frame renders.
    pub const fn set_chunk_budget_bytes(&mut self, bytes: usize) {
        self.chunk_budget_bytes = bytes;
    }

    /// Finding G5 Part B: selects which kernel every LATER chunk dispatches through --
    /// see [`GpuPipelineKind`]'s own doc comment. Takes effect from the next
    /// `dispatch_chunk` call onward; a chunk already dispatched (or in flight in the
    /// overlapped pipeline) is unaffected. Existing signatures
    /// ([`Self::accumulate`]/[`Self::accumulate_cancellable`]/etc.) are unchanged --
    /// this is the caller's opt-in, not a new required argument.
    ///
    /// Eagerly compiles the five wavefront pipelines on first switch to
    /// [`GpuPipelineKind::Wavefront`] (a no-op on a later call, or a switch back to
    /// `Megakernel`): `dispatch_chunk`/`dispatch_chunk_wavefront` take `&self`, not
    /// `&mut self` (required by the overlapped-chunk-pipeline borrow pattern
    /// `accumulate_turn` uses), so lazy build-on-first-dispatch the way
    /// [`Self::ensure_specialized_pipeline`] does for material classes isn't available
    /// here -- this setter is the one `&mut self` call site available to do it instead.
    pub fn set_pipeline_kind(&mut self, kind: GpuPipelineKind) {
        let available = self
            .ctx
            .device
            .limits()
            .max_storage_buffers_per_shader_stage;
        if kind == GpuPipelineKind::Wavefront && available < WAVEFRONT_STORAGE_BUFFERS {
            tracing::warn!(
                available,
                needed = WAVEFRONT_STORAGE_BUFFERS,
                "device cannot bind enough storage buffers for the wavefront pipeline;                  staying on the megakernel"
            );
            self.pipeline_kind = GpuPipelineKind::Megakernel;
            return;
        }
        self.pipeline_kind = kind;
        if kind == GpuPipelineKind::Wavefront && self.wavefront_pipelines.is_none() {
            self.wavefront_pipelines = Some(WavefrontPipelines::new(&self.ctx.device));
        }
    }

    /// Which kernel [`Self::dispatch_chunk`] currently dispatches through -- see
    /// [`Self::set_pipeline_kind`].
    #[must_use]
    pub const fn pipeline_kind(&self) -> GpuPipelineKind {
        self.pipeline_kind
    }

    /// Human-readable adapter name and backend, for logging and for telling the user
    /// which device is actually rendering.
    #[must_use]
    pub fn adapter_label(&self) -> &str {
        &self.adapter_label
    }

    /// Traces `spp` samples per pixel, starting at sample index `sample_offset`, and
    /// ADDS each pixel's summed XYZ into `accum`.
    ///
    /// Accumulating rather than overwriting mirrors the CPU path's
    /// `acc_chunk[i] += sample_sum`, so a caller's progressive-accumulation buffer and its
    /// sample counter mean the same thing regardless of backend. `sample_offset` must be
    /// the number of samples already in `accum` for these pixels: the shader derives each
    /// thread's seed and stratified jitter from the absolute sample index, so reusing an
    /// offset would re-draw identical samples and bias the average.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::UnsupportedEnvironment`] if the scene uses an environment the
    /// megakernel has no `env_mode` for -- currently unreachable (see that variant's own
    /// doc comment; HDR maps render on the GPU since finding G6).
    /// [`GpuFrameError::DeviceLost`] if the device stops making forward progress
    /// mid-frame -- see that variant's own doc comment; `renderer::gpu_backend::GpuBackend`
    /// is the caller expected to react to it by permanently disabling the GPU for the rest
    /// of the process.
    ///
    /// # Panics
    ///
    /// Panics if `accum.len()` is not `width * height`.
    pub fn accumulate(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        // classify_material is the ONE place this decision is made -- see the module doc's
        // "Material-class kernel specialisation" section. accumulate_via_pipeline is
        // parameterised on which pipeline to dispatch through, so
        // run_specialisation_equivalence can force a class directly and compare against
        // what this wrapper picks. cancel: None -- this entry point never stops early.
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(scene, pipeline_class, sample_offset, spp, accum, None)?;
        Ok(())
    }

    /// Like [`Self::accumulate`], but checked for cancellation between chunks.
    ///
    /// `cancel` is polled once per loop iteration, between one chunk's dispatch and the
    /// next's (never mid-chunk) -- see [`AccumulateOutcome::Cancelled`]'s doc comment for
    /// the drain-then-discard guarantee this makes about `accum` once it fires. Intended
    /// caller: `renderer::gpu_backend::GpuBackend::try_accumulate_cancellable`, for a long
    /// dispatch whose caller no longer needs the result (e.g. a disconnected client) and
    /// would rather reclaim the GPU/CPU than wait it out.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub fn accumulate_cancellable(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        self.accumulate_via_pipeline(
            scene,
            pipeline_class,
            sample_offset,
            spp,
            accum,
            Some(cancel),
        )
    }

    /// The body [`Self::accumulate`]/[`Self::accumulate_cancellable`] delegate to,
    /// parameterised on `pipeline_class` (one of [`material_class`]'s values) rather than
    /// deriving it from `scene.material` -- lets [`run_specialisation_equivalence`] force
    /// the GENERIC pipeline for a material [`classify_material`] would otherwise route to
    /// a specialised one, and vice versa, which is what proves the two pipelines agree.
    ///
    /// A thin driver over [`Self::accumulate_turn`]: runs turns of unlimited chunk budget
    /// (`max_chunks: usize::MAX`) back to back, starting from a fresh [`ChunkCursor`],
    /// until the whole request is `Done`/`Cancelled` -- i.e. this function alone never
    /// yields the renderer mid-request, exactly its pre-chunk-fairness behaviour. See
    /// [`Self::accumulate_turn`]'s own doc comment for the turn-granular entry point
    /// `renderer::gpu_backend::GpuBackend` actually drives for fairness across concurrent
    /// requests.
    ///
    /// `cancel`, when `Some`, is [`Self::accumulate_cancellable`]'s cooperative
    /// cancellation flag; `None` (every self-test, and [`Self::accumulate`]) means "never
    /// cancel".
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    fn accumulate_via_pipeline(
        &mut self,
        scene: &GpuFrameScene<'_>,
        pipeline_class: u32,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
        cancel: Option<&AtomicBool>,
    ) -> Result<AccumulateOutcome, GpuFrameError> {
        let mut cursor = ChunkCursor::default();
        let request = TurnRequest {
            scene,
            pipeline_class,
            sample_offset,
            spp,
            cancel,
            max_chunks: usize::MAX,
        };
        loop {
            match self.accumulate_turn(&request, accum, &mut cursor)? {
                ChunkTurnOutcome::Done => return Ok(AccumulateOutcome::Done),
                ChunkTurnOutcome::Cancelled => return Ok(AccumulateOutcome::Cancelled),
                // Unreachable with an unlimited turn budget (the turn loop below only
                // stops early on `max_chunks`, never hit here) -- kept rather than
                // `unreachable!()` so this driver stays correct even if that invariant
                // ever changes.
                ChunkTurnOutcome::MoreWork => {}
            }
        }
    }

    /// One fairness "turn" through the chunk loop: dispatches at most
    /// `request.max_chunks` chunks starting from `cursor`'s progress, updates `cursor` in
    /// place, and returns without leaving anything in-flight on the GPU -- see
    /// [`ChunkTurnOutcome::MoreWork`]'s doc comment for exactly what "nothing in flight"
    /// guarantees for a caller that hands this same renderer to a DIFFERENT request's
    /// turn next.
    ///
    /// Every turn -- including a RESUMED one -- re-verifies and re-uploads
    /// `self.scene_buffers` via [`FrameSceneBuffers::ensure`] before dispatching anything
    /// (see [`Self::prepare_turn`]). That upload is cheap (four small buffers) relative
    /// to a chunk's own GPU work, and it is what makes resuming correct: a different
    /// request's turn may have run in between and overwritten those same persistent
    /// buffers with ITS scene, and this call has no other way to know. See
    /// `renderer::gpu_backend`'s module doc comment ("Scene re-upload: the fairness
    /// cost") for the resulting overhead.
    ///
    /// Split into [`Self::prepare_turn`] (setup) and [`Self::run_turn_chunks`] (the
    /// dispatch/drain loop) purely to keep this function under clippy's function-length
    /// limit; [`TurnRequest`] bundles the arguments both this function and
    /// [`Self::accumulate_via_pipeline`] (its `max_chunks: usize::MAX` driver) share, to
    /// stay under the argument-count limit too.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub(crate) fn accumulate_turn(
        &mut self,
        request: &TurnRequest<'_>,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<ChunkTurnOutcome, GpuFrameError> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        if request.spp == 0 || num_pixels == 0 {
            return Ok(ChunkTurnOutcome::Done);
        }

        // GemMaterial::gpu_supported is the crate's routing predicate; a caller assembling
        // a full scene must consult it before routing to the GPU. Currently unreachable
        // (unconditionally true), but enforced so a future material kind the megakernel
        // cannot handle produces an error here rather than a plausible-looking wrong image.
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }

        let TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        } = self.prepare_turn(request);

        // frame_buffers is threaded through by value (not `&mut self.scene_buffers`
        // directly) so `run_turn_chunks` can keep borrowing `self` mutably for
        // `dispatch_chunk`/`drain_pending_chunk` without an overlapping borrow -- see
        // `FrameSceneBuffers::ensure`'s own doc comment. Not put back into
        // `self.scene_buffers` on an `Err` from below (a `?`-propagated `DeviceLost`):
        // that variant means this whole renderer is about to be discarded (see
        // `GpuFrameError::DeviceLost`'s doc comment), so leaving `self.scene_buffers` as
        // `None` there is harmless.
        let (frame_buffers, cancelled) = self.run_turn_chunks(
            request,
            &state,
            frame_buffers,
            byte_budget_pixels,
            accum,
            cursor,
        )?;

        // Put the (possibly newly-created) persistent scene buffers back for the next
        // turn -- this request's own resumption, or a completely different request's --
        // to reuse.
        self.scene_buffers = Some(frame_buffers);

        if cancelled {
            return Ok(ChunkTurnOutcome::Cancelled);
        }
        if cursor.first_pixel >= num_pixels {
            return Ok(ChunkTurnOutcome::Done);
        }
        Ok(ChunkTurnOutcome::MoreWork)
    }

    /// The setup [`Self::accumulate_turn`] must redo on EVERY turn before it can
    /// dispatch a single chunk -- see that function's own doc comment for why a resumed
    /// turn cannot skip this. Split out purely to keep `accumulate_turn` under clippy's
    /// function-length limit.
    ///
    /// Infallible since finding G6 (every [`EnvironmentSource`] now has an `env_mode`,
    /// see [`environment_params`]) -- no longer wrapped in a `Result` clippy's
    /// `unnecessary_wraps` would flag as never actually `Err`. [`GpuFrameError::UnsupportedMaterial`]
    /// is still checked, but by [`Self::accumulate_turn`] itself, before this is called.
    fn prepare_turn<'a>(&mut self, request: &TurnRequest<'a>) -> TurnSetup<'a> {
        let scene = request.scene;
        let num_pixels = scene.width as usize * scene.height as usize;

        let (
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            use_d65,
            studio_model,
            backdrop,
        ) = environment_params(scene.environment);
        // The per-preset adaptation the CPU tracer applies (identity for an HDR map and
        // for the lit D65 models, Planckian for the rest), so a hybrid frame's CPU and
        // GPU tiles agree.
        let white_balance = environment_white_balance(scene.environment);

        let gpu_material = GpuGemMaterial::encode(scene.material);
        let gpu_finishes = encode_facet_finishes(scene.facet_finishes, scene.planes.len());

        self.ensure_specialized_pipeline(request.pipeline_class);

        // byte_budget_pixels is chunk_budget_bytes's hard ceiling in pixels -- output
        // buffers are sized against it ONCE, up front, so every dispatch below fits
        // without growing buffers mid-frame. See next_chunk_pixels for why the actual
        // per-chunk pixel count is computed fresh every iteration instead.
        let byte_budget_pixels = chunk_pixels_for(self.chunk_budget_bytes, request.spp, num_pixels);
        self.ensure_capacity(byte_budget_pixels * request.spp as usize);
        self.ensure_pixel_capacity(byte_budget_pixels);
        self.ensure_staging_capacity(byte_budget_pixels);
        self.ensure_params_buffers();
        self.ensure_reduce_params_buffers();

        // camera/material/geometry/facet-finishes are PERSISTENT across every
        // `accumulate` call now (see FrameSceneBuffers::ensure), not merely uploaded
        // once per call -- camera_params carries the FULL frame's dimensions, not a
        // chunk-local one. Every chunk below reuses these same four buffers via
        // build_chunk_bind_group, writing only GpuTransportParams's persistent per-slot
        // buffer per chunk. See the module doc comment's "Per-frame uploads, persistent
        // staging" section and finding G3.
        let camera_params = GpuCameraParams {
            origin: scene.camera.origin.to_array(),
            fov_tan: scene.camera.fov_tan,
            forward: scene.camera.forward.to_array(),
            width: scene.width as f32,
            right: scene.camera.right.to_array(),
            height: scene.height as f32,
            up: scene.camera.up.to_array(),
            num_samples: request.spp,
        };
        // Taken out of `self.scene_buffers` and returned as an OWNED value rather than a
        // `&FrameSceneBuffers` borrowed from `self` -- see `FrameSceneBuffers::ensure`'s
        // doc comment for why: it lets `self.dispatch_chunk`/`self.drain_pending_chunk`
        // keep borrowing `self` freely without conflicting with a borrow this would
        // otherwise be holding from `self.scene_buffers`. The caller
        // ([`Self::accumulate_turn`], via [`Self::run_turn_chunks`]) is responsible for
        // putting it back into `self.scene_buffers` once done. Called EVERY turn, resumed
        // or not -- see `accumulate_turn`'s own doc comment for why that's required now
        // that a different request's turn can run in between two of this one's.
        let frame_buffers = FrameSceneBuffers::ensure(
            self.scene_buffers.take(),
            &self.ctx,
            &SceneBufferInputs {
                camera_params: &camera_params,
                material: &gpu_material,
                planes: scene.planes,
                facet_finishes: &gpu_finishes,
                environment: scene.environment,
            },
        );

        let state = ChunkFrameState {
            scene,
            pipeline_class: request.pipeline_class,
            sample_offset: request.sample_offset,
            spp: request.spp,
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            white_balance,
            use_d65,
            studio_model,
            backdrop,
        };

        TurnSetup {
            state,
            frame_buffers,
            byte_budget_pixels,
        }
    }

    /// The dispatch/drain loop body of [`Self::accumulate_turn`]: dispatches at most
    /// `request.max_chunks` chunks starting from `cursor`'s progress, then drains
    /// whatever chunk the overlapped pipeline may still have in flight before returning
    /// -- see [`ChunkTurnOutcome::MoreWork`]'s doc comment for why that final drain is
    /// required regardless of how this loop ends. Updates `cursor` in place and returns
    /// `frame_buffers` back to the caller (to put back into `self.scene_buffers`)
    /// alongside whether `request.cancel` fired during this turn. Split out of
    /// `accumulate_turn` purely to keep that function under clippy's function-length
    /// limit.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    fn run_turn_chunks(
        &mut self,
        request: &TurnRequest<'_>,
        state: &ChunkFrameState<'_>,
        frame_buffers: FrameSceneBuffers,
        byte_budget_pixels: usize,
        accum: &mut [Vec3],
        cursor: &mut ChunkCursor,
    ) -> Result<(FrameSceneBuffers, bool), GpuFrameError> {
        let num_pixels = state.scene.width as usize * state.scene.height as usize;

        // One chunk's dispatch+copy is submitted, then (if a PREVIOUS chunk is still
        // pending) that older chunk is mapped/read/summed -- so by the time the CPU
        // blocks on chunk i's result, chunk i+1's GPU work is already queued behind it.
        // See the module doc comment's "Overlapped chunk pipeline" section. Resumed from
        // `cursor`'s progress rather than always starting at pixel 0/chunk 0, so a turn
        // picks up exactly where the previous one (on this same request) left off.
        let mut pending: Option<PendingChunk> = None;
        let mut first_pixel = cursor.first_pixel;
        let mut chunk_index = cursor.chunk_index;
        let mut cancelled = false;
        let mut chunks_this_turn = 0usize;
        while first_pixel < num_pixels {
            if request
                .cancel
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                cancelled = true;
                break;
            }
            if chunks_this_turn >= request.max_chunks {
                // This turn's budget is spent -- stop dispatching new chunks, but a
                // chunk already dispatched this turn (if any) is still drained below
                // exactly like every other exit from this loop, so nothing is ever left
                // pending across a yielded turn.
                break;
            }

            let chunk_pixels = Self::next_chunk_pixels(
                self.ns_per_tuple_ema,
                byte_budget_pixels,
                state.spp,
                num_pixels,
            );
            let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);

            // Dispatch THEN drain, in that order -- so chunk i+1's GPU work is already
            // queued by the time this call blocks (inside `drain_pending_chunk`) on
            // chunk i's readback. See this function's own "R4" note above and the module
            // doc comment's "Overlapped chunk pipeline" section.
            let new_pending = self.dispatch_chunk(
                state,
                &frame_buffers,
                first_pixel,
                pixels_this_chunk,
                chunk_index,
            );
            if let Some(prev) = pending.take() {
                self.drain_pending_chunk(prev, state.spp, Some(&mut *accum))?;
            }
            pending = Some(new_pending);

            first_pixel += pixels_this_chunk;
            chunk_index += 1;
            chunks_this_turn += 1;
        }

        // Whatever this turn ends with -- done, cancelled, or its chunk budget merely
        // spent -- the ONE chunk the overlapped pipeline may still have in flight is
        // drained right here, unconditionally, before this function returns control to
        // the caller. This is the invariant `ChunkTurnOutcome::MoreWork`'s doc comment
        // promises: a turn never returns with GPU work still outstanding, which is what
        // lets a completely different request safely take the next turn on this same
        // renderer. `cancelled` still discards the drained chunk's samples
        // (drain-then-discard, see `AccumulateOutcome::Cancelled`'s doc comment);
        // reaching the pixel budget or this turn's chunk budget both keep them --
        // neither is a cancellation, so nothing traced this turn is thrown away.
        if let Some(chunk) = pending.take() {
            if cancelled {
                self.drain_pending_chunk(chunk, state.spp, None)?;
            } else {
                self.drain_pending_chunk(chunk, state.spp, Some(&mut *accum))?;
            }
        }

        cursor.first_pixel = first_pixel;
        cursor.chunk_index = chunk_index;

        Ok((frame_buffers, cancelled))
    }

    /// Resolves the four per-slot GPU resources [`Self::dispatch_chunk`] needs for one
    /// chunk (bundled as [`ChunkSlotResources`]), asserting each is large enough for
    /// this dispatch. Split out of `dispatch_chunk` purely to keep that method under
    /// clippy's function-length limit.
    fn resolve_chunk_slot_resources(
        &self,
        staging_slot: usize,
        tuples: usize,
        pixels_this_chunk: usize,
    ) -> ChunkSlotResources<'_> {
        let outputs = self.outputs[staging_slot]
            .as_ref()
            .expect("ensure_capacity just populated both slots");
        assert!(
            tuples <= outputs.capacity,
            "dispatch of {tuples} tuples exceeds output capacity {}",
            outputs.capacity
        );

        let pixel_output = self.pixel_outputs[staging_slot]
            .as_ref()
            .expect("ensure_pixel_capacity just populated both slots");
        assert!(
            pixels_this_chunk <= pixel_output.capacity,
            "reduced-pixel dispatch of {pixels_this_chunk} pixels exceeds pixel output \
             capacity {}",
            pixel_output.capacity
        );

        let staging_buffer = &self.staging[staging_slot]
            .as_ref()
            .expect("ensure_staging_capacity just populated both slots")
            .buffer;

        let params_buf = self.params_buffers[staging_slot]
            .as_ref()
            .expect("ensure_params_buffers just populated both slots");

        ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        }
    }

    /// Builds and submits ONE chunk's transport dispatch, its `reduce_xyz_main`
    /// GPU-side sample-sum dispatch, and the non-blocking readback copy of the REDUCED
    /// per-pixel result, returning the resulting [`PendingChunk`] for a later
    /// [`Self::drain_pending_chunk`] to wait on.
    ///
    /// # Why a second dispatch (finding G2)
    ///
    /// `transport_main` writes one XYZ triple per (pixel, sample) THREAD into
    /// `outputs.xyz()` -- `pixels_this_chunk * spp` triples. Copying and mapping all of
    /// that back to sum it on the CPU (the old behaviour) scales readback with sample
    /// count: at 1080p x 8 spp, ~200MB per progressive pass. `reduce_xyz_main` instead
    /// sums each pixel's `spp` consecutive triples ON the GPU, one thread per PIXEL, into
    /// `self.pixel_outputs[staging_slot]` -- `pixels_this_chunk` triples, independent of
    /// `spp`. Only THAT smaller buffer is copied to staging below; `outputs.xyz()` itself
    /// never leaves the GPU. See `reduce_xyz.wgsl`'s header comment for the determinism
    /// argument (fixed ascending summation order, same as the CPU used to sum in).
    ///
    /// Factored out of [`Self::accumulate_via_pipeline`]'s loop body to keep that function
    /// under clippy's function-length limit; `state` bundles the per-frame-constant
    /// values the loop would otherwise close over directly (see [`ChunkFrameState`]).
    fn dispatch_chunk(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        first_pixel: usize,
        pixels_this_chunk: usize,
        chunk_index: usize,
    ) -> PendingChunk {
        let tuples = pixels_this_chunk * state.spp as usize;
        let planes_len = state.scene.planes.len();

        // camera_params/material/planes/facet_finishes are NOT rebuilt or re-uploaded
        // here -- frame_buffers already holds them, persistent across every accumulate
        // call (see FrameSceneBuffers). params is written into this slot's persistent
        // uniform buffer below instead of uploaded fresh -- see finding G3.
        let params = build_chunk_params(state, first_pixel, pixels_this_chunk);

        let staging_slot = chunk_index % 2;
        let ChunkSlotResources {
            outputs,
            pixel_output,
            staging_buffer,
            params_buf,
        } = self.resolve_chunk_slot_resources(staging_slot, tuples, pixels_this_chunk);

        // Written into this slot's PERSISTENT uniform buffer rather than uploaded fresh
        // -- see GpuFrameRenderer::params_buffers' doc comment for why reusing a slot
        // two chunks later is safe (queue-ordering: this write is submitted strictly
        // after the earlier chunk that read the same slot).
        self.ctx
            .queue
            .write_buffer(params_buf, 0, bytemuck::bytes_of(&params));

        let submitted_at = Instant::now();
        // Finding G5 Part B: which kernel(s) populate `outputs.xyz()` for this chunk --
        // see `GpuPipelineKind`'s own doc comment. Either way, everything from here on
        // (`dispatch_reduce`, the readback copy, `PendingChunk`) is UNCHANGED: both
        // pipelines write into the exact same `outputs.xyz()` binding, at the exact
        // same per-(pixel,sample) index, so the shared tail below needs no branch of
        // its own.
        match self.pipeline_kind {
            GpuPipelineKind::Megakernel => {
                let pipeline = self.pipeline_for_class(state.pipeline_class);
                let bind_group = build_chunk_bind_group(
                    &self.ctx,
                    pipeline,
                    frame_buffers,
                    params_buf,
                    planes_len,
                    outputs,
                );
                let workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
                // The compute dispatch's own submission index is unused: waiting for
                // the readback copy's index (submitted after this one, on the same
                // queue) is sufficient -- see `compute::finish_map_read`'s doc comment.
                let _ = compute::dispatch(
                    &self.ctx.device,
                    &self.ctx.queue,
                    pipeline,
                    &bind_group,
                    (workgroups, 1, 1),
                );
            }
            GpuPipelineKind::Wavefront => {
                self.dispatch_chunk_wavefront(
                    state,
                    frame_buffers,
                    params_buf,
                    planes_len,
                    outputs,
                    tuples,
                );
            }
        }

        // GPU-side sample reduction -- see this function's own doc comment ("Why a
        // second dispatch"). Reads outputs.xyz() (just written by the dispatch above;
        // safe without an explicit barrier because wgpu executes queue submissions in
        // order, and this dispatch is submitted strictly after it on the same queue --
        // the exact guarantee the readback copy right below already relies on) and
        // writes pixel_output.buffer. Split into its own method to keep this one under
        // clippy's function-length limit.
        self.dispatch_reduce(
            state.spp,
            staging_slot,
            pixels_this_chunk,
            outputs,
            pixel_output,
        );

        let copy_index = copy_to_existing_staging::<f32>(
            &self.ctx.device,
            &self.ctx.queue,
            &pixel_output.buffer,
            staging_buffer,
            pixels_this_chunk * 3,
        );
        let rx = compute::begin_map_read(staging_buffer);

        PendingChunk {
            staging_slot,
            copy_index,
            rx,
            first_pixel,
            pixels_this_chunk,
            submitted_at,
        }
    }

    /// Submits `reduce_xyz_main`'s dispatch for one chunk -- the second half of
    /// [`Self::dispatch_chunk`]'s "Why a second dispatch" doc comment, split into its own
    /// method purely to keep `dispatch_chunk` under clippy's function-length limit. Sums
    /// `outputs.xyz()`'s `pixels_this_chunk * num_samples` triples down into
    /// `pixel_output.buffer`'s `pixels_this_chunk` triples.
    fn dispatch_reduce(
        &self,
        num_samples: u32,
        staging_slot: usize,
        pixels_this_chunk: usize,
        outputs: &TransportOutputs,
        pixel_output: &PixelXyzOutput,
    ) {
        let reduce_params_buf = self.reduce_params_buffers[staging_slot]
            .as_ref()
            .expect("ensure_reduce_params_buffers just populated both slots");
        let reduce_params = GpuReduceParams {
            num_pixels: pixels_this_chunk as u32,
            num_samples,
            _pad0: 0,
            _pad1: 0,
        };
        self.ctx
            .queue
            .write_buffer(reduce_params_buf, 0, bytemuck::bytes_of(&reduce_params));
        let reduce_bind_group = compute::bind_buffers(
            &self.ctx.device,
            "reduce xyz bind group",
            &self.reduce_pipeline,
            &[
                (0, reduce_params_buf),
                (1, outputs.xyz()),
                (2, &pixel_output.buffer),
            ],
        );
        let reduce_workgroups = (pixels_this_chunk as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            &self.ctx.device,
            &self.ctx.queue,
            &self.reduce_pipeline,
            &reduce_bind_group,
            (reduce_workgroups, 1, 1),
        );
    }

    /// Finding G5 Part B: runs one chunk through the wavefront pipeline instead of the
    /// megakernel -- `wavefront_generate`, then `wavefront_bounce`/`wavefront_compact_*`
    /// once per bounce round until either every ray has died or
    /// `state.scene.max_bounces` rounds have run, then `wavefront_finalize_survivors`
    /// for whatever remains. Populates `outputs.xyz()` (and, when `params` requested
    /// them, the debug buffers) exactly as the megakernel would have for this same
    /// chunk -- see `dispatch_chunk`'s own comment at its call site.
    ///
    /// Allocates a fresh set of ray-state buffers EVERY call, sized exactly to `tuples`
    /// -- unlike `outputs`/`staging`/etc., which are grown-never-shrunk persistent
    /// buffers reused across chunks. A documented simplification for this first cut
    /// (see this module's doc comment's "Wavefront pipeline" section): the megakernel
    /// path's overlapped, persistent-buffer machinery is substantial
    /// (`FrameSceneBuffers`/`TransportOutputs`/`StagingSlot` growth policies,
    /// double-buffering), and mirroring all of it for a pipeline that defaults OFF
    /// until measured was judged not worth the risk here -- revisit if
    /// `GpuPipelineKind::Wavefront` sees real use and per-chunk allocation shows up in
    /// profiling.
    ///
    /// The compaction round-trip (`wavefront_compact_scan`'s block counts read back,
    /// CPU exclusive-prefix-summed, `compact_block_offset` re-uploaded) is a genuine
    /// synchronous stall per bounce round -- see `wavefront_compact_scan`'s own doc
    /// comment in `wavefront_transport.wgsl` for why an order-preserving compaction was
    /// chosen anyway. This is the other half of why this pipeline stays opt-in: it
    /// cannot currently participate in `dispatch_chunk`'s overlapped-chunk pipeline the
    /// way the megakernel path does.
    fn dispatch_chunk_wavefront(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        planes_len: usize,
        outputs: &TransportOutputs,
        tuples: usize,
    ) {
        let pipelines = self
            .wavefront_pipelines
            .as_ref()
            .expect("set_pipeline_kind(Wavefront) must be called before dispatching it");
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;

        let rays = WavefrontRayBuffers::new(device, tuples);
        let wf_params_buf = compute::upload(
            device,
            "wavefront params",
            &[GpuWavefrontParams {
                chunk_rays: tuples as u32,
                active_count: tuples as u32,
                bounce: 0,
                workgroup_count: (tuples as u32).div_ceil(WORKGROUP_SIZE as u32),
            }],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        // 1. wavefront_generate -- see `wavefront_transport.wgsl`'s header comment,
        // kernel sequence step 1. Needs only `camera`/`params` from the shared scene
        // bindings (camera-ray generation) plus its own ray-state/active-list buffers.
        let generate_bind_group = compute::bind_buffers(
            device,
            "wavefront generate bind group",
            &pipelines.generate,
            &wavefront_generate_bindings(&frame_buffers.camera, params_buf, &wf_params_buf, &rays),
        );
        let generate_workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
        let _ = compute::dispatch(
            device,
            queue,
            &pipelines.generate,
            &generate_bind_group,
            (generate_workgroups, 1, 1),
        );

        let bind_groups = build_wavefront_round_bind_groups(
            device,
            pipelines,
            frame_buffers,
            params_buf,
            planes_len,
            outputs,
            &wf_params_buf,
            &rays,
        );

        // 2-4. Bounce rounds: dispatch, order-preserving compact, repeat -- see
        // `wavefront_transport.wgsl`'s header comment, kernel sequence steps 2-4.
        let (active_count, bounce) = self.run_wavefront_bounce_rounds(
            state.scene.max_bounces,
            tuples as u32,
            &rays,
            &wf_params_buf,
            &bind_groups,
        );

        // 5. wavefront_finalize_survivors -- see `wavefront_transport.wgsl`'s header
        // comment, kernel sequence step 5. Only reached when the bounce budget ran out
        // with rays still alive, not when every ray already died (those are finalized
        // inside `wavefront_bounce` itself, above).
        if active_count > 0 {
            queue.write_buffer(
                &wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays: tuples as u32,
                    active_count,
                    bounce,
                    workgroup_count: active_count.div_ceil(WORKGROUP_SIZE as u32),
                }),
            );
            let finalize_workgroups = active_count.div_ceil(WORKGROUP_SIZE as u32);
            let _ = compute::dispatch(
                device,
                queue,
                &pipelines.finalize_survivors,
                &bind_groups.finalize,
                (finalize_workgroups, 1, 1),
            );
        }
    }

    /// Runs [`WavefrontRayBuffers::active_in`]'s bounce/compact loop -- steps 2-4 of
    /// `wavefront_transport.wgsl`'s kernel sequence -- until either no ray remains
    /// alive or `max_bounces` rounds have run. Returns the final `(active_count,
    /// bounce)` for [`Self::dispatch_chunk_wavefront`]'s own step 5. Split out purely
    /// to keep that function under clippy's function-length limit.
    fn run_wavefront_bounce_rounds(
        &self,
        max_bounces: u32,
        chunk_rays: u32,
        rays: &WavefrontRayBuffers,
        wf_params_buf: &wgpu::Buffer,
        bind_groups: &WavefrontRoundBindGroups,
    ) -> (u32, u32) {
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let pipelines = self
            .wavefront_pipelines
            .as_ref()
            .expect("set_pipeline_kind(Wavefront) must be called before dispatching it");

        let mut active_count = chunk_rays;
        let mut bounce = 0u32;
        while active_count > 0 && bounce < max_bounces {
            let workgroup_count = active_count.div_ceil(WORKGROUP_SIZE as u32);
            queue.write_buffer(
                wf_params_buf,
                0,
                bytemuck::bytes_of(&GpuWavefrontParams {
                    chunk_rays,
                    active_count,
                    bounce,
                    workgroup_count,
                }),
            );

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.bounce,
                &bind_groups.bounce,
                (workgroup_count, 1, 1),
            );
            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scan,
                &bind_groups.compact_scan,
                (workgroup_count, 1, 1),
            );

            // Deterministic CPU-side exclusive prefix sum over this round's per-workgroup
            // alive counts -- see `wavefront_compact_scan`'s own doc comment
            // (`wavefront_transport.wgsl`) for why this stays a synchronous host
            // round-trip rather than a GPU-side atomic counter.
            let block_counts: Vec<u32> = compute::readback(
                device,
                queue,
                &rays.block_alive_count,
                workgroup_count as usize,
            );
            let (block_offsets, new_active_count) = exclusive_prefix_sum(&block_counts);
            queue.write_buffer(&rays.block_offset, 0, bytemuck::cast_slice(&block_offsets));

            compute::dispatch_and_wait(
                device,
                queue,
                &pipelines.compact_scatter,
                &bind_groups.compact_scatter,
                (workgroup_count, 1, 1),
            );

            // `active_ray_indices_next` becomes the next round's `active_ray_indices` --
            // a plain buffer-to-buffer copy rather than swapping which buffer each bind
            // group references (bind groups are immutable once created in `wgpu`), so
            // every bind group built above stays valid for every round.
            if new_active_count > 0 {
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("wavefront active-list copy encoder"),
                });
                encoder.copy_buffer_to_buffer(
                    &rays.active_out,
                    0,
                    &rays.active_in,
                    0,
                    u64::from(new_active_count) * size_of::<u32>() as u64,
                );
                queue.submit(Some(encoder.finish()));
            }

            active_count = new_active_count;
            bounce += 1;
        }
        (active_count, bounce)
    }

    /// Picks how many pixels the NEXT chunk covers.
    ///
    /// `byte_budget_pixels` (from [`chunk_pixels_for`] against `self.chunk_budget_bytes`)
    /// is always the hard upper bound -- time-budgeted sizing only ever shrinks a chunk
    /// below it to approach [`TARGET_CHUNK_MS`], never raises it, so `chunk_budget_bytes`
    /// keeps meaning a byte-budget ceiling on one dispatch's output buffers.
    ///
    /// `ema_ns_per_tuple` being `None` (no chunk has drained yet) uses
    /// [`FIRST_DISPATCH_MAX_TUPLES`] instead of the full byte budget, so a cold
    /// integrated GPU's first dispatch(es) cannot alone trip Windows' TDR watchdog. Once
    /// a measurement exists, the target pixel count is clamped between
    /// [`MIN_CHUNK_BYTES`] worth of tuples (floor) and `byte_budget_pixels` (ceiling).
    fn next_chunk_pixels(
        ema_ns_per_tuple: Option<f64>,
        byte_budget_pixels: usize,
        spp: u32,
        num_pixels: usize,
    ) -> usize {
        let spp_usize = (spp as usize).max(1);
        let Some(ns_per_tuple) = ema_ns_per_tuple.filter(|v| *v > 0.0) else {
            let first_dispatch_pixels = (FIRST_DISPATCH_MAX_TUPLES / spp_usize).max(1);
            return byte_budget_pixels
                .min(first_dispatch_pixels)
                .min(num_pixels)
                .max(1);
        };
        let target_tuples = (TARGET_CHUNK_MS * 1.0e6 / ns_per_tuple).max(1.0);
        let target_pixels = ((target_tuples / spp_usize as f64).floor() as usize).max(1);
        let min_pixels = chunk_pixels_for(MIN_CHUNK_BYTES, spp, num_pixels).min(byte_budget_pixels);
        target_pixels.clamp(min_pixels, byte_budget_pixels)
    }

    /// Blocks until `chunk`'s readback copy has completed, reads its XYZ, updates the
    /// per-tuple timing EMA (see [`Self::ns_per_tuple_ema`]), and -- when `accum` is
    /// `Some` -- sums each pixel's `spp` samples into it. Split out of
    /// [`Self::accumulate_via_pipeline`] so its dispatch loop stays readable.
    ///
    /// `accum: None` is [`AccumulateOutcome::Cancelled`]'s drain-then-discard path: the
    /// chunk is still waited on and read back (so no dangling GPU/mapping state leaks
    /// into the next call), but its result is thrown away rather than summed. The EMA is
    /// still updated either way -- a cancelled chunk's timing is a real measurement too.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::DeviceLost`] if the underlying [`compute::finish_map_read_into`]
    /// reports a timed-out or failed wait.
    fn drain_pending_chunk(
        &mut self,
        chunk: PendingChunk,
        spp: u32,
        accum: Option<&mut [Vec3]>,
    ) -> Result<(), GpuFrameError> {
        let staging_buffer = &self.staging[chunk.staging_slot]
            .as_ref()
            .expect("dispatch_chunk populated this staging slot")
            .buffer;
        // Reads into self.xyz_scratch (reused across every chunk of every frame) rather
        // than allocating a fresh Vec here -- see that field's own doc comment and
        // finding G2. staging_buffer/self.xyz_scratch/self.ctx.device are three disjoint
        // fields of `self`, so borrowing them independently here is fine even though
        // this function takes `&mut self`.
        compute::finish_map_read_into(
            &self.ctx.device,
            staging_buffer,
            chunk.copy_index,
            &chunk.rx,
            &mut self.xyz_scratch,
        )
        .map_err(|e| GpuFrameError::DeviceLost(e.to_string()))?;

        let tuples_measured = chunk.pixels_this_chunk * spp as usize;
        if tuples_measured > 0 {
            let elapsed_ns = chunk.submitted_at.elapsed().as_nanos() as f64;
            let measured_ns_per_tuple = elapsed_ns / tuples_measured as f64;
            self.ns_per_tuple_ema =
                Some(self.ns_per_tuple_ema.map_or(measured_ns_per_tuple, |prev| {
                    CHUNK_TIMING_EMA_ALPHA.mul_add(measured_ns_per_tuple - prev, prev)
                }));
        }

        // self.xyz_scratch already holds `reduce_xyz_main`'s GPU-summed per-pixel
        // triples (see dispatch_chunk's "Why a second dispatch" doc comment) -- added
        // directly, no CPU-side per-sample summation loop needed any more.
        if let Some(accum) = accum {
            let xyz = &self.xyz_scratch;
            for local_pixel in 0..chunk.pixels_this_chunk {
                let base = local_pixel * 3;
                accum[chunk.first_pixel + local_pixel] +=
                    Vec3::new(xyz[base], xyz[base + 1], xyz[base + 2]);
            }
        }
        Ok(())
    }

    /// Builds and caches the specialised pipeline for `class`, if not already built --
    /// see the module doc comment's "Material-class kernel specialisation" section for
    /// why this is lazy. A no-op for [`material_class::GENERIC`], built eagerly in
    /// [`Self::new`].
    fn ensure_specialized_pipeline(&mut self, class: u32) {
        let (slot, label) = match class {
            material_class::ISOTROPIC => (
                &mut self.pipeline_isotropic,
                "transport_main (MATERIAL_CLASS=isotropic)",
            ),
            material_class::UNIAXIAL => (
                &mut self.pipeline_uniaxial,
                "transport_main (MATERIAL_CLASS=uniaxial)",
            ),
            material_class::BIAXIAL => (
                &mut self.pipeline_biaxial,
                "transport_main (MATERIAL_CLASS=biaxial)",
            ),
            // GENERIC (and any other value -- none other is caller-reachable) has nothing
            // to build: Self::new already compiled self.pipeline.
            _ => return,
        };
        if slot.is_none() {
            *slot = Some(compute::create_compute_pipeline_with_constants(
                &self.ctx.device,
                label,
                SHADER_SRC,
                "transport_main",
                &[("MATERIAL_CLASS", f64::from(class))],
            ));
        }
    }

    /// Returns the already-built pipeline for `class` -- [`Self::ensure_specialized_pipeline`]
    /// must have been called for this exact `class` first (every caller in this module
    /// does so immediately before dispatching).
    ///
    /// # Panics
    ///
    /// Panics if `class` is a specialised value whose pipeline was never built via
    /// [`Self::ensure_specialized_pipeline`] -- a bug in this module's own call
    /// ordering, never a condition a caller outside it can trigger.
    const fn pipeline_for_class(&self, class: u32) -> &wgpu::ComputePipeline {
        match class {
            material_class::ISOTROPIC => self.pipeline_isotropic.as_ref(),
            material_class::UNIAXIAL => self.pipeline_uniaxial.as_ref(),
            material_class::BIAXIAL => self.pipeline_biaxial.as_ref(),
            _ => Some(&self.pipeline),
        }
        .expect("ensure_specialized_pipeline must be called for this class before dispatching")
    }

    /// Grows the cached output buffers to hold at least `tuples`, reusing them when
    /// already large enough (the common case: a progressive render re-dispatches the same
    /// resolution until the camera moves). Grows BOTH double-buffered slots identically,
    /// since a chunk can land in either.
    fn ensure_capacity(&mut self, tuples: usize) {
        for slot in &mut self.outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= tuples);
            if !big_enough {
                *slot = Some(TransportOutputs::new_production(&self.ctx.device, tuples));
            }
        }
    }

    /// Grows the persistent staging buffers to hold at least `pixels` pixels' worth of
    /// (GPU-reduced) XYZ floats, reusing them when already large enough. Mirrors
    /// [`Self::ensure_capacity`]'s policy for `outputs` -- grow both slots identically,
    /// never shrink -- via the same pure predicate, [`staging_needs_growth`]. Sized in
    /// PIXELS, not (pixel, sample) tuples, since [`Self::dispatch_chunk`]'s readback copy
    /// now reads from [`PixelXyzOutput`] (finding G2), not [`TransportOutputs::xyz`]
    /// directly -- see [`StagingSlot::capacity`]'s own doc comment.
    fn ensure_staging_capacity(&mut self, pixels: usize) {
        for slot in &mut self.staging {
            let current_capacity = slot.as_ref().map_or(0, |s| s.capacity);
            if staging_needs_growth(current_capacity, pixels) {
                *slot = Some(StagingSlot::new(
                    &self.ctx.device,
                    "transport out xyz staging (persistent)",
                    pixels,
                ));
            }
        }
    }

    /// Grows the cached [`PixelXyzOutput`] buffers to hold at least `pixels` pixels,
    /// reusing them when already large enough. Mirrors [`Self::ensure_capacity`]'s policy
    /// for `outputs` exactly, just sized in pixels rather than (pixel, sample) tuples --
    /// see finding G2.
    fn ensure_pixel_capacity(&mut self, pixels: usize) {
        for slot in &mut self.pixel_outputs {
            let big_enough = slot.as_ref().is_some_and(|o| o.capacity >= pixels);
            if !big_enough {
                *slot = Some(PixelXyzOutput::new(&self.ctx.device, pixels));
            }
        }
    }

    /// Creates both persistent [`GpuTransportParams`] uniform buffers on first use --
    /// see [`Self::params_buffers`]'s doc comment. Fixed-size (the struct never changes
    /// size), so unlike [`Self::ensure_capacity`] this only ever creates, never grows.
    fn ensure_params_buffers(&mut self) {
        for slot in &mut self.params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("transport params (persistent)"),
                    size: size_of::<GpuTransportParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }

    /// Like [`Self::ensure_params_buffers`], for [`Self::reduce_params_buffers`].
    fn ensure_reduce_params_buffers(&mut self) {
        for slot in &mut self.reduce_params_buffers {
            if slot.is_none() {
                *slot = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("reduce xyz params (persistent)"),
                    size: size_of::<GpuReduceParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
        }
    }
}

/// wasm32-only async counterparts to [`GpuFrameRenderer::new`]/[`GpuFrameRenderer::accumulate`].
///
/// A separate `impl` block, not `#[cfg]` branches inside the existing methods: a
/// browser's main thread must never block, so there is no OS thread for
/// `pollster::block_on` to park and no `Device::poll(Maintain::Wait)` to synchronously
/// drive a `Buffer::map_async` callback the way native's `compute::begin_map_read`/
/// `finish_map_read` do. Both methods below are genuinely `async fn`s that `.await`
/// `wgpu`'s own futures -- a different control-flow shape that would make non-wasm32
/// callers pay `async`/`.await` for no reason if forced into the same signatures.
///
/// Also drops the double-buffered chunk overlap: `accumulate`'s "Overlapped chunk
/// pipeline" exists to keep the GPU busy while the CPU blocks mapping the PREVIOUS
/// chunk's readback. On wasm32 nothing blocks in the first place -- awaiting
/// [`map_read_async`] simply yields to the browser's microtask queue at zero CPU cost --
/// so there is no idle window to hide by queuing chunk i+1 early.
/// [`Self::accumulate_async`] dispatches, copies, and awaits each chunk in turn; still
/// chunked, just not pipelined two deep.
// clippy::future_not_send: every async fn below touches wgpu's web backend types, which
// hold browser-side Rc handles and are therefore !Send -- see
// GpuContext::acquire_async's identical allow. This impl block only exists on wasm32,
// where nothing is ever sent across threads, so the lint is meaningless here regardless.
#[allow(
    clippy::future_not_send,
    reason = "wasm32-unknown-unknown has no second thread to send a future to; see the \
              module-level comment above this impl block"
)]
#[cfg(target_arch = "wasm32")]
impl GpuFrameRenderer {
    /// Async, wasm32-only counterpart to [`Self::new`]: acquires a GPU device and
    /// compiles `transport_main` without blocking the browser's main thread.
    ///
    /// Awaits [`GpuContext::acquire_async`] directly rather than going through
    /// [`GpuContext::acquire`]'s `pollster::block_on` wrapper -- see this `impl` block's
    /// doc comment. Pipeline compilation itself is synchronous on every target, so
    /// nothing else here needs `.await`.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::new`]'s -- most commonly, the browser has no WebGPU
    /// support or no compatible adapter (Safari without the WebGPU flag, or a machine
    /// whose driver stack the browser declines to expose).
    pub async fn new_async() -> Result<Self, GpuFrameError> {
        let ctx = GpuContext::acquire_async()
            .await
            .map_err(GpuFrameError::Acquire)?;
        let available = ctx.device.limits().max_storage_buffers_per_shader_stage;
        if available < MEGAKERNEL_STORAGE_BUFFERS {
            return Err(GpuFrameError::DeviceLimits {
                needed: MEGAKERNEL_STORAGE_BUFFERS,
                available,
            });
        }
        let info = ctx.adapter.get_info();
        let adapter_label = format!("{} ({:?})", info.name, info.backend);
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "transport_main",
            SHADER_SRC,
            "transport_main",
        );
        let reduce_pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "reduce_xyz_main",
            REDUCE_SHADER_SRC,
            "reduce_xyz_main",
        );
        Ok(Self {
            ctx,
            pipeline,
            pipeline_isotropic: None,
            pipeline_uniaxial: None,
            pipeline_biaxial: None,
            reduce_pipeline,
            adapter_label,
            outputs: [None, None],
            pixel_outputs: [None, None],
            staging: [None, None],
            params_buffers: [None, None],
            reduce_params_buffers: [None, None],
            scene_buffers: None,
            xyz_scratch: Vec::new(),
            chunk_budget_bytes: CHUNK_BUDGET_BYTES,
            ns_per_tuple_ema: None,
            pipeline_kind: GpuPipelineKind::default(),
            wavefront_pipelines: None,
        })
    }

    /// Async, wasm32-only counterpart to [`Self::accumulate`] -- same inputs, same
    /// summed-XYZ-into-`accum` contract, same decline conditions
    /// ([`GpuFrameError::UnsupportedMaterial`]/[`GpuFrameError::UnsupportedEnvironment`]),
    /// but chunk-at-a-time rather than two-deep pipelined -- see this `impl` block's own
    /// doc comment for why.
    ///
    /// # Errors
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    ///
    /// # Panics
    ///
    /// Same conditions as [`Self::accumulate`]'s.
    pub async fn accumulate_async(
        &mut self,
        scene: &GpuFrameScene<'_>,
        sample_offset: u32,
        spp: u32,
        accum: &mut [Vec3],
    ) -> Result<(), GpuFrameError> {
        let pipeline_class = classify_material(scene.material);
        let num_pixels = scene.width as usize * scene.height as usize;
        assert_eq!(
            accum.len(),
            num_pixels,
            "accumulation buffer must have one entry per pixel"
        );
        if spp == 0 || num_pixels == 0 {
            return Ok(());
        }

        // See Self::accumulate_via_pipeline's identical check: unreachable today, kept as
        // defensive future-proofing.
        if !scene.material.gpu_supported() {
            return Err(GpuFrameError::UnsupportedMaterial(
                scene.material.name.clone(),
            ));
        }

        let (
            env_mode,
            temp_k,
            spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            use_d65,
            studio_model,
            backdrop,
        ) = environment_params(scene.environment);
        // See `Self::prepare_turn`.
        let white_balance = environment_white_balance(scene.environment);

        let gpu_material = GpuGemMaterial::encode(scene.material);
        let gpu_finishes = encode_facet_finishes(scene.facet_finishes, scene.planes.len());
        // Built once for the whole call, like `gpu_material`/`gpu_finishes` above -- this
        // path never had `FrameSceneBuffers`'s cross-call persistence (see this `impl`
        // block's own doc comment), so there is no identity cache to consult here, only
        // "once per call rather than once per chunk". See finding G6.
        let hdr_env = match scene.environment {
            EnvironmentSource::HdrMap(map) => HdrEnvGpuData::upload(&self.ctx.device, map),
            EnvironmentSource::Studio { .. } => HdrEnvGpuData::dummy(&self.ctx.device),
        };

        self.ensure_specialized_pipeline(pipeline_class);

        let chunk_pixels = chunk_pixels_for(self.chunk_budget_bytes, spp, num_pixels);
        self.ensure_capacity(chunk_pixels * spp as usize);
        self.ensure_pixel_capacity(chunk_pixels);

        let mut first_pixel = 0usize;
        let mut chunk_index = 0usize;
        while first_pixel < num_pixels {
            let pixels_this_chunk = chunk_pixels.min(num_pixels - first_pixel);
            let tuples = pixels_this_chunk * spp as usize;

            let camera_params = GpuCameraParams {
                origin: scene.camera.origin.to_array(),
                fov_tan: scene.camera.fov_tan,
                forward: scene.camera.forward.to_array(),
                width: scene.width as f32,
                right: scene.camera.right.to_array(),
                height: scene.height as f32,
                up: scene.camera.up.to_array(),
                num_samples: spp,
            };
            let params = GpuTransportParams::new(
                pixels_this_chunk as u32,
                scene.max_bounces,
                sample_offset,
                env_mode,
                0.0,
                temp_k,
                spot_mult,
                exposure,
                light_yaw,
                light_pitch,
                white_balance.to_array(),
            )
            .with_pixel_offset(first_pixel as u32)
            .with_debug_buffers_disabled()
            .with_studio_use_d65(use_d65)
            .with_studio_model(studio_model)
            .with_backdrop(backdrop);

            let outputs = self.outputs[chunk_index % 2]
                .as_ref()
                .expect("ensure_capacity just populated both slots");

            let bind_args = TransportDispatchArgs {
                ctx: &self.ctx,
                pipeline: self.pipeline_for_class(pipeline_class),
                camera_params: &camera_params,
                params: &params,
                material: &gpu_material,
                planes: scene.planes,
                facet_finishes: &gpu_finishes,
                outputs,
                hdr_env: &hdr_env,
            };

            // Non-blocking submit -- native's pipelined path builds its bind group via
            // build_chunk_bind_group instead (see the module doc comment's "Per-frame
            // uploads, persistent staging" section), but this wasm32 loop re-uploads every
            // buffer every chunk via build_bind_group: it never had the overlapped
            // double-buffering that fix targets, so there is no per-frame state to hoist
            // these uploads out of here.
            let bind_group = build_bind_group(&bind_args);
            let workgroups = (tuples as u32).div_ceil(WORKGROUP_SIZE as u32);
            let _ = compute::dispatch(
                &self.ctx.device,
                &self.ctx.queue,
                bind_args.pipeline,
                &bind_group,
                (workgroups, 1, 1),
            );

            // GPU-side sample reduction, mirroring native's `dispatch_chunk` -- see that
            // function's "Why a second dispatch" doc comment and finding G2. This wasm32
            // loop re-creates the tiny reduce-params buffer fresh every chunk (like every
            // other buffer here, per this function's own doc comment: it never had the
            // overlapped double-buffering finding G3 targets, so there is no per-frame
            // state to persist it in).
            let pixel_output = self.pixel_outputs[chunk_index % 2]
                .as_ref()
                .expect("ensure_pixel_capacity just populated both slots");
            let reduce_params = GpuReduceParams {
                num_pixels: pixels_this_chunk as u32,
                num_samples: spp,
                _pad0: 0,
                _pad1: 0,
            };
            let reduce_params_buf = compute::upload(
                &self.ctx.device,
                "reduce xyz params (wasm32 async)",
                std::slice::from_ref(&reduce_params),
                BufferUsages::UNIFORM,
            );
            let reduce_bind_group = compute::bind_buffers(
                &self.ctx.device,
                "reduce xyz bind group (wasm32 async)",
                &self.reduce_pipeline,
                &[
                    (0, &reduce_params_buf),
                    (1, outputs.xyz()),
                    (2, &pixel_output.buffer),
                ],
            );
            let reduce_workgroups = (pixels_this_chunk as u32).div_ceil(WORKGROUP_SIZE as u32);
            let _ = compute::dispatch(
                &self.ctx.device,
                &self.ctx.queue,
                &self.reduce_pipeline,
                &reduce_bind_group,
                (reduce_workgroups, 1, 1),
            );

            let (staging, _copy_index) = compute::copy_to_staging::<f32>(
                &self.ctx.device,
                &self.ctx.queue,
                &pixel_output.buffer,
                pixels_this_chunk * 3,
                "transport out pixel xyz staging (wasm32 async)",
            );

            // Awaits the browser's own resolution of map_async -- see map_read_async's
            // doc comment for why this needs no Device::poll call, unlike native's
            // finish_map_read. Returns GpuFrameError::DeviceLost on a mapping failure,
            // propagated here with `?`.
            let xyz: Vec<f32> = map_read_async(&staging, pixels_this_chunk * 3).await?;

            // xyz already holds reduce_xyz_main's GPU-summed per-pixel triples -- no
            // CPU-side per-sample summation loop needed any more.
            for local_pixel in 0..pixels_this_chunk {
                let base = local_pixel * 3;
                accum[first_pixel + local_pixel] +=
                    Vec3::new(xyz[base], xyz[base + 1], xyz[base + 2]);
            }

            first_pixel += pixels_this_chunk;
            chunk_index += 1;
        }

        Ok(())
    }
}

/// Awaits `staging`'s `Buffer::map_async` callback and reads back `count` `T`s, wasm32
/// only.
///
/// Native's `compute::finish_map_read` drives the identical callback to completion via
/// `Device::poll(Maintain::Wait)`, blocking the calling OS thread. `wgpu`'s WebGPU
/// backend needs no such call: a browser resolves `GPUBuffer.mapAsync`'s promise on its
/// own microtask queue as soon as ready -- the callback fires whenever this function
/// `.await`s the channel below, which is why this can be a genuine `async fn` instead of
/// needing a `Device::poll` equivalent.
///
/// # Errors
///
/// Returns [`GpuFrameError::DeviceLost`] on a dropped callback, a failed mapping, or an
/// unreadable mapped range, for [`GpuFrameRenderer::accumulate_async`] to propagate with
/// `?`. There is no bounded-wait timeout here the way `compute::GPU_WAIT_TIMEOUT` bounds
/// the native path: nothing here blocks an OS thread, so there is no thread to free by
/// timing out; a browser tab that never resolves `mapAsync` leaves this `Future` pending.
#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::future_not_send,
    reason = "wgpu's web backend's buffer-mapping state is inherently !Send (browser-side \
              Rc handles); wasm32-unknown-unknown has no second thread to send this future \
              to regardless -- see GpuContext::acquire_async's identical allow"
)]
async fn map_read_async<T: bytemuck::Pod>(
    staging: &wgpu::Buffer,
    count: usize,
) -> Result<Vec<T>, GpuFrameError> {
    let (tx, rx) = futures_channel::oneshot::channel();
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            // The receiver can only be dropped by this function returning early, which it
            // never does before this send -- a failed send would mean the callback
            // outlived its own awaiting task, not a race this single-threaded target can
            // produce.
            let _ = tx.send(result);
        });
    rx.await
        .map_err(|_| {
            GpuFrameError::DeviceLost(
                "wgpu buffer-mapping callback was dropped before it fired".to_string(),
            )
        })?
        .map_err(|e| GpuFrameError::DeviceLost(format!("wgpu buffer mapping failed: {e}")))?;
    // Mirrors compute::finish_map_read: staging was sized by copy_to_staging::<T> for
    // precisely `count` T's, so the mapped range's length already equals `count`; `count`
    // is taken as a parameter so the assertion below can catch a size mismatch.
    let out = {
        let data = staging.slice(..).get_mapped_range().map_err(|e| {
            GpuFrameError::DeviceLost(format!("mapped buffer range could not be read: {e}"))
        })?;
        bytemuck::cast_slice::<u8, T>(&data).to_vec()
    };
    debug_assert_eq!(
        out.len(),
        count,
        "staging buffer length must match copy_to_staging's count"
    );
    staging.unmap();
    Ok(out)
}

/// `transport_main`'s declared `@workgroup_size(64)` -- a dispatch of `n` workgroups
/// covers `n * WORKGROUP_SIZE` (pixel, sample) tuples.
const WORKGROUP_SIZE: usize = 64;

/// The lowest cross-backend guarantee for `maxComputeWorkgroupsPerDimension`
/// (WebGPU/Vulkan/D3D12's shared floor) -- `dispatch_workgroups(x, 1, 1)` is only valid
/// for `x <=` this. `transport_main` indexes purely off `global_invocation_id.x`, so every
/// dispatch here is one-dimensional and this bounds how many tuples ONE dispatch may
/// cover, independent of [`CHUNK_BUDGET_BYTES`] -- an 800x600 frame at 32 spp can exceed
/// it in a single chunk if not capped. [`chunk_pixels_for`] caps against this in addition
/// to the byte budget so a chunk is never too large to dispatch.
const MAX_WORKGROUPS_PER_DIMENSION: usize = 65_535;

/// How many pixels one dispatch covers, given a byte budget for its output buffers.
///
/// A pixel's `spp` samples always stay in one dispatch (see the module doc comment), so
/// the budget is divided by `spp` first. Always at least 1 -- a single pixel over budget
/// is still dispatched rather than looping forever on a zero-width chunk -- and never
/// more than the frame has. Also never large enough to need more than
/// [`MAX_WORKGROUPS_PER_DIMENSION`] workgroups -- see that constant's doc comment.
fn chunk_pixels_for(budget_bytes: usize, spp: u32, num_pixels: usize) -> usize {
    let budget_tuples = budget_bytes / (FLOATS_PER_TUPLE * size_of::<f32>());
    let dispatch_limited_tuples = MAX_WORKGROUPS_PER_DIMENSION * WORKGROUP_SIZE;
    let tuples_per_chunk = budget_tuples.min(dispatch_limited_tuples);
    (tuples_per_chunk / spp as usize).max(1).min(num_pixels)
}

/// [`environment_params`]'s return value: `(env_mode, temp_k, spot_mult, exposure,
/// light_yaw, light_pitch, use_d65, studio_model, backdrop)`. `use_d65` mirrors
/// `optics::raytracer::environment::sample_studio_environment_with_rig`'s own
/// `preset.uses_d65()` check -- see
/// `renderer::buffers::GpuTransportParams::studio_use_d65`'s doc comment.
type EnvironmentParams = (u32, f32, f32, f32, f32, f32, bool, u32, f32);

/// Maps an [`EnvironmentSource`] onto the megakernel's `env_mode` and its studio-rig
/// parameters -- see [`EnvironmentParams`] for the returned tuple's field meanings.
///
/// `HdrMap`'s studio-rig fields (`temp_k`/`spot_mult`/`exposure`/`light_yaw`/`light_pitch`/
/// `use_d65`/`studio_model`/`backdrop`) are unused by the shader's `env_mode == transport_env_mode::HDR_MAP` branch
/// (see `sample_environment_with_rig` in `spectral_transport.wgsl`) -- zeroed here rather
/// than left to whatever a caller might otherwise pass, so a stray read of one of them
/// during future maintenance can't silently pick up a stale studio value.
///
/// No longer fallible (see finding G6: every [`EnvironmentSource`] variant has an
/// `env_mode` now) -- returns [`EnvironmentParams`] directly rather than wrapping it in a
/// `Result` that could never be `Err`, per clippy's `unnecessary_wraps`.
/// [`GpuFrameError::UnsupportedEnvironment`] is still enforced elsewhere (kept as
/// defensive future-proofing; see that variant's own doc comment), just not by this
/// function any more.
const fn environment_params(environment: EnvironmentSource<'_>) -> EnvironmentParams {
    match environment {
        EnvironmentSource::Studio {
            preset,
            exposure,
            light_yaw,
            light_pitch,
            backdrop,
        } => (
            transport_env_mode::STUDIO_RIG,
            illuminant_temperature_k(preset),
            preset.params().spot_mult,
            exposure,
            light_yaw,
            light_pitch,
            preset.uses_d65(),
            preset.model().gpu_id(),
            backdrop,
        ),
        EnvironmentSource::HdrMap(_) => (
            transport_env_mode::HDR_MAP,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            false,
            0,
            0.0,
        ),
    }
}

/// Result of [`run_chunk_equivalence`].
pub struct ChunkEquivalenceResult {
    /// How many chunks the deliberately-small budget forced. A run where this is 1
    /// proves nothing, so [`Self::passed`] requires more.
    pub chunks_forced: usize,
    /// Pixels whose accumulated XYZ differed between the two runs, in raw bits.
    pub differing_pixels: usize,
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
}

impl ChunkEquivalenceResult {
    /// Bit-exact equality, over a run that actually chunked.
    ///
    /// Exact rather than tolerant on purpose: chunking must be a pure partition of the
    /// same threads, since each thread's output depends only on its own
    /// `(pixel, sample_num)` and nothing else. Any difference at all means
    /// `pixel_offset` is not reconstructing the global pixel index correctly, and a
    /// tolerance would hide exactly the off-by-one that would produce.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.chunks_forced > 1 && self.differing_pixels == 0
    }
}

/// Renders one frame twice -- once in a single dispatch, once forced into many small
/// chunks -- and requires the two to be bit-identical.
///
/// This is the check for `GpuTransportParams::pixel_offset`, the one piece of shader
/// logic this module added: every other GPU self-test dispatches a whole frame at once
/// and so runs with `pixel_offset == 0`, leaving the chunked path unexercised. An
/// off-by-one there would misplace camera rays and per-pixel jitter rotations by a chunk
/// boundary -- visible as a seam, but only at resolutions large enough to chunk, which is
/// exactly where nothing else was looking.
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_chunk_equivalence(renderer: &mut GpuFrameRenderer) -> ChunkEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (64u32, 64u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    let mut whole = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);
    renderer
        .accumulate(&scene, 0, spp, &mut whole)
        .expect("a cubic material under the studio rig is GPU-supported");

    // Small enough to force several chunks, and deliberately NOT a divisor of the pixel
    // count, so the last chunk is short and a boundary lands mid-row.
    let budget = 700 * FLOATS_PER_TUPLE * size_of::<f32>();
    let chunk_pixels = chunk_pixels_for(budget, spp, num_pixels);
    let chunks_forced = num_pixels.div_ceil(chunk_pixels);

    let mut chunked = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(budget);
    renderer
        .accumulate(&scene, 0, spp, &mut chunked)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in whole.iter().zip(&chunked) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    ChunkEquivalenceResult {
        chunks_forced,
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
    }
}

/// Result of [`run_pipeline_equivalence`].
pub struct PipelineEquivalenceResult {
    pub differing_pixels: usize,
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
}

impl PipelineEquivalenceResult {
    /// Bit-exact equality, exactly as [`ChunkEquivalenceResult::passed`] requires for
    /// the chunked-vs-whole-frame check -- see [`run_pipeline_equivalence`]'s own doc
    /// comment for why the megakernel and the wavefront pipeline must agree to the bit,
    /// not merely statistically.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.differing_pixels == 0
    }
}

/// Renders the same chunk through [`GpuPipelineKind::Megakernel`] and
/// [`GpuPipelineKind::Wavefront`] and requires bit-identical `out_xyz`.
///
/// Finding G5 Part B: `transport_bounce_step`/`transport_finalize_ray`
/// (`shaders/transport_bounce.wgsl`) are the SAME compiled function object either
/// pipeline calls -- see that file's own doc comment -- so a genuine divergence here
/// would mean the wavefront kernels' ray-state struct-of-arrays round-trip (load from
/// `WavefrontRayBuffers`, call the shared function, write back) lost or corrupted a
/// value the megakernel's plain local variables never would, not that the physics
/// itself differs between the two entry points.
///
/// On the same fixture [`run_chunk_equivalence`] uses (a cubic material, the studio
/// rig): deliberately small enough to run in one chunk under both pipelines, since this
/// check is about the per-ray physics, not chunking (already covered separately by
/// [`run_chunk_equivalence`]).
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_pipeline_equivalence(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);
    let mut megakernel = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut megakernel)
        .expect("a cubic material under the studio rig is GPU-supported");

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut wavefront = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut wavefront)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in megakernel.iter().zip(&wavefront) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    PipelineEquivalenceResult {
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
    }
}

/// Renders the same chunk through [`GpuPipelineKind::Wavefront`] TWICE and requires
/// bit-identical `out_xyz`.
///
/// The wavefront-pipeline counterpart of `estimator_check::run_determinism`'s
/// megakernel self-determinism check. See `shaders/wavefront_transport.wgsl`'s module
/// doc comment ("Determinism") for the
/// invariant this proves end to end: no ray's result depends on scheduling, which
/// workgroup it lands in, or the order `wavefront_compact_*` processes rays in -- so two
/// runs against byte-identical input must agree to the bit, exactly as the megakernel's
/// own no-cross-thread-communication guarantee already does.
///
/// # Panics
///
/// Same conditions as [`run_pipeline_equivalence`]'s.
#[must_use]
pub fn run_wavefront_determinism(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut first = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut first)
        .expect("a cubic material under the studio rig is GPU-supported");
    let mut second = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut second)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in first.iter().zip(&second) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    PipelineEquivalenceResult {
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
    }
}

/// One material's result within [`SpecialisationEquivalenceResult`].
///
/// # Why this is self-determinism, not GENERIC-vs-specialised bit-identity
///
/// It might seem that forcing `is_anisotropic`/`is_biaxial` to a value already matching
/// the material's own runtime flags could not change what is computed, so GENERIC and
/// specialised should be byte-identical. Measured on the real AMD Radeon (Vulkan)
/// adapter this crate targets, that is FALSE for the isotropic pipeline -- see the
/// module doc's "Material-class kernel specialisation" section for why (dead-code
/// elimination changes register pressure/scheduling enough to flip a stochastic branch
/// by 1 ULP). Nothing guarantees a driver keeps any particular case bit-identical, so
/// this check does not rely on it; the rigorous GENERIC-vs-specialised correctness gate
/// is `estimator_check::run_specialisation_image_comparison` instead.
///
/// What GPU dispatch determinism DOES guarantee -- and what [`Self::passed`] gates on --
/// is that the SAME compiled pipeline, dispatched twice against identical input,
/// produces byte-identical output: no thread ever reads another thread's output, so
/// scheduling order can never matter WITHIN one pipeline. A specialised pipeline
/// failing that would mean dead-code elimination left something genuinely broken, not
/// merely a differently-scheduled but internally-consistent kernel.
#[derive(Debug, Clone)]
pub struct SpecialisationCaseResult {
    pub material_name: String,
    /// Which [`material_class`] value [`classify_material`] picked for this material --
    /// the specialised pipeline actually under test.
    pub material_class: u32,
    /// Pixels that differed between two dispatches of the SAME specialised pipeline
    /// against identical input -- see this struct's own doc comment. Must be 0 for
    /// [`Self::passed`].
    pub self_determinism_differing_pixels: usize,
    /// Diagnostic only, NOT part of [`Self::passed`]: how many pixels differed between
    /// the GENERIC pipeline and the specialised one, and the largest per-component
    /// difference seen. The rigorous pass/fail gate for this comparison is
    /// `estimator_check::run_specialisation_image_comparison`.
    pub generic_vs_specialised_differing_pixels: usize,
    pub total_pixels: usize,
    pub generic_vs_specialised_max_abs_diff: f32,
}

impl SpecialisationCaseResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.self_determinism_differing_pixels == 0
    }
}

/// Result of [`run_specialisation_equivalence`].
pub struct SpecialisationEquivalenceResult {
    pub cases: Vec<SpecialisationCaseResult>,
}

impl SpecialisationEquivalenceResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(SpecialisationCaseResult::passed)
    }
}

/// For each of one representative isotropic, uniaxial, and biaxial built-in material,
/// checks GPU dispatch determinism and records a diagnostic diff count.
///
/// Dispatches that material's specialised pipeline TWICE against identical input (must
/// be byte-identical -- GPU dispatch determinism, see [`SpecialisationCaseResult`]'s doc
/// comment for why that is the right invariant, not GENERIC-vs-specialised bit-identity),
/// and additionally records how many pixels differ against a GENERIC dispatch, purely as
/// a diagnostic.
///
/// Diamond stands in for isotropic, Zircon for uniaxial (largest built-in birefringence),
/// and Alexandrite for biaxial (a populated `beta_ray` band set, exercising the
/// biaxial-only pleochroic absorption path too).
///
/// Uses [`GpuFrameRenderer::accumulate_via_pipeline`] rather than an ad hoc dispatch
/// routine, so this check exercises the SAME chunking/bind/dispatch code
/// [`GpuFrameRenderer::accumulate`] ships, differing only in which pipeline is forced.
///
/// # Panics
///
/// Panics if `"Diamond"`, `"Zircon"`, or `"Alexandrite"` is ever removed from
/// [`GemMaterial::all_materials`] -- self-test scaffolding, not a code path a real
/// caller can reach with a name that might legitimately be missing.
#[must_use]
pub fn run_specialisation_equivalence(
    renderer: &mut GpuFrameRenderer,
) -> SpecialisationEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 3u32;
    let num_pixels = (width * height) as usize;
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    let representative_materials = ["Diamond", "Zircon", "Alexandrite"];

    let mut cases = Vec::with_capacity(representative_materials.len());
    for name in representative_materials {
        let material = GemMaterial::by_name(name).unwrap_or_else(|| {
            panic!("{name:?} is a built-in material in GemMaterial::all_materials()")
        });
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 8,
            environment,
        };
        let class = classify_material(&material);

        // Self-determinism: the SAME specialised pipeline, dispatched twice against
        // identical input -- must be byte-identical.
        let mut run1 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run1, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut run2 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run2, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let self_determinism_differing_pixels = run1
            .iter()
            .zip(&run2)
            .filter(|(a, b)| {
                a.to_array()
                    .iter()
                    .zip(b.to_array().iter())
                    .any(|(x, y)| x.to_bits() != y.to_bits())
            })
            .count();

        // Diagnostic only: GENERIC vs specialised, same input -- see
        // SpecialisationCaseResult's doc comment for why this is NOT required to be zero.
        let mut generic = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, material_class::GENERIC, 0, spp, &mut generic, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut generic_vs_specialised_differing_pixels = 0usize;
        let mut generic_vs_specialised_max_abs_diff = 0.0f32;
        for (a, b) in generic.iter().zip(&run1) {
            if a.to_array()
                .iter()
                .zip(b.to_array().iter())
                .any(|(x, y)| x.to_bits() != y.to_bits())
            {
                generic_vs_specialised_differing_pixels += 1;
                generic_vs_specialised_max_abs_diff =
                    generic_vs_specialised_max_abs_diff.max((*a - *b).abs().max_element());
            }
        }

        cases.push(SpecialisationCaseResult {
            material_name: name.to_string(),
            material_class: class,
            self_determinism_differing_pixels,
            generic_vs_specialised_differing_pixels,
            total_pixels: num_pixels,
            generic_vs_specialised_max_abs_diff,
        });
    }

    SpecialisationEquivalenceResult { cases }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`staging_needs_growth`] must say "grow" exactly when the current capacity is too
    /// small, and "keep" both when it matches and when strictly larger (never shrink),
    /// mirroring [`GpuFrameRenderer::ensure_capacity`]'s identical check for `outputs`.
    #[test]
    fn staging_needs_growth_only_when_undersized() {
        // Too small: must grow.
        assert!(staging_needs_growth(0, 1));
        assert!(staging_needs_growth(99, 100));
        // Exactly enough: must NOT grow.
        assert!(!staging_needs_growth(100, 100));
        // Already larger than required: must NOT shrink either.
        assert!(!staging_needs_growth(1_000, 100));
    }

    #[test]
    fn chunking_divides_the_budget_by_samples_per_pixel() {
        // 12 bytes per tuple (XYZ only), so a 120-byte budget is exactly 10 tuples: 10
        // pixels at 1 spp, 5 at 2 spp, 3 at 3 spp (integer division, never rounding up).
        let budget = 10 * FLOATS_PER_TUPLE * size_of::<f32>();
        assert_eq!(chunk_pixels_for(budget, 1, 10_000), 10);
        assert_eq!(chunk_pixels_for(budget, 2, 10_000), 5);
        assert_eq!(chunk_pixels_for(budget, 3, 10_000), 3);
    }

    #[test]
    fn a_frame_smaller_than_the_budget_is_one_chunk() {
        let pixels = 800 * 600;
        assert_eq!(chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, pixels), pixels);
    }

    /// A budget too small for even one tuple must still dispatch one pixel rather than
    /// returning zero, which would loop forever on a zero-width chunk.
    #[test]
    fn a_budget_below_one_tuple_still_yields_a_chunk() {
        assert_eq!(chunk_pixels_for(0, 4, 10_000), 1);
        assert_eq!(chunk_pixels_for(8, 1, 10_000), 1);
    }

    /// With no measurement yet (`ema = None`), the first chunk(s) of a fresh renderer
    /// must be capped at [`FIRST_DISPATCH_MAX_TUPLES`], never the full byte budget -- a
    /// cold integrated GPU's first dispatch must not alone trip a TDR watchdog.
    #[test]
    fn first_dispatch_is_capped_regardless_of_byte_budget() {
        let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 4, 10_000_000);
        let picked = GpuFrameRenderer::next_chunk_pixels(None, byte_budget_pixels, 4, 10_000_000);
        assert!(picked <= FIRST_DISPATCH_MAX_TUPLES / 4);
        assert!(picked <= byte_budget_pixels);
    }

    /// Once a measurement exists, a fast-GPU EMA (tiny ns/tuple) must still never exceed
    /// the byte-budget ceiling -- `chunk_budget_bytes` stays a hard upper bound.
    #[test]
    fn time_budgeted_sizing_never_exceeds_the_byte_budget() {
        let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, 10_000_000);
        let picked = GpuFrameRenderer::next_chunk_pixels(
            Some(1.0e-6), // absurdly fast: 1 tuple per picosecond
            byte_budget_pixels,
            1,
            10_000_000,
        );
        assert!(picked <= byte_budget_pixels);
    }

    /// A slow-GPU EMA (large ns/tuple) must still dispatch at least the [`MIN_CHUNK_BYTES`]
    /// floor's worth of pixels (clamped to the byte budget) rather than shrinking toward
    /// zero, so fixed per-dispatch overhead can never dominate.
    #[test]
    fn time_budgeted_sizing_never_shrinks_below_the_floor() {
        let num_pixels = 10_000_000;
        let byte_budget_pixels = chunk_pixels_for(CHUNK_BUDGET_BYTES, 1, num_pixels);
        let min_pixels = chunk_pixels_for(MIN_CHUNK_BYTES, 1, num_pixels);
        let picked = GpuFrameRenderer::next_chunk_pixels(
            Some(1.0e9), // absurdly slow: 1 second per tuple
            byte_budget_pixels,
            1,
            num_pixels,
        );
        assert_eq!(picked, min_pixels);
    }

    /// A forced byte budget SMALLER than [`MIN_CHUNK_BYTES`] (as
    /// [`run_chunk_equivalence`] uses to force many small chunks) must still win: the
    /// byte budget is the one bound that can never be exceeded, even when it is below
    /// the timing floor that would otherwise apply.
    #[test]
    fn a_byte_budget_smaller_than_the_timing_floor_still_wins() {
        let num_pixels = 10_000;
        let tiny_budget_bytes = 700 * FLOATS_PER_TUPLE * size_of::<f32>();
        let byte_budget_pixels = chunk_pixels_for(tiny_budget_bytes, 2, num_pixels);
        for ema in [Some(1.0e-6), Some(1.0e9), None] {
            let picked =
                GpuFrameRenderer::next_chunk_pixels(ema, byte_budget_pixels, 2, num_pixels);
            assert!(
                picked <= byte_budget_pixels,
                "byte budget must never be exceeded (ema={ema:?}, picked={picked}, \
                 byte_budget_pixels={byte_budget_pixels})"
            );
        }
    }

    #[test]
    fn studio_environments_map_onto_the_studio_rig_mode() {
        let (mode, temp_k, spot_mult, exposure, yaw, pitch, use_d65, studio_model, backdrop) =
            environment_params(LightingPreset::Daylight.studio(1.5, 0.4, 0.35));
        assert_eq!(mode, transport_env_mode::STUDIO_RIG);
        assert_eq!(temp_k, illuminant_temperature_k(LightingPreset::Daylight));
        assert_eq!(spot_mult, LightingPreset::Daylight.params().spot_mult);
        assert_eq!((exposure, yaw, pitch), (1.5, 0.4, 0.35));
        // The Daylight preset must route through the D65 table on the GPU too -- see
        // GpuTransportParams::studio_use_d65's doc comment.
        assert!(use_d65);
        assert_eq!(studio_model, 0);
        assert_eq!(backdrop, 0.0);
    }

    /// Finding G6: an HDR map is a SUPPORTED environment (`env_mode ==
    /// transport_env_mode::HDR_MAP`), not a decline -- the studio-rig fields are simply
    /// unused by that branch (see [`environment_params`]'s own doc comment), not left at
    /// some other environment's stale values.
    #[test]
    fn hdr_environments_map_onto_the_hdr_map_mode() {
        let map = crate::renderer::env_map::EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]);
        let (mode, temp_k, spot_mult, exposure, yaw, pitch, use_d65, studio_model, backdrop) =
            environment_params(EnvironmentSource::HdrMap(&map));
        assert_eq!(mode, transport_env_mode::HDR_MAP);
        assert_eq!(
            (temp_k, spot_mult, exposure, yaw, pitch),
            (0.0, 0.0, 0.0, 0.0, 0.0)
        );
        assert!(!use_d65);
        assert_eq!(studio_model, 0);
        assert_eq!(backdrop, 0.0);
    }

    /// This module must ENFORCE `GemMaterial::gpu_supported`, not merely document it --
    /// routing a material the megakernel cannot handle produces a plausible-looking but
    /// wrong image rather than a failure, the worst kind of bug to ship.
    ///
    /// `gpu_supported` is unconditionally `true` today: every built-in, including the
    /// biaxial stones (Alexandrite, Topaz, Tanzanite), is ported and verified at 0 ULP.
    /// This test's real job is not "biaxial is special" but that **this module agrees
    /// with the predicate**, whatever it currently says -- a future material type the
    /// megakernel cannot handle would flip the predicate and fail this test.
    #[test]
    fn every_builtin_routes_the_way_gpu_supported_says() {
        for material in GemMaterial::all_materials() {
            assert!(
                material.gpu_supported(),
                "{} is not GPU-supported, but the megakernel claims to cover every                  built-in since the Phase 4 biaxial port -- if this is a deliberate new                  exclusion, `accumulate`'s decline path must cover it too",
                material.name
            );
        }
    }
}

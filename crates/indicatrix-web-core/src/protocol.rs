//! The messages between the page and its Web Workers.
//!
//! Every message is a [`ToWorker`] or [`FromWorker`] encoded with postcard
//! ([`encode_to_worker`] / [`decode_from_worker`] and their mirrors) and posted as a
//! TRANSFERRED `ArrayBuffer`, so no copy is made on the way.
//!
//! # Handshake
//!
//! 1. A Worker starts, installs its `onmessage` handler and posts
//!    [`FromWorker::Loaded`] (messages posted to a Worker before its handler exists
//!    would be lost, so the host waits for this).
//! 2. The host sends [`ToWorker::Init`] with its [`PROTOCOL_VERSION`] and the Worker's
//!    role and index.
//! 3. The Worker checks the version and answers [`FromWorker::Ready`], or
//!    [`FromWorker::Error`] on a mismatch (a stale cached Worker script next to a new
//!    page, say).
//!
//! # Render Workers
//!
//! [`ToWorker::HdrMap`] and [`ToWorker::SetScene`] (or [`ToWorker::SetGeometryScene`], the
//! same scene with concave tools; build either with [`ToWorker::scene`]) are cached by the
//! Worker;
//! [`ToWorker::TraceChunk`] traces against the cached scene and answers
//! [`FromWorker::ChunkResult`], or [`FromWorker::ChunkDropped`] when the Worker has no
//! scene with that id (so the host never waits for a chunk that will not come). The
//! host drops results whose `scene_id` is not current.
//!
//! A chunk is traced in row groups (finer interleaved partitions). The host may name a
//! `blob:` URL for it with [`ToWorker::WatchCancel`] (`job_id` = the chunk's scene id)
//! just before the [`ToWorker::TraceChunk`], and revoke the URL when the chunk is no
//! longer wanted (the scene was replaced); the Worker looks at it between row groups and
//! answers [`FromWorker::ChunkAborted`] instead of finishing.
//!
//! [`ToWorker::Picture`] turns a finished (or settled) sum of the cached scene into
//! pixels off the page's thread -- the settled live view's denoise, or an export's
//! tone map + PNG encode ([`PictureKind`]) -- and answers [`FromWorker::Picture`] or
//! [`FromWorker::PictureFailed`]. The host keeps these on a Worker of their own, so a
//! picture never takes a render partition away from tracing.
//!
//! # The solve Worker
//!
//! [`ToWorker::Solve`] answers [`FromWorker::SolveResult`]. An Optimize or Retarget
//! search, and a tilt sweep, stream [`FromWorker::Progress`] messages (at most five a
//! second) before their result. The current-view metrics job is short and streams
//! nothing.
//!
//! The analysis Worker also takes [`ToWorker::HdrMap`] and [`ToWorker::ClearHdr`], the
//! same messages the render Workers get, and answers [`FromWorker::HdrLoaded`] or
//! [`FromWorker::HdrError`]. A metrics job whose `hdr_id` names the map the Worker holds
//! is scored under it; any other is scored under its lighting preset, and the result's
//! `scored_under` says which it was. The tilt sweep and the searches are scored under the
//! preset, as on the desktop.
//!
//! A Worker cannot read a message while it computes, so [`ToWorker::Cancel`] only stops
//! jobs still queued behind the running one (and is what a build that can deliver
//! messages mid-job would poll; see [`crate::worker::WorkerHandler`]). A running search
//! is cancelled cooperatively through [`ToWorker::WatchCancel`]'s URL, which the Worker
//! polls between tier decisions and which answers with the best partial result; the
//! host terminates the Worker only when that does not answer in time. A browser that
//! refuses the synchronous request the poll makes (a strict content-security policy) is
//! told apart from a revoked URL: the job then simply runs on, and is ended by the
//! host's own deadline.

use indicatrix::geometry::tool::ToolPrimitive;
use serde::{Deserialize, Serialize};

use crate::{
    scene::SceneSpec,
    solve::{SolveRequest, SolveResponse},
};

/// Bumped whenever a message's encoding changes; checked on [`ToWorker::Init`].
///
/// 2: custom materials travel as `GemMaterial`s; `Picture` / `PictureFailed` added.
/// 3: `SolveRequest::{Optimize, Retarget}` and their `SolveResponse` payloads;
/// `Progress` streams a running search's stage line.
/// 4: `ToWorker::WatchCancel`, the cooperative-cancel probe of a running search.
/// 5: `SolveRequest::{Metrics, Tilt}` and their `SolveResponse` payloads; `Progress`
/// streams a running tilt sweep's stage line.
/// 6: `FromWorker::ChunkAborted`; `WatchCancel` also names a render chunk's abort URL.
/// 7: `OptimizeParams::lighting_preset_index`, the preset the optimizer scores under.
/// 8: the solve role accepts `HdrMap` / `ClearHdr`; `MetricsParams::hdr_id` names the map
/// the current-view metrics are scored under and `MetricsResultData::scored_under` says
/// what they were scored under.
/// 9: `CustomMaterialSpec::color_recipe`, the physics color recipe a custom material
/// may carry (rendered from its stored resolved bands).
/// 10: `RetargetParams::crown_follows_pavilion`, the crown policy that keeps the stone's
/// silhouette (postcard has no field defaults, so the new field is a wire change).
/// 11: `LightingSpec::head_shadow_deg` appended (the viewer's head-shadow radius), and
/// the lighting combo reordered (`LightingPreset::index` now lists the lit presets first),
/// so the preset index means something else than under 10.
/// 12: the seven-band body colour of the path-aware L*C*h editor: `MaterialSelectionData` and
/// `DesignMaterialOverrides` gain `body_color_bands` and `absorption_path_scale_override`,
/// `CustomMaterialSpec` gains `absorption_bands` (postcard has no field defaults, so each is a
/// wire change; an empty list and `None` are one byte each), and the solve role accepts
/// `SolveRequest::BodyColor` (answered by `SolveResponse::BodyColor`), the editor's colour solve.
/// 13: lighting wave. Six presets appended (`LightingPreset::{DaylightSun, Aset, ShopLights,
/// WindowDaylight, WhiteTray, IlluminantA}`) and the lighting combo reordered, so
/// `LightingPreset::index` (carried by `LightingSpec`) means something else than under 12.
///
/// 14: multi-start optimizer. `OptimizeParams::starts` appended (a `u32` varint, one byte
/// for the web's 1; postcard has no field defaults, so it is a wire change).
///
/// 15: absorption units. `indicatrix::optics::materials::GemMaterial` (carried by
/// `MaterialSpec::custom_materials`) gained `absorption_unit` (`AbsorptionUnit`, enum index
/// 0 `ModelUnit` / 1 `PerMm`, one byte, after `absorption_path_scale`): the same stone size
/// now scales per-model-unit and per-millimetre absorption differently, and inclusion
/// scattering no longer scales with the size, so a Worker on 14 would render those scenes
/// with the old rule.
///
/// 16: `CustomMaterialSpec` carries the crystal system, optical character and biaxial delta
/// (`crystal_system` and `optical_character` as strings, `biaxial_delta_beta_alpha` as an
/// `Option<f64>`, appended after `absorption_bands`; postcard has no field defaults, so it is
/// a wire change): a biaxial custom material restored from a design file keeps its optics.
///
/// 17: `LightingSpec::surface_glare` appended (an `f32`, `00 00 80 3F` for the default 1.0;
/// postcard has no field defaults, so it is a wire change): the cross-polarised scale of the
/// first surface reflection that the render settings' "Surface Glare" slider dials in. The
/// export color-space index also gained 2 = Rec.2020 (`PictureKind::Png::color_space`, no new
/// bytes, but a Worker on 16 would write that export as sRGB).
///
/// 18: Web GPU rendering support: `WorkerRole::Gpu`, `ToWorker::GpuCompile`,
/// `ToWorker::TraceFullPass`, `FromWorker::GpuAdapter`, `FromWorker::GpuUnavailable`,
/// `FromWorker::GpuCompiled`, `FromWorker::GpuCompileFailed`, `FromWorker::FullPassResult`,
/// and `FromWorker::GpuLost`.
///
/// 19: meet-preserving retarget. `RetargetResultData` gains `anchors` (the checked masts,
/// `Vec<AnchorChangeData>`) and `validity_headline` (a `String`), because a Shift retarget of a
/// design with pinned meets now runs the Worker-side check and the page applies the angles
/// together with the masts; and `OptimizationProvenance::diagnostics` is no longer serialised
/// (`#[serde(skip)]`), which shortens every `OptimizeResultData` that carries a provenance.
/// Postcard has no field defaults, so both are wire changes.
///
/// 20: concave stones in the browser. `ToWorker::SetGeometryScene` (a scene with its cutting
/// tools, appended last) and `SolveRequest::{MetricsWithTools, TiltWithTools}` (the analysis
/// Worker's metrics and tilt jobs scored with the tools, appended last). All three are new
/// variants, so no existing message's bytes moved: a planar stone still sends `SetScene`,
/// `Metrics` and `Tilt` exactly as under 19 (see `ToWorker::scene`, `SolveRequest::metrics`,
/// `SolveRequest::tilt`). The bump makes a stale cached worker script fail the `Init`
/// handshake instead of failing to decode a new message.
///
/// 21: frosted tilt curves. `TiltParams` gains `finishes` (a `FinishSpec`, appended last; one
/// byte for `AllPolished`, postcard has no field defaults, so it is a wire change for `Tilt`
/// and `TiltWithTools`): the tilt sweep scores the stone with its frosted facets, as the
/// desktop's Tilt Performance dialog does (owner decision 2026-10-19; a frosted facet scatters
/// light, so a frosted design's curves are not its polished ones). A Worker on 20 would sweep
/// every stone polished.
///
/// 22: turn-aware Retarget bounds. `indicatrix_cut_core::meet_preservation::PreservationPolicy`
/// (inside `OptimizationProvenance::policy`, carried by `OptimizeResultData::provenance`)
/// gained `bounds_follow_turn` (a `bool`, one byte, declared last; postcard has no field
/// defaults, so every provenance on the wire is one byte longer), and the policy's meaning
/// changed with it: a Retarget's depth, girdle and junction bounds now grow by the travel the
/// REQUESTED facet turn implies (owner decisions 2026-10-19), and a provenance is re-validated
/// under its policy on Apply. A Worker on 21 would misalign the provenance and refuse a valid
/// turn as a shape limit.
///
/// A change to any message's bytes must bump this and update the pinned bytes in this
/// module's `wire_format_is_pinned_to_worker` and `wire_format_is_pinned_from_worker` tests.
/// 23: `SetEngine` appends renderer selection beside the unchanged scene payload.
///
/// 24: the backdrop is a lit tray. No shape change (`LightingSpec::backdrop_index` is still an
/// `i32`, now 0 as lit, 1 grey, 2 white, 3 black), but the meaning moved: the backdrop is a
/// matte surface of that albedo under the stone, lit by the preset at the light pose and seen by
/// every ray that escapes the stone, not a card only the camera ray sees. A Worker on 23 would
/// render dark windows beside a white surround and merge nothing consistent with the page's
/// other peers. The net protocol's `SceneState::backdrop` moved with it (net 29).
pub const PROTOCOL_VERSION: u32 = 24;

/// What [`ToWorker::Picture`] makes of a sum (see `crate::display`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PictureKind {
    /// The settled live view: `denoise_and_tonemap_frame`, RGBA8.
    DenoisedLive,
    /// A finished export: PNG bytes (`display::export_png`).
    Png {
        /// `display::EXPORT_COLOR_SPACES` index.
        color_space: i32,
        /// Filter with the live view's denoiser first.
        denoise: bool,
    },
}

/// What a Worker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerRole {
    /// Traces render chunks.
    Render,
    /// Runs solve jobs.
    Solve,
    /// Traces GPU passes and compiles GPU pipelines.
    Gpu,
}

/// Page to Worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToWorker {
    /// Assigns the role; see the module's "Handshake".
    Init {
        /// The host's [`PROTOCOL_VERSION`].
        protocol_version: u32,
        /// The role.
        role: WorkerRole,
        /// The Worker's index in its pool (a render Worker's partition).
        worker_index: u32,
    },
    /// Replaces the cached scene.
    SetScene {
        /// The scene's id; results carry it back.
        scene_id: u64,
        /// The scene.
        spec: SceneSpec,
    },
    /// Decodes and caches an HDR map (replacing any previous one), on a render Worker or
    /// a solve-role Worker (the analysis Worker scores the metrics under it).
    HdrMap {
        /// The map's id, named by `SceneSpec::hdr_id` and `MetricsParams::hdr_id`.
        id: u64,
        /// The `.hdr` file's bytes.
        bytes: Vec<u8>,
    },
    /// Frees the cached HDR map.
    ClearHdr,
    /// Traces one chunk of the cached scene.
    TraceChunk {
        /// The scene the host means; a mismatch is dropped.
        scene_id: u64,
        /// The partition (first pixel).
        first_pixel: u32,
        /// The partition count.
        stride: u32,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
    },
    /// Runs a solve job.
    Solve {
        /// The job's id; the result carries it back.
        job_id: u64,
        /// The design as a self-contained native file (`solve::design_to_toml`).
        design_toml: String,
        /// What to do.
        request: SolveRequest,
    },
    /// Cancels every solve job with an id up to and including `job_id`: queued jobs are
    /// skipped, and the Worker's message-cancel mark is raised, which a running job
    /// polls when it cannot probe its cancel URL (see [`crate::worker::WorkerHandler`]).
    /// The host sends it with every graceful cancel of a solve-role job.
    Cancel {
        /// The newest job id to cancel.
        job_id: u64,
    },
    /// Makes pixels from a sum of the cached scene (render Workers).
    Picture {
        /// The scene the sum belongs to; a mismatch fails.
        scene_id: u64,
        /// Samples per pixel in `sums`.
        sample_count: u32,
        /// The full-frame running sum, row-major.
        sums: Vec<[f32; 3]>,
        /// What to make.
        kind: PictureKind,
    },
    /// Names the URL a Worker polls, WHILE it computes job `job_id`, to learn that the
    /// page cancelled the job (sent just before the job's [`ToWorker::Solve`], or, on a
    /// render Worker, just before the [`ToWorker::TraceChunk`] of scene `job_id`).
    ///
    /// A Worker cannot read a message while it computes, so this is a side channel: the
    /// page hands over a `blob:` URL and revokes it to cancel; the Worker's poll of the
    /// URL then fails, its search stops between tier decisions (a chunk between row
    /// groups) and answers with the best result so far. See
    /// [`crate::worker::WorkerHandler::handle_probed`].
    WatchCancel {
        /// The job the URL belongs to (a render chunk's scene id); any other job
        /// ignores it.
        job_id: u64,
        /// The URL to poll.
        url: String,
    },
    /// Compiles GPU kernels in order and reports each.
    GpuCompile {
        /// Material class (0 generic, 1 isotropic, 2 uniaxial, 3 biaxial).
        class: u32,
        /// Kernel names to compile in order.
        kernels: Vec<String>,
        /// Soft deadline in milliseconds (stop compiling new kernels if breached).
        soft_ms: u32,
        /// Hard deadline in milliseconds (terminate worker if breached).
        hard_ms: u32,
    },
    /// Traces one full pass of the cached scene on the GPU.
    TraceFullPass {
        /// The scene the host means; a mismatch is dropped.
        scene_id: u64,
        /// The pass id.
        pass_id: u64,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
    },
    /// [`Self::SetScene`] for a concave stone: the scene is `spec`'s planes minus `tools`.
    ///
    /// Sent instead of `SetScene` only when there are tools, so a planar scene's bytes are
    /// what they were before concave stones reached the browser's Render tab (build one with
    /// [`Self::scene`]). Appended as the last variant, which leaves every other message's
    /// bytes unchanged. A render Worker builds it with
    /// [`GeometrySceneSpec::build`](crate::scene::GeometrySceneSpec::build), which refuses
    /// an invalid tool or more than `MAX_TOOL_PRIMITIVES`; the GPU Worker additionally
    /// declines more than `GPU_MAX_TOOL_PRIMITIVES` for itself.
    SetGeometryScene {
        /// The scene's id; results carry it back.
        scene_id: u64,
        /// The scene's planes, material, camera, lighting and frame.
        spec: SceneSpec,
        /// The resolved cutting tools, in cutting order (never empty on the wire).
        tools: Vec<ToolPrimitive>,
    },
    /// Selects the renderer of the cached scene without changing `SceneSpec`'s bytes.
    SetEngine {
        /// The scene the renderer choice belongs to.
        scene_id: u64,
        /// Renderer and deterministic AA options.
        engine: indicatrix::renderer::engine::EngineSpec,
    },
}

impl ToWorker {
    /// The message that gives a Worker the scene `spec` minus `tools`: [`Self::SetScene`] when
    /// there are no tools (byte-identical to before concave stones), else
    /// [`Self::SetGeometryScene`].
    #[must_use]
    pub fn scene(scene_id: u64, spec: SceneSpec, tools: &[ToolPrimitive]) -> Self {
        if tools.is_empty() {
            Self::SetScene { scene_id, spec }
        } else {
            Self::SetGeometryScene {
                scene_id,
                spec,
                tools: tools.to_vec(),
            }
        }
    }
}

/// Worker to page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "the postcard wire format pins variant order and payload layout (protocol/tests.rs); \
              `SolveResult` carries one finished answer by value and a message is built once per \
              job, so boxing it would only move the allocation and change every consumer"
)]
pub enum FromWorker {
    /// The Worker's handler is installed; send `Init`.
    Loaded {
        /// The Worker's own [`PROTOCOL_VERSION`].
        protocol_version: u32,
    },
    /// `Init` accepted.
    Ready {
        /// The role taken.
        role: WorkerRole,
        /// The index given.
        worker_index: u32,
    },
    /// A traced chunk.
    ChunkResult {
        /// The scene it was traced for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// The partition count.
        stride: u32,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
        /// One summed XYZ radiance per partition pixel, in pixel order.
        sums: Vec<[f32; 3]>,
        /// Tracing time measured in the Worker (`performance.now()`).
        elapsed_ms: f64,
    },
    /// A chunk the Worker could not trace (no scene, or a different one).
    ChunkDropped {
        /// The scene the chunk asked for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// First sample index.
        sample_offset: u32,
    },
    /// The scene could not be built; chunks for it will be dropped.
    SceneError {
        /// The scene.
        scene_id: u64,
        /// Why.
        message: String,
    },
    /// An HDR map is decoded and cached.
    HdrLoaded {
        /// The map.
        id: u64,
        /// Width in texels.
        width: u32,
        /// Height in texels.
        height: u32,
    },
    /// An HDR map failed to decode.
    HdrError {
        /// The map.
        id: u64,
        /// Why.
        message: String,
    },
    /// Progress of a long job (an Optimize or Retarget search, or a tilt sweep).
    Progress {
        /// The job.
        job_id: u64,
        /// Human-readable stage.
        message: String,
        /// Completion fraction when known.
        fraction: Option<f32>,
    },
    /// A finished solve job.
    SolveResult {
        /// The job.
        job_id: u64,
        /// The answer.
        response: SolveResponse,
        /// Time the job took in the Worker.
        elapsed_ms: f64,
    },
    /// Anything else that went wrong (bad message, version mismatch, wrong role).
    Error {
        /// Why.
        message: String,
    },
    /// A finished [`ToWorker::Picture`].
    Picture {
        /// The scene.
        scene_id: u64,
        /// Samples per pixel it was made from.
        sample_count: u32,
        /// What was made.
        kind: PictureKind,
        /// RGBA8 for [`PictureKind::DenoisedLive`], PNG file bytes for
        /// [`PictureKind::Png`].
        bytes: Vec<u8>,
        /// Time it took in the Worker.
        elapsed_ms: f64,
    },
    /// A [`ToWorker::Picture`] that could not be made.
    PictureFailed {
        /// The scene.
        scene_id: u64,
        /// What was asked for.
        kind: PictureKind,
        /// Why.
        message: String,
    },
    /// A chunk the Worker stopped before it was done, because the page revoked its
    /// cancel URL (see [`ToWorker::WatchCancel`]). The host revokes a chunk's URL only
    /// once it has replaced the chunk's scene, so the partition has no result and none
    /// is wanted; the Worker is free for the new scene at once.
    ChunkAborted {
        /// The scene the chunk was traced for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// First sample index.
        sample_offset: u32,
    },
    /// Reports adapter limits and capabilities after GPU worker initialisation.
    GpuAdapter {
        /// Adapter vendor name.
        vendor: String,
        /// Architecture name.
        architecture: String,
        /// Device name.
        device: String,
        /// Human-readable description.
        description: String,
        /// Backend name (e.g. Vulkan, Metal, D3D12).
        backend: String,
        /// Maximum storage buffers per stage.
        max_storage_buffers_per_stage: u32,
        /// Maximum storage buffer binding size in bytes.
        max_storage_binding: u64,
        /// Maximum buffer size in bytes.
        max_buffer: u64,
    },
    /// WebGPU is unavailable or does not satisfy minimum requirements.
    GpuUnavailable {
        /// Why WebGPU could not be used.
        reason: String,
    },
    /// A GPU kernel pipeline compiled successfully.
    GpuCompiled {
        /// Kernel name.
        kernel: String,
        /// Material class.
        class: u32,
        /// Compilation time in milliseconds.
        ms: f64,
    },
    /// A GPU kernel pipeline failed to compile or exceeded budget.
    GpuCompileFailed {
        /// Kernel name.
        kernel: String,
        /// Material class.
        class: u32,
        /// Failure reason.
        reason: String,
        /// Compilation time in milliseconds before failure/timeout.
        ms: f64,
    },
    /// A finished full-frame GPU render pass.
    FullPassResult {
        /// The scene it was traced for.
        scene_id: u64,
        /// The pass id.
        pass_id: u64,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
        /// One summed XYZ radiance per pixel of the entire frame.
        sums: Vec<[f32; 3]>,
        /// Tracing time measured in the Worker (`performance.now()`).
        elapsed_ms: f64,
    },
    /// Device lost or unrecoverable GPU error.
    GpuLost {
        /// Reason for device loss.
        reason: String,
    },
}

/// Encodes a page-to-Worker message.
///
/// # Errors
///
/// postcard's error text (in practice unreachable for these types).
pub fn encode_to_worker(message: &ToWorker) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(message).map_err(|e| format!("encoding a worker message: {e}"))
}

/// Decodes a page-to-Worker message.
///
/// # Errors
///
/// When the bytes are not a valid [`ToWorker`].
pub fn decode_to_worker(bytes: &[u8]) -> Result<ToWorker, String> {
    postcard::from_bytes(bytes).map_err(|e| format!("decoding a worker message: {e}"))
}

/// Encodes a Worker-to-page message.
///
/// # Errors
///
/// postcard's error text (in practice unreachable for these types).
pub fn encode_from_worker(message: &FromWorker) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(message).map_err(|e| format!("encoding a worker reply: {e}"))
}

/// Decodes a Worker-to-page message.
///
/// # Errors
///
/// When the bytes are not a valid [`FromWorker`].
pub fn decode_from_worker(bytes: &[u8]) -> Result<FromWorker, String> {
    postcard::from_bytes(bytes).map_err(|e| format!("decoding a worker reply: {e}"))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod geometry_tests;

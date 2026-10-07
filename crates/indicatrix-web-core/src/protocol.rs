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
//! [`ToWorker::HdrMap`] and [`ToWorker::SetScene`] are cached by the Worker;
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
/// A change to any message's bytes must bump this and update the pinned bytes in this
/// module's `wire_format_is_pinned` test.
pub const PROTOCOL_VERSION: u32 = 14;

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
}

/// Worker to page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

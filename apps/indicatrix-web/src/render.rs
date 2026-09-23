//! Renders through `indicatrix`'s WebGPU compute path -- and only that path -- in
//! successive, presentable chunks rather than one shot.
//!
//! # This build is GPU-only, deliberately, with no CPU fallback
//!
//! Every other caller of `indicatrix`'s GPU renderer in this workspace goes through
//! `indicatrix::renderer::gpu_backend::GpuBackend`, whose contract is "try the GPU,
//! silently fall back to the CPU tracer whenever it declines". This crate calls
//! [`GpuFrameRenderer::new_async`]/`accumulate_async` directly instead, with no
//! fallback: `wasm32-unknown-unknown` has no OS thread for a CPU tracer to parallelize
//! across, and a single-threaded software ray tracer in a browser tab would be worse
//! than a clear "this browser can't run indicatrix-web" message. [`RenderError`] is how
//! that shows up instead of a silent hang or blank canvas: `app.rs` turns every variant
//! into a worded status-line message, and [`accumulate_chunk`] never reintroduces a CPU
//! path -- a decline mid-accumulation is exactly as terminal as one on the first chunk.
//!
//! Concretely, `GpuBackend`'s three decline reasons become, in this build:
//!
//! - **no `gpu` feature**: not a real possibility -- `Cargo.toml` enables it
//!   unconditionally, since a build without it would have nothing to render with.
//! - **no usable adapter**: the real, expected case on some machines (no WebGPU, too
//!   old, or disabled). Reported as [`RenderError::NoWebGpu`].
//! - **an HDR environment map**: `ui/app.slint` offers no HDR-map picker, since the
//!   megakernel has no `env_mode` for one. [`accumulate_chunk`] still handles that
//!   error explicitly (as [`RenderError::Declined`]) so a future picker fails loudly
//!   instead of quietly mis-rendering.
//!
//! Material is never a decline reason here: `GemMaterial::gpu_supported()` is
//! unconditionally `true`, since the biaxial WGSL port covers every material this
//! crate offers.
//!
//! # Progressive accumulation
//!
//! Both the CPU formula and the WGSL shader derive each sample's jitter and RNG from
//! the absolute sample index, so tracing `[sample_offset, sample_offset + spp)` and
//! adding into an already-partially-filled buffer extends the estimate rather than
//! biasing it, provided `sample_offset` is exactly the number of samples already
//! folded in. [`Accumulator`] pairs the running XYZ sum with that count, and
//! [`accumulate_chunk`] always dispatches starting at `accumulator.samples()`. `app.rs`'s
//! `render_loop` calls this repeatedly toward [`TARGET_SPP`] in [`CHUNK_SPP`]-sized
//! steps, presenting the tone-mapped partial sum after every chunk. See
//! [`Accumulator::reset`] for why correctness, not just visual freshness, requires
//! calling it on every scene change.
//!
//! # No canvas, no surface
//!
//! The megakernel `indicatrix::renderer::gpu::frame` dispatches is a compute shader: it
//! writes summed XYZ radiance into a storage buffer and never rasterizes to a
//! swapchain, so this module needs no `wgpu::Surface`. [`accumulate_chunk`] acquires a
//! device, dispatches, reads back a buffer of numbers, and leaves tone-mapping to
//! [`Accumulator::to_rgba`]. Those bytes go to `app.rs` as a plain `Vec<u8>`, wrapped in
//! a `slint::Image` for `ui/app.slint`'s `Image` element to display.

// `clippy::future_not_send`: every `async fn` below is checked `!Send` because it
// touches `wgpu`'s web backend types (browser-side `Rc`-based handles, never `Send`),
// but `wasm32-unknown-unknown` has no thread a future could be sent to regardless. One
// module-level allow rather than one per function.
#![allow(
    clippy::future_not_send,
    reason = "wgpu's web backend types are inherently !Send; wasm32-unknown-unknown \
              has no second thread to send a future to regardless -- see the comment \
              above"
)]

use glam::Vec3;
use indicatrix::{
    geometry::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::Camera},
    renderer::{
        gpu::frame::{GpuFrameRenderer, GpuFrameScene},
        tonemap::tonemap_to_rgba,
    },
};

use crate::scene::{FOV_DEG, MAX_BOUNCES, ViewState};

/// Total samples per pixel [`Accumulator`] accumulates toward before
/// `app.rs`'s `render_loop` stops dispatching further chunks. Matches
/// `apps/indicatrix-cut`'s own desktop default exactly
/// (`bridge::render_thread::context::RenderContext::default`'s `target_samples: 256`),
/// so this crate converges to the same noise floor the desktop viewer settles at.
pub const TARGET_SPP: u32 = 256;

/// Samples per pixel each [`accumulate_chunk`] dispatch traces.
///
/// A reasoned estimate, not a hardware measurement (no real GPU was reachable in this
/// project's browser-automation sandbox -- see [`accumulate_chunk`]'s `log_line` call
/// for the timing instrumentation left in place to calibrate this on real hardware).
/// Reasoned from `renderer::gpu::frame`'s byte-budget and dispatch-size limits:
/// [`indicatrix::renderer::gpu::frame::CHUNK_BUDGET_BYTES`] (64 MiB) and the
/// megakernel's dispatch cap (4,194,240 tuples) together mean
/// [`GpuFrameRenderer::accumulate_async`] stays a single internal GPU dispatch only
/// while `spp * (width * height)` stays under that cap -- `8` sat comfortably under it
/// at this crate's original fixed 660x480.
///
/// The render target is variable (see `scene::MAX_RENDER_DIM`), so at the
/// largest sizes `accumulate_async` transparently splits one call into a few
/// sequential internal dispatches rather than failing (a latency concern only;
/// additivity holds regardless of dispatch count). `8` still keeps [`TARGET_SPP`] /
/// `CHUNK_SPP` = 32 visible refinement steps at any resolution.
///
/// To recalibrate on real hardware: open devtools and read the "chunk: N spp in Xms"
/// lines every real chunk already prints; raise this if chunks land well under ~50ms,
/// lower it if they run long enough to feel like a stutter.
pub const CHUNK_SPP: u32 = 8;

/// Fixed lighting-rig direction (~48 deg azimuth, ~54 deg elevation). `ui/app.slint`
/// exposes no yaw/pitch controls for the key light (only the camera); matches
/// `apps/indicatrix-cut`'s own default so a stone renders lit the same way.
const LIGHT_YAW: f32 = 0.85;
const LIGHT_PITCH: f32 = 0.95;

/// Why [`accumulate_chunk`] could not add another chunk to the accumulator. See this
/// module's doc comment for the "GPU-only, no fallback" rationale.
pub enum RenderError {
    /// No WebGPU adapter/device could be acquired at all -- this browser or machine
    /// cannot run this build. Carries the underlying `GpuFrameError`'s message as a
    /// details line; `app.rs` leads with a fixed, named explanation instead.
    NoWebGpu(String),
    /// A device was acquired, but the megakernel declined this specific chunk
    /// (unsupported environment; unsupported material is defensively unreachable).
    /// Distinguished from [`Self::NoWebGpu`] since a different scene might succeed.
    Declined(String),
}

/// Lazily-acquired WebGPU renderer, held across chunks so acquiring a device and
/// compiling `transport_main` (both slow) happens at most once per page load.
#[derive(Default)]
pub struct GpuState {
    renderer: Option<GpuFrameRenderer>,
    tried: bool,
    /// Set the one time acquisition is attempted, when it failed -- so every chunk
    /// after the first reports the same concrete reason instead of re-attempting
    /// acquisition or falling back to a generic "no GPU" string.
    acquire_error: Option<String>,
}

impl GpuState {
    /// Acquires a device on the first call; every later call reuses whatever the
    /// first call found, success or failure, without touching the network/GPU again.
    async fn ensure_acquired(&mut self) {
        if self.tried {
            return;
        }
        self.tried = true;
        match GpuFrameRenderer::new_async().await {
            Ok(r) => {
                log_line(&format!(
                    "WebGPU render backend active: {}",
                    r.adapter_label()
                ));
                self.renderer = Some(r);
            }
            Err(e) => {
                log_line(&format!("WebGPU acquisition failed: {e}"));
                self.acquire_error = Some(e.to_string());
            }
        }
    }
}

/// The running progressive-accumulation state for one scene: a summed-XYZ buffer plus
/// how many samples per pixel have been folded into it. Deliberately not `Copy`/cheaply
/// cloned: `buffer` is `width * height` [`Vec3`]s (tens of MB at `scene::MAX_RENDER_DIM`),
/// so `app.rs` keeps one instance alive and mutates it in place via [`accumulate_chunk`].
#[derive(Default)]
pub struct Accumulator {
    buffer: Vec<Vec3>,
    samples: u32,
    /// The dimensions `buffer` is currently sized for -- tracked explicitly so
    /// [`Self::reset`] can tell "same pixel count, different shape" (e.g. 800x600 ->
    /// 600x800) apart from "unchanged", which a length-only check would miss.
    width: u32,
    height: u32,
}

impl Accumulator {
    /// Samples per pixel currently folded into this accumulator.
    #[must_use]
    pub const fn samples(&self) -> u32 {
        self.samples
    }

    /// The render-target size this accumulator's buffer currently holds -- what
    /// `app.rs` sizes the `SharedPixelBuffer` it hands to `ui/app.slint` with.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Zeroes the buffer (resizing to `width`x`height` if that differs) and resets the
    /// sample count to `0`, so the next [`accumulate_chunk`] call starts fresh.
    ///
    /// # Why this must be called on every scene change, not just a new file
    ///
    /// Additivity holds only because every accumulated sample was drawn against the
    /// same camera pose, material, and lighting -- the seed/jitter formula is a pure
    /// function of `(pixel, sample_num)` alone. Continuing to accumulate after any of
    /// those change would silently blend two physically different images' radiance
    /// together with no way to separate them back out. `app.rs`'s `render_loop` calls
    /// this on every camera/material/lighting/exposure change, a new file, or a
    /// resize -- a size change invalidates existing samples too, since a ray direction
    /// is as much a function of `width`/`height` as of yaw/pitch/distance.
    pub fn reset(&mut self, width: u32, height: u32) {
        let n = width as usize * height as usize;
        if self.width == width && self.height == height {
            self.buffer.fill(Vec3::ZERO);
        } else {
            self.buffer = vec![Vec3::ZERO; n];
        }
        self.width = width;
        self.height = height;
        self.samples = 0;
    }

    /// Tone-maps the current running sum into `self.width() * self.height() * 4` RGBA8
    /// bytes (row-major, matching `slint::Rgba8Pixel`'s layout), dividing by
    /// [`Self::samples`] to turn the summed radiance into an averaged one.
    /// `samples() == 0` tone-maps to black; guarded explicitly below even though
    /// `app.rs` only calls this after at least one successful chunk.
    #[must_use]
    pub fn to_rgba(&self) -> Vec<u8> {
        let scale = if self.samples == 0 {
            0.0
        } else {
            1.0 / self.samples as f32
        };
        tonemap_to_rgba(&self.buffer, scale)
    }
}

/// What one successful [`accumulate_chunk`] call produced, beyond mutating
/// `accumulator` in place: the backend label `app.rs` shows verbatim.
pub struct ChunkOutcome {
    pub backend_label: String,
}

/// Traces [`CHUNK_SPP`] more samples per pixel of `planes`/`material` under `view`'s
/// camera and lighting, adding into `accumulator` starting at its current
/// [`Accumulator::samples`] -- never `0` or a caller-chosen offset. Traces fewer than
/// [`CHUNK_SPP`] if that many would overshoot `target_spp`, never more.
///
/// # Errors
///
/// See [`RenderError`]. `accumulator` is guaranteed unmodified on either variant -- the
/// megakernel dispatch never partially writes on a decline.
pub async fn accumulate_chunk(
    gpu: &mut GpuState,
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    view: &ViewState,
    accumulator: &mut Accumulator,
    chunk_spp: u32,
) -> Result<ChunkOutcome, RenderError> {
    gpu.ensure_acquired().await;

    let Some(renderer) = gpu.renderer.as_mut() else {
        let detail = gpu
            .acquire_error
            .clone()
            .unwrap_or_else(|| "no adapter reported".to_string());
        return Err(RenderError::NoWebGpu(detail));
    };

    let camera = Camera::new(view.yaw, view.pitch, view.distance, FOV_DEG);
    let environment = crate::scene::lighting_for_index(view.lighting_index)
        .studio(view.exposure, LIGHT_YAW, LIGHT_PITCH)
        .with_backdrop(indicatrix::optics::raytracer::BACKDROP_GREY);

    let gpu_scene = GpuFrameScene {
        camera: &camera,
        // Read from `accumulator` itself, not a separately-passed width/height, so the
        // scene's dimensions can never desync from what actually sized its buffer.
        width: accumulator.width,
        height: accumulator.height,
        planes,
        // No frosted-girdle toggle in this crate's UI -- every facet renders polished
        // (an empty slice means "all polished", same as indicatrix's other entry points).
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment,
    };

    let start = now_ms();
    renderer
        .accumulate_async(
            &gpu_scene,
            accumulator.samples,
            chunk_spp,
            &mut accumulator.buffer,
        )
        .await
        .map_err(|e| RenderError::Declined(e.to_string()))?;
    if let Some(elapsed) = start.zip(now_ms()).map(|(start, now)| now - start) {
        log_line(&format!(
            "chunk: {chunk_spp} spp in {elapsed:.1}ms ({} -> {} total)",
            accumulator.samples,
            accumulator.samples + chunk_spp
        ));
    }

    accumulator.samples += chunk_spp;
    Ok(ChunkOutcome {
        backend_label: format!("WebGPU: {}", renderer.adapter_label()),
    })
}

/// Milliseconds since the page's time origin, per
/// [`Performance.now()`](https://developer.mozilla.org/en-US/docs/Web/API/Performance/now).
/// `None` only if no `Window`/`Performance` object is reachable (not a real possibility
/// here); [`accumulate_chunk`]'s timing log just skips itself rather than unwrapping.
fn now_ms() -> Option<f64> {
    web_sys::window()?.performance().map(|p| p.now())
}

/// Writes one line to the browser devtools console (`console.log`). Used instead of a
/// `tracing` subscriber (this crate registers none, so those calls would be no-ops)
/// for this module's handful of diagnostics -- see [`CHUNK_SPP`] for the measurement
/// this supports.
fn log_line(message: &str) {
    web_sys::console::log_1(&format!("indicatrix-web: {message}").into());
}

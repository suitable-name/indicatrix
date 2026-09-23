//! Adapter/device acquisition for `indicatrix`'s `gpu`-feature compute infrastructure.

use std::{
    fmt,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// Why [`GpuContext::acquire`] could not obtain a usable GPU.
///
/// Both variants carry the underlying `wgpu` error's `Debug` output as a `String`
/// (avoids exposing the raw error type in this crate's public API), so callers can
/// report a diagnostic and exit nonzero without panicking.
#[derive(Debug, Clone)]
pub enum GpuAcquireError {
    /// No backend/driver on this system produced an adapter at all.
    NoAdapter(String),
    /// An adapter was found but creating a logical device from it failed.
    RequestDevice(String),
}

impl fmt::Display for GpuAcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(msg) => write!(
                f,
                "no wgpu adapter is available on this system (no compatible GPU/driver \
                 found via any backend): {msg}"
            ),
            Self::RequestDevice(msg) => write!(f, "failed to acquire a wgpu device: {msg}"),
        }
    }
}

impl std::error::Error for GpuAcquireError {}

/// Storage buffers the transport megakernel (`transport_main`) binds in one stage.
///
/// Bindings 2-10 and 12-13 of `transport_bounce.wgsl`. Must track that binding list;
/// the layout check only sees per-binding shapes, not the count.
pub const MEGAKERNEL_STORAGE_BUFFERS: u32 = 11;

/// Storage buffers the wavefront `wavefront_bounce` kernel binds in one stage
/// (`wavefront_transport.wgsl`).
pub const WAVEFRONT_STORAGE_BUFFERS: u32 = 18;

/// Total uniform + storage buffer bindings `wavefront_bounce`'s auto-derived pipeline
/// layout reaches in one stage (`wavefront_transport.wgsl`).
///
/// Every scene binding (0-15: `camera` through `wf_params`, all reachable via
/// `transport_bounce_step`/`transport_generate_ray`, see
/// [`super::frame::wavefront_bounce_bind_group`]'s own doc comment on why `camera` is
/// included) plus its 14 per-ray `SoA` arrays (16-29) plus `ray_pending_light_mis_dir`
/// (34, the light-MIS interior-direction hand-off) = 31. Distinct from
/// [`WAVEFRONT_STORAGE_BUFFERS`] (a
/// storage-buffer-only count that does not include that binding or the `camera`/`params`
/// uniform bindings this entry point also reaches):
/// `wgpu` 30's `max_buffers_and_acceleration_structures_per_shader_stage` limit counts
/// EVERY buffer/acceleration-structure binding (uniform and storage alike) in one stage,
/// not storage buffers alone, and its WebGPU-baseline default (well under 31) makes
/// `Device::create_compute_pipeline`'s implicit-layout derivation fail outright --
/// "Unable to derive an implicit layout ... Too many bindings" -- rather than merely
/// declining gracefully. [`Self::acquire_async`] requests this many (clamped) from the
/// adapter; [`super::frame::GpuFrameRenderer::set_pipeline_kind`] checks the device
/// actually granted enough before switching to [`super::frame::GpuPipelineKind::Wavefront`].
pub const WAVEFRONT_BOUNCE_TOTAL_BUFFERS: u32 = 31;

/// Upper bound on the per-stage storage-buffer limit requested from an adapter.
const MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE: u32 = 32;

/// Upper bound on the per-stage combined uniform+storage+acceleration-structure buffer
/// limit requested from an adapter -- see [`WAVEFRONT_BOUNCE_TOTAL_BUFFERS`]'s doc
/// comment. A little headroom over that count, same reasoning as
/// [`MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE`]'s own margin over
/// [`WAVEFRONT_STORAGE_BUFFERS`].
const MAX_REQUESTED_BUFFERS_AND_ACCEL_PER_STAGE: u32 = 40;

/// A live `wgpu` adapter/device/queue, acquired once and reused by every self-test.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Set by the [`Device::on_uncaptured_error`](wgpu::Device::on_uncaptured_error)
    /// handler [`Self::acquire_async`] installs, the moment `wgpu` reports a validation
    /// or internal error that no `push_error_scope` caught. Without that handler, `wgpu`'s
    /// documented default for an uncaptured error is to panic the calling thread -- which,
    /// mid-`dispatch_chunk`, panics with the previous chunk's `map_async` already in
    /// flight and its staging buffer never unmapped (see `renderer::gpu::frame`'s module
    /// doc comment and `GpuFrameRenderer::abandon_in_flight`). Reading this flag turns
    /// that panic into an ordinary [`super::frame::GpuFrameError::DeviceLost`] instead --
    /// see `GpuFrameRenderer::accumulate_turn`, the only reader. `Arc` (not a plain
    /// `AtomicBool`): the handler closure captures a clone and outlives this struct field
    /// by however long `wgpu` holds the callback registered.
    pub validation_error_seen: Arc<AtomicBool>,
    /// The `Display` text of the most recent uncaptured `wgpu::Error` the
    /// `on_uncaptured_error` handler installed by [`Self::acquire_async`] observed, if
    /// any. Companion to [`Self::validation_error_seen`]: that flag alone tells a caller
    /// only that *something* went wrong, and before this field existed the actual wgpu
    /// error text reached nowhere but a `tracing::error!` call with no subscriber
    /// installed in most binaries (see `examples/gpu_equivalence_harness.rs`, which now
    /// installs one) -- so every `DeviceLost` produced from this signal read the same
    /// generic sentence with no way to tell a buffer-size validation failure from a
    /// missing bind-group entry. `GpuFrameRenderer::accumulate_turn` reads (and clears)
    /// this in the same step it reads `validation_error_seen`, and folds the text into
    /// its `GpuFrameError::DeviceLost` message. `Mutex`, not an atomic: the payload is a
    /// `String`, and this is on the cold "something already went wrong" path, so lock
    /// contention is irrelevant.
    pub last_uncaptured_error: Arc<Mutex<Option<String>>>,
}

impl GpuContext {
    /// Synchronously acquires a `wgpu` adapter/device/queue via [`pollster::block_on`].
    ///
    /// Prefers a high-performance adapter but accepts whatever the system offers (no
    /// discrete-GPU requirement).
    ///
    /// # Errors
    ///
    /// [`GpuAcquireError::NoAdapter`] if no backend/driver can produce an adapter, or
    /// [`GpuAcquireError::RequestDevice`] if device creation failed. Callers MUST treat
    /// either as a clean, diagnosable condition to report and exit on -- never panic or
    /// `unwrap`, since "no GPU here" is expected on some machines, not a bug.
    pub fn acquire() -> Result<Self, GpuAcquireError> {
        pollster::block_on(Self::acquire_async())
    }

    // `pub(crate)`: wasm32-only `GpuFrameRenderer::new_async` awaits this directly
    // instead of `pollster::block_on` (which parks the OS thread, unavailable on wasm32).
    //
    // `clippy::future_not_send` fires only for wasm32: `wgpu`'s web backend future holds
    // browser-side `Rc` handles, never `Send` -- and there's no second thread on
    // wasm32-unknown-unknown to send it to anyway. `cfg_attr` scopes the allow to that
    // target so a native regression would still be caught by clippy.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::future_not_send,
            reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                      handles); wasm32-unknown-unknown has no second thread regardless"
        )
    )]
    pub(crate) async fn acquire_async() -> Result<Self, GpuAcquireError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| GpuAcquireError::NoAdapter(format!("{e:?}")))?;

        // wgpu's default limits are the WebGPU baseline, where a compute stage may bind
        // only 8 storage buffers. The transport megakernel binds
        // `MEGAKERNEL_STORAGE_BUFFERS` and the wavefront kernels more, so ask for
        // whatever the adapter really offers (bounded, so a huge Vulkan figure does not
        // become a device requirement) and let `GpuFrameRenderer::new` decline cleanly
        // when even that is not enough.
        let adapter_limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_storage_buffers_per_shader_stage: adapter_limits
                .max_storage_buffers_per_shader_stage
                .clamp(
                    wgpu::Limits::default().max_storage_buffers_per_shader_stage,
                    MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE,
                ),
            // A SEPARATE limit from
            // `max_storage_buffers_per_shader_stage` above -- see
            // `WAVEFRONT_BOUNCE_TOTAL_BUFFERS`'s doc comment for why the wavefront
            // pipeline's implicit layout derivation needs this one raised too, not just
            // the storage-buffer-only count.
            max_buffers_and_acceleration_structures_per_shader_stage: adapter_limits
                .max_buffers_and_acceleration_structures_per_shader_stage
                .clamp(
                    wgpu::Limits::default()
                        .max_buffers_and_acceleration_structures_per_shader_stage,
                    MAX_REQUESTED_BUFFERS_AND_ACCEL_PER_STAGE,
                ),
            ..Default::default()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("indicatrix gpu-feature compute device"),
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuAcquireError::RequestDevice(format!("{e:?}")))?;

        // Logging-only: fires on a driver-owned thread whenever `wgpu` detects device
        // loss, independent of `GpuFrameError::DeviceLost` from an in-flight dispatch.
        // The actual "stop retrying" mechanism is `GpuBackend`'s `lost` flag, driven by
        // that error return -- never synchronize crate-wide state from this callback.
        device.set_device_lost_callback(|reason, message| {
            tracing::warn!(?reason, %message, "wgpu device lost");
        });

        // Without this, an uncaptured wgpu error (a validation
        // failure this crate did not wrap in its own `push_error_scope`, e.g. a
        // wavefront chunk's buffer request exceeding `max_storage_buffer_binding_size`)
        // panics the calling thread by wgpu's own documented default.
        // That panic can land mid-`dispatch_chunk`, after the PREVIOUS chunk's
        // `map_async` is already in flight, leaving its staging buffer mapped forever.
        // Logging here and setting `validation_error_seen`
        // instead lets `GpuFrameRenderer::accumulate_turn` turn it into an ordinary
        // `GpuFrameError::DeviceLost` return on this SAME thread, no unwind involved.
        let validation_error_seen = Arc::new(AtomicBool::new(false));
        let validation_error_seen_for_handler = Arc::clone(&validation_error_seen);
        let last_uncaptured_error = Arc::new(Mutex::new(None));
        let last_uncaptured_error_for_handler = Arc::clone(&last_uncaptured_error);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            tracing::error!(%error, "wgpu uncaptured device error");
            validation_error_seen_for_handler.store(true, std::sync::atomic::Ordering::Relaxed);
            // A poisoned mutex here would mean some other thread panicked while holding
            // it, which cannot happen -- nothing ever holds this lock across code that
            // can panic (see `Self::last_uncaptured_error`'s doc comment) -- so
            // `unwrap_or_else` with `into_inner` is a defensive fallback, not a normal
            // path.
            let mut slot = last_uncaptured_error_for_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *slot = Some(error.to_string());
        }));

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            validation_error_seen,
            last_uncaptured_error,
        })
    }
}

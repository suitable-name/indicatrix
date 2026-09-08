//! Adapter/device acquisition for `indicatrix`'s `gpu`-feature compute infrastructure.

use std::fmt;

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

/// Upper bound on the per-stage storage-buffer limit requested from an adapter.
const MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE: u32 = 32;

/// A live `wgpu` adapter/device/queue, acquired once and reused by every self-test.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
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

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }
}

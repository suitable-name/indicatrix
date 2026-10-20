//! Acquisition for the small kernel. Never requests megakernel/wavefront limits.
use crate::renderer::gpu::{GpuAcquireError, GpuContext};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Request an adapter and a device limited to the four-storage-buffer kernel.
///
/// # Errors
///
/// Returns [`GpuAcquireError`] when no adapter is available, the adapter lacks
/// four storage and seven total buffer bindings, or device creation fails.
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                  handles); wasm32-unknown-unknown has no second thread regardless"
    )
)]
pub(in crate::renderer::gpu) async fn acquire() -> Result<GpuContext, GpuAcquireError> {
    let instance = wgpu::Instance::new(crate::renderer::gpu::instance_descriptor());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| GpuAcquireError::NoAdapter(format!("{error:?}")))?;
    let available = adapter.limits();
    if available.max_storage_buffers_per_shader_stage < 4
        || available.max_buffers_and_acceleration_structures_per_shader_stage < 7
    {
        return Err(GpuAcquireError::RequestDevice(
            "Ray-Traced requires four storage and seven total buffer bindings".into(),
        ));
    }
    let limits = wgpu::Limits {
        max_storage_buffers_per_shader_stage: 4,
        max_buffers_and_acceleration_structures_per_shader_stage: 7,
        ..wgpu::Limits::downlevel_defaults().using_resolution(available)
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("indicatrix Ray-Traced four-storage-buffer device"),
            required_limits: limits,
            ..Default::default()
        })
        .await
        .map_err(|error| GpuAcquireError::RequestDevice(format!("{error:?}")))?;
    let validation_error_seen = Arc::new(AtomicBool::new(false));
    let last_uncaptured_error = Arc::new(Mutex::new(None));
    let flag = Arc::clone(&validation_error_seen);
    let text = Arc::clone(&last_uncaptured_error);
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        flag.store(true, Ordering::Relaxed);
        *text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
    }));
    let flag = Arc::clone(&validation_error_seen);
    let text = Arc::clone(&last_uncaptured_error);
    device.set_device_lost_callback(move |reason, message| {
        flag.store(true, Ordering::Relaxed);
        *text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(format!("device lost ({reason:?}): {message}"));
    });
    Ok(GpuContext {
        instance,
        adapter,
        device,
        queue,
        validation_error_seen,
        last_uncaptured_error,
    })
}

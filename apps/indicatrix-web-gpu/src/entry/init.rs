//! The GPU Worker's `Init` handling: the version and role checks, acquiring the adapter,
//! watching for device loss, and reporting the adapter's limits to the page.
//!
//! Split out of `lib.rs` (the message handler grew past the lint's line limit); the shared
//! state, `post` and the device flags live in the parent module.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use indicatrix::renderer::gpu::{GpuContext, event_wavefront::EventWavefrontRenderer};
use indicatrix_web_core::protocol::{FromWorker, PROTOCOL_VERSION, WorkerRole};

use super::{
    COMPILE_BLOCKED, DEVICE_GENERATION, DEVICE_LOST, GpuWorkerState,
    adapter_info::{browser_adapter_info, non_empty_or},
    device_lost, post,
};

/// Whether `Init` may go on: the page's protocol version and the role are the Worker's own,
/// and no live device is held. Posts the reason when not. A lost device's renderer and
/// scene are dropped here, before a new device is acquired.
fn init_allowed(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    protocol_version: u32,
    role: WorkerRole,
) -> bool {
    if protocol_version != PROTOCOL_VERSION {
        post(&FromWorker::Error {
            message: format!(
                "protocol version mismatch: page {protocol_version}, worker {PROTOCOL_VERSION}"
            ),
        });
        return false;
    }
    if role != WorkerRole::Gpu {
        post(&FromWorker::Error {
            message: format!("GPU worker received unexpected role: {role:?}"),
        });
        return false;
    }

    // A second Init is only valid to replace a lost device.
    let mut state = state_rc.borrow_mut();
    if state.context.is_some() {
        if !device_lost() {
            post(&FromWorker::Error {
                message: "GPU worker is already initialized".to_string(),
            });
            return false;
        }
        // Drop the lost device's renderer and scene before acquiring a new one.
        state.renderer = None;
        state.whitted = None;
        state.context = None;
        state.cached_scene = None;
        state.role = None;
    }
    true
}

/// Registers the device-lost callback of the device of `generation`. It touches only
/// `Cell`s and `post`, never the `RefCell` state, so it cannot re-enter a borrow held by a
/// suspended handler; a stale device's callback (an older generation) is ignored.
fn watch_for_device_loss(ctx: &GpuContext, generation: u32) {
    ctx.device.set_device_lost_callback(move |reason, message| {
        if DEVICE_GENERATION.with(Cell::get) != generation {
            return;
        }
        DEVICE_LOST.with(|l| l.set(true));
        post(&FromWorker::GpuLost {
            reason: format!("{reason:?}: {message}"),
        });
    });
}

/// Handles `Init`: acquires the WebGPU device, checks its limits, reports the adapter and
/// keeps the renderer. Any failure is posted to the page and ends the handling.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn handle_init(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    protocol_version: u32,
    role: WorkerRole,
) {
    if !init_allowed(state_rc, protocol_version, role) {
        return;
    }

    let ctx = match GpuContext::acquire_async().await {
        Ok(ctx) => ctx,
        Err(spectral_error) => {
            match indicatrix::renderer::gpu::whitted::acquire_whitted_context().await {
                Ok(context) => context,
                Err(ray_error) => {
                    post(&FromWorker::GpuUnavailable {
                        reason: format!(
                            "could not acquire WebGPU context: {spectral_error}; small Ray-Traced device: {ray_error}"
                        ),
                    });
                    return;
                }
            }
        }
    };

    let generation = DEVICE_GENERATION.with(|g| {
        let next = g.get().wrapping_add(1);
        g.set(next);
        next
    });
    DEVICE_LOST.with(|l| l.set(false));
    COMPILE_BLOCKED.with(|c| c.set(false));

    // Register the device-lost callback before anything else.
    watch_for_device_loss(&ctx, generation);

    let limits = ctx.device.limits();
    let info = ctx.adapter.get_info();

    if limits.max_storage_buffers_per_shader_stage < 4 {
        post(&FromWorker::GpuUnavailable {
            reason: format!(
                "adapter supports only {} storage buffers per stage, 4 required",
                limits.max_storage_buffers_per_shader_stage
            ),
        });
        return;
    }

    // The browser's own GPUAdapterInfo; the wgpu values are the fallback. This
    // must never fail Init, so every JS error keeps them.
    let browser = browser_adapter_info().await.unwrap_or_default();

    post(&FromWorker::GpuAdapter {
        vendor: non_empty_or(browser.vendor, format!("{:04x}", info.vendor)),
        architecture: browser.architecture,
        device: non_empty_or(browser.device, format!("{:04x}", info.device)),
        description: non_empty_or(browser.description, info.name),
        backend: format!("{:?}", info.backend),
        max_storage_buffers_per_stage: limits.max_storage_buffers_per_shader_stage,
        max_storage_binding: limits.max_storage_buffer_binding_size,
        max_buffer: limits.max_buffer_size,
    });

    let mut state = state_rc.borrow_mut();
    state.role = Some(role);
    state.context = Some(ctx.clone());
    if limits.max_storage_buffers_per_shader_stage >= 8 {
        state.renderer = Some(EventWavefrontRenderer::new(ctx));
    }
}

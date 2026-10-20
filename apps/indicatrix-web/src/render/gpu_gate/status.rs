//! The gate's two persistent texts: the sentence in the Render panel that says what the app
//! renders with (`RenderModel.gpu-summary`) and the status strip's short form
//! (`AppModel.backend-text`). Both are built by the pure functions in
//! `indicatrix_web_core::gpu_startup`; this module only gathers what they describe.

use indicatrix_web_core::{
    gpu_startup::{self, GpuState, SummaryInput},
    hdr::render_worker_count,
    host::WorkerPool,
};
use slint::ComponentHandle;

use super::GATE;
use crate::{AppModel, RenderModel, app::Ctx};

/// The CPU render-Worker count: the pool's, or what it would start when it is not up yet.
fn cpu_workers(pool: Option<&WorkerPool>) -> u32 {
    pool.map_or_else(
        || {
            render_worker_count(
                web_sys::window().map_or(1.0, |window| window.navigator().hardware_concurrency()),
            )
        },
        |pool| pool.render().worker_count(),
    )
}

/// The panel sentence and the strip line for the current state.
pub(super) fn texts(ctx: &Ctx, pool: Option<&WorkerPool>) -> Option<(String, String)> {
    let ui = ctx.ui.upgrade()?;
    if ctx.state.borrow().settings.engine_spec().is_ray() {
        let enabled = ctx.state.borrow().settings.gpu_enabled;
        let admitted = enabled
            && super::ray::ready()
            && pool.is_some_and(|pool| pool.render().is_gpu_admitted());
        let unavailable = GATE.with(|gate| {
            let gate = gate.borrow();
            if gate.no_webgpu {
                Some("this browser has no WebGPU".to_owned())
            } else if gate.blocked_at_start {
                Some(
                    "the previous GPU startup was lost; forget its test result to retry".to_owned(),
                )
            } else {
                None
            }
        });
        let reason = if !enabled {
            Some("GPU Rendering is off".to_owned())
        } else if unavailable.is_some() {
            unavailable
        } else {
            pool.and_then(|pool| pool.render().gpu_ineligibility_reason().map(str::to_owned))
                .or_else(super::ray::reason)
        };
        let summary = if admitted {
            "Ray-Traced renders on the GPU; CPU workers may also trace taps.".to_owned()
        } else {
            format!(
                "Ray-Traced renders on the CPU: {}.",
                reason.as_deref().unwrap_or("waiting for a render scene")
            )
        };
        let backend = if admitted {
            "GPU · Ray-Traced".to_owned()
        } else {
            format!(
                "CPU · Ray-Traced · {}",
                gpu_startup::workers_text(cpu_workers(pool))
            )
        };
        return Some((summary, backend));
    }
    let enabled = ctx.state.borrow().settings.gpu_enabled;
    let model = ui.global::<RenderModel>();
    let state = GpuState::from_code(model.get_gpu_state());
    let detail = model.get_gpu_status_text().to_string();
    let (adapter, no_webgpu) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.adapter_name.clone(), g.no_webgpu)
    });
    // The scene's verdict resolves its material, so it is only asked when it matters.
    let (admitted, scene_reason) = match pool {
        Some(pool) if state == GpuState::Ok => (
            pool.render().is_gpu_admitted(),
            pool.render().gpu_ineligibility_reason(),
        ),
        _ => (false, None),
    };
    let input = SummaryInput {
        enabled,
        no_webgpu,
        state,
        workers: cpu_workers(pool),
        adapter: adapter.as_deref(),
        detail: &detail,
        admitted,
        scene_reason,
    };
    Some((
        gpu_startup::summary(&input),
        gpu_startup::backend_line(&input),
    ))
}

/// Writes both texts.
fn apply(ctx: &Ctx, pool: Option<&WorkerPool>) {
    let Some((summary, line)) = texts(ctx, pool) else {
        return;
    };
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_gpu_summary(summary.into());
        ui.global::<AppModel>().set_backend_text(line.into());
    }
}

/// Refreshes the texts after a GPU event or a GPU setting change.
pub(super) fn refresh(ctx: &Ctx) {
    let pool = crate::workers::existing();
    apply(ctx, pool.as_ref());
}

/// The status strip's line per section 6.5 (`CPU · 7 workers`, `GPU + CPU · 7 workers`, with
/// the reason in brackets when the GPU is not in use).
#[must_use]
pub fn format_backend_text(ctx: &Ctx, pool: &WorkerPool) -> String {
    texts(ctx, Some(pool)).map_or_else(
        || {
            format!(
                "CPU \u{b7} {}",
                gpu_startup::workers_text(cpu_workers(Some(pool)))
            )
        },
        |(_, line)| line,
    )
}

/// Updates `AppModel.backend_text` and the panel sentence.
pub fn update_backend_text(ctx: &Ctx, pool: &WorkerPool) {
    apply(ctx, Some(pool));
}

//! Selecting the correct, independently qualified renderer for the current scene.

use indicatrix_web_core::{gpu_admission::AdmissionState, gpu_startup::GpuState, host::WorkerPool};
use slint::ComponentHandle;

use super::{GATE, begin_test, ray, show, start_admit_compile, update_backend_text};
use crate::{RenderModel, app::Ctx};

/// Admits the GPU when the first CPU frame is displayed, the adapter passed, the scene is
/// eligible and the kernels are compiled in this page's worker; starts that compile
/// (`admit`) when it is the only thing missing.
pub fn admit_if_ready(ctx: &Ctx, pool: &WorkerPool) {
    if pool.render().engine().engine != ctx.state.borrow().settings.engine_spec().engine {
        pool.render().set_gpu_admitted(false);
        return;
    }
    if ray::selected(ctx) {
        ray::admit(ctx, pool);
        return;
    }
    if ray::busy() {
        pool.render().set_gpu_admitted(false);
        return;
    }
    if GATE.with(|gate| gate.borrow().ray_only_device) {
        pool.render().set_gpu_admitted(false);
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RenderModel>();
    let has_frame = model.get_has_frame();
    let (compiled, running, failed, record) = GATE.with(|gate| {
        let gate = gate.borrow();
        (
            gate.compiled_ok,
            gate.testing || gate.admitting || gate.starting,
            gate.failed_in_session,
            gate.record.clone(),
        )
    });
    if compiled && !running && !failed {
        show(
            ctx,
            GpuState::Ok,
            "Path-Traced GPU kernels are qualified",
            true,
        );
    } else if !running && !failed && pool.render().is_gpu_ready() {
        match record.as_ref().map(|record| record.state) {
            Some(AdmissionState::Ok) => start_admit_compile(ctx, pool),
            Some(AdmissionState::Unknown) => begin_test(ctx, pool),
            _ => {}
        }
    }
    let is_ok = GpuState::from_code(model.get_gpu_state()) == GpuState::Ok;

    let mut admitted = false;
    if has_frame && is_ok && pool.render().gpu_ineligibility_reason().is_none() {
        let (compiled_ok, running) = GATE.with(|gate| {
            let g = gate.borrow();
            (g.compiled_ok, g.testing || g.admitting)
        });
        if compiled_ok {
            admitted = true;
        } else if !running && pool.render().is_gpu_ready() {
            let qualified = GATE.with(|gate| {
                gate.borrow()
                    .record
                    .as_ref()
                    .is_some_and(|record| record.state == AdmissionState::Ok)
            });
            if qualified {
                start_admit_compile(ctx, pool);
            } else {
                begin_test(ctx, pool);
            }
        }
    }
    pool.render().set_gpu_admitted(admitted);
    update_backend_text(ctx, pool);
}

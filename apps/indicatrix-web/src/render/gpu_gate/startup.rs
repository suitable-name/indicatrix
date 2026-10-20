//! The GPU start-up run of the page: deciding at load what to do
//! (`indicatrix_web_core::gpu_startup::plan`), starting the GPU worker under a crash marker,
//! the progress overlay, and the message at the end that says what the app renders with.
//!
//! This is the page half; the decisions and the texts are pure and tested natively in
//! `indicatrix_web_core::gpu_startup`. The state machine that follows the worker's answers
//! (adapter, compile, self-check) is the parent module's.

use indicatrix_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    gpu_startup::{self, AdapterMarker, GpuState, STARTUP_KEY, Stage, StartupFacts, StartupPlan},
};
use slint::ComponentHandle;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

use super::{GATE, restart_deadline, show, status, stop_deadline};
use crate::{
    RenderModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    storage::admission::{self, AdmissionRecord, AdmissionState},
};

/// Whether the browser has WebGPU (`navigator.gpu`).
fn has_webgpu() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let navigator: JsValue = window.navigator().into();
    js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu"))
        .is_ok_and(|gpu| !gpu.is_undefined() && !gpu.is_null())
}

/// Removes the start-up crash marker (the run got an answer from the worker, or the
/// adapter's own marker took over).
pub(super) fn clear_marker() {
    admission::forget(STARTUP_KEY);
}

/// Decides and starts the run: at page load, when the setting is switched on, and for the
/// retry after "Forget GPU Test Result" on a blocked start (`auto = false`: that retry stops
/// at "GPU not tested yet" for an adapter it has not seen).
pub(super) fn start(ctx: &Ctx, auto: bool) {
    let enabled = ctx.state.borrow().settings.gpu_enabled;
    let record = admission::load(STARTUP_KEY);
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.auto = auto;
        g.no_webgpu = false;
        g.blocked_at_start = false;
        g.starting = false;
        g.testing = false;
        g.admitting = false;
        g.compiled_ok = false;
        g.adapter_key = None;
        g.adapter_name = None;
        g.record = None;
        g.compiled_kernels.clear();
    });
    let facts = StartupFacts {
        enabled,
        has_webgpu: has_webgpu(),
        startup_record: record.as_ref(),
    };
    match gpu_startup::plan(&facts) {
        StartupPlan::Off => {
            show(ctx, GpuState::Off, "", true);
            announce_now(ctx);
        }
        StartupPlan::NoWebGpu => {
            GATE.with(|gate| gate.borrow_mut().no_webgpu = true);
            show(ctx, GpuState::Off, "", false);
            announce_now(ctx);
        }
        StartupPlan::Blocked(reason) => block(ctx, &reason, true),
        StartupPlan::Run => run(ctx, auto),
    }
}

/// The run is not made by itself: the reason is shown, and (with `can_retry`) "Forget GPU
/// Test Result" is the way to try again.
fn block(ctx: &Ctx, reason: &str, can_retry: bool) {
    GATE.with(|gate| gate.borrow_mut().blocked_at_start = can_retry);
    show(
        ctx,
        GpuState::Failed,
        &format!("GPU test failed: {reason}"),
        can_retry,
    );
    announce_now(ctx);
}

/// Spawns the GPU worker under the start-up marker.
fn run(ctx: &Ctx, auto: bool) {
    if GATE.with(|gate| gate.borrow().failed_in_session) {
        show(
            ctx,
            GpuState::Failed,
            "GPU unavailable (failed earlier in this session). Reload the page to try again.",
            false,
        );
        status::refresh(ctx);
        return;
    }
    // The adapter is not known yet, so its own marker cannot exist: this one stands in for
    // it until the worker has answered. Without a place to keep it a crash could not be
    // remembered, and nothing is started by itself.
    let marker = AdmissionRecord {
        state: AdmissionState::Probing,
        when: iso8601_utc_from_epoch_ms(js_sys::Date::now()),
        ..AdmissionRecord::default()
    };
    if !admission::save(STARTUP_KEY, &marker) {
        block(ctx, gpu_startup::REASON_NO_STORAGE, false);
        return;
    }
    begin_run(ctx, auto);
    GATE.with(|gate| gate.borrow_mut().starting = true);
    set_stage(ctx, Stage::Adapter);
    status::refresh(ctx);

    // A worker left over from an earlier switch-on is replaced: the run needs the adapter
    // event of a new one.
    if let Some(pool) = crate::workers::existing() {
        pool.render().terminate_gpu();
    }
    let pool = match crate::render::live::ensure_pool(ctx) {
        Ok(pool) => pool,
        Err(error) => {
            fail_start(ctx, &error, false);
            return;
        }
    };
    pool.render().set_gpu_enabled(true);
    if GATE.with(|gate| gate.borrow().starting) {
        restart_deadline(ctx);
    }
}

/// The worker could not be started or never answered. `keep_marker`: the browser may have
/// lost the GPU (the worker hung), so the next load is blocked until "Forget GPU Test
/// Result"; otherwise the failure was clean and the marker goes.
pub(super) fn fail_start(ctx: &Ctx, reason: &str, keep_marker: bool) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.starting = false;
        g.testing = false;
        g.admitting = false;
        g.compiled_ok = false;
        g.failed_in_session = true;
    });
    if keep_marker {
        let lost = AdmissionRecord {
            state: AdmissionState::Failed,
            reason: Some(reason.to_string()),
            when: iso8601_utc_from_epoch_ms(js_sys::Date::now()),
            ..AdmissionRecord::default()
        };
        admission::save(STARTUP_KEY, &lost);
    } else {
        clear_marker();
    }
    show(
        ctx,
        GpuState::Failed,
        &format!("GPU unavailable: {reason}. Reload the page to try again."),
        false,
    );
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
        pool.render().terminate_gpu();
    }
    end_run(ctx);
}

/// "Forget GPU Test Result" on a blocked start: forgets the marker and starts the run again.
pub(super) fn forget_and_restart(ctx: &Ctx) {
    clear_marker();
    start(ctx, false);
}

/// Puts the progress overlay up and arms the message for the end of the run.
pub(super) fn begin_run(ctx: &Ctx, auto: bool) {
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.auto = auto;
        g.announce_pending = true;
    });
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_startup_hidden(false);
        model.set_startup_progress(0.0);
        model.set_startup_active(true);
    }
}

/// Moves the progress bar and the status line to `stage`.
pub(super) fn set_stage(ctx: &Ctx, stage: Stage) {
    let (adapter, with_self_check) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.adapter_name.clone(), g.with_self_check)
    });
    let label = gpu_startup::stage_label(stage, adapter.as_deref());
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_startup_step(label.as_str().into());
        model.set_startup_progress(gpu_startup::progress(stage, with_self_check));
    }
    match stage {
        Stage::Adapter | Stage::Limits => show(ctx, GpuState::Starting, &label, true),
        Stage::Compile { .. } | Stage::SelfCheck => show(ctx, GpuState::Testing, &label, true),
        Stage::Done => {}
    }
}

/// The run is over (passed, failed, blocked or given up): the overlay goes, the texts are
/// refreshed, and the message says what the app renders with.
pub(super) fn end_run(ctx: &Ctx) {
    let pending = GATE.with(|gate| std::mem::take(&mut gate.borrow_mut().announce_pending));
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_startup_active(false);
    }
    status::refresh(ctx);
    if pending {
        announce(ctx);
    }
}

/// An outcome that needed no run (off, no WebGPU, blocked): the message is still said.
fn announce_now(ctx: &Ctx) {
    GATE.with(|gate| gate.borrow_mut().announce_pending = true);
    end_run(ctx);
}

/// The message: what the app renders with, as a toast (the same sentence stays in the
/// Render panel and, shortened, in the status strip).
fn announce(ctx: &Ctx) {
    let pool = crate::workers::existing();
    let Some((summary, _)) = status::texts(ctx, pool.as_ref()) else {
        return;
    };
    let failed = ctx.ui.upgrade().is_some_and(|ui| {
        GpuState::from_code(ui.global::<RenderModel>().get_gpu_state()) == GpuState::Failed
    });
    let kind = if failed {
        MessageKind::Warning
    } else {
        MessageKind::Info
    };
    show_message(ctx, kind, &summary);
}

/// The setting was switched off while a run was going: the run is dropped quietly and the
/// markers it wrote are put back, so the next load does not read an ended run as a crash.
pub(super) fn abandon(ctx: &Ctx) {
    release_markers();
    GATE.with(|gate| gate.borrow_mut().announce_pending = false);
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<RenderModel>().set_startup_active(false);
    }
}

/// Releases the crash markers this page owns in its current phase
/// (`gpu_startup::release_markers`): the start-up marker while the worker has not answered,
/// the adapter's `Probing` record while the test or the compile runs (back to the state it
/// had). A phase that is over keeps its record. Never panics: the page may be going away.
fn release_markers() {
    super::ray::release_marker();
    let owned = GATE.with(|gate| {
        gate.try_borrow().ok().map(|g| {
            (
                gpu_startup::release_markers(g.starting, g.testing, g.admitting),
                g.adapter_key.clone(),
                g.record.clone(),
            )
        })
    });
    let Some((release, key, record)) = owned else {
        return;
    };
    if release.startup {
        clear_marker();
    }
    if let (AdapterMarker::Restore(state), Some(key)) = (release.adapter, key) {
        if state == AdmissionState::Unknown {
            admission::forget(&key);
        } else if let Some(mut rec) = record {
            rec.state = state;
            rec.reason = None;
            admission::save(&key, &rec);
        }
    }
}

/// Installs the `pagehide` listener once, for the page's lifetime (the closure is leaked
/// with `forget()`, as the unsaved-changes guard's is): reloading or closing the tab while
/// the run is still going ends it on purpose, so its markers are released and the next load
/// tests again instead of reading a lost GPU. A real GPU-process crash fires no such event
/// from the page, so its markers stay.
pub(super) fn install_pagehide() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_pagehide =
        Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| release_markers());
    if window
        .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref())
        .is_err()
    {
        crate::app::diagnostics::console_warn("could not install the GPU start-up pagehide guard");
    }
    on_pagehide.forget();

    // A page the browser kept in its back/forward cache comes back with its Workers and its
    // run, but `pagehide` cleared the markers: put them back for the rest of the run.
    let on_pageshow = Closure::<dyn FnMut(web_sys::PageTransitionEvent)>::new(
        move |event: web_sys::PageTransitionEvent| {
            if event.persisted() {
                rearm_markers();
            }
        },
    );
    if window
        .add_event_listener_with_callback("pageshow", on_pageshow.as_ref().unchecked_ref())
        .is_err()
    {
        crate::app::diagnostics::console_warn("could not install the GPU start-up pageshow guard");
    }
    on_pageshow.forget();
}

/// Writes again the markers [`release_markers`] cleared, for the phase that is still running
/// (`gpu_startup::rearm_markers`). Never panics.
fn rearm_markers() {
    super::ray::rearm_marker();
    let owned = GATE.with(|gate| {
        gate.try_borrow().ok().map(|g| {
            (
                gpu_startup::rearm_markers(g.starting, g.testing, g.admitting),
                g.adapter_key.clone(),
                g.record.clone(),
            )
        })
    });
    let Some((rearm, key, record)) = owned else {
        return;
    };
    if rearm.startup {
        let marker = AdmissionRecord {
            state: AdmissionState::Probing,
            when: iso8601_utc_from_epoch_ms(js_sys::Date::now()),
            ..AdmissionRecord::default()
        };
        admission::save(STARTUP_KEY, &marker);
    }
    if let (true, Some(key), Some(mut rec)) = (rearm.adapter, key, record) {
        rec.state = AdmissionState::Probing;
        admission::save(&key, &rec);
    }
}

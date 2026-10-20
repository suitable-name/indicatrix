//! WebGPU admission gate (section 6.5): per-adapter qualification, compilation benchmarking,
//! and admission state machine for GPU rendering in the browser.
//!
//! State transitions (`RenderModel.gpu-state`, [`GpuState`]):
//! - Off (`0`): GPU rendering is off, or the browser has none.
//! - Unknown (`1`): Adapter detected, never tested. "Test GPU" button visible.
//! - Testing (`2`): Benchmarking compilation of isotropic kernels + 16x16 self-check render
//!   (or, for an adapter that passed earlier, only the compile).
//! - Ok (`3`): Adapter passed testing. Eligible scenes route to GPU + CPU once the
//!   kernels were compiled in this page's worker (`admit`, below).
//! - Failed (`4`): Adapter failed compile benchmark, self-check, or was lost, or the last
//!   run was lost before it finished. Shows the reason + "Forget GPU Test Result" button
//!   when the worker can still be used.
//! - Starting (`5`): GPU rendering is on and the worker is starting.
//!
//! # The start-up run
//!
//! [`init`] runs the test while the page loads (the `startup` module): the GPU worker is
//! started, asked for its adapter, the kernels are compiled and a 16x16 test render runs,
//! with a progress overlay and a message at the end that says what the app renders with.
//! The pure decisions are in `indicatrix_web_core::gpu_startup` and tested natively; the
//! eligibility, record, adapter key and deadlines are in `indicatrix_web_core::gpu_admission`.
//!
//! # Compile steps and crash markers
//!
//! The start-up run, the "Test GPU" run and the `admit` step all send `GpuCompile` for the
//! whole isotropic kernel set. Each writes the `Probing` marker BEFORE the request and
//! rewrites the record when every kernel answered; a marker still there on the next page
//! load means the browser lost the GPU process, and [`init`] turns it into `Failed`, which
//! no run repeats by itself. The adapter is not known before the worker answers, so the
//! start-up run first writes a marker under `gpu_startup::STARTUP_KEY` and replaces it with
//! the adapter's own once it is known. All are watched by a page-side deadline that gives
//! the worker up (terminates it) when it stops answering.

mod admit;
mod ray;
pub use admit::admit_if_ready;
mod startup;
mod status;

use std::{cell::RefCell, collections::BTreeSet, time::Duration};

use indicatrix_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    gpu_admission::{
        self, AdapterFacts, ISOTROPIC_CLASS, ISOTROPIC_KERNELS_ORDER, SELF_CHECK_SCENE_ID,
        SELF_CHECK_SPP, STAGE_DEADLINE_MS,
    },
    gpu_startup::{self, AdapterPlan, GpuState, Stage},
    host::{GpuEvent, WorkerPool},
};
use slint::{ComponentHandle, Timer, TimerMode};

pub use status::{format_backend_text, update_backend_text};

use crate::{
    RenderModel,
    app::Ctx,
    storage::admission::{self, AdmissionRecord, AdmissionState},
};

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent phase flags of the gate (testing, admitting, starting, ...), each read and written on its own"
)]
struct GateState {
    /// The reduced device can run Ray-Traced but not the eight-buffer event kernels.
    ray_only_device: bool,
    adapter_key: Option<String>,
    record: Option<AdmissionRecord>,
    /// "Test GPU" is running (compile, then the self-check).
    testing: bool,
    /// The `admit` compile after a reload is running.
    admitting: bool,
    /// This page's GPU worker compiled the whole kernel set (by a test or by `admit`).
    compiled_ok: bool,
    compiled_kernels: BTreeSet<String>,
    failed_in_session: bool,
    /// The page-side deadline of the running step.
    deadline: Timer,
    /// The GPU worker was spawned and has not reported its adapter yet.
    starting: bool,
    /// The run may start the test by itself (page load, setting switched on).
    auto: bool,
    /// The end of the run says what the app renders with (a message).
    announce_pending: bool,
    /// The browser has no WebGPU.
    no_webgpu: bool,
    /// The run was blocked before the adapter was known (the start-up marker of the last
    /// run was still there); "Forget GPU Test Result" then starts it again.
    blocked_at_start: bool,
    /// The readable name of the adapter, once known.
    adapter_name: Option<String>,
    /// The running compile ends with the 16x16 test render.
    with_self_check: bool,
}

thread_local! {
    static GATE: RefCell<GateState> = RefCell::new(GateState::default());
}

/// Computes the unique admission storage key for this adapter and browser version (section
/// 6.5): FNV-1a 64 over the adapter's fields, the Chrome major version and the kernel set
/// hash (`indicatrix_web_core::gpu_admission::adapter_key`).
#[must_use]
pub fn compute_adapter_key(facts: &AdapterFacts<'_>) -> String {
    gpu_admission::adapter_key(
        facts,
        &get_chrome_major_version(),
        indicatrix::renderer::buffers::KERNEL_SET_HASH,
    )
}

fn get_chrome_major_version() -> String {
    web_sys::window()
        .and_then(|window| window.navigator().user_agent().ok())
        .map_or_else(
            || "0".to_string(),
            |ua| gpu_admission::chrome_major_from_user_agent(&ua),
        )
}

fn kernel_names() -> Vec<String> {
    ISOTROPIC_KERNELS_ORDER
        .iter()
        .map(|&s| s.to_string())
        .collect()
}

/// Shows the gate's state in the Render panel.
fn show(ctx: &Ctx, state: GpuState, text: &str, can_forget: bool) {
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RenderModel>();
        model.set_gpu_state(state.code());
        model.set_gpu_status_text(text.into());
        model.set_gpu_can_forget(can_forget);
    }
}

/// Stores the current record with `state` and `reason`, when the adapter is known.
fn save_record(state: AdmissionState, reason: Option<String>) -> f64 {
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        let key = g.adapter_key.clone();
        match (key, g.record.as_mut()) {
            (Some(key), Some(rec)) => {
                rec.state = state;
                rec.reason = reason;
                rec.when = iso8601_utc_from_epoch_ms(js_sys::Date::now());
                admission::save(&key, rec);
                rec.total_ms
            }
            _ => 0.0,
        }
    })
}

/// (Re)starts the page-side deadline of the running step.
fn restart_deadline(ctx: &Ctx) {
    let c = ctx.clone();
    GATE.with(|gate| {
        gate.borrow().deadline.start(
            TimerMode::SingleShot,
            Duration::from_millis(u64::from(STAGE_DEADLINE_MS)),
            move || on_deadline(&c),
        );
    });
}

fn stop_deadline() {
    GATE.with(|gate| gate.borrow().deadline.stop());
}

/// The worker stopped answering during a start, a test or an admission compile.
fn on_deadline(ctx: &Ctx) {
    let (starting, testing, admitting) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.starting, g.testing, g.admitting)
    });
    if starting {
        startup::fail_start(ctx, "the GPU worker did not answer in time", true);
    } else if testing {
        fail_gpu(ctx, "GPU test timed out");
    } else if admitting {
        fail_gpu(ctx, "GPU compile timed out");
    }
}

/// A test or compile failed: the verdict is stored, the worker is given up for this page
/// session, and the GPU is off.
fn fail_gpu(ctx: &Ctx, reason: &str) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = false;
        g.admitting = false;
        g.starting = false;
        g.compiled_ok = false;
        g.failed_in_session = true;
    });
    save_record(AdmissionState::Failed, Some(reason.to_string()));
    show(
        ctx,
        GpuState::Failed,
        &format!("GPU test failed: {reason}. Reload the page to try again."),
        false,
    );
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
        pool.render().terminate_gpu();
    }
    startup::end_run(ctx);
}

/// Initializes the GPU gate on page load (section 6.5): turns every crash marker left
/// behind in the `Probing` state into `Failed`, then runs the start-up test (or says why
/// not).
pub fn init(ctx: &Ctx) {
    let startup_key = admission::resolve_key(gpu_startup::STARTUP_KEY);
    for probing_key in admission::all_probing() {
        let mut rec = admission::load(&probing_key).unwrap_or_default();
        rec.state = AdmissionState::Failed;
        rec.reason = Some(if probing_key == startup_key {
            gpu_startup::REASON_STARTUP_LOST.to_string()
        } else {
            "the browser lost the GPU during the last test".to_string()
        });
        admission::save(&probing_key, &rec);
    }
    startup::install_pagehide();
    startup::start(ctx, true);
}

/// Whether the pool may start the GPU worker by itself when it is created: not in a browser
/// without WebGPU, and not after a start that was blocked (the worker's adapter request is
/// the step that run lost the GPU in).
#[must_use]
pub fn worker_allowed() -> bool {
    GATE.with(|gate| {
        let g = gate.borrow();
        !g.no_webgpu && !g.blocked_at_start
    })
}

/// Handles toggling "GPU Rendering": switching it on runs the same start-up test, switching
/// it off stops whatever runs and hands the render back to the CPU workers.
pub fn on_gpu_enabled_changed(ctx: &Ctx, enabled: bool) {
    if !enabled {
        ray::abandon();
        stop_deadline();
        startup::abandon(ctx);
        GATE.with(|gate| {
            let mut g = gate.borrow_mut();
            g.testing = false;
            g.admitting = false;
            g.starting = false;
            g.compiled_ok = false;
            g.no_webgpu = false;
            g.blocked_at_start = false;
        });
        show(ctx, GpuState::Off, "", true);
        if let Some(pool) = crate::workers::existing() {
            // Turning the GPU off hands a pass in flight back to the CPU workers.
            pool.render().set_gpu_admitted(false);
            pool.render().set_gpu_enabled(false);
        }
        status::refresh(ctx);
        return;
    }
    startup::start(ctx, true);
}

/// Handles events posted from the GPU worker to the host.
pub fn on_gpu_event(ctx: &Ctx, event: GpuEvent) {
    if ray::event(ctx, &event) {
        status::refresh(ctx);
        return;
    }
    handle_event(ctx, event);
    status::refresh(ctx);
}

/// The worker answered cleanly that it cannot be used (or was lost): the verdict is the
/// session's, the worker is given up and the start-up run ends.
fn give_up(ctx: &Ctx, text: &str) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.failed_in_session = true;
        g.starting = false;
        g.testing = false;
        g.admitting = false;
        g.compiled_ok = false;
    });
    // An answer, even a bad one, is not a lost browser: the start-up marker goes.
    startup::clear_marker();
    show(ctx, GpuState::Failed, text, false);
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
        pool.render().terminate_gpu();
    }
    startup::end_run(ctx);
}

fn handle_event(ctx: &Ctx, event: GpuEvent) {
    match event {
        GpuEvent::Adapter {
            vendor,
            architecture,
            device,
            description,
            backend,
            max_storage_buffers_per_stage,
            max_storage_binding,
            max_buffer,
        } => on_adapter(
            ctx,
            &AdapterFacts {
                vendor: &vendor,
                architecture: &architecture,
                device: &device,
                description: &description,
                backend: &backend,
                max_storage_buffers_per_stage,
                max_storage_binding,
                max_buffer,
            },
        ),
        GpuEvent::Unavailable { reason } => give_up(
            ctx,
            &format!("GPU unavailable: {reason}. Reload the page to try again."),
        ),
        GpuEvent::Compiled { kernel, ms, .. } => on_compiled(ctx, kernel, ms),
        GpuEvent::CompileFailed { kernel, reason, .. } => {
            let busy = GATE.with(|gate| {
                let g = gate.borrow();
                g.testing || g.admitting
            });
            if busy {
                fail_gpu(ctx, &format!("Kernel {kernel} failed to compile: {reason}"));
            }
        }
        GpuEvent::FullPassResult {
            scene_id,
            sums_valid,
            ..
        } => {
            let testing = GATE.with(|gate| gate.borrow().testing);
            if testing && scene_id == SELF_CHECK_SCENE_ID {
                if sums_valid {
                    on_test_success(ctx);
                } else {
                    fail_gpu(ctx, "Self-check render produced invalid pixels");
                }
            }
        }
        GpuEvent::PassDropped { scene_id } | GpuEvent::SceneRejected { scene_id, .. } => {
            let testing = GATE.with(|gate| gate.borrow().testing);
            if testing && scene_id == SELF_CHECK_SCENE_ID {
                fail_gpu(ctx, "The GPU worker could not run the self-check scene");
            }
        }
        GpuEvent::Lost { reason } => {
            stop_deadline();
            GATE.with(|gate| {
                let mut g = gate.borrow_mut();
                g.failed_in_session = true;
                g.starting = false;
                g.testing = false;
                g.admitting = false;
                g.compiled_ok = false;
            });
            save_record(
                AdmissionState::Failed,
                Some(format!("GPU device lost: {reason}")),
            );
            startup::clear_marker();
            show(
                ctx,
                GpuState::Failed,
                &format!("GPU lost: {reason}. Reload the page to try again."),
                false,
            );
            if let Some(pool) = crate::workers::existing() {
                pool.render().set_gpu_admitted(false);
                pool.render().terminate_gpu();
            }
            startup::end_run(ctx);
        }
    }
}

/// The GPU worker reported its adapter: check its limits, look up what is stored for it
/// and do what the start-up run is to do with it (`gpu_startup::adapter_plan`).
fn on_adapter(ctx: &Ctx, facts: &AdapterFacts<'_>) {
    ray::remember(facts);
    GATE.with(|gate| gate.borrow_mut().ray_only_device = facts.max_storage_buffers_per_stage < 8);
    let name = gpu_startup::adapter_display_name(facts);
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.starting = false;
        g.adapter_name = Some(name);
    });
    stop_deadline();
    startup::set_stage(ctx, Stage::Limits);

    if facts.max_storage_buffers_per_stage < 4 {
        give_up(
            ctx,
            &format!(
                "GPU unavailable (max storage buffers {} < 4)",
                facts.max_storage_buffers_per_stage
            ),
        );
        return;
    }

    let key = compute_adapter_key(facts);
    let rec = admission::load(&key).unwrap_or_default();
    let auto = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.adapter_key = Some(key);
        g.record = Some(rec.clone());
        g.compiled_ok = false;
        g.testing = false;
        g.admitting = false;
        g.auto
    });

    let Some(pool) = crate::workers::existing() else {
        return;
    };
    if ray::selected(ctx) {
        startup::clear_marker();
        ray::begin(ctx, &pool, false);
        return;
    }
    if GATE.with(|gate| gate.borrow().ray_only_device) {
        show(
            ctx,
            GpuState::Off,
            "This GPU device supports Ray-Traced; Path-Traced needs eight storage buffers.",
            false,
        );
        set_admitted(false);
        startup::clear_marker();
        startup::end_run(ctx);
        return;
    }
    match gpu_startup::adapter_plan(&rec, auto) {
        AdapterPlan::RunTest => begin_test(ctx, &pool),
        AdapterPlan::CompileOnly => start_admit_compile(ctx, &pool),
        AdapterPlan::WaitForUser => {
            show(ctx, GpuState::Unknown, "GPU not tested yet", true);
            set_admitted(false);
            startup::clear_marker();
            startup::end_run(ctx);
        }
        AdapterPlan::Blocked(reason) => {
            show(
                ctx,
                GpuState::Failed,
                &format!("GPU test failed: {reason}"),
                true,
            );
            set_admitted(false);
            startup::clear_marker();
            startup::end_run(ctx);
        }
    }
}

fn set_admitted(admitted: bool) {
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(admitted);
    }
}

/// One kernel answered a compile request (of a test or of `admit`).
fn on_compiled(ctx: &Ctx, kernel: String, ms: f64) {
    let (testing, admitting) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.testing, g.admitting)
    });
    if !testing && !admitting {
        return;
    }
    let total = ISOTROPIC_KERNELS_ORDER.len();
    let count = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        if let Some(rec) = g.record.as_mut() {
            rec.per_kernel_ms.insert(kernel.clone(), ms);
            rec.total_ms += ms;
            rec.max_ms = rec.max_ms.max(ms);
        }
        g.compiled_kernels.insert(kernel);
        g.compiled_kernels.len()
    });
    restart_deadline(ctx);
    startup::set_stage(ctx, Stage::Compile { done: count, total });
    if count < total {
        return;
    }
    if admitting {
        finish_admit(ctx);
        return;
    }
    // All kernels compiled: run the 16x16 self-check. The worker then holds the self-check
    // scene; the pool gives it the real one again before the next real pass.
    let Some(pool) = crate::workers::existing() else {
        return;
    };
    startup::set_stage(ctx, Stage::SelfCheck);
    if let Err(e) = pool.render().run_gpu_self_check(
        SELF_CHECK_SCENE_ID,
        gpu_admission::self_check_scene(),
        SELF_CHECK_SPP,
    ) {
        fail_gpu(ctx, &format!("Self-check dispatch failed: {e}"));
    }
}

/// Called when the user clicks "Test GPU".
pub fn on_gpu_test_clicked(ctx: &Ctx) {
    if ray::selected(ctx) {
        if let Some(pool) = crate::workers::existing() {
            ray::begin(ctx, &pool, true);
        }
        return;
    }
    if ray::busy() || GATE.with(|gate| gate.borrow().ray_only_device) {
        return;
    }
    let Some(pool) = crate::workers::existing() else {
        show(ctx, GpuState::Unknown, "Workers not ready", true);
        return;
    };
    let (key, busy) = GATE.with(|gate| {
        let g = gate.borrow();
        (g.adapter_key.clone(), g.testing || g.admitting)
    });
    if key.is_none() {
        show(ctx, GpuState::Unknown, "Waiting for GPU adapter...", true);
        return;
    }
    if busy || !pool.render().is_gpu_ready() {
        return;
    }
    // The test needs no scene of its own (it renders the built-in self-check scene), so it
    // does not wait for the first image as it did when it was only a button.
    startup::begin_run(ctx, true);
    begin_test(ctx, &pool);
}

/// Starts the full test of the known adapter: the crash marker first, then the compile
/// request, then (when every kernel answered) the self-check.
fn begin_test(ctx: &Ctx, pool: &WorkerPool) {
    let Some(key) = GATE.with(|gate| gate.borrow().adapter_key.clone()) else {
        return;
    };
    let rec = AdmissionRecord {
        state: AdmissionState::Probing,
        when: iso8601_utc_from_epoch_ms(js_sys::Date::now()),
        ..AdmissionRecord::default()
    };
    // Save probing state into storage BEFORE sending any compile request (section 6.5).
    admission::save(&key, &rec);
    // The adapter's own marker is in place: the start-up one is not needed any more.
    startup::clear_marker();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = true;
        g.compiled_kernels.clear();
        g.record = Some(rec);
        g.with_self_check = true;
    });

    startup::set_stage(
        ctx,
        Stage::Compile {
            done: 0,
            total: ISOTROPIC_KERNELS_ORDER.len(),
        },
    );
    restart_deadline(ctx);
    if let Err(e) = pool.render().send_gpu_compile(
        ISOTROPIC_CLASS,
        kernel_names(),
        gpu_admission::COMPILE_SOFT_MS,
        gpu_admission::COMPILE_HARD_MS,
    ) {
        fail_gpu(ctx, &format!("Could not send compile request: {e}"));
    }
}

/// Called when the user clicks "Forget GPU Test Result".
pub fn on_gpu_forget_clicked(ctx: &Ctx) {
    if GATE.with(|gate| gate.borrow().blocked_at_start) {
        // The adapter was never reached: forgetting the start-up marker starts the run
        // again (it reaches the adapter and stops at "GPU not tested yet" if it is new).
        startup::forget_and_restart(ctx);
        return;
    }
    if ray::selected(ctx) {
        ray::forget(ctx);
        return;
    }
    let (usable, running) = GATE.with(|gate| {
        let g = gate.borrow();
        (!g.failed_in_session, g.testing || g.admitting)
    });
    if !usable || running {
        // The worker is gone for this page session (or busy); only a reload can test again.
        return;
    }
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        if let Some(key) = &g.adapter_key {
            admission::forget(key);
        }
        g.compiled_ok = false;
        g.record = Some(AdmissionRecord::default());
    });
    show(ctx, GpuState::Unknown, "GPU not tested yet", true);
    set_admitted(false);
    status::refresh(ctx);
}

fn on_test_success(ctx: &Ctx) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.testing = false;
        // The whole kernel set was just compiled in this page's worker.
        g.compiled_ok = true;
    });
    let total_ms = save_record(AdmissionState::Ok, None);
    startup::set_stage(ctx, Stage::Done);
    show(
        ctx,
        GpuState::Ok,
        &format!("GPU test passed (total {:.1} s)", total_ms / 1000.0),
        true,
    );
    if let Some(pool) = crate::workers::existing() {
        admit_if_ready(ctx, &pool);
    }
    startup::end_run(ctx);
    if let Some(pool) = crate::workers::existing() {
        ray::begin(ctx, &pool, false);
    }
    crate::render::request_sync(ctx);
}

/// The `admit(scene)` step (section 6.5): with a stored `ok` record, the worker compiles
/// the kernel set under the same `Probing` marker and deadlines as "Test GPU" before the
/// first real pass. Only then is the GPU admitted.
fn start_admit_compile(ctx: &Ctx, pool: &WorkerPool) {
    let key = GATE.with(|gate| gate.borrow().adapter_key.clone());
    let Some(key) = key else {
        return;
    };
    let rec = GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        let mut rec = g.record.clone().unwrap_or_default();
        rec.state = AdmissionState::Probing;
        rec.per_kernel_ms.clear();
        rec.max_ms = 0.0;
        rec.total_ms = 0.0;
        rec.reason = None;
        rec.when = iso8601_utc_from_epoch_ms(js_sys::Date::now());
        g.admitting = true;
        g.compiled_kernels.clear();
        g.record = Some(rec.clone());
        g.with_self_check = false;
        rec
    });
    // The crash marker first, then the request.
    admission::save(&key, &rec);
    startup::clear_marker();
    startup::set_stage(
        ctx,
        Stage::Compile {
            done: 0,
            total: ISOTROPIC_KERNELS_ORDER.len(),
        },
    );
    restart_deadline(ctx);
    if let Err(e) = pool.render().send_gpu_compile(
        ISOTROPIC_CLASS,
        kernel_names(),
        gpu_admission::COMPILE_SOFT_MS,
        gpu_admission::COMPILE_HARD_MS,
    ) {
        fail_gpu(ctx, &format!("Could not send compile request: {e}"));
    }
}

/// Every kernel answered the `admit` compile: the marker goes back to `ok`.
fn finish_admit(ctx: &Ctx) {
    stop_deadline();
    GATE.with(|gate| {
        let mut g = gate.borrow_mut();
        g.admitting = false;
        g.compiled_ok = true;
    });
    let total_ms = save_record(AdmissionState::Ok, None);
    startup::set_stage(ctx, Stage::Done);
    show(
        ctx,
        GpuState::Ok,
        &format!("GPU test passed (total {:.1} s)", total_ms / 1000.0),
        true,
    );
    if let Some(pool) = crate::workers::existing() {
        admit_if_ready(ctx, &pool);
    }
    startup::end_run(ctx);
    if let Some(pool) = crate::workers::existing() {
        ray::begin(ctx, &pool, false);
    }
}

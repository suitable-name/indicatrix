//! Independent Ray-Traced admission; existing spectral records keep their keys and meaning.

use std::{cell::RefCell, time::Duration};

use indicatrix_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    gpu_admission::{self, AdapterFacts, AdmissionState, ProbeRecord},
    gpu_startup::{GpuState, Stage},
    host::{GpuEvent, WorkerPool},
};
use slint::{Timer, TimerMode};

use super::{GATE, admission, show, startup, status};
use crate::app::Ctx;

#[derive(Default)]
struct State {
    key: Option<String>,
    record: ProbeRecord,
    ready: bool,
    busy: bool,
    deadline: Timer,
}

thread_local! { static RAY: RefCell<State> = RefCell::new(State::default()); }

pub(super) fn selected(ctx: &Ctx) -> bool {
    ctx.state.borrow().settings.engine_spec().is_ray()
}

pub(super) fn remember(facts: &AdapterFacts<'_>) {
    let key = gpu_admission::whitted_adapter_key(
        facts,
        &super::get_chrome_major_version(),
        indicatrix::renderer::buffers::whitted::WHITTED_KERNEL_HASH,
    );
    let record = admission::load(&key).unwrap_or_default();
    RAY.with(|ray| {
        *ray.borrow_mut() = State {
            key: Some(key),
            record: ProbeRecord::new(record),
            ..State::default()
        }
    });
}

pub(super) fn ready() -> bool {
    RAY.with(|ray| ray.borrow().ready)
}

pub(super) fn busy() -> bool {
    RAY.with(|ray| ray.borrow().busy)
}

fn save(state: AdmissionState, reason: Option<String>) -> bool {
    RAY.with(|ray| {
        let mut ray = ray.borrow_mut();
        ray.record.settle(state, reason);
        ray.record.current.when = iso8601_utc_from_epoch_ms(js_sys::Date::now());
        ray.key
            .as_ref()
            .is_some_and(|key| admission::save(key, &ray.record.current))
    })
}

fn failed(ctx: &Ctx, reason: &str) {
    RAY.with(|ray| {
        let mut ray = ray.borrow_mut();
        ray.busy = false;
        ray.ready = false;
        ray.deadline.stop();
    });
    save(AdmissionState::Failed, Some(reason.to_owned()));
    if selected(ctx) {
        show(
            ctx,
            GpuState::Failed,
            &format!("Ray-Traced GPU test failed: {reason}"),
            true,
        );
        if let Some(pool) = crate::workers::existing() {
            pool.render().set_gpu_admitted(false);
        }
        startup::end_run(ctx);
    }
    status::refresh(ctx);
}

/// Queued after the event-wavefront test on a normal startup; independent when Ray-Traced
/// is selected, including an adapter that exposes fewer than eight storage buffers.
pub(super) fn begin(ctx: &Ctx, pool: &WorkerPool, explicit: bool) {
    if !ctx.state.borrow().settings.gpu_enabled || !pool.render().is_gpu_ready() {
        return;
    }
    // Both engines use one FIFO worker. A queued test must not spend its deadline
    // waiting for the other engine's compilation or self-check to finish.
    if GATE.with(|gate| {
        let gate = gate.borrow();
        gate.testing || gate.admitting || gate.starting
    }) {
        return;
    }
    let previous_failure = RAY.with(|ray| {
        let ray = ray.borrow();
        (ray.record.current.state == AdmissionState::Failed && !explicit).then(|| {
            ray.record
                .current
                .reason
                .clone()
                .unwrap_or_else(|| "a previous qualification failed".to_owned())
        })
    });
    if let Some(reason) = previous_failure {
        if selected(ctx) {
            show(
                ctx,
                GpuState::Failed,
                &format!("Ray-Traced GPU blocked: {reason}"),
                true,
            );
            startup::end_run(ctx);
        }
        return;
    }
    let blocked = RAY.with(|ray| {
        let ray = ray.borrow();
        ray.key.is_none()
            || ray.busy
            || (ray.ready && !explicit)
            || (ray.record.current.state == AdmissionState::Failed && !explicit)
    });
    if blocked {
        return;
    }
    RAY.with(|ray| {
        let mut ray = ray.borrow_mut();
        ray.busy = true;
        ray.ready = false;
        ray.record.begin();
    });
    if !save(AdmissionState::Probing, None) && !explicit {
        failed(
            ctx,
            "browser storage cannot remember a GPU crash; use Test GPU to qualify explicitly",
        );
        return;
    }
    if selected(ctx) {
        show(
            ctx,
            GpuState::Testing,
            "Testing Ray-Traced GPU against the CPU...",
            false,
        );
        startup::set_stage(ctx, Stage::Compile { done: 0, total: 1 });
    }
    let captured = ctx.clone();
    RAY.with(|ray| {
        ray.borrow().deadline.start(
            TimerMode::SingleShot,
            Duration::from_millis(u64::from(gpu_admission::STAGE_DEADLINE_MS)),
            move || {
                failed(
                    &captured,
                    "qualification timed out; reload to replace the GPU worker",
                );
                super::give_up(
                    &captured,
                    "Ray-Traced GPU timed out. Reload to replace the worker.",
                );
            },
        );
    });
    if let Err(error) = pool.render().send_gpu_compile(
        gpu_admission::WHITTED_CLASS,
        vec![gpu_admission::WHITTED_KERNEL.to_owned()],
        gpu_admission::COMPILE_SOFT_MS,
        gpu_admission::COMPILE_HARD_MS,
    ) {
        failed(ctx, &error);
    }
}

/// Consume only this kernel's answers; spectral events continue through the original gate.
pub(super) fn event(ctx: &Ctx, event: &GpuEvent) -> bool {
    match event {
        GpuEvent::Compiled { kernel, ms, .. } if kernel == gpu_admission::WHITTED_KERNEL => {
            let busy = RAY.with(|ray| ray.borrow().busy);
            if !busy {
                return true;
            }
            RAY.with(|ray| {
                let mut ray = ray.borrow_mut();
                ray.busy = false;
                ray.ready = true;
                ray.deadline.stop();
                ray.record.current.per_kernel_ms.insert(kernel.clone(), *ms);
                ray.record.current.max_ms = *ms;
                ray.record.current.total_ms = *ms;
            });
            save(AdmissionState::Ok, None);
            if selected(ctx) {
                show(
                    ctx,
                    GpuState::Ok,
                    "Ray-Traced GPU passed the CPU comparison",
                    true,
                );
                startup::clear_marker();
                startup::end_run(ctx);
            }
            if let Some(pool) = crate::workers::existing() {
                super::admit_if_ready(ctx, &pool);
            }
            true
        }
        GpuEvent::CompileFailed { kernel, reason, .. }
            if kernel == gpu_admission::WHITTED_KERNEL =>
        {
            if RAY.with(|ray| ray.borrow().busy) {
                failed(ctx, reason);
            }
            true
        }
        GpuEvent::Lost { reason } => {
            reset();
            save(AdmissionState::Failed, Some(reason.clone()));
            false
        }
        _ => false,
    }
}

/// An intentional page exit or GPU switch-off must not look like a GPU crash.
pub(super) fn release_marker() {
    RAY.with(|ray| {
        let Ok(ray) = ray.try_borrow() else { return };
        if ray.busy
            && let (Some(key), Some(record)) = (&ray.key, ray.record.released())
        {
            admission::save(key, record);
        }
    });
}

/// Back/forward-cache restoration resumes the same test, so protect it again.
pub(super) fn rearm_marker() {
    RAY.with(|ray| {
        let Ok(ray) = ray.try_borrow() else { return };
        if ray.busy
            && let (Some(key), Some(record)) = (&ray.key, ray.record.rearmed())
        {
            admission::save(key, record);
        }
    });
}

pub(super) fn abandon() {
    release_marker();
    RAY.with(|ray| ray.borrow_mut().record.abandon());
    reset();
}

pub(super) fn reset() {
    RAY.with(|ray| {
        let mut ray = ray.borrow_mut();
        ray.busy = false;
        ray.ready = false;
        ray.deadline.stop();
    });
}

pub(super) fn forget(ctx: &Ctx) {
    RAY.with(|ray| {
        let mut ray = ray.borrow_mut();
        if ray.busy {
            return;
        }
        if let Some(key) = &ray.key {
            admission::forget(key);
        }
        ray.record = ProbeRecord::default();
        ray.ready = false;
    });
    show(
        ctx,
        GpuState::Unknown,
        "Ray-Traced GPU not tested yet",
        true,
    );
    if let Some(pool) = crate::workers::existing() {
        pool.render().set_gpu_admitted(false);
    }
    status::refresh(ctx);
}

pub(super) fn admit(ctx: &Ctx, pool: &WorkerPool) {
    let spectral_busy = GATE.with(|gate| {
        let gate = gate.borrow();
        gate.testing || gate.admitting || gate.starting
    });
    if !spectral_busy {
        begin(ctx, pool, false);
    }
    let enabled = ctx.state.borrow().settings.gpu_enabled;
    pool.render()
        .set_gpu_admitted(enabled && ready() && pool.render().gpu_ineligibility_reason().is_none());
    status::refresh(ctx);
}

pub(super) fn reason() -> Option<String> {
    RAY.with(|ray| {
        let ray = ray.borrow();
        if ray.busy {
            Some("Ray-Traced GPU qualification in progress".to_owned())
        } else if ray.record.current.state == AdmissionState::Failed {
            ray.record.current.reason.clone()
        } else if !ray.ready {
            Some("Ray-Traced GPU not qualified in this session".to_owned())
        } else {
            None
        }
    })
}

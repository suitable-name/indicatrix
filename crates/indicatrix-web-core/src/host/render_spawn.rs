//! Starting the pool's Workers and its silence watchdog: the closures that carry a
//! Worker's `onmessage` / `error` events back into [`Inner`].

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use super::{
    handle::WorkerHandle,
    now_ms,
    picture_worker::PictureWorker,
    render_callbacks::{Callbacks, finish},
    render_pool::Inner,
    watchdog::Watchdog,
};
use crate::protocol::FromWorker;

/// The silence watchdog: every few seconds, Workers that went quiet while holding a
/// chunk are replaced.
pub(super) fn start_watchdog(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
) -> Option<Watchdog> {
    let weak = weak.clone();
    let callbacks = Rc::clone(callbacks);
    Watchdog::start(move || {
        let Some(inner) = weak.upgrade() else {
            return;
        };
        let deferred = inner.borrow_mut().check_silence(now_ms());
        finish(&callbacks, deferred);
    })
}

pub(super) fn spawn_render_worker(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
    index: u32,
    spawn_id: u64,
) -> Result<WorkerHandle, String> {
    let on_message = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: FromWorker| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner
                .borrow_mut()
                .on_message(index as usize, spawn_id, message);
            finish(&callbacks, deferred);
        }
    };
    let on_error = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: String| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner
                .borrow_mut()
                .on_crash(index as usize, spawn_id, &message);
            finish(&callbacks, deferred);
        }
    };
    WorkerHandle::spawn(on_message, on_error)
}

pub(super) fn spawn_picture_worker(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
    spawn_id: u64,
    worker_index: u32,
) -> Result<PictureWorker, String> {
    let on_message = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: FromWorker| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_picture_message(spawn_id, message);
            finish(&callbacks, deferred);
        }
    };
    let on_error = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: String| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_picture_crash(spawn_id, &message);
            finish(&callbacks, deferred);
        }
    };
    PictureWorker::spawn(spawn_id, worker_index, on_message, on_error)
}

pub(super) fn spawn_gpu_worker(
    weak: &Weak<RefCell<Inner>>,
    callbacks: &Rc<Callbacks>,
    spawn_id: u64,
) -> Result<WorkerHandle, String> {
    let on_message = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: FromWorker| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_gpu_message(spawn_id, message);
            finish(&callbacks, deferred);
        }
    };
    let on_error = {
        let weak = weak.clone();
        let callbacks = Rc::clone(callbacks);
        move |message: String| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let deferred = inner.borrow_mut().on_gpu_crash(spawn_id, &message);
            finish(&callbacks, deferred);
        }
    };
    WorkerHandle::spawn_gpu(on_message, on_error)
}

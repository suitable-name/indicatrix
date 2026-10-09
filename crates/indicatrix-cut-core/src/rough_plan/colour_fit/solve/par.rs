//! Indexed scoped-thread map: the results come back in task order whatever the number of threads,
//! so a fit does not depend on it.
//!
//! Every task is an independent computation (a start, a held-out
//! view), never a shared reduction.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The number of threads to use: `requested`, or the available parallelism when 0, never more
/// than `tasks`, never less than 1.
pub(super) fn thread_count(requested: usize, tasks: usize) -> usize {
    let wanted = if requested == 0 {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    } else {
        requested
    };
    wanted.min(tasks).max(1)
}

/// Runs `work(0..count)` on `threads` threads (the calling thread is one of them) and returns the
/// results in index order. Tasks not started because `cancel` was set come back as `None`.
pub(super) fn par_map<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: &(dyn Fn(usize) -> T + Sync),
) -> Vec<Option<T>> {
    let slots: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let pull = || {
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                break;
            }
            let result = work(index);
            *slots[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        }
    };
    let threads = thread_count(threads, count);
    std::thread::scope(|scope| {
        for _ in 1..threads {
            scope.spawn(pull);
        }
        pull();
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner))
        .collect()
}

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

/// The threads each of `tasks` concurrent tasks may spend inside (the pixel pass of a normal
/// equation build), so that tasks times inner threads stay within the `requested` width (all cores
/// when 0): 16 cores give 8 seed starts 2 threads each and 2 finalists 8 each. Never less than 1.
pub(super) fn inner_threads(requested: usize, tasks: usize) -> usize {
    let total = if requested == 0 {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    } else {
        requested
    };
    (total / thread_count(requested, tasks)).max(1)
}

/// Runs `work(0..count)` on `threads` threads (the calling thread is one of them) and returns the
/// results in index order. Tasks not started because `cancel` was set come back as `None`.
pub(super) fn par_map<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: &(dyn Fn(usize) -> T + Sync),
) -> Vec<Option<T>> {
    par_map_progress(count, threads, cancel, work, &mut |_| {})
}

/// [`par_map`] with a progress hook: `report(finished)` is called on the CALLING thread after
/// each task that thread finished itself, with the number of tasks finished so far by all
/// threads (so the reports never decrease). The results do not depend on the hook.
pub(super) fn par_map_progress<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: &(dyn Fn(usize) -> T + Sync),
    report: &mut dyn FnMut(usize),
) -> Vec<Option<T>> {
    let slots: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let pull = |report: &mut dyn FnMut(usize)| {
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
            let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
            report(done);
        }
    };
    let threads = thread_count(threads, count);
    std::thread::scope(|scope| {
        for _ in 1..threads {
            scope.spawn(|| pull(&mut |_| {}));
        }
        pull(report);
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner))
        .collect()
}

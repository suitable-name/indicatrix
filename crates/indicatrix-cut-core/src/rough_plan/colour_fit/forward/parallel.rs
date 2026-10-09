//! Indexed scoped-thread parallelism: the results come back in task order whatever the number of
//! threads, so a trace or evaluation is bitwise independent of it.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The number of threads to use: `requested`, or the available parallelism when 0, never more
/// than `tasks`, never less than 1.
#[must_use]
pub fn thread_count(requested: usize, tasks: usize) -> usize {
    let wanted = if requested == 0 {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    } else {
        requested
    };
    wanted.min(tasks).max(1)
}

fn pull<T: Send>(
    next: &AtomicUsize,
    done: &AtomicUsize,
    cancel: &AtomicBool,
    work: &(dyn Fn(usize) -> T + Sync),
    slots: &[Mutex<Option<T>>],
    mut report: Option<&mut dyn FnMut(usize)>,
) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let index = next.fetch_add(1, Ordering::Relaxed);
        if index >= slots.len() {
            break;
        }
        let result = work(index);
        *slots[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(report) = report.as_mut() {
            report(finished);
        }
    }
}

/// Runs `work(0..count)` on `threads` threads (the calling thread is one of them) and returns
/// the results in index order. Tasks not started because `cancel` was set come back as `None`.
///
/// `progress` is called on the calling thread with the number of finished tasks.
pub fn run_indexed<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: &(dyn Fn(usize) -> T + Sync),
    progress: &mut dyn FnMut(usize),
) -> Vec<Option<T>> {
    let slots: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let threads = thread_count(threads, count);
    std::thread::scope(|scope| {
        for _ in 1..threads {
            scope.spawn(|| pull(&next, &done, cancel, work, &slots, None));
        }
        pull(&next, &done, cancel, work, &slots, Some(progress));
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().unwrap_or_else(PoisonError::into_inner))
        .collect()
}

//! Scoped-thread helpers for the passes of a rollout and of an evaluation.
//!
//! The units of work are very unequal: a labelled 24-tier slot costs 28 candidate evaluations
//! while a 4-tier slot costs 5, and a design with many free tiers takes ten times as long to
//! evaluate as a small one. Contiguous chunks per thread would leave most threads waiting for the
//! slowest chunk, so work is handed out one unit at a time, to whichever thread is free: through
//! a shared counter ([`run_indexed`]) or a shared iterator over the items ([`run_items`]).
//!
//! Every unit writes its own result and the results come back in unit order, so what a pass
//! computes does not depend on the thread count or on the scheduling.

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, ScopedJoinHandle},
};

/// Threads worth spawning for `tasks` units of work: the core count, but never more than the
/// tasks (so 0 for no work).
#[must_use]
pub fn pass_threads(tasks: usize) -> usize {
    thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(tasks)
}

/// Joins the workers in spawn order, re-raising the panic of the first one that panicked.
fn join_all<R>(handles: Vec<ScopedJoinHandle<'_, Vec<(usize, R)>>>) -> Vec<Vec<(usize, R)>> {
    handles
        .into_iter()
        .map(|handle| match handle.join() {
            Ok(part) => part,
            Err(payload) => std::panic::resume_unwind(payload),
        })
        .collect()
}

/// Puts `(index, result)` pairs back in index order. Every index in `0..count` must appear
/// exactly once.
fn gather<R>(parts: Vec<Vec<(usize, R)>>, count: usize) -> Vec<R> {
    let mut slots: Vec<Option<R>> = (0..count).map(|_| None).collect();
    for (index, result) in parts.into_iter().flatten() {
        slots[index] = Some(result);
    }
    slots.into_iter().flatten().collect()
}

/// `work(i)` for every `i` in `0..tasks`, on up to `threads` scoped threads that pull the next
/// index from a shared counter. The results are returned in index order. With one thread (or one
/// task) everything runs on the calling thread.
pub fn run_indexed<R, F>(tasks: usize, threads: usize, work: F) -> Vec<R>
where
    R: Send,
    F: Fn(usize) -> R + Sync,
{
    if threads <= 1 || tasks <= 1 {
        return (0..tasks).map(work).collect();
    }
    let next = AtomicUsize::new(0);
    let parts = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.min(tasks))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let task = next.fetch_add(1, Ordering::Relaxed);
                        if task >= tasks {
                            return done;
                        }
                        done.push((task, work(task)));
                    }
                })
            })
            .collect();
        join_all(handles)
    });
    gather(parts, tasks)
}

/// `work(i, &mut items[i])` for every item, on up to `threads` scoped threads that take the
/// next item from a shared iterator, one at a time. The results are returned in item order. With
/// one thread (or one item) everything runs on the calling thread.
pub fn run_items<T, R, F>(items: &mut [T], threads: usize, work: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(usize, &mut T) -> R + Sync,
{
    let count = items.len();
    if threads <= 1 || count <= 1 {
        return items
            .iter_mut()
            .enumerate()
            .map(|(index, item)| work(index, item))
            .collect();
    }
    let queue = Mutex::new(items.iter_mut().enumerate());
    let parts = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.min(count))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        // The guard is a temporary of this statement: the lock is free again
                        // before the item is worked on.
                        let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
                        let Some((index, item)) = next else {
                            return done;
                        };
                        done.push((index, work(index, item)));
                    }
                })
            })
            .collect();
        join_all(handles)
    });
    gather(parts, count)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_thread_count_is_the_core_count_capped_by_the_work() {
        let cores = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        assert_eq!(pass_threads(0), 0);
        assert_eq!(pass_threads(1), 1);
        for tasks in [2, 7, 64, 1000] {
            assert_eq!(pass_threads(tasks), cores.min(tasks));
        }
    }

    #[test]
    fn indexed_results_come_back_in_index_order_for_any_thread_count() {
        let expected: Vec<usize> = (0..257).map(|i| i * 3 + 1).collect();
        for threads in [0, 1, 2, 8, 1000] {
            assert_eq!(
                run_indexed(257, threads, |i| i * 3 + 1),
                expected,
                "{threads}"
            );
        }
        assert_eq!(run_indexed(0, 4, |i| i), Vec::<usize>::new());
        assert_eq!(run_indexed(1, 4, |i| i + 5), [5]);
    }

    #[test]
    fn every_indexed_task_runs_exactly_once() {
        let runs = AtomicUsize::new(0);
        let results = run_indexed(100, 6, |i| {
            runs.fetch_add(1, Ordering::Relaxed);
            // Uneven work: the early tasks are the slow ones.
            if i < 4 {
                thread::sleep(Duration::from_millis(20));
            }
            i
        });
        assert_eq!(runs.load(Ordering::Relaxed), 100);
        assert_eq!(results, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn items_are_each_worked_on_once_and_the_results_follow_item_order() {
        for threads in [0, 1, 3, 16] {
            let mut items: Vec<usize> = (0..50).collect();
            let results = run_items(&mut items, threads, |index, item| {
                assert_eq!(index, *item);
                *item += 100;
                index * 2
            });
            assert_eq!(
                results,
                (0..50).map(|i| i * 2).collect::<Vec<_>>(),
                "{threads}"
            );
            assert_eq!(items, (100..150).collect::<Vec<_>>(), "{threads}");
        }
        let mut none: Vec<u8> = Vec::new();
        assert_eq!(run_items(&mut none, 4, |_, _| 0), Vec::<i32>::new());
    }

    #[test]
    fn a_slow_item_does_not_hold_up_the_others() {
        // One 100 ms item among many fast ones: with dynamic scheduling the fast ones all finish
        // on the other threads, so the order in which the items were taken is not contiguous.
        let taken = Mutex::new(Vec::new());
        let mut items: Vec<usize> = (0..40).collect();
        run_items(&mut items, 4, |index, _| {
            if index == 0 {
                thread::sleep(Duration::from_millis(100));
            }
            taken.lock().unwrap().push(index);
        });
        let taken = taken.into_inner().unwrap();
        assert_eq!(taken.len(), 40);
        // Item 0 finished last or nearly so; the thread that held it took no further items
        // while it slept.
        let position = taken.iter().position(|&i| i == 0).unwrap();
        assert!(position > 20, "item 0 finished at position {position}");
    }
}

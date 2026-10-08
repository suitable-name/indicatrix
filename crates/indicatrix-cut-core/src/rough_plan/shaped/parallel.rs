//! Scoped threads for the stages of the shaped and the block planner that split their work.
//!
//! A stage cut into jobs runs them on scoped threads and hands the results back in job
//! order, so the output never depends on the lane count or on thread timing. Progress
//! events from the lanes reach the caller's `on_progress` on the calling thread; when it
//! returns `false` a shared flag stops every lane at its next report. A single job runs on
//! the calling thread and spawns nothing, so a lane count of one is also the path for
//! targets without threads.

use std::{
    ops::Range,
    panic,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use crate::rough_plan::PlanProgress;

/// Stack size of a lane thread in bytes. Generous, since layout trees are built on it.
const LANE_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Where a lane reports progress; returns `false` once the plan is cancelled.
pub type Report<'a> = dyn FnMut(PlanProgress) -> bool + 'a;

/// `len` items split into at most `lanes` consecutive ranges of near-equal size, none
/// empty.
#[must_use]
pub fn even_chunks(len: usize, lanes: usize) -> Vec<Range<usize>> {
    let lanes = lanes.clamp(1, len.max(1));
    let size = len.div_ceil(lanes);
    (0..lanes)
        .map(|lane| (lane * size)..((lane + 1) * size).min(len))
        .filter(|chunk| chunk.start < chunk.end)
        .collect()
}

/// The `a0` slices of the clipped table for `lanes` threads.
///
/// Plane `a0` holds `planes - a0` ranges of the a axis, so equal plane counts would leave
/// the first lane with about twice the average work. The slices are consecutive, cover
/// `0..planes` and each ends where the running range count first reaches its share.
#[must_use]
pub fn balanced_slices(planes: usize, lanes: usize) -> Vec<Range<usize>> {
    let lanes = lanes.clamp(1, planes.max(1));
    let total = planes * (planes + 1) / 2;
    let before = |plane: usize| plane * planes - plane * plane.saturating_sub(1) / 2;
    let mut slices = Vec::with_capacity(lanes);
    let mut start = 0;
    for lane in 1..=lanes {
        let mut end = start;
        while end < planes && before(end) * lanes < total * lane {
            end += 1;
        }
        if end > start {
            slices.push(start..end);
            start = end;
        }
    }
    slices
}

/// Runs `work` once per job and returns the results in job order; `None` when a job
/// gave up or `on_progress` cancelled.
///
/// With more than one job every job gets its own scoped thread, all started before any is
/// joined. A job reports through the callback it is given: the event travels to
/// `on_progress`, which runs on the calling thread, and the callback returns `false` once
/// `on_progress` has cancelled. A single job runs on the calling thread with `on_progress`
/// itself. A panic in a job is re-raised on the calling thread.
pub fn run_lanes<J, T, F>(
    jobs: Vec<J>,
    work: F,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<T>>
where
    J: Send,
    T: Send,
    F: Fn(J, &mut Report<'_>) -> Option<T> + Sync,
{
    if jobs.len() <= 1 {
        return jobs
            .into_iter()
            .map(|job| work(job, &mut *on_progress))
            .collect();
    }
    let stop = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<PlanProgress>();
    thread::scope(|scope| {
        let work = &work;
        let stop = &stop;
        let handles: Vec<_> = jobs
            .into_iter()
            .enumerate()
            .map(|(lane, job)| {
                let sender = sender.clone();
                thread::Builder::new()
                    .name(format!("rough-plan-lane-{lane}"))
                    .stack_size(LANE_STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        let mut report = |event: PlanProgress| {
                            !stop.load(Ordering::Relaxed) && sender.send(event).is_ok()
                        };
                        work(job, &mut report)
                    })
                    .expect("the operating system could not start a planner thread")
            })
            .collect();
        drop(sender);

        // The channel closes when the last lane ends, also by a panic.
        let mut cancelled = false;
        for event in receiver {
            if !cancelled && !on_progress(event) {
                cancelled = true;
                stop.store(true, Ordering::Relaxed);
            }
        }
        let results: Vec<Option<T>> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| panic::resume_unwind(payload))
            })
            .collect();
        if cancelled {
            None
        } else {
            results.into_iter().collect()
        }
    })
}

/// [`run_lanes`] on at most `lanes` threads that pull their jobs from a shared counter.
///
/// Same contract as [`run_lanes`]: results in job order whatever the timing, events forwarded
/// to `on_progress` on the calling thread, a shared flag that stops every lane once it has
/// cancelled, a panic re-raised on the calling thread. Instead of one thread per job,
/// `min(lanes, jobs)` threads each take the next unstarted job index from an atomic counter,
/// so the jobs start in ascending order (put the heavy ones first) and a lane that finishes
/// early takes more. A job that gives up (`None`) stops the other lanes too and the result is
/// `None`. `lanes <= 1` or a single job runs inline like [`run_lanes`].
pub fn run_lanes_dynamic<J, T, F>(
    jobs: Vec<J>,
    lanes: usize,
    work: F,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<T>>
where
    J: Send,
    T: Send,
    F: Fn(J, &mut Report<'_>) -> Option<T> + Sync,
{
    if lanes <= 1 || jobs.len() <= 1 {
        return jobs
            .into_iter()
            .map(|job| work(job, &mut *on_progress))
            .collect();
    }
    let count = jobs.len();
    let threads = lanes.min(count);
    let slots: Vec<Mutex<Option<J>>> = jobs.into_iter().map(|job| Mutex::new(Some(job))).collect();
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<PlanProgress>();
    thread::scope(|scope| {
        let work = &work;
        let stop = &stop;
        let next = &next;
        let slots = &slots;
        let handles: Vec<_> = (0..threads)
            .map(|lane| {
                let sender = sender.clone();
                thread::Builder::new()
                    .name(format!("rough-plan-lane-{lane}"))
                    .stack_size(LANE_STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        let mut report = |event: PlanProgress| {
                            !stop.load(Ordering::Relaxed) && sender.send(event).is_ok()
                        };
                        let mut done: Vec<(usize, T)> = Vec::new();
                        loop {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            if index >= count {
                                break;
                            }
                            let job = slots[index]
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .take()
                                .expect("every job index is handed out once");
                            if let Some(result) = work(job, &mut report) {
                                done.push((index, result));
                            } else {
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        done
                    })
                    .expect("the operating system could not start a planner thread")
            })
            .collect();
        drop(sender);

        // The channel closes when the last lane ends, also by a panic.
        let mut cancelled = false;
        for event in receiver {
            if !cancelled && !on_progress(event) {
                cancelled = true;
                stop.store(true, Ordering::Relaxed);
            }
        }
        let mut results: Vec<(usize, T)> = Vec::with_capacity(count);
        for handle in handles {
            match handle.join() {
                Ok(done) => results.extend(done),
                Err(payload) => panic::resume_unwind(payload),
            }
        }
        if cancelled || results.len() != count {
            return None;
        }
        results.sort_unstable_by_key(|&(index, _)| index);
        Some(results.into_iter().map(|(_, result)| result).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(planes: usize, slice: &Range<usize>) -> usize {
        slice.clone().map(|plane| planes - plane).sum()
    }

    #[test]
    fn even_chunks_tile_the_items_without_gaps_or_empty_parts() {
        for len in [0, 1, 2, 5, 7, 12, 100] {
            for lanes in [1, 2, 3, 8, 200] {
                let chunks = even_chunks(len, lanes);
                assert!(chunks.len() <= lanes.max(1));
                let mut cursor = 0;
                for chunk in &chunks {
                    assert_eq!(chunk.start, cursor, "len {len}, lanes {lanes}");
                    assert!(chunk.end > chunk.start);
                    cursor = chunk.end;
                }
                assert_eq!(cursor, len, "len {len}, lanes {lanes}");
            }
        }
    }

    #[test]
    fn balanced_slices_tile_the_planes_and_share_the_range_count() {
        for planes in [1, 2, 3, 8, 16, 20] {
            for lanes in [1, 2, 3, 4, 8] {
                let slices = balanced_slices(planes, lanes);
                let mut cursor = 0;
                for slice in &slices {
                    assert_eq!(slice.start, cursor, "planes {planes}, lanes {lanes}");
                    assert!(slice.end > slice.start);
                    cursor = slice.end;
                }
                assert_eq!(cursor, planes, "planes {planes}, lanes {lanes}");
                assert!(slices.len() <= lanes.max(1));

                // No lane carries more than its share plus one plane's weight.
                let total = planes * (planes + 1) / 2;
                let ideal = total.div_ceil(slices.len().max(1));
                for slice in &slices {
                    assert!(
                        weight(planes, slice) <= ideal + planes,
                        "planes {planes}, lanes {lanes}, slice {slice:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn run_lanes_returns_the_results_in_job_order_whatever_the_thread_timing() {
        let jobs: Vec<u64> = (0..8).collect();
        let results = run_lanes(
            jobs,
            |job, _report| {
                // The early jobs take longest, so they finish last.
                thread::sleep(std::time::Duration::from_millis((8 - job) * 3));
                Some(job * job)
            },
            &mut |_| true,
        );
        assert_eq!(results, Some(vec![0, 1, 4, 9, 16, 25, 36, 49]));
    }

    #[test]
    fn run_lanes_forwards_every_event_to_the_calling_thread() {
        let caller = thread::current().id();
        let mut seen = 0;
        let results = run_lanes(
            vec![0_usize, 1, 2],
            |job, report| {
                for _ in 0..5 {
                    if !report(PlanProgress::Pareto) {
                        return None;
                    }
                }
                Some(job)
            },
            &mut |_| {
                assert_eq!(thread::current().id(), caller);
                seen += 1;
                true
            },
        );
        assert_eq!(results, Some(vec![0, 1, 2]));
        assert_eq!(seen, 15);
    }

    #[test]
    fn a_cancel_from_the_callback_stops_every_lane_and_gives_none() {
        let results = run_lanes(
            vec![0_usize, 1, 2],
            |_job, report| {
                // Runs until the callback's cancel reaches this lane.
                while report(PlanProgress::Pareto) {}
                None::<usize>
            },
            &mut |_| false,
        );
        assert_eq!(results, None);
    }

    #[test]
    fn a_single_job_runs_inline_with_the_callback_itself() {
        let caller = thread::current().id();
        let results = run_lanes(
            vec![7_usize],
            |job, report| {
                assert_eq!(thread::current().id(), caller);
                assert!(
                    !report(PlanProgress::Pareto),
                    "the callback's false is passed on"
                );
                Some(job)
            },
            &mut |_| false,
        );
        assert_eq!(results, Some(vec![7]));
    }

    #[test]
    fn run_lanes_dynamic_returns_the_results_in_job_order_for_any_lane_count() {
        for lanes in [1, 3, 8, 20] {
            let jobs: Vec<u64> = (0..12).collect();
            let results = run_lanes_dynamic(
                jobs,
                lanes,
                |job, _report| {
                    // The early jobs take longest, so they finish last.
                    thread::sleep(std::time::Duration::from_millis((12 - job) * 2));
                    Some(job * job)
                },
                &mut |_| true,
            );
            let want: Vec<u64> = (0..12).map(|job| job * job).collect();
            assert_eq!(results, Some(want), "lanes {lanes}");
        }
    }

    #[test]
    fn run_lanes_dynamic_hands_the_jobs_out_in_ascending_order_to_at_most_lanes_threads() {
        let started = Mutex::new(Vec::new());
        let threads = Mutex::new(Vec::new());
        let results = run_lanes_dynamic(
            (0..30_usize).collect(),
            3,
            |job, _report| {
                started.lock().unwrap().push(job);
                threads.lock().unwrap().push(thread::current().id());
                thread::sleep(std::time::Duration::from_millis(1));
                Some(job)
            },
            &mut |_| true,
        );
        assert_eq!(results, Some((0..30).collect::<Vec<_>>()));
        let threads = threads.into_inner().unwrap();
        let distinct = threads
            .iter()
            .enumerate()
            .filter(|(i, id)| !threads[..*i].contains(id))
            .count();
        assert!(distinct <= 3);
        assert!(!threads.contains(&thread::current().id()));
        assert_eq!(started.into_inner().unwrap().len(), 30);
    }

    #[test]
    fn run_lanes_dynamic_forwards_every_event_to_the_calling_thread() {
        let caller = thread::current().id();
        for lanes in [2, 8] {
            let mut seen = 0;
            let results = run_lanes_dynamic(
                (0..6_usize).collect(),
                lanes,
                |job, report| {
                    for _ in 0..5 {
                        if !report(PlanProgress::Pareto) {
                            return None;
                        }
                    }
                    Some(job)
                },
                &mut |_| {
                    assert_eq!(thread::current().id(), caller);
                    seen += 1;
                    true
                },
            );
            assert_eq!(results, Some((0..6).collect::<Vec<_>>()), "lanes {lanes}");
            assert_eq!(seen, 30, "lanes {lanes}");
        }
    }

    #[test]
    fn a_cancel_stops_every_dynamic_lane_and_gives_none() {
        for lanes in [2, 5] {
            let results = run_lanes_dynamic(
                (0..9_usize).collect(),
                lanes,
                |_job, report| {
                    // Runs until the callback's cancel reaches this lane.
                    while report(PlanProgress::Pareto) {}
                    None::<usize>
                },
                &mut |_| false,
            );
            assert_eq!(results, None, "lanes {lanes}");
        }
    }

    #[test]
    fn a_single_dynamic_job_or_lane_runs_inline() {
        let caller = thread::current().id();
        for (jobs, lanes) in [(vec![7_usize], 4), (vec![1, 2, 3], 1)] {
            let want = jobs.clone();
            let results = run_lanes_dynamic(
                jobs,
                lanes,
                |job, _report| {
                    assert_eq!(thread::current().id(), caller);
                    Some(job)
                },
                &mut |_| true,
            );
            assert_eq!(results, Some(want));
        }
    }

    #[test]
    #[should_panic(expected = "job failed")]
    fn a_panic_in_a_dynamic_job_reaches_the_caller() {
        let _ = run_lanes_dynamic(
            vec![0_usize, 1, 2, 3],
            3,
            |job, _report| {
                assert!(job != 1, "job failed");
                Some(job)
            },
            &mut |_| true,
        );
    }

    #[test]
    #[should_panic(expected = "job failed")]
    fn a_panic_in_a_job_reaches_the_caller() {
        let _ = run_lanes(
            vec![0_usize, 1, 2],
            |job, _report| {
                assert!(job != 1, "job failed");
                Some(job)
            },
            &mut |_| true,
        );
    }
}

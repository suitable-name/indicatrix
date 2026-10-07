//! The parallel building blocks of a plan: the six cut orders, the planes of the clipped
//! piece table, the chunks of the single-stone fit and the refinement of the best layouts.
//!
//! Each block runs one of the core's single-threaded stages on scoped threads. Work items
//! are handed out through an atomic index, so an early finisher takes the next item, and
//! the parts are merged by item index. The result therefore depends neither on the lane
//! count nor on thread timing.

use super::{
    WORKER_STACK_BYTES,
    tracker::{Note, Progress},
};
use glam::DVec3;
use indicatrix_cut_core::rough_plan::{
    CandidateDesign, CutOrder, DesignHull, FitMesh, FitStage, PieceTable, PlanProgress,
    PlanSettings, RoughLayout, RoughMesh, SingleFit, fit_shortlisted_with, merge_fits,
    screen_designs_with,
    shaped::{BuildClipParams, ClippedTable, ShapedGrid, a_range_jobs, build_clipped_a_range},
    shortlist,
};
use std::{
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread::ScopedJoinHandle,
};

/// How many chunks of designs each lane gets on average. More chunks than lanes let the
/// lane that draws the cheap designs take more of them.
const CHUNKS_PER_LANE: usize = 4;

/// Lowers the calling thread below normal priority, so a plan that uses every spare core
/// leaves the window's own thread and the rest of the desktop responsive.
#[cfg(windows)]
pub(super) fn lower_thread_priority() {
    use std::ffi::c_void;

    /// The Win32 `THREAD_PRIORITY_BELOW_NORMAL` level.
    const BELOW_NORMAL: i32 = -1;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn SetThreadPriority(thread: *mut c_void, priority: i32) -> i32;
    }

    // SAFETY: `GetCurrentThread` returns a pseudo handle that is valid on the calling
    // thread and needs no closing; `SetThreadPriority` only reads its two arguments.
    let lowered = unsafe { SetThreadPriority(GetCurrentThread(), BELOW_NORMAL) } != 0;
    if !lowered {
        tracing::debug!("Rough planner: could not lower a planner thread's priority");
    }
}

/// Lowers the calling thread below normal priority. Only Windows schedules the planner's
/// threads this way; elsewhere the thread keeps its priority.
#[cfg(not(windows))]
pub(super) const fn lower_thread_priority() {}

/// The result of a scoped thread; a panic in the thread is re-raised on the caller.
pub(super) fn join_or_resume<T>(handle: ScopedJoinHandle<'_, T>) -> T {
    match handle.join() {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

/// One lane of [`run_dynamic`]: takes the next free index until none is left, the run was
/// stopped or `work` refuses to go on. A panic in `work` stops the run, calls `abort`
/// and is re-raised.
fn lane_loop<T, W>(
    count: usize,
    next: &AtomicUsize,
    stopped: &AtomicBool,
    abort: &(dyn Fn() + Sync),
    work: &W,
) -> Vec<(usize, T)>
where
    W: Fn(usize) -> Option<T>,
{
    lower_thread_priority();
    let mut done = Vec::new();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        while !stopped.load(Ordering::Relaxed) {
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                break;
            }
            if let Some(value) = work(index) {
                done.push((index, value));
            } else {
                stopped.store(true, Ordering::Relaxed);
                break;
            }
        }
    }));
    if let Err(payload) = outcome {
        stopped.store(true, Ordering::Relaxed);
        abort();
        resume_unwind(payload);
    }
    done
}

/// Runs `work(index)` for every index in `0..count` on up to `lanes` scoped threads with
/// the worker stack size and returns the results in index order.
///
/// Indices are handed out in ascending order through an atomic counter, so the load
/// balances itself and the result does not depend on the lane count. `work` returns `None`
/// to stop the run (a cancel): no further index is taken and the result is `None`. A panic
/// in `work` calls `abort` so sibling lanes stop at their next poll, and is re-raised here,
/// into the worker's `catch_unwind`.
fn run_dynamic<T, W>(
    count: usize,
    lanes: usize,
    abort: &(dyn Fn() + Sync),
    work: W,
) -> Option<Vec<T>>
where
    T: Send,
    W: Fn(usize) -> Option<T> + Sync,
{
    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let (next_ref, stopped_ref, work_ref) = (&next, &stopped, &work);
    let per_lane = std::thread::scope(|scope| {
        // Every lane is started before any is joined, so they run concurrently.
        let mut handles = Vec::new();
        for lane in 0..lanes.clamp(1, count.max(1)) {
            let handle = std::thread::Builder::new()
                .name(format!("rough-plan-lane-{lane}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    lane_loop(count, next_ref, stopped_ref, abort, work_ref)
                })
                .expect("the operating system could not start a planner thread");
            handles.push(handle);
        }
        handles.into_iter().map(join_or_resume).collect::<Vec<_>>()
    });
    if stopped.load(Ordering::Relaxed) {
        return None;
    }
    let mut done: Vec<(usize, T)> = per_lane.into_iter().flatten().collect();
    done.sort_unstable_by_key(|(index, _)| *index);
    Some(done.into_iter().map(|(_, value)| value).collect())
}

/// `len` items split into at most `lanes` consecutive ranges of near-equal size, none
/// empty.
pub(super) fn even_chunks(len: usize, lanes: usize) -> Vec<Range<usize>> {
    let lanes = lanes.clamp(1, len.max(1));
    let size = len.div_ceil(lanes);
    (0..lanes)
        .map(|lane| (lane * size)..((lane + 1) * size).min(len))
        .filter(|chunk| chunk.start < chunk.end)
        .collect()
}

/// The chunks a stage over `len` designs is cut into for `lanes` lanes.
fn design_chunks(len: usize, lanes: usize) -> Vec<Range<usize>> {
    even_chunks(len, lanes.saturating_mul(CHUNKS_PER_LANE))
}

/// The cut-order DPs on up to `min(6, lanes)` scoped threads. `work` runs one order and
/// reports through the callback it is given. `None` if any order was cancelled; layouts
/// are concatenated in `CutOrder::ALL` order.
pub(super) fn run_orders<W>(
    lanes: usize,
    progress: &dyn Progress,
    work: W,
) -> Option<Vec<RoughLayout>>
where
    W: Fn(CutOrder, &mut dyn FnMut(PlanProgress) -> bool) -> Option<Vec<RoughLayout>> + Sync,
{
    let per_order = run_dynamic(
        CutOrder::ALL.len(),
        lanes.min(CutOrder::ALL.len()),
        &|| progress.abort(),
        |index| {
            let result = work(CutOrder::ALL[index], &mut |event| progress.event(event));
            progress.note(Note::OrderFinished);
            result
        },
    )?;
    Some(per_order.into_iter().flatten().collect())
}

/// What a clipped piece table is built from.
pub(super) struct ClipJob<'a> {
    /// The unit grid over the rough.
    pub grid: &'a ShapedGrid,
    /// The Pareto front the table scores.
    pub front: &'a [CandidateDesign],
    /// The cut, prism and pebble planes (not the bounding-box faces).
    pub non_box: &'a [(DVec3, f64)],
    /// The size-only table of interior pieces, built from `front`.
    pub size_table: &'a PieceTable,
    /// The plan's settings.
    pub settings: &'a PlanSettings,
    /// The rough's non-convex mesh, `None` for a convex rough.
    pub mesh: Option<&'a RoughMesh>,
}

/// Builds the clipped piece table one a-range at a time on up to `lanes` threads, handing
/// the ranges out through an atomic index. Every job holds the same number of entries and
/// reports `grid_poll_events(per_a)` `Grid` events. `None` when cancelled.
pub(super) fn build_clipped_table_parallel(
    job: &ClipJob<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<ClippedTable> {
    let ranges = a_range_jobs(job.grid);
    let parts = run_dynamic(ranges.len(), lanes, &|| progress.abort(), |index| {
        let (a0, a1) = ranges[index];
        let params = BuildClipParams {
            grid: job.grid,
            front: job.front,
            non_box_planes: job.non_box,
            mesh: job.mesh,
            size_table: job.size_table,
            settings: job.settings,
            slice: a0..a0 + 1,
            cached_classes: None,
        };
        build_clipped_a_range(&params, (a0, a1), &mut |event| progress.event(event))
    })?;
    // The blocks are merged in job order, so their concatenation is the full table.
    Some(ClippedTable::concat(job.grid.cells, parts))
}

/// What the single-stone fit is run on.
pub(super) struct FitJob<'a> {
    /// The fine region, already inset by skin and allowance.
    pub region: &'a [(DVec3, f64)],
    /// The cheaper region the screening runs against.
    pub coarse_region: &'a [(DVec3, f64)],
    /// The designs' convex outlines, sorted by entry id.
    pub hulls: &'a [DesignHull],
    /// The plan's settings.
    pub settings: &'a PlanSettings,
    /// How many fits to keep.
    pub keep: usize,
    /// The rough's non-convex mesh with its clearance, `None` for a convex rough.
    pub mesh: Option<FitMesh<'a>>,
}

/// Screens every chunk of the hulls; the scores come back concatenated in hull order.
fn screen_in_chunks(
    job: &FitJob<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<(i64, f64)>> {
    let chunks = design_chunks(job.hulls.len(), lanes);
    let parts = run_dynamic(chunks.len(), lanes, &|| progress.abort(), |index| {
        screen_designs_with(
            job.coarse_region,
            &job.hulls[chunks[index].clone()],
            job.settings,
            job.mesh,
            &mut |event| progress.event(event),
        )
    })?;
    Some(parts.into_iter().flatten().collect())
}

/// Runs the exact search and the polish on the shortlisted designs, chunk by chunk.
fn fit_in_chunks(
    job: &FitJob<'_>,
    subset: &[DesignHull],
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<SingleFit>> {
    let chunks = design_chunks(subset.len(), lanes);
    let parts = run_dynamic(chunks.len(), lanes, &|| progress.abort(), |index| {
        fit_shortlisted_with(
            job.region,
            &subset[chunks[index].clone()],
            job.settings,
            job.mesh,
            &mut |event| progress.event(event),
        )
    })?;
    Some(parts.into_iter().flatten().collect())
}

/// The exact single-stone fit spread over `lanes` threads: screening per chunk of the
/// designs, one global shortlist, the exact search per chunk of the shortlisted designs,
/// then one merge. Equals the core's sequential `fit_single_stones` for any lane count.
/// `None` when cancelled.
pub(super) fn fit_single_stones_parallel(
    job: &FitJob<'_>,
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<SingleFit>> {
    if job.hulls.is_empty() || job.keep == 0 {
        return Some(Vec::new());
    }
    progress.note(Note::Fit(FitStage::Screen, job.hulls.len()));
    let scores = screen_in_chunks(job, lanes, progress)?;

    let mut chosen = shortlist(&scores, job.keep);
    chosen.sort_unstable();
    let subset: Vec<DesignHull> = job
        .hulls
        .iter()
        .filter(|hull| chosen.binary_search(&hull.entry_id).is_ok())
        .cloned()
        .collect();
    progress.note(Note::Fit(FitStage::Exact, subset.len()));
    progress.note(Note::Fit(FitStage::Polish, subset.len()));

    let fits = fit_in_chunks(job, &subset, lanes, progress)?;
    Some(merge_fits(fits, job.keep))
}

/// Runs `work(slot)` for every slot in `0..count` on up to `lanes` threads and returns the
/// results in slot order. A `Refine` event is reported before each slot starts; `None` if
/// one of them asks to stop.
pub(super) fn refine_parallel<W>(
    count: usize,
    lanes: usize,
    progress: &dyn Progress,
    work: W,
) -> Option<Vec<RoughLayout>>
where
    W: Fn(usize) -> RoughLayout + Sync,
{
    run_dynamic(count, lanes, &|| progress.abort(), |slot| {
        progress.event(PlanProgress::Refine).then(|| work(slot))
    })
}

#[cfg(test)]
mod tests {
    use super::{even_chunks, run_dynamic};
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

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
    fn run_dynamic_returns_the_results_in_index_order_whatever_the_thread_timing() {
        for lanes in [1, 3, 8, 20] {
            let results = run_dynamic(8, lanes, &|| {}, |index| {
                // The early indices take longest, so they finish last.
                std::thread::sleep(std::time::Duration::from_millis((8 - index as u64) * 3));
                Some(index * index)
            });
            assert_eq!(
                results,
                Some(vec![0, 1, 4, 9, 16, 25, 36, 49]),
                "lanes = {lanes}"
            );
        }
    }

    #[test]
    fn run_dynamic_over_nothing_is_an_empty_result() {
        assert_eq!(run_dynamic(0, 4, &|| {}, Some), Some(Vec::<usize>::new()));
    }

    #[test]
    fn a_refusal_stops_the_run_and_hands_out_no_further_index() {
        // One lane takes the indices in order: 0, 1 and 2 run, the refusal at 2 ends it.
        let calls = AtomicUsize::new(0);
        let result = run_dynamic(10, 1, &|| {}, |index| {
            calls.fetch_add(1, Ordering::Relaxed);
            (index != 2).then_some(index)
        });
        assert_eq!(result, None);
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_panic_in_a_job_reaches_the_caller_and_aborts_the_siblings() {
        let aborted = AtomicBool::new(false);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run_dynamic(3, 2, &|| aborted.store(true, Ordering::Relaxed), |index| {
                assert!(index != 1, "job failed");
                Some(index)
            })
        }));
        assert!(outcome.is_err(), "the panic is re-raised to the caller");
        assert!(aborted.load(Ordering::Relaxed), "the abort hook ran");
    }
}

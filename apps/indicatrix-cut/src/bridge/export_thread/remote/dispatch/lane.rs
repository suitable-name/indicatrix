//! [`run_remote_lane`]: the remote lane's own claim/dispatch/merge loop for the whole
//! concurrent phase, plus its consecutive-failure backoff.

use super::{
    super::{
        capability::RemoteCapability,
        rate::{remote_chunk_samples, shortfall},
    },
    progress::RemoteProgress,
    run_batch::run_remote_batch,
};
use crate::bridge::export_thread::sample_cursor::SampleCursor;
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Consecutive remote CHUNK failures [`run_remote_lane`] tolerates before it PAUSES
/// remote (`ComputeTarget::Both` only) -- see [`remote_retry_backoff`]. `2`, not `1`: a
/// single dropped connection is common enough (one network blip) that pausing after
/// just one would waste real remaining capacity; two in a row is past "transient".
pub(super) const MAX_CONSECUTIVE_REMOTE_FAILURES: u32 = 2;

/// The first pause [`run_remote_lane`] takes once remote has failed
/// [`MAX_CONSECUTIVE_REMOTE_FAILURES`] chunks in a row; each further consecutive
/// failure doubles it, up to [`REMOTE_RETRY_BACKOFF_MAX`].
pub(super) const REMOTE_RETRY_BACKOFF_INITIAL: Duration = Duration::from_secs(15);
pub(super) const REMOTE_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(120);

/// How long to pause the remote lane after `consecutive_failures` failed chunks in a
/// row (called only once that count has reached [`MAX_CONSECUTIVE_REMOTE_FAILURES`]):
/// 15 s, 30 s, 60 s, 120 s, 120 s, ... Remote is never written off for the rest of an
/// export -- a worker that was unreachable for a minute (a wireless roam, a machine
/// that was busy with something else, a transient timeout) is offered work again as
/// soon as the pause ends, and every chunk it then completes counts. Local keeps
/// claiming from the shared cursor throughout, so a pause costs the export nothing but
/// remote's own share of throughput while it lasts.
pub(super) fn remote_retry_backoff(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(MAX_CONSECUTIVE_REMOTE_FAILURES);
    let scaled = REMOTE_RETRY_BACKOFF_INITIAL.saturating_mul(1u32 << doublings.min(8));
    scaled.min(REMOTE_RETRY_BACKOFF_MAX)
}

/// Sleeps for `total`, returning early with `false` the moment `cancel` is raised or
/// `cursor`'s shared pool runs dry (local has claimed everything that was left -- no
/// point holding the export open for a pause with nothing to hand remote afterwards).
/// `true` means the full pause elapsed with work still available.
pub(super) fn pause_remote_lane(
    cursor: &SampleCursor,
    cancel: &AtomicBool,
    total: Duration,
) -> bool {
    let started = Instant::now();
    while started.elapsed() < total {
        if cancel.load(Ordering::Relaxed) || cursor.shared_pool_exhausted() {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}

/// [`run_remote_lane`]'s result once its claim loop has permanently ended.
pub(in crate::bridge::export_thread) struct RemoteLaneOutcome {
    /// `Some` iff the lane ended in a state that must fail the WHOLE export outright
    /// -- only possible when `fallback_to_local` was `false` (`ComputeTarget::RemoteOnly`)
    /// and a chunk came back short: no local lane is running to pick up the unfinished
    /// remainder, so finishing anyway would produce a silently incomplete image.
    /// `run_export` turns this into `ExportOutcome::Failed` rather than writing a
    /// partial image.
    pub(in crate::bridge::export_thread) fatal: Option<String>,
    /// This lane's current best throughput estimate (samples/sec) at the moment its
    /// claim loop ended -- whatever `initial_rate` was seeded with, updated by every
    /// chunk that measured a fresher one. `render_accumulation` carries this forward
    /// into `super::super::super::worker::AccumulationCarry::remote_rate` so a caller
    /// rendering many frames of the same scene (the tilt performance video) seeds the
    /// NEXT frame's remote lane from here instead of re-running a calibration probe.
    pub(in crate::bridge::export_thread) final_rate: Option<f64>,
}

/// Runs the REMOTE lane for the whole concurrent phase: repeatedly sizes a chunk from
/// the current best rate estimate (see [`remote_chunk_samples`]), claims it from
/// `cursor`, dispatches it via [`run_remote_batch`], merges whatever prefix completed
/// into `progress`, and loops -- until `cursor` has nothing left to claim, the export
/// is cancelled, remote fails too many times in a row (`Both` only), or (`RemoteOnly`
/// only) a single failed chunk makes the whole export unrecoverable.
///
/// # Why every chunk gets its OWN fresh `Accumulator`
///
/// `Accumulator::begin_request` ZEROES the accumulator's buffer and resets its
/// `samples_done` on every dispatch -- it is designed for one session's single active
/// request, not for accumulating several requests' results on top of each other.
/// Reusing one `Accumulator` across chunks would silently discard every earlier
/// chunk's radiance. So each chunk gets a brand new `Accumulator`, and this function
/// itself sums each chunk's contribution into `progress`'s persistent buffer exactly
/// once, right after that chunk ends.
///
/// # `fallback_to_local` and `remote_lane_done`
///
/// `fallback_to_local` is `true` only for `ComputeTarget::Both`: it gates whether a
/// short chunk's unfinished remainder is handed to `cursor.return_to_local` for local
/// to pick up, or turns the whole export into an outright failure (`RemoteOnly`, which
/// has no local lane to hand anything to).
///
/// `remote_lane_done` is set to `true` as the VERY LAST thing this function does, after
/// every `cursor.return_to_local` call it will ever make has already happened --
/// `batch::run_local_batches` depends on that ordering to know it's safe to stop
/// waiting for further retries.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one remote lane's own identity \
              (the shared cursor, worker capability, scene, resolution, the initial \
              rate estimate, the cross-thread progress/notes state, whether a failed \
              chunk may fall back to local, and cancellation/completion signalling) -- \
              bundling them into a struct would just move the same count into field \
              access, not reduce it"
)]
pub(in crate::bridge::export_thread) fn run_remote_lane(
    cursor: &SampleCursor,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    initial_rate: f64,
    progress: &RemoteProgress,
    fallback_to_local: bool,
    cancel: &AtomicBool,
    remote_lane_done: &AtomicBool,
) -> RemoteLaneOutcome {
    let outcome = run_remote_lane_claim_loop(
        cursor,
        capability,
        scene_state,
        width,
        height,
        initial_rate,
        progress,
        fallback_to_local,
        cancel,
    );
    // See "`fallback_to_local` and `remote_lane_done`" above for why this must be the
    // LAST thing this function does.
    remote_lane_done.store(true, Ordering::Release);
    outcome
}

/// Folds one finished chunk's radiance into `progress`'s merged buffer and adds its
/// `done` samples to the traced count. A chunk with `done == 0` contributes nothing
/// and is skipped; `done` is always the valid prefix `run_remote_batch` reports.
fn merge_chunk_into_progress(
    progress: &RemoteProgress,
    chunk_accumulator: &Mutex<Accumulator>,
    done: u32,
) {
    if done == 0 {
        return;
    }
    let chunk = chunk_accumulator
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut merged = progress
        .accum
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    for (dst, src) in merged.iter_mut().zip(chunk.buffer()) {
        *dst += *src;
    }
    drop(merged);
    drop(chunk);
    progress.traced.fetch_add(done, Ordering::Relaxed);
}

#[expect(
    clippy::too_many_arguments,
    reason = "see `run_remote_lane`'s own identical `#[expect]` -- this is that \
              function's claim loop, split out only so the `remote_lane_done` \
              store-on-every-exit-path guarantee lives in exactly one place (the \
              wrapper) rather than being duplicated at every `return` this loop has"
)]
fn run_remote_lane_claim_loop(
    cursor: &SampleCursor,
    capability: &RemoteCapability,
    scene_state: &SceneState,
    width: u32,
    height: u32,
    initial_rate: f64,
    progress: &RemoteProgress,
    fallback_to_local: bool,
    cancel: &AtomicBool,
) -> RemoteLaneOutcome {
    let mut rate = initial_rate;
    let mut consecutive_failures = 0u32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        }

        let Some((start, count)) = cursor.claim(remote_chunk_samples(rate)) else {
            // Nothing left in the shared pool -- a normal, successful end to this
            // lane's work (this only ever inspects the shared pool, never the
            // local-only retry pile, which is `claim_local`'s alone).
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        };

        let chunk_accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
        *progress
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&chunk_accumulator));
        let (done, cancelled, error, measured_rate) = run_remote_batch(
            capability,
            scene_state.clone(),
            start,
            count,
            width,
            height,
            &chunk_accumulator,
            cancel,
        );
        *progress
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;

        merge_chunk_into_progress(progress, &chunk_accumulator, done);

        if let Some(measured) = measured_rate {
            rate = measured;
        }

        if cancelled {
            // `run_export`'s own top-level `cancel` check discards the whole export
            // regardless of what got done here -- nothing further to decide.
            return RemoteLaneOutcome {
                fatal: None,
                final_rate: Some(rate),
            };
        }

        let missing = shortfall(count, done);
        if missing == 0 {
            consecutive_failures = 0;
            continue;
        }

        let message = error.as_deref().unwrap_or("connection ended early");
        if !fallback_to_local {
            // `RemoteOnly`: no local lane exists to hand this remainder to -- see
            // `RemoteLaneOutcome::fatal`'s own doc comment.
            return RemoteLaneOutcome {
                fatal: Some(format!(
                    "Remote worker failed ({message}) -- {missing} of {count} samples in \
                     this chunk were never traced, and this export has no local fallback \
                     (Compute: Remote)."
                )),
                final_rate: Some(rate),
            };
        }

        cursor.return_to_local(start + done, missing);
        progress
            .notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(format!(
                "Remote worker failed ({message}) partway through a chunk -- the \
                 remaining {missing} samples will finish locally. Every sample it \
                 completed first is still included."
            ));
        consecutive_failures += 1;
        if consecutive_failures >= MAX_CONSECUTIVE_REMOTE_FAILURES {
            let pause = remote_retry_backoff(consecutive_failures);
            progress
                .notes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(format!(
                    "Remote worker failed {consecutive_failures} chunks in a row -- \
                     pausing it for {}s before offering it more work; rendering \
                     continues locally in the meantime.",
                    pause.as_secs()
                ));
            if !pause_remote_lane(cursor, cancel, pause) {
                // Cancelled, or local finished everything during the pause -- either
                // way there is nothing left for remote to claim.
                return RemoteLaneOutcome {
                    fatal: None,
                    final_rate: Some(rate),
                };
            }
        }
    }
}

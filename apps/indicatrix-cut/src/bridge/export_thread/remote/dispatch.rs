//! Dispatching against a remote worker: one `RenderRequest` ([`run_remote_batch`]),
//! building its `SceneState` ([`scene_state_from_snapshot`]), the cross-thread progress
//! state a running lane publishes ([`RemoteProgress`]), and the lane itself that
//! repeatedly claims and dispatches chunks for the whole concurrent phase
//! ([`run_remote_lane`]).
//!
//! # Timeouts and liveness (export side)
//!
//! [`run_remote_batch`] applies [`LIVENESS_TIMEOUT`]/[`FIRST_EVENT_TIMEOUT`] to its own
//! `rx.recv_timeout` polling loop, mirroring
//! `bridge::remote::remote_render::connection`'s identical pair one level down the
//! stack: the connection thread applies the same two-tier deadline to the actual
//! socket and forwards every event down an unbounded channel this loop merely drains.
//! The thread running this loop does nothing between chunks but claim the next one,
//! dispatch it, and sum the result -- tens of milliseconds even at 4K -- so
//! `last_update` is never compared against a stale value from being off doing
//! something else. The real, reproduced false positive was the FIRST wait's deadline
//! being too tight for a coarse worker cadence, which [`FIRST_EVENT_TIMEOUT`] fixes.

use super::{
    capability::RemoteCapability,
    rate::{remote_chunk_samples, remote_marginal_rate, shortfall},
};
use crate::bridge::{
    export_thread::{sample_cursor::SampleCursor, scene_snapshot::SceneSnapshot},
    remote::remote_render::{self, RemoteRenderRequest, RemoteUpdate},
};
use glam::Vec3;
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

/// A single fixed id: every remote dispatch this module makes opens its OWN fresh
/// connection (`spawn_remote_render` calls `connect_and_handshake` itself), so nothing
/// is ever pipelined behind something else on the same socket the way the live
/// viewport's `next_request_id` counter has to guard against. Reusing `1` across
/// separate connections is therefore safe.
const REQUEST_ID: u32 = 1;

/// Mirrors `bridge::remote::remote_render::connection`'s own `LIVENESS_TIMEOUT` (same
/// value/reasoning) but kept as its own constant: this is a ONE-SHOT dispatch's idle
/// channel, not the persistent connection's socket-level polling loop, and the two
/// deliberately don't share an implementation. Without it, a worker that accepted the
/// request and then never sent another update would hang this call, and therefore the
/// whole remote lane, forever.
///
/// Applies once this dispatch has seen at least one [`RemoteUpdate`] -- see
/// [`FIRST_EVENT_TIMEOUT`] for the longer grace given to the wait for that first one.
const LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

/// Mirrors `bridge::remote::remote_render::connection`'s own `FIRST_EVENT_TIMEOUT`.
///
/// # The false positive this closes
///
/// Confirmed against a real export: at 4K with a worker cadence of 20 samples/tick, a
/// calibration probe can legitimately take longer than [`LIVENESS_TIMEOUT`] to produce
/// its first [`RemoteUpdate`] while genuinely still computing. Before this constant
/// existed, `run_remote_batch`'s `last_update` clock was judged against the single,
/// tighter `LIVENESS_TIMEOUT` for that entire first wait, so a calibration probe could
/// be reported as "worker silent" while busy.
const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// The slowest link this watchdog assumes when budgeting time for one full-resolution
/// `FRAME` to arrive: 4 MiB/s (~34 Mbit/s), well under a congested wireless link.
/// Only the allowance's SIZE depends on it; a genuinely dead worker is still detected,
/// just `frame_bytes / this` later at most (~25 s at 4K, ~6 s at 1080p).
const MIN_ASSUMED_LINK_BYTES_PER_SEC: u64 = 4 * 1024 * 1024;

/// How much longer than the base deadline a wait may legitimately last while one
/// `frame_bytes`-sized payload is in flight. `run_remote_batch`'s only signal of life
/// is a complete `RemoteUpdate`, and the connection thread produces one for a `FRAME`
/// only after the WHOLE payload has been read -- so during a large transfer nothing
/// reaches `rx` even though bytes are flowing. Without this allowance a 4K frame
/// (~100 MB) crossing a link slower than ~100 Mbit/s was reported as "worker silent"
/// mid-transfer.
const fn transfer_allowance(frame_bytes: u64) -> Duration {
    Duration::from_secs(frame_bytes.div_ceil(MIN_ASSUMED_LINK_BYTES_PER_SEC))
}

/// Which deadline currently applies to an idle wait on `rx`: mirrors
/// `bridge::remote::remote_render::connection::liveness_deadline`, plus the
/// [`transfer_allowance`] for a payload of `frame_bytes` (one full-resolution `FRAME`
/// at the request's dimensions), since a wait here can span an entire frame transfer.
const fn liveness_deadline(seen_first_update: bool, frame_bytes: u64) -> Duration {
    let base = if seen_first_update {
        LIVENESS_TIMEOUT
    } else {
        FIRST_EVENT_TIMEOUT
    };
    base.saturating_add(transfer_allowance(frame_bytes))
}

/// How long [`run_remote_batch`] waits for the worker's `DONE { cancelled: true }`
/// confirmation after sending a cancel before giving up and returning anyway. Without
/// this, a worker that never acknowledges the cancel would hang this call -- and
/// therefore the whole export -- waiting for a confirmation that never arrives.
const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Builds the `indicatrix_net::SceneState` a remote worker needs from an export's own
/// [`SceneSnapshot`] plus its `width x height` -- the export-side equivalent of
/// `gui::remote::orchestrator::tick::scene_state_from_snapshot`, duplicated since that
/// function is private to a different module tree.
#[must_use]
pub(in crate::bridge::export_thread) fn scene_state_from_snapshot(
    snapshot: &SceneSnapshot,
    width: u32,
    height: u32,
) -> SceneState {
    SceneState {
        width,
        height,
        yaw: snapshot.yaw,
        pitch: snapshot.pitch,
        distance: snapshot.distance,
        light_yaw: snapshot.light_yaw,
        light_pitch: snapshot.light_pitch,
        exposure: snapshot.exposure,
        max_bounces: snapshot.max_bounces,
        lighting_preset: snapshot.lighting_preset,
        material: snapshot.material.clone(),
        planes: snapshot.active_planes.clone(),
        girdle_frosted: !snapshot.facet_finishes.is_empty(),
        backdrop: snapshot.backdrop,
    }
}

/// Runs one `RenderRequest` against `capability.worker`, covering `[first_sample,
/// first_sample + samples)` at `width x height`, blocking until it finishes, fails, or
/// `cancel` is observed -- in which case [`remote_render::RemoteRenderHandle::cancel`]
/// is sent and this waits for the worker's own `DONE { cancelled: true }` confirmation
/// rather than abandoning the connection outright.
///
/// `accumulator` is caller-provided so a caller running this on a background thread can
/// peek at its live `buffer()`/`samples_done()` from another thread while this call is
/// still in flight -- `run_export` uses that for its export-progress preview.
///
/// `spawn_remote_render` owns the socket on its own thread and reports progress via a
/// callback; this wraps that in a plain channel so the caller can wait on it
/// synchronously -- unlike the live-viewport orchestrator, which drives the same
/// `spawn_remote_render` from UI-thread callbacks that must never block.
///
/// Returns `(samples_done, cancelled, error, rate_samples_per_sec)`: `samples_done` is
/// always exactly what the accumulator ended up holding (a valid prefix) regardless of
/// how this ended; `error` is `Some` only when the request failed outright, never
/// merely because it was cancelled or ran short.
///
/// `rate_samples_per_sec` is remote's measured STEADY-STATE throughput for this
/// dispatch (see [`remote_marginal_rate`]). `None` when there isn't enough signal to
/// trust one: no update ever reported progress, or the span collapsed to nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one RenderRequest's own identity \
              (worker capability, scene, sample range, resolution, the shared \
              accumulator, cancellation) -- bundling them into a struct would just move \
              the same count into field access, not reduce it, and `RemoteRenderRequest` \
              already plays that role one level down in `bridge::remote::remote_render`"
)]
pub(in crate::bridge::export_thread) fn run_remote_batch(
    capability: &RemoteCapability,
    scene: SceneState,
    first_sample: u32,
    samples: u32,
    width: u32,
    height: u32,
    accumulator: &Arc<Mutex<Accumulator>>,
    cancel: &AtomicBool,
) -> (u32, bool, Option<String>, Option<f64>) {
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    let handle = remote_render::spawn_remote_render(
        RemoteRenderRequest {
            worker: capability.worker.clone(),
            request_id: REQUEST_ID,
            scene,
            first_sample,
            samples,
            width,
            height,
        },
        Arc::clone(accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    // The Instant/samples_done pair at the FIRST update that reported real progress --
    // the starting point `remote_marginal_rate` measures from, excluding connection
    // setup/handshake/upload since none of that repeats on a real dispatch.
    let mut first_progress: Option<(Instant, u32)> = None;
    let mut cancel_sent = false;
    // `Some` from the moment `cancel()` is sent -- once set, an idle poll is judged
    // against `CANCEL_WAIT_TIMEOUT` instead of `LIVENESS_TIMEOUT`.
    let mut cancel_sent_at: Option<Instant> = None;
    let mut last_update = Instant::now();
    // Whether ANY `RemoteUpdate` has arrived yet -- selects `FIRST_EVENT_TIMEOUT`
    // (before) or `LIVENESS_TIMEOUT` (after) via `liveness_deadline`.
    let mut first_update_seen = false;
    // One full-resolution `FRAME` payload at this request's dimensions -- what a single
    // idle wait on `rx` may legitimately have in flight (see `transfer_allowance`).
    let frame_bytes =
        u64::from(width) * u64::from(height) * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
    loop {
        if !cancel_sent && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancel_sent = true;
            cancel_sent_at = Some(Instant::now());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(update) => {
                last_update = Instant::now();
                first_update_seen = true;
                match update {
                    RemoteUpdate::Done { cancelled, .. } => {
                        let samples_done = accumulator
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .samples_done();
                        let rate = first_progress.and_then(|(first_instant, first_samples)| {
                            remote_marginal_rate(
                                samples_done.saturating_sub(first_samples),
                                Instant::now().saturating_duration_since(first_instant),
                            )
                        });
                        return (samples_done, cancelled, None, rate);
                    }
                    RemoteUpdate::Failed { message, .. } => {
                        let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                        return (acc.samples_done(), false, Some(message), None);
                    }
                    RemoteUpdate::Frame { samples_done, .. }
                    | RemoteUpdate::Progress { samples_done, .. } => {
                        if first_progress.is_none() && samples_done > 0 {
                            first_progress = Some((Instant::now(), samples_done));
                        }
                    }
                    // Nothing terminal -- the accumulator already reflects every Frame
                    // as it's applied (`Accumulator::apply`).
                    RemoteUpdate::Connected { .. } | RemoteUpdate::Preview { .. } => {}
                }
            }
            // Nothing arrived within this poll: bounded by `CANCEL_WAIT_TIMEOUT` if a
            // cancel was already sent, by `LIVENESS_TIMEOUT` otherwise.
            Err(RecvTimeoutError::Timeout) => match cancel_sent_at {
                Some(sent_at) if sent_at.elapsed() > CANCEL_WAIT_TIMEOUT => {
                    // The worker never confirmed the cancel -- return as cancelled
                    // anyway rather than hang the export on a confirmation that may
                    // never come.
                    tracing::warn!(
                        "remote render: worker never confirmed cancellation within \
                         {CANCEL_WAIT_TIMEOUT:?} -- returning with samples completed so \
                         far"
                    );
                    let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                    return (acc.samples_done(), true, None, None);
                }
                None if last_update.elapsed()
                    > liveness_deadline(first_update_seen, frame_bytes) =>
                {
                    // No update for longer than the applicable deadline, and
                    // cancellation was never requested -- presume the worker dead.
                    let message = format!(
                        "worker silent for {:.0?} -- treating as failed",
                        last_update.elapsed()
                    );
                    tracing::warn!("remote render: {message}");
                    let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                    return (acc.samples_done(), false, Some(message), None);
                }
                // Still within whichever wait applies -- keep polling.
                Some(_) | None => {}
            },
            Err(RecvTimeoutError::Disconnected) => {
                // The worker thread ended without a terminal update (likely a panic
                // inside it) -- treat it as a failure so the caller falls back to local.
                let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                return (
                    acc.samples_done(),
                    false,
                    Some("remote render worker thread ended unexpectedly".to_string()),
                    None,
                );
            }
        }
    }
}

/// Cross-thread state [`run_remote_lane`] publishes for `run_export`'s progress-
/// reporting closure to read WHILE the lane is still running -- a lane runs many
/// sequential chunk dispatches, each needing its OWN fresh `Accumulator` (see
/// [`run_remote_lane`] for why one can't be reused across chunks).
///
/// Every field is behind `Mutex`/`AtomicU32` interior mutability so this can be shared
/// as a plain `&RemoteProgress` between the remote lane's thread and the thread
/// driving local -- no `Arc` needed, since both threads live only for the duration of
/// the `thread::scope` call in `run_export` that this outlives.
pub(in crate::bridge::export_thread) struct RemoteProgress {
    /// The sum of every chunk this lane has FULLY merged so far -- like `gpu_accum`,
    /// only ever folded into the export's own `accum` ONCE, after the concurrent phase
    /// has completely ended (merging earlier would race local's CPU threads, which
    /// write to overlapping pixel indices).
    accum: Mutex<Vec<Vec3>>,
    /// How many samples are folded into `accum` above. Kept alongside it so progress
    /// reporting is one atomic load, not a buffer scan every tick.
    traced: AtomicU32,
    /// The chunk currently in flight, if any -- `Some` only while a
    /// [`run_remote_batch`] call is in progress. Lets the progress closure show live
    /// sub-chunk progress rather than one that only advances in ~22-second jumps.
    in_flight: Mutex<Option<Arc<Mutex<Accumulator>>>>,
    /// User-facing notes this lane produced mid-export -- queued since more than one
    /// can happen over a long export. `run_export`'s progress closure drains at most
    /// one per tick via [`take_note`](Self::take_note).
    notes: Mutex<VecDeque<String>>,
}

impl RemoteProgress {
    pub(in crate::bridge::export_thread) fn new(pixel_count: usize) -> Self {
        Self {
            accum: Mutex::new(vec![Vec3::ZERO; pixel_count]),
            traced: AtomicU32::new(0),
            in_flight: Mutex::new(None),
            notes: Mutex::new(VecDeque::new()),
        }
    }

    /// Total samples this lane has traced so far: every fully-merged chunk, plus
    /// whatever the in-flight chunk has already streamed back. Never double-counts a
    /// chunk that finishes between the two reads below -- at worst slightly
    /// UNDER-reports for one tick, never over-, since `run_remote_lane` always clears
    /// `in_flight` before adding to `traced`.
    pub(in crate::bridge::export_thread) fn samples_done(&self) -> u32 {
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let live = in_flight.as_ref().map_or(0, |acc| {
            acc.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .samples_done()
        });
        drop(in_flight);
        self.traced.load(Ordering::Relaxed) + live
    }

    /// A live combined preview buffer: every fully-merged chunk's radiance plus the
    /// in-flight chunk's current buffer, pixel-summed. Always returns a full
    /// `width * height` buffer -- an export with no remote contribution yet is simply
    /// all zero, exactly like `gpu_accum` before the GPU's first batch lands.
    pub(in crate::bridge::export_thread) fn preview_buffer(&self) -> Vec<Vec3> {
        let mut buf = self
            .accum
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(acc) = in_flight.as_ref() {
            let acc = acc.lock().unwrap_or_else(PoisonError::into_inner);
            for (dst, src) in buf.iter_mut().zip(acc.buffer()) {
                *dst += *src;
            }
        }
        drop(in_flight);
        buf
    }

    /// Drains the OLDEST queued note, if any. `run_export`'s progress closure calls
    /// this once per tick so a mid-export chunk failure surfaces as a toast.
    pub(in crate::bridge::export_thread) fn take_note(&self) -> Option<String> {
        self.notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// Consumes `self` and returns everything this lane ever fully merged, for
    /// `run_export` to fold into its own `accum` exactly once after the concurrent
    /// phase has ended -- mirrors `gpu_accum`'s single end-of-export merge.
    pub(in crate::bridge::export_thread) fn into_buffer(self) -> Vec<Vec3> {
        self.accum
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Consecutive remote CHUNK failures [`run_remote_lane`] tolerates before it PAUSES
/// remote (`ComputeTarget::Both` only) -- see [`remote_retry_backoff`]. `2`, not `1`: a
/// single dropped connection is common enough (one network blip) that pausing after
/// just one would waste real remaining capacity; two in a row is past "transient".
const MAX_CONSECUTIVE_REMOTE_FAILURES: u32 = 2;

/// The first pause [`run_remote_lane`] takes once remote has failed
/// [`MAX_CONSECUTIVE_REMOTE_FAILURES`] chunks in a row; each further consecutive
/// failure doubles it, up to [`REMOTE_RETRY_BACKOFF_MAX`].
const REMOTE_RETRY_BACKOFF_INITIAL: Duration = Duration::from_secs(15);
const REMOTE_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(120);

/// How long to pause the remote lane after `consecutive_failures` failed chunks in a
/// row (called only once that count has reached [`MAX_CONSECUTIVE_REMOTE_FAILURES`]):
/// 15 s, 30 s, 60 s, 120 s, 120 s, ... Remote is never written off for the rest of an
/// export -- a worker that was unreachable for a minute (a wireless roam, a machine
/// that was busy with something else, a transient timeout) is offered work again as
/// soon as the pause ends, and every chunk it then completes counts. Local keeps
/// claiming from the shared cursor throughout, so a pause costs the export nothing but
/// remote's own share of throughput while it lasts.
fn remote_retry_backoff(consecutive_failures: u32) -> Duration {
    let doublings = consecutive_failures.saturating_sub(MAX_CONSECUTIVE_REMOTE_FAILURES);
    let scaled = REMOTE_RETRY_BACKOFF_INITIAL.saturating_mul(1u32 << doublings.min(8));
    scaled.min(REMOTE_RETRY_BACKOFF_MAX)
}

/// Sleeps for `total`, returning early with `false` the moment `cancel` is raised or
/// `cursor`'s shared pool runs dry (local has claimed everything that was left -- no
/// point holding the export open for a pause with nothing to hand remote afterwards).
/// `true` means the full pause elapsed with work still available.
fn pause_remote_lane(cursor: &SampleCursor, cancel: &AtomicBool, total: Duration) -> bool {
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
            return RemoteLaneOutcome { fatal: None };
        }

        let Some((start, count)) = cursor.claim(remote_chunk_samples(rate)) else {
            // Nothing left in the shared pool -- a normal, successful end to this
            // lane's work (this only ever inspects the shared pool, never the
            // local-only retry pile, which is `claim_local`'s alone).
            return RemoteLaneOutcome { fatal: None };
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

        if done > 0 {
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

        if let Some(measured) = measured_rate {
            rate = measured;
        }

        if cancelled {
            // `run_export`'s own top-level `cancel` check discards the whole export
            // regardless of what got done here -- nothing further to decide.
            return RemoteLaneOutcome { fatal: None };
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
                return RemoteLaneOutcome { fatal: None };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::export_thread::scene_snapshot::SceneSnapshot;
    use indicatrix::{geometry::cuts::StandardGemCuts, optics::materials::GemMaterial};

    /// The export's own bounce cap must reach the remote `RenderRequest`, not whatever
    /// the live viewport was set to. Uses a value distinct from any plausible viewport
    /// default (`RenderContext::default().max_bounces` is 12) to prove a combined
    /// local+remote export can't trace the two engines at different caps.
    #[test]
    fn scene_state_from_snapshot_carries_the_snapshots_own_bounce_cap_not_a_viewport_default() {
        let snapshot = SceneSnapshot {
            yaw: 0.6,
            pitch: 0.45,
            distance: 2.4,
            light_yaw: 0.85,
            light_pitch: 0.95,
            material: GemMaterial::diamond(),
            lighting_preset: indicatrix::optics::raytracer::LightingPreset::RingLights,
            max_bounces: 64, // deliberately NOT `RenderContext::default().max_bounces` (12)
            exposure: 1.0,
            backdrop: 0.0,
            active_planes: StandardGemCuts::standard_round_brilliant(),
            facet_finishes: Vec::new(),
            env_map: None,
        };

        let state = scene_state_from_snapshot(&snapshot, 1920, 1080);

        assert_eq!(
            state.max_bounces, 64,
            "the remote RenderRequest's scene must carry the export's OWN bounce cap"
        );
    }

    // `liveness_deadline`: the pure decision behind the export-side liveness fix --
    // see `FIRST_EVENT_TIMEOUT`'s doc comment for the bug this closes.

    #[test]
    fn liveness_deadline_grants_the_first_event_grace_before_any_update_has_arrived() {
        assert_eq!(
            liveness_deadline(false, 0),
            FIRST_EVENT_TIMEOUT,
            "the wait for a dispatch's very first RemoteUpdate (even Connected) must \
             use the longer grace, not the steady-state deadline"
        );
    }

    #[test]
    fn liveness_deadline_switches_to_the_tighter_steady_state_timeout_once_seen() {
        assert_eq!(
            liveness_deadline(true, 0),
            LIVENESS_TIMEOUT,
            "once a dispatch has produced at least one update, every wait after it must \
             use the steady-state liveness timeout, not the first-event grace"
        );
    }

    #[test]
    fn first_event_timeout_is_strictly_longer_than_liveness_timeout() {
        assert!(FIRST_EVENT_TIMEOUT > LIVENESS_TIMEOUT);
    }

    /// The 4K regression: one FRAME is ~100 MB, which a ~150 Mbit/s wireless link
    /// drains in 5-8 s -- at or past the bare 8 s steady-state deadline. The allowance
    /// must lift the deadline well clear of that, and scale with the frame, not a
    /// resolution-blind constant.
    #[test]
    fn liveness_deadline_budgets_a_whole_frame_transfer_on_a_slow_link() {
        let bytes_4k = 3840 * 2160 * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
        let bytes_1080p = 1920 * 1080 * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
        let at_4k = liveness_deadline(true, bytes_4k);
        let at_1080p = liveness_deadline(true, bytes_1080p);
        assert!(at_4k >= LIVENESS_TIMEOUT + Duration::from_secs(20));
        assert!(at_4k > at_1080p);
        assert_eq!(transfer_allowance(0), Duration::ZERO);
    }

    #[test]
    fn remote_retry_backoff_doubles_from_the_threshold_and_caps() {
        assert_eq!(
            remote_retry_backoff(MAX_CONSECUTIVE_REMOTE_FAILURES),
            REMOTE_RETRY_BACKOFF_INITIAL
        );
        assert_eq!(
            remote_retry_backoff(MAX_CONSECUTIVE_REMOTE_FAILURES + 1),
            REMOTE_RETRY_BACKOFF_INITIAL * 2
        );
        assert_eq!(remote_retry_backoff(50), REMOTE_RETRY_BACKOFF_MAX);
    }

    #[test]
    fn pause_remote_lane_returns_early_once_the_shared_pool_is_dry() {
        let cursor = SampleCursor::new(0, 4);
        assert_eq!(cursor.claim(4), Some((0, 4)));
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        assert!(!pause_remote_lane(
            &cursor,
            &cancel,
            Duration::from_secs(30)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

//! [`run_remote_batch`]: dispatching a single `RenderRequest` against a remote worker,
//! including the export-side liveness/cancellation watchdog timeouts.
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
//! something else. A coarse worker cadence can legitimately make the FIRST wait take
//! longer than a steady-state one, which is why [`FIRST_EVENT_TIMEOUT`] grants it a
//! longer deadline.

use super::super::{capability::RemoteCapability, rate::remote_request_rate};
use crate::bridge::remote::remote_render::{self, RemoteRenderRequest, RemoteUpdate};
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
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
pub(super) const LIVENESS_TIMEOUT: Duration = Duration::from_secs(8);

/// Mirrors `bridge::remote::remote_render::connection`'s own `FIRST_EVENT_TIMEOUT`.
///
/// # Why the first wait needs a longer grace
///
/// Confirmed against a real export: at 4K with a worker cadence of 20 samples/tick, a
/// calibration probe can legitimately take longer than [`LIVENESS_TIMEOUT`] to produce
/// its first [`RemoteUpdate`] while genuinely still computing. `run_remote_batch`'s
/// `last_update` clock is judged against this longer deadline for that first wait
/// specifically, so a calibration probe busy but slow to report isn't mistaken for
/// "worker silent".
pub(super) const FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

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
pub(super) const fn transfer_allowance(frame_bytes: u64) -> Duration {
    Duration::from_secs(frame_bytes.div_ceil(MIN_ASSUMED_LINK_BYTES_PER_SEC))
}

/// Which deadline currently applies to an idle wait on `rx`: mirrors
/// `bridge::remote::remote_render::connection::liveness_deadline`, plus the
/// [`transfer_allowance`] for a payload of `frame_bytes` (one full-resolution `FRAME`
/// at the request's dimensions), since a wait here can span an entire frame transfer.
pub(in crate::bridge::export_thread) const fn liveness_deadline(
    seen_first_update: bool,
    frame_bytes: u64,
) -> Duration {
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
pub(in crate::bridge::export_thread) const CANCEL_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// The first and the latest report of real progress one dispatch has produced: the two
/// points a plain worker's rendering rate is measured between (see
/// [`remote_request_rate`]).
#[derive(Debug, Default)]
pub(super) struct ProgressSpan {
    /// When the first report with `samples_done > 0` arrived, and what it said.
    first: Option<(Instant, u32)>,
    /// When the latest report that ADVANCED `samples_done` arrived, and what it said.
    last: Option<(Instant, u32)>,
}

impl ProgressSpan {
    /// Records a `Frame`/`Progress` report of `samples_done` that arrived at `at`. A
    /// report that does not raise `samples_done` above the latest one recorded (a repeat,
    /// or a heartbeat with nothing new) changes nothing.
    pub(super) fn note(&mut self, at: Instant, samples_done: u32) {
        let latest = self.last.map_or(0, |(_, samples)| samples);
        if samples_done <= latest {
            return;
        }
        if self.first.is_none() {
            self.first = Some((at, samples_done));
        }
        self.last = Some((at, samples_done));
    }

    /// The samples gained and the time elapsed between the first and the latest
    /// recorded report; `None` until two reports with different `samples_done` have
    /// been seen.
    pub(super) fn advance(&self) -> Option<(u32, Duration)> {
        let (first_at, first_samples) = self.first?;
        let (last_at, last_samples) = self.last?;
        if last_samples <= first_samples {
            return None;
        }
        Some((
            last_samples - first_samples,
            last_at.saturating_duration_since(first_at),
        ))
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
/// `rate_samples_per_sec` is remote's measured throughput for this dispatch, taken at
/// its `DONE` (see `super::super::rate::remote_request_rate`): against a coordinator
/// (`RemoteCapability::coordinator`) `samples_done` over the WHOLE request, from just
/// before it is sent to `DONE`, with every fixed cost in it; against a plain worker the
/// span between its first and its last progress report, or the whole request when it
/// reported progress only once. Never the window from the first progress report to
/// `DONE`, which contains the tail transfer but not the first chunk's work. `None` when
/// the dispatch ended without a `DONE` or traced nothing.
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
    // The instant the request is sent -- the start of the whole-request span its rate
    // is measured over (connection, handshake and upload included).
    let request_sent_at = Instant::now();
    let handle = remote_render::spawn_remote_render(
        RemoteRenderRequest {
            worker: capability.worker.clone(),
            request_id: REQUEST_ID,
            scene,
            first_sample,
            samples,
            width,
            height,
            // Exports and tilt videos (the only callers) are throughput work.
            intent: indicatrix_net::messages::RequestIntent::Batch,
            display_only: false,
        },
        Arc::clone(accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    // The first and the latest update that advanced `samples_done` -- what a plain
    // worker's rate is measured between (`remote_request_rate`).
    let mut span = ProgressSpan::default();
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
                let received_at = Instant::now();
                last_update = received_at;
                first_update_seen = true;
                match update {
                    RemoteUpdate::Done { cancelled, .. } => {
                        let samples_done = accumulator
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .samples_done();
                        let rate = remote_request_rate(
                            capability.coordinator,
                            samples_done,
                            received_at.saturating_duration_since(request_sent_at),
                            span.advance(),
                        );
                        return (samples_done, cancelled, None, rate);
                    }
                    RemoteUpdate::Failed { message, .. }
                    | RemoteUpdate::Unsupported { message, .. } => {
                        let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                        return (acc.samples_done(), false, Some(message), None);
                    }
                    RemoteUpdate::Frame { samples_done, .. }
                    | RemoteUpdate::Progress { samples_done, .. } => {
                        span.note(received_at, samples_done);
                    }
                    // Nothing terminal (Connected, Preview, a capability change, or a
                    // picture event, which never answers a RENDER) -- the accumulator
                    // already reflects every Frame as it's applied (`Accumulator::apply`).
                    _ => {}
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

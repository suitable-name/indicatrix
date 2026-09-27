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

use super::super::{capability::RemoteCapability, rate::remote_marginal_rate};
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
/// dispatch (see `super::super::rate::remote_marginal_rate`). `None` when there isn't
/// enough signal to trust one: no update ever reported progress, or the span collapsed
/// to nothing.
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
            // Exports and tilt videos (the only callers) are throughput work.
            intent: indicatrix_net::messages::RequestIntent::Batch,
            display_only: false,
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
                    RemoteUpdate::Failed { message, .. }
                    | RemoteUpdate::Unsupported { message, .. } => {
                        let acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
                        return (acc.samples_done(), false, Some(message), None);
                    }
                    RemoteUpdate::Frame { samples_done, .. }
                    | RemoteUpdate::Progress { samples_done, .. }
                        if first_progress.is_none() && samples_done > 0 =>
                    {
                        first_progress = Some((Instant::now(), samples_done));
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

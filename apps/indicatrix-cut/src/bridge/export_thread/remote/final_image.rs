//! "Final picture only" transfer for the still export and the tilt video: one v14 `FinalImageRequest` per image, answered by the
//! remote with `PROGRESS` heartbeats, one tone-mapped PNG and `DONE`.
//!
//! - [`plan_export_transfer`] decides, purely, whether an image uses it at all.
//! - [`run_final_image_request`] dispatches one request and waits for the picture.
//! - [`final_picture_follow_up`] decides, purely, what a non-picture outcome means:
//!   `UNSUPPORTED_REQUEST` (a plain worker) falls back to full data and is remembered
//!   per remote ([`remember_final_picture_refused`]); a failure falls back to full data
//!   under `Both` (which itself falls back to local) and fails under `RemoteOnly`.

use super::dispatch::{CANCEL_WAIT_TIMEOUT, liveness_deadline};
use crate::{
    bridge::{
        export_thread::{params::ComputeTarget, tonemap_png::tonemap_accumulation},
        remote::remote_render::{self, RemoteFinalImageRequest, RemoteUpdate},
    },
    settings::{ExportTransfer, WorkerSettings},
};
use glam::Vec3;
use indicatrix::color::ColorSpace;
use indicatrix_net::{
    SceneState,
    client::{Accumulator, PreviewSnapshot},
};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// Every final-image dispatch opens its own connection, so one fixed id is safe --
/// the same reasoning as `dispatch::run_batch`'s own `REQUEST_ID`.
const REQUEST_ID: u32 = 1;

/// Which transfer one image actually uses -- [`plan_export_transfer`]'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bridge::export_thread) enum TransferPlan {
    /// One `FinalImageRequest`; the local lanes do not take part.
    FinalPicture,
    /// The ordinary full-data path (`render_accumulation`).
    FullData,
    /// Full data because this remote already refused a final picture
    /// (`UNSUPPORTED_REQUEST`) earlier -- the caller notes it once.
    FullDataRefusedBefore,
}

/// Whether an image asked for with `requested` actually uses final-picture transfer:
/// only with a configured remote, a compute target that includes remote, and a scene
/// the remote may render at all (`hdr_refused`: an HDR environment map that cannot be
/// sent to any remote -- the full-data path then renders locally and says so; whether
/// the remote itself renders HDR is checked against its `WELCOME` at dispatch).
/// A remote that already refused (`refused_before`) is not asked again.
#[must_use]
pub(in crate::bridge::export_thread) const fn plan_export_transfer(
    requested: ExportTransfer,
    compute_target: ComputeTarget,
    remote_configured: bool,
    hdr_refused: bool,
    refused_before: bool,
) -> TransferPlan {
    let wanted = matches!(requested, ExportTransfer::FinalPicture)
        && !matches!(compute_target, ComputeTarget::LocalOnly)
        && remote_configured
        && !hdr_refused;
    match (wanted, refused_before) {
        (false, _) => TransferPlan::FullData,
        (true, true) => TransferPlan::FullDataRefusedBefore,
        (true, false) => TransferPlan::FinalPicture,
    }
}

/// How one [`run_final_image_request`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bridge::export_thread) enum FinalPictureOutcome {
    /// The decoded picture: exactly `width * height * 4` RGBA8 bytes.
    Completed {
        rgba: Vec<u8>,
        /// v16: how many of the viewer's own reserved samples the coordinator rendered
        /// itself because the [`LocalShare`] contribution did not arrive in time (or was
        /// invalid) -- `0` for a request with no local share, and for every ordinary
        /// full-remote picture. `Stats.reclaimed_samples`, unpacked here so
        /// `worker::final_picture::final_picture` can surface it as a note without
        /// reaching back into the accumulator itself.
        reclaimed_samples: u32,
    },
    /// `cancel` was observed; the remote confirmed (or never answered within
    /// [`CANCEL_WAIT_TIMEOUT`]).
    Cancelled,
    /// The remote answered `UNSUPPORTED_REQUEST` -- a plain worker.
    Unsupported(String),
    /// Anything else went wrong (unreachable, error, silence, a bad picture).
    Failed(String),
}

/// What the caller does after a [`FinalPictureOutcome`] -- [`final_picture_follow_up`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bridge::export_thread) enum FinalPictureFollowUp {
    /// Write this picture.
    Use(Vec<u8>),
    /// Stop: the user cancelled.
    Cancelled,
    /// Render this image through the full-data path instead, showing `note`;
    /// `remember` marks the remote as refusing final pictures for the rest of the
    /// session (only for `UNSUPPORTED_REQUEST`, which will not change by retrying).
    FallBackToFullData { note: String, remember: bool },
    /// Fail the image: `RemoteOnly` has no other lane to fall back to.
    Fail(String),
}

/// The fallback rule for a final-picture attempt -- see the module doc comment.
#[must_use]
pub(in crate::bridge::export_thread) fn final_picture_follow_up(
    outcome: FinalPictureOutcome,
    compute_target: ComputeTarget,
) -> FinalPictureFollowUp {
    match outcome {
        FinalPictureOutcome::Completed { rgba, .. } => FinalPictureFollowUp::Use(rgba),
        FinalPictureOutcome::Cancelled => FinalPictureFollowUp::Cancelled,
        FinalPictureOutcome::Unsupported(message) => FinalPictureFollowUp::FallBackToFullData {
            note: format!(
                "The remote does not support final-picture transfer ({message}); \
                 using full data instead."
            ),
            remember: true,
        },
        FinalPictureOutcome::Failed(message) => {
            if matches!(compute_target, ComputeTarget::RemoteOnly) {
                FinalPictureFollowUp::Fail(format!("Remote final-picture render failed: {message}"))
            } else {
                FinalPictureFollowUp::FallBackToFullData {
                    note: format!(
                        "Remote final-picture render failed ({message}); continuing with \
                         full data."
                    ),
                    remember: false,
                }
            }
        }
    }
}

/// Remotes (by address) that answered a `FinalImageRequest` with
/// `UNSUPPORTED_REQUEST` this session -- "remember per connection": a plain worker does
/// not grow the feature by being asked again. Cleared when the remote is re-saved
/// ([`forget_final_picture_refusals`]), since that may point at an upgraded server.
static FINAL_PICTURE_REFUSED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Records that `worker` refused a final picture (see [`FINAL_PICTURE_REFUSED`]).
pub(in crate::bridge::export_thread) fn remember_final_picture_refused(worker: &WorkerSettings) {
    FINAL_PICTURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(worker.address.clone());
}

/// Whether `worker` already refused a final picture this session.
#[must_use]
pub(in crate::bridge::export_thread) fn final_picture_refused(worker: &WorkerSettings) -> bool {
    FINAL_PICTURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&worker.address)
}

/// Forgets every remembered refusal -- called when the remote endpoint is saved. Also
/// clears [`FINAL_PICTURE_RATES`]: a re-saved remote may point at a different machine
/// entirely, so a stale local/remote split measured against the old one must not seed
/// the next [`viewer_share`] call.
pub fn forget_final_picture_refusals() {
    FINAL_PICTURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    FINAL_PICTURE_RATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

// ---- v16: viewer contribution -- share sizing, the rate book, and the local render's
// handle into `run_final_image_request` --------------------------------------------

/// The viewer's own contribution to a final-picture export that reserved a tail for it
/// (v16) -- see [`super::super::worker::core::render_local_share`], which actually
/// renders it, and [`run_final_image_request`]'s own doc comment for how this reaches
/// the wire. Built by `worker::final_picture::final_picture` and consumed entirely
/// inside [`run_final_image_request`]'s own polling loop.
pub(in crate::bridge::export_thread) struct LocalShare<'a> {
    /// How many samples the viewer's own render is worth -- `== viewer_samples`.
    pub samples: u32,
    /// The local render loop's own running total (relative to `first_sample`: `0` at
    /// the start, `samples` once finished), read every poll for combined progress.
    pub done: &'a AtomicU32,
    /// Yields the finished radiance sum (and how long it took) once the local render
    /// thread completes normally; never yields at all if it was stopped early.
    pub result: mpsc::Receiver<(Vec<Vec3>, Duration)>,
    /// Set to stop the local render loop early: a user cancel, or the remote's own
    /// `DONE` arriving first (this contribution is no longer needed).
    pub stop: &'a AtomicBool,
}

/// Default viewer share (10%) when no rate history exists yet for a remote -- see
/// [`viewer_share`]. Conservative: small enough that a wildly wrong first guess (a much
/// faster or slower machine than the remote) costs little either way, while still
/// measuring something real to seed the next export's proportional split from.
pub(in crate::bridge::export_thread) const DEFAULT_VIEWER_SHARE: f64 = 0.10;

/// The viewer's own sample share of a final-picture export's sample budget:
/// `clamp(round(samples * local/(local+remote)), 0, samples/2)`. Falls back to
/// [`DEFAULT_VIEWER_SHARE`] of `samples` (still capped at half) whenever either rate is
/// unknown, non-positive, or non-finite -- there is nothing yet to split proportionally
/// by. The half-of-`samples` cap ([`FinalImageRequest::viewer_share_valid`]) is the
/// server's own hard limit; enforcing it here too means a caller never has to retry a
/// refused request.
#[must_use]
pub(in crate::bridge::export_thread) fn viewer_share(
    samples: u32,
    local_rate: Option<f64>,
    remote_rate: Option<f64>,
) -> u32 {
    let half = samples / 2;
    let fraction = match (local_rate, remote_rate) {
        (Some(local), Some(remote))
            if local.is_finite() && remote.is_finite() && local > 0.0 && remote > 0.0 =>
        {
            local / (local + remote)
        }
        _ => DEFAULT_VIEWER_SHARE,
    };
    let share = (f64::from(samples) * fraction).round().max(0.0) as u32;
    share.min(half)
}

/// One remote's measured local/remote throughput, PIXEL-normalised (`rate * pixels`,
/// samples/sec times pixel count) exactly like `coordinator::job::RateBook` -- see that
/// type's own doc comment for why: a rate measured at one export's resolution must still
/// mean something at a very differently sized one.
#[derive(Debug, Clone, Copy, Default)]
struct SplitRates {
    local_px_rate: Option<f64>,
    remote_px_rate: Option<f64>,
}

/// Per-remote (by [`WorkerSettings::address`]) [`SplitRates`] book -- process-wide (not
/// per-carry, per-export) so the SECOND final-picture export against the same remote
/// already has a measured split to start [`viewer_share`] from, rather than
/// [`DEFAULT_VIEWER_SHARE`] every single time. A `BTreeMap`, not a hash map (house rule:
/// no hash-map iteration in a decision path -- reads here are by exact key anyway),
/// keyed the same way [`FINAL_PICTURE_REFUSED`] just above is.
static FINAL_PICTURE_RATES: Mutex<BTreeMap<String, SplitRates>> = Mutex::new(BTreeMap::new());

/// This remote's current local/remote rates in plain samples/sec for an image of
/// `pixels` pixels -- the stored pixel-samples/sec figures divided back down, mirroring
/// `RateBook::get`. `None` for either half means "not measured yet for this remote" (a
/// fresh session, or [`forget_final_picture_refusals`] cleared it).
#[must_use]
pub(in crate::bridge::export_thread) fn split_rates(
    worker: &WorkerSettings,
    pixels: u32,
) -> (Option<f64>, Option<f64>) {
    // `.copied()` takes the (small, `Copy`) entry out from under the lock in this one
    // statement, so the guard drops here rather than staying held through the
    // denormalising below.
    let entry = FINAL_PICTURE_RATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&worker.address)
        .copied();
    let Some(entry) = entry else {
        return (None, None);
    };
    let denorm = |px_rate: Option<f64>| px_rate.map(|rate| rate / f64::from(pixels.max(1)));
    (denorm(entry.local_px_rate), denorm(entry.remote_px_rate))
}

/// Records this contributed picture's measured local/remote rates (plain samples/sec at
/// `pixels`), pixel-normalising before storing (`rate * pixels`), mirroring
/// `RateBook::set`. `None` leaves that half of the book unchanged -- nothing new was
/// measured this time (no local share was ever requested, say, or the remote lane's own
/// rate couldn't be pinned down).
pub(in crate::bridge::export_thread) fn record_split_rates(
    worker: &WorkerSettings,
    pixels: u32,
    local: Option<f64>,
    remote: Option<f64>,
) {
    if local.is_none() && remote.is_none() {
        return;
    }
    let pixels = f64::from(pixels.max(1));
    let local = local.map(|rate| rate * pixels);
    let remote = remote.map(|rate| rate * pixels);
    // One statement: the lock is held only for this `entry`/`and_modify`/`or_insert_with`
    // chain, not across separate `if let` statements afterward.
    FINAL_PICTURE_RATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(worker.address.clone())
        .and_modify(|entry| {
            if let Some(local) = local {
                entry.local_px_rate = Some(local);
            }
            if let Some(remote) = remote {
                entry.remote_px_rate = Some(remote);
            }
        })
        .or_insert(SplitRates {
            local_px_rate: local,
            remote_px_rate: remote,
        });
}

/// Decodes the accumulator's `FINAL_IMAGE` into RGBA8 of exactly `width x height`.
///
/// # Errors
///
/// A human-readable message when no picture arrived, its size differs from the
/// request, or the payload does not decode (`indicatrix_net::display::decode_rgba8`'s
/// bounded checks).
pub(in crate::bridge::export_thread) fn decode_final_picture(
    accumulator: &Accumulator,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let picture = accumulator
        .final_image()
        .ok_or_else(|| "the remote finished without sending a picture".to_string())?;
    if (picture.width, picture.height) != (width, height) {
        return Err(format!(
            "the remote sent a {}x{} picture for a {width}x{height} request",
            picture.width, picture.height
        ));
    }
    indicatrix_net::display::decode_rgba8(picture.encoding, width, height, &picture.bytes)
        .map_err(|e| format!("the remote's picture did not decode: {e}"))
}

/// Tone-maps a `PREVIEW` snapshot into the small sRGB thumbnail
/// [`ExportProgress::preview`](crate::bridge::export_thread::ExportProgress) already
/// knows how to show -- the same `tonemap_accumulation` the full-data preview thumbnail
/// and the final PNG both use, always through the `Srgb` path since this is a live
/// on-screen thumbnail, never written to disk (mirrors
/// `export_thread::preview::downsample_preview`'s own reasoning for always going
/// through `Srgb`, one level up in this same module tree).
fn preview_image(snapshot: &PreviewSnapshot) -> SharedPixelBuffer<Rgba8Pixel> {
    let rgba = tonemap_accumulation(
        snapshot.width,
        snapshot.height,
        snapshot.samples_done.max(1),
        &snapshot.buffer,
        ColorSpace::Srgb,
    );
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(snapshot.width, snapshot.height);
    // Zero-copy reinterpret RGBA8 bytes into `Rgba8Pixel` -- same idiom as
    // `export_thread::preview::generate_preview_buffer`.
    let dst = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(&rgba);
    dst.copy_from_slice(src);
    buffer
}

/// Dispatches one `FinalImageRequest` for `[0, samples)` of `scene` (whose
/// `width`/`height` are the output size) against `worker` and blocks until the picture
/// arrives, the request fails, or `cancel` is observed (then `CANCEL` is sent and the
/// remote's `DONE { cancelled }` awaited, bounded by [`CANCEL_WAIT_TIMEOUT`]).
///
/// `local` is the viewer's own concurrently-running contribution to a reserved tail
/// (v16, see [`LocalShare`]'s own doc comment) -- `None` reproduces today's pure-remote
/// picture (`viewer_samples: 0` on the wire). Every poll of this loop also checks
/// `local.result` for the finished sum and uploads it the moment it's ready
/// ([`remote_render::RemoteRenderHandle::contribute`]), resetting the liveness clock
/// exactly like the `NEED_ASSET` upload does -- the server sends nothing while it reads
/// the upload, so a wait spanning it needs the same transfer allowance a `FRAME` gets.
///
/// `on_progress(remote_done, local_done, preview)` fires for every `PROGRESS` heartbeat
/// (`preview: None`) and every `PREVIEW` the coordinator forces on for this job's own
/// periodic look (`preview: Some`, tone-mapped via [`preview_image`]) -- see
/// `coordinator::job::serve_final_image`'s own doc comment on why a `FinalImageRequest`
/// always gets one even though its wire request has no `StreamConfig` of its own.
/// `remote_done`/`local_done` are each lane's own running total, reported SEPARATELY:
/// combining them into one number (and capping at `samples`, since the coordinator's own
/// `PROGRESS` only folds a merged contribution in once, which can transiently make the
/// raw sum overshoot) is the caller's job, since only the caller knows whether `local` is
/// even in play -- see `worker::final_picture::report_combined_progress`.
///
/// Liveness mirrors the full-data chunk watchdog (`dispatch::liveness_deadline`): the
/// first event gets the long grace, every later wait the heartbeat-derived deadline
/// plus an allowance for one payload in flight -- a `width * height * 4`-byte picture
/// normally, or the viewer's own `width * height * 12`-byte radiance upload once it has
/// started.
pub(in crate::bridge::export_thread) fn run_final_image_request(
    worker: &WorkerSettings,
    scene: SceneState,
    samples: u32,
    color_space: ColorSpace,
    cancel: &AtomicBool,
    local: Option<&LocalShare<'_>>,
    mut on_progress: impl FnMut(u32, u32, Option<SharedPixelBuffer<Rgba8Pixel>>),
) -> FinalPictureOutcome {
    let (width, height) = (scene.width, scene.height);
    let accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    let viewer_samples = local.map_or(0, |share| share.samples);
    let handle = remote_render::spawn_final_image_request(
        RemoteFinalImageRequest {
            worker: worker.clone(),
            request_id: REQUEST_ID,
            scene,
            first_sample: 0,
            samples,
            color_space,
            viewer_samples,
        },
        Arc::clone(&accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    let picture_bytes = u64::from(width) * u64::from(height) * 4;
    let contribution_bytes =
        u64::from(width) * u64::from(height) * indicatrix_net::radiance::BYTES_PER_PIXEL as u64;
    let mut cancel_sent_at: Option<Instant> = None;
    let mut last_update = Instant::now();
    let mut seen_first = false;
    // Once uploaded, later waits budget for the (much smaller, and one-shot) upload
    // frame instead of a whole finished picture -- see `contribution_bytes` above.
    let mut contribution_sent = false;
    loop {
        if cancel_sent_at.is_none() && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancel_sent_at = Some(Instant::now());
            stop_local(local);
        }
        // The local share may finish at any point in this loop, independent of
        // whatever the remote is doing -- check every iteration, not just when a
        // `RemoteUpdate` happens to arrive in the same 100ms slice.
        if !contribution_sent
            && let Some(share) = local
            && let Ok((sum, _elapsed)) = share.result.try_recv()
            && handle.contribute(sum)
        {
            contribution_sent = true;
            last_update = Instant::now();
        }
        let local_done = local.map_or(0, |share| share.done.load(Ordering::Relaxed));
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(update) => {
                last_update = Instant::now();
                seen_first = true;
                if let Some(outcome) = on_update(
                    update,
                    &accumulator,
                    width,
                    height,
                    local_done,
                    &mut on_progress,
                ) {
                    stop_local(local);
                    return outcome;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let in_flight_bytes = if contribution_sent {
                    contribution_bytes
                } else {
                    picture_bytes
                };
                match cancel_sent_at {
                    Some(sent_at) if sent_at.elapsed() > CANCEL_WAIT_TIMEOUT => {
                        tracing::warn!("final picture: the remote never confirmed the cancel");
                        stop_local(local);
                        return FinalPictureOutcome::Cancelled;
                    }
                    None if last_update.elapsed()
                        > liveness_deadline(seen_first, in_flight_bytes) =>
                    {
                        stop_local(local);
                        return FinalPictureOutcome::Failed(format!(
                            "remote silent for {:.0?}",
                            last_update.elapsed()
                        ));
                    }
                    Some(_) | None => {}
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                stop_local(local);
                return FinalPictureOutcome::Failed(
                    "the remote connection thread ended unexpectedly".to_string(),
                );
            }
        }
    }
}

/// Raises `local.stop` if there is a local share in play -- every exit path of
/// [`run_final_image_request`]'s loop calls this, so a local render thread never keeps
/// tracing samples nobody will use once the request itself has ended.
fn stop_local(local: Option<&LocalShare<'_>>) {
    if let Some(share) = local {
        share.stop.store(true, Ordering::Relaxed);
    }
}

/// One update of [`run_final_image_request`]'s loop: `Some` ends the request.
fn on_update(
    update: RemoteUpdate,
    accumulator: &Mutex<Accumulator>,
    width: u32,
    height: u32,
    local_done: u32,
    on_progress: &mut impl FnMut(u32, u32, Option<SharedPixelBuffer<Rgba8Pixel>>),
) -> Option<FinalPictureOutcome> {
    match update {
        RemoteUpdate::Progress { samples_done, .. } => {
            on_progress(samples_done, local_done, None);
            None
        }
        // The coordinator's periodic look at this job (see `run_final_image_request`'s
        // own doc comment): the accumulator already holds the applied snapshot by the
        // time this update reaches here (`connection::stream_io::to_remote_update` only
        // translates an already-applied `ApplyOutcome`), so just read and tone-map it.
        RemoteUpdate::Preview { .. } => {
            let guard = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(snapshot) = guard.last_preview() {
                let samples_done = snapshot.samples_done;
                let image = preview_image(snapshot);
                drop(guard);
                on_progress(samples_done, local_done, Some(image));
            }
            None
        }
        RemoteUpdate::Done {
            cancelled: true, ..
        } => Some(FinalPictureOutcome::Cancelled),
        RemoteUpdate::Done {
            cancelled: false, ..
        } => {
            let guard = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
            // v16: how many of the viewer's own reserved samples the coordinator ended
            // up rendering itself -- `0` when there was no local share, or it arrived in
            // time.
            let reclaimed_samples = guard
                .done_stats()
                .map_or(0, |stats| stats.reclaimed_samples);
            let decoded = decode_final_picture(&guard, width, height);
            drop(guard);
            Some(match decoded {
                Ok(rgba) => FinalPictureOutcome::Completed {
                    rgba,
                    reclaimed_samples,
                },
                Err(message) => FinalPictureOutcome::Failed(message),
            })
        }
        RemoteUpdate::Unsupported { message, .. } => {
            Some(FinalPictureOutcome::Unsupported(message))
        }
        RemoteUpdate::Failed { message, .. } => Some(FinalPictureOutcome::Failed(message)),
        // The picture itself is read on `Done`; nothing else answers this request.
        RemoteUpdate::Connected { .. }
        | RemoteUpdate::FinalImage { .. }
        | RemoteUpdate::Frame { .. }
        | RemoteUpdate::DisplayFrame { .. }
        | RemoteUpdate::CapabilityChanged { .. } => None,
    }
}

#[cfg(test)]
mod tests;

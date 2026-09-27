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
        export_thread::params::ComputeTarget,
        remote::remote_render::{self, RemoteFinalImageRequest, RemoteUpdate},
    },
    settings::{ExportTransfer, WorkerSettings},
};
use indicatrix::color::ColorSpace;
use indicatrix_net::{SceneState, client::Accumulator};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
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
    Completed(Vec<u8>),
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
        FinalPictureOutcome::Completed(rgba) => FinalPictureFollowUp::Use(rgba),
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

/// Forgets every remembered refusal -- called when the remote endpoint is saved.
pub fn forget_final_picture_refusals() {
    FINAL_PICTURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
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

/// Dispatches one `FinalImageRequest` for `[0, samples)` of `scene` (whose
/// `width`/`height` are the output size) against `worker` and blocks until the picture
/// arrives, the request fails, or `cancel` is observed (then `CANCEL` is sent and the
/// remote's `DONE { cancelled }` awaited, bounded by [`CANCEL_WAIT_TIMEOUT`]).
/// `on_progress(samples_done)` fires for every `PROGRESS` heartbeat.
///
/// Liveness mirrors the full-data chunk watchdog (`dispatch::liveness_deadline`): the
/// first event gets the long grace, every later wait the heartbeat-derived deadline
/// plus an allowance for one `width * height * 4`-byte picture in flight.
pub(in crate::bridge::export_thread) fn run_final_image_request(
    worker: &WorkerSettings,
    scene: SceneState,
    samples: u32,
    color_space: ColorSpace,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u32),
) -> FinalPictureOutcome {
    let (width, height) = (scene.width, scene.height);
    let accumulator = Arc::new(Mutex::new(Accumulator::new(width, height)));
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    let handle = remote_render::spawn_final_image_request(
        RemoteFinalImageRequest {
            worker: worker.clone(),
            request_id: REQUEST_ID,
            scene,
            first_sample: 0,
            samples,
            color_space,
        },
        Arc::clone(&accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    let picture_bytes = u64::from(width) * u64::from(height) * 4;
    let mut cancel_sent_at: Option<Instant> = None;
    let mut last_update = Instant::now();
    let mut seen_first = false;
    loop {
        if cancel_sent_at.is_none() && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancel_sent_at = Some(Instant::now());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(update) => {
                last_update = Instant::now();
                seen_first = true;
                if let Some(outcome) =
                    on_update(update, &accumulator, width, height, &mut on_progress)
                {
                    return outcome;
                }
            }
            Err(RecvTimeoutError::Timeout) => match cancel_sent_at {
                Some(sent_at) if sent_at.elapsed() > CANCEL_WAIT_TIMEOUT => {
                    tracing::warn!("final picture: the remote never confirmed the cancel");
                    return FinalPictureOutcome::Cancelled;
                }
                None if last_update.elapsed() > liveness_deadline(seen_first, picture_bytes) => {
                    return FinalPictureOutcome::Failed(format!(
                        "remote silent for {:.0?}",
                        last_update.elapsed()
                    ));
                }
                Some(_) | None => {}
            },
            Err(RecvTimeoutError::Disconnected) => {
                return FinalPictureOutcome::Failed(
                    "the remote connection thread ended unexpectedly".to_string(),
                );
            }
        }
    }
}

/// One update of [`run_final_image_request`]'s loop: `Some` ends the request.
fn on_update(
    update: RemoteUpdate,
    accumulator: &Mutex<Accumulator>,
    width: u32,
    height: u32,
    on_progress: &mut impl FnMut(u32),
) -> Option<FinalPictureOutcome> {
    match update {
        RemoteUpdate::Progress { samples_done, .. } => {
            on_progress(samples_done);
            None
        }
        RemoteUpdate::Done {
            cancelled: true, ..
        } => Some(FinalPictureOutcome::Cancelled),
        RemoteUpdate::Done {
            cancelled: false, ..
        } => {
            let decoded = decode_final_picture(
                &accumulator.lock().unwrap_or_else(PoisonError::into_inner),
                width,
                height,
            );
            Some(match decoded {
                Ok(rgba) => FinalPictureOutcome::Completed(rgba),
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
        | RemoteUpdate::Preview { .. }
        | RemoteUpdate::DisplayFrame { .. }
        | RemoteUpdate::CapabilityChanged { .. } => None,
    }
}

#[cfg(test)]
mod tests;

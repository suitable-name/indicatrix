//! The one-shot connection lifecycle: one connection, one request, then the thread
//! ends -- [`spawn_remote_render`] (a `RenderRequest`) and [`spawn_final_image_request`]
//! (a v14 `FinalImageRequest`), sharing the worker body [`run`].

use super::{
    super::types::{
        HandleKind, RemoteCommand, RemoteError, RemoteFinalImageRequest, RemoteRenderHandle,
        RemoteRenderRequest, RemoteStream, RemoteUpdate,
    },
    handshake::connect_and_handshake,
    stream_io::{to_remote_update, try_read_stream_event},
};
use crate::settings::WorkerSettings;
use indicatrix_net::{
    client::{Accumulator, ApplyOutcome},
    messages::{FinalImageRequest, FinalOutput, RenderCapability, RenderRequest, StreamEvent},
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

/// What one one-shot connection sends once the handshake is done.
enum OneShotRequest {
    /// A `RenderRequest` (export chunks, batch previews).
    Render(RemoteRenderRequest),
    /// A v14 `FinalImageRequest` ("final picture only").
    FinalImage(RemoteFinalImageRequest),
}

impl OneShotRequest {
    const fn worker(&self) -> &WorkerSettings {
        match self {
            Self::Render(r) => &r.worker,
            Self::FinalImage(r) => &r.worker,
        }
    }

    const fn request_id(&self) -> u32 {
        match self {
            Self::Render(r) => r.request_id,
            Self::FinalImage(r) => r.request_id,
        }
    }

    /// The scene the request renders.
    const fn scene(&self) -> &indicatrix_net::SceneState {
        match self {
            Self::Render(r) => &r.scene,
            Self::FinalImage(r) => &r.scene,
        }
    }

    /// The absolute sample range `[first, first + samples)` the request asks for.
    const fn range(&self) -> (u32, u32) {
        match self {
            Self::Render(r) => (r.first_sample, r.samples),
            Self::FinalImage(r) => (r.first_sample, r.samples),
        }
    }

    /// Writes the request on `stream` against the server's advertised `capability`.
    fn send(
        &self,
        stream: &mut RemoteStream,
        capability: &RenderCapability,
    ) -> Result<(), RemoteError> {
        match self {
            Self::Render(request) => {
                let render_request = RenderRequest {
                    request_id: request.request_id,
                    scene: request.scene.clone(),
                    first_sample: request.first_sample,
                    samples: request.samples,
                    // `export_stream_config`, not `stream_config`: the one-shot callers are
                    // the export's chunk dispatch and the batch preview (the live viewport
                    // uses `super::persistent` with the live `stream_config`), so the
                    // export-specific choices (no preview stream, a larger cadence floor)
                    // apply unconditionally here.
                    stream: request
                        .worker
                        .export_stream_config(capability.min_cadence_ms),
                    intent: request.intent,
                };
                indicatrix_net::client::send_render_request(stream, &render_request)?;
            }
            Self::FinalImage(request) => {
                let final_request = FinalImageRequest {
                    request_id: request.request_id,
                    scene: request.scene.clone(),
                    first_sample: request.first_sample,
                    samples: request.samples,
                    width: request.scene.width,
                    height: request.scene.height,
                    color_space: request.color_space.into(),
                    output: FinalOutput::PngRgba8,
                    viewer_samples: request.viewer_samples,
                };
                indicatrix_net::client::send_final_image_request(stream, &final_request)?;
            }
        }
        Ok(())
    }
}

/// Connects to `request.worker`, performs the mutual-TLS handshake and
/// `HELLO`/`WELCOME`, then sends and streams one `RenderRequest` covering
/// `[request.first_sample, request.first_sample + request.samples)` of
/// `request.scene` at the session's `request.width x request.height` resolution.
///
/// `accumulator` is shared with the caller: [`Accumulator::begin_request`] is called
/// here (synchronously, right before the request is sent -- see
/// `indicatrix_net::client::session`'s module docs on why that ordering matters) and every
/// reply is applied into it as it arrives, under a short-held lock each time so the
/// caller can read a consistent snapshot from another thread at any point.
pub fn spawn_remote_render(
    request: RemoteRenderRequest,
    accumulator: Arc<Mutex<Accumulator>>,
    on_update: impl FnMut(RemoteUpdate) + Send + 'static,
) -> RemoteRenderHandle {
    spawn(OneShotRequest::Render(request), accumulator, on_update)
}

/// [`spawn_remote_render`]'s twin for a v14 `FinalImageRequest` ("final picture only"):
/// the remote renders `[first_sample, first_sample + samples)` and tone-maps it itself.
/// Replies are `PROGRESS` heartbeats, one `FINAL_IMAGE` (lands, still encoded, in
/// `accumulator.final_image()`, reported as [`RemoteUpdate::FinalImage`]) and `DONE`;
/// a cancel ends it with `DONE { cancelled: true }` and no picture; a plain worker
/// answers [`RemoteUpdate::Unsupported`].
pub fn spawn_final_image_request(
    request: RemoteFinalImageRequest,
    accumulator: Arc<Mutex<Accumulator>>,
    on_update: impl FnMut(RemoteUpdate) + Send + 'static,
) -> RemoteRenderHandle {
    spawn(OneShotRequest::FinalImage(request), accumulator, on_update)
}

fn spawn(
    request: OneShotRequest,
    accumulator: Arc<Mutex<Accumulator>>,
    mut on_update: impl FnMut(RemoteUpdate) + Send + 'static,
) -> RemoteRenderHandle {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let request_id = request.request_id();
        let result = run(
            &request,
            &accumulator,
            &rx,
            &mut on_update,
            super::LIVENESS_TIMEOUT,
        );
        if let Err(e) = result {
            on_update(RemoteUpdate::Failed {
                request_id,
                message: e.to_string(),
            });
        }
    });

    RemoteRenderHandle(HandleKind::OneShot(tx))
}

/// Fails with [`RemoteError::CancelUnacknowledged`] once a `CANCEL` written at
/// `cancel_sent_at` has gone unanswered for [`super::CANCEL_ACK_TIMEOUT`]. Called on
/// every loop pass -- idle polls and non-terminal events alike -- because a heartbeating
/// worker keeps producing the latter and would otherwise never let the silence clock fire.
fn ensure_cancel_acknowledged(cancel_sent_at: Option<Instant>) -> Result<(), RemoteError> {
    let now = Instant::now();
    if super::cancel_ack_overdue(cancel_sent_at, now, super::CANCEL_ACK_TIMEOUT) {
        let waited =
            cancel_sent_at.map_or(Duration::ZERO, |sent| now.saturating_duration_since(sent));
        return Err(RemoteError::CancelUnacknowledged(waited));
    }
    Ok(())
}

/// The one-shot body, with the liveness deadline threaded in as a parameter -- see
/// [`super::LIVENESS_TIMEOUT`]'s own doc comment for why.
fn run(
    request: &OneShotRequest,
    accumulator: &Arc<Mutex<Accumulator>>,
    commands: &mpsc::Receiver<RemoteCommand>,
    on_update: &mut dyn FnMut(RemoteUpdate),
    liveness_timeout: Duration,
) -> Result<(), RemoteError> {
    let request_id = request.request_id();
    let (mut stream, welcome) = connect_and_handshake(request.worker())?;
    on_update(RemoteUpdate::Connected {
        request_id,
        info: welcome.clone().into(),
    });

    // Ask before sending: the handshake advertises render capacity precisely so a client
    // never discovers its absence by having a request rejected downstream.
    let Some(capability) = welcome.render.as_ref() else {
        return Err(RemoteError::NoRenderCapacity);
    };

    {
        let (first_sample, samples) = request.range();
        let mut acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
        // Declares the requested range too: a FRAME outside it is a protocol
        // bug and is never summed -- see `Accumulator::begin_request_for_range`.
        acc.begin_request_for_range(request_id, first_sample, samples);
    }
    // HDR guard: never send an HDR scene to a remote whose WELCOME says it cannot render
    // one -- it would be refused at best, studio-lit at worst.
    if request.scene().hdr().is_some() && !capability.hdr {
        return Err(RemoteError::HdrUnsupported);
    }
    // Zoning guard (`zoning` builds): see `persistent::dispatch_render`.
    #[cfg(feature = "zoning")]
    if crate::bridge::remote::zoned_scene_refusal(
        request.scene().material.zoning.is_some(),
        welcome.zoning,
    )
    .is_err()
    {
        return Err(RemoteError::ZoningUnsupported);
    }
    request.send(&mut stream, capability)?;

    // Reset right after the request is actually on the wire -- see `LIVENESS_TIMEOUT`'s
    // own doc comment for why the countdown starts from the most recent sign of life,
    // and sending the request counts as one (a fresh TCP+TLS round trip and a write just
    // succeeded), not from whenever this function happened to be called.
    let mut last_event = Instant::now();
    // Whether ANY stream event has been observed yet for this request -- selects
    // `FIRST_EVENT_TIMEOUT` (before) or `liveness_timeout` (after) via
    // `liveness_deadline`. See `FIRST_EVENT_TIMEOUT`'s own doc comment for why the wait
    // for the very first tick needs more slack than every wait after it.
    let mut seen_first_event = false;
    // When the first `CANCEL` went out; a repeated cancel keeps the original stamp so the
    // acknowledgement window never restarts.
    let mut cancel_sent_at: Option<Instant> = None;
    loop {
        match commands.try_recv() {
            Ok(RemoteCommand::Cancel) => {
                indicatrix_net::client::send_cancel(&mut stream, request_id)?;
                cancel_sent_at.get_or_insert_with(Instant::now);
            }
            // v16: the viewer's own reserved-tail sum, ready to upload as one
            // `CONTRIBUTION` -- only meaningful for a `FinalImageRequest`; a plain
            // `RenderRequest` never gets a `RemoteRenderHandle::contribute` call in the
            // first place (nothing constructs one for it), so this is a defensive no-op
            // there rather than an error.
            Ok(RemoteCommand::Contribute(sum)) => {
                if let OneShotRequest::FinalImage(final_request) = request {
                    let first = final_request.first_sample + final_request.samples
                        - final_request.viewer_samples;
                    // One adaptive link per connection, keyed by the coordinator's address:
                    // the upload is encoded for the link speed measured last time and its
                    // own blocking write (straight on the TLS socket, no buffering in
                    // between) is timed for the next connection.
                    let link = super::payload_setting::open_link(&request.worker().address);
                    indicatrix_net::client::send_contribution_with_link(
                        &mut stream,
                        request_id,
                        (first, final_request.viewer_samples),
                        (final_request.scene.width, final_request.scene.height),
                        &sum,
                        &link,
                    )?;
                    // Same reset as the `NEED_ASSET` upload above: a large upload can
                    // legitimately take a while, and the server sends nothing back while
                    // it reads it.
                    last_event = Instant::now();
                }
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                // `Disconnected` means the handle was dropped -- nobody can ever
                // cancel or observe this render again; nothing further to do but keep
                // draining until DONE so the connection ends cleanly rather than being
                // torn down mid-message.
            }
        }

        let Some((event, payload)) = try_read_stream_event(&mut stream, super::POLL_INTERVAL)?
        else {
            // Nothing arrived within this poll -- distinct from an actual I/O error
            // (see `RemoteError::WorkerSilent`'s own doc comment): if this has gone on
            // longer than the currently-applicable deadline, the worker has stopped
            // saying anything at all and this gives up rather than polling a dead
            // connection forever.
            let deadline = super::liveness_deadline(seen_first_event, liveness_timeout);
            if last_event.elapsed() > deadline {
                return Err(RemoteError::WorkerSilent(last_event.elapsed()));
            }
            ensure_cancel_acknowledged(cancel_sent_at)?;
            continue;
        };
        last_event = Instant::now();
        seen_first_event = true;

        // The server needs the scene's HDR map before it can start. Answered
        // inline (this thread owns the stream); the upload's own writes are bounded by
        // the socket's write timeout, and the clock restarts once it is on the wire.
        if let StreamEvent::NeedAsset { content_hash } = &event {
            super::asset_upload::send_requested_asset(&mut stream, content_hash)
                .map_err(RemoteError::Asset)?;
            last_event = Instant::now();
            continue;
        }

        let outcome = {
            let mut acc = accumulator.lock().unwrap_or_else(PoisonError::into_inner);
            acc.apply(&event, payload.as_deref())
                .map_err(|e| RemoteError::Client(e.into()))?
        };

        let is_terminal = matches!(
            outcome,
            ApplyOutcome::Done { .. } | ApplyOutcome::WorkerError
        );
        if let Some(update) = to_remote_update(request_id, &event, outcome) {
            on_update(update);
        }
        if is_terminal {
            return Ok(());
        }
        ensure_cancel_acknowledged(cancel_sent_at)?;
    }
}

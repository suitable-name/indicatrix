//! The persistent connection lifecycle: one connection reused across many renders --
//! [`spawn_remote_connection`], its worker body `run_connection`, and the per-request
//! dispatch [`dispatch_render`] that drives it.

use super::{
    super::types::{
        ConnectionCommand, CurrentRequest, RemoteConnectionHandle, RemoteError, RemoteStream,
        RemoteUpdate, RenderCommand,
    },
    handshake::connect_and_handshake,
    stream_io::try_read_stream_event,
};
use crate::settings::WorkerSettings;
use std::{
    sync::mpsc::{self, TryRecvError},
    thread,
    time::{Duration, Instant},
};

/// Spawns a background thread that owns one persistent mutual-TLS connection to
/// `worker`, reused across every [`RemoteConnectionHandle::render`] call against the
/// returned handle -- see the module doc comment's "Two connection lifecycles" section.
///
/// Connects lazily: no socket is opened, and no handshake cost paid, until the first
/// [`RemoteConnectionHandle::render`] call actually needs one -- a worker configured but
/// never used in a session costs nothing beyond one idle channel and one idle thread.
#[must_use]
pub fn spawn_remote_connection(worker: WorkerSettings) -> RemoteConnectionHandle {
    let (tx, rx) = mpsc::channel();
    let worker_for_thread = worker.clone();
    thread::spawn(move || run_connection(&worker_for_thread, &rx, super::LIVENESS_TIMEOUT));
    RemoteConnectionHandle {
        worker,
        commands: tx,
    }
}

/// The background loop behind [`spawn_remote_connection`]: owns `worker`'s connection
/// (lazily -- `stream`/`welcome` start `None`) for as long as `commands` stays open,
/// reconnecting transparently whenever it's found dead rather than exiting after one
/// request the way `super::one_shot::run` does. Mirrors that function's
/// read-and-interleave-commands loop shape; the difference is entirely at the top
/// level -- no `DONE` here ever ends the loop, only `commands` disconnecting (the
/// handle was dropped) does.
///
/// `liveness_timeout` is [`super::LIVENESS_TIMEOUT`] via [`spawn_remote_connection`] in
/// production; threaded as a parameter so `super::tests`'s `check_liveness` tests can
/// exercise the decision with a much shorter deadline.
fn run_connection(
    worker: &WorkerSettings,
    commands: &mpsc::Receiver<ConnectionCommand>,
    liveness_timeout: Duration,
) {
    let mut stream: Option<RemoteStream> = None;
    let mut welcome: Option<indicatrix_net::messages::Welcome> = None;
    let mut current: Option<CurrentRequest> = None;
    // Reset on every received `StreamEvent` and every fresh dispatch (a reconnect or a
    // request sent on a reused connection both count as a sign of life) -- see
    // `LIVENESS_TIMEOUT`'s own doc comment.
    let mut last_event = Instant::now();
    // Whether the CURRENT request (if any) has produced any stream event yet -- selects
    // `FIRST_EVENT_TIMEOUT` (before) or `liveness_timeout` (after) via
    // `liveness_deadline`, exactly like `run_with_liveness_timeout`'s own
    // `seen_first_event`. Reset alongside `last_event` on every fresh dispatch: a
    // persistent connection serves many requests over its life, and each one gets its
    // own full first-event grace, not whatever was left over from the last one.
    let mut seen_first_event = false;

    loop {
        match commands.try_recv() {
            Ok(ConnectionCommand::Render(cmd)) => {
                dispatch_render(worker, &mut stream, &mut welcome, &mut current, *cmd);
                last_event = Instant::now();
                seen_first_event = false;
            }
            Ok(ConnectionCommand::Cancel { request_id }) => {
                // A no-op if `request_id` isn't (or is no longer) the current request --
                // see `ConnectionCommand::Cancel`'s own doc comment for why that's safe.
                if current.as_ref().is_some_and(|c| c.request_id == request_id)
                    && let Some(s) = stream.as_mut()
                    && indicatrix_net::client::send_cancel(s, request_id).is_err()
                {
                    // The connection is dead -- the next Render dispatch reconnects
                    // fully rather than trying to salvage this write.
                    stream = None;
                    welcome = None;
                }
            }
            Err(TryRecvError::Empty) => {}
            // The handle was dropped -- `RemoteConnectionHandle`'s own doc comment
            // commits to tearing this connection down (window close, a worker
            // address/cert_dir edit, removal from the list), so this exits immediately
            // rather than `super::one_shot::run`'s one-shot "drain until DONE" courtesy:
            // there is no caller left to observe a DONE even if this waited for one.
            Err(TryRecvError::Disconnected) => return,
        }

        let Some(s) = stream.as_mut() else {
            // Nothing connected (never dialed yet, or just found dead above/below) and
            // nothing queued to prompt a dial -- avoid busy-looping `try_recv` against a
            // socket that doesn't exist.
            thread::sleep(super::POLL_INTERVAL);
            continue;
        };

        match try_read_stream_event(s, super::POLL_INTERVAL) {
            Ok(Some((event, payload))) => {
                last_event = Instant::now();
                seen_first_event = true;
                // The server needs the scene's HDR map first -- send it inline.
                // A map this viewer cannot send fails the request and drops the
                // connection (the server would otherwise wait for bytes that never come).
                if let indicatrix_net::messages::StreamEvent::NeedAsset { content_hash } = &event {
                    if !answer_need_asset(s, content_hash, &mut current) {
                        stream = None;
                        welcome = None;
                    }
                    last_event = Instant::now();
                    continue;
                }
                // v14: a coordinator re-advertises its capacity mid-connection. Keep
                // the cached WELCOME current, so the next dispatch's render-capacity
                // check sees the new capability rather than the one from connect time.
                if let indicatrix_net::messages::StreamEvent::CapabilityChanged { render } = &event
                    && let Some(w) = welcome.as_mut()
                {
                    w.render.clone_from(render);
                }
                super::route_event(&mut current, &event, payload.as_deref());
            }
            Ok(None) => {
                // Nothing pending within this poll -- if a request has been waiting
                // this long for ANY sign of life (the currently-applicable deadline --
                // see `liveness_deadline`'s own doc comment for why that's not always
                // `liveness_timeout`), give up on it (see `check_liveness`'s own doc
                // comment) and drop the connection so the next dispatch reconnects
                // rather than reusing a worker that's gone quiet.
                let deadline = super::liveness_deadline(seen_first_event, liveness_timeout);
                if super::check_liveness(&mut current, last_event, deadline) {
                    stream = None;
                    welcome = None;
                }
            }
            Err(e) => {
                // A transport error mid-stream: report it for whatever was in flight
                // (matching `super::one_shot::run`'s own top-level `Err` ->
                // `RemoteUpdate::Failed` handling) and drop the connection -- the NEXT
                // Render dispatch reconnects rather than this thread ever retrying on
                // its own; see `RemoteConnectionHandle`'s "Reconnection" doc section.
                if let Some(mut cur) = current.take() {
                    (cur.on_update)(RemoteUpdate::Failed {
                        request_id: cur.request_id,
                        message: RemoteError::from(e).to_string(),
                    });
                }
                stream = None;
                welcome = None;
            }
        }
    }
}

/// Sends the HDR map a `NEED_ASSET` names (see `super::asset_upload`). On failure the
/// current request is failed with the reason and `false` tells the caller to drop the
/// connection.
fn answer_need_asset(
    stream: &mut RemoteStream,
    hash: &indicatrix_net::messages::ContentHash,
    current: &mut Option<CurrentRequest>,
) -> bool {
    let Err(reason) = super::asset_upload::send_requested_asset(stream, hash) else {
        return true;
    };
    if let Some(mut cur) = current.take() {
        (cur.on_update)(RemoteUpdate::Failed {
            request_id: cur.request_id,
            message: RemoteError::Asset(reason).to_string(),
        });
    }
    false
}

/// The live view's stream configuration: the worker's own [`WorkerSettings::stream_config`]
/// for full-data transfer, or -- final-picture transfer, `display_only` -- the same cadence
/// with `TransferMode::DisplayOnly` and no preview stream (the finished display frames
/// ARE the preview).
pub(super) fn live_stream_config(
    worker: &WorkerSettings,
    min_cadence_ms: u32,
    width: u32,
    height: u32,
    display_only: bool,
) -> indicatrix_net::messages::StreamConfig {
    let mut config = worker.stream_config(min_cadence_ms, width, height);
    if display_only {
        config.transfer_mode = indicatrix_net::messages::TransferMode::DisplayOnly;
        config.preview = None;
    }
    config
}

/// Handles one [`ConnectionCommand::Render`]: supersedes whatever was previously
/// current, connects/reconnects on demand, checks render capacity, and sends the
/// request -- everything `super::one_shot::run` does once per call, except this can run
/// many times against the same `stream`/`welcome` across the life of `run_connection`'s
/// loop.
fn dispatch_render(
    worker: &WorkerSettings,
    stream: &mut Option<RemoteStream>,
    welcome: &mut Option<indicatrix_net::messages::Welcome>,
    current: &mut Option<CurrentRequest>,
    cmd: RenderCommand,
) {
    let RenderCommand {
        request,
        accumulator,
        mut on_update,
    } = cmd;
    let request_id = request.request_id;

    // Supersede whatever was still in flight: best-effort CANCEL on the wire (best
    // -effort because a dead connection is about to be recreated below regardless, and
    // because the REAL correctness guard is the new accumulator's own epoch -- see
    // `RemoteConnectionHandle::render`'s doc comment), then drop the old on_update/
    // accumulator so nothing reachable from here ever reports for it again.
    if let Some(previous) = current.take()
        && let Some(s) = stream.as_mut()
    {
        let _ = indicatrix_net::client::send_cancel(s, previous.request_id);
    }

    if stream.is_none() {
        match connect_and_handshake(worker) {
            Ok((s, w)) => {
                *stream = Some(s);
                *welcome = Some(w);
            }
            Err(e) => {
                on_update(RemoteUpdate::Failed {
                    request_id,
                    message: e.to_string(),
                });
                return; // this request never started -- `current` stays `None`
            }
        }
    }
    let w = welcome
        .as_ref()
        .expect("stream is Some at this point, and the two are always set together");
    on_update(RemoteUpdate::Connected {
        request_id,
        info: w.clone().into(),
    });

    let Some(capability) = w.render.as_ref() else {
        on_update(RemoteUpdate::Failed {
            request_id,
            message: RemoteError::NoRenderCapacity.to_string(),
        });
        return;
    };
    // HDR guard: never send an HDR scene to a remote that does not advertise HDR support.
    if request.scene.hdr().is_some() && !capability.hdr {
        on_update(RemoteUpdate::Failed {
            request_id,
            message: RemoteError::HdrUnsupported.to_string(),
        });
        return;
    }

    let render_request = indicatrix_net::messages::RenderRequest {
        request_id,
        scene: request.scene,
        first_sample: request.first_sample,
        samples: request.samples,
        stream: live_stream_config(
            worker,
            capability.min_cadence_ms,
            request.width,
            request.height,
            request.display_only,
        ),
        intent: request.intent,
    };

    {
        let mut acc = accumulator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Declares the requested range too: a FRAME outside it is a protocol
        // bug and is never summed -- see `Accumulator::begin_request_for_range`.
        acc.begin_request_for_range(request_id, request.first_sample, request.samples);
    }

    let s = stream
        .as_mut()
        .expect("just connected above if this was None");
    match indicatrix_net::client::send_render_request(s, &render_request) {
        Ok(()) => {
            *current = Some(CurrentRequest {
                request_id,
                accumulator,
                on_update,
            });
        }
        Err(e) => {
            // The write failed -- the connection is dead. Drop it (the next dispatch
            // reconnects) and report failure now rather than leaving `current` pointing
            // at a request that was never actually sent.
            *stream = None;
            *welcome = None;
            on_update(RemoteUpdate::Failed {
                request_id,
                message: RemoteError::from(e).to_string(),
            });
        }
    }
}

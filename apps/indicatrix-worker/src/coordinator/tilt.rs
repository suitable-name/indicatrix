//! `TILT_CURVES` on a coordinator: computed on the own lane when there is one
//! (the plain worker's handler, inline), otherwise forwarded whole to ONE idle joined
//! worker and its reply relayed under the viewer's `request_id`.

use super::{
    Registry, ViewerSession,
    lanes::{POLL, PatientReader, WRITE_TIMEOUT, next_worker_request_id},
};
use crate::{
    serve::{CancelPoll, poll_for_cancel},
    stream_emit::TimeoutRead,
    validate,
};
use indicatrix_net::messages::{
    ClientMessage, ErrorMsg, NetError, TiltCurvesRequest, TiltCurvesResponse, error_codes,
};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

use super::{LaneTimeouts, WorkerConn, WorkerHandle};

/// How long a tilt request waits for an idle joined worker.
const WORKER_WAIT: Duration = Duration::from_secs(5);

/// A worker computing tilt curves sends nothing until its one reply (~1.4 s in a release
/// build, ~35 s unoptimised), so silence is only judged against the whole computation.
const TILT_TIMEOUTS: LaneTimeouts = LaneTimeouts {
    first_event: Duration::from_secs(120),
    liveness: Duration::from_secs(120),
    cancel_wait: Duration::from_secs(10),
};

/// Serves one `TiltCurvesRequest` (see the module doc comment).
///
/// # Errors
///
/// [`NetError`] for a transport failure on the viewer connection.
pub fn serve_tilt<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &TiltCurvesRequest,
    session: &ViewerSession,
) -> Result<(), NetError> {
    let coordinator = &session.coordinator;
    if coordinator.own().is_some() {
        return crate::serve::handle_tilt_curves_request(stream, request);
    }
    if let Err(message) = validate::validate_scene(&request.scene) {
        return reply(
            stream,
            &TiltCurvesResponse::Error(ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message,
            }),
        );
    }
    let Some(handle) = coordinator.registry().and_then(wait_for_idle_worker) else {
        return reply(
            stream,
            &TiltCurvesResponse::Error(ErrorMsg {
                code: error_codes::NO_RENDER_CAPACITY,
                message:
                    "no idle joined worker to compute the tilt curves on, and this coordinator \
                          has no own render lane (--render)"
                        .to_string(),
            }),
        );
    };
    let result = forward(stream, request, handle);
    let _ = stream.set_read_timeout(None);
    result
}

fn reply<S: Write>(stream: &mut S, response: &TiltCurvesResponse) -> Result<(), NetError> {
    indicatrix_net::messages::write_message(stream, response)
}

/// Arms the worker socket's deadlines and writes `message` to it.
fn send_request(mut conn: &mut dyn WorkerConn, message: &ClientMessage) -> Result<(), String> {
    conn.set_timeouts(Some(POLL), Some(WRITE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    indicatrix_net::messages::write_message(&mut conn, message).map_err(|e| e.to_string())
}

/// Checks out any idle worker, waiting up to [`WORKER_WAIT`].
fn wait_for_idle_worker(registry: &std::sync::Arc<Registry>) -> Option<WorkerHandle> {
    let deadline = Instant::now() + WORKER_WAIT;
    loop {
        if let Some(handle) = Registry::checkout(registry, |_| true) {
            return Some(handle);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// Sends the request to the worker under a coordinator-local id and relays its reply;
/// a viewer `CANCEL` (or hang-up) is passed on as the worker's `CANCEL`.
fn forward<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &TiltCurvesRequest,
    mut handle: WorkerHandle,
) -> Result<(), NetError> {
    let worker_id = handle.info().worker_id;
    let local_id = next_worker_request_id();
    let message = ClientMessage::TiltCurvesRequest(Box::new(TiltCurvesRequest {
        request_id: local_id,
        scene: request.scene.clone(),
    }));
    let mut viewer_gone = false;
    let outcome = {
        let conn = handle.stream();
        send_request(conn, &message).and_then(|()| {
            let mut reader = PatientReader::new(conn, local_id, TILT_TIMEOUTS, || {
                viewer_gone
                    || match poll_for_cancel(stream, request.request_id) {
                        Ok(CancelPoll::Pending) => false,
                        Ok(CancelPoll::Cancelled | CancelPoll::Closed) | Err(_) => {
                            viewer_gone = true;
                            true
                        }
                    }
            });
            indicatrix_net::messages::read_message::<_, TiltCurvesResponse>(&mut reader)
                .map_err(|e| e.to_string())
        })
    };
    let response = match outcome {
        Ok(response) => {
            let _ = handle.stream().set_timeouts(None, None);
            drop(handle);
            relabel(response, request.request_id, worker_id)
        }
        Err(why) => {
            handle.discard(&format!("tilt-curves request failed: {why}"));
            TiltCurvesResponse::Error(ErrorMsg {
                code: error_codes::ALL_WORKERS_LOST,
                message: format!("the joined worker computing the tilt curves was lost ({why})"),
            })
        }
    };
    if viewer_gone {
        // The viewer cancelled (it gets the worker's `Cancelled`, or the curves if they
        // finished first -- as from a plain worker) or hung up (the write just fails).
        let _ = reply(stream, &response);
        return Ok(());
    }
    reply(stream, &response)
}

/// The worker's reply under the viewer's `request_id`; a worker `ERROR` is re-worded,
/// never forwarded verbatim.
fn relabel(response: TiltCurvesResponse, request_id: u32, worker_id: u32) -> TiltCurvesResponse {
    match response {
        TiltCurvesResponse::Curves(mut result) => {
            result.request_id = request_id;
            TiltCurvesResponse::Curves(result)
        }
        TiltCurvesResponse::Cancelled { .. } => TiltCurvesResponse::Cancelled { request_id },
        TiltCurvesResponse::Error(e) => TiltCurvesResponse::Error(ErrorMsg {
            code: e.code,
            message: format!(
                "joined worker #{worker_id} could not compute the tilt curves (worker error code {})",
                e.code
            ),
        }),
    }
}

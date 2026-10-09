//! The post-`WELCOME` request loop, shared by every render-capable connection whoever
//! dialled it: a viewer connection `serve` accepted, and a
//! `join`ed worker's outbound connection to its coordinator. On the latter the roles
//! invert at the application layer -- the worker is the TLS/TCP client but the PROTOCOL
//! server: it reads `ClientMessage`s and answers `StreamEvent`s, exactly as here.

use super::{NO_RENDER_CAPACITY_CODE, batch, handle_non_render_message};
use crate::{
    assets::{self, AssetCache, Fetched, HdrRoute, HeldAsset},
    cli::ComputeMode,
    coordinator::{self, CapabilityWatch, ViewerSession},
    serve::library::LibraryHandle,
    stream_emit::{self, StreamOutcome, TimeoutRead, TimeoutWrite, is_stream_timeout},
    validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    framing::{FramingError, IDLE_READ_TIMEOUT},
    messages::{
        BatchRenderRequest, ClientMessage, ErrorMsg, FinalImageRequest, NetError, PayloadEncoding,
        RenderRequest, StreamEvent, TiltCurvesRequest, TransferMode, adaptive::PeerLink,
        error_codes,
    },
};
use std::{
    io::{Read, Write},
    sync::Arc,
};

/// `<- ERROR`/`TiltCurvesResponse::Error` code for a request whose embedded scene
/// failed [`validate::validate_scene`] -- shared verbatim between `RenderRequest`
/// and `TiltCurvesRequest` validation failures even though the reply envelope
/// differs.
pub const VALIDATION_FAILED_CODE: u32 = error_codes::VALIDATION_FAILED;
/// `<- ERROR`/`TiltCurvesResponse::Error` code for a `indicatrix` panic caught by
/// `catch_unwind` on a validation-passing but pathological scene; shared with
/// [`VALIDATION_FAILED_CODE`]'s reasoning.
pub const TRACE_PANIC_CODE: u32 = error_codes::TRACE_PANIC;

/// Everything [`serve_requests`] needs for one connection.
pub struct RequestContext<'a> {
    /// CPU tracer threads (`0` = all cores).
    pub threads: usize,
    /// The process's shared GPU backend (disabled when this machine renders on the CPU
    /// only, or has no own lane at all).
    pub gpu: &'a Arc<GpuBackend>,
    /// The connection's library, opened by its first library request (never by any other
    /// kind of request), or `None` on a `join`ed worker's connection (the coordinator
    /// serves the library).
    pub db: Option<&'a LibraryHandle>,
    /// `--only-gpu`/`--only-cpu`/hybrid for the own lane.
    pub compute_mode: ComputeMode,
    /// The encoding negotiated in `WELCOME`: what a request sends when there is no
    /// [`Self::link`].
    pub payload_encoding: PayloadEncoding,
    /// The connection's adaptive-compression state, shared by every request on it.
    pub link: Option<&'a Arc<PeerLink>>,
    /// Whether this machine renders requests itself (`serve --render`, or any `join`ed
    /// worker). Without it (and without a [`Self::session`]) a `RenderRequest` is refused
    /// (see [`refuse_without_own_lane`]).
    pub own_lane: bool,
    /// A coordinator's viewer connection: requests execute through
    /// [`crate::coordinator`] (own lane directly, or a job over joined workers), and
    /// capacity changes are announced between requests. `None` on a `join`ed worker's
    /// connection and a plain single worker.
    pub session: Option<&'a ViewerSession>,
    /// The HDR asset cache (v14): with it, requests whose scene names an HDR map
    /// are resolved first (`crate::assets::ensure_environment`); without it they are
    /// refused (see `crate::assets::hdr_route`).
    pub assets: Option<&'a AssetCache>,
}

/// Writes the `UNSUPPORTED_REQUEST` reply for a well-formed request this server does
/// not implement yet -- the connection stays open for the next one.
fn refuse_unsupported<S: Write>(
    stream: &mut S,
    what: &str,
    request_id: u32,
) -> Result<(), NetError> {
    tracing::info!("refusing an unsupported request: {what}");
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            message: format!("{what} is not supported by this server (a plain worker)"),
            request_id: Some(request_id),
        }),
        None,
    )
}

/// The reply to a `RenderRequest` on a connection with no render lane and no
/// coordinator behind it (its `WELCOME.render` was `None`, so a well-behaved viewer
/// never sends one): `NO_RENDER_CAPACITY`. A coordinator's viewer connection never gets
/// here -- [`coordinator::serve_render`] words its own refusals.
fn refuse_without_own_lane<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    let error = ErrorMsg {
        code: NO_RENDER_CAPACITY_CODE,
        message: format!(
            "RenderRequest (request_id={request_id}): this server has no render lane; its WELCOME \
             advertised no render capability"
        ),
        request_id: Some(request_id),
    };
    tracing::info!("{}", error.message);
    indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)
}

/// The post-`WELCOME` request loop: dispatches `Cancel`/`Library`/`Ping` inline and
/// runs each `RenderRequest` (validate, then stream) and `TiltCurvesRequest` until the
/// peer closes the connection.
///
/// Each `RenderRequest` usually comes from a fresh [`read_next_message`] call, but may
/// already be in hand if the client pipelined it ahead of the previous request's `DONE`
/// (handed back by [`stream_emit::run_stream`]). `TiltCurvesRequest` supports no
/// pipelining and goes straight to [`crate::serve::tilt::handle_tilt_curves_request`].
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. A validation failure or caught
/// tracing panic is NOT an error return -- both are reported to the peer as a
/// `StreamEvent::Error` and the loop continues.
pub fn serve_requests<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    // A `RenderRequest` `run_stream` already pulled off the wire while streaming the
    // previous request (client-pipelined ahead of its `DONE`), to be processed next
    // iteration as if freshly read. `None` means read one instead.
    let mut pending_request: Option<RenderRequest> = None;
    // A coordinator viewer connection announces capacity changes between requests.
    let mut watch = ctx.session.and_then(CapabilityWatch::new);

    loop {
        let next = match pending_request.take() {
            Some(r) => NextRequest::Render(r),
            None => match read_next_message(stream, ctx.db, watch.as_mut())? {
                Some(n) => n,
                // The peer closing the connection is the normal end of this loop.
                None => return Ok(()),
            },
        };
        // A coordinator does not forward zones to its joined workers and never advertises the
        // zoning capability; a payload that arrives anyway is refused, not dropped silently
        // (the picture would render as its base zone).
        #[cfg(feature = "zoning")]
        if ctx.session.is_some() && refuse_zoning_through_coordinator(stream, &next)? {
            continue;
        }
        // An HDR scene's map is resolved (and pinned for the whole request)
        // before the request is served at all.
        let hdr_pin = match prepare_hdr(stream, &next, ctx)? {
            Prepared::Serve(pin) => pin,
            Prepared::Skip(next_request) => {
                pending_request = next_request.map(|next| *next);
                continue;
            }
            Prepared::Closed => return Ok(()),
        };

        pending_request = match (next, ctx.session) {
            (NextRequest::Render(request), Some(session)) => {
                coordinator::serve_render(stream, request, session, hdr_pin.held.clone())?
            }
            (NextRequest::Render(request), None) if ctx.own_lane => {
                serve_one_render(stream, request, ctx)?
            }
            (NextRequest::Render(request), None) => {
                refuse_without_own_lane(stream, request.request_id)?;
                None
            }
            (NextRequest::FinalImage(request), Some(session)) => {
                coordinator::serve_final_image(stream, &request, session, hdr_pin.held.clone())?
            }
            (NextRequest::FinalImage(request), None) => {
                refuse_unsupported(
                    stream,
                    &format!("FinalImageRequest (request_id={})", request.request_id),
                    request.request_id,
                )?;
                None
            }
            (NextRequest::TiltCurves(tilt_request), Some(session)) => {
                coordinator::serve_tilt(stream, &tilt_request, session)?;
                None
            }
            // A single request/response call (its own validation, catch_unwind, reply
            // envelope) -- unlike the validate-then-stream dance of a render.
            (NextRequest::TiltCurves(tilt_request), None) => {
                crate::serve::tilt::handle_tilt_curves_request(stream, &tilt_request)?;
                None
            }
            // v24: a batch of finished pictures, served on the own lane (a coordinator
            // without one refuses it, and the viewer falls back to single requests). It
            // keeps serving later batches that arrive while it runs, so it returns only
            // once none is unfinished.
            (NextRequest::Batch(batch_request), _) => {
                batch::serve_batches(stream, batch_request, ctx)?;
                None
            }
        };
        // Served: the request no longer needs its HDR map pinned.
        drop(hdr_pin);
    }
}

/// `zoning` builds: refuses `next` with `UNSUPPORTED_REQUEST` when the viewer sent a
/// `ZoningPayload` for it on a coordinator connection. `Ok(true)` means the request was
/// refused and must not be served.
#[cfg(feature = "zoning")]
fn refuse_zoning_through_coordinator<S: Write>(
    stream: &mut S,
    next: &NextRequest,
) -> Result<bool, NetError> {
    let request_id = match next {
        NextRequest::Render(r) => r.request_id,
        NextRequest::FinalImage(r) => r.request_id,
        NextRequest::TiltCurves(r) => r.request_id,
        NextRequest::Batch(r) => r.request_id,
    };
    if crate::serve::zoning::take(request_id).is_none() {
        return Ok(false);
    }
    tracing::info!(
        "refusing request {request_id}: zoned materials are not served through a coordinator"
    );
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            message: format!(
                "request {request_id}: a coordinator does not serve zoned materials (its WELCOME \
                 does not advertise the zoning capability); render the picture locally"
            ),
            request_id: Some(request_id),
        }),
        None,
    )?;
    Ok(true)
}

/// Validates and streams one `RenderRequest` on the own lane; returns the client's
/// already-pipelined next `RenderRequest`, if `run_stream` pulled one off the wire.
fn serve_one_render<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    mut request: RenderRequest,
    ctx: &RequestContext<'_>,
) -> Result<Option<RenderRequest>, NetError> {
    // v14 "final picture only" live transfer: display frames need the coordinator's
    // tone-map + denoise path, which a plain worker lacks.
    if request.stream.transfer_mode == TransferMode::DisplayOnly {
        refuse_unsupported(
            stream,
            &format!(
                "TransferMode::DisplayOnly (request_id={})",
                request.request_id
            ),
            request.request_id,
        )?;
        return Ok(None);
    }

    // `zoning` builds: re-attach the zones the viewer sent ahead of this request (they are
    // `serde(skip)` in the scene). A payload that does not fit refuses the request.
    #[cfg(feature = "zoning")]
    if let Err(message) =
        crate::serve::zoning::attach_to_render(request.request_id, &mut request.scene)
    {
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::Error(ErrorMsg {
                code: VALIDATION_FAILED_CODE,
                message,
                request_id: Some(request.request_id),
            }),
            None,
        )?;
        return Ok(None);
    }

    // `validate_stream_config` takes `&mut request.stream` (it clamps `cadence_ms` in
    // place) alongside `&request.scene` -- two disjoint field borrows.
    if let Err(msg) =
        validate::validate_request(&request.scene, request.first_sample, request.samples)
            .and_then(|()| validate::validate_stream_config(&mut request.stream, &request.scene))
    {
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::Error(ErrorMsg {
                code: VALIDATION_FAILED_CODE,
                message: msg,
                request_id: Some(request.request_id),
            }),
            None,
        )?;
        return Ok(None);
    }

    // Runs the tracer on its own thread (never touching `stream`) and this thread as
    // the emitter -- see `stream_emit`'s module docs. The tracer thread runs inside
    // `catch_unwind`, surfaced here as [`StreamOutcome::TracePanicked`].
    let started = std::time::Instant::now();
    let (outcome, next) = stream_emit::run_stream(
        stream,
        &request,
        ctx.threads,
        ctx.gpu,
        ctx.compute_mode,
        ctx.payload_encoding,
        ctx.link,
    )?;
    // The one line a request that SUCCEEDS leaves on the console: without it "went
    // quiet" cannot tell "stopped being asked" from "stopped answering".
    tracing::info!(
        request_id = request.request_id,
        intent = ?request.intent,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        samples = request.samples,
        route = "plain worker",
        elapsed = ?started.elapsed(),
        %outcome,
        "worker: served a render request"
    );
    let error = match outcome {
        StreamOutcome::Completed => None,
        StreamOutcome::TracePanicked => {
            tracing::warn!(
                "tracing panicked for a request that passed validation (request_id={}, first_sample={}, samples={})",
                request.request_id,
                request.first_sample,
                request.samples
            );
            Some(ErrorMsg {
                code: TRACE_PANIC_CODE,
                message: "internal error while tracing this request".to_string(),
                request_id: Some(request.request_id),
            })
        }
        // `stream_emit::run_stream` already stamped this error's `request_id`.
        StreamOutcome::Failed(error) => Some(error),
    };
    if let Some(error) = error {
        indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
    }
    Ok(next)
}

/// What a request holds while it is served: nothing (studio-lit), the decoded
/// map (a plain or joined worker), or the job's [`HeldAsset`] (a coordinator; it also
/// pins the own lane's decoded map, if any).
struct HdrPin {
    /// Only held (dropped when the request has been served).
    _map: Option<Arc<indicatrix::renderer::env_map::EnvironmentMap>>,
    /// A coordinator's job asset, handed to the job.
    held: Option<Arc<HeldAsset>>,
}

impl HdrPin {
    /// Nothing to hold (studio-lit, or refused elsewhere).
    const NONE: Self = Self {
        _map: None,
        held: None,
    };

    /// A decoded map, pinned.
    const fn map(map: Arc<indicatrix::renderer::env_map::EnvironmentMap>) -> Self {
        Self {
            _map: Some(map),
            held: None,
        }
    }

    /// A coordinator's held asset.
    const fn held(held: Arc<HeldAsset>) -> Self {
        Self {
            _map: None,
            held: Some(held),
        }
    }
}

/// What [`prepare_hdr`] decided for one request.
enum Prepared {
    /// Serve the request, holding this pin until it has been served.
    Serve(HdrPin),
    /// Do not serve it (refused, failed or cancelled -- the reply was already sent);
    /// serve this pipelined request next, if any.
    Skip(Option<Box<RenderRequest>>),
    /// The peer closed the connection.
    Closed,
}

/// Resolves the HDR map a render or final-image request names (v14): the
/// policy first ([`assets::hdr_route`]), then [`assets::ensure_environment`] -- or, on a
/// coordinator's viewer connection, [`assets::ensure_held`] through the coordinator's
/// own cache (decoding only for its own lane). A request that fails validation is
/// passed through untouched -- its own path refuses it with the usual
/// `VALIDATION_FAILED`, and no asset is fetched for it.
fn prepare_hdr<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    next: &NextRequest,
    ctx: &RequestContext<'_>,
) -> Result<Prepared, NetError> {
    let (request_id, scene, first_sample, samples, cadence_ms) = match next {
        // Requests this connection refuses anyway (no own lane; a final picture on a
        // plain worker) are not worth an asset transfer.
        NextRequest::Render(_) if ctx.session.is_none() && !ctx.own_lane => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        NextRequest::FinalImage(_) if ctx.session.is_none() => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        // A batch item naming an HDR map fails by itself (`batch::check_item`); no asset is
        // fetched for a batch (nor for a tilt-curves request).
        NextRequest::Batch(_) | NextRequest::TiltCurves(_) => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        NextRequest::Render(r) => (
            r.request_id,
            &r.scene,
            r.first_sample,
            r.samples,
            r.stream.cadence_ms,
        ),
        NextRequest::FinalImage(r) => (r.request_id, &r.scene, r.first_sample, r.samples, 1000),
    };
    let Some(hdr) = scene.hdr() else {
        return Ok(Prepared::Serve(HdrPin::NONE));
    };
    if validate::validate_request(scene, first_sample, samples).is_err() {
        return Ok(Prepared::Serve(HdrPin::NONE));
    }
    match assets::hdr_route(scene, ctx.assets, ctx.session) {
        HdrRoute::NotHdr => return Ok(Prepared::Serve(HdrPin::NONE)),
        HdrRoute::Refuse(error) => {
            tracing::info!("request {request_id}: {}", error.message);
            indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
            return Ok(Prepared::Skip(None));
        }
        HdrRoute::Serve => {}
    }
    let pending = assets::Pending {
        request_id,
        cadence_ms,
        hdr,
    };
    match (ctx.session, ctx.assets) {
        (Some(session), _) => {
            let coordinator = &session.coordinator;
            let Some(cache) = coordinator.assets() else {
                return Ok(Prepared::Serve(HdrPin::NONE)); // `hdr_route` refuses without one
            };
            let decode = coordinator.own().is_some();
            let fetched = assets::ensure_held(stream, cache, pending, decode)?;
            finish_prepare(stream, fetched, HdrPin::held)
        }
        (None, Some(cache)) => {
            let fetched = assets::ensure_environment(stream, cache, pending)?;
            finish_prepare(stream, fetched, HdrPin::map)
        }
        // `hdr_route` refuses without a cache.
        (None, None) => Ok(Prepared::Serve(HdrPin::NONE)),
    }
}

/// Turns a resolve's outcome into [`Prepared`], writing the error a failed one owes.
fn finish_prepare<S: Write, T>(
    stream: &mut S,
    fetched: Fetched<T>,
    pin: impl FnOnce(T) -> HdrPin,
) -> Result<Prepared, NetError> {
    Ok(match fetched {
        Fetched::Ready(value) => Prepared::Serve(pin(value)),
        Fetched::Failed(error) => {
            indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
            Prepared::Skip(None)
        }
        Fetched::Superseded(next_request) => Prepared::Skip(next_request),
        Fetched::Closed => Prepared::Closed,
    })
}

/// Answers a `PING` (v14) with the matching `PONG`, immediately.
pub fn write_pong<S: Write>(stream: &mut S, nonce: u64) -> Result<(), NetError> {
    indicatrix_net::messages::write_stream_event(stream, &StreamEvent::Pong { nonce }, None)
}

/// What [`read_next_message`] found once it stopped reading. `Cancel`/`Library`/`Ping`
/// are fully handled inline before this ever returns, so this enum only carries the
/// request kinds that end the read loop.
enum NextRequest {
    Render(RenderRequest),
    TiltCurves(TiltCurvesRequest),
    FinalImage(FinalImageRequest),
    Batch(BatchRenderRequest),
}

/// Reads the next message off `stream`: blocking, or -- on a coordinator viewer
/// connection (`watch`) -- polling so capacity changes go out while it waits (see
/// [`coordinator::read_message_watching`]). Dispatches `Cancel`/`Library`/`Ping` inline
/// and keeps reading; only a request ends the wait (see [`NextRequest`]).
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure (including a connection that ends
/// inside a frame). `Ok(None)` (not an error) for a clean EOF between messages, and for
/// a connection that sent nothing for [`IDLE_READ_TIMEOUT`], which is closed with a
/// logged reason.
fn read_next_message<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    db: Option<&LibraryHandle>,
    mut watch: Option<&mut CapabilityWatch>,
) -> Result<Option<NextRequest>, NetError> {
    loop {
        let read = if let Some(watch) = watch.as_deref_mut() {
            coordinator::read_message_watching(stream, watch)
        } else {
            // Arm the idle deadline for this wait: the emitter and the asset fetch
            // reset the socket's read timeout to blocking after they finish. A
            // `join`ed worker's connection has no database; its stream maps
            // "blocking" to its own, shorter coordinator-liveness deadline, which
            // must stay in force.
            let idle = db.is_some().then_some(IDLE_READ_TIMEOUT);
            let _ = stream.set_read_timeout(idle);
            indicatrix_net::messages::read_control_message(stream).map(Some)
        };
        let msg: ClientMessage = match read {
            Ok(Some(m)) => m,
            Ok(None) => return Ok(None),
            Err(NetError::Framing(FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(NetError::Framing(FramingError::Io(e))) if is_stream_timeout(&e) => {
                tracing::info!(
                    "closing a connection that sent nothing for {} s (idle timeout)",
                    IDLE_READ_TIMEOUT.as_secs()
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        match msg {
            ClientMessage::RenderRequest(r) => return Ok(Some(NextRequest::Render(*r))),
            ClientMessage::TiltCurvesRequest(r) => {
                return Ok(Some(NextRequest::TiltCurves(*r)));
            }
            ClientMessage::FinalImageRequest(r) => {
                return Ok(Some(NextRequest::FinalImage(*r)));
            }
            ClientMessage::BatchRenderRequest(r) => return Ok(Some(NextRequest::Batch(*r))),
            ClientMessage::Ping { nonce } => write_pong(stream, nonce)?,
            // The zones of the request that follows (`zoning` builds): kept for it.
            #[cfg(feature = "zoning")]
            ClientMessage::ZoningPayload(payload) => crate::serve::zoning::stash(*payload),
            // An asset nobody asked for: consume its payload frame.
            ClientMessage::Asset(header) => assets::discard_asset(stream, &header)?,
            // A stray v16 contribution (e.g. arriving after this request's own DONE):
            // consume its payload frame to stay in sync.
            ClientMessage::Contribution(header) => {
                indicatrix_net::messages::discard_contribution_payload(stream, &header)?;
            }
            other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                handle_non_render_message(stream, &other, db)?;
            }
        }
    }
}

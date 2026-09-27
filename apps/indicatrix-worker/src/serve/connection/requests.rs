//! The post-`WELCOME` request loop, shared by every render-capable connection whoever
//! dialled it: a viewer connection `serve` accepted, and a
//! `join`ed worker's outbound connection to its coordinator. On the latter the roles
//! invert at the application layer -- the worker is the TLS/TCP client but the PROTOCOL
//! server: it reads `ClientMessage`s and answers `StreamEvent`s, exactly as here.

use super::{NO_RENDER_CAPACITY_CODE, handle_non_render_message};
use crate::{
    assets::{self, AssetCache, Fetched, HdrRoute, HeldAsset},
    cli::ComputeMode,
    coordinator::{self, CapabilityWatch, ViewerSession},
    stream_emit::{self, StreamOutcome, TimeoutRead, TimeoutWrite},
    validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    framing::FramingError,
    messages::{
        ClientMessage, ErrorMsg, FinalImageRequest, NetError, PayloadEncoding, RenderRequest,
        StreamEvent, TiltCurvesRequest, TransferMode, error_codes,
    },
};
use indicatrix_vault::db::sqlite::Database;
use std::{
    io::{Read, Write},
    sync::Arc,
};

/// `<- ERROR`/`TiltCurvesResponse::Error` code for a request whose embedded scene
/// failed [`validate::validate_scene`] -- shared verbatim between `RenderRequest`
/// and `TiltCurvesRequest` validation failures even though the reply envelope
/// differs.
pub const VALIDATION_FAILED_CODE: u32 = 2;
/// `<- ERROR`/`TiltCurvesResponse::Error` code for a `indicatrix` panic caught by
/// `catch_unwind` on a validation-passing but pathological scene; shared with
/// [`VALIDATION_FAILED_CODE`]'s reasoning.
pub const TRACE_PANIC_CODE: u32 = 3;

/// Everything [`serve_requests`] needs for one connection.
pub struct RequestContext<'a> {
    /// CPU tracer threads (`0` = all cores).
    pub threads: usize,
    /// The process's shared GPU backend (disabled when this machine renders on the CPU
    /// only, or has no own lane at all).
    pub gpu: &'a Arc<GpuBackend>,
    /// The library database, or `None` on a `join`ed worker's connection (the
    /// coordinator serves the library).
    pub db: Option<&'a Database>,
    /// `--only-gpu`/`--only-cpu`/hybrid for the own lane.
    pub compute_mode: ComputeMode,
    /// The encoding negotiated in `WELCOME` for every `FRAME`/`PREVIEW` on this connection.
    pub payload_encoding: PayloadEncoding,
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
fn refuse_unsupported<S: Write>(stream: &mut S, what: &str) -> Result<(), NetError> {
    tracing::info!("refusing an unsupported request: {what}");
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            message: format!("{what} is not supported by this server (a plain worker)"),
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
        };
        // Served: the request no longer needs its HDR map pinned.
        drop(hdr_pin);
    }
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
            }),
            None,
        )?;
        return Ok(None);
    }

    // Runs the tracer on its own thread (never touching `stream`) and this thread as
    // the emitter -- see `stream_emit`'s module docs. The tracer thread runs inside
    // `catch_unwind`, surfaced here as [`StreamOutcome::TracePanicked`].
    let (outcome, next) = stream_emit::run_stream(
        stream,
        &request,
        ctx.threads,
        ctx.gpu,
        ctx.compute_mode,
        ctx.payload_encoding,
    )?;
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
            })
        }
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
        NextRequest::Render(r) => (
            r.request_id,
            &r.scene,
            r.first_sample,
            r.samples,
            r.stream.cadence_ms,
        ),
        NextRequest::FinalImage(r) => (r.request_id, &r.scene, r.first_sample, r.samples, 1000),
        NextRequest::TiltCurves(_) => return Ok(Prepared::Serve(HdrPin::NONE)),
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
}

/// Reads the next message off `stream`: blocking, or -- on a coordinator viewer
/// connection (`watch`) -- polling so capacity changes go out while it waits (see
/// [`coordinator::read_message_watching`]). Dispatches `Cancel`/`Library`/`Ping` inline
/// and keeps reading; only a request ends the wait (see [`NextRequest`]).
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. `Ok(None)` (not an error) for
/// a clean EOF.
fn read_next_message<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    db: Option<&Database>,
    mut watch: Option<&mut CapabilityWatch>,
) -> Result<Option<NextRequest>, NetError> {
    loop {
        let read = match watch.as_deref_mut() {
            Some(watch) => coordinator::read_message_watching(stream, watch),
            None => indicatrix_net::messages::read_message(stream).map(Some),
        };
        let msg: ClientMessage = match read {
            Ok(Some(m)) => m,
            Ok(None) => return Ok(None),
            Err(NetError::Framing(FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
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
            ClientMessage::Ping { nonce } => write_pong(stream, nonce)?,
            // An asset nobody asked for: consume its payload frame.
            ClientMessage::Asset(header) => assets::discard_asset(stream, &header)?,
            other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                handle_non_render_message(stream, &other, db)?;
            }
        }
    }
}

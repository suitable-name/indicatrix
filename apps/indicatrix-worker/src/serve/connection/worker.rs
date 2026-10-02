//! The viewer side of a render-capable (`worker`) build: `HELLO`/`WELCOME` with the
//! full build-compatibility gate and the coordinator's render advertisement, then
//! [`super::requests::serve_requests`].

use super::{
    BUILD_MISMATCH_CODE, NO_RENDER_CAPACITY_CODE, handle_non_render_message,
    link::LinkSettings,
    requests::{RequestContext, serve_requests, write_pong},
};
use crate::{
    assets::AssetCache,
    cli::ComputeMode,
    coordinator::{Coordinator, Registry, ViewerSession, viewer_render_capability},
    render_core,
    serve::library::LibraryHandle,
    stream_emit::{self, TimeoutRead, TimeoutWrite, is_stream_timeout},
    validate,
};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    framing::{FramingError, IDLE_READ_TIMEOUT},
    handshake,
    messages::{
        Backend, ClientMessage, ErrorMsg, Hello, LOOPBACK_SERVER_PREFERENCE, NetError,
        PROTOCOL_VERSION, PayloadEncoding, PeerRole, RenderCapability, StreamEvent,
        TiltCurvesResponse, Welcome, adaptive::PeerLink, negotiate,
    },
};
use std::{
    io::{Read, Write},
    sync::Arc,
};

/// Everything one viewer connection needs besides its stream.
pub struct ViewerContext<'a> {
    /// CPU tracer threads for the own lane (`0` = all cores).
    pub threads: usize,
    /// The process's shared GPU backend ([`GpuBackend::disabled`] without an own lane).
    pub gpu: &'a Arc<GpuBackend>,
    /// This connection's read-only library, opened by its first library request (never
    /// by a render, tilt, final-image, ping or asset message).
    pub db: &'a LibraryHandle,
    /// `--only-gpu`/`--only-cpu`/hybrid for the own lane.
    pub compute_mode: ComputeMode,
    /// This server's payload-encoding preference for the connection (v14): what `WELCOME`
    /// announces as the connection's encoding (each frame's own header says what it
    /// carries).
    pub encodings: &'a [PayloadEncoding],
    /// How the connection's frames are compressed (`--payload-encoding`, the peer's
    /// identity and whether it is loopback). `None` pins every frame to the negotiated
    /// encoding without measuring anything.
    pub link: Option<LinkSettings>,
    /// `serve --render`: this machine renders requests itself.
    pub own_lane: bool,
    /// The coordinator's joined-worker registry (`None` without a worker port).
    pub registry: Option<&'a Arc<Registry>>,
    /// The role of the peer's client certificate (`None` for `--insecure-no-tls`).
    pub cert_role: Option<PeerRole>,
    /// A coordinator's viewer connection: the coordinator executing its requests and the
    /// viewer's identity for the one-active-job-per-viewer FIFO. `None` serves requests
    /// like a plain worker (own lane only; no `DisplayOnly`/`FinalImageRequest`).
    pub coordinator: Option<(&'a Arc<Coordinator>, String)>,
    /// The HDR asset cache (v14); `None` refuses HDR scenes and advertises
    /// `RenderCapability::hdr = false`. On a coordinator this is the same cache its
    /// [`Coordinator`] holds jobs' maps through (`Coordinator::with_assets`).
    pub assets: Option<&'a AssetCache>,
}

/// Handles one connection end to end, always on the CPU.
///
/// A thin wrapper around [`handle_connection_with_gpu`] passing
/// [`GpuBackend::disabled`], so this crate's own tests stay deterministic (tracing on
/// the CPU reference path) regardless of `--features gpu`. Uses
/// [`LOOPBACK_SERVER_PREFERENCE`] (raw payloads), what `serve` uses for a loopback peer.
///
/// # Errors
///
/// See [`handle_connection_with_gpu`].
pub fn handle_connection<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: S,
    threads: usize,
    db: &LibraryHandle,
) -> Result<(), NetError> {
    // `ComputeMode` is moot here: the disabled backend below declines every dispatch
    // regardless, so calibration is never attempted.
    handle_connection_with_gpu(
        stream,
        threads,
        &Arc::new(GpuBackend::disabled()),
        db,
        ComputeMode::OnlyCpu,
        &LOOPBACK_SERVER_PREFERENCE,
    )
}

/// Handles one connection end to end as a plain single worker.
///
/// An own render lane, no joined-worker registry, and no certificate role to check
/// (the caller already authenticated the peer, or this is a test double). Exactly
/// `serve --render`'s behaviour before coordinator mode -- see
/// [`handle_viewer_connection`].
///
/// # Errors
///
/// See [`handle_viewer_connection`].
pub fn handle_connection_with_gpu<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: S,
    threads: usize,
    gpu: &Arc<GpuBackend>,
    db: &LibraryHandle,
    compute_mode: ComputeMode,
    encodings: &[PayloadEncoding],
) -> Result<(), NetError> {
    handle_viewer_connection(
        stream,
        &ViewerContext {
            threads,
            gpu,
            db,
            compute_mode,
            encodings,
            link: None,
            own_lane: true,
            registry: None,
            cert_role: None,
            coordinator: None,
            assets: None,
        },
    )
}

/// Handles one viewer connection end to end.
///
/// First the `HELLO`/`WELCOME` handshake: the role gate (viewer `HELLO` and, over TLS,
/// a viewer certificate -- see `crate::serve::handshake`), then the build-hash /
/// source-hash / protocol check ([`handshake::verify_compatible`]). A peer whose `HELLO`
/// reports [`handshake::UNKNOWN_BUILD_HASH`] (a library-only client) is paired as
/// library-only instead (see [`pair_as_library_only_if_unknown_build`]). Then
/// [`serve_requests`] until the peer closes.
///
/// `WELCOME.render` follows [`viewer_render_capability`]: `None`
/// without an own lane and without joined workers; the own lane's plain `Cpu`/`Gpu`
/// backend with `--render` and no workers; `Backend::Coordinator{..}` once workers
/// have joined. `ctx.encodings` is negotiated against the viewer's
/// `HELLO.accept_encodings` into `WELCOME.payload_encoding`.
///
/// Generic over `Read + Write` rather than `TcpStream` (see `crate::serve`'s module
/// docs).
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. A validation failure or caught
/// tracing panic is NOT an error return -- see [`serve_requests`].
pub fn handle_viewer_connection<S: Read + Write + TimeoutRead + TimeoutWrite>(
    mut stream: S,
    ctx: &ViewerContext<'_>,
) -> Result<(), NetError> {
    let check = crate::serve::handshake::read_and_check_hello(
        &mut stream,
        PeerRole::Viewer,
        ctx.cert_role,
    )?;
    // HELLO has arrived -- the pre-protocol deadline the accept loop applied to the raw
    // socket (`serve::HANDSHAKE_TIMEOUT`) has done its job. Reads now wait at most
    // `IDLE_READ_TIMEOUT` for a peer's next message (each request loop re-arms it);
    // writes go back to blocking. Best-effort -- a failed call just leaves the previous
    // deadline in place.
    let _ = stream.set_read_timeout(Some(IDLE_READ_TIMEOUT));
    let _ = stream.set_write_timeout(None);
    let remote_hello = match check {
        crate::serve::handshake::HelloCheck::Accepted(hello) => hello,
        crate::serve::handshake::HelloCheck::Refused(refusal) => {
            crate::serve::handshake::send_refusal(&mut stream, &refusal);
            return Ok(());
        }
    };
    let local_hello = handshake::local_hello();

    if let Some(result) =
        pair_as_library_only_if_unknown_build(&mut stream, &local_hello, &remote_hello, ctx.db)
    {
        return result;
    }

    if let Err(incompatible) = handshake::verify_compatible(&local_hello, &remote_hello) {
        refuse_incompatible_handshake(&mut stream, &local_hello, &remote_hello, incompatible);
        return Ok(());
    }

    let payload_encoding = negotiate(ctx.encodings, &remote_hello.accept_encodings);
    // One adaptive link per connection: the viewer's `HELLO` bounds what it may be sent.
    let link = ctx.link.as_ref().map_or_else(
        || Arc::new(PeerLink::fixed(payload_encoding)),
        |settings| settings.open(&remote_hello.accept_encodings),
    );
    // The own lane renders HDR scenes exactly when it has an asset cache.
    let own = ctx.own_lane.then(|| RenderCapability {
        hdr: ctx.assets.is_some(),
        ..local_render_capability(ctx.gpu, ctx.threads)
    });
    let render = viewer_render_capability(
        own.as_ref(),
        ctx.registry.map(|registry| registry.capacity()),
        ctx.assets.is_some(),
    );
    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        build_hash: local_hello.build_hash,
        source_hash: local_hello.source_hash,
        // Shares the render gate; kept an explicit field (see `Welcome::tilt_curves`).
        tilt_curves: render.is_some(),
        render: render.clone(),
        library: true,
        // A viewer is never registered; only the worker port hands out registrations.
        registration: None,
        payload_encoding,
    };
    indicatrix_net::messages::write_message(&mut stream, &welcome)?;

    let session = ctx
        .coordinator
        .as_ref()
        .map(|(coordinator, viewer)| ViewerSession {
            coordinator: Arc::clone(coordinator),
            viewer: Arc::from(viewer.as_str()),
            payload_encoding,
            link: Some(Arc::clone(&link)),
            advertised: render,
            own_capability: own,
            // Coordinator-PROCESS-wide, not a fresh book per connection -- see
            // `Coordinator::rates`'s doc comment.
            rates: Arc::clone(coordinator.rates()),
        });
    serve_requests(
        &mut stream,
        &RequestContext {
            threads: ctx.threads,
            gpu: ctx.gpu,
            db: Some(ctx.db),
            compute_mode: ctx.compute_mode,
            payload_encoding,
            link: Some(&link),
            own_lane: ctx.own_lane,
            session: session.as_ref(),
            assets: ctx.assets,
        },
    )
}

/// This machine's own render capability: the backend `gpu` will actually trace on
/// (`Gpu` iff an adapter was genuinely acquired, decided once so a worker never claims
/// GPU while silently tracing on CPU), this build's limits. What a plain worker's
/// `WELCOME` advertises and what a `join`ing worker reports in its `HELLO`. `hdr` is
/// `false` here; a caller with an asset cache sets it.
///
/// `--only-cpu` shows up here as a disabled `gpu` (hence `Cpu`); a later per-request
/// decline still falls back silently, as documented in `gpu_backend`.
///
/// Gives a lost device its chance to come back first ([`GpuBackend::try_recover`], which
/// honours the backend's cool-down and attempt budget), so a connection made after a
/// recovery advertises the GPU again instead of the stale CPU fallback.
#[must_use]
pub fn local_render_capability(gpu: &GpuBackend, threads: usize) -> RenderCapability {
    let _ = gpu.try_recover();
    let backend = gpu.adapter_label().map_or_else(
        || Backend::Cpu {
            threads: render_core::effective_thread_count(threads) as u32,
        },
        |adapter| Backend::Gpu { adapter },
    );
    RenderCapability {
        backend,
        max_pixels: validate::MAX_PIXELS,
        min_cadence_ms: stream_emit::MIN_CADENCE_FLOOR_MS,
        hdr: false,
    }
}

/// Logs why [`handshake::verify_compatible`] refused `local_hello`/`remote_hello`
/// and writes an `ErrorMsg` in place of `WELCOME` -- best-effort, like
/// [`super::refuse_for_capacity`].
pub fn refuse_incompatible_handshake<S: Write>(
    stream: &mut S,
    local_hello: &Hello,
    remote_hello: &Hello,
    incompatible: handshake::Incompatible,
) {
    let message = format!(
        "refusing to pair: this side build_hash={:02x?} protocol_version={}, peer build_hash={:02x?} \
         protocol_version={} ({incompatible})",
        local_hello.build_hash,
        local_hello.protocol_version,
        remote_hello.build_hash,
        remote_hello.protocol_version
    );
    tracing::warn!("{message}");
    let _ = indicatrix_net::messages::write_message(
        stream,
        &ErrorMsg {
            code: BUILD_MISMATCH_CODE,
            message,
            // Refused before WELCOME -- no request exists yet.
            request_id: None,
        },
    );
}

/// Pairs `stream` as library-only when `remote_hello` understands this server's
/// protocol version but declares no `indicatrix` build at all
/// (`build_hash == handshake::UNKNOWN_BUILD_HASH`, e.g. a library-only client).
///
/// `Some` means this connection is now fully handled and the caller should return the
/// inner `Result` immediately; `None` means this peer is NOT a library-only downgrade
/// case, and the caller should continue on to the normal [`handshake::verify_compatible`]
/// gate (whose "an unknown build is never compatible" rule would otherwise refuse it).
fn pair_as_library_only_if_unknown_build<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    local_hello: &Hello,
    remote_hello: &Hello,
    db: &LibraryHandle,
) -> Option<Result<(), NetError>> {
    if remote_hello.protocol_version != local_hello.protocol_version
        || remote_hello.build_hash != handshake::UNKNOWN_BUILD_HASH
    {
        return None;
    }
    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        build_hash: local_hello.build_hash,
        source_hash: local_hello.source_hash,
        render: None,
        library: true,
        tilt_curves: false,
        registration: None,
        // No radiance ever flows on a library-only pairing.
        payload_encoding: PayloadEncoding::Raw,
    };
    Some(
        indicatrix_net::messages::write_message(stream, &welcome)
            .and_then(|()| serve_library_only_connection(stream, db)),
    )
}

/// The library-only refusal for a request that needs render capacity.
fn no_render_capacity(what: &str, request_id: u32) -> ErrorMsg {
    ErrorMsg {
        code: NO_RENDER_CAPACITY_CODE,
        message: format!(
            "this connection was paired as library-only (HELLO reported no indicatrix build) and cannot {what}"
        ),
        request_id: Some(request_id),
    }
}

/// Serves a peer whose `HELLO` reported [`handshake::UNKNOWN_BUILD_HASH`] after this
/// server has already sent it a `WELCOME` advertising no render capacity. Dispatches
/// `Cancel`/`Library`/`Ping`; a render, tilt or final-image request is refused with
/// [`NO_RENDER_CAPACITY_CODE`] instead of ever reaching the tracer.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure (including a connection that ends
/// inside a frame). `Ok(())` (not an error) for a clean EOF between messages, and for a
/// connection that sent nothing for [`IDLE_READ_TIMEOUT`], which is closed with a logged
/// reason.
fn serve_library_only_connection<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    db: &LibraryHandle,
) -> Result<(), NetError> {
    loop {
        let _ = stream.set_read_timeout(Some(IDLE_READ_TIMEOUT));
        let msg: ClientMessage = match indicatrix_net::messages::read_control_message(stream) {
            Ok(m) => m,
            Err(NetError::Framing(FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(NetError::Framing(FramingError::Io(e))) if is_stream_timeout(&e) => {
                tracing::info!(
                    "closing a library-only connection that sent nothing for {} s (idle timeout)",
                    IDLE_READ_TIMEOUT.as_secs()
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        match msg {
            ClientMessage::RenderRequest(r) => indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::Error(no_render_capacity("render", r.request_id)),
                None,
            )?,
            ClientMessage::TiltCurvesRequest(r) => indicatrix_net::messages::write_message(
                stream,
                &TiltCurvesResponse::Error(no_render_capacity("compute tilt curves", r.request_id)),
            )?,
            ClientMessage::FinalImageRequest(r) => indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::Error(no_render_capacity("render a final image", r.request_id)),
                None,
            )?,
            ClientMessage::Ping { nonce } => write_pong(stream, nonce)?,
            // Nothing here ever asks for an asset; consume an unrequested one's payload.
            ClientMessage::Asset(header) => crate::assets::discard_asset(stream, &header)?,
            // Nothing here ever asks for a contribution either; consume an unrequested
            // one's payload to stay in sync.
            ClientMessage::Contribution(header) => {
                indicatrix_net::messages::discard_contribution_payload(stream, &header)?;
            }
            other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                handle_non_render_message(stream, &other, Some(db))?;
            }
        }
    }
}

//! Per-connection request handling: the `HELLO`/`WELCOME` handshake and the loop
//! dispatching whatever the peer sends -- `ClientMessage::Library` always, and (only on
//! a `worker` build) `ClientMessage::RenderRequest`/`ClientMessage::TiltCurvesRequest`
//! too -- until the peer closes the connection. See `crate::serve`'s module docs for the
//! full architecture this participates in.

use indicatrix_net::messages::{ClientMessage, ErrorMsg, NetError};
#[cfg(not(feature = "worker"))]
use indicatrix_net::messages::{Hello, PROTOCOL_VERSION, Welcome};
use indicatrix_vault::db::sqlite::Database;
#[cfg(feature = "worker")]
use std::io::Write;
#[cfg(not(feature = "worker"))]
use std::io::{Read, Write};
use std::net::SocketAddr;

pub(super) fn report_connection_result(
    peer: Option<SocketAddr>,
    result: std::thread::Result<Result<(), NetError>>,
) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("connection {peer:?} ended with an error: {e}"),
        Err(_) => tracing::warn!("connection {peer:?} panicked and was dropped"),
    }
}

/// `<- ERROR` code for a `HELLO` this worker refuses to pair with -- a protocol-version
/// mismatch (library-only builds) or (also) a `indicatrix` build-hash mismatch (`worker`
/// builds -- see [`handle_connection_with_gpu`]).
pub(super) const BUILD_MISMATCH_CODE: u32 = 1;

/// `<- ERROR` code for a `RenderRequest` arriving at a worker with no render capacity.
/// Distinct from [`BUILD_MISMATCH_CODE`] on purpose: that one means "we cannot pair at
/// all", this one means "we paired fine, but I only serve the library" -- a client can
/// act on the difference (fall back to local rendering rather than dropping the worker).
pub(super) const NO_RENDER_CAPACITY_CODE: u32 = 2;

/// `<- ERROR` code for [`refuse_for_capacity`]: this worker is already handling
/// `--max-connections` connections and refuses to accept another. Sent in place of
/// `WELCOME`, exactly like [`BUILD_MISMATCH_CODE`] -- see that function's doc comment for
/// why no new wire message type was needed for this.
pub(super) const CONNECTION_LIMIT_REACHED_CODE: u32 = 4;

/// Sends a single `<- ERROR` reply in place of `WELCOME`, telling a peer this worker is
/// already at `--max-connections` capacity, then lets the caller drop the connection --
/// `serve::run`'s accept loop calls this instead of ever handing the connection to
/// [`handle_connection`]/[`handle_connection_with_gpu`] when [`super::ConnectionLimiter::try_acquire`]
/// reports the cap is reached.
///
/// Deliberately reuses [`ErrorMsg`], the same `HELLO`-phase refusal shape
/// [`BUILD_MISMATCH_CODE`] already sends in place of `WELCOME`, rather than adding a new
/// wire message: `indicatrix_net::client::handshake` already reads exactly one reply after
/// writing `HELLO` and tries to decode it as `Welcome` first, falling back to `ErrorMsg` --
/// so this needs no new enum variant and no `PROTOCOL_VERSION` bump (see that constant's
/// doc comment on when a bump IS required -- appending a wire enum variant or struct
/// field, neither of which happens here).
///
/// Never reads the peer's own `HELLO` off the wire first -- unnecessary, since the reply
/// above is decoded the same way regardless of whether the server ever read what the
/// client sent. For a TLS listener, call this only after [`super::tls::accept_tls`] has
/// already completed (handshake AND allowlist check): an unauthenticated peer must not
/// learn this worker's capacity state before authenticating. For the loopback plaintext
/// listener, there is no authentication step to wait for, so this runs immediately.
pub(super) fn refuse_for_capacity<S: Write>(
    stream: &mut S,
    peer: Option<SocketAddr>,
    active: usize,
    max: usize,
) {
    tracing::info!(
        "connection {peer:?}: refusing -- at capacity ({active}/{max} active connections; see --max-connections)"
    );
    let _ = indicatrix_net::messages::write_message(
        stream,
        &ErrorMsg {
            code: CONNECTION_LIMIT_REACHED_CODE,
            message: format!(
                "this worker is already handling {max} concurrent connection(s) (--max-connections); try again \
                 later"
            ),
        },
    );
}

/// Dispatches `ClientMessage::Library` to [`super::library::handle_request`] and writes
/// its reply directly (plain request/response, never a `StreamEvent` stream). A
/// `Cancel` with nothing currently streaming is logged and ignored. Shared by both
/// build modes.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure.
#[allow(
    unreachable_patterns,
    clippy::match_wildcard_for_single_variants,
    reason = "the final arm must stay a wildcard: it matches ClientMessage::RenderRequest and \
              ClientMessage::TiltCurvesRequest only when indicatrix-net's `render` feature is on. \
              That is a DIFFERENT crate's flag from this one's `worker` -- this crate has no \
              feature of its own named `render` at all, so `#[cfg(feature = \"render\")]` inside \
              THIS crate is always false regardless of whether indicatrix-net's variants actually \
              exist (cargo unifies features workspace-wide, so `apps/indicatrix-cut` turning \
              indicatrix-net's `render` on can leave this crate's own `worker` feature off while the \
              variants are still genuinely in scope -- confirmed by `cargo check -p indicatrix-worker \
              --features worker`, which fails to compile a two-cfg-arm version of this match with \
              exactly the \"unexpected cfg condition value: render\" + \"non-exhaustive patterns\" \
              pair this comment replaces). Spelling the variants out, as clippy suggests, fails to \
              compile in a true library-only build (no other workspace member pulling `render` in \
              at all) where neither variant exists; the wildcard is correct in every build. Kept \
              as `allow` (not `expect`) because firing is conditional on that feature -- \
              unconditionally expecting it would warn on every build where the variants ARE \
              nameable."
)]
fn handle_non_render_message<S: Write>(
    stream: &mut S,
    msg: &ClientMessage,
    db: &Database,
) -> Result<(), NetError> {
    match msg {
        ClientMessage::Cancel(c) => {
            tracing::debug!(
                "received CANCEL for request_id={} with no request currently streaming on this connection -- ignoring",
                c.request_id
            );
            Ok(())
        }
        ClientMessage::Library(req) => {
            let response = super::library::handle_request(req, db);
            indicatrix_net::messages::write_message(stream, &response)
        }
        // Wildcard is the only correct choice here (see the `#[allow]` reason above).
        // Always replies `StreamEvent::Error`, even for a `TiltCurvesRequest` -- a
        // mismatch for that family's reader, but acceptable since this arm is only
        // reachable on a build without `worker`, where the peer already has enough
        // signal (a decode failure, or an absent advertised capability) to know
        // something is wrong.
        _ => {
            tracing::warn!(
                "received a RenderRequest or TiltCurvesRequest, but this worker advertises no \
                 render/tilt-curve capacity (built without its `worker` feature) -- replying \
                 with a protocol error"
            );
            indicatrix_net::messages::write_message(
                stream,
                &indicatrix_net::messages::StreamEvent::Error(indicatrix_net::messages::ErrorMsg {
                    code: NO_RENDER_CAPACITY_CODE,
                    message: "this worker serves the design library only and cannot render or \
                              compute tilt curves; its WELCOME advertises both capacities as \
                              absent"
                        .to_string(),
                }),
            )
        }
    }
}

/// Reads and dispatches the peer's next post-handshake message (a blocking read).
/// `Cancel`/`Library` are handled inline and the loop continues; a clean EOF ends the
/// connection.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. `Ok(None)` (not an error) for a
/// clean EOF.
#[cfg(not(feature = "worker"))]
fn serve_until_render_request_or_eof<S: Read + Write>(
    stream: &mut S,
    db: &Database,
) -> Result<Option<std::convert::Infallible>, NetError> {
    loop {
        let msg: ClientMessage = match indicatrix_net::messages::read_message(stream) {
            Ok(m) => m,
            Err(NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        handle_non_render_message(stream, &msg, db)?;
    }
}

/// Restores blocking reads/writes once `HELLO` has arrived, releasing the pre-`HELLO`
/// deadline `serve::run`'s accept loop applied to the raw socket (see
/// `serve::HANDSHAKE_TIMEOUT`) -- the library-only build's counterpart to
/// `stream_emit::TimeoutRead`/`TimeoutWrite`, which [`handle_connection_with_gpu`] uses
/// for the same purpose but which don't exist here: `stream_emit` is gated on the
/// `worker` feature this build doesn't have. Implemented only for the two concrete
/// transports `serve::run` ever hands to [`handle_connection`] -- a plain `TcpStream`,
/// or one wrapped in mutual TLS -- since this build has no test double standing in for
/// either (see `serve::tests`, compiled only under `worker`).
#[cfg(not(feature = "worker"))]
pub trait ClearHandshakeTimeout {
    /// Best-effort, like `serve::tune_accepted_socket`: a failed `set_*_timeout` call is
    /// swallowed rather than turned into a connection-ending error, since the loop ahead
    /// works fine either way -- it would just run unbounded by a timeout, the same as a
    /// connection this deadline was never applied to.
    fn clear_handshake_timeout(&mut self);
}

#[cfg(not(feature = "worker"))]
impl ClearHandshakeTimeout for std::net::TcpStream {
    fn clear_handshake_timeout(&mut self) {
        let _ = self.set_read_timeout(None);
        let _ = self.set_write_timeout(None);
    }
}

#[cfg(not(feature = "worker"))]
impl ClearHandshakeTimeout for rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream> {
    fn clear_handshake_timeout(&mut self) {
        let _ = self.sock.set_read_timeout(None);
        let _ = self.sock.set_write_timeout(None);
    }
}

/// Handles one connection end to end on a library-only build (no `worker` feature).
///
/// `HELLO`/`WELCOME` (`WELCOME::render` always `None`, so no build-compatibility check
/// runs), then a loop dispatching `Cancel`/`Library` messages until the peer closes the
/// connection. Only a `HELLO` [`Welcome::protocol_version`] mismatch is refused -- the
/// one wire-format compatibility question still meaningful without a build to compare.
///
/// # Errors
///
/// See [`handle_connection_with_gpu`]; same failure modes minus anything render-specific.
#[cfg(not(feature = "worker"))]
pub fn handle_connection<S: Read + Write + ClearHandshakeTimeout>(
    mut stream: S,
    db: &Database,
) -> Result<(), NetError> {
    let remote_hello: Hello = indicatrix_net::messages::read_message(&mut stream)?;
    // HELLO has arrived -- the pre-protocol deadline (`serve::HANDSHAKE_TIMEOUT`) has
    // done its job. Restore blocking reads/writes before anything below (WELCOME/ERROR,
    // then `serve_until_render_request_or_eof`'s loop) relies on it, same as a
    // connection that was never bounded by a deadline at all.
    stream.clear_handshake_timeout();

    if remote_hello.protocol_version != PROTOCOL_VERSION {
        let message = format!(
            "refusing to pair: this worker speaks protocol_version={PROTOCOL_VERSION}, peer speaks \
             protocol_version={}",
            remote_hello.protocol_version
        );
        tracing::warn!("{message}");
        let _ = indicatrix_net::messages::write_message(
            &mut stream,
            &ErrorMsg {
                code: BUILD_MISMATCH_CODE,
                message,
            },
        );
        return Ok(());
    }

    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        build_hash: indicatrix_net::handshake::UNKNOWN_BUILD_HASH,
        source_hash: indicatrix_net::handshake::UNKNOWN_BUILD_HASH,
        render: None,
        library: true,
        tilt_curves: false,
    };
    indicatrix_net::messages::write_message(&mut stream, &welcome)?;

    serve_until_render_request_or_eof(&mut stream, db)?.map_or(Ok(()), |never| match never {})
}

#[cfg(feature = "worker")]
mod worker {
    use super::{
        BUILD_MISMATCH_CODE, Database, NO_RENDER_CAPACITY_CODE, handle_non_render_message,
    };
    use crate::{
        cli::ComputeMode,
        render_core,
        stream_emit::{self, StreamOutcome, TimeoutRead, TimeoutWrite},
        validate,
    };
    use indicatrix::renderer::gpu_backend::GpuBackend;
    use indicatrix_net::{
        framing::FramingError,
        handshake,
        messages::{
            Backend, ClientMessage, ErrorMsg, Hello, NetError, PROTOCOL_VERSION, RenderCapability,
            RenderRequest, StreamEvent, TiltCurvesRequest, TiltCurvesResponse, Welcome,
        },
    };
    use std::{
        io::{Read, Write},
        sync::Arc,
    };

    /// Handles one connection end to end, always on the CPU.
    ///
    /// A thin wrapper around [`handle_connection_with_gpu`] passing
    /// [`GpuBackend::disabled`], so this crate's own tests stay deterministic (tracing
    /// on the CPU reference path) regardless of `--features gpu`. `crate::serve::run`'s
    /// accept loop calls [`handle_connection_with_gpu`] directly with the real, shared
    /// [`GpuBackend`] acquired at startup.
    ///
    /// # Errors
    ///
    /// See [`handle_connection_with_gpu`].
    pub fn handle_connection<S: Read + Write + TimeoutRead + TimeoutWrite>(
        stream: S,
        threads: usize,
        db: &Database,
    ) -> Result<(), NetError> {
        // `ComputeMode` is moot here: the disabled backend below declines every dispatch
        // regardless, so calibration is never attempted.
        handle_connection_with_gpu(
            stream,
            threads,
            &Arc::new(GpuBackend::disabled()),
            db,
            ComputeMode::OnlyCpu,
        )
    }

    /// Handles one connection end to end.
    ///
    /// First the `HELLO`/`WELCOME` handshake, refusing on a build-hash/source-hash or
    /// protocol-version mismatch (per [`handshake::verify_compatible`]) -- the one code
    /// path that runs the full build-compatibility gate; the library-only
    /// [`super::handle_connection`] never does, having no build to compare. A peer whose
    /// `HELLO` reports [`handshake::UNKNOWN_BUILD_HASH`] (e.g. a library-only client with
    /// no `indicatrix` build to report at all -- see `indicatrix_net::library`'s module
    /// doc comment) is a special case (see
    /// [`pair_as_library_only_if_unknown_build`]): rather than being refused by
    /// [`handshake::verify_compatible`]'s "an unknown build is never compatible" rule,
    /// it is paired as library-only, exactly like [`super::handle_connection`] pairs
    /// one -- see [`serve_library_only_connection`]. Then a loop dispatching
    /// `Cancel`/`Library`/`RenderRequest`/`TiltCurvesRequest` until the peer closes. Each
    /// `RenderRequest` usually comes from a fresh [`read_next_message`] call, but may
    /// already be in hand if the client pipelined it ahead of the previous request's
    /// `DONE` (handed back by [`stream_emit::run_stream`]). `TiltCurvesRequest` supports
    /// no pipelining and goes straight to
    /// [`crate::serve::tilt::handle_tilt_curves_request`] as a single request/response
    /// call, unlike `RenderRequest`'s validate-then-stream handling below.
    ///
    /// Generic over `Read + Write` rather than `TcpStream` (see `crate::serve`'s module
    /// docs).
    ///
    /// `gpu` decides both what `WELCOME::render.backend` reports and what actually
    /// traces requests: [`Backend::Gpu`] iff `gpu.adapter_label()` is `Some` (an adapter
    /// was genuinely acquired at startup), else [`Backend::Cpu`] -- decided once at
    /// handshake time so a worker never claims GPU while silently tracing on CPU. A
    /// later per-request decline (an unsupported material) still falls back silently,
    /// as documented in `gpu_backend`.
    ///
    /// `compute_mode` (`--only-gpu`/`--only-cpu`/hybrid) threads through to every
    /// request's [`stream_emit::run_stream`] call, reaching `tracer::run_tracer` where
    /// it decides whether a batch job attempts a hybrid CPU+GPU split. It plays no role
    /// in the `backend`/`WELCOME` decision above, which is about which engine(s) could
    /// serve this connection at all.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] for a transport-level failure. A validation failure or
    /// caught tracing panic is NOT an error return -- both are reported to the peer as
    /// an `ErrorMsg`/`StreamEvent::Error` and the loop continues.
    pub fn handle_connection_with_gpu<S: Read + Write + TimeoutRead + TimeoutWrite>(
        mut stream: S,
        threads: usize,
        gpu: &Arc<GpuBackend>,
        db: &Database,
        compute_mode: ComputeMode,
    ) -> Result<(), NetError> {
        let remote_hello: Hello = indicatrix_net::messages::read_message(&mut stream)?;
        // HELLO has arrived -- the pre-protocol deadline `serve::run`'s accept loop
        // applied to the raw socket (`serve::HANDSHAKE_TIMEOUT`, covering a TLS listener's
        // handshake and, either way, this HELLO read) has done its job. Restore blocking
        // reads/writes: everything below (WELCOME/ERROR, then this loop's own
        // `read_next_message`) expects it, as does `stream_emit::run_stream`'s
        // `TimeoutCache`, fresh per request and assuming nothing has touched the read
        // timeout since. Best-effort, like `serve::tune_accepted_socket` -- a failed call
        // just leaves the deadline in place rather than ending the connection over it.
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(None);
        let local_hello = handshake::local_hello();

        if let Some(result) =
            pair_as_library_only_if_unknown_build(&mut stream, &local_hello, &remote_hello, db)
        {
            return result;
        }

        if let Err(incompatible) = handshake::verify_compatible(&local_hello, &remote_hello) {
            refuse_incompatible_handshake(&mut stream, &local_hello, &remote_hello, incompatible);
            return Ok(());
        }

        let backend = gpu.adapter_label().map_or_else(
            || Backend::Cpu {
                threads: render_core::effective_thread_count(threads) as u32,
            },
            |adapter| Backend::Gpu { adapter },
        );
        let welcome = Welcome {
            protocol_version: PROTOCOL_VERSION,
            build_hash: local_hello.build_hash,
            source_hash: local_hello.source_hash,
            render: Some(RenderCapability {
                backend,
                max_pixels: validate::MAX_PIXELS,
                min_cadence_ms: stream_emit::MIN_CADENCE_FLOOR_MS,
            }),
            library: true,
            // Shares the same `worker`-feature gate as `RenderRequest`; stays an
            // explicit field rather than being inferred from `render.is_some()` (see
            // `Welcome::tilt_curves`'s doc comment).
            tilt_curves: true,
        };
        indicatrix_net::messages::write_message(&mut stream, &welcome)?;

        // A `RenderRequest` `run_stream` already pulled off the wire while streaming
        // the previous request (client-pipelined ahead of its `DONE`), to be processed
        // next iteration as if freshly read. `None` means read one instead. Never a
        // `TiltCurvesRequest`, which supports no pipelining.
        let mut pending_request: Option<RenderRequest> = None;

        loop {
            let next = match pending_request.take() {
                Some(r) => NextRequest::Render(r),
                None => match read_next_message(&mut stream, db)? {
                    Some(n) => n,
                    // The peer closing the connection is the normal end of this loop,
                    // not a failure.
                    None => return Ok(()),
                },
            };

            let mut request: RenderRequest = match next {
                NextRequest::Render(r) => r,
                NextRequest::TiltCurves(tilt_request) => {
                    // A single request/response call (its own validation, catch_unwind,
                    // reply envelope) -- unlike the validate-then-stream dance below.
                    crate::serve::tilt::handle_tilt_curves_request(&mut stream, &tilt_request)?;
                    continue;
                }
            };

            // `validate_stream_config` takes `&mut request.stream` (it
            // clamps `cadence_ms` in place) alongside `&request.scene` -- two disjoint
            // field borrows of the same `request`, not a whole-struct borrow, so this
            // compiles despite the apparent mutable/shared overlap.
            if let Err(msg) =
                validate::validate_request(&request.scene, request.first_sample, request.samples)
                    .and_then(|()| {
                        validate::validate_stream_config(&mut request.stream, &request.scene)
                    })
            {
                indicatrix_net::messages::write_stream_event(
                    &mut stream,
                    &StreamEvent::Error(ErrorMsg {
                        code: VALIDATION_FAILED_CODE,
                        message: msg,
                    }),
                    None,
                )?;
                continue;
            }

            // Runs the tracer on its own thread (never touching `stream`) and this
            // thread as the emitter -- see `stream_emit`'s module docs. Defense in
            // depth: validation should already reject anything pathological, but
            // `run_stream`'s tracer thread runs inside `catch_unwind` regardless,
            // surfaced here as [`StreamOutcome::TracePanicked`].
            let (outcome, next) =
                stream_emit::run_stream(&mut stream, &request, threads, gpu, compute_mode)?;
            // The client's already-pipelined `RenderRequest`, if any, pulled off the
            // wire while `request` was streaming; queued so the next iteration
            // processes it without blocking on another read.
            pending_request = next;
            match outcome {
                StreamOutcome::Completed => {}
                StreamOutcome::TracePanicked => {
                    tracing::warn!(
                        "tracing panicked for a request that passed validation (request_id={}, first_sample={}, samples={})",
                        request.request_id,
                        request.first_sample,
                        request.samples
                    );
                    indicatrix_net::messages::write_stream_event(
                        &mut stream,
                        &StreamEvent::Error(ErrorMsg {
                            code: TRACE_PANIC_CODE,
                            message: "internal error while tracing this request".to_string(),
                        }),
                        None,
                    )?;
                }
            }
        }
    }

    /// Logs why [`handshake::verify_compatible`] refused `local_hello`/`remote_hello`
    /// and writes an `ErrorMsg` in place of `WELCOME` -- best-effort, like
    /// [`super::refuse_for_capacity`]: the peer may already have hung up, but the
    /// warning logged here is the meaningful outcome either way.
    fn refuse_incompatible_handshake<S: Write>(
        stream: &mut S,
        local_hello: &Hello,
        remote_hello: &Hello,
        incompatible: handshake::Incompatible,
    ) {
        let message = format!(
            "refusing to pair: worker build_hash={:02x?} protocol_version={}, viewer build_hash={:02x?} \
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
            },
        );
    }

    /// Pairs `stream` as library-only when `remote_hello` understands this
    /// worker's protocol version but declares no `indicatrix` build at all
    /// (`build_hash == handshake::UNKNOWN_BUILD_HASH`, e.g. a library-only client -- see
    /// `indicatrix_net::library`'s module doc comment).
    ///
    /// `Some` means this connection is now fully handled and the caller
    /// ([`handle_connection_with_gpu`]) should return the inner `Result` immediately;
    /// `None` means this peer is NOT a library-only downgrade case, and the caller
    /// should continue on to the normal [`handshake::verify_compatible`] gate. Checked
    /// before that gate so its "an unknown build is never compatible with anything"
    /// rule -- correct for two peers that both want to render -- never fires here:
    /// instead of refusing, this sends a `WELCOME` with no render capacity and serves
    /// only `Cancel`/`Library` from here on ([`serve_library_only_connection`]).
    fn pair_as_library_only_if_unknown_build<S: Read + Write>(
        stream: &mut S,
        local_hello: &Hello,
        remote_hello: &Hello,
        db: &Database,
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
        };
        Some(
            indicatrix_net::messages::write_message(stream, &welcome)
                .and_then(|()| serve_library_only_connection(stream, db)),
        )
    }

    /// Serves a peer whose `HELLO` reported [`handshake::UNKNOWN_BUILD_HASH`] (a
    /// library-only client -- see [`handle_connection_with_gpu`]'s doc comment)
    /// after this worker has already sent it a `WELCOME` advertising no render
    /// capacity for this connection. Dispatches `Cancel`/`Library` exactly like
    /// [`handle_connection_with_gpu`]'s own loop (via [`handle_non_render_message`]); a
    /// `RenderRequest` or `TiltCurvesRequest` is refused with [`NO_RENDER_CAPACITY_CODE`]
    /// instead of ever reaching the tracer, since this worker already told the peer it
    /// has no render capacity here.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] for a transport-level failure. `Ok(())` (not an error) for a
    /// clean EOF.
    fn serve_library_only_connection<S: Read + Write>(
        stream: &mut S,
        db: &Database,
    ) -> Result<(), NetError> {
        loop {
            let msg: ClientMessage = match indicatrix_net::messages::read_message(stream) {
                Ok(m) => m,
                Err(NetError::Framing(FramingError::Io(e)))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            match msg {
                ClientMessage::RenderRequest(_) => {
                    indicatrix_net::messages::write_stream_event(
                        stream,
                        &StreamEvent::Error(ErrorMsg {
                            code: NO_RENDER_CAPACITY_CODE,
                            message: "this connection was paired as library-only (HELLO reported \
                                      no indicatrix build) and cannot render"
                                .to_string(),
                        }),
                        None,
                    )?;
                }
                ClientMessage::TiltCurvesRequest(_) => {
                    indicatrix_net::messages::write_message(
                        stream,
                        &TiltCurvesResponse::Error(ErrorMsg {
                            code: NO_RENDER_CAPACITY_CODE,
                            message: "this connection was paired as library-only (HELLO reported \
                                      no indicatrix build) and cannot compute tilt curves"
                                .to_string(),
                        }),
                    )?;
                }
                other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                    handle_non_render_message(stream, &other, db)?;
                }
            }
        }
    }

    /// What [`read_next_message`] found once it stopped reading. `Cancel`/`Library` are
    /// fully handled inline before this ever returns, so this enum only carries the two
    /// request kinds that end the read loop: `RenderRequest` (to
    /// [`stream_emit::run_stream`]) or `TiltCurvesRequest` (to
    /// [`crate::serve::tilt::handle_tilt_curves_request`]).
    enum NextRequest {
        Render(RenderRequest),
        TiltCurves(TiltCurvesRequest),
    }

    /// Reads the next message directly off `stream` (blocking). Dispatches
    /// `Cancel`/`Library` inline and keeps reading; only a `RenderRequest` or
    /// `TiltCurvesRequest` ends the wait (see [`NextRequest`]).
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] for a transport-level failure. `Ok(None)` (not an error) for
    /// a clean EOF.
    fn read_next_message<S: Read + Write>(
        stream: &mut S,
        db: &Database,
    ) -> Result<Option<NextRequest>, NetError> {
        loop {
            let msg: ClientMessage = match indicatrix_net::messages::read_message(stream) {
                Ok(m) => m,
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
                other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                    handle_non_render_message(stream, &other, db)?;
                }
            }
        }
    }

    /// `<- ERROR`/`TiltCurvesResponse::Error` code for a request whose embedded scene
    /// failed [`validate::validate_scene`] -- shared verbatim between `RenderRequest`
    /// and `TiltCurvesRequest` validation failures even though the reply envelope
    /// differs.
    pub(in crate::serve) const VALIDATION_FAILED_CODE: u32 = 2;
    /// `<- ERROR`/`TiltCurvesResponse::Error` code for a `indicatrix` panic caught by
    /// `catch_unwind` on a validation-passing but pathological scene; shared with
    /// [`VALIDATION_FAILED_CODE`]'s reasoning.
    pub(in crate::serve) const TRACE_PANIC_CODE: u32 = 3;
}

#[cfg(feature = "worker")]
pub(super) use worker::{TRACE_PANIC_CODE, VALIDATION_FAILED_CODE};
#[cfg(feature = "worker")]
pub use worker::{handle_connection, handle_connection_with_gpu};

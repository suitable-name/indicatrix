//! The `serve` subcommand: accept `indicatrix_net` connections over TCP and reply.
//!
//! Performs mutual TLS (unless `--insecure-no-tls`) and a `HELLO`/`WELCOME` handshake,
//! then loops handling whatever the peer sends. Every build serves the read-only
//! design-library protocol ([`library`], backed by [`indicatrix_vault`]); a build with
//! the `worker` feature additionally serves `RenderRequest`s and `TiltCurvesRequest`s.
//!
//! [`connection::handle_connection`] dispatches tagged `ClientMessage`s in one loop.
//! `Library` requests always go to the shared [`library::handle_request`] (plain
//! request/response, unlike streamed `RenderRequest`s); `TiltCurvesRequest` goes to
//! [`tilt::handle_tilt_curves_request`] and needs no `Database`, just caller-supplied
//! geometry.
//!
//! **The library protocol sits behind the exact same mutual-TLS authentication as the
//! render protocol -- there is no separate check.** Both are message variants read off
//! the same already-authenticated stream and the same `Auth::Allowlist`/
//! `Auth::AnyCaSignedClient` decision in [`accept_tls`].
//!
//! `WELCOME` carries `render: Option<RenderCapability>` (`Some` only on a `worker` build
//! with compute capacity), `library: bool` (always `true`), and `tilt_curves: bool`
//! (equal to `render.is_some()`). A client must check these before sending the
//! corresponding request -- on a library-only build the render/tilt message variants
//! don't exist on the wire at all, so sending one anyway fails to decode.
//!
//! # TLS
//!
//! Connection handlers are generic over `Read + Write` and never name `TcpStream`
//! directly; only [`accept_tls`] wraps an accepted socket in
//! `rustls::StreamOwned<ServerConnection, TcpStream>`. Mutual TLS answers "may this peer
//! talk to me at all", a different question from
//! `indicatrix_net::handshake::verify_compatible`'s physics-compatibility check. TLS
//! validates the certificate chain only; the [`Auth::Allowlist`] check in [`accept_tls`]
//! against `peer_certificates()` is the actual authorization decision (no CRL/OCSP,
//! just a fingerprint list). `--trust-any-client-cert` ([`Auth::AnyCaSignedClient`])
//! skips it and must be an explicit flag. `--insecure-no-tls` skips TLS entirely, is
//! refused on a non-loopback `--bind`, and every such connection logs a warning.
//!
//! # Robustness
//!
//! - Every `RenderRequest`/`LibraryRequest` is validated defensively; failures get an
//!   `Error`/`NotFound` reply, not a dropped connection.
//! - Tracing runs inside `catch_unwind` (worker builds); [`run`]'s accept loop wraps the
//!   whole per-connection handler in a second `catch_unwind` as backstop.
//! - The listener binds to loopback only unless `--allow-remote` is passed, independent
//!   of TLS.
//! - An authenticated client can still ask for an absurd render or search:
//!   `crate::validate`'s caps bound the former, `Database::search_diagrams`'s 1000-row
//!   cap bounds the latter unconditionally.
//! - Every accepted connection gets `TCP_NODELAY` and TCP keepalive best-effort (see
//!   [`tune_accepted_socket`]), for faster reclaim of a thread blocked reading from a
//!   peer that vanished without a clean FIN/RST.
//! - A just-accepted socket gets [`HANDSHAKE_TIMEOUT`] applied before its TLS handshake
//!   (if any) and its first `HELLO`, so a client that connects and never speaks (or
//!   completes TLS and then never sends `HELLO`) can't hold a connection thread open
//!   indefinitely (slowloris) -- see [`connection::handle_connection_with_gpu`] (`worker`
//!   builds) and [`connection::handle_connection`] (library-only builds) for where the
//!   deadline is cleared once `HELLO` arrives.
//! - `--max-connections` (default 64, see [`ConnectionLimiter`]) bounds how many
//!   connections this worker handles at once, one cap shared by the one listener
//!   regardless of build mode (library-only or `worker`) or transport (TLS or
//!   `--insecure-no-tls`) -- see [`ConnectionLimiter`]'s doc comment. A connection over
//!   the cap is still accepted and given a definitive `<- ERROR` refusal (see
//!   [`connection::refuse_for_capacity`]) rather than left to hang or being reset
//!   without explanation.

use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use indicatrix_vault::db::sqlite::Database;

use crate::{cli::ServeArgs, enroll};
// A library-only build has no render capacity, so nothing below routes a `ComputeMode`.
#[cfg(feature = "worker")]
use crate::cli::ComputeMode;

mod connection;
pub mod library;
#[cfg(feature = "worker")]
mod tilt;
mod tls;
// These tests drive a real render round trip, uncompilable without `render` support.
// `library`'s own tests cover the library protocol unconditionally.
#[cfg(all(test, feature = "worker"))]
mod tests;

use connection::report_connection_result;
use tls::{Auth, Transport, accept_tls, build_transport};

#[cfg(not(feature = "worker"))]
pub use connection::handle_connection;
#[cfg(feature = "worker")]
pub use connection::{handle_connection, handle_connection_with_gpu};

/// Opens the design-library database this `serve` instance serves: `args.db` if given,
/// else the default `facet_diagrams.sqlite` relative to the process's working directory.
///
/// Always opened READ-ONLY -- this phase never writes to the catalogue, so `serve`
/// fails fast with a clear error if the database doesn't already exist rather than
/// silently creating an empty one.
///
/// Called once per connection (see [`resolve_library_db_path`]), not opened once and
/// shared: `rusqlite::Connection` is `Send` but not `Sync`, and SQLite's concurrency
/// model is many independent reader connections.
///
/// # Errors
///
/// A human-readable message if the database can't be opened read-only (missing file,
/// permissions, not a valid SQLite database).
fn open_library_database(path: &std::path::Path) -> Result<Database, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("--db {}: path is not valid Unicode", path.display()))?;
    Database::open_read_only(path_str).map_err(|e| {
        format!(
            "failed to open the design-library database read-only at {}: {e:#}",
            path.display()
        )
    })
}

/// Resolves `--db` (or the default `facet_diagrams.sqlite`) to the path every
/// connection thread opens its own [`open_library_database`] handle from.
///
/// [`run`] hands out a path, not an already-open [`Database`]: `rusqlite::Connection`
/// isn't `Sync`, so it can't be shared via `Arc` the way `GpuBackend` is below. [`run`]
/// still opens (and immediately drops) one at startup, purely to fail fast on a bad
/// `--db` before binding a socket.
#[must_use]
fn resolve_library_db_path(args: &ServeArgs) -> std::path::PathBuf {
    args.db
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(indicatrix_vault::db::sqlite::DEFAULT_DB_FILE))
}

/// How long an accepted connection may sit idle before the OS probes it with a TCP
/// keepalive packet. Catches a peer that vanished without a clean FIN/RST; not meant to
/// second-guess a busy connection.
const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);

/// How often a keepalive probe repeats once idle. Short relative to
/// `TCP_KEEPALIVE_IDLE` so a dead peer is detected within a few seconds, rather than
/// waiting on the OS's own default probe count/interval.
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// Best-effort tuning for one just-accepted `stream`: disables Nagle's algorithm
/// (`TCP_NODELAY`, since this protocol writes many small latency-sensitive messages)
/// and enables TCP keepalive so a peer that vanishes without a clean FIN/RST is
/// eventually detected instead of leaking the connection's thread forever. Never
/// aborts the accept loop on failure; logged at `debug` since failure is expected to be
/// harmless on deployed platforms.
fn tune_accepted_socket(stream: &TcpStream, peer: Option<SocketAddr>) {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!("connection {peer:?}: failed to set TCP_NODELAY: {e}");
    }

    let sock_ref = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(TCP_KEEPALIVE_IDLE)
        .with_interval(TCP_KEEPALIVE_INTERVAL);
    if let Err(e) = sock_ref.set_tcp_keepalive(&keepalive) {
        tracing::debug!("connection {peer:?}: failed to set TCP keepalive: {e}");
    }
}

/// How long a just-accepted connection may take to get through its pre-protocol
/// handshake -- the TLS handshake (see [`tls::accept_tls`]) plus the first `HELLO`
/// frame, or (under `--insecure-no-tls`) just the first `HELLO` frame -- before this
/// worker gives up on it. Mitigates a slowloris-style client that connects (and, for
/// TLS, maybe completes the handshake) and then never speaks: without this, such a
/// client holds a connection thread open for as long as the OS's own TCP defaults
/// allow.
///
/// Applied once, in [`run`]'s accept loop, to the raw accepted `TcpStream` -- before
/// either transport branch, so it covers a TLS listener's handshake (run inside
/// [`tls::accept_tls`]) and, either way, the `HELLO` read that follows. Cleared the
/// moment `HELLO` arrives (see [`connection::handle_connection_with_gpu`]'s and the
/// library-only [`connection::handle_connection`]'s doc comments), so it never bounds
/// anything the connection loop itself does afterward -- that loop has its own,
/// different timeout story (see `stream_emit::TimeoutCache`).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Applies [`HANDSHAKE_TIMEOUT`] to `stream`'s read and write timeouts. Best-effort,
/// like [`tune_accepted_socket`]: a failure is logged at `debug` and never aborts the
/// accept loop -- worst case, this one connection goes back to being unbounded, exactly
/// like every connection was before this deadline existed.
fn apply_handshake_timeout(stream: &TcpStream, peer: Option<SocketAddr>) {
    if let Err(e) = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake read timeout: {e}");
    }
    if let Err(e) = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake write timeout: {e}");
    }
}

/// Caps how many connections this worker handles at once (`--max-connections`, default
/// 64). One instance, owned by [`run`], shared by every accepted connection regardless
/// of transport (TLS or `--insecure-no-tls`) or build mode (library-only or `worker`) --
/// there is exactly one listener in this codebase (see this module's own doc comment),
/// so one shared cap is the whole design here: there is no separate library-only
/// listener to give its own cap to. The token-based enrollment listener
/// (`crate::enroll`) is a genuinely separate listener/concern (bootstrapping trust, not
/// serving render/library requests) and is deliberately not covered by this cap.
#[derive(Debug)]
struct ConnectionLimiter {
    active: Arc<AtomicUsize>,
    max: usize,
}

impl ConnectionLimiter {
    fn new(max: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    /// Attempts to reserve one connection slot for a just-accepted socket.
    ///
    /// `Ok` hands back a [`ConnectionSlot`] that releases the slot when dropped --
    /// including when the connection thread's `catch_unwind`-wrapped handler panics:
    /// `Drop::drop` still runs while a panic unwinds (just not through an `abort`), and
    /// nothing in the connection-handling path aborts. `Err` carries the active count
    /// observed at the moment of refusal (for [`connection::refuse_for_capacity`]'s log
    /// line); the slot this call provisionally reserved to make that observation is
    /// released again immediately, so a refused connection never itself counts against
    /// the cap.
    fn try_acquire(&self) -> Result<ConnectionSlot, usize> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        if active > self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            Err(active - 1)
        } else {
            Ok(ConnectionSlot {
                active: Arc::clone(&self.active),
            })
        }
    }
}

/// RAII handle for one reserved [`ConnectionLimiter`] slot. Held by a connection thread
/// for the lifetime of [`connection::handle_connection`]/[`connection::handle_connection_with_gpu`]
/// (moved into the `thread::spawn` closure alongside everything else that connection
/// needs); [`Drop::drop`] releases the slot regardless of how that call returns --
/// normally, via `?`, or via a caught panic.
#[derive(Debug)]
struct ConnectionSlot {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs the `serve` subcommand: binds `args.bind` and accepts connections forever.
///
/// Refuses a non-loopback bind address unless `args.allow_remote` is set, independent
/// of TLS. Builds a mutual-TLS server config from `args.ca`/`args.cert`/`args.key`
/// (required unless `args.insecure_no_tls`), pre-loading the fingerprint allowlist once
/// to fail fast on a bad path (the real per-connection check in [`accept_tls`] re-reads
/// it every time). Each accepted connection is handled on its own thread.
///
/// Resolves and preflights the design-library database path once here; every
/// connection thread then opens its own read-only handle from that path (see
/// [`resolve_library_db_path`]). `worker` builds also acquire this process's
/// `GpuBackend` exactly once here (shared via `Arc`, since unlike `Database` it is
/// `Sync`) -- `OnlyCpu` forces `GpuBackend::disabled`; `Hybrid`/`OnlyGpu` acquire a
/// real one.
///
/// Also builds this call's one [`ConnectionLimiter`] from `args.max_connections`
/// (default 64) and applies [`HANDSHAKE_TIMEOUT`] to every accepted socket before
/// dispatching it -- see both items' doc comments, and this module's own "Robustness"
/// section.
///
/// # Errors
///
/// Returns a human-readable message if `args.bind` doesn't parse as a socket address,
/// if it's non-loopback without `--allow-remote`, if `--insecure-no-tls` is combined
/// with a non-loopback bind, if TLS is enabled but `--ca`/`--cert`/`--key` are missing
/// or unloadable, if the allowlist can't be loaded (and `--trust-any-client-cert` isn't
/// set), if the design-library database can't be opened, or if the bind itself fails
/// (e.g. the port is already in use).
pub fn run(args: &ServeArgs) -> Result<(), String> {
    let bind_addr: SocketAddr = args
        .bind
        .parse()
        .map_err(|e| format!("invalid --bind address {:?}: {e}", args.bind))?;

    if !bind_addr.ip().is_loopback() && !args.allow_remote {
        return Err(format!(
            "refusing to bind non-loopback address {bind_addr} without --allow-remote -- exposing this worker \
             beyond localhost must be explicit (see --help)"
        ));
    }

    let transport = build_transport(args, bind_addr)?;
    let db_path = resolve_library_db_path(args);
    // Fail fast on a bad `--db` before binding a socket; dropped immediately, each
    // connection opens its own.
    drop(open_library_database(&db_path)?);
    let limiter = ConnectionLimiter::new(args.max_connections);

    #[cfg(feature = "worker")]
    let gpu = Arc::new(match args.compute_mode {
        ComputeMode::OnlyCpu => indicatrix::renderer::gpu_backend::GpuBackend::disabled(),
        ComputeMode::Hybrid | ComputeMode::OnlyGpu => {
            indicatrix::renderer::gpu_backend::GpuBackend::acquire()
        }
    });

    let listener =
        TcpListener::bind(bind_addr).map_err(|e| format!("failed to bind {bind_addr}: {e}"))?;
    tracing::info!(
        "indicatrix-worker serve: listening on {bind_addr} ({}, {})",
        match &transport {
            Transport::Tls {
                auth: Auth::Allowlist(_),
                ..
            } => "mutual TLS, allowlist enforced",
            Transport::Tls {
                auth: Auth::AnyCaSignedClient,
                ..
            } => "mutual TLS, ANY CA-signed client trusted",
            Transport::Insecure => "PLAINTEXT, no TLS",
        },
        capability_summary(args),
    );

    // Token-based enrollment runs its own separate listener, never a mode of the render
    // listener above, so it can't accidentally relax mutual TLS for everyone.
    enroll::maybe_start_from_serve_args(args, bind_addr)?;

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                tune_accepted_socket(&stream, peer);
                apply_handshake_timeout(&stream, peer);
                let slot = limiter.try_acquire();
                #[cfg(feature = "worker")]
                spawn_connection_handler(
                    stream,
                    peer,
                    &transport,
                    slot,
                    &ConnectionContext {
                        db_path: &db_path,
                        threads: args.threads,
                        gpu: &gpu,
                        compute_mode: args.compute_mode,
                        max_connections: args.max_connections,
                    },
                );
                #[cfg(not(feature = "worker"))]
                spawn_connection_handler(
                    stream,
                    peer,
                    &transport,
                    &db_path,
                    slot,
                    args.max_connections,
                );
            }
            Err(e) => tracing::warn!("accept error: {e}"),
        }
    }

    Ok(())
}

/// Connection-independent context [`spawn_connection_handler`] needs for every accepted
/// connection, bundled into one struct so that function takes a handful of arguments
/// instead of growing another positional parameter each time a new one is needed.
#[cfg(feature = "worker")]
struct ConnectionContext<'a> {
    db_path: &'a std::path::Path,
    threads: usize,
    gpu: &'a Arc<indicatrix::renderer::gpu_backend::GpuBackend>,
    compute_mode: ComputeMode,
    max_connections: usize,
}

/// Dispatches one just-accepted `stream` on `transport` (mutual-TLS handshake vs
/// plaintext) and spawns its own thread running [`connection::handle_connection_with_gpu`]
/// (`worker` builds) or [`connection::handle_connection`] otherwise.
///
/// `slot` is [`ConnectionLimiter::try_acquire`]'s verdict for this connection, decided
/// back in [`run`]'s accept loop (before the transport-specific work below, so refusing
/// a connection never pays for a database open or GPU handle clone it won't use). `Ok`
/// moves the [`ConnectionSlot`] into the spawned thread, held for the connection's whole
/// lifetime; `Err(active)` skips straight to [`connection::refuse_for_capacity`] instead
/// of ever calling [`connection::handle_connection_with_gpu`] -- for a TLS listener,
/// only after [`accept_tls`] itself has already succeeded (handshake AND allowlist),
/// per that function's doc comment on why an unauthenticated peer must not learn
/// capacity state first; for `--insecure-no-tls`, immediately, since there is no
/// authentication step to wait for.
#[cfg(feature = "worker")]
fn spawn_connection_handler(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    transport: &Transport,
    slot: Result<ConnectionSlot, usize>,
    ctx: &ConnectionContext<'_>,
) {
    let db_path = ctx.db_path.to_path_buf();
    let gpu = Arc::clone(ctx.gpu);
    let threads = ctx.threads;
    let compute_mode = ctx.compute_mode;
    let max_connections = ctx.max_connections;
    match transport {
        Transport::Insecure => {
            tracing::warn!(
                "--insecure-no-tls: accepted PLAINTEXT connection from {peer:?} -- no TLS, no \
                 authentication"
            );
            thread::spawn(move || {
                let mut stream = stream;
                match slot {
                    Ok(_slot) => {
                        let Some(db) = open_connection_database(&db_path, peer) else {
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_connection_with_gpu(
                                stream,
                                threads,
                                &gpu,
                                &db,
                                compute_mode,
                            )
                        }));
                        report_connection_result(peer, result);
                        // `_slot` releases this connection's counted slot right here.
                    }
                    Err(active) => {
                        connection::refuse_for_capacity(&mut stream, peer, active, max_connections);
                    }
                }
            });
        }
        Transport::Tls { config, auth } => {
            let config = Arc::clone(config);
            let auth = auth.clone();
            thread::spawn(move || {
                let Some(mut tls_stream) = accept_tls(stream, &config, &auth, peer) else {
                    return; // accept_tls already logged why
                };
                match slot {
                    Ok(_slot) => {
                        let Some(db) = open_connection_database(&db_path, peer) else {
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_connection_with_gpu(
                                tls_stream,
                                threads,
                                &gpu,
                                &db,
                                compute_mode,
                            )
                        }));
                        report_connection_result(peer, result);
                    }
                    Err(active) => {
                        connection::refuse_for_capacity(
                            &mut tls_stream,
                            peer,
                            active,
                            max_connections,
                        );
                    }
                }
            });
        }
    }
}

/// Same dispatch as the `worker` overload, minus the GPU/thread-count plumbing a
/// library-only build has nothing to pass. See that overload's doc comment for what
/// `slot`/`max_connections` do.
#[cfg(not(feature = "worker"))]
fn spawn_connection_handler(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    transport: &Transport,
    db_path: &std::path::Path,
    slot: Result<ConnectionSlot, usize>,
    max_connections: usize,
) {
    let db_path = db_path.to_path_buf();
    match transport {
        Transport::Insecure => {
            tracing::warn!(
                "--insecure-no-tls: accepted PLAINTEXT connection from {peer:?} -- no TLS, no \
                 authentication"
            );
            thread::spawn(move || {
                let mut stream = stream;
                match slot {
                    Ok(_slot) => {
                        let Some(db) = open_connection_database(&db_path, peer) else {
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_connection(stream, &db)
                        }));
                        report_connection_result(peer, result);
                        // `_slot` releases this connection's counted slot right here.
                    }
                    Err(active) => {
                        connection::refuse_for_capacity(&mut stream, peer, active, max_connections);
                    }
                }
            });
        }
        Transport::Tls { config, auth } => {
            let config = Arc::clone(config);
            let auth = auth.clone();
            thread::spawn(move || {
                let Some(mut tls_stream) = accept_tls(stream, &config, &auth, peer) else {
                    return; // accept_tls already logged why
                };
                match slot {
                    Ok(_slot) => {
                        let Some(db) = open_connection_database(&db_path, peer) else {
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_connection(tls_stream, &db)
                        }));
                        report_connection_result(peer, result);
                    }
                    Err(active) => {
                        connection::refuse_for_capacity(
                            &mut tls_stream,
                            peer,
                            active,
                            max_connections,
                        );
                    }
                }
            });
        }
    }
}

/// Opens this connection's own read-only [`Database`] handle from `path` (see
/// [`resolve_library_db_path`]). `None` (after logging why) if it can't be opened, so a
/// transient failure drops just this one connection rather than the accept loop.
fn open_connection_database(path: &std::path::Path, peer: Option<SocketAddr>) -> Option<Database> {
    match open_library_database(path) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::warn!("connection {peer:?}: {e}");
            None
        }
    }
}

/// One-line capability summary for [`run`]'s startup log line, matching what `WELCOME`
/// actually advertises.
#[cfg(feature = "worker")]
fn capability_summary(args: &ServeArgs) -> String {
    match args.compute_mode {
        ComputeMode::OnlyCpu => "library + render (CPU only, --only-cpu)".to_string(),
        ComputeMode::OnlyGpu => "library + render (GPU only, --only-gpu)".to_string(),
        ComputeMode::Hybrid => "library + render".to_string(),
    }
}

#[cfg(not(feature = "worker"))]
fn capability_summary(_args: &ServeArgs) -> String {
    "library only (no render capacity -- build with --features worker to add it)".to_string()
}

/// Unit tests for [`ConnectionLimiter`]/[`ConnectionSlot`] and
/// [`connection::refuse_for_capacity`] -- deliberately not gated on `feature = "worker"`
/// (unlike `mod tests` above/`serve::tests`), since the connection cap applies to a
/// library-only build exactly as much as a `worker` one (see this module's own doc
/// comment). No real sockets or threads: [`ConnectionLimiter`] is tested purely as
/// counter logic, and `refuse_for_capacity` against an in-memory buffer -- see
/// `serve::tests`/`serve::tests::mtls` (both `worker`-only) for the real-socket,
/// real-TLS end of the test pyramid this complements.
#[cfg(test)]
mod limiter_tests {
    use super::{ConnectionLimiter, connection};
    use std::sync::atomic::Ordering;

    #[test]
    fn acquires_up_to_max_and_refuses_the_next() {
        let limiter = ConnectionLimiter::new(2);
        let a = limiter
            .try_acquire()
            .expect("1st connection is under the cap");
        let b = limiter
            .try_acquire()
            .expect("2nd connection is exactly at the cap");
        let refused = limiter
            .try_acquire()
            .expect_err("3rd connection is over the cap");
        assert_eq!(
            refused, 2,
            "the refusal should report the active count that caused it"
        );

        drop(a);
        let c = limiter
            .try_acquire()
            .expect("a slot freed by drop can be reacquired");
        drop(b);
        drop(c);
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_refused_attempt_never_permanently_eats_into_the_cap() {
        let limiter = ConnectionLimiter::new(1);
        let _held = limiter.try_acquire().unwrap();
        for _ in 0..5 {
            limiter.try_acquire().unwrap_err();
        }
        // Each refusal above released the slot it provisionally reserved to observe the
        // count -- the active count must still read exactly 1 (`_held`), not 6.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_slot_releases_its_count_even_when_the_holder_panics() {
        let limiter = ConnectionLimiter::new(1);
        let slot = limiter.try_acquire().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = slot; // moved in; dropped while unwinding out of this closure
            panic!("simulated connection-thread panic while holding a ConnectionSlot");
        }));
        assert!(result.is_err());
        // `Drop::drop` runs while a panic unwinds (this is what `serve::run`'s real
        // connection threads rely on -- see `ConnectionLimiter::try_acquire`'s doc
        // comment) -- the slot must be released exactly as if the closure had returned
        // normally instead of panicking.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn refuse_for_capacity_writes_a_decodable_error_naming_the_capacity_code() {
        let mut buf: Vec<u8> = Vec::new();
        connection::refuse_for_capacity(&mut buf, None, 64, 64);

        let mut cursor = std::io::Cursor::new(buf);
        let err: indicatrix_net::messages::ErrorMsg =
            indicatrix_net::messages::read_message(&mut cursor).unwrap();
        assert_eq!(err.code, connection::CONNECTION_LIMIT_REACHED_CODE);
        assert!(err.message.contains("64"), "{}", err.message);

        // The same shape `indicatrix_net::client::handshake` already knows how to fall
        // back to for a BUILD_MISMATCH_CODE refusal (see `refuse_for_capacity`'s doc
        // comment) -- confirmed here by decoding as a bare `ErrorMsg` with no `Welcome`
        // ever written first.
    }
}

//! Per-socket plumbing shared by every listener (viewer, worker, enrollment) and by
//! `join`'s outbound connections: TCP tuning, the pre-protocol handshake deadline, the
//! per-connection read-only library database, and the payload-encoding preference.

use std::{
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use indicatrix_vault::db::sqlite::Database;

use crate::cli::ServeArgs;

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
pub(super) fn open_library_database(path: &std::path::Path) -> Result<Database, String> {
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
/// [`super::run`] hands out a path, not an already-open [`Database`]: `rusqlite::Connection`
/// isn't `Sync`, so it can't be shared via `Arc` the way `GpuBackend` is below. [`super::run`]
/// still opens (and immediately drops) one at startup, purely to fail fast on a bad
/// `--db` before binding a socket.
#[must_use]
pub(super) fn resolve_library_db_path(args: &ServeArgs) -> std::path::PathBuf {
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
pub fn tune_accepted_socket(stream: &TcpStream, peer: Option<SocketAddr>) {
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
/// handshake -- the TLS handshake (see [`super::tls::accept_tls`]) plus the first `HELLO`
/// frame, or (under `--insecure-no-tls`) just the first `HELLO` frame -- before this
/// worker gives up on it. Mitigates a slowloris-style client that connects (and, for
/// TLS, maybe completes the handshake) and then never speaks: without this, such a
/// client holds a connection thread open for as long as the OS's own TCP defaults
/// allow.
///
/// Applied once, in [`super::run`]'s accept loop, to the raw accepted `TcpStream` -- before
/// either transport branch, so it covers a TLS listener's handshake (run inside
/// [`super::tls::accept_tls`]) and, either way, the `HELLO` read that follows. Cleared the
/// moment `HELLO` arrives (see the connection handlers' doc comments), so it never bounds
/// anything the connection loop itself does afterward -- that loop has its own,
/// different timeout story (see `stream_emit::TimeoutCache`).
///
/// `pub(crate)` so `crate::enroll`'s connection handling can apply the same bound to its
/// own TLS-handshake-then-one-message exchange -- that listener has no
/// mutual-TLS authentication step at all (see its own module doc comment), so it needs a
/// deadline at least as much as this one does.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Applies [`HANDSHAKE_TIMEOUT`] to `stream`'s read and write timeouts. Best-effort,
/// like [`tune_accepted_socket`]: a failure is logged at `debug` and never aborts the
/// accept loop -- worst case, this one connection goes back to being unbounded, exactly
/// like every connection was before this deadline existed.
pub fn apply_handshake_timeout(stream: &TcpStream, peer: Option<SocketAddr>) {
    if let Err(e) = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake read timeout: {e}");
    }
    if let Err(e) = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake write timeout: {e}");
    }
}

/// The payload-encoding preference (v14) this server uses for a connection from `peer`:
/// raw only for a loopback peer (memory bandwidth beats every codec), else the measured
/// default -- zstd level 1, then LZ4, then raw (coordinator guide section 11). The value
/// handed to the connection handler, which negotiates against the
/// peer's `HELLO.accept_encodings`; a later CLI setting plugs in here.
#[cfg(feature = "worker")]
pub fn payload_preference_for(
    peer: Option<SocketAddr>,
) -> &'static [indicatrix_net::messages::PayloadEncoding] {
    if peer.is_some_and(|p| p.ip().is_loopback()) {
        &indicatrix_net::messages::LOOPBACK_SERVER_PREFERENCE
    } else {
        &indicatrix_net::messages::DEFAULT_SERVER_PREFERENCE
    }
}

/// Opens this connection's own read-only [`Database`] handle from `path` (see
/// [`resolve_library_db_path`]). `None` (after logging why) if it can't be opened, so a
/// transient failure drops just this one connection rather than the accept loop.
pub(super) fn open_connection_database(
    path: &std::path::Path,
    peer: Option<SocketAddr>,
) -> Option<Database> {
    match open_library_database(path) {
        Ok(db) => Some(db),
        Err(e) => {
            tracing::warn!("connection {peer:?}: {e}");
            None
        }
    }
}

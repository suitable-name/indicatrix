//! Per-socket plumbing shared by every listener (viewer, worker, enrollment) and by
//! `join`'s outbound connections: TCP tuning, the pre-protocol handshake deadline, the
//! read-only library database opener, and the payload-encoding preference.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use indicatrix_vault::db::sqlite::Database;

use crate::cli::ServeArgs;

/// A TOTAL deadline across however many logical reads a caller performs through this
/// wrapper -- unlike the per-read idle timeout [`apply_handshake_timeout`] alone
/// applies, which resets its own allowance on every call and so never fires against a
/// peer that keeps a connection alive by trickling data, always inside that idle window
/// (see [`HANDSHAKE_TIMEOUT`]'s doc comment and F-06a). Checked once per underlying
/// `read` call -- BEFORE attempting it -- so such a peer is cut off the moment the
/// whole budget has elapsed, without waiting for any single read to time out on its own.
///
/// Generic over any `Read`/`Write` (a `HELLO` read over a connection's own stream type,
/// generic in turn; an `EnrollRequest` read over an in-memory duplex in this crate's own
/// tests) rather than requiring the concrete ability to re-arm an OS-level socket
/// timeout -- see [`DeadlineSocket`] for the complement that DOES re-arm a raw socket,
/// used where the concrete type is already in hand (a TLS handshake). A single
/// underlying read that blocks past the deadline (the peer goes fully silent rather than
/// trickling) is still bounded only by whatever timeout `inner` already carries -- one
/// more wait shaped like [`HANDSHAKE_TIMEOUT`], worst case, not unbounded.
pub struct DeadlineIo<'a, S> {
    inner: &'a mut S,
    deadline: Instant,
}

impl<'a, S> DeadlineIo<'a, S> {
    pub(crate) const fn new(inner: &'a mut S, deadline: Instant) -> Self {
        Self { inner, deadline }
    }

    /// `Err(TimedOut)` once `deadline` has passed, without touching `inner` at all.
    fn check_deadline(&self) -> std::io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "pre-HELLO handshake deadline exceeded",
            ));
        }
        Ok(())
    }
}

impl<S: Read> Read for DeadlineIo<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.check_deadline()?;
        self.inner.read(buf)
    }
}

impl<S: Write> Write for DeadlineIo<'_, S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The complement to [`DeadlineIo`] for the one phase where the concrete socket is
/// already in hand and re-arming its real OS-level read timeout is possible: re-arms
/// `sock`'s read timeout to the time remaining before `deadline` before every read,
/// tightening [`DeadlineIo`]'s bound for a peer that goes fully silent (rather than
/// trickling) right up against the deadline -- cut off within roughly one read's worth
/// of slack instead of another whole idle timeout. Used only for
/// [`super::tls::accept_tls`]'s and `crate::enroll::connection::accept_enroll_tls`'s TLS
/// handshake, the one call in this crate that owns a concrete [`TcpStream`] across a
/// `Read`/`Write` call it doesn't otherwise control the internals of
/// (`rustls::ServerConnection::complete_io`'s own internal read/write loop).
pub struct DeadlineSocket<'a> {
    sock: &'a TcpStream,
    deadline: Instant,
}

impl<'a> DeadlineSocket<'a> {
    pub(crate) const fn new(sock: &'a TcpStream, deadline: Instant) -> Self {
        Self { sock, deadline }
    }

    /// Re-arms `sock`'s read timeout to the time remaining, or fails outright without
    /// touching `sock` at all once `deadline` has already passed.
    fn rearm(&self) -> std::io::Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "pre-HELLO handshake deadline exceeded",
            ));
        }
        self.sock.set_read_timeout(Some(remaining))
    }
}

impl Read for DeadlineSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.rearm()?;
        let mut sock = self.sock;
        sock.read(buf)
    }
}

impl Write for DeadlineSocket<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut sock = self.sock;
        sock.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut sock = self.sock;
        sock.flush()
    }
}

/// Opens the design-library database this `serve` instance serves: `args.db` if given,
/// else the default `facet_diagrams.sqlite` relative to the process's working directory.
///
/// Always opened READ-ONLY -- this phase never writes to the catalogue, so `serve`
/// fails fast with a clear error if the database doesn't already exist rather than
/// silently creating an empty one.
///
/// Called once at start-up to validate the path, and then by each connection's
/// [`super::library::LibraryHandle`] on its first library request (see
/// [`resolve_library_db_path`]), not opened once and shared: `rusqlite::Connection` is
/// `Send` but not `Sync`, and SQLite's concurrency model is many independent reader
/// connections.
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
/// connection opens its own [`open_library_database`] handle from, when it first serves
/// a library request.
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

/// The TOTAL budget for a just-accepted connection's pre-protocol handshake -- the TLS
/// handshake (see [`super::tls::accept_tls`]) and, separately, the first `HELLO` frame
/// (or, under `--insecure-no-tls`, just the `HELLO` frame) -- before this worker gives up
/// on it. Mitigates a slowloris-style client that connects (and, for TLS, maybe
/// completes the handshake) and then never speaks, OR that never goes idle long enough
/// to trip a per-read timeout by trickling a byte at a time forever (F-06a): without a
/// real deadline enforcing this budget, such a client holds a connection thread open for
/// as long as the OS's own TCP defaults allow -- observed as long as ~86 hours for a
/// 16 KiB TLS record trickled one byte per 19s.
///
/// Two DIFFERENT mechanisms apply this bound, and it's spent independently by each:
/// - [`apply_handshake_timeout`] sets a fixed, per-read IDLE timeout on the raw
///   `TcpStream` once, at accept time -- catches a peer that goes silent for the whole
///   window, but not one that keeps sending SOMETHING, just slowly.
/// - [`accept_tls`](super::tls::accept_tls)'s TLS handshake and
///   [`super::handshake::read_and_check_hello`]'s `HELLO` read EACH additionally wrap
///   their own reads in [`DeadlineSocket`]/[`DeadlineIo`] with a deadline computed as
///   `Instant::now() + HANDSHAKE_TIMEOUT` at the START of that phase -- a REAL deadline,
///   re-checked (and, where the concrete socket is in hand, re-armed) before every read,
///   so a trickling peer is cut off once its own phase's budget elapses regardless of how
///   many small reads it takes. Because each phase computes its own fresh deadline (nothing
///   here changes either function's signature to thread one shared instant through both --
///   see those functions' doc comments), the two phases are bounded independently rather
///   than sharing one end-to-end budget: a connection that spends up to
///   `HANDSHAKE_TIMEOUT` in the TLS handshake and then up to another `HANDSHAKE_TIMEOUT`
///   reading `HELLO` is still finite (worst case ~2x this constant), a large improvement
///   over the previous unbounded behaviour, if not a single tight `HANDSHAKE_TIMEOUT`-wide
///   window.
///
/// Cleared the moment `HELLO` arrives (see the connection handlers' doc comments), so it
/// never bounds anything the connection loop itself does afterward -- that loop has its
/// own, different timeout story (see `stream_emit::TimeoutCache`).
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

#[cfg(test)]
mod tests {
    use super::DeadlineIo;
    use std::{
        io::Read,
        time::{Duration, Instant},
    };

    /// A `Read` that hands back exactly one byte per call, sleeping `gap` first every
    /// time -- simulates a peer that keeps a connection alive by trickling data, always
    /// well inside any single idle read timeout, forever.
    struct Trickle {
        gap: Duration,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.gap);
            buf[0] = 0;
            Ok(1)
        }
    }

    /// F-06a: a peer sending one byte every 100 ms is still cut off once a 300 ms TOTAL
    /// budget elapses -- `DeadlineIo` checks the deadline before every read, so this
    /// takes effect on roughly the 3rd/4th read rather than needing any single read to
    /// go idle.
    #[test]
    fn deadline_io_drops_a_trickling_peer_once_the_total_budget_elapses() {
        let mut trickle = Trickle {
            gap: Duration::from_millis(100),
        };
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut guarded = DeadlineIo::new(&mut trickle, deadline);

        let start = Instant::now();
        let mut buf = [0u8; 16];
        let err = guarded.read_exact(&mut buf).expect_err(
            "a 16-byte read at one byte/100ms must never complete inside a 300ms budget",
        );
        let elapsed = start.elapsed();

        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(
            elapsed >= Duration::from_millis(250) && elapsed < Duration::from_millis(700),
            "expected to be dropped at ~300ms, was dropped at {elapsed:?}"
        );
    }

    /// The complement: a deadline that has already passed refuses the very first read,
    /// with no attempt to read anything at all (proven by a `Trickle`-shaped reader that
    /// would otherwise sleep -- if `DeadlineIo` called it, this test would take 100ms
    /// instead of returning immediately).
    #[test]
    fn deadline_io_refuses_immediately_once_the_deadline_has_already_passed() {
        let mut trickle = Trickle {
            gap: Duration::from_millis(100),
        };
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("the process has been running for at least 1ms");
        let mut guarded = DeadlineIo::new(&mut trickle, deadline);

        let start = Instant::now();
        let mut buf = [0u8; 1];
        let err = guarded.read_exact(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "must fail without ever calling the inner reader"
        );
    }
}

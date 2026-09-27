//! Opening the TCP socket, the mutual-TLS handshake and `HELLO`/`WELCOME` exchange,
//! and the settings UI's standalone "Test connection" operation built on top of it.

use super::super::types::{RemoteError, RemoteStream, host_from_address};
use crate::settings::WorkerSettings;
use socket2::{SockRef, TcpKeepalive};
use std::net::{TcpStream, ToSocketAddrs};

/// # Known cost, measured, and what was done about it
///
/// This function's full cost (three file reads, building a `rustls::ClientConfig`, TCP
/// connect, TLS 1.3 handshake, `HELLO`/`WELCOME`) measured end-to-end on loopback
/// against a real worker: ~2.5ms median, ~2.6ms mean, dominated by the TCP connect (1
/// RTT) and TLS handshake (1 more RTT) -- so on a link with `R` ms of RTT, expect
/// roughly this total plus `~2R` ms.
///
/// On loopback/LAN this is noise next to the render itself and the 600ms
/// `SETTLE_DEBOUNCE` gating every dispatch. But higher-latency links (a remote worker
/// over a slower connection) are a real deployment target, and on one of those `~2R` ms
/// per settle is a user-visible stall before the first `FRAME`, repeated on every
/// drag-then-pause. That is why `super::persistent::spawn_remote_connection` exists: it
/// lets `gui::remote::orchestrator` hold one connection open across a whole live session
/// and pay this cost once. `super::one_shot::spawn_remote_render` (this function's other
/// caller, along with [`test_connection`]) stays one connection per call, for callers
/// where that repetition doesn't apply.
///
/// Resolves `address` (a `host:port` string) and connects with a bounded deadline
/// ([`super::CONNECT_TIMEOUT`]), then configures the socket for a long-lived,
/// latency-sensitive, bidirectional protocol connection:
///
/// - `TCP_NODELAY` -- a `RenderRequest`/`CANCEL`/stream-event frame must never sit in
///   Nagle's buffer waiting for more data that isn't coming; this protocol's messages are
///   sent one at a time; not batched.
/// - OS-level TCP keepalive ([`super::KEEPALIVE_TIME`]/[`super::KEEPALIVE_INTERVAL`], via
///   the `socket2` crate -- `std::net::TcpStream` has no keepalive-tuning API of its own)
///   so a network-level dead peer is noticed by the kernel even while nothing above this
///   layer is trying to read or write.
///
/// # Errors
///
/// Any I/O error resolving `address` (including "no address found" for a string that
/// doesn't resolve to anything), connecting within [`super::CONNECT_TIMEOUT`], or
/// configuring the socket.
fn open_tcp(address: &str) -> std::io::Result<TcpStream> {
    let addr = address.to_socket_addrs()?.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no address found for {address:?}"),
        )
    })?;
    let tcp = TcpStream::connect_timeout(&addr, super::CONNECT_TIMEOUT)?;
    tcp.set_nodelay(true)?;
    let keepalive = TcpKeepalive::new()
        .with_time(super::KEEPALIVE_TIME)
        .with_interval(super::KEEPALIVE_INTERVAL);
    SockRef::from(&tcp).set_tcp_keepalive(&keepalive)?;
    Ok(tcp)
}

/// Connects to `worker.address` over mutual TLS (certificates from `worker.cert_dir`)
/// and performs the `HELLO`/`WELCOME` handshake, including
/// [`indicatrix_net::handshake::verify_compatible`]'s client-side defense-in-depth
/// check. Shared by `super::one_shot::run` (a full render), [`test_connection`]
/// (handshake only), and, `pub(crate)`, by `bridge::library::client` (the read-only
/// design-library protocol rides the same authenticated connection) -- so every caller
/// connects identically.
///
/// The returned stream carries [`super::WRITE_TIMEOUT`] for the rest of its life and,
/// past this function's [`super::HANDSHAKE_TIMEOUT`]-bounded read, no read timeout at
/// all -- callers set their own afterward (`super::one_shot::run`'s/
/// `super::persistent::run_connection`'s poll-timeout reads via
/// `super::stream_io::try_read_one_frame`, or `bridge::library::client`'s longer,
/// static one).
///
/// # `ErrorKind::WriteZero` is a real I/O failure here, not a partial-write retry signal
///
/// `rustls::Stream::write`'s `complete_io` can fail (a write timing out against
/// [`super::WRITE_TIMEOUT`]), but its public signature surfaces that as plain
/// `WriteZero` rather than the underlying `TimedOut`. Every write-error site in this
/// module (and `bridge::library::client`) already treats any error as fatal without
/// matching a specific `ErrorKind`, so this is handled correctly -- noted here so a
/// future reader chasing a `WriteZero` in a log doesn't go looking for an actual
/// zero-length write bug.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn connect_and_handshake(
    worker: &WorkerSettings,
) -> Result<(RemoteStream, indicatrix_net::messages::Welcome), RemoteError> {
    let ca = indicatrix_net::tls::load_ca(&worker.ca_path())?;
    let cert_chain = indicatrix_net::tls::load_certs(&worker.client_cert_path())?;
    let key = indicatrix_net::tls::load_private_key(&worker.client_key_path())?;
    let config = indicatrix_net::tls::client_config(ca, cert_chain, key)?;

    let tcp = open_tcp(&worker.address)?;
    let host = host_from_address(&worker.address);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| RemoteError::InvalidServerName(host.to_string()))?;
    let conn = rustls::ClientConnection::new(config, server_name)
        .map_err(indicatrix_net::tls::TlsError::Rustls)?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    // Bounds the TLS handshake below AND the HELLO/WELCOME read inside `handshake` --
    // both run before this function returns, and neither had any deadline before.
    stream
        .sock
        .set_read_timeout(Some(super::HANDSHAKE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Persists on the socket for the connection's whole remaining life -- see this
    // function's own doc comment's `WriteZero` section for what a timed-out write looks
    // like to a caller.
    stream
        .sock
        .set_write_timeout(Some(super::WRITE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Force the handshake to complete now, mirroring
    // `apps/indicatrix-worker/src/serve.rs::accept_tls` on the server side, so a TLS
    // failure (wrong CA, expired cert, clock skew, SAN mismatch) is diagnosed here
    // rather than surfacing later as an opaque I/O error out of the handshake below.
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(RemoteError::Io)?;

    let welcome = indicatrix_net::client::handshake::handshake(&mut stream)?;
    // Clear the handshake-scoped read timeout now that it's served its purpose --
    // every read after this point sets its own (see this function's own doc comment),
    // so leaving `HANDSHAKE_TIMEOUT` in place would just be a stale, misleading value
    // sitting on the socket between here and the first such call.
    stream
        .sock
        .set_read_timeout(None)
        .map_err(RemoteError::Io)?;
    Ok((stream, welcome))
}

/// The settings UI's "Test connection" operation: connect, handshake, report worker
/// identity/backend/build compatibility, then disconnect (simply by dropping the
/// stream when this returns) -- no render. Blocking; callers run this on their own
/// worker thread (see `gui::remote::setup_worker_callbacks`) and report the result back
/// to the UI thread themselves.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn test_connection(
    worker: &WorkerSettings,
) -> Result<indicatrix_net::client::ConnectionInfo, RemoteError> {
    let (_stream, welcome) = connect_and_handshake(worker)?;
    Ok(welcome.into())
}

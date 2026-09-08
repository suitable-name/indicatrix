//! Owns the actual mutual-TLS socket to one configured remote worker and drives one
//! `RenderRequest` against it.
//!
//! Exercised only by running a real worker and viewer against each other; the pure
//! helpers are unit-tested but the socket-owning driver is not.
//!
//! # Why one thread owns both directions
//!
//! TLS record state (`rustls::ClientConnection`) cannot be safely split across two
//! threads the way a plaintext `TcpStream::try_clone` could be. A worker thread here
//! is the sole owner of the stream: it alternates a short, timeout-bounded read of the
//! next [`indicatrix_net::messages::StreamEvent`] with a non-blocking check of its
//! inbound command channel, so `CANCEL` can be written promptly.
//!
//! # Two connection lifecycles: one-shot and persistent
//!
//! [`connection::spawn_remote_render`] owns its connection for exactly one
//! `RenderRequest` (connect, handshake, stream to `DONE`, exit) -- used by
//! `bridge::export_thread::remote`'s one-shot dispatches.
//!
//! [`connection::spawn_remote_connection`] returns a [`types::RemoteConnectionHandle`]
//! whose background thread outlives any single request, accepting a stream of
//! [`types::RemoteRenderRequest`]s and reusing one mutual-TLS connection across all of
//! them, reconnecting (full handshake) only once it has actually died. Used by
//! `gui::remote::orchestrator`, which dispatches repeatedly against the same worker
//! across many camera settles in one session.
//!
//! # Timeouts and liveness
//!
//! Every blocking step has a deadline: [`connection::connect_and_handshake`] bounds the
//! TCP connect, TLS handshake, `HELLO`/`WELCOME` exchange, and every write. Both `run`
//! and `run_connection` additionally enforce a liveness deadline -- no event
//! (`FRAME`/`PREVIEW`/`PROGRESS`/`DONE`/`ERROR`) within it fails the request rather than
//! polling forever. The deadline is two-tiered: a longer `FIRST_EVENT_TIMEOUT` covers a
//! dispatch's first event (worker calibration/warm-up at high resolution can legitimately
//! outrun the steady-state deadline), and the tighter, heartbeat-derived
//! `LIVENESS_TIMEOUT` covers every wait after that. See `connection`'s own doc comment
//! for the constant list and the `liveness_deadline` decision. Every
//! [`types::RemoteUpdate`] carries a `request_id` so a consumer juggling more than one
//! request can tell a stale update from a current one.
//!
//! `bridge::export_thread::remote::dispatch` applies the identical two-tier deadline one
//! level up its own stack.
//!
//! # Module split
//!
//! Split into [`types`] (handle/command/error types both lifecycles share) and
//! [`connection`] (the two lifecycles, plus the shared handshake and frame/event readers).

mod connection;
mod types;

pub use connection::{
    connect_and_handshake, spawn_remote_connection, spawn_remote_render, test_connection,
};
pub use types::{
    RemoteConnectionHandle, RemoteError, RemoteRenderHandle, RemoteRenderRequest, RemoteStream,
    RemoteUpdate,
};

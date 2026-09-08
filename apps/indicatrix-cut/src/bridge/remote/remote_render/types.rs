//! The handle/command/error types both connection lifecycles ([`super::connection`])
//! share: [`RemoteError`], [`RemoteCommand`]/[`RemoteUpdate`], [`RemoteRenderHandle`],
//! [`RemoteRenderRequest`], and [`RemoteConnectionHandle`] plus its own private
//! command/request bookkeeping types. See this group's own `mod.rs` doc comment.

use crate::settings::WorkerSettings;
use indicatrix_net::{
    client::{Accumulator, ClientError, ConnectionInfo},
    messages::NetError,
    tls::TlsError,
};
use std::{
    fmt,
    net::TcpStream,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

pub type RemoteStream = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// One decoded [`indicatrix_net::messages::StreamEvent`] paired with its raw payload
/// frame, where it has one (`Frame`/`Preview`) -- the return shape of
/// `super::connection::try_read_stream_event`, factored into a named type purely to
/// keep that function's signature simple.
pub(super) type StreamEventFrame = (indicatrix_net::messages::StreamEvent, Option<Vec<u8>>);

/// Everything that can go wrong establishing or running a remote render, folded into
/// one type so [`super::connection::spawn_remote_render`]'s worker thread has a
/// single error path to report through [`RemoteUpdate::Failed`].
#[derive(Debug)]
pub enum RemoteError {
    Tls(TlsError),
    Io(std::io::Error),
    Client(ClientError),
    /// `WorkerSettings::address`'s host portion isn't a valid TLS server name (e.g.
    /// empty, or not parseable as a hostname/IP).
    InvalidServerName(String),
    /// The worker connected and authenticated fine but advertises no render capacity
    /// (`Welcome::render` is `None`) -- it is a library-only server, built without its
    /// `worker` feature. Distinct from every other variant here on purpose: nothing is
    /// broken, the operator simply pointed the viewer at a server that does not render,
    /// and telling them that is more useful than a generic connection failure.
    NoRenderCapacity,
    /// [`super::connection::spawn_remote_render`]'s one-shot loop went longer than its
    /// liveness deadline (`super::connection::LIVENESS_TIMEOUT` by default) without a
    /// single `FRAME`/`PREVIEW`/`PROGRESS`/`DONE`/`ERROR` from the worker, with the
    /// request still in flight. Distinct from [`Self::Io`] on purpose: no I/O error
    /// actually occurred -- the read kept returning `WouldBlock`/`TimedOut` cleanly,
    /// exactly as `try_read_stream_event` treats "nothing pending yet" -- the
    /// connection just stopped saying anything at all, which a bare I/O error would
    /// never distinguish from a worker legitimately taking its time on a slow tick.
    WorkerSilent(Duration),
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tls(e) => write!(f, "TLS error: {e}"),
            Self::Io(e) => write!(f, "connection error: {e}"),
            Self::Client(e) => write!(f, "{e}"),
            Self::InvalidServerName(host) => write!(f, "not a valid worker hostname: {host:?}"),
            Self::NoRenderCapacity => write!(
                f,
                "this worker serves the design library but cannot render -- it was built                  without its `worker` feature"
            ),
            Self::WorkerSilent(elapsed) => write!(f, "worker silent for {elapsed:.0?}"),
        }
    }
}

impl From<TlsError> for RemoteError {
    fn from(e: TlsError) -> Self {
        Self::Tls(e)
    }
}
impl From<std::io::Error> for RemoteError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<ClientError> for RemoteError {
    fn from(e: ClientError) -> Self {
        Self::Client(e)
    }
}
impl From<NetError> for RemoteError {
    fn from(e: NetError) -> Self {
        Self::Client(e.into())
    }
}

/// A command the UI/orchestrator side sends to the running worker thread -- see the
/// module doc comment on why this goes through a channel rather than a second thread
/// touching the socket directly.
pub enum RemoteCommand {
    Cancel,
}

/// One update surfaced to the caller's `on_update` callback as a remote render
/// progresses. Deliberately does not carry the accumulated buffer itself -- the caller
/// already shares the same `Arc<Mutex<Accumulator>>` passed to
/// [`super::connection::spawn_remote_render`] and reads it directly under its own lock;
/// this just says when something changed.
///
/// Every variant carries `request_id` (see [`Self::request_id`]) so a consumer juggling
/// more than one request over its lifetime (`gui::remote::orchestrator`'s persistent
/// connection, which supersedes an in-flight request whenever the camera settles again
/// before the previous one finished) can tell a stale update apart from one for what's
/// current now. [`super::connection::route_event`]'s `Accumulator::apply` epoch check
/// already prevents a stale reply from corrupting the accumulator itself; this is the
/// analogous guard for `on_update` side effects (toasts, UI state) that `apply` has no
/// way to gate on its own.
#[derive(Debug, Clone)]
pub enum RemoteUpdate {
    /// The handshake completed; streaming is about to begin. Corresponds to
    /// `bridge::remote::handoff::HandoffEvent::RemoteStreamStarted`.
    Connected {
        request_id: u32,
        info: ConnectionInfo,
    },
    Frame {
        request_id: u32,
        samples_done: u32,
    },
    Preview {
        request_id: u32,
    },
    Progress {
        request_id: u32,
        samples_done: u32,
    },
    Done {
        request_id: u32,
        cancelled: bool,
    },
    /// The attempt failed for any reason -- connection, handshake, or a transport
    /// error mid-stream. Corresponds to
    /// `bridge::remote::handoff::HandoffEvent::RemoteFailed`.
    Failed {
        request_id: u32,
        message: String,
    },
}

impl RemoteUpdate {
    /// The request this update pertains to -- see this type's own doc comment for why
    /// every variant carries one.
    #[must_use]
    pub const fn request_id(&self) -> u32 {
        match self {
            Self::Connected { request_id, .. }
            | Self::Frame { request_id, .. }
            | Self::Preview { request_id }
            | Self::Progress { request_id, .. }
            | Self::Done { request_id, .. }
            | Self::Failed { request_id, .. } => *request_id,
        }
    }
}

/// How [`RemoteRenderHandle::cancel`] reaches the worker thread that owns the
/// connection -- a one-shot thread only ever has one request to cancel, while a
/// [`RemoteConnectionHandle`]'s persistent thread may have moved on to a later request
/// by the time an older handle's `cancel()` is called; the `request_id` carried here
/// lets that thread tell "cancel my still-current request" apart from "cancel one
/// I've already superseded" (a harmless no-op). See [`ConnectionCommand::Cancel`].
pub(super) enum HandleKind {
    OneShot(mpsc::Sender<RemoteCommand>),
    Connection {
        request_id: u32,
        commands: mpsc::Sender<ConnectionCommand>,
    },
}

/// Handle returned by [`super::connection::spawn_remote_render`]/
/// [`RemoteConnectionHandle::render`]. Cancelling is fire-and-forget: send the cancel
/// command and let the worker thread's own `DONE { cancelled: true }` (surfaced via
/// `on_update`) confirm it, matching `bridge::export_thread::ExportHandle`'s existing
/// cooperative-cancellation shape in this same crate.
pub struct RemoteRenderHandle(pub(super) HandleKind);

impl RemoteRenderHandle {
    pub fn cancel(&self) {
        match &self.0 {
            HandleKind::OneShot(commands) => {
                let _ = commands.send(RemoteCommand::Cancel);
            }
            HandleKind::Connection {
                request_id,
                commands,
            } => {
                let _ = commands.send(ConnectionCommand::Cancel {
                    request_id: *request_id,
                });
            }
        }
    }
}

/// Extracts the host portion of a `host:port` address (whatever follows the last `:` is
/// the port, matching `WorkerSettings::address`'s plain `host:port` convention, not a
/// bracketed URI authority). Falls back to the whole string if there's no `:` at all,
/// so a malformed address still produces some `ServerName` attempt and a clear
/// TLS-layer error rather than silently doing nothing.
#[must_use]
pub(super) fn host_from_address(address: &str) -> &str {
    address.rsplit_once(':').map_or(address, |(host, _)| host)
}

/// Everything [`super::connection::spawn_remote_render`] needs to know about the ONE
/// `RenderRequest` it will send, grouped into a struct purely to keep that function's
/// (and `run`'s) own parameter list short -- see [`WorkerSettings`]'s own doc comment
/// for why `width`/`height` travel alongside `worker` here rather than living on
/// `WorkerSettings` itself (render resolution is session-wide, not per-worker).
pub struct RemoteRenderRequest {
    pub worker: WorkerSettings,
    pub request_id: u32,
    pub scene: indicatrix_net::SceneState,
    pub first_sample: u32,
    pub samples: u32,
    pub width: u32,
    pub height: u32,
}

/// A handle to one persistent, mutual-TLS connection to a configured remote worker,
/// reused across many settles of a live viewport session instead of paying a fresh
/// TCP+TLS+HELLO/WELCOME handshake on every one -- see the module doc comment's "Known
/// cost" section. Owned by `gui::remote::orchestrator`: exactly one live handle per
/// worker in use, created lazily by [`super::connection::spawn_remote_connection`] and
/// dropped (tearing the connection down) whenever
/// `gui::remote::orchestrator::connection_is_stale` decides the cached identity no
/// longer matches what's configured (address/`cert_dir` edited, worker removed, remote
/// compute switched off). A settings change touching certificates must force a
/// reconnect: a connection already authenticated under the old certificates would
/// otherwise keep being used as if the new ones had been verified.
///
/// # Reconnection
///
/// The background thread's connection will eventually die on its own (worker restarts,
/// laptop sleeps, a NAT entry expires) with nothing this side did wrong. [`Self::render`]
/// never surfaces that as a render failure on its own: the next call after a dead
/// connection is noticed transparently reconnects first (a full mutual-TLS handshake,
/// never a shortcut) before sending that request. Only if the reconnect itself fails
/// does the request end in [`RemoteUpdate::Failed`].
///
/// # Not used for the design-library protocol or for exports
///
/// `bridge::library::client`'s `LibrarySession` already solves connection reuse for its
/// own access pattern (many requests in a tight mirror-sync loop) independently --
/// sharing one connection between a live render stream and an on-demand library
/// browse/sync would entangle two independently-lifecycled features for no measured
/// benefit. `bridge::export_thread::remote` keeps using
/// [`super::connection::spawn_remote_render`]'s one-shot form too: an export's few
/// dispatches happen once per user action, not repeatedly across a session's many
/// settles, so the repeated-handshake cost this type avoids is a non-issue there.
pub struct RemoteConnectionHandle {
    pub(super) worker: WorkerSettings,
    pub(super) commands: mpsc::Sender<ConnectionCommand>,
}

impl RemoteConnectionHandle {
    /// The worker settings this connection was built for -- compared (by
    /// `gui::remote::orchestrator::connection_is_stale`) against whatever is currently
    /// configured to decide whether this handle is still current or needs replacing.
    #[must_use]
    pub const fn worker(&self) -> &WorkerSettings {
        &self.worker
    }

    /// Dispatches `request` onto this persistent connection.
    ///
    /// If a previous request is still in flight (no terminal [`RemoteUpdate`] observed
    /// yet), it is superseded: a best-effort `CANCEL` is sent for it, then its
    /// `on_update` callback and accumulator are dropped without being touched again.
    /// Any reply for the superseded request already in flight on the wire is guarded
    /// against by [`indicatrix_net::client::Accumulator::apply`]'s own epoch check
    /// (this new request's `accumulator` only ever begins its own epoch) -- that, not
    /// the best-effort `CANCEL`, is the actual correctness mechanism.
    ///
    /// Connects (or reconnects, after a dead connection) on demand, on the background
    /// thread, so this call itself never blocks.
    pub fn render(
        &self,
        request: RemoteRenderRequest,
        accumulator: Arc<Mutex<Accumulator>>,
        on_update: impl FnMut(RemoteUpdate) + Send + 'static,
    ) -> RemoteRenderHandle {
        let request_id = request.request_id;
        let _ = self
            .commands
            .send(ConnectionCommand::Render(Box::new(RenderCommand {
                request,
                accumulator,
                on_update: Box::new(on_update),
            })));
        RemoteRenderHandle(HandleKind::Connection {
            request_id,
            commands: self.commands.clone(),
        })
    }
}

/// One command sent to a [`super::connection::spawn_remote_connection`] background
/// thread over its `commands` channel.
pub(super) enum ConnectionCommand {
    /// Dispatch a new [`RemoteRenderRequest`], superseding whatever was previously in
    /// flight -- see [`RemoteConnectionHandle::render`]'s doc comment. Boxed (along
    /// with its two companion fields, wrapped together purely so all three travel as
    /// one allocation) to keep this enum's `Cancel` variant from paying for
    /// `RemoteRenderRequest`'s much larger `SceneState` -- mirrors
    /// `indicatrix_net::messages::ClientMessage::RenderRequest`'s own identical reasoning.
    Render(Box<RenderCommand>),
    /// Cancel `request_id` specifically -- a no-op if the connection's current request
    /// is no longer `request_id` (already superseded or finished), which is exactly
    /// what makes it safe for [`RemoteRenderHandle::cancel`] to call this even long
    /// after its own request could plausibly have already ended.
    Cancel { request_id: u32 },
}

/// The payload of [`ConnectionCommand::Render`] -- see that variant's own doc comment
/// for why it travels boxed.
pub(super) struct RenderCommand {
    pub(super) request: RemoteRenderRequest,
    pub(super) accumulator: Arc<Mutex<Accumulator>>,
    pub(super) on_update: Box<dyn FnMut(RemoteUpdate) + Send>,
}

/// The one request currently "owned" by a persistent connection's background thread --
/// the only request whose replies `super::connection::route_event` will ever apply or
/// report.
pub(super) struct CurrentRequest {
    pub(super) request_id: u32,
    pub(super) accumulator: Arc<Mutex<Accumulator>>,
    pub(super) on_update: Box<dyn FnMut(RemoteUpdate) + Send>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_from_address_strips_the_trailing_port() {
        assert_eq!(host_from_address("worker.local:9443"), "worker.local");
        assert_eq!(host_from_address("192.168.1.50:9443"), "192.168.1.50");
    }

    #[test]
    fn host_from_address_falls_back_to_the_whole_string_without_a_colon() {
        assert_eq!(host_from_address("worker.local"), "worker.local");
        assert_eq!(host_from_address(""), "");
    }

    #[test]
    fn remote_error_display_is_human_readable() {
        let e = RemoteError::InvalidServerName("bad host".to_string());
        assert!(e.to_string().contains("bad host"));
    }
}

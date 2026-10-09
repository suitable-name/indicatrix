//! The handle/command/error types both connection lifecycles ([`super::connection`])
//! share: [`RemoteError`], [`RemoteCommand`]/[`RemoteUpdate`], [`RemoteRenderHandle`],
//! [`RemoteRenderRequest`], and [`RemoteConnectionHandle`] plus its own private
//! command/request bookkeeping types. See this group's own `mod.rs` doc comment.

use crate::settings::WorkerSettings;
use glam::Vec3;
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

/// Buffered TLS stream to a remote worker.
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
    /// Protocol v14: the scene is lit by an HDR map but the remote's `WELCOME` does not
    /// advertise HDR support (`RenderCapability::hdr`), so the request was not sent.
    HdrUnsupported,
    /// Protocol v14: the remote asked for the scene's HDR map (`NEED_ASSET`) and it could
    /// not be sent (unknown hash, unreadable or changed file, or the upload failed).
    Asset(String),
    /// A `CANCEL` was written and the worker neither ended the request nor closed the
    /// stream within `super::connection::CANCEL_ACK_TIMEOUT`. Distinct from
    /// [`Self::WorkerSilent`] on purpose: a worker that keeps heartbeating resets the
    /// silence clock on every `PROGRESS`, so a wedged tracer that ignores the cancel
    /// would otherwise keep this one-shot thread alive for as long as the socket lives.
    CancelUnacknowledged(Duration),
    /// `zoning` builds: the scene's material has colour zones but the remote's `WELCOME` does
    /// not advertise the zoning capability (a default build, or a coordinator), so the request
    /// was not sent -- it would render as its base zone only. The picture renders locally.
    #[cfg(feature = "zoning")]
    ZoningUnsupported,
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
                "this worker serves the design library but cannot render -- it was built without its `worker` feature"
            ),
            Self::WorkerSilent(elapsed) => write!(f, "worker silent for {elapsed:.0?}"),
            Self::HdrUnsupported => write!(
                f,
                "the remote cannot render HDR environments (it does not advertise HDR support)"
            ),
            Self::Asset(reason) => write!(f, "could not send the HDR map: {reason}"),
            Self::CancelUnacknowledged(waited) => {
                write!(f, "worker did not acknowledge CANCEL within {waited:.0?}")
            }
            #[cfg(feature = "zoning")]
            Self::ZoningUnsupported => write!(
                f,
                "worker has no zoning support (it does not advertise the zoning capability)"
            ),
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
    /// v16: the viewer's own float-XYZ sum for a `FinalImageRequest`'s reserved tail,
    /// ready to upload as one `CONTRIBUTION`. One-shot only -- see
    /// [`RemoteRenderHandle::contribute`].
    Contribute(Vec<Vec3>),
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
    /// The attempt failed for any reason -- connection, handshake, a transport error
    /// mid-stream, or a server `ERROR` (including a coordinator's `ALL_WORKERS_LOST`,
    /// whose message then reads "All remote workers were lost"). Corresponds to
    /// `bridge::remote::handoff::HandoffEvent::RemoteFailed`.
    Failed {
        request_id: u32,
        message: String,
    },
    /// v14: the server answered `ERROR { code: UNSUPPORTED_REQUEST }` -- it is a plain
    /// worker that does not implement what was asked (a `FinalImageRequest`, or a
    /// `TransferMode::DisplayOnly` render). Terminal for the request, but NOT a broken
    /// server: callers fall back to full-data transfer instead of treating it as a
    /// failure.
    Unsupported {
        request_id: u32,
        message: String,
    },
    /// v14: a `DISPLAY_FRAME` (a finished, denoised 8-bit picture) replaced the
    /// accumulator's `last_display_frame`; `samples_done` is what it reflects.
    DisplayFrame {
        request_id: u32,
        samples_done: u32,
    },
    /// v14: the request's `FINAL_IMAGE` is in the accumulator's `final_image`; `Done`
    /// follows.
    FinalImage {
        request_id: u32,
    },
    /// v14: the server's render capability changed since its `WELCOME` (a coordinator
    /// gained or lost joined workers). Not tied to the request; carried with the
    /// current request id only so the usual staleness gate applies.
    CapabilityChanged {
        request_id: u32,
        render: Option<indicatrix_net::messages::RenderCapability>,
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
            | Self::Failed { request_id, .. }
            | Self::Unsupported { request_id, .. }
            | Self::DisplayFrame { request_id, .. }
            | Self::FinalImage { request_id }
            | Self::CapabilityChanged { request_id, .. } => *request_id,
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
    /// Requests cancellation; the running job stops at its next check.
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

    /// Uploads `sum` (the viewer's own float-XYZ radiance sum) as one `CONTRIBUTION`
    /// for the in-flight `FinalImageRequest` -- v16, final-picture-only exports with a
    /// reserved local tail. Returns `true` once the upload is queued for the one-shot
    /// worker thread to send; `false` on a persistent connection, which never dispatches
    /// a `FinalImageRequest` with `viewer_samples > 0` in the first place (the live
    /// viewport's own transfer is a different one entirely -- see
    /// [`RemoteConnectionHandle`]'s own doc comment on why it stays one-shot-only for
    /// exports).
    #[must_use]
    pub fn contribute(&self, sum: Vec<Vec3>) -> bool {
        match &self.0 {
            HandleKind::OneShot(commands) => commands.send(RemoteCommand::Contribute(sum)).is_ok(),
            HandleKind::Connection { .. } => false,
        }
    }
}

/// Extracts the host portion of a `host:port` address (whatever follows the last `:` is
/// the port, matching `WorkerSettings::address`'s plain `host:port` convention), with the
/// brackets of an IPv6 literal removed -- the one shared rule,
/// [`indicatrix_net::tls::host_for_server_name`]. Falls back to the whole string if
/// there's no `:` at all, so a malformed address still produces some `ServerName` attempt
/// and a clear TLS-layer error rather than silently doing nothing.
#[must_use]
pub(super) fn host_from_address(address: &str) -> &str {
    indicatrix_net::tls::host_for_server_name(address).unwrap_or(address)
}

/// Everything [`super::connection::spawn_remote_render`] needs to know about the ONE
/// `RenderRequest` it will send, grouped into a struct purely to keep that function's
/// (and `run`'s) own parameter list short -- see [`WorkerSettings`]'s own doc comment
/// for why `width`/`height` travel alongside `worker` here rather than living on
/// `WorkerSettings` itself (render resolution is session-wide, not per-worker).
pub struct RemoteRenderRequest {
    /// Connection settings of the remote worker.
    pub worker: WorkerSettings,
    /// Identifier correlating replies with this request.
    pub request_id: u32,
    /// Scene state sent to the worker.
    pub scene: indicatrix_net::SceneState,
    /// Index of the first sample in this chunk.
    pub first_sample: u32,
    /// Number of samples in this chunk.
    pub samples: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Why the request is made (protocol v14): `Interactive` for the live viewport,
    /// `Batch` for exports, tilt videos and batch previews. A coordinator uses it to pick
    /// lanes; a plain worker ignores it.
    pub intent: indicatrix_net::messages::RequestIntent,
    /// Final-picture live transfer, live view only: ask for finished, denoised display frames
    /// (`TransferMode::DisplayOnly`, no preview stream) instead of float deltas. Only
    /// the persistent live connection honours it; every other caller passes `false`.
    pub display_only: bool,
}

/// Everything [`super::connection::spawn_final_image_request`] needs for ONE
/// `FinalImageRequest` ("final picture only" transfer for the still export
/// and the tilt video): the remote renders the whole range and replies with one PNG.
pub struct RemoteFinalImageRequest {
    /// The remote to connect to.
    pub worker: WorkerSettings,
    /// Epoch id, echoed on every reply.
    pub request_id: u32,
    /// The fully resolved scene; its `width`/`height` are the output size.
    pub scene: indicatrix_net::SceneState,
    /// First absolute sample index.
    pub first_sample: u32,
    /// Number of samples (the tone-mapping divisor).
    pub samples: u32,
    /// The color space the remote tone-maps into.
    pub color_space: indicatrix::color::ColorSpace,
    /// v16: the LAST `viewer_samples` of `[first_sample, first_sample + samples)`,
    /// reserved for the viewer's own local render -- `0` reproduces today's behaviour
    /// (the server plans and renders the whole range). See
    /// `indicatrix_net::messages::FinalImageRequest::viewer_samples`'s own doc comment.
    pub viewer_samples: u32,
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
    fn host_from_address_strips_ipv6_brackets() {
        assert_eq!(host_from_address("[::1]:9443"), "::1");
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

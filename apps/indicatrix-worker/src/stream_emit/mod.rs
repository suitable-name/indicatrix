//! Progressive streaming: the emitter/tracer split described in `serve`'s module
//! docs.
//!
//! This module holds every piece that's testable as pure logic, independent of any
//! actual socket: [`PendingDelta`] (delta coalescing), [`next_batch_size`] (adaptive
//! sub-batch sizing), and [`downsample_preview`] (the cumulative reduced-resolution
//! snapshot `PREVIEW` sends). [`run_stream`] wires these together with a tracer thread
//! and drives the actual `Read + Write` stream; see its own doc comment and `serve`'s
//! module docs for the full architecture.

use std::{
    io::{Read, Write},
    time::Duration,
};

mod downsample;
mod emitter;
mod sizing;
#[cfg(test)]
mod tests;
mod tracer;

pub use emitter::run_stream;

/// This worker's cadence FLOOR, advertised in `WELCOME::min_cadence_ms` -- see that
/// field's doc comment. Matches [`TARGET_SUBBATCH`], the sub-batch duration
/// [`next_batch_size`] targets: emitting faster than one sub-batch completes is not
/// meaningful, since there is nothing new to send in between.
pub const MIN_CADENCE_FLOOR_MS: u32 = 100;

/// How long `emitter::poll_for_client_message` and `crate::serve::tilt::poll_for_cancel`
/// wait for the REMAINDER of a message once its first byte has arrived, instead of
/// blocking forever. Shared here so the two independent implementations of this pattern
/// can't drift to different bounds.
///
/// Both read a message's length-prefix first byte with a raw, timeout-tolerant `read()`
/// so a timeout can only land BEFORE any byte has arrived. Once a byte arrives, the
/// remainder gets this bounded window instead of `None`; a timeout within it is a
/// protocol error (tearing down the connection), closing the gap where a peer sending a
/// partial frame and going silent used to hang the reading thread forever.
///
/// 5s is generous relative to both functions' millisecond-scale first-byte poll
/// interval: a real message (tiny -- a `CANCEL` or pipelined `RenderRequest`) completes
/// well under a second even on a slow link, leaving slack for congestion without
/// leaving a stalled connection blocking indefinitely.
pub const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(5);

/// The hard, client-`cadence_ms`-independent ceiling on how long [`run_stream`]'s
/// emitter may go without writing some `StreamEvent` (a bare `Progress` heartbeat, if
/// nothing else was due) while a request is streaming.
///
/// The editor's remote-render connection declares a worker dead after its own
/// `LIVENESS_TIMEOUT` (8s) with no `StreamEvent` received (30s grace before the first
/// event); this is a quarter of that, leaving slack for one lost heartbeat without the
/// next also missing the deadline.
///
/// Can't just be `request.stream.cadence_ms`: that's client-chosen with only a floor
/// ([`MIN_CADENCE_FLOOR_MS`]), and a large export legitimately requests a large cadence
/// for fewer, bigger payloads that could otherwise go silent past `LIVENESS_TIMEOUT`.
/// The emitter loop wakes at least this often unconditionally and heartbeats with the
/// latest `samples_done` whenever nothing else went out; a due cadence tick is never
/// delayed by this.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// True if `e` is what a stream-level timeout looks like on this crate's two real
/// transports (a raw `TcpStream`, and TLS via `rustls::StreamOwned`) -- the single place
/// both `emitter::poll_for_client_message` and `crate::serve::tilt::poll_for_cancel`
/// decide "nothing pending yet" from a genuine protocol error.
///
/// A raw socket's timeout elapsing surfaces as [`std::io::ErrorKind::WouldBlock`] or
/// [`std::io::ErrorKind::TimedOut`] depending on platform.
/// [`std::io::ErrorKind::WriteZero`] is a third, non-obvious member once TLS is
/// involved: `rustls`'s `Stream::read`/`write` drive `complete_io()` internally, and a
/// single call can require both a socket read and write under the hood. When that
/// inner I/O times out, rustls reports `WriteZero` instead of propagating the original
/// `WouldBlock`/`TimedOut` -- without treating it the same, a merely-slow-but-alive
/// peer's poll could be misclassified as a fatal protocol error.
#[must_use]
pub fn is_stream_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WriteZero
    )
}

/// A stream that can have a short read timeout applied, so [`run_stream`]'s emitter loop
/// can poll for an incoming `CANCEL` without ever blocking longer than the timeout.
///
/// Implemented for the real transports (`TcpStream`, and `rustls::StreamOwned` wrapping
/// one) by delegating to the underlying socket. A test double can implement this to
/// toggle its own "exhausted input means `WouldBlock`, not EOF" behavior -- see
/// `serve`'s `tests::DuplexHalf`.
pub trait TimeoutRead {
    /// # Errors
    ///
    /// Returns whatever the underlying transport's own timeout-setting call returns.
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()>;
}

impl TimeoutRead for std::net::TcpStream {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        Self::set_read_timeout(self, duration)
    }
}

impl<C, T: TimeoutRead + Read + Write> TimeoutRead for rustls::StreamOwned<C, T> {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(duration)
    }
}

/// Mirrors `std::io::Read`/`Write`'s blanket impls for `&mut T`, so a test can drive
/// `run_stream` through a `&mut SomeTestDouble` and still inspect it afterward.
impl<T: TimeoutRead + ?Sized> TimeoutRead for &mut T {
    fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        (**self).set_read_timeout(duration)
    }
}

/// Caches the last read-timeout value applied to a stream through
/// [`TimeoutCache::apply`], so a caller that (like [`emitter::run_stream`]'s loop) asks
/// for the same value on every poll only pays the underlying `set_read_timeout`
/// syscall once, not every time.
///
/// # Why this exists
///
/// `run_stream`'s emitter calls [`emitter::poll_for_client_message`] every
/// [`emitter::EMITTER_POLL`] (20ms), which used to call `set_read_timeout` twice per
/// call (once for the poll window, once more after the first byte of a message
/// arrives) unconditionally -- on an idle streaming connection (no `CANCEL`, no
/// pipelined `RenderRequest` ever arriving) that's ~100 setsockopt syscalls/s for no
/// behavioral benefit, since the value being set is identical to what's already on the
/// socket. [`Self::apply`] makes the second call a no-op whenever the requested value
/// hasn't changed since the last one this cache actually applied.
///
/// Owned by `run_stream` for the life of one request and passed by `&mut` into
/// `poll_for_client_message`; a fresh cache per request means a new request never
/// trusts an assumption inherited from a previous one about what's currently on the
/// socket -- see [`Self::new`].
/// Whether [`TimeoutCache`] has applied a read timeout yet, and if so, the last value
/// it applied -- a named alternative to `Option<Option<Duration>>` (flagged by
/// `clippy::option_option`, and less self-explanatory at every call site than a type
/// whose variants say what they mean).
#[derive(Debug, Default, PartialEq, Eq)]
enum LastTimeout {
    /// [`TimeoutCache::apply`] has never been called (or never succeeded) yet -- the
    /// next call always reaches the real `set_read_timeout`, whatever value it's given,
    /// rather than assuming the socket already matches some value.
    #[default]
    Unknown,
    /// The last value successfully applied -- itself `Option<Duration>` (`None` is a
    /// meaningful, cacheable value here: "no timeout").
    Applied(Option<Duration>),
}

#[derive(Debug, Default)]
pub struct TimeoutCache {
    last: LastTimeout,
}

impl TimeoutCache {
    /// A fresh cache with nothing recorded yet -- the next [`Self::apply`] call always
    /// reaches the real `set_read_timeout`, whatever value it's given.
    pub(super) const fn new() -> Self {
        Self {
            last: LastTimeout::Unknown,
        }
    }

    /// Applies `duration` as `stream`'s read timeout via [`TimeoutRead::set_read_timeout`],
    /// skipping the call entirely if `duration` is exactly what this cache last applied
    /// successfully.
    ///
    /// A failed `set_read_timeout` call does NOT update the cached value: whatever was
    /// last successfully applied is presumably still what the socket actually has, so a
    /// later retry of the same `duration` correctly tries again rather than being
    /// skipped as a false no-op.
    pub(super) fn apply<S: TimeoutRead + ?Sized>(
        &mut self,
        stream: &mut S,
        duration: Option<Duration>,
    ) -> std::io::Result<()> {
        if self.last == LastTimeout::Applied(duration) {
            return Ok(());
        }
        stream.set_read_timeout(duration)?;
        self.last = LastTimeout::Applied(duration);
        Ok(())
    }
}

/// A stream that can have a write timeout applied, so [`run_stream`]'s emitter can never
/// block on a single `write()` call for longer than [`emitter::WRITE_TIMEOUT`] --
/// confirmed root cause of two real-world reports (the worker appearing to "stop sending
/// data" mid-render, and a `CANCEL` not taking effect promptly). [`run_stream`]'s
/// emitter both writes payloads and polls for `CANCEL` sequentially in one loop;
/// without a write timeout, a peer whose read loop falls behind blocks that thread
/// indefinitely. A bounded timeout turns that into "blocks for at most one timeout,
/// then this connection ends" -- the only safe option, since a `write_all` that has
/// already pushed part of a length-prefixed frame cannot be abandoned mid-message
/// without desyncing the peer's framing. Implemented the same way as [`TimeoutRead`].
pub trait TimeoutWrite {
    /// # Errors
    ///
    /// Returns whatever the underlying transport's own timeout-setting call returns.
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()>;
}

impl TimeoutWrite for std::net::TcpStream {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        Self::set_write_timeout(self, duration)
    }
}

impl<C, T: TimeoutWrite + Read + Write> TimeoutWrite for rustls::StreamOwned<C, T> {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_write_timeout(duration)
    }
}

impl<T: TimeoutWrite + ?Sized> TimeoutWrite for &mut T {
    fn set_write_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
        (**self).set_write_timeout(duration)
    }
}

/// How [`run_stream`] ended, for `handle_connection`'s caller to decide what (if
/// anything) still needs to be written. `run_stream` has already sent every
/// `StreamEvent` for [`Completed`](StreamOutcome::Completed) (normal finish or
/// cancellation both produce a `DONE`, just with a different `cancelled` flag); only
/// [`TracePanicked`](StreamOutcome::TracePanicked) requires the caller to send
/// `StreamEvent::Error` itself. Only half of what [`run_stream`] returns -- see its own
/// doc comment for the `Option<RenderRequest>` alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The request ran to completion (`DONE { cancelled: false }`) or was cancelled
    /// (`DONE { cancelled: true }`) -- either way, every [`StreamEvent`] this request
    /// will ever produce has already been written.
    Completed,
    /// `indicatrix`'s tracer panicked on this (validation-passing but pathological) scene.
    /// Nothing beyond whatever `FRAME`/`PREVIEW`/`PROGRESS` had already gone out before
    /// the panic has been written -- no `DONE`, since there is no valid outcome to
    /// report. The caller is expected to send `StreamEvent::Error` itself.
    TracePanicked,
}

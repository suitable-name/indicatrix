//! A coordinator viewer connection between requests: waiting for the next message while
//! telling the viewer about capacity changes (`CAPABILITY_CHANGED`, v14 -- sent between
//! requests, never inside one).

use super::{Capacity, ViewerSession, viewer_render_capability};
use crate::stream_emit::{FRAME_REMAINDER_TIMEOUT, TimeoutRead, is_stream_timeout};
use indicatrix_net::{
    framing::{FramingError, IDLE_READ_TIMEOUT, LEN_PREFIX_BYTES, MAX_CONTROL_FRAME_LEN},
    messages::{ClientMessage, NetError, RenderCapability, StreamEvent},
};
use std::{
    io::{Read, Write},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// How often an idle viewer connection looks for capacity changes.
const IDLE_POLL: Duration = Duration::from_millis(250);

/// Watches the registry's capacity for one viewer connection and re-advertises its
/// render capability when it changes.
pub struct CapabilityWatch {
    changes: mpsc::Receiver<Capacity>,
    registry: Arc<super::Registry>,
    own: Option<RenderCapability>,
    advertised: Option<RenderCapability>,
    /// The coordinator keeps an HDR asset cache (it holds HDR jobs' maps for joined
    /// workers).
    holds_assets: bool,
}

impl CapabilityWatch {
    /// A watch for `session`, or `None` when the coordinator has no worker port (its
    /// capability can never change).
    #[must_use]
    pub fn new(session: &ViewerSession) -> Option<Self> {
        let registry = Arc::clone(session.coordinator.registry()?);
        Some(Self {
            changes: registry.subscribe(),
            registry,
            own: session.own_capability.clone(),
            advertised: session.advertised.clone(),
            holds_assets: session.coordinator.assets().is_some(),
        })
    }

    /// Drains pending capacity notifications and, if what `WELCOME.render` would say now
    /// differs from what the viewer was last told, sends `CAPABILITY_CHANGED`.
    ///
    /// # Errors
    ///
    /// [`NetError`] if the write fails.
    pub fn flush<S: Write>(&mut self, stream: &mut S) -> Result<(), NetError> {
        let mut changed = false;
        while self.changes.try_recv().is_ok() {
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        let render = viewer_render_capability(
            self.own.as_ref(),
            Some(self.registry.capacity()),
            self.holds_assets,
        );
        if render == self.advertised {
            return Ok(());
        }
        tracing::debug!("coordinator: telling a viewer its render capability is now {render:?}");
        indicatrix_net::messages::write_stream_event(
            stream,
            &StreamEvent::CapabilityChanged {
                render: render.clone(),
            },
            None,
        )?;
        self.advertised = render;
        Ok(())
    }
}

/// Reads the viewer's next message, sending capability changes while it waits (see
/// [`CapabilityWatch::flush`]). `Ok(None)` on a clean close at a message boundary.
///
/// The first byte is awaited with a short, timeout-tolerant read so a timeout can only
/// land before a message starts; the rest is read under [`FRAME_REMAINDER_TIMEOUT`], and
/// is bounded by [`MAX_CONTROL_FRAME_LEN`]. The read timeout is back to blocking when
/// this returns.
///
/// # Errors
///
/// [`NetError`] for a transport or decoding failure, including a connection that ends
/// inside a frame. A viewer that sends nothing for [`IDLE_READ_TIMEOUT`] yields a
/// `TimedOut` I/O error, which the caller treats as an idle close.
pub fn read_message_watching<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    watch: &mut CapabilityWatch,
) -> Result<Option<ClientMessage>, NetError> {
    let result = read_watching(stream, watch);
    let _ = stream.set_read_timeout(None);
    result
}

fn read_watching<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    watch: &mut CapabilityWatch,
) -> Result<Option<ClientMessage>, NetError> {
    let io = |e: std::io::Error| NetError::Framing(FramingError::Io(e));
    stream.set_read_timeout(Some(IDLE_POLL)).map_err(io)?;
    let waiting_since = Instant::now();
    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    let first = loop {
        watch.flush(stream)?;
        if waiting_since.elapsed() >= IDLE_READ_TIMEOUT {
            return Err(io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "idle timeout: the viewer sent no message",
            )));
        }
        match stream.read(&mut len_bytes) {
            Ok(0) => return Ok(None),
            Ok(n) => break n,
            Err(e) if is_stream_timeout(&e) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io(e)),
        }
    };
    stream
        .set_read_timeout(Some(FRAME_REMAINDER_TIMEOUT))
        .map_err(io)?;
    let payload = indicatrix_net::framing::read_frame_continuing(
        stream,
        &len_bytes[..first],
        MAX_CONTROL_FRAME_LEN,
    )?;
    Ok(Some(indicatrix_net::messages::decode_control_frame(
        &payload,
    )?))
}

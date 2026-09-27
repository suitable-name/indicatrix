//! A coordinator viewer connection between requests: waiting for the next message while
//! telling the viewer about capacity changes (`CAPABILITY_CHANGED`, v14 -- sent between
//! requests, never inside one).

use super::{Capacity, ViewerSession, viewer_render_capability};
use crate::stream_emit::{FRAME_REMAINDER_TIMEOUT, TimeoutRead, is_stream_timeout};
use indicatrix_net::{
    framing::{FramingError, LEN_PREFIX_BYTES, MAX_FRAME_LEN},
    messages::{ClientMessage, NetError, RenderCapability, StreamEvent},
};
use std::{
    io::{Read, Write},
    sync::{Arc, mpsc},
    time::Duration,
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
/// [`CapabilityWatch::flush`]). `Ok(None)` on a clean close.
///
/// The first byte is awaited with a short, timeout-tolerant read so a timeout can only
/// land before a message starts; the rest is read under [`FRAME_REMAINDER_TIMEOUT`].
/// The read timeout is back to blocking when this returns.
///
/// # Errors
///
/// [`NetError`] for a transport or decoding failure.
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
    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    let first = loop {
        watch.flush(stream)?;
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
    stream.read_exact(&mut len_bytes[first..]).map_err(io)?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(NetError::Framing(FramingError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        }));
    }
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload).map_err(io)?;
    Ok(Some(postcard::from_bytes(&payload)?))
}

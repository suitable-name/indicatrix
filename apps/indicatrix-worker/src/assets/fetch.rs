//! [`ensure_environment`]: resolving the HDR map a request names before it is served --
//! from the decoded registry, else the on-disk cache, else by asking the client.
//! [`ensure_held`] is the coordinator's variant: it makes sure the coordinator
//! HOLDS the bytes for the job (to answer its joined workers' `NEED_ASSET`), asking the
//! viewer only when its own cache lacks them, and decodes only for its own lane.
//!
//! ```text
//! <- NEED_ASSET { content_hash }
//! -> ASSET { content_hash, len } + payload     (within ASSET_WAIT; hash verified)
//!    ... decode (PROGRESS heartbeats every 2 s while it runs), cache, serve the request
//! ```
//!
//! While waiting, a `PING` is answered, a `CANCEL` of this request or a pipelined
//! `RenderRequest` ends the wait with `DONE { cancelled: true }` (exactly like a
//! streaming request), and anything else is ignored as it would be mid-stream.

use super::{AssetCache, HeldAsset, decoded};
use crate::stream_emit::{self, RawPoll, TimeoutRead, TimeoutWrite};
use indicatrix::renderer::env_map::EnvironmentMap;
use indicatrix_net::{
    framing::FramingError,
    messages::{
        AssetError, ClientMessage, Done, ErrorMsg, NetError, Progress, RenderRequest, Stats,
        StreamEvent, error_codes, hash_hex, read_asset_payload,
    },
    scene::HdrEnvironment,
};
use std::{
    io::{Read, Write},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// How long the client has to START sending a requested asset.
pub const ASSET_WAIT: Duration = Duration::from_secs(60);

/// Per-read deadline while an asset payload is arriving (a stall this long mid-payload
/// ends the connection).
const PAYLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounds every write this module makes (heartbeats, `NEED_ASSET`, `DONE`).
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the wait loop polls for the client's next message.
const POLL: Duration = Duration::from_millis(100);

/// What resolving a request's HDR map came to.
pub enum Fetched<T = Arc<EnvironmentMap>> {
    /// Resolved. For [`ensure_environment`] the map is decoded and pinned: hold this
    /// `Arc` until the request has been served; for [`ensure_held`] the job's
    /// [`HeldAsset`].
    Ready(T),
    /// The asset could not be obtained: send this error for the request and carry on.
    Failed(ErrorMsg),
    /// The client cancelled the request (or pipelined the next one) while it waited;
    /// `DONE { cancelled: true }` was already sent. Serve the pipelined request next.
    Superseded(Option<Box<RenderRequest>>),
    /// The client closed the connection.
    Closed,
}

impl<T> Fetched<T> {
    /// `Ok` with the ready value, else the same outcome for another ready type.
    fn into_ready<U>(self) -> Result<T, Fetched<U>> {
        match self {
            Self::Ready(value) => Ok(value),
            Self::Failed(error) => Err(Fetched::Failed(error)),
            Self::Superseded(next) => Err(Fetched::Superseded(next)),
            Self::Closed => Err(Fetched::Closed),
        }
    }
}

/// The request `ensure_environment` resolves a map for.
#[derive(Debug, Clone, Copy)]
pub struct Pending<'a> {
    /// The request's id (for `PROGRESS`/`DONE`).
    pub request_id: u32,
    /// The request's `StreamConfig::cadence_ms` (echoed in a cancelled `DONE`).
    pub cadence_ms: u32,
    /// The map the scene names.
    pub hdr: &'a HdrEnvironment,
}

/// Resolves `pending.hdr` -- see the module doc comment. Restores blocking reads and no
/// write timeout before returning.
///
/// # Errors
///
/// [`NetError`] for a transport failure, or an asset payload that cannot be read in
/// sync (over `MAX_ASSET_LEN`, truncated): the connection must end.
pub fn ensure_environment<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    cache: &AssetCache,
    pending: Pending<'_>,
) -> Result<Fetched, NetError> {
    let hash = pending.hdr.content_hash;
    // The decoded registry is process-wide; only a hash this node obtained itself may
    // skip the protocol (see `AssetCache::has_held`).
    if cache.has_held(&hash)
        && let Some(map) = decoded::lookup(&hash)
    {
        return Ok(Fetched::Ready(map));
    }
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let result = from_cache_or_client(stream, cache, pending);
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
    result
}

/// The on-disk cache's copy if it has one, else the client's -- decoded.
fn from_cache_or_client<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    cache: &AssetCache,
    pending: Pending<'_>,
) -> Result<Fetched, NetError> {
    let bytes = match cache.get(&pending.hdr.content_hash) {
        Some(bytes) => bytes,
        None => match request_from_client(stream, cache, pending)?.into_ready() {
            Ok(bytes) => bytes,
            Err(other) => return Ok(other),
        },
    };
    decode_with_heartbeats(stream, pending, Arc::new(bytes))
}

/// The coordinator's resolve: holds the bytes `pending.hdr` names for the job.
///
/// They are already in `cache` (not read now), else asked of the viewer exactly as
/// [`ensure_environment`] does. With `decode` (the own lane renders) the decoded map is
/// resolved too. Restores blocking reads and no write timeout before returning.
///
/// # Errors
///
/// As [`ensure_environment`].
pub fn ensure_held<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    cache: &Arc<AssetCache>,
    pending: Pending<'_>,
    decode: bool,
) -> Result<Fetched<Arc<HeldAsset>>, NetError> {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let result = hold(stream, cache, pending, decode);
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);
    result
}

/// [`ensure_held`] with the stream's timeouts armed.
///
/// Always resolves and pins the bytes in memory for the job's lifetime (see
/// [`HeldAsset`]), rather than trusting the on-disk cache to still have them whenever a
/// joined worker later asks for them: an LRU eviction (another job's larger map, a
/// write failure) between now and then must never turn "the coordinator holds this
/// job's map" into "the coordinator lost it," discarding an otherwise healthy worker
/// connection.
fn hold<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    cache: &Arc<AssetCache>,
    pending: Pending<'_>,
    decode: bool,
) -> Result<Fetched<Arc<HeldAsset>>, NetError> {
    let hash = pending.hdr.content_hash;
    let received: Arc<Vec<u8>> = match cache.get(&hash) {
        Some(bytes) => Arc::new(bytes),
        None => match request_from_client(stream, cache, pending)?.into_ready() {
            Ok(bytes) => Arc::new(bytes),
            Err(other) => return Ok(other),
        },
    };
    let map = if decode {
        match decode_held(stream, cache, pending, Some(&received))?.into_ready() {
            Ok(map) => Some(map),
            Err(other) => return Ok(other),
        }
    } else {
        None
    };
    Ok(Fetched::Ready(Arc::new(HeldAsset::new(
        *pending.hdr,
        Arc::clone(cache),
        Some(received),
        map,
    ))))
}

/// The decoded map for a coordinator's own lane: already decoded in this process, else
/// decoded from `received` (just sent by the viewer) or the cache's copy.
fn decode_held<S: Write>(
    stream: &mut S,
    cache: &AssetCache,
    pending: Pending<'_>,
    received: Option<&Arc<Vec<u8>>>,
) -> Result<Fetched, NetError> {
    let hash = pending.hdr.content_hash;
    if let Some(map) = decoded::lookup(&hash) {
        return Ok(Fetched::Ready(map));
    }
    let bytes = received.cloned().or_else(|| cache.get(&hash).map(Arc::new));
    let Some(bytes) = bytes else {
        return Ok(Fetched::Failed(asset_failed(
            format!(
                "HDR map {}: the coordinator's asset cache lost it; send the request again",
                hash_hex(&hash)
            ),
            pending.request_id,
        )));
    };
    decode_with_heartbeats(stream, pending, bytes)
}

/// Sends `NEED_ASSET` and waits for the bytes (see the module doc comment); verified
/// bytes are also stored in `cache`.
fn request_from_client<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    cache: &AssetCache,
    pending: Pending<'_>,
) -> Result<Fetched<Vec<u8>>, NetError> {
    let hash = pending.hdr.content_hash;
    tracing::info!(
        "request {}: asking the client for HDR map {}",
        pending.request_id,
        hash_hex(&hash)
    );
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::NeedAsset { content_hash: hash },
        None,
    )?;
    let started = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut timeouts = stream_emit::TimeoutCache::new();
    loop {
        let message = match stream_emit::poll_raw_client_message(stream, POLL, &mut timeouts)? {
            RawPoll::Closed => return Ok(Fetched::Closed),
            RawPoll::Pending => {
                if started.elapsed() > ASSET_WAIT {
                    return Ok(Fetched::Failed(asset_failed(
                        format!(
                            "the client did not send HDR map {} within {} s",
                            hash_hex(&hash),
                            ASSET_WAIT.as_secs()
                        ),
                        pending.request_id,
                    )));
                }
                if last_heartbeat.elapsed() >= stream_emit::HEARTBEAT_INTERVAL {
                    write_heartbeat(stream, pending.request_id)?;
                    last_heartbeat = Instant::now();
                }
                continue;
            }
            RawPoll::Message(message) => message,
        };
        match message {
            ClientMessage::Asset(header) if header.content_hash == hash => {
                let _ = stream.set_read_timeout(Some(PAYLOAD_READ_TIMEOUT));
                let bytes = match read_asset_payload(stream, &header) {
                    Ok(bytes) => bytes,
                    Err(e @ (AssetError::HashMismatch | AssetError::LengthMismatch { .. })) => {
                        return Ok(Fetched::Failed(asset_failed(
                            format!("HDR map {}: {e}", hash_hex(&hash)),
                            pending.request_id,
                        )));
                    }
                    Err(AssetError::TooLarge { len }) => {
                        return Err(NetError::Framing(FramingError::FrameTooLarge {
                            len,
                            max: indicatrix_net::messages::MAX_ASSET_LEN,
                        }));
                    }
                    Err(AssetError::Framing(e)) => return Err(NetError::Framing(e)),
                };
                if let Err(e) = cache.put(&hash, &bytes) {
                    tracing::warn!("asset cache: not storing {}: {e}", hash_hex(&hash));
                }
                return Ok(Fetched::Ready(bytes));
            }
            ClientMessage::Asset(other) => {
                // Not the asset this request is waiting for: consume it to stay in sync.
                discard_asset(stream, &other)?;
            }
            ClientMessage::Ping { nonce } => indicatrix_net::messages::write_stream_event(
                stream,
                &StreamEvent::Pong { nonce },
                None,
            )?,
            ClientMessage::Cancel(cancel) if cancel.request_id == pending.request_id => {
                write_cancelled_done(stream, pending)?;
                return Ok(Fetched::Superseded(None));
            }
            ClientMessage::RenderRequest(next) => {
                write_cancelled_done(stream, pending)?;
                return Ok(Fetched::Superseded(Some(next)));
            }
            // A stray v16 contribution while this request waits for its HDR map:
            // consume its payload frame to keep the stream in sync (the `other` arm
            // below never reads payloads, so it must never see this variant).
            ClientMessage::Contribution(header) => {
                indicatrix_net::messages::discard_contribution_payload(stream, &header)?;
            }
            other => tracing::debug!(
                "request {}: ignoring {other:?} while waiting for its HDR map",
                pending.request_id
            ),
        }
    }
}

/// Consumes and drops the payload of an `ASSET` nobody asked for (keeping the stream in
/// sync), streaming it to a sink without buffering or hashing it; a mismatching length is
/// harmless here.
///
/// # Errors
///
/// [`NetError`] when the payload cannot be read in sync.
pub fn discard_asset<S: Read>(
    stream: &mut S,
    header: &indicatrix_net::messages::AssetHeader,
) -> Result<(), NetError> {
    tracing::debug!(
        "discarding an unrequested asset {} ({} bytes)",
        hash_hex(&header.content_hash),
        header.len
    );
    match indicatrix_net::messages::discard_asset_payload(stream, header) {
        Ok(()) | Err(AssetError::HashMismatch | AssetError::LengthMismatch { .. }) => Ok(()),
        Err(AssetError::TooLarge { len }) => Err(NetError::Framing(FramingError::FrameTooLarge {
            len,
            max: indicatrix_net::messages::MAX_ASSET_LEN,
        })),
        Err(AssetError::Framing(e)) => Err(NetError::Framing(e)),
    }
}

/// Decodes `bytes` on a helper thread (see `decoded::decode_and_register`), writing a
/// `PROGRESS` heartbeat every `HEARTBEAT_INTERVAL` meanwhile so the client's liveness
/// watchdog sees a large decode as progress, not silence.
fn decode_with_heartbeats<S: Write>(
    stream: &mut S,
    pending: Pending<'_>,
    bytes: Arc<Vec<u8>>,
) -> Result<Fetched, NetError> {
    let hdr = *pending.hdr;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(decoded::decode_and_register(&hdr, &bytes));
    });
    loop {
        match rx.recv_timeout(stream_emit::HEARTBEAT_INTERVAL) {
            Ok(Ok(map)) => return Ok(Fetched::Ready(map)),
            Ok(Err(reason)) => {
                return Ok(Fetched::Failed(asset_failed(
                    format!(
                        "HDR map {} does not decode: {reason}",
                        hash_hex(&hdr.content_hash)
                    ),
                    pending.request_id,
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => write_heartbeat(stream, pending.request_id)?,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ok(Fetched::Failed(asset_failed(
                    "decoding the HDR map panicked".to_string(),
                    pending.request_id,
                )));
            }
        }
    }
}

/// A bare `PROGRESS { samples_done: 0 }`: proof of life before any sample exists.
fn write_heartbeat<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Progress(Progress {
            request_id,
            samples_done: 0,
        }),
        None,
    )
}

/// `DONE { cancelled: true }` for a request cancelled before it traced anything.
fn write_cancelled_done<S: Write>(stream: &mut S, pending: Pending<'_>) -> Result<(), NetError> {
    indicatrix_net::messages::write_stream_event(
        stream,
        &StreamEvent::Done(Done {
            request_id: pending.request_id,
            cancelled: true,
            stats: Stats {
                samples_done: 0,
                requested_cadence_ms: pending.cadence_ms,
                effective_cadence_ms: 0,
                reclaimed_samples: 0,
            },
        }),
        None,
    )
}

/// An `ASSET_FAILED` error carrying `message` (logged), naming the request it concerns
/// (v15) so a late failure can't fail whatever request the client has since moved on to.
fn asset_failed(message: String, request_id: u32) -> ErrorMsg {
    tracing::info!("{message}");
    ErrorMsg {
        code: error_codes::ASSET_FAILED,
        message,
        request_id: Some(request_id),
    }
}

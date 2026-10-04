//! Wires the write and read sides of one render session together.
//!
//! The write side (`RenderRequest`/`CANCEL`) and the read side (the [`StreamEvent`]
//! reply stream, driven through an [`Accumulator`]).
//!
//! # Why this is two independent halves, not one call
//!
//! [`run_client_session`] blocks reading [`StreamEvent`]s for as long as the connection
//! stays open, while [`send_cancel`]/[`send_render_request`] need to be callable from
//! elsewhere (a UI thread reacting to the user dragging the stone again) at any time,
//! including mid-blocking-read. A real `TcpStream` supports this split via
//! `TcpStream::try_clone` -- one clone goes to [`run_client_session`] as the reader, the
//! other stays with whoever decides when to cancel or pipeline the next request. This
//! module doesn't assume a socket (both halves are generic over `Read`/`Write`
//! independently), but is written to compose that way.
//!
//! [`run_client_session`] takes the [`Accumulator`] by `&mut` rather than owning it: the
//! caller also needs `Accumulator::begin_request` to run synchronously, from whichever
//! thread sends the request, exactly once per `RenderRequest` -- at send time, not at
//! first-reply time (see [`Accumulator::begin_request`]'s doc comment).

use super::{
    ClientError,
    accumulate::{Accumulator, ApplyOutcome},
};
use crate::messages::{self, Cancel, ClientMessage, ErrorMsg, Stats, StreamEvent};
use std::io::{Read, Write};

/// Sends a `RenderRequest` -- the client's `-> RENDER`.
///
/// Callers MUST call [`Accumulator::begin_request`] with `request.request_id` before or
/// immediately after this returns, before any reply for it can possibly be processed --
/// the accumulator may be owned by a different thread than the one sending the request
/// (see the module doc comment).
///
/// Wraps `request` in the tagged [`ClientMessage::RenderRequest`] envelope; callers
/// never construct that by hand. Only compiled under this crate's `render` feature --
/// check `crate::messages::Welcome::render` before calling.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_render_request<W: Write>(
    writer: &mut W,
    request: &crate::messages::RenderRequest,
) -> Result<(), ClientError> {
    messages::write_message(
        writer,
        &ClientMessage::RenderRequest(Box::new(request.clone())),
    )?;
    Ok(())
}

/// Sends a `PING` (v14).
///
/// The answer arrives as [`StreamEvent::Pong`] with the same `nonce`, reported by
/// [`run_client_session`] as [`SessionUpdate::Pong`]. Only compiled under this crate's
/// `render` feature, like the variant itself.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_ping<W: Write>(writer: &mut W, nonce: u64) -> Result<(), ClientError> {
    messages::write_message(writer, &ClientMessage::Ping { nonce })?;
    Ok(())
}

/// Sends a `FINAL_IMAGE_REQUEST` (v14).
///
/// See `crate::messages::final_image` for the reply sequence. Call
/// [`Accumulator::begin_request`] with its `request_id` first, as for a `RenderRequest`;
/// the picture lands in [`Accumulator::final_image`]. Only compiled under this crate's
/// `render` feature.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_final_image_request<W: Write>(
    writer: &mut W,
    request: &crate::messages::FinalImageRequest,
) -> Result<(), ClientError> {
    messages::write_message(
        writer,
        &ClientMessage::FinalImageRequest(Box::new(request.clone())),
    )?;
    Ok(())
}

/// Sends the asset a server asked for with `NEED_ASSET` (v14, HDR environment maps).
///
/// The `ASSET` header, then `bytes` as one raw frame (see `crate::messages::asset`).
/// `bytes` must be exactly the asset the server named -- its SHA-256 is recomputed here
/// and sent, and a server refuses bytes whose hash is not the one it asked for.
/// `render`-feature only.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if `bytes` exceeds `crate::messages::MAX_ASSET_LEN` or
/// writing fails.
#[cfg(feature = "render")]
pub fn send_asset<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<(), ClientError> {
    messages::write_asset_message(writer, bytes)?;
    Ok(())
}

/// Sends a `CONTRIBUTION` (v16): the viewer's own share of a `FinalImageRequest`'s
/// reserved tail, `sum` being the float-XYZ radiance sum of exactly `range.1` samples
/// starting at `range.0`.
///
/// `range` is `(first_sample, samples)` -- the same shape
/// `FinalImageRequest::reserved_range` returns -- collapsed into one parameter to keep
/// this function's arity under clippy's default limit.
///
/// A thin wrapper over `crate::messages::write_contribution_message`. `render`-feature
/// only, like the variant itself.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_contribution<W: Write>(
    writer: &mut W,
    request_id: u32,
    range: (u32, u32),
    width: u32,
    height: u32,
    sum: &[glam::Vec3],
    encoder: &mut crate::radiance::PayloadEncoder,
) -> Result<(), ClientError> {
    messages::write_contribution_message(writer, request_id, range, width, height, sum, encoder)?;
    Ok(())
}

/// [`send_contribution`] through the connection's [`messages::adaptive::PeerLink`].
///
/// The payload is encoded for the measured link speed and the blocking write is timed, so
/// the peer's last known bandwidth carries over to the next connection.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_contribution_with_link<W: Write>(
    writer: &mut W,
    request_id: u32,
    range: (u32, u32),
    size: (u32, u32),
    sum: &[glam::Vec3],
    link: &messages::adaptive::PeerLink,
) -> Result<(), ClientError> {
    messages::write_contribution_message_with_link(
        writer, request_id, range, size.0, size.1, sum, link,
    )?;
    Ok(())
}

/// Sends a `TILT_CURVES` request -- the client's `-> TiltCurvesRequest(...)`.
///
/// Wraps `request` in the tagged [`ClientMessage::TiltCurvesRequest`] envelope, the way
/// [`send_render_request`] wraps a `RenderRequest`. Only compiled under this crate's
/// `render` feature -- check `crate::messages::Welcome::tilt_curves` before calling.
///
/// The reply is a SINGLE [`crate::messages::TiltCurvesResponse`], read back with
/// [`recv_tilt_curves_response`], never through [`run_client_session`]'s [`StreamEvent`]
/// loop (scoped to `RENDER`'s progressive-reply shape). Mirrors
/// [`send_library_request`]/[`crate::library::LibraryResponse`], not
/// [`send_render_request`]/[`StreamEvent`].
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
#[cfg(feature = "render")]
pub fn send_tilt_curves_request<W: Write>(
    writer: &mut W,
    request: &crate::messages::TiltCurvesRequest,
) -> Result<(), ClientError> {
    messages::write_message(
        writer,
        &ClientMessage::TiltCurvesRequest(Box::new(request.clone())),
    )?;
    Ok(())
}

/// Reads the single reply to a [`send_tilt_curves_request`] call.
///
/// One blocking read of exactly one [`crate::messages::TiltCurvesResponse`]. A plain,
/// un-epoch-gated read, matching how a [`crate::library::LibraryResponse`] is read:
/// `TILT_CURVES` supports neither pipelining nor cancel-then-retry on one connection in
/// this phase, so there is no [`Accumulator`] epoch to gate against.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if reading or decoding fails.
#[cfg(feature = "render")]
pub fn recv_tilt_curves_response<R: Read>(
    reader: &mut R,
) -> Result<crate::messages::TiltCurvesResponse, ClientError> {
    Ok(messages::read_control_message(reader)?)
}

/// Sends a [`crate::library::LibraryRequest`] -- the client's `-> Library(...)`.
///
/// Always available, regardless of this crate's `render` feature. A reply arrives as a
/// single [`crate::library::LibraryResponse`] via `crate::messages::read_message`, not
/// through [`run_client_session`]'s `StreamEvent` loop -- library requests are
/// request/response, never streamed.
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
pub fn send_library_request<W: Write>(
    writer: &mut W,
    request: &crate::library::LibraryRequest,
) -> Result<(), ClientError> {
    messages::write_message(writer, &ClientMessage::Library(Box::new(request.clone())))?;
    Ok(())
}

/// Sends a `CANCEL` for `request_id` -- the client's `-> CANCEL`.
///
/// Does not touch any [`Accumulator`]; it naturally stops accepting payload for this
/// epoch once [`Accumulator::begin_request`] is next called for a different id, which
/// is the caller's responsibility, same as after [`send_render_request`].
///
/// # Errors
///
/// Returns [`ClientError::Net`] if writing fails.
pub fn send_cancel<W: Write>(writer: &mut W, request_id: u32) -> Result<(), ClientError> {
    messages::write_message(writer, &ClientMessage::Cancel(Cancel { request_id }))?;
    Ok(())
}

/// One [`StreamEvent`] processed by [`run_client_session`].
///
/// Carries enough context (the event's `request_id`, where it has one) for a caller to
/// decide what to do -- update a progress bar, redraw a preview, surface an error toast
/// -- without separately tracking epochs itself; [`Accumulator`] has already done that
/// gating by the time this reaches the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    /// A `FRAME` delta for the current epoch was summed into the accumulator.
    /// [`Accumulator::buffer`] already reflects it.
    Frame { request_id: u32, samples_done: u32 },
    /// A `PREVIEW` snapshot for the current epoch replaced
    /// [`Accumulator::last_preview`]. Display-only -- see that method's doc comment.
    Preview { request_id: u32 },
    /// A `PROGRESS` ping for the current epoch.
    Progress { request_id: u32, samples_done: u32 },
    /// `DONE` for the current epoch.
    Done {
        request_id: u32,
        cancelled: bool,
        stats: Stats,
    },
    /// The worker rejected the request outright -- never epoch-gated, since `ERROR`
    /// carries no `request_id` of its own.
    WorkerError(ErrorMsg),
    /// A payload arrived for a `request_id` that no longer matches the accumulator's
    /// current epoch -- correctly dropped. Surfaced only so a caller can log it at
    /// debug level; no action is expected.
    StaleDropped { request_id: u32 },
    /// v14: a `DISPLAY_FRAME` for the current epoch replaced
    /// [`Accumulator::last_display_frame`].
    DisplayFrame { request_id: u32 },
    /// v14: the current epoch's `FINAL_IMAGE` is in [`Accumulator::final_image`].
    FinalImage { request_id: u32 },
    /// v14: a `PONG` answering the `PING` with this `nonce`.
    Pong { nonce: u64 },
    /// v14: the server's render capability changed since `WELCOME`.
    CapabilityChanged {
        render: Option<crate::messages::RenderCapability>,
    },
    /// v14: the server needs the asset with this SHA-256 before it proceeds --
    /// answer with [`send_asset`] on the write side.
    NeedAsset { content_hash: [u8; 32] },
}

fn to_update(event: &StreamEvent, outcome: ApplyOutcome) -> SessionUpdate {
    match outcome {
        ApplyOutcome::FrameSummed { samples_done } => {
            let StreamEvent::Frame(h) = event else {
                unreachable!("FrameSummed only ever comes from applying a Frame event")
            };
            SessionUpdate::Frame {
                request_id: h.request_id,
                samples_done,
            }
        }
        ApplyOutcome::PreviewReplaced => {
            let StreamEvent::Preview(h) = event else {
                unreachable!("PreviewReplaced only ever comes from applying a Preview event")
            };
            SessionUpdate::Preview {
                request_id: h.request_id,
            }
        }
        ApplyOutcome::Progress { samples_done } => {
            let StreamEvent::Progress(p) = event else {
                unreachable!("Progress outcome only ever comes from applying a Progress event")
            };
            SessionUpdate::Progress {
                request_id: p.request_id,
                samples_done,
            }
        }
        ApplyOutcome::Done { cancelled } => {
            let StreamEvent::Done(d) = event else {
                unreachable!("Done outcome only ever comes from applying a Done event")
            };
            SessionUpdate::Done {
                request_id: d.request_id,
                cancelled,
                stats: d.stats,
            }
        }
        ApplyOutcome::WorkerError => {
            let StreamEvent::Error(e) = event else {
                unreachable!("WorkerError outcome only ever comes from applying an Error event")
            };
            SessionUpdate::WorkerError(e.clone())
        }
        ApplyOutcome::StaleDropped => {
            let Some(request_id) = event.request_id() else {
                unreachable!("request-less events are never epoch-gated, so never StaleDropped")
            };
            SessionUpdate::StaleDropped { request_id }
        }
        ApplyOutcome::DisplayFrameReplaced | ApplyOutcome::FinalImageReceived => {
            let request_id = event
                .request_id()
                .expect("picture events always carry a request_id");
            if outcome == ApplyOutcome::DisplayFrameReplaced {
                SessionUpdate::DisplayFrame { request_id }
            } else {
                SessionUpdate::FinalImage { request_id }
            }
        }
        ApplyOutcome::Pong { nonce } => SessionUpdate::Pong { nonce },
        ApplyOutcome::NeedAsset { content_hash } => SessionUpdate::NeedAsset { content_hash },
        ApplyOutcome::CapabilityChanged => {
            let StreamEvent::CapabilityChanged { render } = event else {
                unreachable!("CapabilityChanged only ever comes from a CapabilityChanged event")
            };
            SessionUpdate::CapabilityChanged {
                render: render.clone(),
            }
        }
    }
}

/// Drives `accumulator` from whatever `reader` sends for as long as the connection
/// stays open.
///
/// Runs across as many pipelined `RenderRequest`s as the caller sends on the write side.
/// Every [`StreamEvent`] read is applied via [`Accumulator::apply`] and reported to
/// `on_update`.
///
/// Returns `Ok(())` on a clean EOF at a frame boundary -- the normal way this loop ends,
/// treating "peer closed the connection" as done, not a failure. A connection that ends
/// inside a frame is an error ([`crate::framing::FramingError::TruncatedFrame`]). There is deliberately no other way to
/// stop this loop from the inside: a blocking read can't notice an out-of-band "please
/// stop" short of the connection closing, so a caller that wants to stop early shuts
/// down the underlying stream from another thread (e.g. `TcpStream::shutdown`), which
/// unblocks the read with an I/O error (an `Err`, not `Ok(())`) that a caller doing this
/// deliberately should treat as expected, not a real failure.
///
/// # Errors
///
/// Returns [`ClientError::Net`] for a transport-level failure other than a clean EOF
/// between frames,
/// or [`ClientError::Radiance`] if a `FRAME`/`PREVIEW` payload fails to decode.
pub fn run_client_session<R: Read>(
    reader: &mut R,
    accumulator: &mut Accumulator,
    mut on_update: impl FnMut(SessionUpdate),
) -> Result<(), ClientError> {
    loop {
        let (event, payload) = match messages::read_stream_event(reader) {
            Ok(v) => v,
            Err(messages::NetError::Framing(crate::framing::FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        let outcome = accumulator.apply(&event, payload.as_deref())?;
        on_update(to_update(&event, outcome));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        client::accumulate::ApplyOutcome,
        messages::{Done, FrameHeader, write_stream_event},
        radiance,
    };
    use glam::Vec3;
    use std::io::Cursor;

    #[cfg(feature = "render")]
    fn tiny_scene() -> crate::SceneState {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };
        crate::SceneState {
            width: 2,
            height: 2,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: crate::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: Default::default(),
        }
    }

    #[cfg(feature = "render")]
    fn render_request(request_id: u32) -> crate::messages::RenderRequest {
        use crate::messages::{PreviewConfig, RequestIntent, StreamConfig, TransferMode};
        crate::messages::RenderRequest {
            request_id,
            scene: tiny_scene(),
            first_sample: 0,
            samples: 4,
            stream: StreamConfig {
                transfer_mode: TransferMode::LiveProgressive,
                cadence_ms: 50,
                preview: Some(PreviewConfig {
                    width: 1,
                    height: 1,
                }),
            },
            intent: RequestIntent::Interactive,
        }
    }

    /// The v14 connection-level events reach `on_update` through the session loop,
    /// ahead of (and independent of) any request epoch.
    #[test]
    fn run_client_session_reports_pong_and_capability_changed() {
        let mut wire = Vec::new();
        write_stream_event(&mut wire, &StreamEvent::Pong { nonce: 5 }, None).unwrap();
        write_stream_event(
            &mut wire,
            &StreamEvent::CapabilityChanged { render: None },
            None,
        )
        .unwrap();
        let mut acc = Accumulator::new(1, 1);
        let mut updates = Vec::new();
        run_client_session(&mut Cursor::new(wire), &mut acc, |u| updates.push(u)).unwrap();
        assert_eq!(
            updates,
            vec![
                SessionUpdate::Pong { nonce: 5 },
                SessionUpdate::CapabilityChanged { render: None },
            ]
        );
    }

    #[cfg(feature = "render")]
    #[test]
    fn send_render_request_wraps_it_in_the_tagged_client_message_envelope() {
        let mut buf = Vec::new();
        send_render_request(&mut buf, &render_request(9)).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: ClientMessage = messages::read_message(&mut cursor).unwrap();
        assert!(matches!(
            decoded,
            ClientMessage::RenderRequest(r) if r.request_id == 9
        ));
    }

    #[cfg(feature = "render")]
    #[test]
    fn send_tilt_curves_request_wraps_it_in_the_tagged_client_message_envelope() {
        let request = crate::messages::TiltCurvesRequest {
            request_id: 9,
            scene: tiny_scene(),
        };
        let mut buf = Vec::new();
        send_tilt_curves_request(&mut buf, &request).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: ClientMessage = messages::read_message(&mut cursor).unwrap();
        assert!(matches!(
            decoded,
            ClientMessage::TiltCurvesRequest(r) if r.request_id == 9
        ));
    }

    #[cfg(feature = "render")]
    #[test]
    fn recv_tilt_curves_response_reads_back_each_variant() {
        use crate::messages::{AxisTiltCurves, TiltCurvesResponse, TiltCurvesResult};

        let axis = AxisTiltCurves {
            brilliance_pct: vec![1.0; 181],
            extinction_pct: vec![2.0; 181],
            windowing_pct: vec![3.0; 181],
        };
        for response in [
            TiltCurvesResponse::Curves(Box::new(TiltCurvesResult {
                request_id: 9,
                axes: [axis.clone(), axis.clone(), axis.clone(), axis],
            })),
            TiltCurvesResponse::Cancelled { request_id: 9 },
            TiltCurvesResponse::Error(ErrorMsg {
                code: 2,
                message: "scene.planes must not be empty".to_string(),
                request_id: None,
            }),
        ] {
            let mut buf = Vec::new();
            messages::write_message(&mut buf, &response).unwrap();
            let mut cursor = Cursor::new(buf);
            let decoded = recv_tilt_curves_response(&mut cursor).unwrap();
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn send_cancel_wraps_the_request_id_in_the_tagged_envelope() {
        let mut buf = Vec::new();
        send_cancel(&mut buf, 9).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: ClientMessage = messages::read_message(&mut cursor).unwrap();
        assert_eq!(decoded, ClientMessage::Cancel(Cancel { request_id: 9 }));
    }

    #[test]
    fn send_library_request_wraps_it_in_the_tagged_client_message_envelope() {
        let mut buf = Vec::new();
        send_library_request(&mut buf, &crate::library::LibraryRequest::FilterOptions).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: ClientMessage = messages::read_message(&mut cursor).unwrap();
        assert_eq!(
            decoded,
            ClientMessage::Library(Box::new(crate::library::LibraryRequest::FilterOptions))
        );
    }

    #[test]
    fn run_client_session_returns_ok_on_clean_eof_with_no_events() {
        let mut cursor = Cursor::new(Vec::new());
        let mut acc = Accumulator::new(2, 2);
        let mut updates = Vec::new();
        run_client_session(&mut cursor, &mut acc, |u| updates.push(u)).unwrap();
        assert_eq!(updates.len(), 0);
    }

    /// End-to-end version of `Accumulator`'s own epoch tests: a scripted reply stream
    /// carrying a cancelled request's leftover FRAME/DONE, immediately followed by a
    /// pipelined request's own FRAME/DONE. Proves the reader loop, not just
    /// `Accumulator` in isolation, honors the epoch rule.
    #[test]
    fn stale_events_from_a_cancelled_request_are_dropped_while_the_pipelined_ones_apply() {
        let pixel_count = 4usize;
        let mut wire = Vec::new();

        // Epoch 1's leftovers: already in flight when the write-side thread sent
        // CANCEL/RenderRequest(2) and called `begin_request(2)`, so by the time this
        // reader loop processes them the accumulator's current epoch is already 2.
        let delta1 = vec![Vec3::splat(1.0); pixel_count];
        let bytes1 = radiance::encode(&delta1);
        let header1 = FrameHeader::for_payload(1, 0, 2, &bytes1);
        write_stream_event(&mut wire, &StreamEvent::Frame(header1), Some(&bytes1)).unwrap();
        write_stream_event(
            &mut wire,
            &StreamEvent::Done(Done {
                request_id: 1,
                cancelled: true,
                stats: Stats {
                    samples_done: 2,
                    requested_cadence_ms: 50,
                    effective_cadence_ms: 0,
                    reclaimed_samples: 0,
                },
            }),
            None,
        )
        .unwrap();

        // Epoch 2: the pipelined request's own FRAME + DONE.
        let delta2 = vec![Vec3::splat(9.0); pixel_count];
        let bytes2 = radiance::encode(&delta2);
        let header2 = FrameHeader::for_payload(2, 0, 4, &bytes2);
        write_stream_event(&mut wire, &StreamEvent::Frame(header2), Some(&bytes2)).unwrap();
        write_stream_event(
            &mut wire,
            &StreamEvent::Done(Done {
                request_id: 2,
                cancelled: false,
                stats: Stats {
                    samples_done: 4,
                    requested_cadence_ms: 50,
                    effective_cadence_ms: 20,
                    reclaimed_samples: 0,
                },
            }),
            None,
        )
        .unwrap();

        let mut cursor = Cursor::new(wire);
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(2); // write-side thread already moved on to epoch 2

        let mut updates = Vec::new();
        run_client_session(&mut cursor, &mut acc, |u| updates.push(u)).unwrap();

        assert_eq!(
            updates,
            vec![
                SessionUpdate::StaleDropped { request_id: 1 },
                SessionUpdate::StaleDropped { request_id: 1 },
                SessionUpdate::Frame {
                    request_id: 2,
                    samples_done: 4
                },
                SessionUpdate::Done {
                    request_id: 2,
                    cancelled: false,
                    stats: Stats {
                        samples_done: 4,
                        requested_cadence_ms: 50,
                        effective_cadence_ms: 20,
                        reclaimed_samples: 0
                    }
                },
            ]
        );

        // Only epoch 2's delta (9.0 per pixel) reflects in the buffer, never epoch 1's.
        for v in acc.buffer() {
            assert!((*v - Vec3::splat(9.0)).length() < 1e-6);
        }
    }

    #[test]
    fn run_client_session_reports_preview_progress_and_worker_error() {
        let mut wire = Vec::new();
        let preview_buf = vec![Vec3::splat(3.0); 1];
        let preview_bytes = radiance::encode(&preview_buf);
        let preview_header =
            crate::messages::PreviewHeader::for_payload(1, 1, 1, 2, &preview_bytes);
        write_stream_event(
            &mut wire,
            &StreamEvent::Preview(preview_header),
            Some(&preview_bytes),
        )
        .unwrap();
        write_stream_event(
            &mut wire,
            &StreamEvent::Progress(crate::messages::Progress {
                request_id: 1,
                samples_done: 2,
            }),
            None,
        )
        .unwrap();
        write_stream_event(
            &mut wire,
            &StreamEvent::Error(ErrorMsg {
                code: 3,
                message: "internal error while tracing this request".to_string(),
                request_id: None,
            }),
            None,
        )
        .unwrap();

        let mut cursor = Cursor::new(wire);
        let mut acc = Accumulator::new(2, 2);
        acc.begin_request(1);
        let mut updates = Vec::new();
        run_client_session(&mut cursor, &mut acc, |u| updates.push(u)).unwrap();

        assert_eq!(
            updates,
            vec![
                SessionUpdate::Preview { request_id: 1 },
                SessionUpdate::Progress {
                    request_id: 1,
                    samples_done: 2
                },
                SessionUpdate::WorkerError(ErrorMsg {
                    code: 3,
                    message: "internal error while tracing this request".to_string(),
                    request_id: None,
                }),
            ]
        );
    }

    #[test]
    fn apply_outcome_variants_are_exhaustively_reachable_via_to_update() {
        // Guards `to_update`'s `unreachable!` arms against drifting out of sync with
        // `Accumulator::apply`.
        let mut acc = Accumulator::new(1, 1);
        acc.begin_request(1);
        let (event, bytes) = {
            let buf = vec![Vec3::ONE; 1];
            let bytes = radiance::encode(&buf);
            let header = FrameHeader::for_payload(1, 0, 1, &bytes);
            (StreamEvent::Frame(header), bytes)
        };
        let outcome = acc.apply(&event, Some(&bytes)).unwrap();
        assert!(matches!(outcome, ApplyOutcome::FrameSummed { .. }));
        let _ = to_update(&event, outcome);
    }
}

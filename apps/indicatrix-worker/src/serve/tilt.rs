//! `TILT_CURVES`: the single-request/single-response analytic tilt-performance sweep --
//! contrast with `RENDER`'s progressive, multi-message stream (`crate::stream_emit`).
//! See `indicatrix_net::messages::tilt`'s module docs for the wire types.
//!
//! [`handle_tilt_curves_request`] is the one entry point, called from
//! `super::connection`'s worker-build request loop where `RenderRequest` instead calls
//! `crate::stream_emit::run_stream`.
//!
//! # Runs inline, unlike `RenderRequest`
//!
//! `stream_emit::run_stream` splits tracing onto its own thread so a slow `write()`
//! never stalls sample production across a multi-second stream. A `TiltCurvesRequest`
//! writes exactly one ~8,688-byte reply at the end, so there is nothing to stall and the
//! thread split buys nothing. Measured cost is ~1.36s total (four axes, ~340ms each).
//!
//! # Cancellation: 4 checkpoints, ~340ms worst-case latency
//!
//! [`poll_for_cancel`] runs once before each axis rather than `stream_emit`'s continuous
//! polling loop -- there's nothing to report progress on between axes. A `CANCEL`
//! arriving mid-axis is only noticed at the next checkpoint, bounding worst-case
//! cancellation latency to one axis's duration (~340ms).
//!
//! # `ComputeMode` plays no role here
//!
//! `evaluate_full_axis_profile_at_azimuth` is a CPU-only analytic sweep with no GPU
//! kernel; `--only-gpu`/`--only-cpu` have nothing to change, so
//! [`handle_tilt_curves_request`] deliberately takes no `ComputeMode` parameter.
//!
//! # No pipelining
//!
//! Only one `TiltCurvesRequest` may be outstanding per connection (unlike
//! `RenderRequest`, which `run_stream` accepts pipelined as an implicit cancel).
//! [`poll_for_cancel`] logs and drops anything else pipelined mid-computation. A client
//! wanting concurrent tilt-curve requests opens more than one connection.

use crate::{stream_emit::TimeoutRead, validate};
use indicatrix_net::messages::{
    AxisTiltCurves, ErrorMsg, NetError, TILT_CURVE_AXIS_COUNT, TiltCurvesRequest,
    TiltCurvesResponse, TiltCurvesResult,
};
use std::{
    io::{Read, Write},
    time::Duration,
};

/// How long [`poll_for_cancel`] waits, per checkpoint, for a `CANCEL` that might already
/// be sitting in the socket's receive buffer. Short and non-looping: this runs exactly
/// once per checkpoint, so it only needs to catch what's already arrived over the
/// ~340ms since the previous one.
const CANCEL_POLL_TIMEOUT: Duration = Duration::from_millis(5);

/// What one [`poll_for_cancel`] check found.
enum CancelPoll {
    /// Nothing pending within the poll window.
    Pending,
    /// A `CANCEL` for this exact `request_id`.
    Cancelled,
    /// The peer closed the connection.
    Closed,
}

/// Checks whether a `CANCEL` for `request_id` is already sitting on `stream`'s socket
/// buffer, without blocking longer than [`CANCEL_POLL_TIMEOUT`].
///
/// Mirrors `crate::stream_emit`'s private `poll_for_client_message`: the length-prefix's
/// first byte is read with a raw, timeout-tolerant `read()` rather than `read_exact`
/// (whose docs leave it unspecified how many bytes land on an error), since a timeout
/// must never land mid-message. Once a byte is seen, the rest of that message is read
/// under a bounded timeout (`FRAME_REMAINDER_TIMEOUT`) where a timeout IS a protocol
/// error. The first byte's timeout tolerance goes through
/// [`crate::stream_emit::is_stream_timeout`] since a TLS stream's timeout can surface as
/// `WriteZero` even from a read.
///
/// Anything besides a matching `CANCEL` -- a stale `CANCEL`, or any other pipelined
/// `ClientMessage` -- is logged and dropped as [`CancelPoll::Pending`]: `TILT_CURVES`
/// supports no pipelining, and the connection's next ordinary read picks it up once this
/// reply has gone out.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure other than the tolerated timeout.
fn poll_for_cancel<S: Read + TimeoutRead>(
    stream: &mut S,
    request_id: u32,
) -> Result<CancelPoll, NetError> {
    stream
        .set_read_timeout(Some(CANCEL_POLL_TIMEOUT))
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

    let mut len_bytes = [0u8; indicatrix_net::framing::LEN_PREFIX_BYTES];
    let n = match stream.read(&mut len_bytes) {
        Ok(0) => return Ok(CancelPoll::Closed),
        Ok(n) => n,
        Err(e) if crate::stream_emit::is_stream_timeout(&e) => {
            return Ok(CancelPoll::Pending);
        }
        Err(e) => {
            return Err(NetError::Framing(
                indicatrix_net::framing::FramingError::Io(e),
            ));
        }
    };

    // At least one byte of a message has arrived -- commit to reading the rest of it
    // under a bounded timeout (N11), not `None` (see this function's own doc comment).
    stream
        .set_read_timeout(Some(crate::stream_emit::FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;
    if n < len_bytes.len() {
        stream
            .read_exact(&mut len_bytes[n..])
            .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > indicatrix_net::framing::MAX_FRAME_LEN {
        return Err(NetError::Framing(
            indicatrix_net::framing::FramingError::FrameTooLarge {
                len,
                max: indicatrix_net::framing::MAX_FRAME_LEN,
            },
        ));
    }
    let mut payload = vec![0u8; len as usize];
    stream
        .read_exact(&mut payload)
        .map_err(|e| NetError::Framing(indicatrix_net::framing::FramingError::Io(e)))?;

    let msg: indicatrix_net::messages::ClientMessage = postcard::from_bytes(&payload)?;
    Ok(match msg {
        indicatrix_net::messages::ClientMessage::Cancel(c) if c.request_id == request_id => {
            CancelPoll::Cancelled
        }
        other => {
            tracing::debug!(
                "ignoring a message pipelined while request_id={request_id}'s tilt-curves \
                 computation was in flight (TILT_CURVES has no mid-request pipelining in this \
                 phase): {other:?}"
            );
            CancelPoll::Pending
        }
    })
}

/// Handles one `TiltCurvesRequest` end to end: validates the embedded scene, computes
/// all [`TILT_CURVE_AXIS_COUNT`] axes, and writes exactly one [`TiltCurvesResponse`] --
/// never a stream. See the module docs for why this runs inline, the cancellation
/// design, and why `ComputeMode` plays no role.
///
/// Validates with [`validate::validate_scene`] -- the same path `RenderRequest` uses, so
/// a rejected scene gets [`TiltCurvesResponse::Error`] carrying
/// `super::connection::VALIDATION_FAILED_CODE`, the same numeric code `RenderRequest`
/// uses in its own envelope.
///
/// Runs the per-axis evaluation inside `catch_unwind`, mirroring
/// `stream_emit::tracer::run_tracer`'s precedent: a pathological but validation-passing
/// scene is reported as [`TiltCurvesResponse::Error`] with
/// `super::connection::TRACE_PANIC_CODE`, not a crashed connection.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure while polling for `CANCEL` or
/// writing the reply. A validation failure or caught panic is NOT an error return --
/// both are reported as `TiltCurvesResponse::Error` and this returns `Ok(())`.
///
/// # Panics
///
/// Never panics in practice: the `unreachable!()` converting the axes `Vec` into a
/// fixed-size array is unreachable by construction (the `debug_assert_eq!` above the
/// loop pins the two independently-defined axis counts equal in every debug build).
pub(super) fn handle_tilt_curves_request<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &TiltCurvesRequest,
) -> Result<(), NetError> {
    if let Err(message) = validate::validate_scene(&request.scene) {
        return indicatrix_net::messages::write_message(
            stream,
            &TiltCurvesResponse::Error(ErrorMsg {
                code: super::connection::VALIDATION_FAILED_CODE,
                message,
            }),
        );
    }

    debug_assert_eq!(
        indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG.len(),
        TILT_CURVE_AXIS_COUNT,
        "indicatrix's PROFILE_AZIMUTHS_DEG and indicatrix-net's independently-pinned \
         TILT_CURVE_AXIS_COUNT must agree -- see indicatrix_net::messages::tilt's module doc \
         comment for why the two are pinned separately rather than sharing one constant"
    );

    let mut axes: Vec<AxisTiltCurves> = Vec::with_capacity(TILT_CURVE_AXIS_COUNT);
    let scene = &request.scene;

    for (axis_index, &azimuth_deg) in indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG
        .iter()
        .enumerate()
    {
        match poll_for_cancel(stream, request.request_id)? {
            CancelPoll::Cancelled => {
                tracing::debug!(
                    "request_id={}: CANCEL honored after {} of {TILT_CURVE_AXIS_COUNT} axes",
                    request.request_id,
                    axes.len(),
                );
                return indicatrix_net::messages::write_message(
                    stream,
                    &TiltCurvesResponse::Cancelled {
                        request_id: request.request_id,
                    },
                );
            }
            CancelPoll::Closed => return Ok(()),
            CancelPoll::Pending => {}
        }

        let axis_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth(
                &scene.planes,
                &scene.material,
                azimuth_deg,
                scene.light_yaw,
                scene.light_pitch,
            )
        }));

        if let Ok((brilliance_pct, extinction_pct, windowing_pct)) = axis_result {
            axes.push(AxisTiltCurves::from_arrays(
                brilliance_pct,
                extinction_pct,
                windowing_pct,
            ));
        } else {
            tracing::warn!(
                "tilt-curves computation panicked for a request that passed validation \
                 (request_id={}, axis_index={axis_index}, azimuth_deg={azimuth_deg})",
                request.request_id,
            );
            return indicatrix_net::messages::write_message(
                stream,
                &TiltCurvesResponse::Error(ErrorMsg {
                    code: super::connection::TRACE_PANIC_CODE,
                    message: "internal error while computing tilt-performance curves".to_string(),
                }),
            );
        }
    }

    let axes: [AxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().unwrap_or_else(|v: Vec<_>| {
        unreachable!(
            "exactly TILT_CURVE_AXIS_COUNT={TILT_CURVE_AXIS_COUNT} axes were pushed above, got {}",
            v.len()
        )
    });

    indicatrix_net::messages::write_message(
        stream,
        &TiltCurvesResponse::Curves(Box::new(TiltCurvesResult {
            request_id: request.request_id,
            axes,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use indicatrix_net::{SceneState, messages::write_message};
    use std::io::Cursor;

    /// A minimal `Read + Write + TimeoutRead` double over two in-memory buffers,
    /// mirroring `super::super::tests::DuplexHalf` (kept separate rather than reaching
    /// across that module boundary). `closed` distinguishes "peer hung up" (`Ok(0)`
    /// even with a timeout active) from "nothing pending yet" (`WouldBlock`/`TimedOut`),
    /// since a bare `timeout_active` flag alone can't tell them apart.
    struct DuplexHalf {
        in_: Cursor<Vec<u8>>,
        out: Vec<u8>,
        timeout_active: bool,
        closed: bool,
    }

    impl DuplexHalf {
        const fn new(input: Vec<u8>) -> Self {
            Self {
                in_: Cursor::new(input),
                out: Vec::new(),
                timeout_active: false,
                closed: false,
            }
        }
    }

    impl Read for DuplexHalf {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.in_.read(buf)?;
            if n == 0 && !self.closed && self.timeout_active {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "DuplexHalf: no more scripted input (yet)",
                ));
            }
            Ok(n)
        }
    }

    impl Write for DuplexHalf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TimeoutRead for DuplexHalf {
        fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
            self.timeout_active = duration.is_some();
            Ok(())
        }
    }

    fn valid_scene() -> SceneState {
        SceneState {
            width: 4,
            height: 4,
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
        }
    }

    fn read_response(out: &[u8]) -> TiltCurvesResponse {
        let mut cursor = Cursor::new(out.to_vec());
        indicatrix_net::messages::read_message(&mut cursor).unwrap()
    }

    #[test]
    fn rejects_an_invalid_scene_with_the_shared_validation_failed_code() {
        let mut scene = valid_scene();
        scene.planes.clear(); // fails validate_scene: planes must not be empty
        let request = TiltCurvesRequest {
            request_id: 1,
            scene,
        };
        let mut duplex = DuplexHalf::new(Vec::new());

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        match response {
            TiltCurvesResponse::Error(ErrorMsg { code, message }) => {
                assert_eq!(code, super::super::connection::VALIDATION_FAILED_CODE);
                assert!(message.contains("planes"), "{message}");
            }
            other => panic!("expected TiltCurvesResponse::Error, got {other:?}"),
        }
    }

    #[test]
    fn a_matching_cancel_already_on_the_wire_is_honored_before_any_axis_computes() {
        let request = TiltCurvesRequest {
            request_id: 5,
            scene: valid_scene(),
        };
        // Already sitting in the input buffer; the first checkpoint must catch it.
        let mut cancel_bytes = Vec::new();
        write_message(
            &mut cancel_bytes,
            &indicatrix_net::messages::ClientMessage::Cancel(indicatrix_net::messages::Cancel {
                request_id: 5,
            }),
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(cancel_bytes);

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        assert_eq!(response, TiltCurvesResponse::Cancelled { request_id: 5 });
    }

    #[test]
    fn a_cancel_for_a_different_request_id_is_ignored_and_the_request_completes() {
        let request = TiltCurvesRequest {
            request_id: 5,
            scene: valid_scene(),
        };
        let mut stale_cancel_bytes = Vec::new();
        write_message(
            &mut stale_cancel_bytes,
            &indicatrix_net::messages::ClientMessage::Cancel(indicatrix_net::messages::Cancel {
                request_id: 999,
            }),
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(stale_cancel_bytes);

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        match response {
            TiltCurvesResponse::Curves(result) => {
                assert_eq!(result.request_id, 5);
                assert_eq!(result.axes.len(), TILT_CURVE_AXIS_COUNT);
            }
            other => panic!("expected TiltCurvesResponse::Curves, got {other:?}"),
        }
    }

    #[test]
    fn a_closed_connection_writes_nothing() {
        let request = TiltCurvesRequest {
            request_id: 1,
            scene: valid_scene(),
        };
        let mut duplex = DuplexHalf::new(Vec::new());
        duplex.closed = true; // simulates a peer that already hung up

        handle_tilt_curves_request(&mut duplex, &request).unwrap();
        assert!(
            duplex.out.is_empty(),
            "a peer that already closed the connection gets no reply written"
        );
    }

    /// Pins that `poll_for_cancel` tolerates a `WriteZero` (which a real TLS stream can
    /// surface for a socket timeout even from a read) exactly like `WouldBlock`/
    /// `TimedOut`, rather than tearing the connection down.
    struct WriteZeroOnFirstRead;

    impl Read for WriteZeroOnFirstRead {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "scripted: TLS complete_io() swallowed an inner write timeout",
            ))
        }
    }

    impl TimeoutRead for WriteZeroOnFirstRead {
        fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn poll_for_cancel_tolerates_a_write_zero_on_the_first_byte() {
        let mut stream = WriteZeroOnFirstRead;
        let result = poll_for_cancel(&mut stream, 1);
        assert!(
            matches!(result, Ok(CancelPoll::Pending)),
            "a WriteZero on the very first read must be tolerated as \"nothing pending \
             yet\", not a fatal protocol error"
        );
    }
}

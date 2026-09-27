//! Protocol v14 end to end through the real connection handler: payload-encoding
//! negotiation, compressed `FRAME`/`PREVIEW` streams summing bit-identically to raw ones
//! in the client `Accumulator`, `PING`/`PONG`, and the "not supported by this server"
//! refusals for a worker-role `HELLO`, `FinalImageRequest` and `TransferMode::DisplayOnly`.

use super::fixtures::{
    DuplexHalf, final_only, heavier_scene, read_stream_until_done, test_db, tiny_scene,
};
use crate::{cli::ComputeMode, render_core, serve::handle_connection_with_gpu};
use indicatrix::renderer::gpu_backend::GpuBackend;
use indicatrix_net::{
    client::{Accumulator, ApplyOutcome},
    handshake,
    messages::{
        ClientMessage, DEFAULT_SERVER_PREFERENCE, ErrorMsg, FinalImageRequest, FinalOutput,
        FrameHeader, Hello, PayloadEncoding, PeerRole, PreviewConfig, RenderCapability,
        RenderRequest, RequestIntent, StreamConfig, StreamEvent, TransferMode, Welcome,
        WireColorSpace, error_codes,
    },
    radiance::PayloadDecoder,
};
use std::{io::Cursor, sync::Arc};

/// Runs one scripted connection through `handle_connection_with_gpu` (CPU only) with the
/// given server preference and returns everything the server wrote.
fn run_scripted(input: Vec<u8>, encodings: &[PayloadEncoding]) -> Cursor<Vec<u8>> {
    let mut duplex = DuplexHalf::new(input);
    handle_connection_with_gpu(
        &mut duplex,
        2,
        &Arc::new(GpuBackend::disabled()),
        &test_db(),
        ComputeMode::OnlyCpu,
        encodings,
    )
    .unwrap();
    Cursor::new(duplex.out)
}

fn write_hello(buf: &mut Vec<u8>, hello: &Hello) {
    indicatrix_net::messages::write_message(buf, hello).unwrap();
}

fn write(buf: &mut Vec<u8>, msg: &ClientMessage) {
    indicatrix_net::messages::write_message(buf, msg).unwrap();
}

fn bits(values: &[glam::Vec3]) -> Vec<u32> {
    values
        .iter()
        .flat_map(glam::Vec3::to_array)
        .map(f32::to_bits)
        .collect()
}

/// Streams a real render once per encoding. For each run, the client `Accumulator` fed
/// the wire events directly must hold exactly (bit for bit) the sum an accumulator gets
/// from the same deltas re-sent raw; the compressed encodings must actually be used on
/// the wire; and the result must still match tracing the range directly.
#[test]
fn every_payload_encoding_streams_frames_that_sum_bit_identically_to_raw() {
    let scene = heavier_scene();
    let (w, h) = (scene.width, scene.height);
    let direct = render_core::trace_samples(&scene, 0, 32, 2);
    let encodings = [
        PayloadEncoding::Raw,
        PayloadEncoding::ShuffleZstd { level: 1 },
        PayloadEncoding::ShuffleLz4,
    ];
    for encoding in encodings {
        let mut input = Vec::new();
        write_hello(&mut input, &handshake::local_hello());
        let request = RenderRequest {
            request_id: 21,
            scene: scene.clone(),
            first_sample: 0,
            samples: 32,
            stream: StreamConfig {
                transfer_mode: TransferMode::LiveProgressive,
                cadence_ms: 0,
                preview: Some(PreviewConfig {
                    width: 12,
                    height: 12,
                }),
            },
            intent: RequestIntent::Interactive,
        };
        write(&mut input, &ClientMessage::RenderRequest(Box::new(request)));

        let mut out = run_scripted(input, &[encoding]);
        let welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
        assert_eq!(welcome.payload_encoding, encoding);
        assert!(welcome.registration.is_none());
        let events = read_stream_until_done(&mut out);

        let mut wire_acc = Accumulator::new(w, h);
        wire_acc.begin_request_for_range(21, 0, 32);
        let mut raw_acc = Accumulator::new(w, h);
        raw_acc.begin_request_for_range(21, 0, 32);
        let mut decoder = PayloadDecoder::new();
        let mut used_negotiated = false;
        for (event, payload) in &events {
            let outcome = wire_acc.apply(event, payload.as_deref()).unwrap();
            assert_ne!(outcome, ApplyOutcome::StaleDropped);
            if let StreamEvent::Frame(fh) = event {
                used_negotiated |= fh.encoding == encoding;
                let bytes = payload.as_deref().unwrap();
                let delta = decoder
                    .decode_to_vec(fh.encoding, fh.raw_len, bytes, w, h)
                    .unwrap();
                let raw = indicatrix_net::radiance::encode(&delta);
                let raw_header = FrameHeader::for_payload(21, fh.first_sample, fh.samples, &raw);
                raw_acc
                    .apply(&StreamEvent::Frame(raw_header), Some(&raw))
                    .unwrap();
            }
        }
        assert!(used_negotiated, "{encoding:?} never appeared on a FRAME");
        // Any PREVIEW that arrived was decoded by `apply` above (an error would have
        // failed the unwrap); whether one arrives at all depends on tick timing.
        assert_eq!(wire_acc.samples_done(), 32);
        assert_eq!(
            bits(wire_acc.buffer()),
            bits(raw_acc.buffer()),
            "{encoding:?}"
        );

        for (a, b) in wire_acc.buffer().iter().zip(&direct) {
            let scale = a.abs().max(b.abs()).max(glam::Vec3::splat(1e-6));
            assert!(
                ((*a - *b).abs() / scale).max_element() < 1e-3,
                "{encoding:?}: wire={a:?} direct={b:?}"
            );
        }
    }
}

/// The server picks the first entry of ITS preference the viewer accepts; nothing shared
/// means `Raw`.
#[test]
fn welcome_carries_the_encoding_negotiated_from_the_viewers_accept_list() {
    for (accepts, expected) in [
        (
            vec![PayloadEncoding::ShuffleLz4],
            PayloadEncoding::ShuffleLz4,
        ),
        (
            vec![
                PayloadEncoding::ShuffleZstd { level: 9 },
                PayloadEncoding::Raw,
            ],
            PayloadEncoding::ShuffleZstd { level: 1 },
        ),
        (vec![], PayloadEncoding::Raw),
    ] {
        let mut input = Vec::new();
        let hello = Hello {
            accept_encodings: accepts.clone(),
            ..handshake::local_hello()
        };
        write_hello(&mut input, &hello);
        let mut out = run_scripted(input, &DEFAULT_SERVER_PREFERENCE);
        let welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
        assert_eq!(welcome.payload_encoding, expected, "accepts={accepts:?}");
    }
}

/// A `PING` between requests is answered with a `PONG` carrying the same nonce.
#[test]
fn a_ping_between_requests_is_answered_with_a_pong() {
    let mut input = Vec::new();
    write_hello(&mut input, &handshake::local_hello());
    write(&mut input, &ClientMessage::Ping { nonce: 0xFEED });
    let mut out = run_scripted(input, &DEFAULT_SERVER_PREFERENCE);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
    let (event, payload) = indicatrix_net::messages::read_stream_event(&mut out).unwrap();
    assert_eq!(event, StreamEvent::Pong { nonce: 0xFEED });
    assert!(payload.is_none());
}

/// `FinalImageRequest` and a `DisplayOnly` render are refused with
/// `UNSUPPORTED_REQUEST`, and the connection keeps serving the next request.
#[test]
fn final_image_and_display_only_are_refused_and_the_connection_stays_usable() {
    let mut input = Vec::new();
    write_hello(&mut input, &handshake::local_hello());
    write(
        &mut input,
        &ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
            request_id: 1,
            scene: tiny_scene(),
            first_sample: 0,
            samples: 2,
            width: 4,
            height: 4,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
        })),
    );
    let display_only = RenderRequest {
        request_id: 2,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: StreamConfig {
            transfer_mode: TransferMode::DisplayOnly,
            cadence_ms: 100,
            preview: None,
        },
        intent: RequestIntent::Interactive,
    };
    write(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(display_only)),
    );
    let good = RenderRequest {
        request_id: 3,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
        intent: RequestIntent::Batch,
    };
    write(&mut input, &ClientMessage::RenderRequest(Box::new(good)));

    let mut out = run_scripted(input, &DEFAULT_SERVER_PREFERENCE);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out).unwrap();
    for what in ["FinalImageRequest", "DisplayOnly"] {
        let events = read_stream_until_done(&mut out);
        let [(StreamEvent::Error(err), None)] = events.as_slice() else {
            panic!("expected one StreamEvent::Error for {what}, got {events:?}");
        };
        assert_eq!(err.code, error_codes::UNSUPPORTED_REQUEST);
        assert!(err.message.contains(what), "{}", err.message);
        assert!(err.message.contains("not supported by this server"));
    }
    let events = read_stream_until_done(&mut out);
    assert!(matches!(events.last().unwrap().0, StreamEvent::Done(d) if d.request_id == 3));
}

/// A render worker trying to `join` this server is refused in place of `WELCOME`.
#[test]
fn a_worker_role_hello_is_refused_with_role_refused() {
    let mut input = Vec::new();
    let worker = Hello {
        role: PeerRole::Worker,
        capability: Some(RenderCapability {
            backend: indicatrix_net::messages::Backend::Cpu { threads: 8 },
            max_pixels: 1_000_000,
            min_cadence_ms: 100,
            hdr: false,
        }),
        ..handshake::local_hello()
    };
    write_hello(&mut input, &worker);
    let mut out = run_scripted(input, &DEFAULT_SERVER_PREFERENCE);
    let err: ErrorMsg = indicatrix_net::messages::read_message(&mut out).unwrap();
    assert_eq!(err.code, error_codes::ROLE_REFUSED);
    assert_eq!(
        out.position(),
        out.get_ref().len() as u64,
        "nothing after the refusal"
    );
}

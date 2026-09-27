//! Round-trip and wire-stability tests for the `stream` module: header/message structs,
//! [`super::ClientMessage`], [`super::StreamEvent`], and the `postcard` discriminants
//! pinned by the module doc comment's "Variant order" section.

use super::{
    super::codec::{read_message, write_message},
    *,
};

#[test]
fn error_round_trips() {
    let err = ErrorMsg {
        code: 42,
        message: "scene exceeds max_pixels".to_string(),
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &err).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: ErrorMsg = read_message(&mut cursor).unwrap();
    assert_eq!(err, decoded);
}

#[test]
fn frame_message_round_trips_and_validates_payload_len() {
    let xyz_bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let header = FrameHeader::for_payload(7, 64, 32, &xyz_bytes);
    assert_eq!(header.payload_len, xyz_bytes.len() as u32);
    assert_eq!(header.request_id, 7);

    let mut buf = Vec::new();
    write_frame_message(&mut buf, &header, &xyz_bytes).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let (decoded_header, decoded_bytes) = read_frame_message(&mut cursor).unwrap();
    assert_eq!(decoded_header, header);
    assert_eq!(decoded_bytes, xyz_bytes);
}

#[test]
fn frame_message_rejects_a_forged_payload_len() {
    let xyz_bytes = vec![0u8; 12];
    let lying_header = FrameHeader {
        request_id: 1,
        first_sample: 0,
        samples: 1,
        payload_len: 999,
        encoding: crate::messages::PayloadEncoding::Raw,
        raw_len: 12,
    };

    let mut buf = Vec::new();
    write_frame_message(&mut buf, &lying_header, &xyz_bytes).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let result = read_frame_message(&mut cursor);
    assert!(matches!(
        result,
        Err(super::super::codec::NetError::FramePayloadLenMismatch {
            declared: 999,
            actual: 12
        })
    ));
}

#[test]
fn preview_message_round_trips_and_validates_payload_len() {
    let xyz_bytes = vec![9u8; 24];
    let header = PreviewHeader::for_payload(7, 4, 2, 128, &xyz_bytes);
    assert_eq!(header.payload_len, xyz_bytes.len() as u32);

    let mut buf = Vec::new();
    write_preview_message(&mut buf, &header, &xyz_bytes).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let (decoded_header, decoded_bytes) = read_preview_message(&mut cursor).unwrap();
    assert_eq!(decoded_header, header);
    assert_eq!(decoded_bytes, xyz_bytes);
}

#[test]
fn preview_message_rejects_a_forged_payload_len() {
    let xyz_bytes = vec![0u8; 24];
    let lying_header = PreviewHeader {
        request_id: 1,
        width: 4,
        height: 2,
        samples_done: 8,
        payload_len: 999,
        encoding: crate::messages::PayloadEncoding::Raw,
        raw_len: 24,
    };

    let mut buf = Vec::new();
    write_preview_message(&mut buf, &lying_header, &xyz_bytes).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let result = read_preview_message(&mut cursor);
    assert!(matches!(
        result,
        Err(super::super::codec::NetError::FramePayloadLenMismatch {
            declared: 999,
            actual: 24
        })
    ));
}

#[test]
fn progress_round_trips() {
    let progress = Progress {
        request_id: 42,
        samples_done: 128,
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &progress).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: Progress = read_message(&mut cursor).unwrap();
    assert_eq!(progress, decoded);
}

#[test]
fn cancel_round_trips() {
    let cancel = Cancel { request_id: 42 };
    let mut buf = Vec::new();
    write_message(&mut buf, &cancel).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: Cancel = read_message(&mut cursor).unwrap();
    assert_eq!(cancel, decoded);
}

#[test]
fn done_round_trips_both_cancelled_states() {
    for cancelled in [false, true] {
        let done = Done {
            request_id: 42,
            cancelled,
            stats: Stats {
                samples_done: 256,
                requested_cadence_ms: 250,
                effective_cadence_ms: 1400,
            },
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &done).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Done = read_message(&mut cursor).unwrap();
        assert_eq!(done, decoded);
    }
}

#[test]
fn client_message_cancel_round_trips() {
    let msg = ClientMessage::Cancel(Cancel { request_id: 7 });
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: ClientMessage = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, msg);
}

#[cfg(feature = "render")]
#[test]
fn client_message_render_request_round_trips() {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let scene = crate::scene::SceneState {
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
        environment: crate::scene::SceneEnvironment::Studio,
    };
    let msg = ClientMessage::RenderRequest(Box::new(super::super::render::RenderRequest {
        request_id: 8,
        scene,
        first_sample: 0,
        samples: 4,
        stream: super::super::render::StreamConfig {
            transfer_mode: super::super::render::TransferMode::FinalOnly,
            cadence_ms: 100,
            preview: None,
        },
        intent: super::super::render::RequestIntent::Batch,
    }));
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: ClientMessage = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, msg);
}

#[cfg(feature = "render")]
#[test]
fn client_message_tilt_curves_request_round_trips() {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let scene = crate::scene::SceneState {
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
        environment: crate::scene::SceneEnvironment::Studio,
    };
    let msg = ClientMessage::TiltCurvesRequest(Box::new(super::super::tilt::TiltCurvesRequest {
        request_id: 11,
        scene,
    }));
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let decoded: ClientMessage = read_message(&mut cursor).unwrap();
    assert_eq!(decoded, msg);
}

/// Pins [`ClientMessage::Cancel`]/[`ClientMessage::Library`] at `postcard` variant
/// indices 0/1 -- the two always-compiled-in variants a library-only build and a
/// full (`render`-feature) build must agree on regardless of which one either peer
/// was built with (see the module doc comment's "Variant order" section). `postcard`
/// encodes a variant as a one-byte leading varint for any index below 128, so the
/// first encoded byte is the declaration index this test pins.
#[test]
fn cancel_and_library_keep_postcard_discriminants_0_and_1() {
    let cancel = ClientMessage::Cancel(Cancel { request_id: 1 });
    let cancel_bytes = postcard::to_allocvec(&cancel).unwrap();
    assert_eq!(cancel_bytes[0], 0, "Cancel must stay discriminant 0");

    let library = ClientMessage::Library(Box::new(crate::library::LibraryRequest::FilterOptions));
    let library_bytes = postcard::to_allocvec(&library).unwrap();
    assert_eq!(library_bytes[0], 1, "Library must stay discriminant 1");
}

/// Pins [`ClientMessage::RenderRequest`]/[`ClientMessage::TiltCurvesRequest`] at
/// `postcard` variant indices 2/3, both after `Cancel`/`Library` and
/// `TiltCurvesRequest` after `RenderRequest` -- the order the module doc comment
/// requires. Only meaningful in a `render`-enabled build.
#[cfg(feature = "render")]
#[test]
fn render_gated_variants_are_appended_in_order_after_cancel_and_library() {
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let scene = crate::scene::SceneState {
        width: 1,
        height: 1,
        yaw: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: 0.0,
        light_pitch: 0.0,
        exposure: 1.0,
        max_bounces: 1,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: crate::scene::SceneEnvironment::Studio,
    };

    let render_request =
        ClientMessage::RenderRequest(Box::new(super::super::render::RenderRequest {
            request_id: 1,
            scene: scene.clone(),
            first_sample: 0,
            samples: 1,
            stream: super::super::render::StreamConfig {
                transfer_mode: super::super::render::TransferMode::FinalOnly,
                cadence_ms: 100,
                preview: None,
            },
            intent: super::super::render::RequestIntent::Interactive,
        }));
    let render_bytes = postcard::to_allocvec(&render_request).unwrap();
    assert_eq!(render_bytes[0], 2, "RenderRequest must stay discriminant 2");

    let tilt_request =
        ClientMessage::TiltCurvesRequest(Box::new(super::super::tilt::TiltCurvesRequest {
            request_id: 1,
            scene,
        }));
    let tilt_bytes = postcard::to_allocvec(&tilt_request).unwrap();
    assert_eq!(
        tilt_bytes[0], 3,
        "TiltCurvesRequest must be discriminant 3 -- appended after RenderRequest"
    );
}

#[test]
fn stream_event_frame_and_preview_round_trip_with_their_payload() {
    let xyz_bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let frame_header = FrameHeader::for_payload(7, 0, 4, &xyz_bytes);
    let event = StreamEvent::Frame(frame_header);
    let mut buf = Vec::new();
    write_stream_event(&mut buf, &event, Some(&xyz_bytes)).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
    assert_eq!(decoded_event, event);
    assert_eq!(decoded_payload, Some(xyz_bytes.clone()));

    let preview_header = PreviewHeader::for_payload(7, 2, 2, 16, &xyz_bytes);
    let event = StreamEvent::Preview(preview_header);
    let mut buf = Vec::new();
    write_stream_event(&mut buf, &event, Some(&xyz_bytes)).unwrap();
    let mut cursor = std::io::Cursor::new(buf);
    let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
    assert_eq!(decoded_event, event);
    assert_eq!(decoded_payload, Some(xyz_bytes));
}

#[test]
fn stream_event_progress_done_and_error_round_trip_with_no_payload() {
    for event in [
        StreamEvent::Progress(Progress {
            request_id: 7,
            samples_done: 64,
        }),
        StreamEvent::Done(Done {
            request_id: 7,
            cancelled: false,
            stats: Stats {
                samples_done: 64,
                requested_cadence_ms: 250,
                effective_cadence_ms: 300,
            },
        }),
        StreamEvent::Error(ErrorMsg {
            code: 3,
            message: "internal error while tracing this request".to_string(),
        }),
    ] {
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, None).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let (decoded_event, decoded_payload) = read_stream_event(&mut cursor).unwrap();
        assert_eq!(decoded_event, event);
        assert_eq!(decoded_payload, None);
    }
}

#[test]
fn stream_event_a_sequence_reads_back_in_order() {
    // Two FRAMEs, a PROGRESS, then DONE -- an interleaved reply sequence.
    let bytes_a = vec![0u8; 12];
    let bytes_b = vec![1u8; 12];
    let mut buf = Vec::new();
    write_stream_event(
        &mut buf,
        &StreamEvent::Frame(FrameHeader::for_payload(1, 0, 4, &bytes_a)),
        Some(&bytes_a),
    )
    .unwrap();
    write_stream_event(
        &mut buf,
        &StreamEvent::Frame(FrameHeader::for_payload(1, 4, 4, &bytes_b)),
        Some(&bytes_b),
    )
    .unwrap();
    write_stream_event(
        &mut buf,
        &StreamEvent::Progress(Progress {
            request_id: 1,
            samples_done: 8,
        }),
        None,
    )
    .unwrap();
    write_stream_event(
        &mut buf,
        &StreamEvent::Done(Done {
            request_id: 1,
            cancelled: false,
            stats: Stats {
                samples_done: 8,
                requested_cadence_ms: 0,
                effective_cadence_ms: 0,
            },
        }),
        None,
    )
    .unwrap();

    let mut cursor = std::io::Cursor::new(buf);
    let mut events = Vec::new();
    loop {
        let (event, _payload) = read_stream_event(&mut cursor).unwrap();
        let done = matches!(event, StreamEvent::Done(_));
        events.push(event);
        if done {
            break;
        }
    }
    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], StreamEvent::Frame(_)));
    assert!(matches!(events[1], StreamEvent::Frame(_)));
    assert!(matches!(events[2], StreamEvent::Progress(_)));
    assert!(matches!(events[3], StreamEvent::Done(_)));
}

/// Every v14 event round-trips through `write_stream_event`/`read_stream_event`, the
/// payload-carrying ones together with their raw payload frame.
#[test]
fn v14_stream_events_round_trip_with_and_without_payloads() {
    use crate::messages::{DisplayEncoding, PayloadEncoding, RenderCapability};

    let payload = vec![7u8; 40];
    let with_payload = [
        StreamEvent::Frame(FrameHeader {
            request_id: 2,
            first_sample: 0,
            samples: 8,
            payload_len: 40,
            encoding: PayloadEncoding::ShuffleZstd { level: 1 },
            raw_len: 48,
        }),
        StreamEvent::Preview(PreviewHeader {
            request_id: 2,
            width: 2,
            height: 2,
            samples_done: 8,
            payload_len: 40,
            encoding: PayloadEncoding::ShuffleLz4,
            raw_len: 48,
        }),
        StreamEvent::DisplayFrame(DisplayFrameHeader {
            request_id: 2,
            samples_done: 8,
            width: 5,
            height: 2,
            encoding: DisplayEncoding::Rgba8,
            payload_len: 40,
        }),
        StreamEvent::FinalImage(FinalImageHeader {
            request_id: 2,
            width: 5,
            height: 2,
            samples_done: 8,
            encoding: DisplayEncoding::Png,
            payload_len: 40,
        }),
    ];
    for event in with_payload {
        assert_eq!(event.payload_len(), Some(40));
        assert_eq!(event.request_id(), Some(2));
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, Some(&payload)).unwrap();
        let (decoded, decoded_payload) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(decoded_payload.as_deref(), Some(payload.as_slice()));
    }

    let without_payload = [
        StreamEvent::Pong {
            nonce: 0xDEAD_BEEF_0000_0001,
        },
        StreamEvent::CapabilityChanged { render: None },
        StreamEvent::CapabilityChanged {
            render: Some(RenderCapability {
                backend: crate::messages::Backend::Coordinator {
                    workers: 2,
                    threads: 32,
                    gpus: 1,
                },
                max_pixels: 33_177_600,
                min_cadence_ms: 100,
                hdr: false,
            }),
        },
        StreamEvent::NeedAsset {
            content_hash: [0xAB; 32],
        },
    ];
    for event in without_payload {
        assert_eq!(event.payload_len(), None);
        assert_eq!(event.request_id(), None);
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, None).unwrap();
        let (decoded, decoded_payload) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(decoded_payload, None);
    }
}

/// Pins the `StreamEvent` discriminants: the five v13 variants keep 0..=4 and the v14
/// ones are appended in order.
#[test]
fn stream_event_discriminants_are_append_only() {
    let first_byte = |e: &StreamEvent| postcard::to_allocvec(e).unwrap()[0];
    assert_eq!(
        first_byte(&StreamEvent::Frame(FrameHeader::for_payload(1, 0, 1, &[]))),
        0
    );
    assert_eq!(
        first_byte(&StreamEvent::Preview(PreviewHeader::for_payload(
            1,
            0,
            0,
            0,
            &[]
        ))),
        1
    );
    assert_eq!(
        first_byte(&StreamEvent::Progress(Progress {
            request_id: 1,
            samples_done: 0
        })),
        2
    );
    assert_eq!(
        first_byte(&StreamEvent::Error(ErrorMsg {
            code: 1,
            message: String::new()
        })),
        4
    );
    assert_eq!(first_byte(&StreamEvent::Pong { nonce: 1 }), 5);
    assert_eq!(
        first_byte(&StreamEvent::CapabilityChanged { render: None }),
        8
    );
    assert_eq!(
        first_byte(&StreamEvent::NeedAsset {
            content_hash: [0; 32]
        }),
        9
    );
}

/// `Asset` (HDR maps) is appended after `FinalImageRequest` (index 6), and the v14
/// scene environment keeps `Studio` at 0 with `Hdr` appended at 1.
#[cfg(feature = "render")]
#[test]
fn asset_and_scene_environment_discriminants_are_pinned() {
    use crate::{
        messages::AssetHeader,
        scene::{HdrEnvironment, SceneEnvironment},
    };
    let asset = ClientMessage::Asset(AssetHeader {
        content_hash: [3; 32],
        len: 1024,
    });
    assert_eq!(postcard::to_allocvec(&asset).unwrap()[0], 6);
    let mut buf = Vec::new();
    write_message(&mut buf, &asset).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, asset);

    assert_eq!(
        postcard::to_allocvec(&SceneEnvironment::Studio).unwrap(),
        [0]
    );
    let hdr = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [5; 32],
        width: 2048,
        height: 1024,
    });
    assert_eq!(postcard::to_allocvec(&hdr).unwrap()[0], 1);
}

/// `Ping` and `FinalImageRequest` are appended after `TiltCurvesRequest` (indices 4/5)
/// and round-trip.
#[cfg(feature = "render")]
#[test]
fn ping_and_final_image_request_round_trip_at_discriminants_4_and_5() {
    use crate::messages::{FinalImageRequest, FinalOutput, WireColorSpace};
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    let ping = ClientMessage::Ping { nonce: 77 };
    let ping_bytes = postcard::to_allocvec(&ping).unwrap();
    assert_eq!(ping_bytes[0], 4, "Ping must be discriminant 4");
    let mut buf = Vec::new();
    write_message(&mut buf, &ping).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, ping);

    let scene = crate::scene::SceneState {
        width: 2,
        height: 2,
        yaw: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: 0.0,
        light_pitch: 0.0,
        exposure: 1.0,
        max_bounces: 1,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: crate::scene::SceneEnvironment::Studio,
    };
    let request = ClientMessage::FinalImageRequest(Box::new(FinalImageRequest {
        request_id: 9,
        scene,
        first_sample: 0,
        samples: 16,
        width: 2,
        height: 2,
        color_space: WireColorSpace::DisplayP3,
        output: FinalOutput::PngRgba8,
    }));
    let request_bytes = postcard::to_allocvec(&request).unwrap();
    assert_eq!(
        request_bytes[0], 5,
        "FinalImageRequest must be discriminant 5"
    );
    let mut buf = Vec::new();
    write_message(&mut buf, &request).unwrap();
    let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(buf)).unwrap();
    assert_eq!(decoded, request);
}

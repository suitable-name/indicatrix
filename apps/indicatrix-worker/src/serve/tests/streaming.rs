//! Tests for progressive streaming's basic shape: delta `FRAME`s tiling the requested
//! sample range with no gaps or overlaps, `PROGRESS` still going out under
//! `FinalOnly`, and a `PREVIEW` payload never being decodable at the full-resolution
//! `FRAME` shape.

use super::fixtures::{
    DuplexHalf, final_only, heavier_scene, live_progressive, read_stream_until_done, test_db,
    tiny_scene,
};
use crate::serve::handle_connection;
use indicatrix_net::{
    handshake,
    messages::{ClientMessage, Done, RenderRequest, StreamEvent, Welcome},
};
use std::io::Cursor;

#[test]
fn delta_frames_tile_the_requested_sample_range_with_no_gaps_or_overlaps() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 11,
        scene: heavier_scene(),
        first_sample: 100,
        samples: 64,
        stream: live_progressive(0), // due on every emitter tick
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let mut ranges: Vec<(u32, u32)> = events
        .iter()
        .filter_map(|(e, _)| match e {
            StreamEvent::Frame(h) => Some((h.first_sample, h.samples)),
            _ => None,
        })
        .collect();
    assert!(!ranges.is_empty(), "expected at least one FRAME delta");
    ranges.sort_by_key(|r| r.0);

    let mut cursor = 100u32;
    for (first, samples) in &ranges {
        assert_eq!(
            *first, cursor,
            "gap or overlap: expected next delta to start at {cursor}, got {first}"
        );
        cursor += samples;
    }
    assert_eq!(
        cursor,
        100 + 64,
        "deltas must tile the whole requested range exactly"
    );

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

#[test]
fn final_only_transfer_mode_still_emits_progress() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 12,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let frame_count = events
        .iter()
        .filter(|(e, _)| matches!(e, StreamEvent::Frame(_)))
        .count();
    assert_eq!(
        frame_count, 1,
        "FinalOnly must send exactly one FRAME, covering the whole request"
    );

    let progress_count = events
        .iter()
        .filter(|(e, _)| matches!(e, StreamEvent::Progress(_)))
        .count();
    assert!(
        progress_count >= 1,
        "FinalOnly must still emit PROGRESS on the cadence even though FRAME only arrives once"
    );

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

#[test]
fn preview_frames_never_enter_the_full_resolution_accumulator_path() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    let scene = tiny_scene(); // 4x4
    let mut stream_cfg = live_progressive(0);
    stream_cfg.preview = Some(indicatrix_net::messages::PreviewConfig {
        width: 2,
        height: 2,
    });
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 31,
        scene: scene.clone(),
        first_sample: 0,
        samples: 8,
        stream: stream_cfg,
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    let previews: Vec<_> = events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Preview(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert!(
        !previews.is_empty(),
        "expected at least one PREVIEW with a preview configured"
    );

    for (header, payload) in &previews {
        assert_eq!(header.width, 2);
        assert_eq!(header.height, 2);

        // Decoding it against the scene's full resolution must fail with a length
        // mismatch -- a PREVIEW payload can never be silently fed into the FRAME path.
        let full_res_attempt = indicatrix_net::radiance::decode(payload, scene.width, scene.height);
        assert!(
            matches!(
                full_res_attempt,
                Err(indicatrix_net::radiance::RadianceError::LengthMismatch { .. })
            ),
            "{full_res_attempt:?}"
        );

        // It DOES decode correctly at its own declared (reduced) resolution -- it's
        // valid radiance data, just never for the full-resolution accumulator.
        assert!(indicatrix_net::radiance::decode(payload, header.width, header.height).is_ok());
    }
}

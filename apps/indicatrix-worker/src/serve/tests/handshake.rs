//! Tests for the `HELLO`/`WELCOME` handshake: build-hash matching, the library-only
//! pairing for an unknown build hash, and that a validation failure keeps the connection
//! open for the next request.

use super::fixtures::{DuplexHalf, final_only, read_stream_until_done, test_db, tiny_scene};
use crate::serve::{
    connection::{BUILD_MISMATCH_CODE, NO_RENDER_CAPACITY_CODE, VALIDATION_FAILED_CODE},
    handle_connection,
};
use indicatrix_net::{
    handshake,
    messages::{
        Backend, ClientMessage, Done, ErrorMsg, Hello, RenderRequest, StreamEvent, Welcome,
    },
};
use std::io::Cursor;

#[test]
fn handle_connection_refuses_a_mismatched_build_hash() {
    let mut input = Vec::new();
    let bad_hello = Hello::viewer(
        indicatrix_net::messages::PROTOCOL_VERSION,
        [0xAB; 8],
        handshake::UNKNOWN_BUILD_HASH,
    );
    indicatrix_net::messages::write_message(&mut input, &bad_hello).unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let err: ErrorMsg = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert_eq!(err.code, BUILD_MISMATCH_CODE);
    assert!(err.message.contains("refusing to pair"), "{}", err.message);
}

#[test]
fn handle_connection_accepts_a_matching_build_hash_and_sends_welcome() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    // No RenderRequest follows -- the reader hits EOF, which handle_connection
    // treats as the peer having closed the connection after the handshake.

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert_eq!(welcome.build_hash, handshake::local_hello().build_hash);
    assert!(matches!(
        welcome.render.as_ref().unwrap().backend,
        Backend::Cpu { .. }
    ));
}

/// A `HELLO` declaring no `indicatrix` build at all
/// (`build_hash == UNKNOWN_BUILD_HASH`, e.g. a library-only client) must not be refused
/// by `verify_compatible`'s "an unknown build is never compatible with anything" rule --
/// it is paired as library-only instead: `WELCOME::render` is `None`, and a later
/// `RenderRequest` on that same connection is refused with `NO_RENDER_CAPACITY_CODE`
/// rather than reaching the tracer.
#[test]
fn handle_connection_pairs_an_unknown_build_hash_as_library_only_and_refuses_a_later_render() {
    let mut input = Vec::new();
    let library_only_hello = Hello::viewer(
        indicatrix_net::messages::PROTOCOL_VERSION,
        handshake::UNKNOWN_BUILD_HASH,
        handshake::UNKNOWN_BUILD_HASH,
    );
    indicatrix_net::messages::write_message(&mut input, &library_only_hello).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 1,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    assert!(
        welcome.render.is_none(),
        "an unknown-build-hash peer must be welcomed with no render capacity"
    );
    assert!(welcome.library, "the library protocol stays available");
    assert!(!welcome.tilt_curves);

    let events = read_stream_until_done(&mut out_cursor);
    assert_eq!(
        events.len(),
        1,
        "a RenderRequest on a library-only-paired connection gets exactly one reply"
    );
    let StreamEvent::Error(err) = &events[0].0 else {
        panic!("expected StreamEvent::Error, got {:?}", events[0].0);
    };
    assert_eq!(err.code, NO_RENDER_CAPACITY_CODE);
}

#[test]
fn handle_connection_rejects_a_request_that_fails_validation_but_keeps_the_connection_open() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    let mut bad_scene = tiny_scene();
    bad_scene.planes[0].normal = [f32::NAN, 0.0, 0.0];
    let bad_request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 1,
        scene: bad_scene,
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(bad_request)),
    )
    .unwrap();

    // A second, well-formed request right after the bad one -- proves the
    // connection is still alive and serving requests after a validation failure.
    let good_request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 2,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(good_request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 1, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();

    // The bad request's reply: a single StreamEvent::Error, nothing else.
    let bad_events = read_stream_until_done(&mut out_cursor);
    assert_eq!(bad_events.len(), 1);
    let StreamEvent::Error(err) = &bad_events[0].0 else {
        panic!("expected StreamEvent::Error, got {:?}", bad_events[0].0);
    };
    assert_eq!(err.code, VALIDATION_FAILED_CODE);

    // The good request's reply: FinalOnly, so exactly one Frame (with correct
    // request_id and sample count) followed by Done { cancelled: false }.
    let good_events = read_stream_until_done(&mut out_cursor);
    let frames: Vec<_> = good_events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 1);
    let (header, payload) = frames[0];
    assert_eq!(header.request_id, 2);
    assert_eq!(header.samples, 2);
    assert_eq!(
        payload.len(),
        4 * 4 * indicatrix_net::radiance::BYTES_PER_PIXEL
    );
    assert!(matches!(
        good_events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));
}

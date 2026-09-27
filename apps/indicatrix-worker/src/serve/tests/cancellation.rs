//! Tests for cancellation and the write-timeout bound it relies on: a mid-stream
//! `CANCEL` producing `DONE { cancelled: true }` with no further payload, a peer that
//! never drains ending the connection rather than hanging forever (both `TimedOut` and
//! `WriteZero` timeout kinds), stale `request_id`s being identifiable, and a pipelined
//! `RenderRequest` being queued as an implicit cancel.

use super::fixtures::{
    BackpressureDuplex, DuplexHalf, event_request_id, final_only, heavier_scene, live_progressive,
    read_stream_until_done, test_db, tiny_scene,
};
use crate::serve::handle_connection;
use indicatrix_net::{
    handshake,
    messages::{Cancel, ClientMessage, Done, RenderRequest, StreamEvent, Welcome},
};
use std::io::Cursor;

#[test]
fn cancel_mid_stream_produces_done_cancelled_and_no_further_payload() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 13,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::Cancel(Cancel { request_id: 13 }),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    for (event, _) in &events {
        assert_eq!(event_request_id(event), 13);
    }
    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: true,
            ..
        })
    ));

    // No further payload: handle_connection's outer loop went straight back to
    // reading the next RenderRequest and hit EOF, writing nothing more.
    assert_eq!(
        out_cursor.position(),
        out_cursor.get_ref().len() as u64,
        "no bytes may follow DONE{{cancelled: true, ..}}"
    );
}

/// Why this passes while the real bug (`repro::repro_slow_reader_blocks_the_emitter_and_delays_cancel`,
/// in the sibling `repro` module) doesn't reproduce on `DuplexHalf`: `CANCEL` is already
/// sitting in the scripted input at time zero, and `DuplexHalf::write`'s unbounded `Vec`
/// can never block. The real bug needs `CANCEL` arriving *while* the emitter is stuck
/// inside a blocked `write()` -- `BackpressureDuplex` supplies that.
#[test]
fn a_peer_that_never_drains_ends_the_connection_rather_than_hanging_forever() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 21,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    // Tiny on purpose: the WELCOME handshake write always gets through regardless of
    // capacity (see BackpressureDuplex's doc comment), so this only needs to be
    // smaller than the first FRAME/PROGRESS write once streaming starts.
    let mut duplex = BackpressureDuplex::new(input, 8);
    let result = handle_connection(&mut duplex, 2, &test_db());

    // The bound this closes: without WRITE_TIMEOUT this would hang forever; instead
    // it returns an error within one WRITE_TIMEOUT.
    let err = result.expect_err(
        "a peer that never drains must end the connection with an error, not hang forever",
    );
    assert!(
        matches!(
            err,
            indicatrix_net::messages::NetError::Framing(indicatrix_net::framing::FramingError::Io(ref e))
                if e.kind() == std::io::ErrorKind::TimedOut
        ),
        "expected the write-timeout error to surface as-is, got {err:?}"
    );
}

/// The same bound as the test above, but against a peer that reports `WriteZero`
/// instead of `TimedOut` -- what a real `rustls` `StreamOwned` reports for an
/// underlying write timeout (see `crate::stream_emit::is_stream_timeout`). Nothing in
/// `run_stream`'s write path classifies by kind before propagating, so this must
/// behave identically: an error, not a hang, with `WriteZero` preserved as-is.
#[test]
fn a_peer_that_never_drains_ends_the_connection_rather_than_hanging_forever_write_zero() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 23,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = BackpressureDuplex::new_with_kind(input, 8, std::io::ErrorKind::WriteZero);
    let result = handle_connection(&mut duplex, 2, &test_db());

    let err = result.expect_err(
        "a peer that never drains must end the connection with an error, not hang forever, \
         regardless of which io::ErrorKind the write failure carries",
    );
    assert!(
        matches!(
            err,
            indicatrix_net::messages::NetError::Framing(indicatrix_net::framing::FramingError::Io(ref e))
                if e.kind() == std::io::ErrorKind::WriteZero
        ),
        "expected the WriteZero error to surface as-is, got {err:?}"
    );
}

/// The positive case: a peer with plenty of room to drain into never sees a write
/// timeout, and `CANCEL` still produces `DONE{cancelled: true}` and no further
/// payload -- proving `WRITE_TIMEOUT` bounds the pathological case above without
/// punishing a merely-finite, still-draining peer.
#[test]
fn cancel_still_completes_cleanly_against_a_finite_but_sufficient_peer() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
    let request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 22,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::Cancel(Cancel { request_id: 22 }),
    )
    .unwrap();

    // Generous: comfortably more than everything this tiny 24x24 request could
    // possibly produce before CANCEL is observed.
    let mut duplex = BackpressureDuplex::new(input, 1_000_000);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
    let events = read_stream_until_done(&mut out_cursor);

    assert!(matches!(
        events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: true,
            ..
        })
    ));
}

#[test]
fn stale_request_id_frames_are_identifiable() {
    // Two separate connections: a client that moved on to a new request_id (via any
    // means) can mechanically recognize a reply carrying the old one as stale. The
    // pipelined-on-one-connection scenario gets its own test below.
    fn run_one_request(request_id: u32) -> Vec<(StreamEvent, Option<Vec<u8>>)> {
        let mut input = Vec::new();
        indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();
        let request = RenderRequest {
            intent: indicatrix_net::messages::RequestIntent::Batch,
            request_id,
            scene: tiny_scene(),
            first_sample: 0,
            samples: 4,
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
        let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();
        read_stream_until_done(&mut out_cursor)
    }

    let first_events = run_one_request(21);
    let second_events = run_one_request(22);

    // A client tracking "current epoch = 22" can identify every event belonging to
    // the first, now-stale request_id as one to drop.
    assert_ne!(first_events.len(), 0);
    for (event, _) in &first_events {
        assert_eq!(event_request_id(event), 21);
        assert_ne!(event_request_id(event), 22);
    }
    assert_ne!(second_events.len(), 0);
    for (event, _) in &second_events {
        assert_eq!(event_request_id(event), 22);
    }
}

/// The scenario `stream_emit::run_stream`'s pipelining support exists for: a client
/// writes its next `RenderRequest` right behind the first, on the same connection,
/// without reading `DONE` for the first one. Without that support this is the failure
/// mode `poll_for_client_message` warns about (spurious error, connection torn down) --
/// `handle_connection` returning `Ok(())` is itself part of what this test proves.
#[test]
fn pipelined_render_request_on_one_connection_is_queued_after_an_implicit_cancel() {
    let mut input = Vec::new();
    indicatrix_net::messages::write_message(&mut input, &handshake::local_hello()).unwrap();

    // Heavy enough that the emitter gets multiple chances to poll (and observe the
    // pipelined request) before the tracer finishes on its own.
    let first_request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 41,
        scene: heavier_scene(),
        first_sample: 0,
        samples: 64,
        stream: live_progressive(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(first_request)),
    )
    .unwrap();

    // Written immediately behind the first -- no DONE read in between. A different
    // (smaller) scene so a leaked buffer from the first request would show up as a
    // payload-size mismatch rather than silently passing.
    let second_request = RenderRequest {
        intent: indicatrix_net::messages::RequestIntent::Batch,
        request_id: 42,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 4,
        stream: final_only(0),
    };
    indicatrix_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(second_request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    handle_connection(&mut duplex, 2, &test_db()).unwrap();

    let mut out_cursor = Cursor::new(duplex.out);
    let _welcome: Welcome = indicatrix_net::messages::read_message(&mut out_cursor).unwrap();

    // The first request's reply: implicitly cancelled, like an explicit CANCEL --
    // every event carries request_id 41, ending in DONE { cancelled: true }.
    let first_events = read_stream_until_done(&mut out_cursor);
    for (event, _) in &first_events {
        assert_eq!(event_request_id(event), 41);
    }
    assert!(
        matches!(
            first_events.last().unwrap().0,
            StreamEvent::Done(Done {
                cancelled: true,
                ..
            })
        ),
        "the superseded request must still get a proper DONE {{ cancelled: true }}, \
         not be silently dropped"
    );

    // The pipelined request's reply, immediately following on the same connection.
    // FinalOnly, so exactly one FRAME sized for its own scene, then DONE { cancelled: false }.
    let second_events = read_stream_until_done(&mut out_cursor);
    let frames: Vec<_> = second_events
        .iter()
        .filter_map(|(e, p)| match e {
            StreamEvent::Frame(h) => Some((h, p.as_ref().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(frames.len(), 1);
    let (header, payload) = frames[0];
    assert_eq!(header.request_id, 42);
    assert_eq!(header.samples, 4);
    // 4x4 (tiny_scene), not 24x24 -- proves accumulation started fresh, not
    // inheriting the cancelled first request's buffers.
    assert_eq!(
        payload.len(),
        4 * 4 * indicatrix_net::radiance::BYTES_PER_PIXEL
    );
    for (event, _) in &second_events {
        assert_eq!(event_request_id(event), 42);
    }
    assert!(matches!(
        second_events.last().unwrap().0,
        StreamEvent::Done(Done {
            cancelled: false,
            ..
        })
    ));

    // Nothing beyond the two requests' worth of events was written.
    assert_eq!(
        out_cursor.position(),
        out_cursor.get_ref().len() as u64,
        "no bytes may follow the second request's DONE"
    );
}

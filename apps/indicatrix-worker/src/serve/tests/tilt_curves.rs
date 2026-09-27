//! `TILT_CURVES` leaves the connection usable for a later `LibraryRequest`.

use super::fixtures::{test_db, tiny_scene};
use crate::serve::handle_connection;
use indicatrix_net::{handshake, messages::ClientMessage};
use std::net::TcpListener;

/// `serve::tilt::handle_tilt_curves_request` must restore the short read timeout
/// `poll_for_cancel` applies to the socket
/// (`CANCEL_POLL_TIMEOUT`/`crate::stream_emit::FRAME_REMAINDER_TIMEOUT`) before
/// returning -- otherwise this connection's next `read_message` call fails unless the
/// next frame happens to already be sitting in the socket buffer. Driven over a REAL
/// loopback `TcpStream` (unlike this folder's `DuplexHalf`-based tests, which can never
/// actually time out a read) with the second message written only after the first reply
/// comes back -- exactly the case a leftover short timeout would break.
#[test]
fn tilt_curves_request_then_library_request_both_get_replies_over_a_real_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 2, &test_db()).unwrap();
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    indicatrix_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: indicatrix_net::messages::Welcome =
        indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(welcome.tilt_curves);

    let tilt_request = indicatrix_net::messages::TiltCurvesRequest {
        request_id: 1,
        scene: tiny_scene(),
    };
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::TiltCurvesRequest(Box::new(tilt_request)),
    )
    .unwrap();
    let tilt_response: indicatrix_net::messages::TiltCurvesResponse =
        indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            tilt_response,
            indicatrix_net::messages::TiltCurvesResponse::Curves(_)
        ),
        "{tilt_response:?}"
    );

    // Sent only now -- not already buffered when the TILT_CURVES reply above went out
    // -- so this proves the read timeout the emitter left on the socket was actually
    // restored to blocking, not that a lucky pre-buffered read papered over the bug.
    indicatrix_net::messages::write_message(
        &mut client,
        &ClientMessage::Library(Box::new(
            indicatrix_net::library::LibraryRequest::FilterOptions,
        )),
    )
    .unwrap();
    let library_response: indicatrix_net::library::LibraryResponse =
        indicatrix_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            library_response,
            indicatrix_net::library::LibraryResponse::FilterOptions { .. }
        ),
        "{library_response:?}"
    );

    drop(client);
    server.join().unwrap();
}

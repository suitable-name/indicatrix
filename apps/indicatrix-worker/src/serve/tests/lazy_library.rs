//! The library database opens on a connection's first library request and never for
//! anything else: a connection that only renders never touches it (so a database that
//! cannot be opened costs it nothing), and a library request over such a database gets
//! an error reply while the connection keeps serving renders.

use super::fixtures::{
    DuplexHalf, final_only, read_stream_until_done, test_db_file, tiny_scene, unique_temp_dir,
};
use crate::serve::{
    handle_connection,
    library::{LIBRARY_ERROR_CODE, LibraryHandle},
};
use indicatrix_net::{
    handshake,
    library::{LibraryRequest, LibraryResponse},
    messages::{
        ClientMessage, Done, RenderRequest, RequestIntent, StreamEvent, Welcome, read_message,
        read_stream_event, write_message,
    },
};
use std::{io::Cursor, path::PathBuf};

/// A path no database exists at.
fn absent_database() -> PathBuf {
    unique_temp_dir("lazy-library").join("absent.sqlite")
}

/// A connection's scripted input: the `HELLO`, then `messages` in order.
fn script(messages: &[ClientMessage]) -> DuplexHalf {
    let mut input = Vec::new();
    write_message(&mut input, &handshake::local_hello()).unwrap();
    for message in messages {
        write_message(&mut input, message).unwrap();
    }
    DuplexHalf::new(input)
}

fn render_request(request_id: u32) -> ClientMessage {
    ClientMessage::RenderRequest(Box::new(RenderRequest {
        intent: RequestIntent::Batch,
        request_id,
        scene: tiny_scene(),
        first_sample: 0,
        samples: 2,
        stream: final_only(0),
    }))
}

fn library_request() -> ClientMessage {
    ClientMessage::Library(Box::new(LibraryRequest::FilterOptions))
}

/// Asserts that `events` are one finished render of `request_id`: a `FRAME`, then a
/// `DONE` that was not cancelled.
fn assert_rendered(events: &[(StreamEvent, Option<Vec<u8>>)], request_id: u32) {
    assert!(
        events.iter().any(
            |(event, _)| matches!(event, StreamEvent::Frame(header) if header.request_id == request_id)
        ),
        "no FRAME for request {request_id}"
    );
    assert!(
        matches!(
            events.last().unwrap().0,
            StreamEvent::Done(Done {
                cancelled: false,
                ..
            })
        ),
        "request {request_id} did not end in a completed DONE"
    );
}

#[test]
fn a_connection_that_never_asks_the_library_never_opens_it() {
    let library = LibraryHandle::lazy(absent_database(), None);
    let mut duplex = script(&[ClientMessage::Ping { nonce: 7 }, render_request(1)]);
    handle_connection(&mut duplex, 1, &library).unwrap();

    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = read_message(&mut out).unwrap();
    let (pong, _) = read_stream_event(&mut out).unwrap();
    assert!(matches!(pong, StreamEvent::Pong { nonce: 7 }), "{pong:?}");
    assert_rendered(&read_stream_until_done(&mut out), 1);
    assert!(
        !library.is_open(),
        "a ping and a render must not open the library database"
    );
}

#[test]
fn a_library_request_over_an_unopenable_database_gets_an_error_and_rendering_goes_on() {
    let library = LibraryHandle::lazy(absent_database(), None);
    let mut duplex = script(&[library_request(), render_request(2)]);
    handle_connection(&mut duplex, 1, &library).unwrap();

    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = read_message(&mut out).unwrap();
    let response: LibraryResponse = read_message(&mut out).unwrap();
    let LibraryResponse::Error(error) = response else {
        panic!("expected LibraryResponse::Error, got {response:?}");
    };
    assert_eq!(error.code, LIBRARY_ERROR_CODE);
    assert_rendered(&read_stream_until_done(&mut out), 2);
    assert!(!library.is_open());
}

#[test]
fn the_first_library_request_opens_the_database_and_later_ones_reuse_it() {
    let path = test_db_file();
    let library = LibraryHandle::lazy(path.clone(), None);
    assert!(!library.is_open(), "building the handle opens nothing");
    let mut duplex = script(&[library_request(), library_request()]);
    handle_connection(&mut duplex, 1, &library).unwrap();

    let mut out = Cursor::new(duplex.out);
    let _welcome: Welcome = read_message(&mut out).unwrap();
    for _ in 0..2 {
        let response: LibraryResponse = read_message(&mut out).unwrap();
        assert!(
            matches!(response, LibraryResponse::FilterOptions { .. }),
            "{response:?}"
        );
    }
    assert!(library.is_open());

    drop(library);
    std::fs::remove_file(&path).ok();
}

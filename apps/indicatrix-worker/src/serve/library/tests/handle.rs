//! Tests for [`LibraryHandle`]: the database opens on the first library request and not
//! before, a handle around an open database never reopens it, and a failed open is an
//! error reply that the next request retries.

use super::fixtures::populated_temp_db;
use crate::serve::library::{LIBRARY_ERROR_CODE, LibraryHandle};
use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use indicatrix_vault::db::sqlite::Database;

#[test]
fn a_lazy_handle_opens_the_database_on_its_first_request_and_keeps_it() {
    let path = populated_temp_db();
    let library = LibraryHandle::lazy(path.clone(), None);
    assert!(!library.is_open(), "building the handle opens nothing");

    let response = library.handle(&LibraryRequest::FilterOptions);
    assert!(
        matches!(response, LibraryResponse::FilterOptions { .. }),
        "{response:?}"
    );
    assert!(library.is_open());

    let response = library.handle(&LibraryRequest::FetchDesign { entry_id: 1 });
    assert!(
        matches!(response, LibraryResponse::Design(_)),
        "{response:?}"
    );

    drop(library);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_handle_around_an_open_database_serves_it_without_a_path() {
    let path = populated_temp_db();
    let db = Database::open_read_only(path.to_str().unwrap()).unwrap();
    let library = LibraryHandle::open(db);
    assert!(library.is_open());

    let response = library.handle(&LibraryRequest::FetchDesign { entry_id: 1 });
    assert!(
        matches!(response, LibraryResponse::Design(_)),
        "{response:?}"
    );

    drop(library);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_handle_over_a_missing_file_answers_an_error_and_retries_on_the_next_request() {
    let path = std::env::temp_dir().join(format!(
        "indicatrix-worker-lazy-library-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let library = LibraryHandle::lazy(path.clone(), None);

    let response = library.handle(&LibraryRequest::FilterOptions);
    let LibraryResponse::Error(error) = response else {
        panic!("expected LibraryResponse::Error, got {response:?}");
    };
    assert_eq!(error.code, LIBRARY_ERROR_CODE);
    assert!(
        !error.message.contains(path.to_str().unwrap()),
        "the peer must not see the server's path: {}",
        error.message
    );
    assert!(!library.is_open());
    assert!(!path.exists(), "a read-only open never creates the file");

    // The file appears (a restore, a late mount): the same handle opens it now.
    drop(Database::new(Some(path.to_str().unwrap())).unwrap());
    let response = library.handle(&LibraryRequest::FilterOptions);
    assert!(
        matches!(response, LibraryResponse::FilterOptions { .. }),
        "{response:?}"
    );
    assert!(library.is_open());

    drop(library);
    std::fs::remove_file(&path).ok();
}

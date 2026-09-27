//! Tests for the library-search freeze fix's staleness mechanism -- mirrors
//! `gui::editor::auto_solve`'s own `current_seq`/`is_current` tests exactly, same
//! shape and same reasoning.

use super::dispatch::{bump_search_seq, is_current_search};

#[test]
fn a_freshly_bumped_sequence_number_is_current() {
    let seq = bump_search_seq();
    assert!(is_current_search(seq));
}

#[test]
fn an_older_sequence_number_is_superseded_by_a_newer_bump() {
    let old_seq = bump_search_seq();
    // A second call bumps the counter again, exactly like a later
    // `refresh_diagram_list` call (sync or async) would.
    bump_search_seq();
    assert!(!is_current_search(old_seq));
}

#[test]
fn a_sync_refresh_supersedes_an_older_pending_async_dispatch() {
    // The exact race `SEARCH_SEQ`'s own doc comment describes: an async dispatch
    // captured `async_seq`, then the cutter cleared the performance filter before
    // it completed, so a plain `bump_search_seq()` call (the sync path's own,
    // unconditional bump -- see `refresh_diagram_list`) must supersede it even
    // though the sync path itself never reads `SEARCH_SEQ` again afterwards.
    let async_seq = bump_search_seq();
    assert!(is_current_search(async_seq));
    bump_search_seq(); // stands in for the sync path's own unconditional bump
    assert!(
        !is_current_search(async_seq),
        "a sync refresh must supersede an older async dispatch's sequence number"
    );
}

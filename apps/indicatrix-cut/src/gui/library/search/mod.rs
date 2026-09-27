//! Searching, filtering and sorting the design library.
//!
//! Covers reading the panel's own filter state ([`filters`]), fetching/applying a local result
//! ([`fetch`]), the debounced background dispatch for a slow, performance-filtered
//! local query ([`dispatch`]), and the remote counterpart of the same search
//! ([`remote_search`]).

mod dispatch;
mod fetch;
mod filters;
mod remote_search;
#[cfg(test)]
mod tests;

pub use dispatch::refresh_diagram_list;
pub(in crate::gui) use dispatch::{bump_search_seq, is_current_search};
pub use fetch::{
    DiagramListRow, DisplaySearchOptions, FetchedDiagramList, apply_diagram_list_to_ui,
    fetch_diagram_list, fetch_diagram_list_with_options,
};
pub use filters::{
    current_filtered_entry_ids, read_id_filter, read_local_only, read_range_filter,
    read_sort_order, read_tag_filter,
};
pub(in crate::gui) use remote_search::refresh_diagram_list_remote;

use crate::{MainWindow, bridge::library::source::LibrarySource};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

/// Dispatches to [`refresh_diagram_list`] or a background remote search.
///
/// Every search/filter callback in this crate calls this instead of `refresh_diagram_list`
/// directly and doesn't itself need to know which source is active.
pub fn refresh_diagram_list_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => {
            refresh_diagram_list(ui, db_mutex, search, shape_filter, gear_filter);
        }
        LibrarySource::Remote(worker) => {
            refresh_diagram_list_remote(ui, worker, search, shape_filter, gear_filter);
        }
    }
}

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

use crate::{LibraryModel, MainWindow, bridge::library::source::LibrarySource};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, SharedString};
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

/// [`refresh_diagram_list_via_source`] for a keystroke in the search box.
///
/// The search runs once the typing pauses (see [`dispatch::debounce_text_refresh`]), not
/// once per character. The shape and gear dropdown texts are the ones showing when the
/// key was pressed.
///
/// The deferred search is dropped, not run, when the box or either dropdown no longer
/// shows what this call was given: a programmatic reset or a dropdown change in the
/// meantime has already refreshed with the current values. A refresh that runs sooner
/// for any other reason supersedes it too (see `refresh_diagram_list`).
pub fn refresh_diagram_list_via_source_debounced(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    let ui_weak = ui.as_weak();
    let db_mutex = Arc::clone(db_mutex);
    let source = Arc::clone(source);
    let search = search.to_string();
    let shape_filter = shape_filter.to_string();
    let gear_filter = gear_filter.to_string();
    dispatch::debounce_text_refresh(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let (shown_shape, shown_gear) = selected_shape_and_gear(&ui);
        if ui.global::<LibraryModel>().get_search_text().as_str() != search
            || shown_shape.as_str() != shape_filter
            || shown_gear.as_str() != gear_filter
        {
            return;
        }
        refresh_diagram_list_via_source(
            &ui,
            &db_mutex,
            &source,
            &search,
            &shape_filter,
            &gear_filter,
        );
    });
}

/// The shape and gear dropdown texts `ui` currently shows selected (empty for an index
/// outside the option lists).
fn selected_shape_and_gear(ui: &MainWindow) -> (SharedString, SharedString) {
    let library = ui.global::<LibraryModel>();
    let shape = library
        .get_shape_options()
        .row_data(library.get_selected_shape_index() as usize)
        .unwrap_or_default();
    let gear = library
        .get_gear_options()
        .row_data(library.get_selected_gear_index() as usize)
        .unwrap_or_default();
    (shape, gear)
}

//! Shared local-library helpers: re-running the catalogue query after any write
//! (import/rename/delete/shape-change) -- [`import`], [`organize`], and [`export`]
//! (see this group's own `mod.rs`) all call [`refresh_after_library_change`] as their
//! last step.
//!
//! [`import`]: super::import
//! [`organize`]: super::organize
//! [`export`]: super::export

use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::library::{
        diagram_list::{apply_attribute_range_bounds_preserving_filters, fetch_attribute_ranges},
        search::{
            DisplaySearchOptions, apply_diagram_list_to_ui, bump_search_seq,
            fetch_diagram_list_with_options, is_current_search, read_id_filter, read_local_only,
            read_range_filter, read_sort_order, read_tag_filter,
        },
    },
};
use indicatrix_vault::db::sqlite::{Database, SortOrder};
use slint::{ComponentHandle, Model, Weak};
use std::{
    sync::{Arc, Mutex},
    thread,
};
use tracing::warn;

/// Re-reads the search/shape/gear filters currently showing in `ui` and re-runs the
/// catalogue query -- the shared "refresh everything that could have changed" step
/// used by import/rename/delete/shape-change.
///
/// Import/rename/delete only ever touch the LOCAL database (rename/delete refuse
/// outright while browsing remote -- see `setup_rename_callback`/`setup_delete_callback`'s
/// own doc comments; import always writes locally regardless of which library is being
/// browsed). So the attribute-range refresh -- which reads local attribute ranges --
/// only happens here while [`LibrarySource::Local`] is actually active; a currently-Remote
/// view instead goes through [`crate::gui::library::search::refresh_diagram_list_via_source`]
/// (already off the UI thread -- see `search::refresh_diagram_list_remote`) so it stays
/// showing remote results rather than silently switching to local ones after a
/// local-only write.
///
/// The LOCAL path itself runs the two `Database` reads (`fetch_attribute_ranges`,
/// `fetch_diagram_list`) on a background thread and only marshals the resulting
/// `ui.set_*` calls back through `upgrade_in_event_loop` -- unlike every OTHER caller
/// of those same reads (a search-box keystroke, a filter-slider drag), which stay
/// synchronous because that responsiveness is the point. This call site is different:
/// it always follows a database WRITE (import/rename/delete/shape-change), so the
/// small extra latency of a thread hop is invisible, while running the read
/// synchronously here is not -- against the real ~3,187-design catalogue this
/// crate's own `perf_probe_refresh_after_library_change_cost` test measures
/// `get_attribute_ranges` + an unfiltered `search_diagrams` at tens of milliseconds
/// combined, long enough on the UI thread to read as a second freeze right after a big
/// import finishes.
pub fn refresh_after_library_change(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => {
            // A write that can create/empty a tag (add/remove
            // tag) needs the chip filter row's own vocabulary refreshed alongside
            // the list -- cheap enough (one query) to run synchronously here rather
            // than threading it through `spawn_local_refresh`'s worker thread.
            crate::gui::library::diagram_list::sync_tag_vocabulary_to_ui(ui, db);
            let search = ui.global::<LibraryModel>().get_search_text().to_string();
            let shape_idx = ui.global::<LibraryModel>().get_selected_shape_index() as usize;
            let shape = ui
                .global::<LibraryModel>()
                .get_shape_options()
                .row_data(shape_idx)
                .unwrap_or_default()
                .to_string();
            let gear_idx = ui.global::<LibraryModel>().get_selected_gear_index() as usize;
            let gear = ui
                .global::<LibraryModel>()
                .get_gear_options()
                .row_data(gear_idx)
                .unwrap_or_default()
                .to_string();
            // Same reasoning as the filter-preservation step just below: this
            // refresh follows a WRITE, not a request to go back to the default
            // view, so the cutter's own range filters, sort and "My designs"
            // choices all have to survive it. Every one is read here, on the UI
            // thread, since the fetch itself runs on a worker that has no
            // `MainWindow` to read them from.
            //
            // Bumped here, BEFORE the background fetch is even spawned,
            // and carried through as `query.seq` so `spawn_local_refresh`'s own
            // completion closure can check it before touching the UI -- see that
            // function's own doc comment.
            let seq = bump_search_seq();
            let query = DiagramListQuery {
                seq,
                search,
                shape,
                gear,
                range: read_range_filter(ui),
                order: read_sort_order(ui),
                local_only: read_local_only(ui),
                tag_filter: read_tag_filter(ui),
                id_filter: read_id_filter(ui),
            };
            spawn_local_refresh(ui.as_weak(), Arc::clone(db), query);
        }
        LibrarySource::Remote(_) => {
            let search = ui.global::<LibraryModel>().get_search_text();
            let shape_idx = ui.global::<LibraryModel>().get_selected_shape_index() as usize;
            let shape = ui
                .global::<LibraryModel>()
                .get_shape_options()
                .row_data(shape_idx)
                .unwrap_or_default();
            let gear_idx = ui.global::<LibraryModel>().get_selected_gear_index() as usize;
            let gear = ui
                .global::<LibraryModel>()
                .get_gear_options()
                .row_data(gear_idx)
                .unwrap_or_default();
            crate::gui::library::search::refresh_diagram_list_via_source(
                ui, db, source, &search, &shape, &gear,
            );
        }
    }
}

/// Runs [`fetch_attribute_ranges`] + [`fetch_diagram_list`] on their own worker
/// thread and applies both results in one hop back onto the UI thread -- the
/// background half of [`refresh_after_library_change`]'s `LibrarySource::Local`
/// branch; see that function's own doc comment for why this one call site needs to be
/// async where `sync_range_bounds_to_ui`/`refresh_diagram_list`'s other callers don't.
/// Everything the catalogue list is currently filtered and ordered by, as it was
/// on screen at the moment of the write this refresh follows -- see
/// [`spawn_local_refresh`]'s own call site for why it has to be snapshotted on the
/// UI thread rather than re-read (it cannot be) on the worker.
struct DiagramListQuery {
    /// Captured by [`refresh_after_library_change`] at dispatch time -- the
    /// same `bump_search_seq`/`is_current_search` staleness guard
    /// `gui::library::search::dispatch_background_search` already uses for its own
    /// async dispatch.
    seq: u64,
    search: String,
    shape: String,
    gear: String,
    range: indicatrix_vault::model::filter::RangeFilter,
    order: SortOrder,
    local_only: bool,
    tag_filter: Option<String>,
    id_filter: Option<Vec<i64>>,
}

/// A keystroke/filter change that lands while this background refresh is
/// still in flight takes the FAST synchronous path (`refresh_diagram_list`) and
/// displays first; without the [`crate::gui::library::search::is_current_search`]
/// check below, this closure's own (now-stale) results would then land right on top
/// of it moments later, silently reverting the list to the OLD search text/filters.
fn spawn_local_refresh(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    query: DiagramListQuery,
) {
    thread::spawn(move || {
        let seq = query.seq;
        let ranges = fetch_attribute_ranges(&db);
        let list_result = fetch_diagram_list_with_options(
            &db,
            &query.search,
            &query.shape,
            &query.gear,
            &query.range,
            DisplaySearchOptions {
                order: query.order,
                local_only: query.local_only,
                tag_filter: query.tag_filter.as_deref(),
                id_filter: query.id_filter.as_deref(),
            },
        );
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if !is_current_search(seq) {
                return;
            }
            if let Some(ranges) = ranges {
                // This refresh follows a WRITE (import/rename/delete/
                // shape-change), not a request to clear the cutter's own range
                // filters -- see `apply_attribute_range_bounds_preserving_filters`'s
                // own doc comment for why it preserves them rather than resetting.
                apply_attribute_range_bounds_preserving_filters(&ui, &ranges);
            }
            match list_result {
                Ok(fetched) => apply_diagram_list_to_ui(&ui, fetched),
                Err(e) => {
                    warn!("Failed to search diagrams: {e:?}");
                    ui.global::<LibraryModel>()
                        .set_status_message(format!("Error searching database: {e}").into());
                }
            }
        });
    });
}

#[cfg(test)]
pub(super) mod test_support {
    use indicatrix_vault::db::sqlite::Database;
    use std::{
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
    };

    pub(in crate::gui::library::local) const VALID_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

    /// A fresh, empty scratch directory under the OS temp dir -- same naming
    /// convention `indicatrix_vault::db::sqlite::tests::temp_db_path` uses for its own
    /// throwaway files, extended to a directory here.
    pub(in crate::gui::library::local) fn temp_dir_for_test(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "indicatrix_cut_test_{label}_{n}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    /// A fresh path for a throwaway SQLite file -- never the real
    /// `facet_diagrams.sqlite` (this crate's domain rules forbid that outright); every
    /// test below builds its own temp database via [`Database::new`].
    pub(in crate::gui::library::local) fn temp_db_path_for_test(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "indicatrix_cut_test_db_{label}_{n}_{}.sqlite",
            std::process::id()
        ))
    }

    pub(in crate::gui::library::local) fn open_temp_db(
        path: &std::path::Path,
    ) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::new(Some(path.to_str().expect("temp path is valid UTF-8")))
                .expect("create fresh temp test db"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use indicatrix_vault::db::sqlite::Database;
    use std::path::Path;

    /// Manual perf probe: `refresh_after_library_change`
    /// re-queries the whole catalogue synchronously on the UI thread after every
    /// import. Opens the user's real ~3,187-design `facet_diagrams.sqlite`
    /// READ-ONLY (never as a test fixture to write into -- see this crate's own
    /// domain rules) and times exactly the two calls that completion path makes:
    /// `get_attribute_ranges` (`sync_range_bounds_to_ui`) and an unfiltered
    /// `search_diagrams` (`refresh_diagram_list`). `#[ignore]`d: it depends on a file
    /// that only exists on this workstation, so it must never run in CI; run
    /// explicitly with `cargo test -p indicatrix-cut -- --ignored perf_probe --nocapture`.
    #[test]
    #[ignore = "manual perf probe against the real local catalogue, not for CI"]
    fn perf_probe_refresh_after_library_change_cost() {
        let real_db_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../facet_diagrams.sqlite");
        if !Path::new(real_db_path).is_file() {
            eprintln!("skipping: {real_db_path} not found on this machine");
            return;
        }
        let db = Database::open_read_only(real_db_path).expect("open real catalogue read-only");

        let t0 = std::time::Instant::now();
        let ranges = db.get_attribute_ranges().expect("get_attribute_ranges");
        let ranges_elapsed = t0.elapsed();

        let range = indicatrix_vault::model::filter::RangeFilter::default();
        let t1 = std::time::Instant::now();
        let items = db
            .search_diagrams("", "All", "All", &range)
            .expect("search_diagrams");
        let search_elapsed = t1.elapsed();

        let t2 = std::time::Instant::now();
        let total = db.get_total_count().expect("get_total_count");
        let count_elapsed = t2.elapsed();

        eprintln!(
            "get_attribute_ranges: {ranges_elapsed:?} ({ranges:?})\n\
             search_diagrams (unfiltered): {search_elapsed:?} ({} rows)\n\
             get_total_count: {count_elapsed:?} ({total} designs)",
            items.len(),
        );
        // Not a hard perf assertion (machine-dependent) -- this is a measurement
        // probe, not a regression gate.
    }
}

//! The synchronous/background split for the local search path: a request-sequence
//! counter that lets a stale async result be dropped, a debounced background dispatch
//! for the (rare, slow) performance-filtered query, and [`refresh_diagram_list`], the
//! single entry point every local search/filter change calls.

use super::{
    fetch::{
        DisplaySearchOptions, apply_diagram_list_to_ui, fetch_diagram_list_with_options,
        fetch_diagram_list_with_own_connection,
    },
    filters::{
        read_id_filter, read_local_only, read_range_filter, read_sort_order, read_tag_filter,
    },
};
use crate::{LibraryModel, MainWindow};
use indicatrix_vault::{
    db::sqlite::{Database, SortOrder},
    model::filter::RangeFilter,
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tracing::error;

/// Debounce delay between a search/filter change and a background performance-
/// filtered re-query actually dispatching (see [`dispatch_background_search`]) --
/// collapses a burst of range-slider drag ticks to the LAST one, the same shape and
/// magnitude as `gui::editor::auto_solve::AUTO_SOLVE_DEBOUNCE`.
const SEARCH_DEBOUNCE_DELAY: Duration = Duration::from_millis(120);

/// How long the search box waits after the last keystroke before the query runs (see
/// [`debounce_text_refresh`]) -- a burst of typing runs the query once, for the text
/// the cutter stopped on, instead of once per character.
const TEXT_FILTER_DEBOUNCE: Duration = Duration::from_millis(120);

thread_local! {
    /// Bumped by every [`refresh_diagram_list`] call, sync or async alike -- an async
    /// dispatch captures its own value at dispatch time (BEFORE the debounce delay,
    /// not after) and only applies its result if this still matches on completion
    /// (see [`is_current_search`]). This is the exact `current_seq`/`is_current` shape
    /// `gui::editor::auto_solve::Runtime` already uses for a background solve --
    /// bumping unconditionally (not just on an async dispatch) is what lets a
    /// SYNCHRONOUS refresh (e.g. the performance filter was just cleared) correctly
    /// supersede an older async one still in flight, even though the sync path never
    /// looks at this counter itself.
    static SEARCH_SEQ: Cell<u64> = const { Cell::new(0) };
    /// The debounce timer [`dispatch_background_search`] restarts on every eligible
    /// call -- replacing it cancels whatever the previous one had pending, same as
    /// `gui::editor::auto_solve::Runtime::debounce`.
    static SEARCH_DEBOUNCE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    /// The timer [`debounce_text_refresh`] restarts on every keystroke. Any refresh that
    /// runs sooner takes it, since that refresh already reads the latest text.
    static TEXT_DEBOUNCE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Runs `refresh` once, [`TEXT_FILTER_DEBOUNCE`] after the LAST call to this function;
/// a call made before that replaces the pending `refresh` with its own. The search box
/// goes through this so typing does not run a query per character.
///
/// UI thread only (it starts a `slint::Timer`).
pub(super) fn debounce_text_refresh(refresh: impl FnOnce() + 'static) {
    // `slint::Timer` wants an `FnMut`; the `Option` lets the one-shot `refresh` move out.
    let mut refresh = Some(refresh);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        TEXT_FILTER_DEBOUNCE,
        move || {
            if let Some(refresh) = refresh.take() {
                refresh();
            }
        },
    );
    // Replacing the previous timer drops it, which cancels whatever it had pending.
    TEXT_DEBOUNCE.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// `pub(in crate::gui)`, not private: [`crate::gui::library::remote`] (both
/// branches of `setup_library_source_callbacks`) and
/// [`crate::gui::library::local::helpers::refresh_after_library_change`]
/// also need to invalidate an in-flight search of their own.
pub(in crate::gui) fn bump_search_seq() -> u64 {
    SEARCH_SEQ.with(|cell| {
        let next = cell.get() + 1;
        cell.set(next);
        next
    })
}

/// See [`bump_search_seq`]'s own doc comment for why this is `pub(in crate::gui)`
/// rather than private.
pub(in crate::gui) fn is_current_search(seq: u64) -> bool {
    SEARCH_SEQ.with(|cell| cell.get() == seq)
}

/// Owned counterpart of [`DisplaySearchOptions`] -- a background search thread cannot
/// borrow `tag_filter`/`id_filter` from the UI-thread-local `String`/`Vec`
/// `super::filters::{read_tag_filter, read_id_filter}` return, so
/// [`BackgroundSearchRequest`] carries owned copies instead. `Clone`, not just owned:
/// the debounce timer closure below must stay `FnMut`-compatible (Slint's `Timer`
/// API), so it clones this out of its capture on each call rather than moving it --
/// harmless since it only actually runs once (`TimerMode::SingleShot`).
#[derive(Clone)]
struct BackgroundSearchOptions {
    order: SortOrder,
    local_only: bool,
    tag_filter: Option<String>,
    id_filter: Option<Vec<i64>>,
}

/// Everything [`dispatch_background_search`] needs beyond `ui`/`db_mutex`, bundled to
/// keep that function's argument count under clippy's limit -- same reasoning as
/// `gui::editor::auto_solve::BackgroundSolveResult`.
#[derive(Clone)]
struct BackgroundSearchRequest {
    /// Captured by [`refresh_diagram_list`] at dispatch time, BEFORE the debounce
    /// delay -- see [`SEARCH_SEQ`]'s own doc comment for why that timing matters.
    seq: u64,
    search: String,
    shape_filter: String,
    gear_filter: String,
    range: RangeFilter,
    options: BackgroundSearchOptions,
}

/// Moves a performance-filtered [`fetch_diagram_list_with_options`] call off the UI
/// thread -- the library-search freeze fix. Measured against the real ~3,300-design
/// catalogue: an active performance filter's fallback (walking every SQL-narrowed
/// candidate and decoding its tilt curves in Rust, see
/// `indicatrix_vault::db::sqlite::search`'s own module doc comment) took ~145ms for
/// `search_diagrams_display` plus ~395ms for `count_matching_diagrams` -- both of
/// which `fetch_diagram_list_with_options` calls on every keystroke/slider-tick,
/// versus ~3ms for the same call with no performance filter active. Half a second of
/// frozen UI per interaction compounds badly during a slider drag, which is why the
/// no-filter path ([`refresh_diagram_list`]'s other branch) stays synchronous -- it
/// has nothing to hide behind a worker thread.
///
/// Follows `gui::editor::auto_solve::dispatch_background_solve`'s own shape: a
/// debounced `slint::Timer::start` (`TimerMode::SingleShot`) defers the actual
/// `thread::spawn`, and the worker posts its result back via
/// `Weak::upgrade_in_event_loop`, checking [`is_current_search`] before touching
/// anything -- a superseded result (a newer keystroke/slider-tick landed, or the
/// cutter cleared the performance filter and the fast sync path already ran) is
/// simply dropped.
///
/// Sets `LibraryModel.status_message` to "Searching..." immediately (not after the
/// debounce delay) so the cutter sees SOMETHING happened right away, and clears it
/// back to empty once a current result lands -- the sync path never touches this
/// property at all, so this is the one place a stale "Searching..." could otherwise
/// linger.
fn dispatch_background_search(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    request: BackgroundSearchRequest,
) {
    ui.global::<LibraryModel>()
        .set_status_message("Searching...".into());

    let ui_weak = ui.as_weak();
    let db_mutex = Arc::clone(db_mutex);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        SEARCH_DEBOUNCE_DELAY,
        move || {
            let db_mutex = Arc::clone(&db_mutex);
            let ui_weak = ui_weak.clone();
            let request = request.clone();
            thread::spawn(move || {
                let BackgroundSearchRequest {
                    seq,
                    search,
                    shape_filter,
                    gear_filter,
                    range,
                    options,
                } = request;
                // Its own dedicated read-only connection, not the shared
                // `db_mutex` -- see `fetch_diagram_list_with_own_connection`'s own doc
                // comment.
                let result = fetch_diagram_list_with_own_connection(
                    &db_mutex,
                    &search,
                    &shape_filter,
                    &gear_filter,
                    &range,
                    DisplaySearchOptions {
                        order: options.order,
                        local_only: options.local_only,
                        tag_filter: options.tag_filter.as_deref(),
                        id_filter: options.id_filter.as_deref(),
                    },
                );
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    if !is_current_search(seq) {
                        // A newer search (or a sync refresh that ran in the meantime,
                        // e.g. clearing the performance filter) already owns the
                        // display -- nothing here is still current enough to show.
                        return;
                    }
                    match result {
                        Ok(fetched) => {
                            apply_diagram_list_to_ui(&ui, fetched);
                            ui.global::<LibraryModel>()
                                .set_status_message(String::new().into());
                        }
                        Err(e) => {
                            error!("Failed to search diagrams: {:?}", e);
                            ui.global::<LibraryModel>().set_status_message(
                                format!("Error searching database: {e}").into(),
                            );
                        }
                    }
                });
            });
        },
    );
    SEARCH_DEBOUNCE.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// The local-database search entry point.
///
/// Every local search/filter change in this crate calls this (see `super::refresh_diagram_list_via_source` for the dispatch
/// that picks this over the remote path). Fast, no-performance-filter searches run
/// synchronously; a performance-filtered query is debounced and moved onto a
/// background thread instead -- see [`dispatch_background_search`]'s own doc comment
/// for the measured cost that split exists to hide.
pub fn refresh_diagram_list(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    // Drop any debounce timer a PREVIOUS call to this function left
    // pending. Its own eventual result is already safely dropped by
    // `is_current_search` (the `seq` bump right below invalidates it), but without
    // this it still fires ~120ms later and still runs the locked query
    // `dispatch_background_search`'s own doc comment measures at up to ~540ms against
    // the real catalogue, for a result nobody will ever see -- wasted work every time
    // a performance filter is added and then cleared inside one debounce window.
    SEARCH_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });
    // A keystroke still waiting out its debounce is superseded: this call already runs
    // with the latest text. (When the debounce itself fires into here, this drops the
    // timer that is running -- Slint allows a timer callback to drop its own timer.)
    TEXT_DEBOUNCE.with(|cell| {
        cell.borrow_mut().take();
    });

    // Bumped unconditionally, sync or async -- see `SEARCH_SEQ`'s own doc comment.
    let seq = bump_search_seq();
    let range = read_range_filter(ui);
    let order = read_sort_order(ui);
    let local_only = read_local_only(ui);
    let tag_filter = read_tag_filter(ui);
    let id_filter = read_id_filter(ui);

    if range.performance.is_empty() {
        // The fast, index-backed path (see `dispatch_background_search`'s own doc
        // comment for the measured ~3ms this stays synchronous for) -- a round trip
        // through a worker thread would only add latency here, never remove a freeze
        // that does not exist on this branch.
        match fetch_diagram_list_with_options(
            db_mutex,
            search,
            shape_filter,
            gear_filter,
            &range,
            DisplaySearchOptions {
                order,
                local_only,
                tag_filter: tag_filter.as_deref(),
                id_filter: id_filter.as_deref(),
            },
        ) {
            Ok(fetched) => {
                apply_diagram_list_to_ui(ui, fetched);
                // A "Searching..." message `dispatch_background_search`
                // set for a NOW-superseded async dispatch (e.g. the performance
                // filter that triggered it was just cleared, landing this call on the
                // fast sync path instead) would otherwise linger forever, since the sync
                // path never touches `status_message` on success otherwise. Never
                // clobbers a DIFFERENT message this same call already set (e.g.
                // `read_performance_filters`'s "N tilt-performance filter(s) had an
                // invalid tilt radius" rejection notice, reachable here when every
                // supplied filter was rejected and `range.performance` ends up empty).
                if ui.global::<LibraryModel>().get_status_message() == "Searching..." {
                    ui.global::<LibraryModel>()
                        .set_status_message(String::new().into());
                }
            }
            Err(e) => {
                error!("Failed to search diagrams: {:?}", e);
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Error searching database: {e}").into());
            }
        }
        return;
    }

    dispatch_background_search(
        ui,
        db_mutex,
        BackgroundSearchRequest {
            seq,
            search: search.to_string(),
            shape_filter: shape_filter.to_string(),
            gear_filter: gear_filter.to_string(),
            range,
            options: BackgroundSearchOptions {
                order,
                local_only,
                tag_filter,
                id_filter,
            },
        },
    );
}

use crate::{
    DiagramItem, LibraryModel, MainWindow,
    bridge::library::source::{self as library_source, LibrarySource},
    settings::WorkerSettings,
};
use anyhow::Result;
use indicatrix_net::library::{
    DesignSummary, LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
    PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::{
        entry::DiagramListItem,
        filter::RangeFilter,
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tracing::{error, warn};

/// A slider value that sits exactly on its data-bounds edge means "not filtering on
/// this side" -- returns `None` so `search_diagrams` skips the predicate entirely
/// (which also means rows with no value at all for that attribute stay visible, same
/// as the unfiltered "All" state of the existing shape/gear dropdowns). Any other
/// value is an active bound.
fn active_bound(value: f32, bound_edge: f32) -> Option<f64> {
    let moved_off_edge = (value - bound_edge).abs() >= 1e-6;
    moved_off_edge.then_some(f64::from(value))
}

/// Builds the `RangeFilter` to hand to `search_diagrams`.
///
/// Reads the four range-filter sliders' current min/max values, the RI centre+
/// tolerance filter, the "show ignored" toggle, and every active tilt-performance
/// filter row off `ui`. See `active_bound` for what makes a min/max bound "active"
/// versus effectively unset.
#[must_use]
pub fn read_range_filter(ui: &MainWindow) -> RangeFilter {
    RangeFilter {
        ri_min: active_bound(
            ui.global::<LibraryModel>().get_ri_filter_min(),
            ui.global::<LibraryModel>().get_ri_bounds_min(),
        ),
        ri_max: active_bound(
            ui.global::<LibraryModel>().get_ri_filter_max(),
            ui.global::<LibraryModel>().get_ri_bounds_max(),
        ),
        lw_min: active_bound(
            ui.global::<LibraryModel>().get_lw_filter_min(),
            ui.global::<LibraryModel>().get_lw_bounds_min(),
        ),
        lw_max: active_bound(
            ui.global::<LibraryModel>().get_lw_filter_max(),
            ui.global::<LibraryModel>().get_lw_bounds_max(),
        ),
        volume_min: active_bound(
            ui.global::<LibraryModel>().get_volume_filter_min(),
            ui.global::<LibraryModel>().get_volume_bounds_min(),
        ),
        volume_max: active_bound(
            ui.global::<LibraryModel>().get_volume_filter_max(),
            ui.global::<LibraryModel>().get_volume_bounds_max(),
        ),
        facets_min: active_bound(
            ui.global::<LibraryModel>().get_facets_filter_min(),
            ui.global::<LibraryModel>().get_facets_bounds_min(),
        )
        .map(|v| v.round() as i64),
        facets_max: active_bound(
            ui.global::<LibraryModel>().get_facets_filter_max(),
            ui.global::<LibraryModel>().get_facets_bounds_max(),
        )
        .map(|v| v.round() as i64),
        // A separate RI band, `[center - tolerance, center + tolerance]`, composing
        // with `ri_min`/`ri_max` above by intersection -- see
        // `RangeFilter::ri_tolerance`'s doc comment. `None` whenever the panel's
        // toggle is off, same convention as `active_bound`.
        ri_tolerance: ui
            .global::<LibraryModel>()
            .get_ri_tolerance_enabled()
            .then(|| {
                (
                    f64::from(ui.global::<LibraryModel>().get_ri_tolerance_center()),
                    f64::from(ui.global::<LibraryModel>().get_ri_tolerance_value()),
                )
            }),
        performance: read_performance_filters(ui),
        include_ignored: ui.global::<LibraryModel>().get_show_ignored(),
    }
}

/// Maps `LibraryModel.sort_order_index` (the header toolbar's sort selector) onto a
/// [`SortOrder`].
///
/// Index<->variant mapping happens here, once, the same convention
/// [`read_performance_filters`] uses for its own index-coded fields. An out-of-range
/// index (should not happen; the combo box's own model bounds it) falls back to
/// [`SortOrder::CatalogueOrder`] rather than panicking.
#[must_use]
pub fn read_sort_order(ui: &MainWindow) -> SortOrder {
    match ui.global::<LibraryModel>().get_sort_order_index() {
        1 => SortOrder::Title,
        2 => SortOrder::Newest,
        3 => SortOrder::RecentlyEdited,
        _ => SortOrder::CatalogueOrder,
    }
}

/// Reads the library panel's "My designs" toggle (`filter_panel.slint`) -- `true`
/// restricts the catalogue display to the cutter's own locally-imported designs.
#[must_use]
pub fn read_local_only(ui: &MainWindow) -> bool {
    ui.global::<LibraryModel>().get_local_only_filter()
}

/// Reads the library panel's tag chip filter (`filter_panel.slint`) --
/// `LibraryModel.active_tag_filter_name` is `""` when no chip is
/// selected.
///
/// Returns the tag's NAME, not its id: resolving a name to a `tags.id` is
/// a database read (`Database::tag_id_by_name`), so that resolution happens in
/// [`fetch_diagram_list_with_options`], the one place in this path that already
/// holds the `Database` lock -- this function stays a pure `ui` read like every
/// other `read_*` in this module.
#[must_use]
pub fn read_tag_filter(ui: &MainWindow) -> Option<String> {
    let name = ui.global::<LibraryModel>().get_active_tag_filter_name();
    (!name.is_empty()).then(|| name.to_string())
}

/// Reads the "show these N" restriction a batch import leaves behind
/// (`LibraryModel.recent_import_filter`) -- an
/// empty list (the ordinary case) means no restriction.
///
/// Cleared by `gui::library::diagram_list::setup_search_and_filter_callbacks`'s four
/// handlers the moment the cutter makes any real search/filter change, so it never
/// outlives the view it was created for.
#[must_use]
pub fn read_id_filter(ui: &MainWindow) -> Option<Vec<i64>> {
    let model = ui.global::<LibraryModel>().get_recent_import_filter();
    if model.row_count() == 0 {
        return None;
    }
    Some(model.iter().map(i64::from).collect())
}

/// Reads `ui.global::<LibraryModel>().get_performance_filters()` (the tilt-performance filter panel's rows,
/// `PerformanceFilterRow` -- see that Slint struct's doc comment for the index<->enum
/// mapping this function applies) and builds the `Vec<PerformanceFilter>`
/// `read_range_filter` embeds in the `RangeFilter` it returns.
///
/// Every row is validated through `PerformanceFilter::new` rather than hand-built:
/// `filter_panel.slint`'s radius slider is already bounded to `0.0..=90.0`, so
/// rejection should never actually happen in practice, but this is still the one
/// boundary where a raw `tilt_radius_deg` becomes a real `PerformanceFilter`. A
/// rejected row is dropped from the query (never silently clamped into range) and
/// reported through `ui.set_status_message`.
fn read_performance_filters(ui: &MainWindow) -> Vec<PerformanceFilter> {
    let rows = ui.global::<LibraryModel>().get_performance_filters();
    let mut filters = Vec::with_capacity(rows.row_count());
    let mut rejected = 0usize;
    for row in rows.iter() {
        let metric = match row.metric_index {
            0 => PerformanceMetric::Brilliance,
            1 => PerformanceMetric::Extinction,
            _ => PerformanceMetric::Windowing,
        };
        let bound = if row.bound_index == 0 {
            PerformanceBound::AtMost(row.threshold_pct)
        } else {
            PerformanceBound::AtLeast(row.threshold_pct)
        };
        let aggregate = if row.aggregate_index == 0 {
            PerformanceAggregate::Worst
        } else {
            PerformanceAggregate::Mean
        };
        match PerformanceFilter::new(metric, bound, row.tilt_radius_deg, aggregate) {
            Ok(filter) => filters.push(filter),
            Err(_) => rejected += 1,
        }
    }
    if rejected > 0 {
        ui.global::<LibraryModel>().set_status_message(
            format!(
                "{rejected} tilt-performance filter(s) had an invalid tilt radius and were \
                 ignored."
            )
            .into(),
        );
    }
    filters
}

/// Every entry id matching the library panel's current search/filter state.
///
/// The real, uncapped "filtered set" the owner's "regenerate previews/tilt curves
/// for the filtered set" batch actions need -- covers search text, shape/gear dropdowns, and every range/tag/
/// local-only filter, via [`Database::matching_entry_ids`]. Reads `ui` exactly the way
/// [`fetch_diagram_list_with_options`]'s own callers do (see e.g.
/// `gui::library::diagram_list::setup_search_and_filter_callbacks`'s
/// `on_filters_changed`), so "the filtered set" always means the same rows the panel
/// is currently showing.
///
/// Local-catalogue only: there is no remote equivalent of
/// [`Database::matching_entry_ids`] (a [`LibrarySource::Remote`] session has no local
/// `Database` at all) -- a caller under a remote source should refuse this action
/// rather than call it (see `gui::batch::preview`/`gui::batch::tilt::wiring`'s own
/// "regenerate for filtered set" callbacks for exactly that guard).
///
/// # Errors
///
/// Returns whatever [`Database::matching_entry_ids`] returns.
pub fn current_filtered_entry_ids(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
) -> Result<Vec<i64>> {
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
    let clean_shape = if shape == "All Shapes" {
        "All".to_string()
    } else {
        shape.to_string()
    };
    let clean_gear = if gear == "All Gears" {
        "All".to_string()
    } else {
        gear.to_string()
    };
    let range = read_range_filter(ui);
    let local_only = read_local_only(ui);
    let tag_filter_name = read_tag_filter(ui);
    let id_filter = read_id_filter(ui);

    let db = db_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let tag_filter = tag_filter_name
        .as_deref()
        .and_then(|name| db.tag_id_by_name(name).ok().flatten());
    let filters = DisplayFilters {
        // `matching_entry_ids` always walks in `SortOrder::CatalogueOrder` internally
        // (see that method's own doc comment) -- this field is otherwise unused by it,
        // kept at the default purely because `DisplayFilters` has no other convenient
        // "don't care" spelling.
        order: SortOrder::CatalogueOrder,
        local_only,
        tag_filter,
        id_filter: id_filter.as_deref(),
    };
    db.matching_entry_ids(&search, &clean_shape, &clean_gear, &range, filters)
}

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

/// The remote counterpart of [`refresh_diagram_list`]: one `LibraryRequest::Search`
/// against `worker`, off the UI thread. Shares the same 1000-result cap as the local
/// query -- both are ultimately backed by `Database::search_diagrams`. `pub(crate)`,
/// not private: also called directly from `gui::library_remote` right after a source
/// switch succeeds, when the caller already has the `WorkerSettings` in hand and can
/// skip `refresh_diagram_list_via_source`'s `LibrarySource` re-check.
///
/// Captures [`bump_search_seq`] BEFORE the request is even built (matching
/// [`refresh_diagram_list`]'s own "bump unconditionally, before the async work starts"
/// rule) and checks [`is_current_search`] inside the reply closure before any model
/// write. Without this, a slow remote reply could land after a newer keystroke (or
/// after the cutter switched back to the local library entirely -- see
/// `remote::setup_library_source_callbacks`'s own bump on every source switch) and
/// overwrite the current list with stale REMOTE entry ids that a local-source guard
/// would then happily act on (`toggle_ignored`/`delete_diagram` in `local::organize`).
pub(crate) fn refresh_diagram_list_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    let seq = bump_search_seq();
    let clean_shape = if shape_filter == "All Shapes" {
        "All".to_string()
    } else {
        shape_filter.to_string()
    };
    let clean_gear = if gear_filter == "All Gears" {
        "All".to_string()
    } else {
        gear_filter.to_string()
    };
    let range = to_range_wire(&read_range_filter(ui));
    // `apps/indicatrix-worker`'s `serve::library` applies both the sort order and the
    // tag chip (see `LibraryRequest::Search`'s own doc comment), so a remote search
    // honours them exactly like `refresh_diagram_list`'s local counterpart does via
    // `Database::search_diagrams_display`.
    let request = LibraryRequest::Search {
        query: search.to_string(),
        shape_filter: clean_shape,
        gear_filter: clean_gear,
        range,
        order: to_sort_order_wire(read_sort_order(ui)),
        tag_filter: read_tag_filter(ui),
    };
    library_source::spawn_library_request(ui.as_weak(), worker, request, move |ui, result| {
        if !is_current_search(seq) {
            // A newer search (local or remote) or a source switch already owns
            // the display -- see this function's own doc comment.
            return;
        }
        match result {
            Ok(LibraryResponse::SearchResults {
                items,
                excluded_for_missing_curves,
            }) => {
                let total = items.len();
                let slint_items: Vec<DiagramItem> = items.iter().map(to_diagram_item).collect();
                ui.global::<LibraryModel>()
                    .set_diagram_list(ModelRc::new(VecModel::from(slint_items)));
                ui.global::<LibraryModel>().set_total_count(total as i32);
                // No server-side "real match count" exists yet for a remote source --
                // `LibraryResponse::SearchResults` carries only the capped item list
                // (see `indicatrix_net::library`'s wire types), not a separate
                // `COUNT(*)` the way `Database::count_matching_diagrams` provides
                // locally. Setting it to this same capped `total`
                // keeps the property populated with a truthful lower bound rather than
                // stale/zero, matching this path's pre-existing `total_count` behaviour
                // -- a real fix needs a new field on that wire response, out of scope
                // for this crate alone (`indicatrix-net`/`indicatrix-worker`).
                ui.global::<LibraryModel>().set_matched_count(total as i32);
                // `DesignSummary::ignored` and this reply's `excluded_for_missing_curves`
                // let a remote-sourced list drive the same ignored-row styling and
                // missing-curves notice the local path (`refresh_diagram_list` below)
                // already does. `cast_possible_truncation` is workspace-`allow`ed for
                // this `u32 as i32`, matching every other numeric cast into a Slint
                // `int` property in this file.
                ui.global::<LibraryModel>()
                    .set_performance_excluded_count(excluded_for_missing_curves as i32);
            }
            Ok(_) => {
                ui.global::<LibraryModel>()
                    .set_status_message("Unexpected reply searching the remote library.".into());
            }
            Err(e) => {
                error!("Remote search failed: {e}");
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Remote search failed: {e}").into());
            }
        }
    });
}

/// Maps a local [`RangeFilter`] onto the wire [`RangeFilterWire`] a remote
/// [`LibraryRequest::Search`]/`SearchPage` carries; `apps/indicatrix-worker`'s
/// `serve::library::from_range_wire` is the other half of this pair. Not `const`:
/// mapping `performance` element-wise needs an allocation.
fn to_range_wire(range: &RangeFilter) -> RangeFilterWire {
    RangeFilterWire {
        ri_min: range.ri_min,
        ri_max: range.ri_max,
        lw_min: range.lw_min,
        lw_max: range.lw_max,
        volume_min: range.volume_min,
        volume_max: range.volume_max,
        facets_min: range.facets_min,
        facets_max: range.facets_max,
        ri_tolerance: range.ri_tolerance,
        include_ignored: range.include_ignored,
        performance: range
            .performance
            .iter()
            .map(to_performance_filter_wire)
            .collect(),
    }
}

/// Maps a local [`SortOrder`] (as [`read_sort_order`] reads it off the header
/// toolbar's sort selector) onto the wire [`SortOrderWire`] a remote
/// [`LibraryRequest::Search`] carries -- `apps/indicatrix-worker`'s
/// `serve::library` is the other half of this pair, mirroring `to_range_wire`'s own
/// local/wire split just above.
const fn to_sort_order_wire(order: SortOrder) -> SortOrderWire {
    match order {
        SortOrder::CatalogueOrder => SortOrderWire::CatalogueOrder,
        SortOrder::Title => SortOrderWire::Title,
        SortOrder::Newest => SortOrderWire::Newest,
        SortOrder::RecentlyEdited => SortOrderWire::RecentlyEdited,
    }
}

const fn to_performance_filter_wire(filter: &PerformanceFilter) -> PerformanceFilterWire {
    let metric = match filter.metric {
        PerformanceMetric::Brilliance => PerformanceMetricWire::Brilliance,
        PerformanceMetric::Extinction => PerformanceMetricWire::Extinction,
        PerformanceMetric::Windowing => PerformanceMetricWire::Windowing,
    };
    let bound = match filter.bound {
        PerformanceBound::AtMost(t) => PerformanceBoundWire::AtMost(t),
        PerformanceBound::AtLeast(t) => PerformanceBoundWire::AtLeast(t),
    };
    let aggregate = match filter.aggregate {
        PerformanceAggregate::Worst => PerformanceAggregateWire::Worst,
        PerformanceAggregate::Mean => PerformanceAggregateWire::Mean,
    };
    PerformanceFilterWire {
        metric,
        bound,
        tilt_radius_deg: filter.tilt_radius_deg,
        aggregate,
    }
}

/// The synthetic URL scheme every locally-imported design is saved under (see
/// `indicatrix_vault::local::import_asc`) -- shared by [`to_diagram_item`] and
/// [`apply_diagram_list_to_ui`] so a design's "Mine" badge is
/// decided identically for the local and remote-browsed paths.
fn is_local_url(url: &str) -> bool {
    url.starts_with("local://")
}

/// Reads [`DesignSummary::ignored`] straight off the row, giving a remote-sourced row
/// the same ignored flag a local-sourced
/// [`indicatrix_vault::model::entry::DiagramListItem::ignored`] already carries. Same
/// treatment for [`is_local_url`]/`DiagramItem::is_local`: a
/// remote-browsed row is exactly as able to say "the far end's own local import" as a
/// local one, from the same `url` field.
pub(crate) fn to_diagram_item(item: &DesignSummary) -> DiagramItem {
    DiagramItem {
        id: item.entry_id as i32,
        title: item.title.clone().into(),
        shape: item.shape.clone().unwrap_or_default().into(),
        gear: item.index_gear.clone().unwrap_or_default().into(),
        facets: item.facets_count.clone().unwrap_or_default().into(),
        designer: item.designer_info.clone().unwrap_or_default().into(),
        lw_ratio: item.lw_ratio.clone().unwrap_or_default().into(),
        ri: item.refractive_index.clone().unwrap_or_default().into(),
        ignored: item.ignored,
        is_local: is_local_url(&item.url),
        // Tags are a purely local-catalogue concept (see
        // `Database::migrate_tag_tables`'s doc comment) -- the remote library
        // protocol carries no tag data, so a remote-browsed row always shows none.
        tags: ModelRc::new(VecModel::from(Vec::<slint::SharedString>::new())),
    }
}

/// One row [`fetch_diagram_list`] returns: the plain catalogue row plus whether it is
/// currently marked ignored.
pub struct DiagramListRow {
    pub item: DiagramListItem,
    /// `true` iff this row is a currently-ignored design being shown because
    /// `RangeFilter::include_ignored` is on. Always `false` while that toggle is off --
    /// an ignored row can't reach this struct in that case, since it's excluded by the
    /// query itself.
    pub ignored: bool,
    /// This row's tag names, alphabetical -- looked up in bulk by
    /// [`fetch_diagram_list_with_options`] via `Database::tags_by_entry` rather than
    /// one query per row.
    pub tags: Vec<String>,
}

/// [`fetch_diagram_list`]'s full result.
///
/// The rows, the catalogue's total design count (unrelated to how many matched), the
/// real match count for the active search/filters (see
/// [`indicatrix_vault::db::sqlite::Database::count_matching_diagrams`], distinct from
/// both `total` and `rows.len()`, which is capped), and how many otherwise-matching
/// designs a currently-active tilt-performance filter excluded for having no computed
/// curves at all -- see
/// `indicatrix_vault::model::filter::PerformanceSearchResult::excluded_for_missing_curves`'s
/// doc comment for what that count does and doesn't include.
pub struct FetchedDiagramList {
    pub rows: Vec<DiagramListRow>,
    pub total: usize,
    pub matched_count: usize,
    pub excluded_for_missing_curves: usize,
    /// `true` when `total` and/or `matched_count` above is a FALLBACK value because
    /// the real `COUNT(*)` query behind it failed, rather than being swallowed
    /// silently via `unwrap_or(result.items.len())`, which would make a broken count
    /// query look exactly like "every matching design fit on this page" instead of an
    /// error.
    /// [`apply_diagram_list_to_ui`] turns this into a `status_message` note; the rows
    /// that DID come back are still rendered regardless.
    pub count_unavailable: bool,
}

/// The `Database` read half of [`refresh_diagram_list`], with no sort/"My designs"
/// preference.
///
/// Exactly [`fetch_diagram_list_with_options`] at [`SortOrder::CatalogueOrder`]/
/// `local_only: false`, kept as its own function so this crate's existing call sites (a
/// post-import/rename/delete background refresh, see `gui::library::local::helpers`)
/// keep compiling and behaving exactly as before.
///
/// # Errors
///
/// Returns an error under the same conditions as [`fetch_diagram_list_with_options`].
pub fn fetch_diagram_list(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
) -> Result<FetchedDiagramList> {
    fetch_diagram_list_with_options(
        db_mutex,
        search,
        shape_filter,
        gear_filter,
        range,
        DisplaySearchOptions::default(),
    )
}

/// [`fetch_diagram_list_with_options`]'s sort/restriction options.
///
/// Bundled into one value purely to keep that function under clippy's
/// `too_many_arguments` lint -- same reasoning as
/// `indicatrix_vault::db::sqlite::DisplayFilters`, which this expands into once
/// `tag_filter` is resolved to an id.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplaySearchOptions<'a> {
    pub order: SortOrder,
    pub local_only: bool,
    /// The tag chip filter's NAME -- see [`read_tag_filter`]'s
    /// own doc comment for why this stays a name this far down, resolved to an id
    /// only once the `Database` lock is already held below.
    pub tag_filter: Option<&'a str>,
    /// "Show these N" restriction to an explicit id set -- see [`read_id_filter`]'s own doc comment.
    pub id_filter: Option<&'a [i64]>,
}

/// [`fetch_diagram_list`], plus a caller-chosen [`SortOrder`] and "My designs"/tag/
/// batch-id restriction.
///
/// What [`refresh_diagram_list`] actually calls, reading every field of `options` off
/// `ui` via [`read_sort_order`]/[`read_local_only`]/[`read_tag_filter`]/
/// [`read_id_filter`].
///
/// Has no `ui` dependency beyond the already-resolved `range`/`options` -- split out
/// so `gui::library`'s post-import/rename/delete/shape-change refresh can run this on
/// a background thread (re-running an unfiltered search against a several-thousand-
/// design catalogue synchronously on the UI thread is itself a perceptible freeze) and
/// only marshal the cheap [`apply_diagram_list_to_ui`] step back onto the UI thread.
/// Every other caller (search box edits, filter slider drags) stays on the synchronous
/// [`refresh_diagram_list`] below, which a filter that must feel immediate as you type
/// actually needs.
///
/// # Errors
///
/// Returns an error if `Database::search_diagrams_display_with_count` fails (a bad
/// filter combination, or the connection itself). `get_total_count`'s own failure is
/// tolerated instead (see [`FetchedDiagramList::count_unavailable`]).
pub fn fetch_diagram_list_with_options(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    // Scoped so the guard drops before this returns: this runs on a background thread
    // where any extra time holding the database lock is time a UI-thread callback can
    // block on it.
    let db = db_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = fetch_diagram_list_from_db(&db, search, shape_filter, gear_filter, range, options);
    // Explicit: the rest of this function needs no lock, and the UI thread may be
    // waiting on this same mutex.
    drop(db);
    result
}

/// The pure `Database` half of [`fetch_diagram_list_with_options`] -- operates on an
/// already-open connection directly, with no locking of its own, so a caller that
/// holds its OWN dedicated connection (see [`fetch_diagram_list_with_own_connection`])
/// never touches the shared `Mutex<Database>` at all.
fn fetch_diagram_list_from_db(
    db: &Database,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    let clean_shape = if shape_filter == "All Shapes" {
        "All"
    } else {
        shape_filter
    };
    let clean_gear = if gear_filter == "All Gears" {
        "All"
    } else {
        gear_filter
    };

    // A tag name that no longer resolves (deleted between the UI reading its
    // chip and this call) is treated as no restriction rather than an error --
    // consistent with `shape_filter`/`gear_filter`'s own "not found = All"
    // tolerance a few lines up.
    let tag_filter_id = options
        .tag_filter
        .and_then(|name| db.tag_id_by_name(name).ok().flatten());
    let filters = DisplayFilters {
        order: options.order,
        local_only: options.local_only,
        tag_filter: tag_filter_id,
        id_filter: options.id_filter,
    };
    // One candidate walk for both the display page and
    // the real match count, instead of `search_diagrams_display` +
    // `count_matching_diagrams` separately -- see
    // `Database::search_diagrams_display_with_count`'s own doc comment for why
    // decoding every active-performance-filter candidate's tilt-curve blob twice
    // would cost.
    let (result, matched_count) =
        db.search_diagrams_display_with_count(search, clean_shape, clean_gear, range, filters)?;

    // `get_total_count` is a separate, whole-catalogue query (unrelated to
    // the search/filter above) whose own failure is handled explicitly rather than
    // swallowed into `result.items.len()` with nothing on screen saying so -- see
    // `FetchedDiagramList::count_unavailable`'s own doc comment.
    let mut count_unavailable = false;
    let total = db.get_total_count().unwrap_or_else(|e| {
        warn!("get_total_count failed: {e}");
        count_unavailable = true;
        result.items.len()
    });

    // One bulk query for every entry's tags rather than one
    // `tags_for_entry` call per row -- see `Database::tags_by_entry`'s own doc
    // comment. A failed lookup degrades to "no tags shown" rather than failing
    // the whole list fetch.
    let mut tags_by_entry = db.tags_by_entry().unwrap_or_default();

    // `ignored` is read straight off the row (`DiagramListItem::ignored`).
    let rows: Vec<DiagramListRow> = result
        .items
        .into_iter()
        .map(|item| {
            let ignored = item.ignored;
            let tags = tags_by_entry.remove(&item.id).unwrap_or_default();
            DiagramListRow {
                item,
                ignored,
                tags,
            }
        })
        .collect();

    Ok(FetchedDiagramList {
        rows,
        total,
        matched_count,
        excluded_for_missing_curves: result.excluded_for_missing_curves,
        count_unavailable,
    })
}

/// Runs [`fetch_diagram_list_from_db`] against a FRESH, dedicated read-only connection
/// to `crate::gui::DB_PATH` instead of the shared `db_mutex` -- the shared
/// `Mutex<Database>` every UI-thread read/write in this crate also locks is held for
/// the whole performance-filtered query (measured against the real catalogue: ~540ms),
/// so a background search that still used it would only move the freeze off the
/// keystroke, not remove it -- a row click or the synchronous branch of
/// `refresh_diagram_list` could still block on it for that long. WAL (see
/// `indicatrix_vault::db::sqlite`'s own module doc comment) lets a second, independent
/// connection read concurrently with the writer instead of queuing behind it.
///
/// `crate::gui::DB_PATH` is private to `gui`, not `pub(crate)` -- reachable here
/// unchanged because `gui::library::search` is one of `gui`'s own descendant modules
/// (plain Rust module-privacy, no visibility bump needed).
///
/// Falls back to [`fetch_diagram_list_with_options`] (the shared-lock, pre-fix
/// behaviour) when a dedicated connection can't be opened -- the one real case being
/// `gui::run_gui`'s `:memory:` fallback database (started when the real file at
/// `DB_PATH` itself failed to open at startup): there is no second connection to open
/// to a database that only exists inside the already-open one, and that session must
/// keep working rather than erroring out of every performance-filtered search.
fn fetch_diagram_list_with_own_connection(
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    options: DisplaySearchOptions<'_>,
) -> Result<FetchedDiagramList> {
    match Database::open_read_only(crate::gui::DB_PATH) {
        Ok(read_db) => {
            fetch_diagram_list_from_db(&read_db, search, shape_filter, gear_filter, range, options)
        }
        Err(e) => {
            warn!(
                "background search: could not open a dedicated read-only connection to \
                 {:?} ({e}); falling back to the shared database lock for this query",
                crate::gui::DB_PATH
            );
            fetch_diagram_list_with_options(
                db_mutex,
                search,
                shape_filter,
                gear_filter,
                range,
                options,
            )
        }
    }
}

/// The `ui.set_*` half of [`refresh_diagram_list`] -- see [`fetch_diagram_list`]'s doc
/// comment for why these are split.
///
/// Before applying the new list, reconciles `LibraryModel.selected_entry_id`
/// against it -- a previously-selected design that no longer appears in the new result
/// set (a search/filter change, not a delete: the design itself still exists) must not
/// be left "selected" with nothing on screen to show for it. Clears the detail pane via
/// [`super::detail::clear_current_detail_display`] in that case; `diagram_list.slint`'s
/// own `keyboard_focus_index` resets separately, on every `diagram_list` change (not
/// only a length change), so the two together drop both halves of stale selection.
pub fn apply_diagram_list_to_ui(ui: &MainWindow, fetched: FetchedDiagramList) {
    let selected_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if selected_id >= 0
        && !fetched
            .rows
            .iter()
            .any(|row| row.item.id == i64::from(selected_id))
    {
        super::detail::clear_current_detail_display(ui);
    }
    let count_unavailable = fetched.count_unavailable;

    let slint_items: Vec<DiagramItem> = fetched
        .rows
        .into_iter()
        .map(|row| DiagramItem {
            id: row.item.id as i32,
            title: row.item.title.into(),
            shape: row.item.shape.unwrap_or_default().into(),
            gear: row.item.index_gear.unwrap_or_default().into(),
            facets: row.item.facets_count.unwrap_or_default().into(),
            designer: row.item.designer_info.unwrap_or_default().into(),
            lw_ratio: row.item.lw_ratio.unwrap_or_default().into(),
            ri: row.item.refractive_index.unwrap_or_default().into(),
            ignored: row.ignored,
            is_local: is_local_url(&row.item.url),
            // This row's tag chips.
            tags: ModelRc::new(VecModel::from(
                row.tags
                    .into_iter()
                    .map(Into::into)
                    .collect::<Vec<slint::SharedString>>(),
            )),
        })
        .collect();

    ui.global::<LibraryModel>()
        .set_diagram_list(ModelRc::new(VecModel::from(slint_items)));
    ui.global::<LibraryModel>()
        .set_total_count(fetched.total as i32);
    // Real match count for the active search/filters -- see
    // `FetchedDiagramList::matched_count`'s doc comment. Nowhere near `i32::MAX` for
    // the same reason `total`/`excluded_for_missing_curves` below aren't.
    ui.global::<LibraryModel>()
        .set_matched_count(fetched.matched_count as i32);
    // Counts designs within one query's result set (`SEARCH_RESULT_CAP`-bounded,
    // currently 1000), nowhere near `i32::MAX`.
    ui.global::<LibraryModel>()
        .set_performance_excluded_count(fetched.excluded_for_missing_curves as i32);

    // Say so, in plain text, rather than quietly showing a fallback total
    // that happens to equal the page size -- the rows above are still rendered
    // either way.
    if count_unavailable {
        ui.global::<LibraryModel>()
            .set_status_message("Count unavailable.".into());
    }
}

/// Debounce delay between a search/filter change and a background performance-
/// filtered re-query actually dispatching (see [`dispatch_background_search`]) --
/// collapses a burst of range-slider drag ticks to the LAST one, the same shape and
/// magnitude as `gui::editor::auto_solve::AUTO_SOLVE_DEBOUNCE`.
const SEARCH_DEBOUNCE_DELAY: Duration = Duration::from_millis(120);

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
/// [`read_tag_filter`]/[`read_id_filter`] return, so [`BackgroundSearchRequest`]
/// carries owned copies instead. `Clone`, not just owned: the debounce timer closure
/// below must stay `FnMut`-compatible (Slint's `Timer` API), so it clones this out of
/// its capture on each call rather than moving it -- harmless since it only actually
/// runs once (`TimerMode::SingleShot`).
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- bump_search_seq / is_current_search (the library-search freeze fix's
    // staleness mechanism -- mirrors `gui::editor::auto_solve`'s own
    // `current_seq`/`is_current` tests exactly, same shape and same reasoning) ---

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
}

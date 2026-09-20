use crate::{
    DiagramItem, LibraryModel, MainWindow,
    bridge::library::source::{self as library_source, LibrarySource},
    settings::WorkerSettings,
};
use anyhow::Result;
use indicatrix_net::library::{
    DesignSummary, LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
    PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire,
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
use std::sync::{Arc, Mutex};
use tracing::error;

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
/// restricts the catalogue display to the cutter's own locally-imported designs (CAD
/// audit item 189).
#[must_use]
pub fn read_local_only(ui: &MainWindow) -> bool {
    ui.global::<LibraryModel>().get_local_only_filter()
}

/// Reads the library panel's tag chip filter (`filter_panel.slint`, CAD audit item
/// 190's tag half) -- `LibraryModel.active_tag_filter_name` is `""` when no chip is
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
/// (`LibraryModel.recent_import_filter`, CAD audit item 187's batch case) -- an
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
pub(crate) fn refresh_diagram_list_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
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
    let request = LibraryRequest::Search {
        query: search.to_string(),
        shape_filter: clean_shape,
        gear_filter: clean_gear,
        range,
    };
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        request,
        |ui, result| match result {
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
                // locally (CAD audit item 193). Setting it to this same capped `total`
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
        },
    );
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
/// [`apply_diagram_list_to_ui`] so a design's "Mine" badge (CAD audit item 189) is
/// decided identically for the local and remote-browsed paths.
fn is_local_url(url: &str) -> bool {
    url.starts_with("local://")
}

/// Reads [`DesignSummary::ignored`] straight off the row, giving a remote-sourced row
/// the same ignored flag a local-sourced
/// [`indicatrix_vault::model::entry::DiagramListItem::ignored`] already carries. Same
/// treatment for [`is_local_url`]/`DiagramItem::is_local` (CAD audit item 189): a
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
        // CAD audit item 190: tags are a purely local-catalogue concept (see
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
    /// This row's tag names, alphabetical (CAD audit item 190) -- looked up in bulk by
    /// [`fetch_diagram_list_with_options`] via `Database::tags_by_entry` rather than
    /// one query per row.
    pub tags: Vec<String>,
}

/// [`fetch_diagram_list`]'s full result.
///
/// The rows, the catalogue's total design count (unrelated to how many matched), the
/// real match count for the active search/filters (CAD audit item 193 -- see
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
    /// The tag chip filter's NAME (CAD audit item 190) -- see [`read_tag_filter`]'s
    /// own doc comment for why this stays a name this far down, resolved to an id
    /// only once the `Database` lock is already held below.
    pub tag_filter: Option<&'a str>,
    /// "Show these N" restriction to an explicit id set (CAD audit item 187's batch
    /// case) -- see [`read_id_filter`]'s own doc comment.
    pub id_filter: Option<&'a [i64]>,
}

/// [`fetch_diagram_list`], plus a caller-chosen [`SortOrder`] and "My designs"/tag/
/// batch-id restriction (CAD audit items 187/189/190).
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
/// Returns an error if `Database::search_diagrams_display` fails (a bad filter
/// combination, or the connection itself). `get_total_count`'s/`count_matching_diagrams`'s
/// own failures are tolerated instead (`unwrap_or` below).
pub fn fetch_diagram_list_with_options(
    db_mutex: &Arc<Mutex<Database>>,
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

    // Scoped so the guard drops before this returns: this runs on a background thread
    // where any extra time holding the database lock is time a UI-thread callback can
    // block on it.
    let (rows, total, matched_count, excluded_for_missing_curves) = {
        let db = db_mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let result = db.search_diagrams_display(search, clean_shape, clean_gear, range, filters)?;
        let total = db.get_total_count().unwrap_or(result.items.len());
        let matched_count = db
            .count_matching_diagrams(search, clean_shape, clean_gear, range, filters)
            .unwrap_or(result.items.len());
        // CAD audit item 190: one bulk query for every entry's tags rather than one
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
        // Explicit: moving the results into the tuple needs no lock, and the UI
        // thread may be waiting on this same mutex.
        drop(db);
        (
            rows,
            total,
            matched_count,
            result.excluded_for_missing_curves,
        )
    };
    Ok(FetchedDiagramList {
        rows,
        total,
        matched_count,
        excluded_for_missing_curves,
    })
}

/// The `ui.set_*` half of [`refresh_diagram_list`] -- see [`fetch_diagram_list`]'s doc
/// comment for why these are split.
pub fn apply_diagram_list_to_ui(ui: &MainWindow, fetched: FetchedDiagramList) {
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
            // CAD audit item 190: this row's tag chips.
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
    // Real match count for the active search/filters (CAD audit item 193) -- see
    // `FetchedDiagramList::matched_count`'s doc comment. Nowhere near `i32::MAX` for
    // the same reason `total`/`excluded_for_missing_curves` below aren't.
    ui.global::<LibraryModel>()
        .set_matched_count(fetched.matched_count as i32);
    // Counts designs within one query's result set (`SEARCH_RESULT_CAP`-bounded,
    // currently 1000), nowhere near `i32::MAX`.
    ui.global::<LibraryModel>()
        .set_performance_excluded_count(fetched.excluded_for_missing_curves as i32);
}

pub fn refresh_diagram_list(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    search: &str,
    shape_filter: &str,
    gear_filter: &str,
) {
    let range = read_range_filter(ui);
    let order = read_sort_order(ui);
    let local_only = read_local_only(ui);
    let tag_filter = read_tag_filter(ui);
    let id_filter = read_id_filter(ui);
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
        Ok(fetched) => apply_diagram_list_to_ui(ui, fetched),
        Err(e) => {
            error!("Failed to search diagrams: {:?}", e);
            ui.global::<LibraryModel>()
                .set_status_message(format!("Error searching database: {e}").into());
        }
    }
}

//! Reads the library panel's search box, sliders, dropdowns and filter chips off
//! `ui` into the plain filter values the rest of this module's fetch/search paths
//! consume.

use crate::{LibraryModel, MainWindow};
use anyhow::Result;
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::{
        filter::RangeFilter,
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use slint::{ComponentHandle, Model};
use std::sync::{Arc, Mutex};

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
        has_concave: None,
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
/// [`super::fetch::fetch_diagram_list_with_options`], the one place in this path that
/// already holds the `Database` lock -- this function stays a pure `ui` read like every
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
/// [`super::fetch::fetch_diagram_list_with_options`]'s own callers do (see e.g.
/// `gui::library::diagram_list::setup_search_and_filter_callbacks`'s
/// `on_filters_changed`), so "the filtered set" always means the same rows the panel
/// is currently showing.
///
/// Local-catalogue only: there is no remote equivalent of
/// [`Database::matching_entry_ids`] (a `LibrarySource::Remote` session has no local
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

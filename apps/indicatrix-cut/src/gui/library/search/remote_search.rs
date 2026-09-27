//! The remote counterpart of the local search path: one `LibraryRequest::Search`
//! against a worker, plus the local<->wire mappings a remote search request needs.

use super::{
    dispatch::{bump_search_seq, is_current_search},
    fetch::to_diagram_item,
    filters::{read_range_filter, read_sort_order, read_tag_filter},
};
use crate::{
    DiagramItem, LibraryModel, MainWindow, bridge::library::source as library_source,
    settings::WorkerSettings,
};
use indicatrix_net::library::{
    LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
    PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire, SortOrderWire,
};
use indicatrix_vault::{
    db::sqlite::SortOrder,
    model::{
        filter::RangeFilter,
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use slint::{ComponentHandle, ModelRc, VecModel};
use tracing::error;

/// The remote counterpart of `super::dispatch::refresh_diagram_list`: one
/// `LibraryRequest::Search` against `worker`, off the UI thread. Shares the same
/// 1000-result cap as the local query -- both are ultimately backed by
/// `Database::search_diagrams`. `pub(in crate::gui)`, not private: also called directly from
/// `gui::library::remote` right after a source switch succeeds, when the caller already
/// has the `WorkerSettings` in hand and can skip
/// `super::refresh_diagram_list_via_source`'s `LibrarySource` re-check.
///
/// Captures [`bump_search_seq`] BEFORE the request is even built (matching
/// `super::dispatch::refresh_diagram_list`'s own "bump unconditionally, before the
/// async work starts" rule) and checks [`is_current_search`] inside the reply closure
/// before any model write. Without this, a slow remote reply could land after a newer
/// keystroke (or after the cutter switched back to the local library entirely -- see
/// `remote::setup_library_source_callbacks`'s own bump on every source switch) and
/// overwrite the current list with stale REMOTE entry ids that a local-source guard
/// would then happily act on (`toggle_ignored`/`delete_diagram` in `local::organize`).
pub(in crate::gui) fn refresh_diagram_list_remote(
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

/// Maps a local [`SortOrder`] (as `super::filters::read_sort_order` reads it off the
/// header toolbar's sort selector) onto the wire [`SortOrderWire`] a remote
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

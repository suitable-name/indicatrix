//! Links out of the planner window: into the design library and into the manual.
//!
//! - selecting a design (a design name in a result) opens it in the main window's
//!   library, and the planner stays open;
//! - "Show in library" narrows the library to the designs of one result, or of all
//!   results, with a banner that says where the narrowing comes from;
//! - the help button opens the manual chapter on planning a rough.
//!
//! None of the library links works while a remote library is active: the ids belong to
//! the local database.

use super::{
    counts::{is_remote, refresh_counts},
    host::{Host, on_host},
    inputs::FilterSnapshot,
    run::{DesignStatus, ResultsSource},
};
use crate::{
    LibraryModel, MainWindow, RoughPlanModel,
    bridge::library::source::LibrarySource,
    gui::{library::local::refresh_after_library_change, show_toast},
};
use indicatrix_cut_core::rough_plan::RoughLayout;
use indicatrix_vault::{db::sqlite::Database, model::filter::RangeFilter};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::debug;

/// Shown in the view's hint line when the help window cannot be opened.
const HELP_FALLBACK: &str = "Planning a rough: model the rough (base and cuts) on the left, press Plan, and \
     pick a result to see its cuts in 3D. Tick results to save them; Ctrl+S saves, Ctrl+O lists \
     the saved plans.";

/// Registers the library link callbacks and the help button on the planner window.
pub(super) fn setup_link_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_select_design(|entry_id| on_host(|host| select_from_planner(host, entry_id)));
    model.on_show_result_in_library(|index| on_host(|host| show_result_in_library(host, index)));
    model.on_show_all_in_library(|| on_host(show_all_in_library));
    model.on_open_help(|| on_host(open_help));
}

/// `RoughPlanModel.select_design` of the planner window. A remote library has no local
/// designs to select, so the click does nothing there.
fn select_from_planner(host: &Rc<Host>, entry_id: i32) {
    if is_remote(&host.source) {
        return;
    }
    if let Some(main) = host.main.upgrade() {
        select_design(&main, i64::from(entry_id));
        // The planner is a separate window and may be covering the library.
        main.window().set_minimized(false);
        let _ = main.window().show();
    }
}

/// Selects a design in the main library by its catalogue entry ID.
///
/// Dispatches to `LibraryModel.invoke_select_diagram`, opening the design's detail pane
/// in the main window while keeping the rough planner window open and interactive. An id
/// beyond the library's `int` range cannot be a library entry and is ignored.
pub fn select_design(main: &MainWindow, entry_id: i64) {
    if let Ok(id) = i32::try_from(entry_id) {
        main.global::<LibraryModel>().invoke_select_diagram(id);
    } else {
        debug!("Rough planner: design {entry_id} is beyond the library's id range");
    }
}

/// "designs" with its count: "1 design", "17 designs".
fn designs_text(count: usize) -> String {
    format!("{count} design{}", if count == 1 { "" } else { "s" })
}

/// The start of a banner: "Rough plan", or `Saved plan "name"` for a loaded plan.
fn label_prefix(saved_name: Option<&str>) -> String {
    saved_name.map_or_else(
        || "Rough plan".to_string(),
        |name| format!("Saved plan \"{name}\""),
    )
}

/// The banner for the designs of one result: "Rough plan · result #3 (4 designs)".
#[must_use]
pub fn label_for_result(saved_name: Option<&str>, result_number: usize, designs: usize) -> String {
    format!(
        "{} \u{b7} result #{result_number} ({})",
        label_prefix(saved_name),
        designs_text(designs)
    )
}

/// The banner for the designs of every result: "Rough plan · all 10 results (17 designs)".
#[must_use]
pub fn label_for_all(saved_name: Option<&str>, results: usize, designs: usize) -> String {
    format!(
        "{} \u{b7} all {results} result{} ({})",
        label_prefix(saved_name),
        if results == 1 { "" } else { "s" },
        designs_text(designs)
    )
}

/// The library entry ids of the designs `layouts` use, sorted and without duplicates.
/// Designs that are gone from the library are left out; designs found again under their
/// title count as the design they resolved to.
fn library_ids<'a>(
    layouts: impl IntoIterator<Item = &'a RoughLayout>,
    statuses: &BTreeMap<i64, DesignStatus>,
) -> Vec<i64> {
    let ids: BTreeSet<i64> = layouts
        .into_iter()
        .flat_map(|layout| layout.stones.iter().map(|stone| stone.entry_id))
        .filter_map(|id| match statuses.get(&id) {
            Some(DesignStatus::Deleted) => None,
            Some(DesignStatus::MatchedByTitle { resolved_entry_id }) => Some(*resolved_entry_id),
            _ => Some(id),
        })
        .collect();
    ids.into_iter().collect()
}

/// The ids as the library's `int` filter holds them, sorted and without duplicates. An
/// id beyond `i32` cannot be a library entry; it is skipped (and logged).
fn filter_ids(ids: &[i64]) -> Vec<i32> {
    let mut out: Vec<i32> = ids
        .iter()
        .filter_map(|&id| {
            i32::try_from(id)
                .inspect_err(|_| debug!("Rough planner: design {id} is beyond the id range"))
                .ok()
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether the library's search text or filters (the design-id restriction aside)
/// narrow what the library shows. "Show ignored" only widens, so it does not count.
#[must_use]
fn other_filters_active(filters: &FilterSnapshot) -> bool {
    let narrowing_ranges = RangeFilter {
        include_ignored: false,
        ..filters.range.clone()
    };
    !filters.search.trim().is_empty()
        || filters.shape != "All"
        || filters.gear != "All"
        || filters.local_only
        || filters.tag_name.is_some()
        || narrowing_ranges != RangeFilter::default()
}

/// `label` with the note that the library's own search and filters still apply. The
/// designs are shown through the id restriction AND those filters, so the count in the
/// banner is an upper bound until the note is gone.
#[must_use]
fn narrowed_label(label: &str) -> String {
    format!("{label} \u{b7} within the current search and filters")
}

/// Sets the library panel's filter to the given design ids with a banner `label`.
///
/// The library's own search text and filters keep applying on top of the ids; when any
/// is active the banner says so, because the library can then show fewer designs than
/// the banner counts.
///
/// Returns `false`, changing nothing, when no id is left to show (an empty filter would
/// show the whole library under a banner that says otherwise).
pub fn show_designs_in_library(
    main: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    ids: &[i64],
    label: String,
) -> bool {
    let id_items = filter_ids(ids);
    if id_items.is_empty() {
        return false;
    }
    let label = if other_filters_active(&FilterSnapshot::read(main)) {
        narrowed_label(&label)
    } else {
        label
    };
    let model = main.global::<LibraryModel>();
    model.set_recent_import_filter(ModelRc::new(VecModel::from(id_items)));
    model.set_id_filter_label(label.into());
    refresh_after_library_change(main, db, source);
    main.window().set_minimized(false);
    let _ = main.window().show();
    true
}

/// What the library link buttons need from the session: the shown layouts and how to
/// call the plan.
struct LinkTarget {
    ids: Vec<i64>,
    label: String,
}

/// The name of the loaded plan whose results are on screen, if any.
fn loaded_name(source: &ResultsSource) -> Option<&str> {
    match source {
        ResultsSource::Loaded { name, .. } => Some(name),
        ResultsSource::Planned => None,
    }
}

/// The ids and banner for result row `index`.
fn target_of_result(host: &Host, index: usize) -> Option<LinkTarget> {
    let session = host.session.borrow();
    let run = &session.run;
    let layout = run.layouts.get(index)?;
    let ids = library_ids([layout], &run.statuses);
    let label = label_for_result(loaded_name(&run.source), index + 1, ids.len());
    Some(LinkTarget { ids, label })
}

/// The ids and banner for all shown results.
fn target_of_all(host: &Host) -> Option<LinkTarget> {
    let session = host.session.borrow();
    let run = &session.run;
    if run.layouts.is_empty() {
        return None;
    }
    let ids = library_ids(&run.layouts, &run.statuses);
    let label = label_for_all(loaded_name(&run.source), run.layouts.len(), ids.len());
    Some(LinkTarget { ids, label })
}

/// Applies a link target: filters the library to it, or says why that is not possible.
fn apply_target(host: &Host, target: Option<LinkTarget>) {
    if is_remote(&host.source) {
        return;
    }
    let (Some(target), Some(main)) = (target, host.main.upgrade()) else {
        return;
    };
    let shown = show_designs_in_library(&main, &host.db, &host.source, &target.ids, target.label);
    if !shown {
        show_toast(&main, "None of these designs is in the library.", "info");
        return;
    }
    if !host.window.global::<RoughPlanModel>().get_running() {
        // "Current library filter" now means exactly these designs; its count follows.
        refresh_counts(&main, &host.window, &host.db, &host.source, &host.session);
    }
}

/// `RoughPlanModel.show_result_in_library`.
fn show_result_in_library(host: &Rc<Host>, index: i32) {
    if let Ok(index) = usize::try_from(index) {
        apply_target(host, target_of_result(host, index));
    }
}

/// `RoughPlanModel.show_all_in_library`.
fn show_all_in_library(host: &Rc<Host>) {
    apply_target(host, target_of_all(host));
}

/// `RoughPlanModel.open_help`: opens the chapter "Planning a rough" in the help window
/// (`gui::help`), which has the manual built in. If the help window cannot be opened, a
/// short help text goes to the view's hint line and a toast says why.
fn open_help(host: &Rc<Host>) {
    let Some(main) = host.main.upgrade() else {
        host.window
            .global::<RoughPlanModel>()
            .set_view_hint(HELP_FALLBACK.into());
        return;
    };
    if let Err(message) =
        crate::gui::help::open_topic(&main, crate::gui::help::topics::ROUGH_PLANNER)
    {
        host.window
            .global::<RoughPlanModel>()
            .set_view_hint(HELP_FALLBACK.into());
        show_toast(&main, &message, "error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{Axis, CutOrder, CutPlan, PlacedStone, fit::StonePose};

    fn layout_of(ids: &[i64]) -> RoughLayout {
        let stone = |entry_id| PlacedStone {
            entry_id,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [1.0; 3],
            stone_size_mm: [1.0; 3],
            table_axis: Axis::Y,
            carat: 0.1,
            volume_mm3: 1.0,
            pose: StonePose {
                center_mm: [0.5; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        };
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: ids.iter().map(|&id| stone(id)).collect(),
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.1,
            total_volume_mm3: 1.0,
            yield_fraction: 0.1,
            exact_fit: false,
        }
    }

    #[test]
    fn the_banners_read_as_the_plan_specifies() {
        assert_eq!(
            label_for_result(None, 3, 4),
            "Rough plan \u{b7} result #3 (4 designs)"
        );
        assert_eq!(
            label_for_all(None, 10, 17),
            "Rough plan \u{b7} all 10 results (17 designs)"
        );
        assert_eq!(
            label_for_result(Some("Aqua pebble"), 2, 3),
            "Saved plan \"Aqua pebble\" \u{b7} result #2 (3 designs)"
        );
        assert_eq!(
            label_for_all(None, 1, 1),
            "Rough plan \u{b7} all 1 result (1 design)"
        );
    }

    fn unfiltered() -> FilterSnapshot {
        FilterSnapshot {
            search: String::new(),
            shape: "All".to_string(),
            gear: "All".to_string(),
            range: RangeFilter::default(),
            local_only: false,
            tag_name: None,
            id_filter: None,
        }
    }

    #[test]
    fn only_a_narrowing_search_or_filter_counts_as_another_filter() {
        assert!(!other_filters_active(&unfiltered()));
        // A design-id restriction is what the planner itself sets; blanks are no search.
        let restricted = FilterSnapshot {
            search: "  ".to_string(),
            id_filter: Some(vec![1, 2]),
            ..unfiltered()
        };
        assert!(!other_filters_active(&restricted));
        // "Show ignored" widens the list.
        let ignored = FilterSnapshot {
            range: RangeFilter {
                include_ignored: true,
                ..RangeFilter::default()
            },
            ..unfiltered()
        };
        assert!(!other_filters_active(&ignored));
        let narrowing = [
            FilterSnapshot {
                search: "oval".to_string(),
                ..unfiltered()
            },
            FilterSnapshot {
                shape: "Round".to_string(),
                ..unfiltered()
            },
            FilterSnapshot {
                gear: "96".to_string(),
                ..unfiltered()
            },
            FilterSnapshot {
                local_only: true,
                ..unfiltered()
            },
            FilterSnapshot {
                tag_name: Some("favourite".to_string()),
                ..unfiltered()
            },
            FilterSnapshot {
                range: RangeFilter {
                    ri_min: Some(1.5),
                    ..RangeFilter::default()
                },
                ..unfiltered()
            },
        ];
        for filters in &narrowing {
            assert!(other_filters_active(filters), "{filters:?}");
        }
    }

    #[test]
    fn the_banner_says_when_the_library_filters_still_apply() {
        assert_eq!(
            narrowed_label(&label_for_all(None, 10, 17)),
            "Rough plan \u{b7} all 10 results (17 designs) \u{b7} within the current search and filters"
        );
    }

    #[test]
    fn the_ids_of_a_result_are_sorted_unique_and_skip_deleted_designs() {
        let layouts = [layout_of(&[9, 4, 9, 7]), layout_of(&[4, 12])];
        let statuses = BTreeMap::from([
            (7, DesignStatus::Deleted),
            (
                12,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 40,
                },
            ),
            (4, DesignStatus::Changed),
        ]);
        assert_eq!(library_ids(&layouts, &statuses), vec![4, 9, 40]);
        assert_eq!(library_ids(&layouts[..1], &statuses), vec![4, 9]);
        assert_eq!(library_ids([], &statuses), Vec::<i64>::new());
        let only_deleted = [layout_of(&[7])];
        assert_eq!(library_ids(&only_deleted, &statuses), Vec::<i64>::new());
    }

    #[test]
    fn ids_beyond_i32_are_skipped_and_the_rest_is_sorted_and_deduplicated() {
        let big = i64::from(i32::MAX) + 1;
        assert_eq!(filter_ids(&[5, big, 3, 5, -1]), vec![-1, 3, 5]);
        assert_eq!(filter_ids(&[big]), Vec::<i32>::new());
        assert_eq!(filter_ids(&[]), Vec::<i32>::new());
    }
}

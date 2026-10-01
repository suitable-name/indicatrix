//! Converts a catalogue row into the Slint-facing `DiagramItem`, runs the local
//! `Database` read behind a search/filter, and applies the result onto `ui`.

use crate::{
    DiagramItem, LibraryModel, MainWindow, gui::library::detail::clear_current_detail_display,
};
use anyhow::Result;
use indicatrix_net::library::DesignSummary;
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters, SortOrder},
    model::{entry::DiagramListItem, filter::RangeFilter},
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};
use tracing::warn;

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
pub(super) fn to_diagram_item(item: &DesignSummary) -> DiagramItem {
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
        // The planner exclusion mark lives in the local database only (see
        // `Database::set_planner_excluded`) and the remote library protocol carries
        // none, so a remote-browsed row is never shown as excluded.
        planner_excluded: false,
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
    /// `true` iff the Rough Planner leaves this design out of its candidate set
    /// (`Database::planner_excluded_among`). Independent of `ignored`: an excluded
    /// design is listed and searchable like any other.
    pub planner_excluded: bool,
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

/// The `Database` read half of `super::dispatch::refresh_diagram_list`, with no
/// sort/"My designs" preference.
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
    /// The tag chip filter's NAME -- see `super::filters::read_tag_filter`'s
    /// own doc comment for why this stays a name this far down, resolved to an id
    /// only once the `Database` lock is already held below.
    pub tag_filter: Option<&'a str>,
    /// "Show these N" restriction to an explicit id set -- see
    /// `super::filters::read_id_filter`'s own doc comment.
    pub id_filter: Option<&'a [i64]>,
}

/// [`fetch_diagram_list`], plus a caller-chosen [`SortOrder`] and "My designs"/tag/
/// batch-id restriction.
///
/// What `super::dispatch::refresh_diagram_list` actually calls, reading every field of
/// `options` off `ui` via `super::filters::{read_sort_order, read_local_only,
/// read_tag_filter, read_id_filter}`.
///
/// Has no `ui` dependency beyond the already-resolved `range`/`options` -- split out
/// so `gui::library`'s post-import/rename/delete/shape-change refresh can run this on
/// a background thread (re-running an unfiltered search against a several-thousand-
/// design catalogue synchronously on the UI thread is itself a perceptible freeze) and
/// only marshal the cheap [`apply_diagram_list_to_ui`] step back onto the UI thread.
/// Every other caller (search box edits, filter slider drags) stays on the synchronous
/// `super::dispatch::refresh_diagram_list` below, which a filter that must feel
/// immediate as you type actually needs.
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
    // `tags_for_entry` call per row -- see `Database::tags_for_entries`'s own doc
    // comment. A failed lookup degrades to "no tags shown" rather than failing
    // the whole list fetch. An empty result has no rows to label, so it skips the query.
    let ids: Vec<i64> = result.items.iter().map(|item| item.id).collect();
    let mut tags_by_entry = if ids.is_empty() {
        HashMap::new()
    } else {
        db.tags_for_entries(&ids).unwrap_or_default()
    };
    // The planner-exclusion marks of the same ids, in one query. This runs on the
    // read-only connection too; a failure degrades to "none excluded" like the tags do.
    let planner_excluded_ids = db.planner_excluded_among(&ids).unwrap_or_else(|e| {
        warn!("planner_excluded_among failed: {e}");
        BTreeSet::new()
    });

    // `ignored` is read straight off the row (`DiagramListItem::ignored`).
    let rows: Vec<DiagramListRow> = result
        .items
        .into_iter()
        .map(|item| {
            let ignored = item.ignored;
            let planner_excluded = planner_excluded_ids.contains(&item.id);
            let tags = tags_by_entry.remove(&item.id).unwrap_or_default();
            DiagramListRow {
                item,
                ignored,
                planner_excluded,
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
/// `super::dispatch::refresh_diagram_list` could still block on it for that long. WAL
/// (see `indicatrix_vault::db::sqlite`'s own module doc comment) lets a second,
/// independent connection read concurrently with the writer instead of queuing
/// behind it.
///
/// `crate::gui::DB_PATH` is private to `gui`, not `pub(crate)` -- reachable here
/// unchanged because `gui::library::search::fetch` is one of `gui`'s own descendant
/// modules (plain Rust module-privacy, no visibility bump needed).
///
/// Falls back to [`fetch_diagram_list_with_options`] (the shared-lock, pre-fix
/// behaviour) when a dedicated connection can't be opened -- the one real case being
/// `gui::run_gui`'s `:memory:` fallback database (started when the real file at
/// `DB_PATH` itself failed to open at startup): there is no second connection to open
/// to a database that only exists inside the already-open one, and that session must
/// keep working rather than erroring out of every performance-filtered search.
pub(super) fn fetch_diagram_list_with_own_connection(
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

/// The Slint-facing row for one fetched catalogue row.
fn row_to_item(row: DiagramListRow) -> DiagramItem {
    DiagramItem {
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
        planner_excluded: row.planner_excluded,
        // This row's tag chips.
        tags: ModelRc::new(VecModel::from(
            row.tags
                .into_iter()
                .map(Into::into)
                .collect::<Vec<SharedString>>(),
        )),
    }
}

/// Whether `model` already shows exactly `rows`: the same designs in the same order,
/// with every displayed field (not only the id) equal -- a rename or a tag change keeps
/// the id but must still replace the list.
fn list_unchanged(model: &ModelRc<DiagramItem>, rows: &[DiagramListRow]) -> bool {
    model.row_count() == rows.len()
        && model
            .iter()
            .zip(rows)
            .all(|(shown, row)| row_matches_item(row, &shown))
}

/// Whether `shown` is what [`row_to_item`] would build from `row`.
fn row_matches_item(row: &DiagramListRow, shown: &DiagramItem) -> bool {
    let item = &row.item;
    shown.id == item.id as i32
        && shown.title.as_str() == item.title
        && text_matches(&shown.shape, item.shape.as_deref())
        && text_matches(&shown.gear, item.index_gear.as_deref())
        && text_matches(&shown.facets, item.facets_count.as_deref())
        && text_matches(&shown.designer, item.designer_info.as_deref())
        && text_matches(&shown.lw_ratio, item.lw_ratio.as_deref())
        && text_matches(&shown.ri, item.refractive_index.as_deref())
        && shown.ignored == row.ignored
        && shown.planner_excluded == row.planner_excluded
        && shown.is_local == is_local_url(&item.url)
        && shown.tags.row_count() == row.tags.len()
        && shown
            .tags
            .iter()
            .zip(&row.tags)
            .all(|(shown_tag, tag)| shown_tag.as_str() == tag.as_str())
}

/// Whether the displayed `shown` text equals `value`, an absent value reading as empty.
fn text_matches(shown: &SharedString, value: Option<&str>) -> bool {
    shown.as_str() == value.unwrap_or_default()
}

/// The `ui.set_*` half of `super::dispatch::refresh_diagram_list` -- see
/// [`fetch_diagram_list`]'s doc comment for why these are split.
///
/// Before applying the new list, reconciles `LibraryModel.selected_entry_id`
/// against it -- a previously-selected design that no longer appears in the new result
/// set (a search/filter change, not a delete: the design itself still exists) must not
/// be left "selected" with nothing on screen to show for it. Clears the detail pane via
/// [`clear_current_detail_display`] in that case; `diagram_list.slint`'s
/// own `keyboard_focus_index` resets separately, on every `diagram_list` change (not
/// only a length change), so the two together drop both halves of stale selection.
///
/// The list model itself is replaced only when `fetched` differs from what is shown
/// (see [`list_unchanged`]); the counts and the other properties are always updated.
pub fn apply_diagram_list_to_ui(ui: &MainWindow, fetched: FetchedDiagramList) {
    let selected_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if selected_id >= 0
        && !fetched
            .rows
            .iter()
            .any(|row| row.item.id == i64::from(selected_id))
    {
        clear_current_detail_display(ui);
    }
    let count_unavailable = fetched.count_unavailable;

    // A refresh that returns the very rows already on screen (a filter tick that changed
    // nothing, a keystroke that did not narrow the result) keeps the shown model: a new
    // one would make the list re-create every visible card and reset the keyboard focus
    // for no visible change.
    if !list_unchanged(
        &ui.global::<LibraryModel>().get_diagram_list(),
        &fetched.rows,
    ) {
        let slint_items: Vec<DiagramItem> = fetched.rows.into_iter().map(row_to_item).collect();
        ui.global::<LibraryModel>()
            .set_diagram_list(ModelRc::new(VecModel::from(slint_items)));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, title: &str, tags: &[&str]) -> DiagramListRow {
        DiagramListRow {
            item: DiagramListItem {
                id,
                title: title.to_string(),
                url: format!("local://{id}"),
                design_id: None,
                shape: Some("Round".to_string()),
                index_gear: None,
                facets_count: Some("57+8".to_string()),
                designer_info: None,
                lw_ratio: None,
                refractive_index: None,
                volume: None,
                competition_diagram: None,
                ignored: false,
            },
            ignored: false,
            planner_excluded: false,
            tags: tags.iter().map(|&tag| tag.to_string()).collect(),
        }
    }

    fn shown(rows: Vec<DiagramListRow>) -> ModelRc<DiagramItem> {
        let items: Vec<DiagramItem> = rows.into_iter().map(row_to_item).collect();
        ModelRc::new(VecModel::from(items))
    }

    fn catalogue() -> Vec<DiagramListRow> {
        vec![row(1, "Alpha", &[]), row(2, "Beta", &["keep"])]
    }

    #[test]
    fn the_rows_a_model_was_built_from_are_unchanged() {
        assert!(list_unchanged(&shown(catalogue()), &catalogue()));
    }

    #[test]
    fn a_renamed_design_with_the_same_id_is_a_change() {
        let mut rows = catalogue();
        rows[1].item.title = "Beta II".to_string();
        assert!(!list_unchanged(&shown(catalogue()), &rows));
    }

    #[test]
    fn a_changed_tag_list_is_a_change() {
        let mut rows = catalogue();
        rows[0].tags.push("new".to_string());
        assert!(!list_unchanged(&shown(catalogue()), &rows));
        let mut retagged = catalogue();
        retagged[1].tags = vec!["other".to_string()];
        assert!(!list_unchanged(&shown(catalogue()), &retagged));
    }

    #[test]
    fn a_flipped_ignored_flag_is_a_change() {
        let mut rows = catalogue();
        rows[0].ignored = true;
        assert!(!list_unchanged(&shown(catalogue()), &rows));
    }

    #[test]
    fn a_flipped_planner_flag_is_a_change() {
        let mut rows = catalogue();
        rows[0].planner_excluded = true;
        assert!(!list_unchanged(&shown(catalogue()), &rows));
        // Un-excluding a shown excluded row is a change as well.
        assert!(!list_unchanged(&shown(rows), &catalogue()));
    }

    #[test]
    fn a_different_order_or_length_is_a_change() {
        let mut reordered = catalogue();
        reordered.reverse();
        assert!(!list_unchanged(&shown(catalogue()), &reordered));
        let mut shorter = catalogue();
        shorter.pop();
        assert!(!list_unchanged(&shown(catalogue()), &shorter));
        assert!(!list_unchanged(&shown(shorter), &catalogue()));
    }
}

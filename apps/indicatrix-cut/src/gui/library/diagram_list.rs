//! Diagram-list loading, range-filter bounds, search/filter callbacks, and
//! diagram-selection/URL-open/file-export callbacks.
//!
//! Split out of `gui::mod` purely to keep that module (already sizeable) from growing
//! further -- same reasoning as `gui::detail`/`gui::search`/`gui::remote`.

use crate::{
    LibraryModel, MainWindow, PerformanceFilterRow, ViewportModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        library::{
            detail::{export_diagram_file_via_source, load_diagram_detail_via_source},
            search::{refresh_diagram_list, refresh_diagram_list_via_source},
        },
        render::camera_lighting::resubmit_live_solid,
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_vault::{db::sqlite::Database, model::filter::AttributeRanges};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

/// Clears the search box, shape/gear dropdown selections, tag chip filter and
/// just-imported restriction back to their defaults -- shared by every
/// "start browsing a library from scratch" entry point
/// ([`load_filter_options_and_initial_list`], and
/// `gui::library::remote::load_filter_options_and_initial_list_remote`) so the box/
/// dropdowns/chip the cutter sees on screen always match the literal `""`/
/// `"All Shapes"`/`"All Gears"` filter values those callers then query with, instead
/// of the query silently running against different values than what's displayed
/// (stale text in the box, a stale `selected_shape_index` into a since-changed
/// options model, `current_filtered_entry_ids` reading a batch action against a
/// different set than what's on screen).
pub(in crate::gui) fn reset_search_and_filter_ui_inputs(ui: &MainWindow) {
    let model = ui.global::<LibraryModel>();
    model.set_search_text(SharedString::new());
    model.set_selected_shape_index(0);
    model.set_selected_gear_index(0);
    model.set_active_tag_filter_name(SharedString::new());
    model.set_recent_import_filter(ModelRc::new(VecModel::from(Vec::<i32>::new())));
}

/// Loads the shape/gear filter dropdown options and the initial diagram list from
/// the database. Split out of `run_gui` purely to keep that function under clippy's
/// function-length lint.
pub(in crate::gui) fn load_filter_options_and_initial_list(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
) {
    // This is also called on switching back to the local library
    // (`remote::setup_library_source_callbacks`'s `idx < 0` branch), where the box/
    // dropdowns/chip may still show whatever the just-left remote session had.
    reset_search_and_filter_ui_inputs(ui);
    {
        let db_guard = db.lock().unwrap();
        let mut shape_opts = vec!["All Shapes".to_string()];
        if let Ok(shapes) = db_guard.get_unique_shapes() {
            shape_opts.extend(shapes);
        }
        let shape_model: Vec<SharedString> = shape_opts
            .into_iter()
            .map(std::convert::Into::into)
            .collect();
        ui.global::<LibraryModel>()
            .set_shape_options(ModelRc::new(VecModel::from(shape_model)));

        let mut gear_opts = vec!["All Gears".to_string()];
        if let Ok(gears) = db_guard.get_unique_gears() {
            gear_opts.extend(gears);
        }
        drop(db_guard);
        let gear_model: Vec<SharedString> = gear_opts
            .into_iter()
            .map(std::convert::Into::into)
            .collect();
        ui.global::<LibraryModel>()
            .set_gear_options(ModelRc::new(VecModel::from(gear_model)));
    }

    sync_range_bounds_to_ui(ui, db);
    sync_tag_vocabulary_to_ui(ui, db);
    refresh_diagram_list(ui, db, "", "All Shapes", "All Gears");

    // A failed `get_total_count` is handled explicitly rather than swallowed into a
    // plain `0`, which would read back as "the library is empty" rather than "the
    // count query failed" -- indistinguishable from a genuinely empty database.
    let total_count_result = db.lock().unwrap().get_total_count();
    match total_count_result {
        Ok(total_count) => {
            ui.global::<LibraryModel>().set_status_message(
                format!("Database loaded: {total_count} diagrams available.").into(),
            );
        }
        Err(e) => {
            warn!("get_total_count failed during initial load: {e}");
            ui.global::<LibraryModel>().set_status_message(
                "Database loaded, but the diagram count is unavailable.".into(),
            );
        }
    }
}

/// Pushes the full catalogue tag vocabulary (names only, alphabetical) into
/// `LibraryModel.all_tags` -- the chip filter row and the "Add
/// tag..." picker both read this rather than each running their own
/// `Database::list_tags` query. Called at startup and after any write that could
/// have created or emptied a tag (see
/// `gui::library::local::organize::setup_add_tag_callback`/
/// `setup_remove_tag_callback`).
///
/// Plain tag names, not `{id, name}` pairs: every Rust entry point that acts on a
/// tag (`add_tag_to_entry`/`remove_tag_from_entry`/the chip filter) takes the name
/// and resolves it server-side (`Database::tag_id_by_name`/`add_tag_to_entry`'s own
/// create-or-reuse lookup) -- see `read_tag_filter`'s own doc comment for why. That
/// keeps the Slint side needing no new struct type at all, just one more `[string]`
/// property alongside `shape_options`/`gear_options`.
pub(in crate::gui) fn sync_tag_vocabulary_to_ui(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let tags = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .list_tags()
        .unwrap_or_default();
    let names: Vec<SharedString> = tags.into_iter().map(|t| t.name.into()).collect();
    ui.global::<LibraryModel>()
        .set_all_tags(ModelRc::new(VecModel::from(names)));
}

/// Pushes the catalogue's actual min/max (RI, L/W, volume, facet count) into the
/// range-filter sliders.
///
/// Sets both the `*_bounds_*` properties (the sliders' scale) and the `*_filter_*`
/// properties (their starting position, the full range, i.e. unfiltered). Called once
/// at startup, after the diagram list's filter dropdowns are populated and before the
/// first `refresh_diagram_list` call, so the very first query already has
/// correctly-scaled (inert) range filters rather than the sliders' `0.0..1.0`-ish
/// `.slint` placeholder defaults. Also re-called after anything that could have
/// widened the real min/max -- an import in this crate's own `gui::library`, or a
/// bulk library change made by a downstream binary -- see the `pub`-ness note below.
///
/// A query failure leaves the sliders at their placeholder bounds rather than
/// panicking -- matching `load_filter_options_and_initial_list`'s existing
/// `unwrap_or_default`/silent-failure tolerance for a database that can't be read.
/// **`pub`, not `pub(crate)`, deliberately.** A binary that reuses this crate as a
/// library (see `crate`'s own doc comment) must call this after any bulk change that
/// widens the catalogue's real min/max for a range-filterable attribute, exactly as
/// `gui::library`'s import handler in this crate does. Narrowing this to `pub(crate)`
/// would compile here and silently leave those sliders stale there.
///
/// Recovers rather than panics on a poisoned `db` mutex (`unwrap_or_else(
/// std::sync::PoisonError::into_inner)`, not a bare `.unwrap()`) -- a panic anywhere
/// else while holding the lock (e.g. mid-import) would otherwise poison it and take
/// this call down too, on the very next library-change refresh. Matches the
/// poison-recovery convention `gui::library` already uses throughout.
pub fn sync_range_bounds_to_ui(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let Some(ranges) = fetch_attribute_ranges(db) else {
        return;
    };
    apply_attribute_ranges_to_ui(ui, &ranges);
}

/// [`sync_range_bounds_to_ui`]'s counterpart -- fetches then applies via
/// [`apply_attribute_range_bounds_preserving_filters`] instead of the reset variant.
/// The right call after a write a cutter did not ask to have their range filters
/// cleared for (e.g. `gui::library::detail`'s own metadata-save refresh).
pub fn sync_range_bounds_to_ui_preserving_filters(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let Some(ranges) = fetch_attribute_ranges(db) else {
        return;
    };
    apply_attribute_range_bounds_preserving_filters(ui, &ranges);
}

/// The `Database` read half of [`sync_range_bounds_to_ui`], with no `ui` dependency --
/// split out so `gui::library`'s post-import/rename/delete refresh can run this on a
/// background thread (see that module's own doc comment on `refresh_after_library_change`
/// for why: re-querying the catalogue synchronously on the UI thread is itself a
/// perceptible freeze against a several-thousand-design library) and only marshal the
/// cheap [`apply_attribute_ranges_to_ui`] step back onto the UI thread.
pub fn fetch_attribute_ranges(db: &Arc<Mutex<Database>>) -> Option<AttributeRanges> {
    db.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_attribute_ranges()
        .ok()
}

/// The `ui.set_*` half of [`sync_range_bounds_to_ui`] -- see [`fetch_attribute_ranges`]'s
/// doc comment for why these are split. Sets both the `*_bounds_*` properties (the
/// sliders' scale) and the `*_filter_*` properties (reset to the full range, i.e.
/// unfiltered) from an already-fetched [`AttributeRanges`].
///
/// Only ever called where a full reset is actually wanted: the initial load (nothing
/// to preserve yet) and the range panel's own explicit Reset button -- see
/// [`apply_attribute_range_bounds_preserving_filters`] for the version every other
/// refresh uses, which updates the sliders' scale WITHOUT silently discarding whatever
/// a cutter had already narrowed them to.
pub fn apply_attribute_ranges_to_ui(ui: &MainWindow, ranges: &AttributeRanges) {
    ui.global::<LibraryModel>()
        .set_ri_bounds_min(ranges.ri.0 as f32);
    ui.global::<LibraryModel>()
        .set_ri_bounds_max(ranges.ri.1 as f32);
    ui.global::<LibraryModel>()
        .set_ri_filter_min(ranges.ri.0 as f32);
    ui.global::<LibraryModel>()
        .set_ri_filter_max(ranges.ri.1 as f32);

    ui.global::<LibraryModel>()
        .set_lw_bounds_min(ranges.lw_ratio.0 as f32);
    ui.global::<LibraryModel>()
        .set_lw_bounds_max(ranges.lw_ratio.1 as f32);
    ui.global::<LibraryModel>()
        .set_lw_filter_min(ranges.lw_ratio.0 as f32);
    ui.global::<LibraryModel>()
        .set_lw_filter_max(ranges.lw_ratio.1 as f32);

    ui.global::<LibraryModel>()
        .set_volume_bounds_min(ranges.volume.0 as f32);
    ui.global::<LibraryModel>()
        .set_volume_bounds_max(ranges.volume.1 as f32);
    ui.global::<LibraryModel>()
        .set_volume_filter_min(ranges.volume.0 as f32);
    ui.global::<LibraryModel>()
        .set_volume_filter_max(ranges.volume.1 as f32);

    ui.global::<LibraryModel>()
        .set_facets_bounds_min(ranges.facets.0 as f32);
    ui.global::<LibraryModel>()
        .set_facets_bounds_max(ranges.facets.1 as f32);
    ui.global::<LibraryModel>()
        .set_facets_filter_min(ranges.facets.0 as f32);
    ui.global::<LibraryModel>()
        .set_facets_filter_max(ranges.facets.1 as f32);
}

/// Clamps `current` (a filter's current `(min, max)`) into `bounds` (that attribute's
/// new `(min, max)`) -- shared by every attribute
/// [`apply_attribute_range_bounds_preserving_filters`] updates. A bounds widening (the
/// common case: an import added a design outside the previous min/max) leaves an
/// already-inside filter value untouched; only a bounds NARROWING (a delete, or a
/// metadata edit that moved the catalogue's own extreme value) ever moves a filter
/// value at all, and then only just enough to stay valid.
fn clamp_filter_range(current: (f32, f32), bounds: (f32, f32)) -> (f32, f32) {
    let (bounds_min, bounds_max) = bounds;
    let min = current.0.clamp(bounds_min, bounds_max);
    let max = current.1.clamp(bounds_min, bounds_max);
    // `min` must never exceed `max` after independently clamping each -- only
    // reachable when `bounds` narrowed past a filter that had already been widened to
    // (or past) the old bounds on one side only, an edge case worth handling exactly
    // rather than leaving an inverted slider.
    if min > max { (max, max) } else { (min, max) }
}

/// [`apply_attribute_ranges_to_ui`]'s counterpart: updates the `*_bounds_*`
/// properties (the sliders' scale) from a freshly re-queried [`AttributeRanges`] the
/// same way, but CLAMPS the existing `*_filter_*` values into the new bounds instead
/// of resetting them to the full range. Used by every refresh that follows a write a
/// cutter did not ask to have their filters cleared for (an import, a rename/delete/
/// shape-change, a metadata save) -- [`apply_attribute_ranges_to_ui`] itself stays the
/// right call for the one-time initial load, where there is no prior filter to
/// preserve, and for the range panel's own explicit Reset button.
pub fn apply_attribute_range_bounds_preserving_filters(ui: &MainWindow, ranges: &AttributeRanges) {
    let model = ui.global::<LibraryModel>();

    let ri_bounds = (ranges.ri.0 as f32, ranges.ri.1 as f32);
    model.set_ri_bounds_min(ri_bounds.0);
    model.set_ri_bounds_max(ri_bounds.1);
    let (ri_min, ri_max) = clamp_filter_range(
        (model.get_ri_filter_min(), model.get_ri_filter_max()),
        ri_bounds,
    );
    model.set_ri_filter_min(ri_min);
    model.set_ri_filter_max(ri_max);

    let lw_bounds = (ranges.lw_ratio.0 as f32, ranges.lw_ratio.1 as f32);
    model.set_lw_bounds_min(lw_bounds.0);
    model.set_lw_bounds_max(lw_bounds.1);
    let (lw_min, lw_max) = clamp_filter_range(
        (model.get_lw_filter_min(), model.get_lw_filter_max()),
        lw_bounds,
    );
    model.set_lw_filter_min(lw_min);
    model.set_lw_filter_max(lw_max);

    let volume_bounds = (ranges.volume.0 as f32, ranges.volume.1 as f32);
    model.set_volume_bounds_min(volume_bounds.0);
    model.set_volume_bounds_max(volume_bounds.1);
    let (volume_min, volume_max) = clamp_filter_range(
        (model.get_volume_filter_min(), model.get_volume_filter_max()),
        volume_bounds,
    );
    model.set_volume_filter_min(volume_min);
    model.set_volume_filter_max(volume_max);

    let facets_bounds = (ranges.facets.0 as f32, ranges.facets.1 as f32);
    model.set_facets_bounds_min(facets_bounds.0);
    model.set_facets_bounds_max(facets_bounds.1);
    let (facets_min, facets_max) = clamp_filter_range(
        (model.get_facets_filter_min(), model.get_facets_filter_max()),
        facets_bounds,
    );
    model.set_facets_filter_min(facets_min);
    model.set_facets_filter_max(facets_max);
}

/// Wires up the search-text, shape-filter, and gear-filter callbacks that re-run the
/// diagram list query. Split out of `run_gui` purely to keep that function under
/// clippy's function-length lint.
///
/// Every handler here calls [`refresh_diagram_list_via_source`], not
/// [`refresh_diagram_list`] directly -- with `source` at its default (`LibrarySource::Local`,
/// see that type's own doc comment), the dispatcher calls the exact same
/// `refresh_diagram_list` these handlers called before this dispatch existed, so nothing
/// changes here until a user actually switches sources.
/// Clears the "show these N just-imported designs" restriction a batch import can
/// leave on `LibraryModel.recent_import_filter` --
/// called from the top of every real search/filter-change handler below so that
/// view never outlives the one refresh it was created for. A no-op (cheap: setting
/// an already-empty model) when no batch view is active, so every handler can call
/// it unconditionally rather than checking first.
fn clear_recent_import_filter(ui: &MainWindow) {
    ui.global::<LibraryModel>()
        .set_recent_import_filter(ModelRc::new(VecModel::from(Vec::<i32>::new())));
}

pub(in crate::gui) fn setup_search_and_filter_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_search = Arc::clone(db);
    let source_search = Arc::clone(source);
    let ui_weak_search = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_search_changed(move |text: SharedString| {
            if let Some(ui) = ui_weak_search.upgrade() {
                clear_recent_import_filter(&ui);
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

                refresh_diagram_list_via_source(
                    &ui,
                    &db_search,
                    &source_search,
                    &text,
                    &shape,
                    &gear,
                );
            }
        });

    let db_shape = Arc::clone(db);
    let source_shape = Arc::clone(source);
    let ui_weak_shape = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_filter_shape_changed(move |shape: SharedString| {
            if let Some(ui) = ui_weak_shape.upgrade() {
                clear_recent_import_filter(&ui);
                let search = ui.global::<LibraryModel>().get_search_text();
                let gear_idx = ui.global::<LibraryModel>().get_selected_gear_index() as usize;
                let gear = ui
                    .global::<LibraryModel>()
                    .get_gear_options()
                    .row_data(gear_idx)
                    .unwrap_or_default();

                refresh_diagram_list_via_source(
                    &ui,
                    &db_shape,
                    &source_shape,
                    &search,
                    &shape,
                    &gear,
                );
            }
        });

    let db_gear = Arc::clone(db);
    let source_gear = Arc::clone(source);
    let ui_weak_gear = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_filter_gear_changed(move |gear: SharedString| {
            if let Some(ui) = ui_weak_gear.upgrade() {
                clear_recent_import_filter(&ui);
                let search = ui.global::<LibraryModel>().get_search_text();
                let shape_idx = ui.global::<LibraryModel>().get_selected_shape_index() as usize;
                let shape = ui
                    .global::<LibraryModel>()
                    .get_shape_options()
                    .row_data(shape_idx)
                    .unwrap_or_default();

                refresh_diagram_list_via_source(
                    &ui,
                    &db_gear,
                    &source_gear,
                    &search,
                    &shape,
                    &gear,
                );
            }
        });

    // Fired on every range-filter slider drag tick and by the panel's Reset button
    // (`header.slint`). `refresh_diagram_list` itself reads the current filter values
    // straight off `ui` (via `read_range_filter`), so this handler only needs to
    // re-supply the other three existing filters.
    let db_range = Arc::clone(db);
    let source_range = Arc::clone(source);
    let ui_weak_range = ui.as_weak();
    ui.global::<LibraryModel>().on_filters_changed(move || {
        if let Some(ui) = ui_weak_range.upgrade() {
            clear_recent_import_filter(&ui);
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

            refresh_diagram_list_via_source(&ui, &db_range, &source_range, &search, &shape, &gear);
        }
    });
}

/// Wires the tilt-performance filter panel's add/remove/clear-all callbacks.
///
/// `performance_filters` is Rust-mediated but UI-owned (see `app.slint`'s own doc
/// comment on that property, and `filter_panel.slint`'s add-filter form, which commits
/// a whole new row in one `performance_filter_add` call rather than editing the list
/// in place): every callback here reads the CURRENT list straight off `ui`, mutates a
/// plain `Vec` copy, and writes the whole thing back. There is no separate Rust-side
/// source of truth to keep in sync with it -- the same "the property IS the state"
/// shape `WorkerItem`'s list already uses (`gui::remote::worker_callbacks`), just
/// without that one's settings-file persistence, since a search filter is session-only
/// state.
///
/// Each handler only replaces `performance_filters` itself; it does NOT also invoke
/// `filters_changed` -- `filter_panel.slint`'s own `+ Add Filter`/'×' handlers already
/// call `root.changed()` right after `performance_filter_add`/`_remove` (Slint invokes
/// a Rust callback synchronously, so by the time that second call runs, the `Vec` this
/// module wrote back is already the property's current value), so the resulting
/// re-query already happens exactly once per user action.
pub(in crate::gui) fn setup_performance_filter_callbacks(ui: &MainWindow) {
    let ui_weak_add = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_performance_filter_add(move |row: PerformanceFilterRow| {
            if let Some(ui) = ui_weak_add.upgrade() {
                let mut rows: Vec<PerformanceFilterRow> = ui
                    .global::<LibraryModel>()
                    .get_performance_filters()
                    .iter()
                    .collect();
                rows.push(row);
                ui.global::<LibraryModel>()
                    .set_performance_filters(ModelRc::new(VecModel::from(rows)));
            }
        });

    let ui_weak_remove = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_performance_filter_remove(move |idx: i32| {
            if let Some(ui) = ui_weak_remove.upgrade() {
                let mut rows: Vec<PerformanceFilterRow> = ui
                    .global::<LibraryModel>()
                    .get_performance_filters()
                    .iter()
                    .collect();
                if let Ok(idx) = usize::try_from(idx)
                    && idx < rows.len()
                {
                    rows.remove(idx);
                }
                ui.global::<LibraryModel>()
                    .set_performance_filters(ModelRc::new(VecModel::from(rows)));
            }
        });

    let ui_weak_clear = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_performance_filters_clear(move || {
            if let Some(ui) = ui_weak_clear.upgrade() {
                ui.global::<LibraryModel>()
                    .set_performance_filters(ModelRc::new(VecModel::from(Vec::<
                        PerformanceFilterRow,
                    >::new(
                    ))));
            }
        });
}

/// Wires up diagram-selection, "open URL externally", and file-export callbacks.
/// Split out of `run_gui` purely to keep that function under clippy's
/// function-length lint.
///
/// `on_select_diagram`/`on_export_file` dispatch via `source` so a design
/// selected while browsing a remote library is looked up (and its attachments
/// exported) against THAT library, never the local database -- see
/// `gui::detail::load_diagram_detail_via_source`'s own doc comment on why that
/// dispatch matters (a remote entry id and a local row id are independent id spaces).
/// `on_open_diagram_url` needs no such dispatch: it only ever acts on the URL string
/// already loaded into `current_detail`, regardless of which source populated it.
pub(in crate::gui) fn setup_diagram_selection_and_export_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
) {
    let db_select = Arc::clone(db);
    let source_select = Arc::clone(source);
    let render_ctx_select = render_ctx.clone();
    let preview_state_select = Arc::clone(preview_state);
    let ui_weak_select = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_select_diagram(move |id: i32| {
            if let Some(ui) = ui_weak_select.upgrade() {
                load_diagram_detail_via_source(
                    &ui,
                    &db_select,
                    &source_select,
                    &render_ctx_select,
                    i64::from(id),
                    &preview_state_select,
                );
                // Live Render tab, Solid mode: `load_diagram_detail_via_source`'s LOCAL
                // branch already wrote fresh planes into `render_ctx` synchronously,
                // above -- redraw the solid immediately rather than leaving the
                // previous design on screen until the next camera drag. The REMOTE
                // branch is async and resubmits itself once its own planes land (see
                // `gui::library::detail::apply_design_record_to_ui`); this call still
                // fires for it too, but against whatever planes were already active
                // (typically the previously selected design), and is simply
                // superseded a moment later by that async resubmit -- harmless, since
                // `SolidPreviewState` coalesces to the LAST submitted request.
                if ui.get_render_view_tab() == 0
                    && ui.global::<ViewportModel>().get_live_view_mode() == 0
                {
                    let ctx = render_ctx_select
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    resubmit_live_solid(&ui, &ctx, &preview_state_select);
                }
            }
        });

    ui.global::<LibraryModel>()
        .on_open_diagram_url(move |url: SharedString| {
            let url_str = url.to_string();
            info!("Opening URL: {}", url_str);
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("xdg-open").arg(&url_str).spawn();
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", &url_str])
                .spawn();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&url_str).spawn();
        });

    let db_export = Arc::clone(db);
    let source_export = Arc::clone(source);
    let ui_weak_export = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_export_file(move |file_name: SharedString| {
            if let Some(ui) = ui_weak_export.upgrade() {
                let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
                if entry_id >= 0 {
                    export_diagram_file_via_source(
                        &ui,
                        &db_export,
                        &source_export,
                        i64::from(entry_id),
                        &file_name,
                    );
                } else {
                    show_toast(&ui, "No diagram selected for export.", "error");
                }
            }
        });
}

/// Wires `LibraryModel::regenerate_previews_for_filtered_set`/
/// `regenerate_tilt_curves_for_filtered_set` -- the owner's "regenerate library
/// previews/tilt curves for designs already in the catalogue" request,
/// for the panel's currently active search/filter set rather than a
/// single design or the whole catalogue.
///
/// Each callback resolves [`super::search::current_filtered_entry_ids`] and hands
/// the result straight to the existing per-batch confirm-step opener
/// (`gui::batch::preview::offer_batch_confirmation`/`gui::batch::tilt::
/// offer_batch_confirmation`) -- the same dialog, progress, and cancel machinery
/// every other trigger of these two batches already uses; this adds no second copy
/// of either batch's generation logic. Refuses (with a toast) under a
/// [`LibrarySource::Remote`] session, since [`Database::matching_entry_ids`] only
/// ever sees the local catalogue -- silently running it there would offer to
/// regenerate an unrelated, likely empty or wrong, local id set for whatever the
/// panel is showing from the remote worker.
///
/// Also a no-op (with a toast, not a silent nothing) when the filtered set is
/// empty, since [`offer_batch_confirmation`](crate::gui::batch::preview::offer_batch_confirmation)
/// itself only skips silently -- a cutter who just pressed the button deserves to
/// know why nothing opened.
pub(in crate::gui) fn setup_regenerate_filtered_set_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_previews = Arc::clone(db);
    let source_previews = Arc::clone(source);
    let ui_weak_previews = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_regenerate_previews_for_filtered_set(move || {
            let Some(ui) = ui_weak_previews.upgrade() else {
                return;
            };
            let Some(ids) = resolve_filtered_set_or_toast(&ui, &db_previews, &source_previews)
            else {
                return;
            };
            crate::gui::batch::preview::offer_batch_confirmation(&ui, &ids);
        });

    let db_tilt = Arc::clone(db);
    let source_tilt = Arc::clone(source);
    let ui_weak_tilt = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_regenerate_tilt_curves_for_filtered_set(move || {
            let Some(ui) = ui_weak_tilt.upgrade() else {
                return;
            };
            let Some(ids) = resolve_filtered_set_or_toast(&ui, &db_tilt, &source_tilt) else {
                return;
            };
            crate::gui::batch::tilt::offer_batch_confirmation(&ui, &ids);
        });
}

/// Shared body of both `setup_regenerate_filtered_set_callbacks` handlers: the
/// remote-source guard, the actual [`super::search::current_filtered_entry_ids`]
/// query, and the "nothing matched"/query-failure toasts -- pulled out so neither
/// handler duplicates this five-way branch.
fn resolve_filtered_set_or_toast(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) -> Option<Vec<i64>> {
    if source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_remote()
    {
        show_toast(
            ui,
            "Switch to the local library to regenerate designs for the filtered set.",
            "error",
        );
        return None;
    }
    match super::search::current_filtered_entry_ids(ui, db) {
        Ok(ids) if ids.is_empty() => {
            show_toast(ui, "No designs match the current filters.", "info");
            None
        }
        Ok(ids) => Some(ids),
        Err(e) => {
            show_toast(
                ui,
                &format!("Could not resolve the filtered set: {e}"),
                "error",
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_filter_range;

    #[test]
    fn clamp_filter_range_leaves_an_already_inside_filter_untouched() {
        // The common case: an import widened the catalogue's own bounds,
        // but the cutter's existing filter is still inside them and must not move.
        assert_eq!(clamp_filter_range((1.5, 2.0), (1.3, 2.9)), (1.5, 2.0));
    }

    #[test]
    fn clamp_filter_range_pulls_an_out_of_range_bound_back_in() {
        assert_eq!(clamp_filter_range((1.0, 3.5), (1.3, 2.9)), (1.3, 2.9));
    }

    #[test]
    fn clamp_filter_range_never_produces_an_inverted_min_max() {
        // Both `min` and `max` sit above the new (narrowed) bounds -- clamping each
        // independently would otherwise leave `min == max == bounds_max`, which is
        // still valid, not inverted; this exercises the one case that COULD invert
        // (`min` clamped up past an already-clamped `max`).
        assert_eq!(clamp_filter_range((5.0, 6.0), (1.0, 2.0)), (2.0, 2.0));
    }
}

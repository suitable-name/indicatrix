use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel,
    bridge::{
        library::source::{self as library_source, LibrarySource},
        render_thread::RenderContext,
    },
    gui::{library::search::refresh_diagram_list_via_source, show_toast, sync_range_bounds_to_ui},
    settings::WorkerSettings,
};
use indicatrix::geometry::{
    cuts::{FacetSpec, StandardGemCuts},
    plane::GpuFacetPlane,
};
use indicatrix_net::library::{DesignRecord, LibraryRequest, LibraryResponse};
use indicatrix_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// Dispatches to [`load_diagram_detail`] (the LOCAL database lookup, unchanged -- see
/// this crate's requirement that local behaviour stay byte-for-byte identical)
/// or [`load_diagram_detail_remote`], depending on which library is currently active.
/// Every call site that used to call `load_diagram_detail` directly now calls this
/// instead, so `entry_id` is always interpreted against the SAME library it was listed
/// from (a remote entry id and a local row id occupy independent id spaces -- see
/// `bridge::library_mirror`'s module doc comment on identity -- so this dispatch is
/// what keeps a remote-listed id from ever being looked up against the local database
/// by mistake).
pub fn load_diagram_detail_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => load_diagram_detail(ui, db_mutex, render_ctx, entry_id),
        LibrarySource::Remote(worker) => {
            load_diagram_detail_remote(ui, worker, render_ctx, entry_id);
        }
    }
}

pub fn load_diagram_detail(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    let db = match db_mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    match db.get_diagram_full(entry_id) {
        Ok(Some(full)) => {
            // See `indicatrix_vault::local::import_asc`: a locally-imported design's
            // `url` is a synthetic `local://<file name>` id, not a real web page --
            // gates "Open on Web"/"Copy Link" in `detail_header.slint`. Computed
            // before `full.url` is moved into the struct literal below.
            let is_local = full.url.starts_with("local://");
            let detail_data = DiagramDetailData {
                id: full.entry_id as i32,
                title: full.title.into(),
                url: full.url.into(),
                designer: full.designer_info.clone().unwrap_or_default().into(),
                shape: full.shape.clone().unwrap_or_default().into(),
                gear: full.index_gear.clone().unwrap_or_default().into(),
                facets: full.facets_count.unwrap_or_default().into(),
                lw_ratio: format_optional_proportion(full.lw_ratio.as_deref()).into(),
                ri: full.refractive_index.unwrap_or_default().into(),
                volume: format_optional_proportion(full.volume.as_deref()).into(),
                competition: full.competition_diagram.unwrap_or_default().into(),
                image_name: full.diagram_image_name.unwrap_or_default().into(),
                has_image: full.diagram_image_data.is_some(),
                is_local,
                hw_ratio: format_optional_proportion(full.hw_ratio.as_deref()).into(),
                cw_ratio: format_optional_proportion(full.cw_ratio.as_deref()).into(),
                pw_ratio: format_optional_proportion(full.pw_ratio.as_deref()).into(),
                symmetry_order: full.symmetry_order.unwrap_or_default().into(),
                mirror_symmetry: full.mirror_symmetry.unwrap_or(false),
            };
            ui.global::<LibraryModel>().set_current_detail(detail_data);

            let angle_items: Vec<AngleItem> = full
                .angle_settings
                .into_iter()
                .map(|a| AngleItem {
                    order_idx: a.order_index as i32,
                    facet: a.facet.into(),
                    angle: a.angle.into(),
                    index_val: a.index.into(),
                    notes: a.notes.into(),
                })
                .collect();
            ui.global::<LibraryModel>()
                .set_current_angles(ModelRc::new(VecModel::from(angle_items.clone())));

            let file_items: Vec<FileItem> = full
                .attached_files
                .into_iter()
                .map(|f| {
                    let size_kb = f.content.len() as f64 / 1024.0;
                    FileItem {
                        name: f.name.into(),
                        url: f.url.into(),
                        size_str: format!("{size_kb:.1} KB").into(),
                    }
                })
                .collect();
            ui.global::<LibraryModel>()
                .set_current_files(ModelRc::new(VecModel::from(file_items)));
            ui.global::<LibraryModel>()
                .set_selected_entry_id(entry_id as i32);

            // Shape picker (`detail_header.slint`'s pencil next to the "Shape:" chip,
            // local designs only -- see `library::setup_set_shape_callback`'s doc
            // comment). Recomputed on every open so it always reflects this design and
            // the library's present shape vocabulary, not whatever the last-opened
            // design left behind.
            let (shape_options, shape_index) =
                crate::gui::library::local::build_shape_picker_options(&db, full.shape.as_deref());
            ui.global::<LibraryModel>()
                .set_shape_picker_options(ModelRc::new(VecModel::from(shape_options)));
            ui.global::<LibraryModel>()
                .set_shape_picker_current_index(shape_index);

            // Closing the stale-cached-curve seam: the Tilt Performance
            // dialog's "re-render with the current material?" banner
            // (`gui::tilt_profile::cached_curve_material_is_stale`, wired on
            // `MainWindow::is_cached_curve_material_stale`) needs to know what
            // material this design's cached preview/tilt-curve artifacts were
            // actually generated under -- `Database::get_preview_images`'s
            // `material` field is that value (set once, at first preview
            // generation, via `Database::ensure_preview_material`, and reused
            // forever after -- see that method's own doc comment). An empty
            // string ("no cached material on file yet") is exactly
            // `cached_curve_material_is_stale`'s own documented "never stale"
            // case, so `unwrap_or_default` here needs no special-casing.
            let cached_curve_material = db
                .get_preview_images(entry_id)
                .ok()
                .and_then(|p| p.material)
                .unwrap_or_default();
            ui.global::<TiltModel>()
                .set_cached_curve_material(cached_curve_material.into());
            // Every remaining use of `db` is done -- drop the lock explicitly rather
            // than holding it through `apply_reconstructed_planes` below (which
            // doesn't need it), the same "don't hold a mutex longer than its last use"
            // discipline this crate's other long-lived-guard call sites already follow.
            drop(db);

            apply_reconstructed_planes(
                render_ctx,
                full.shape.as_deref(),
                full.index_gear.as_deref(),
                &angle_items,
                entry_id,
            );
        }
        Ok(None) => {
            ui.global::<LibraryModel>()
                .set_status_message("Diagram detail not found.".into());
        }
        Err(e) => {
            error!("Failed to fetch diagram full detail: {:?}", e);
            ui.global::<LibraryModel>()
                .set_status_message(format!("Error loading detail: {e}").into());
        }
    }
}

/// The remote counterpart of [`load_diagram_detail`]: fetches one design's full record
/// over the network (`LibraryRequest::FetchDesign`, off the UI thread -- see
/// `bridge::library_source`'s module doc comment) and applies it to the SAME
/// `DiagramDetailData`/`AngleItem`/`FileItem`/render-context fields the local path
/// populates, via [`apply_design_record_to_ui`], so the detail panel and 3D viewport
/// behave identically regardless of source.
///
/// [`FileItem::size_str`] is built from the attachment's advertised
/// [`indicatrix_net::library::AttachedFileMeta::size`] here (never its content -- the library
/// protocol deliberately never inlines attachment bytes into a `FetchDesign` reply, see
/// `indicatrix_net::library`'s module doc comment's "Attachments" section); the actual bytes
/// are fetched lazily, only if the user exports that specific file (see
/// `export_diagram_file_via_source`).
fn load_diagram_detail_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    let render_ctx = render_ctx.clone();
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| match result {
            Ok(LibraryResponse::Design(record)) => {
                apply_design_record_to_ui(ui, &render_ctx, &record);
            }
            Ok(LibraryResponse::NotFound) => {
                ui.global::<LibraryModel>()
                    .set_status_message("Diagram detail not found on the remote library.".into());
            }
            Ok(_) => {
                ui.global::<LibraryModel>()
                    .set_status_message("Unexpected reply fetching remote diagram detail.".into());
            }
            Err(e) => {
                error!("Remote FetchDesign failed: {e}");
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Error loading remote detail: {e}").into());
            }
        },
    );
}

fn apply_design_record_to_ui(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    record: &DesignRecord,
) {
    let is_local = record.url.starts_with("local://");
    let detail_data = DiagramDetailData {
        id: record.entry_id as i32,
        title: record.title.clone().into(),
        url: record.url.clone().into(),
        designer: record.designer_info.clone().unwrap_or_default().into(),
        shape: record.shape.clone().unwrap_or_default().into(),
        gear: record.index_gear.clone().unwrap_or_default().into(),
        facets: record.facets_count.clone().unwrap_or_default().into(),
        lw_ratio: format_optional_proportion(record.lw_ratio.as_deref()).into(),
        ri: record.refractive_index.clone().unwrap_or_default().into(),
        volume: format_optional_proportion(record.volume.as_deref()).into(),
        competition: record
            .competition_diagram
            .clone()
            .unwrap_or_default()
            .into(),
        image_name: record.diagram_image_name.clone().unwrap_or_default().into(),
        has_image: record.diagram_image_data.is_some(),
        is_local,
        // `indicatrix_net::library::DesignRecord` carries these as of `PROTOCOL_VERSION`
        // v6 (see that constant's doc comment for the full story) -- before that bump
        // this struct had no equivalent fields, so a remote design's H/W, C/W, P/W
        // chips and symmetry/mirror fields always rendered blank regardless of the
        // design's actual data, even though the local path (`load_diagram_detail`
        // above) already showed them. Formatted the same way as the local path, via
        // the same [`format_optional_proportion`], so remote and local render
        // identically. Metadata editing itself stays local-only (gated on
        // `detail.is_local` in `detail_header.slint`, same as rename/shape already
        // are) -- this only means these chips are no longer BLANK for a remote design,
        // not that they become editable there.
        hw_ratio: format_optional_proportion(record.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(record.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(record.pw_ratio.as_deref()).into(),
        symmetry_order: record.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: record.mirror_symmetry.unwrap_or(false),
    };
    ui.global::<LibraryModel>().set_current_detail(detail_data);
    // The remote counterpart of `load_diagram_detail`'s own cached-material read. This
    // used to be hardcoded empty, because `DesignRecord` carried no preview-material
    // field -- which `cached_curve_material_is_stale` documents as "never stale", so the
    // Tilt Performance dialog's "re-render with the current material?" offer silently
    // never appeared for a remote-browsed design. `DesignRecord::preview_material`
    // (added in `PROTOCOL_VERSION` v5) closes that: an absent value still means "nothing
    // generated yet, nothing to be stale against", which is the same thing empty meant
    // before, so the no-previews case is unchanged.
    ui.global::<TiltModel>()
        .set_cached_curve_material(record.preview_material.clone().unwrap_or_default().into());

    let angle_items: Vec<AngleItem> = record
        .angle_settings
        .iter()
        .map(|a| AngleItem {
            order_idx: a.order_index as i32,
            facet: a.facet.clone().into(),
            angle: a.angle.clone().into(),
            index_val: a.index.clone().into(),
            notes: a.notes.clone().into(),
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(angle_items.clone())));

    let file_items: Vec<FileItem> = record
        .attachments
        .iter()
        .map(|f| {
            let size_kb = f.size as f64 / 1024.0;
            FileItem {
                name: f.name.clone().into(),
                url: f.url.clone().into(),
                size_str: format!("{size_kb:.1} KB").into(),
            }
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(file_items)));
    ui.global::<LibraryModel>()
        .set_selected_entry_id(record.entry_id as i32);

    apply_reconstructed_planes(
        render_ctx,
        record.shape.as_deref(),
        record.index_gear.as_deref(),
        &angle_items,
        record.entry_id,
    );
}

/// Formats a stored proportion string to 3 decimal places for the detail header's
/// metric chips -- DISPLAY ONLY. What actually lives in the database is never touched
/// by this (see `indicatrix_vault::db::sqlite::Database::update_diagram_metadata`,
/// which always round-trips a proportion's full, unrounded text -- the user's own
/// instruction is "store everything, discard nothing, don't recalculate").
///
/// Parses first: only a value that parses cleanly as a finite `f64` gets reformatted.
/// Anything that doesn't parse -- a scraped legacy string in some other format, or a
/// future value this app didn't write -- passes through completely untouched, so it
/// can never be coerced into a misleading "0.000". Checked against the real
/// ~3,187-design catalogue (`facet_diagrams.sqlite`): every non-null `lw_ratio`/
/// `hw_ratio`/`cw_ratio`/`pw_ratio`/`volume` value there is already a plain
/// REAL-affinity number with no exceptions, but the model type is `Option<String>` and
/// nothing guarantees that stays true forever, so this stays defensive rather than
/// assuming it.
fn format_proportion(raw: &str) -> String {
    match raw.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => format!("{n:.3}"),
        _ => raw.to_string(),
    }
}

/// [`format_proportion`] over an `Option<&str>`, collapsing `None` to `""` -- the same
/// "empty string hides the chip" convention every other optional field on
/// `DiagramDetailData` already uses (see `detail_header.slint`'s `if root.detail.xxx
/// != "":` chips).
fn format_optional_proportion(raw: Option<&str>) -> String {
    raw.map(format_proportion).unwrap_or_default()
}

/// Trims `text` and turns a blank result into `None` -- the metadata editor's
/// convention for "the user cleared this field", matching
/// `Database::rename_diagram_entry`'s own trim. `None` is what
/// [`MetadataUpdate`] stores as a field's new value in that case, not an empty-string
/// sentinel -- consistent with every other optional column this app writes.
fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Wires up the detail header's metadata editor modal (see
/// `metadata_editor_dialog.slint`) -> `save_metadata`.
///
/// Local designs only, same guard and same reasoning as
/// `library::setup_rename_callback`/`setup_set_shape_callback`: a remote-sourced
/// `selected_entry_id` names a row in a REMOTE server's catalogue, not this process's
/// local database, and the library-sync protocol is read-only besides.
/// `detail_header.slint` additionally only offers the editor for `root.detail.is_local`,
/// so this is a backstop, not the only thing standing between a remote id and the
/// local database.
///
/// Title is saved through [`Database::rename_diagram_entry`] -- it lives in
/// `diagram_entries`, not `diagram_details`, and already has its own narrow, correct
/// setter with none of the subset trap `update_diagram_metadata` exists for. Every
/// other field goes through `update_diagram_metadata` in one call, which -- unlike
/// `save_diagram_detail` -- touches only the twelve columns it's given and leaves
/// everything else (including every field a bare `FullDiagramRecord` used to be unable
/// to see at all) untouched; see that method's own doc comment for the full story.
///
/// On success, reloads the design from the database via [`load_diagram_detail`] rather
/// than hand-patching `current_detail`'s dozen fields in place -- the reload picks up
/// SQLite's own numeric normalisation (e.g. `"1.760"` reads back `"1.76"`) and this
/// module's own 3-decimal display formatting for exactly the same reason opening the
/// design fresh would, with one implementation instead of two that could drift apart.
/// Also re-reconstructs the 3D viewport's planes, which matters when the edit changed
/// `shape` (the one field here that feeds `reconstruct_planes`' emerald/baguette/rect
/// special case).
pub fn setup_save_metadata_callback(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let db_meta = Arc::clone(db_mutex);
    let source_meta = Arc::clone(source);
    let render_ctx_meta = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_save_metadata(
        move |title: SharedString,
              designer: SharedString,
              shape: SharedString,
              refractive_index: SharedString,
              index_gear: SharedString,
              facets_count: SharedString,
              symmetry_order: SharedString,
              mirror_symmetry: bool,
              lw_ratio: SharedString,
              hw_ratio: SharedString,
              cw_ratio: SharedString,
              pw_ratio: SharedString,
              volume: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_meta
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to edit a design's metadata.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let entry_id = i64::from(entry_id);

            let result = {
                let db = db_meta
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.rename_diagram_entry(entry_id, &title).and_then(|()| {
                    let update = MetadataUpdate {
                        designer_info: non_empty(&designer),
                        shape: non_empty(&shape),
                        refractive_index: non_empty(&refractive_index),
                        index_gear: non_empty(&index_gear),
                        facets_count: non_empty(&facets_count),
                        symmetry_order: non_empty(&symmetry_order),
                        mirror_symmetry: Some(mirror_symmetry),
                        lw_ratio: non_empty(&lw_ratio),
                        hw_ratio: non_empty(&hw_ratio),
                        cw_ratio: non_empty(&cw_ratio),
                        pw_ratio: non_empty(&pw_ratio),
                        volume: non_empty(&volume),
                    };
                    db.update_diagram_metadata(entry_id, &update)
                })
            };
            match result {
                Ok(()) => {
                    show_toast(&ui, "Metadata updated.", "success");
                    load_diagram_detail(&ui, &db_meta, &render_ctx_meta, entry_id);
                    // Refreshes the visible list/filters the same way
                    // `library::refresh_after_library_change` does for rename/shape --
                    // that helper is private to `gui::library`, so this repeats its
                    // essential two steps (range bounds, then the list query) rather
                    // than reaching into another module's private function.
                    sync_range_bounds_to_ui(&ui, &db_meta);
                    let search = ui.global::<LibraryModel>().get_search_text();
                    let shape_idx = ui.global::<LibraryModel>().get_selected_shape_index() as usize;
                    let shape_filter = ui
                        .global::<LibraryModel>()
                        .get_shape_options()
                        .row_data(shape_idx)
                        .unwrap_or_default();
                    let gear_idx = ui.global::<LibraryModel>().get_selected_gear_index() as usize;
                    let gear_filter = ui
                        .global::<LibraryModel>()
                        .get_gear_options()
                        .row_data(gear_idx)
                        .unwrap_or_default();
                    refresh_diagram_list_via_source(
                        &ui,
                        &db_meta,
                        &source_meta,
                        &search,
                        &shape_filter,
                        &gear_filter,
                    );
                }
                Err(e) => show_toast(&ui, &format!("Metadata update failed: {e}"), "error"),
            }
        },
    );
}

/// Shared by [`load_diagram_detail`] (local) and [`apply_design_record_to_ui`] (remote):
/// rebuilds the 3D viewport's facet planes from a design's shape/gear/angle-settings --
/// exactly one reconstruction implementation for both sources, so they can never drift
/// apart.
fn apply_reconstructed_planes(
    render_ctx: &Arc<Mutex<RenderContext>>,
    shape: Option<&str>,
    index_gear: Option<&str>,
    angle_items: &[AngleItem],
    entry_id: i64,
) {
    // `indicatrix` must not depend on Slint, so convert the Slint-generated `AngleItem`
    // rows into plain `FacetSpec`s at this boundary.
    let facet_specs: Vec<FacetSpec> = angle_items
        .iter()
        .map(|a| FacetSpec {
            facet: a.facet.to_string(),
            angle: a.angle.to_string(),
            index: a.index_val.to_string(),
            notes: a.notes.to_string(),
        })
        .collect();
    let planes = reconstruct_planes(shape, index_gear, &facet_specs);

    info!(
        "Reconstructed {} 3D facet planes for diagram #{}",
        planes.len(),
        entry_id
    );

    // Library records carry no reference angle, so the diagram view's index wheel
    // uses the gear tooth count alone (96 when the record has none).
    let gear_teeth: u32 = index_gear
        .and_then(|g| g.trim().parse().ok())
        .filter(|&g| g > 0)
        .unwrap_or(96);

    let mut ctx = render_ctx.lock().unwrap();
    ctx.active_planes = std::sync::Arc::new(planes);
    ctx.design_gear = Some((gear_teeth, 0.0));
    ctx.dirty = true;
}

/// Rebuilds a design's 3D facet planes from its shape/gear/angle-settings. Pulled out
/// of [`apply_reconstructed_planes`] (which still owns the actual `RenderContext`
/// write) so `gui::library`'s metadata-fill step for a freshly imported `.asc` --
/// `measure_solid` needs the SAME planes the viewport would show, not a second,
/// possibly-drifting re-parse -- can call exactly this and nothing more. `shape` is
/// `None` at import time (it's the very thing not parsed yet), which is fine: the
/// emerald-cut special case below just doesn't fire, same as it wouldn't for any other
/// design whose `shape` isn't one of those three substrings.
pub fn reconstruct_planes(
    shape: Option<&str>,
    index_gear: Option<&str>,
    facet_specs: &[FacetSpec],
) -> Vec<GpuFacetPlane> {
    let shape_str = shape.unwrap_or_default().to_lowercase();
    let gear_num: u32 = index_gear.unwrap_or_default().parse().unwrap_or(96);

    if shape_str.contains("emerald") || shape_str.contains("baguette") || shape_str.contains("rect")
    {
        StandardGemCuts::emerald_cut()
    } else if !facet_specs.is_empty() {
        StandardGemCuts::from_database_angles(facet_specs, gear_num)
    } else {
        StandardGemCuts::standard_round_brilliant()
    }
}

pub fn export_diagram_file(
    db_mutex: &Arc<Mutex<Database>>,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) -> Result<String, String> {
    let full_result = {
        let db = match db_mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        db.get_diagram_full(entry_id)
    };

    if let Ok(Some(full)) = full_result
        && let Some(f) = full.attached_files.iter().find(|af| af.name == file_name)
    {
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(dest_path, &f.content).is_ok() {
            return Ok(format!("Saved '{}' to {}", f.name, dest_path.display()));
        }
    }
    Err(format!("Failed to export file '{file_name}'."))
}

/// Dispatches "export this attachment" to the LOCAL synchronous path ([`export_diagram_file`],
/// unchanged -- reports through `ui.status_message`/a toast exactly as the pre-Phase-2
/// caller did) or a background remote fetch, depending on which library is active.
///
/// Unlike [`export_diagram_file`] this reports its own result directly onto `ui` rather
/// than returning one -- the remote path is unavoidably asynchronous (a `FetchAttachment`
/// round trip), so both branches report the same way for one consistent call
/// convention at the single call site (`gui::diagram_list::setup_diagram_selection_and_export_callbacks`).
///
/// Prompts for a destination with a native Save As dialog, seeded with the
/// attachment's own `file_name` under the existing `./exports/` default directory (if
/// it exists yet), BEFORE dispatching to either branch below -- one prompt shared by
/// both, so cancelling never writes a file either way, matching this task's cancel
/// rule. Blocking `rfd::FileDialog`, called directly on the Slint UI/event-loop thread
/// -- see `apps/indicatrix-cut/Cargo.toml`'s `rfd` dependency comment for why that's the
/// supported way to call it here.
pub fn export_diagram_file_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    entry_id: i64,
    file_name: &str,
) {
    let mut dialog = rfd::FileDialog::new().set_file_name(file_name);
    let default_dir = std::path::Path::new("exports");
    if default_dir.is_dir() {
        dialog = dialog.set_directory(default_dir);
    }
    if let Some(ext) = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
    {
        dialog = dialog.add_filter(ext, &[ext]);
    }
    let Some(dest_path) = dialog.save_file() else {
        let msg = "Export cancelled.".to_string();
        ui.global::<LibraryModel>()
            .set_status_message(msg.clone().into());
        crate::gui::show_toast(ui, &msg, "info");
        return;
    };

    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => {
            match export_diagram_file(db_mutex, entry_id, file_name, &dest_path) {
                Ok(msg) => {
                    ui.global::<LibraryModel>()
                        .set_status_message(msg.clone().into());
                    crate::gui::show_toast(ui, &msg, "success");
                }
                Err(err) => {
                    ui.global::<LibraryModel>()
                        .set_status_message(err.clone().into());
                    crate::gui::show_toast(ui, &err, "error");
                }
            }
        }
        LibrarySource::Remote(worker) => {
            export_diagram_file_remote(ui, worker, entry_id, file_name, &dest_path);
        }
    }
}

/// The remote counterpart of [`export_diagram_file`]. `FetchAttachment` identifies an
/// attachment by id, not name (see `indicatrix_net::library`'s module doc comment), and
/// `FileItem` -- what the attachments tab actually displays -- carries only a name, so
/// this re-fetches the design's metadata first to resolve `file_name` to an attachment
/// id, then fetches that attachment's bytes: two round trips for an occasional,
/// user-initiated export, not a cost paid by browsing itself. `dest_path` is already
/// resolved (the Save As dialog already ran, in [`export_diagram_file_via_source`]) --
/// this never prompts, it only writes.
fn export_diagram_file_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    entry_id: i64,
    file_name: &str,
    dest_path: &std::path::Path,
) {
    let file_name = file_name.to_string();
    let dest_path = dest_path.to_path_buf();
    let worker_for_attachment = worker.clone();
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| {
            let attachment_id = match result {
                Ok(LibraryResponse::Design(record)) => record
                    .attachments
                    .iter()
                    .find(|f| f.name == file_name)
                    .map(|f| f.id),
                _ => None,
            };
            let Some(attachment_id) = attachment_id else {
                report_export_failure(ui, &file_name);
                return;
            };
            let file_name_for_failure = file_name.clone();
            let dest_path_for_attachment = dest_path.clone();
            library_source::spawn_library_request(
                ui.as_weak(),
                worker_for_attachment,
                LibraryRequest::FetchAttachment { attachment_id },
                move |ui, result| match result {
                    Ok(LibraryResponse::Attachment { name, content }) => {
                        if let Some(parent) = dest_path_for_attachment.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if std::fs::write(&dest_path_for_attachment, &content).is_ok() {
                            let msg =
                                format!("Saved '{name}' to {}", dest_path_for_attachment.display());
                            ui.global::<LibraryModel>()
                                .set_status_message(msg.clone().into());
                            crate::gui::show_toast(ui, &msg, "success");
                        } else {
                            report_export_failure(ui, &file_name_for_failure);
                        }
                    }
                    _ => report_export_failure(ui, &file_name_for_failure),
                },
            );
        },
    );
}

fn report_export_failure(ui: &MainWindow, file_name: &str) {
    let msg = format!("Failed to export file '{file_name}'.");
    ui.global::<LibraryModel>()
        .set_status_message(msg.clone().into());
    crate::gui::show_toast(ui, &msg, "error");
}

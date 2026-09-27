//! Builds the detail pane and 3D viewport from a design in the LOCAL database.

use super::{
    planes::{ReconstructedPlanesInput, apply_reconstructed_planes},
    shared::{catalogue_material_guess, format_optional_proportion, sides_from_angle_sequence},
};
use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel,
    bridge::render_thread::RenderContext,
};
use indicatrix::geometry::plane::GpuFacetPlane;
use indicatrix_vault::{db::sqlite::Database, model::entry::FullDiagramRecord};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex};
use tracing::error;

/// A one-line status-message note rather than a persistent badge: a proper
/// "Derived from: <title>" chip on `detail_header.slint` would need a new field on
/// `DiagramDetailData` (`ui/types.slint`) or a new `LibraryModel` property
/// (`ui/models/library.slint`), plus re-exporting it through `app.slint`'s own
/// `export {}` list before Rust can reach it -- this status-message note is the
/// cheaper stand-in until that lands. `None`/an error both mean "no recorded
/// source" here (a design that was never re-imported, or whose recorded source row
/// was since deleted), matching `get_derived_from_title`'s own doc comment.
fn note_derived_from_if_any(ui: &MainWindow, db: &Database, entry_id: i64) {
    if let Some((_, title)) = db.get_derived_from_title(entry_id).ok().flatten() {
        ui.global::<LibraryModel>()
            .set_status_message(format!("Derived from: {title}").into());
    }
}

/// Pushes the shape-picker options/current-index (`detail_header.slint`'s pencil
/// next to the "Shape:" chip, local designs only -- see
/// `library::setup_set_shape_callback`'s doc comment; recomputed on every open so
/// it always reflects this design and the library's present shape vocabulary, not
/// whatever the last-opened design left behind), the cached-curve-material
/// readout, and the derived-from note. Split out of [`load_diagram_detail`]
/// purely to keep that function under clippy's function-length lint.
///
/// Closes the stale-cached-curve seam: the Tilt Performance dialog's
/// "re-render with the current material?" banner
/// (`gui::tilt_profile::cached_curve_material_is_stale`, wired on
/// `MainWindow::is_cached_curve_material_stale`) needs to know what material this
/// design's cached preview/tilt-curve artifacts were actually generated under --
/// `Database::get_preview_images`'s `material` field is that value (set once, at
/// first preview generation, via `Database::ensure_preview_material`, and reused
/// forever after -- see that method's own doc comment). An empty string ("no
/// cached material on file yet") is exactly `cached_curve_material_is_stale`'s own
/// documented "never stale" case, so `unwrap_or_default` here needs no
/// special-casing.
///
/// Returns the `Option<String>` (not `unwrap_or_default`'d away
/// immediately) so the caller can ALSO feed it to `apply_catalogue_material` as
/// the persisted-preview-material fast path, ahead of that function's own
/// refractive-index nearest-match guess -- see its doc comment.
fn push_shape_picker_and_cached_material(
    ui: &MainWindow,
    db: &Database,
    entry_id: i64,
    shape: Option<&str>,
) -> Option<String> {
    let (shape_options, shape_index) =
        crate::gui::library::local::build_shape_picker_options(db, shape);
    ui.global::<LibraryModel>()
        .set_shape_picker_options(ModelRc::new(VecModel::from(shape_options)));
    ui.global::<LibraryModel>()
        .set_shape_picker_current_index(shape_index);

    let cached_curve_material = db
        .get_preview_images(entry_id)
        .ok()
        .and_then(|p| p.material);
    ui.global::<TiltModel>()
        .set_cached_curve_material(cached_curve_material.clone().unwrap_or_default().into());
    note_derived_from_if_any(ui, db, entry_id);
    cached_curve_material
}

/// [`crate::gui::editor::resolve_catalogue_planes`], `None`-ified
/// on any failure -- a resolution failure (no attached `.asc` and no angle-settings
/// row at all) and "the design has no valid anchor" (`Ok(None)`) both mean the same
/// thing to this route: fall back to `super::planes::reconstruct_planes`.
///
/// `gui::editor` (`mod editor;`, `gui/mod.rs`) is declared unconditionally -- this
/// crate's `library` module already depends on it elsewhere (e.g.
/// `editor::material_lookup`, imported by `super::shared`).
/// [`crate::gui::editor::resolve_catalogue_planes`]'s own signature still avoids
/// naming `indicatrix_cut_core::Design` so this module never needs one either.
fn resolve_catalogue_planes_for_entry(
    full: &FullDiagramRecord,
) -> Option<(Vec<GpuFacetPlane>, u32, f32)> {
    crate::gui::editor::resolve_catalogue_planes(full)
        .ok()
        .flatten()
}

/// Loads one design's full record from the LOCAL database and applies it to the
/// detail pane, angle table, attachments list and 3D viewport.
pub(super) fn load_diagram_detail(
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
            // Cloned before the struct literal below moves it: the viewport's material
            // comes from this same value -- see `apply_reconstructed_planes`.
            let ri_text = full.refractive_index.clone();
            // Resolve the SAME real planes/gear/reference-angle
            // the editor's own "Load Selected" would show for this row, BEFORE
            // `full.title`/`full.attached_files`/`full.angle_settings` are moved out
            // below -- `resolve_catalogue_planes_for_entry` only borrows `full`. A
            // resolution failure (no attached `.asc` and no angle-settings row at all)
            // just means `apply_reconstructed_planes` falls back to its own
            // placeholder, not an error worth surfacing here.
            let catalogue_planes = resolve_catalogue_planes_for_entry(&full);
            let detail_data = DiagramDetailData {
                id: full.entry_id as i32,
                title: full.title.into(),
                url: full.url.into(),
                designer: full.designer_info.clone().unwrap_or_default().into(),
                shape: full.shape.clone().unwrap_or_default().into(),
                gear: full.index_gear.clone().unwrap_or_default().into(),
                facets: full.facets_count.unwrap_or_default().into(),
                lw_ratio: format_optional_proportion(full.lw_ratio.as_deref()).into(),
                material_guess: catalogue_material_guess(full.refractive_index.as_deref()).into(),
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

            // The sides come from the schedule's own row ORDER (see
            // `sides_from_angle_sequence`'s own doc comment), computed over the
            // already order_idx-ordered `full.angle_settings` (`entries.rs`'s own
            // `ORDER BY order_idx ASC`) before it's consumed below.
            let sides =
                sides_from_angle_sequence(full.angle_settings.iter().map(|a| a.angle.as_str()));
            let angle_items: Vec<AngleItem> = full
                .angle_settings
                .into_iter()
                .zip(sides)
                .map(|(a, side)| AngleItem {
                    order_idx: a.order_index as i32,
                    side,
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

            // Shape picker, cached-curve-material readout and the derived-from note --
            // split into a helper purely to keep this function under clippy's
            // function-length lint; see that helper's own doc comment.
            let cached_curve_material =
                push_shape_picker_and_cached_material(ui, &db, entry_id, full.shape.as_deref());
            // Every remaining use of `db` is done -- drop the lock explicitly rather
            // than holding it through `apply_reconstructed_planes` below (which
            // doesn't need it), the same "don't hold a mutex longer than its last use"
            // discipline this crate's other long-lived-guard call sites already follow.
            drop(db);

            apply_reconstructed_planes(
                ui,
                render_ctx,
                entry_id,
                ReconstructedPlanesInput {
                    shape: full.shape.as_deref(),
                    index_gear: full.index_gear.as_deref(),
                    angle_items: &angle_items,
                    refractive_index: ri_text.as_deref(),
                    preview_material: cached_curve_material.as_deref(),
                    real_design: catalogue_planes,
                },
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

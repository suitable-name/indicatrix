//! Builds the detail pane and 3D viewport from a design in the LOCAL database.
//!
//! The detail pane, angle table and attachment list (names and sizes only, read
//! without any attachment bytes) are pushed synchronously. The
//! viewport's planes are not: resolving them parses the design file and runs a full
//! solve (0.01 ms to 5.9 s), so [`load_diagram_detail`] hands that to a worker
//! thread and applies the result from the event loop -- unless a later click
//! superseded it first.

use super::{
    planes::{DesignFileMaterial, ReconstructedPlanesInput, apply_reconstructed_planes},
    shared::{
        catalogue_material_guess, format_optional_proportion, sides_from_rows, standard_codes,
    },
};
use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{editor::CataloguePlanes, library::read_access::with_read_db},
};
use indicatrix_vault::{db::sqlite::Database, model::entry::FullDiagramMeta};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tracing::error;

/// Numbers the local detail loads so a slow planes resolution that finishes after a
/// newer click is recognised as stale and dropped.
struct LoadSequence(AtomicU64);

impl LoadSequence {
    const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Starts a load: every earlier one is stale from now on.
    fn begin(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    /// Whether `sequence` is the newest load.
    fn is_current(&self, sequence: u64) -> bool {
        self.0.load(Ordering::Acquire) == sequence
    }
}

/// The newest local detail load.
static DETAIL_LOADS: LoadSequence = LoadSequence::new();

/// Whether a resolved load should still draw its planes: it must be the newest
/// load, and the detail pane must still be showing the entry it resolved (a click on
/// a remote row moves the selection without touching [`DETAIL_LOADS`]).
fn planes_still_wanted(is_newest_load: bool, selected_entry_id: i32, entry_id: i64) -> bool {
    is_newest_load && i64::from(selected_entry_id) == entry_id
}

/// Everything the detail pane reads from the database, fetched in one pass over a
/// read-only connection so the UI thread neither waits on the shared writer lock nor
/// reads any attachment bytes (perf review UI-5): `meta` carries each attachment's name
/// and size only.
struct DetailFetch {
    meta: FullDiagramMeta,
    shape_options: Vec<SharedString>,
    shape_index: i32,
    /// `Database::get_preview_material`: reads only the `preview_material` column,
    /// never the two cached preview PNGs.
    cached_curve_material: Option<String>,
    /// Title of the design this one was derived from, when one is recorded.
    derived_from_title: Option<String>,
}

/// Reads `entry_id`'s detail-pane data from `db`; `Ok(None)` when the row is gone.
fn fetch_detail(db: &Database, entry_id: i64) -> anyhow::Result<Option<DetailFetch>> {
    let Some(meta) = db.get_diagram_full_meta(entry_id)? else {
        return Ok(None);
    };
    let (shape_options, shape_index) =
        crate::gui::library::local::build_shape_picker_options(db, meta.shape.as_deref());
    let cached_curve_material = db.get_preview_material(entry_id).ok().flatten();
    // `None`/an error both mean "no recorded source" (a design that was never
    // re-imported, or whose recorded source row was since deleted), matching
    // `get_derived_from_title`'s own doc comment.
    let derived_from_title = db
        .get_derived_from_title(entry_id)
        .ok()
        .flatten()
        .map(|(_, title)| title);
    Ok(Some(DetailFetch {
        meta,
        shape_options,
        shape_index,
        cached_curve_material,
        derived_from_title,
    }))
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
/// `Database::get_preview_material` is that value (set once, at first preview
/// generation, via `Database::ensure_preview_material`, and reused forever after --
/// see that method's own doc comment; a design whose file names a material has the
/// name written over instead, `Database::set_preview_material`). An empty string ("no
/// cached material on file yet") is exactly `cached_curve_material_is_stale`'s own
/// documented "never stale" case, so `unwrap_or_default` here needs no
/// special-casing.
///
/// The derived-from note is a one-line status message rather than a persistent badge: a
/// proper chip on `detail_header.slint` would need a new `DiagramDetailData` field or
/// `LibraryModel` property re-exported through `app.slint`'s `export {}` list.
///
/// Returns the `Option<String>` (not `unwrap_or_default`'d away
/// immediately) so the caller can ALSO feed it to `apply_catalogue_material` as
/// the persisted-preview-material fast path, ahead of that function's own
/// refractive-index nearest-match guess -- see its doc comment.
fn push_shape_picker_and_cached_material(
    ui: &MainWindow,
    shape_options: Vec<SharedString>,
    shape_index: i32,
    cached_curve_material: Option<String>,
    derived_from_title: Option<String>,
) -> Option<String> {
    ui.global::<LibraryModel>()
        .set_shape_picker_options(ModelRc::new(VecModel::from(shape_options)));
    ui.global::<LibraryModel>()
        .set_shape_picker_current_index(shape_index);
    ui.global::<TiltModel>()
        .set_cached_curve_material(cached_curve_material.clone().unwrap_or_default().into());
    if let Some(title) = derived_from_title {
        ui.global::<LibraryModel>()
            .set_status_message(format!("Derived from: {title}").into());
    }
    cached_curve_material
}

/// The detail pane's header fields for `full`.
fn detail_data(full: &FullDiagramMeta) -> DiagramDetailData {
    DiagramDetailData {
        id: i32::try_from(full.entry_id).unwrap_or(i32::MAX),
        title: full.title.clone().into(),
        url: full.url.clone().into(),
        designer: full.designer_info.clone().unwrap_or_default().into(),
        shape: full.shape.clone().unwrap_or_default().into(),
        gear: full.index_gear.clone().unwrap_or_default().into(),
        facets: full.facets_count.clone().unwrap_or_default().into(),
        lw_ratio: format_optional_proportion(full.lw_ratio.as_deref()).into(),
        material_guess: catalogue_material_guess(full.refractive_index.as_deref()).into(),
        ri: full.refractive_index.clone().unwrap_or_default().into(),
        volume: format_optional_proportion(full.volume.as_deref()).into(),
        competition: full.competition_diagram.clone().unwrap_or_default().into(),
        image_name: full.diagram_image_name.clone().unwrap_or_default().into(),
        has_image: full.diagram_image_data.is_some(),
        // See `indicatrix_vault::local::import_asc`: a locally-imported design's
        // `url` is a synthetic `local://<file name>` id, not a real web page --
        // gates "Open on Web"/"Copy Link" in `detail_header.slint`.
        is_local: full.url.starts_with("local://"),
        hw_ratio: format_optional_proportion(full.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(full.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(full.pw_ratio.as_deref()).into(),
        symmetry_order: full.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: full.mirror_symmetry.unwrap_or(false),
    }
}

/// The angle table's rows for `full`. The sides come from each row's own
/// facet/angle/index label (see `sides_from_rows`'s own doc comment), computed over
/// the already order_idx-ordered `full.angle_settings` (`entries.rs`'s own
/// `ORDER BY order_idx ASC`).
fn angle_items(full: &FullDiagramMeta) -> Vec<AngleItem> {
    let sides = sides_from_rows(
        full.angle_settings
            .iter()
            .map(|a| (a.facet.as_str(), a.angle.as_str(), a.index.as_str())),
    );
    let labels: Vec<(&str, &str)> = full
        .angle_settings
        .iter()
        .map(|a| (a.facet.as_str(), a.index.as_str()))
        .collect();
    let codes = standard_codes(&labels, &sides);
    full.angle_settings
        .iter()
        .zip(sides)
        .zip(codes)
        .map(|((a, side), code)| AngleItem {
            order_idx: a.order_index.cast_signed(),
            side,
            code: code.into(),
            facet: a.facet.clone().into(),
            angle: a.angle.trim_start_matches('-').to_string().into(),
            index_val: a.index.clone().into(),
            notes: a.notes.clone().into(),
            second_line: a.tool_line.clone().unwrap_or_default().into(),
        })
        .collect()
}

/// The attachment list's rows for `full`.
fn file_items(full: &FullDiagramMeta) -> Vec<FileItem> {
    full.attached_files
        .iter()
        .map(|f| {
            // `f.size` is SQL `length(content)`, the byte count the old
            // `content.len()` gave.
            let size_kb = f.size as f64 / 1024.0;
            FileItem {
                name: f.name.clone().into(),
                url: f.url.clone().into(),
                size_str: format!("{size_kb:.1} KB").into(),
            }
        })
        .collect()
}

/// Loads one design's full record from the LOCAL database and applies it to the
/// detail pane, angle table and attachments list; the 3D viewport follows once its
/// planes are resolved off the UI thread (see [`spawn_planes_resolution`]).
pub(super) fn load_diagram_detail(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    // Own read-only connection, metadata-only record: no attachment bytes are read and
    // the shared writer lock is not taken (it is only the fallback for `:memory:`).
    let fetched = with_read_db(db_mutex, |db| fetch_detail(db, entry_id));

    match fetched {
        Ok(Some(fetched)) => {
            let DetailFetch {
                meta,
                shape_options,
                shape_index,
                cached_curve_material,
                derived_from_title,
            } = fetched;
            ui.global::<LibraryModel>()
                .set_current_detail(detail_data(&meta));
            ui.global::<LibraryModel>()
                .set_current_angles(ModelRc::new(VecModel::from(angle_items(&meta))));
            ui.global::<LibraryModel>()
                .set_current_files(ModelRc::new(VecModel::from(file_items(&meta))));
            ui.global::<LibraryModel>()
                .set_selected_entry_id(i32::try_from(entry_id).unwrap_or(i32::MAX));

            // Shape picker, cached-curve-material readout and the derived-from note --
            // split into a helper purely to keep this function under clippy's
            // function-length lint; see that helper's own doc comment.
            let cached_curve_material = push_shape_picker_and_cached_material(
                ui,
                shape_options,
                shape_index,
                cached_curve_material,
                derived_from_title,
            );

            spawn_planes_resolution(ui, db_mutex, render_ctx, meta, cached_curve_material);
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

/// What the event loop needs to draw a resolved record's planes. Owned and `Send`:
/// the worker drops the record itself (attachment blobs included) and forwards only
/// this.
struct ResolvedPlanes {
    entry_id: i64,
    shape: Option<String>,
    index_gear: Option<String>,
    refractive_index: Option<String>,
    preview_material: Option<String>,
    /// The planes, gear teeth and reference angle, with the material and girdle
    /// diameter the design file records -- or why resolving them panicked. A design file
    /// that merely fails to read or solve is not an error here:
    /// [`crate::gui::editor::resolve_catalogue_planes`] falls back to the angle table for
    /// it, the SAME resolution (design file first) the preview and tilt batches use, so
    /// this view's stone and a record's cached thumbnails and tilt curves never describe
    /// different geometry. It parses and solves the design file: it runs on the worker
    /// thread, never the UI thread. Paired with the ascending indices of the planes the
    /// design frosts ([`crate::gui::editor::resolve_catalogue_planes_with_frosting`]), so the
    /// viewport shows the finish the editor, the thumbnail and the Compare window show.
    planes: Result<(CataloguePlanes, Vec<u32>), String>,
}

/// Resolves `full`'s planes on a worker thread and draws them from the event loop,
/// unless a newer detail load superseded this one first. `preview_material` is the
/// record's persisted preview material, as [`push_shape_picker_and_cached_material`]
/// read it.
///
/// The viewport keeps showing the previous design until the planes land; a click
/// that supersedes this one before the worker starts makes it return without
/// resolving anything.
fn spawn_planes_resolution(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    meta: FullDiagramMeta,
    preview_material: Option<String>,
) {
    let sequence = DETAIL_LOADS.begin();
    let ui_weak = ui.as_weak();
    let render_ctx = Arc::clone(render_ctx);
    let db_mutex = Arc::clone(db_mutex);
    let spawned = std::thread::Builder::new()
        .name("catalogue-planes".to_string())
        .spawn(move || {
            if !DETAIL_LOADS.is_current(sequence) {
                return;
            }
            // The design file is an attachment, so the planes resolution needs the
            // full record; reading its blobs is done here, off the UI thread and on a
            // read-only connection of its own.
            let entry_id = meta.entry_id;
            let full = with_read_db(&db_mutex, |db| db.get_diagram_full(entry_id));
            let planes = match full {
                Ok(Some(full)) => {
                    crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(
                        || crate::gui::editor::resolve_catalogue_planes_with_frosting(&full),
                    ))
                }
                Ok(None) => Err("the design no longer exists".to_string()),
                Err(e) => Err(format!("the design could not be read: {e}")),
            };
            let resolved = ResolvedPlanes {
                entry_id,
                shape: meta.shape,
                index_gear: meta.index_gear,
                refractive_index: meta.refractive_index,
                preview_material,
                planes,
            };
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                apply_resolved_planes(&ui, &render_ctx, sequence, resolved);
            });
        });
    if let Err(e) = spawned {
        error!("Could not start the catalogue planes worker: {e}");
    }
}

/// Draws `resolved`'s planes, on the UI thread, if they are still wanted (see
/// [`planes_still_wanted`]).
fn apply_resolved_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    sequence: u64,
    resolved: ResolvedPlanes,
) {
    let selected = ui.global::<LibraryModel>().get_selected_entry_id();
    if !planes_still_wanted(
        DETAIL_LOADS.is_current(sequence),
        selected,
        resolved.entry_id,
    ) {
        return;
    }
    let (catalogue, frosted) = match resolved.planes {
        Ok(resolved_planes) => resolved_planes,
        Err(message) => {
            error!(
                "Resolving the planes of diagram #{} failed: {message}",
                resolved.entry_id
            );
            ui.global::<LibraryModel>().set_status_message(
                format!("Could not build the 3D view of this design: {message}").into(),
            );
            return;
        }
    };
    // The design file's material and size, for the viewport to draw the stone as the
    // editor and the exports do (`planes::apply_catalogue_material`).
    let design_material = DesignFileMaterial::from(&catalogue);
    let (tools, tool_placements) = (catalogue.render_tools, catalogue.render_placements);
    let real_design = (
        catalogue.planes,
        catalogue.gear_teeth,
        catalogue.gear_reference_angle,
    );
    apply_reconstructed_planes(
        ui,
        render_ctx,
        resolved.entry_id,
        ReconstructedPlanesInput {
            shape: resolved.shape.as_deref(),
            index_gear: resolved.index_gear.as_deref(),
            // Unread: `real_design` is always `Some` here, and the angle table is
            // only consulted when it is `None`.
            angle_items: &[],
            refractive_index: resolved.refractive_index.as_deref(),
            preview_material: resolved.preview_material.as_deref(),
            real_design: Some(real_design),
            design_material,
            // The stone with its concave tools, as the Compare window draws it.
            tools,
            tool_placements,
            concave_rows: Vec::new(),
            frosted,
        },
    );
    request_live_solid_redraw(ui);
}

/// Redraws the Live Render tab's solid raster when it is the one showing, now that
/// the planes behind it have changed.
///
/// The selection callback's own redraw
/// (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`) runs
/// before the planes land, so it draws the previous design. This goes through
/// `ViewportModel.live_view_mode_changed` because its handler is the one place,
/// besides that callback, that holds the solid preview state and redraws from
/// `RenderContext::active_planes`; invoking it with the unchanged mode only repeats
/// that redraw.
fn request_live_solid_redraw(ui: &MainWindow) {
    let viewport = ui.global::<ViewportModel>();
    let mode = viewport.get_live_view_mode();
    if ui.get_render_view_tab() == 0 && mode == 0 {
        viewport.invoke_live_view_mode_changed(mode);
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadSequence, planes_still_wanted};

    #[test]
    fn only_the_newest_load_is_current() {
        let loads = LoadSequence::new();
        let first = loads.begin();
        assert!(loads.is_current(first));
        let second = loads.begin();
        assert_ne!(first, second);
        assert!(
            !loads.is_current(first),
            "a later click supersedes the first"
        );
        assert!(loads.is_current(second));
    }

    #[test]
    fn a_load_that_never_began_is_not_current() {
        let loads = LoadSequence::new();
        let _ = loads.begin();
        assert!(!loads.is_current(0));
    }

    #[test]
    fn planes_are_drawn_only_for_the_newest_load_of_the_selected_entry() {
        assert!(planes_still_wanted(true, 7, 7));
        assert!(
            !planes_still_wanted(false, 7, 7),
            "a superseded load never draws"
        );
        assert!(
            !planes_still_wanted(true, 8, 7),
            "the selection moved to another row"
        );
    }
}

use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel, ViewportModel,
    bridge::{
        library::source::{self as library_source, LibrarySource},
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::{
        editor::material_lookup::{MATERIAL_MATCH_TOLERANCE, material_for_refractive_index},
        library::{
            diagram_list::sync_range_bounds_to_ui_preserving_filters,
            search::refresh_diagram_list_via_source,
        },
        render::camera_lighting::resubmit_live_solid,
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
    settings::WorkerSettings,
};
use indicatrix::{
    geometry::{
        cuts::{FacetSpec, StandardGemCuts},
        plane::GpuFacetPlane,
    },
    optics::materials::GemMaterial,
};
// This crate's own `gui::editor::material_lookup` already depends on
// `indicatrix_cut_core` unconditionally at the top of this file
// (`material_for_refractive_index` below) -- this adds no new dependency, only a
// second, narrower use of the same crate already pulled in that way.
use indicatrix_cut_core::material::{BuiltinMaterials, MaterialLookup};
use indicatrix_net::library::{DesignRecord, LibraryRequest, LibraryResponse};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{entry::FullDiagramRecord, metadata_update::MetadataUpdate},
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// The angle magnitude (degrees) at or above which a catalogue schedule row is
/// treated as describing the girdle itself rather than a crown or pavilion facet --
/// a fallback heuristic (see [`sides_from_angle_sequence`]'s own doc
/// comment for why a sign-based classifier cannot work on this data).
const GIRDLE_ANGLE_THRESHOLD_DEG: f64 = 89.5;

/// Parses a catalogue angle-settings row's `angle` text the same way
/// `indicatrix_vault::local::parse_angle_deg` does -- that function is private to the
/// vault crate, so this reimplements its two-line body: strip a trailing `\u{b0}`
/// degree sign, then a plain `f64` parse. `None` for text that still doesn't parse.
fn parse_catalogue_angle_deg(angle: &str) -> Option<f64> {
    angle.trim().trim_end_matches('\u{b0}').trim().parse().ok()
}

/// Derives every row's [`AngleItem::side`] (`-1` pavilion, `1` crown, `0` neither) from
/// the ORDER a schedule lists `angles` in, not from the sign of the angle text.
///
/// Measured on the real catalogue: 50,809 of 50,817 stored `angle_settings.angle`
/// values end in `\u{b0}` (so a bare `str::parse` fails on virtually every row unless
/// that's stripped first, reporting `0`/neither side for everything), and every stored pavilion angle is
/// written UNSIGNED -- so even after stripping the degree sign, a sign check
/// (`deg < 0.0` / `deg > 0.0`) still reports every real row as `0`. The Pavilion/Crown
/// filter pills in `cutting_table.slint` were therefore dead for every stored design.
///
/// This crate's own schedule rows (`gui::editor::state::cutting_schedule_rows`) take
/// the side from the SOLVER's per-tier block classification instead, which needs a
/// solved [`indicatrix_cut_core::Design`] and is `pub(super)` to `gui::editor` -- not
/// reachable from this module (which deliberately never names that feature-gated
/// `Design` type in its own signatures, see `resolve_catalogue_planes_for_entry`'s own
/// doc comment) without pulling in the whole editor pipeline just to classify a
/// display-only column. This is the documented fallback in its place: a catalogue
/// schedule reliably lists crown facets first, then the girdle row(s) (~90 degrees),
/// then pavilion facets -- the same order `.asc`/`GemCad` schedules and this crate's own
/// tier table use. So the FIRST row whose angle is at least
/// [`GIRDLE_ANGLE_THRESHOLD_DEG`] marks the crown/pavilion boundary: every row at or
/// past that magnitude (there may be more than one girdle-adjacent facet) reports `0`
/// (the girdle itself, neither side); every row before the boundary is crown (`1`);
/// every row strictly after it is pavilion (`-1`). A row whose angle text doesn't
/// parse at all also reports `0`.
fn sides_from_angle_sequence<'a>(angles: impl Iterator<Item = &'a str>) -> Vec<i32> {
    let degrees: Vec<Option<f64>> = angles.map(parse_catalogue_angle_deg).collect();
    let girdle_idx = degrees
        .iter()
        .position(|deg| deg.is_some_and(|d| d >= GIRDLE_ANGLE_THRESHOLD_DEG));
    degrees
        .iter()
        .enumerate()
        .map(|(i, deg)| match deg {
            None => 0,
            Some(d) if *d >= GIRDLE_ANGLE_THRESHOLD_DEG => 0,
            Some(_) => {
                if girdle_idx.is_some_and(|g| i > g) {
                    -1
                } else {
                    1
                }
            }
        })
        .collect()
}

/// Counts `angles` under `mode` (0 = All, 1 = Pavilion, 2 = Crown) by
/// [`AngleItem::side`] -- exactly `cutting_table.slint`'s own `row_shown` predicate
/// (side < 0 pavilion, side > 0 crown), pulled out so [`setup_filtered_row_count_callback`]
/// stays a thin Slint-callback wrapper.
#[must_use]
fn count_angles_for_mode(angles: &ModelRc<AngleItem>, mode: i32) -> i32 {
    let count = angles
        .iter()
        .filter(|a| match mode {
            1 => a.side < 0,
            2 => a.side > 0,
            _ => true,
        })
        .count();
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Registers `LibraryModel::filtered_row_count` -- the per-mode row count
/// `cutting_table.slint`'s "All Steps (N)"/Pavilion/Crown pills need, which cannot
/// be computed in Slint itself (see the comment above that file's row-count
/// pills for why: Slint has no general loop or array `.filter`/`.reduce`, and a
/// `pure function` there cannot even recurse to fake one). Reads
/// `LibraryModel.current_angles` fresh on every call rather than
/// taking it as a second argument -- see [`count_angles_for_mode`]'s doc comment,
/// and `library.slint`'s own doc comment on `filtered_row_count` for why this
/// still reacts correctly to a `current_angles` change.
pub fn setup_filtered_row_count_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_filtered_row_count(move |mode: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return 0;
            };
            let angles = ui.global::<LibraryModel>().get_current_angles();
            count_angles_for_mode(&angles, mode)
        });
}

/// Dispatches to [`load_diagram_detail`] (the LOCAL database lookup, unchanged -- see
/// this crate's requirement that local behaviour stay byte-for-byte identical)
/// or [`load_diagram_detail_remote`], depending on which library is currently active.
/// Every call site calls this rather than `load_diagram_detail` directly,
/// so `entry_id` is always interpreted against the SAME library it was listed
/// from (a remote entry id and a local row id occupy independent id spaces -- see
/// `bridge::library_mirror`'s module doc comment on identity -- so this dispatch is
/// what keeps a remote-listed id from ever being looked up against the local database
/// by mistake).
///
/// `preview_state` is only used by the REMOTE branch -- see [`load_diagram_detail_remote`]'s
/// own doc comment for why: the local branch writes `render_ctx.active_planes`
/// synchronously, before this function returns, so its own Live-Render-tab Solid-mode
/// resubmit happens at the call site instead
/// (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`).
pub fn load_diagram_detail_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => load_diagram_detail(ui, db_mutex, render_ctx, entry_id),
        LibrarySource::Remote(worker) => {
            load_diagram_detail_remote(ui, worker, render_ctx, entry_id, preview_state);
        }
    }
}

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

/// Clears the detail pane's display fields and drops the current row selection.
///
/// Shared by [`crate::gui::library::local::organize`]'s delete handler (the original
/// "the selected design is gone" case) and
/// [`crate::gui::library::search::apply_diagram_list_to_ui`] (a search/
/// filter refresh that drops the previously selected row off the new result list must
/// not leave `LibraryModel.selected_entry_id` pointing at a design no longer even
/// visible in the list it came from). Deliberately leaves `RenderContext`/the 3D
/// viewport untouched: unlike an actual delete, the design itself hasn't changed here,
/// only whether the LIST currently shows it, so there is nothing wrong with the
/// viewport going on tracing it.
pub fn clear_current_detail_display(ui: &MainWindow) {
    ui.global::<LibraryModel>()
        .set_current_detail(DiagramDetailData {
            id: -1,
            title: SharedString::default(),
            url: SharedString::default(),
            designer: SharedString::default(),
            shape: SharedString::default(),
            gear: SharedString::default(),
            facets: SharedString::default(),
            lw_ratio: SharedString::default(),
            ri: SharedString::default(),
            material_guess: SharedString::default(),
            volume: SharedString::default(),
            competition: SharedString::default(),
            image_name: SharedString::default(),
            has_image: false,
            is_local: false,
            hw_ratio: SharedString::default(),
            cw_ratio: SharedString::default(),
            pw_ratio: SharedString::default(),
            symmetry_order: SharedString::default(),
            mirror_symmetry: false,
        });
    ui.global::<LibraryModel>().set_selected_entry_id(-1);
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(Vec::new())));
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(Vec::new())));
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
///
/// `preview_state` is forwarded to [`apply_design_record_to_ui`], not used here
/// directly -- it's only needed once the design's record (and its planes) actually
/// arrive, inside the completion closure below.
fn load_diagram_detail_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
) {
    let render_ctx = render_ctx.clone();
    let preview_state = Arc::clone(preview_state);
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| match result {
            Ok(LibraryResponse::Design(record)) => {
                apply_design_record_to_ui(ui, &render_ctx, &record, &preview_state);
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

/// [`DiagramDetailData::material_guess`]'s own computation -- a catalogue row
/// records only a refractive index string (`ri_text`, `None`/unparseable
/// reads as "nothing to guess"), so this is the only honest way to name the
/// stone for the card ("Inferred material shown as a guess, never as a fact").
/// Formatted
/// identically to the editor's own material-guess badge ("Sapphire? (from RI
/// 1.76)") so the two never disagree about wording; `""` when nothing built
/// in is close enough within [`MATERIAL_MATCH_TOLERANCE`] -- hides the chip
/// entirely rather than showing an empty/misleading one.
fn catalogue_material_guess(ri_text: Option<&str>) -> String {
    let Some(n_d) = ri_text.and_then(|s| s.trim().parse::<f64>().ok()) else {
        return String::new();
    };
    material_for_refractive_index(n_d).map_or_else(String::new, |(name, _)| {
        format!("{name}? (from RI {n_d:.2})")
    })
}

fn apply_design_record_to_ui(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    record: &DesignRecord,
    preview_state: &Arc<SolidPreviewState>,
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
        material_guess: catalogue_material_guess(record.refractive_index.as_deref()).into(),
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
        // v6 (see that constant's doc comment for the full story), formatted the same
        // way as the local path via the same [`format_optional_proportion`], so remote
        // and local render identically. Metadata editing itself stays local-only (gated
        // on `detail.is_local` in `detail_header.slint`, same as rename/shape already
        // are) -- these chips are populated for a remote design, but not editable
        // there.
        hw_ratio: format_optional_proportion(record.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(record.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(record.pw_ratio.as_deref()).into(),
        symmetry_order: record.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: record.mirror_symmetry.unwrap_or(false),
    };
    ui.global::<LibraryModel>().set_current_detail(detail_data);
    // The remote counterpart of `load_diagram_detail`'s own cached-material read, via
    // `DesignRecord::preview_material` (added in `PROTOCOL_VERSION` v5). An absent
    // value means "nothing generated yet, nothing to be stale against" -- the same
    // thing `cached_curve_material_is_stale` documents as "never stale" -- so the Tilt
    // Performance dialog's "re-render with the current material?" offer only appears
    // once a remote design actually has a cached material to compare against.
    ui.global::<TiltModel>()
        .set_cached_curve_material(record.preview_material.clone().unwrap_or_default().into());
    // Provenance is a local-catalogue-only concept (same as tags) -- a remote-browsed
    // design never gets the "Derived from" status note `load_diagram_detail` (the
    // local route) sets.

    // See `apply_reconstructed_planes`'s call site in `load_diagram_detail`
    // for the local path's identical treatment -- `sides_from_angle_sequence` is
    // shared by both so the two can never disagree about a design's own sides.
    let sides = sides_from_angle_sequence(record.angle_settings.iter().map(|a| a.angle.as_str()));
    let angle_items: Vec<AngleItem> = record
        .angle_settings
        .iter()
        .zip(sides)
        .map(|(a, side)| AngleItem {
            order_idx: a.order_index as i32,
            side,
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
        ui,
        render_ctx,
        record.entry_id,
        ReconstructedPlanesInput {
            shape: record.shape.as_deref(),
            index_gear: record.index_gear.as_deref(),
            angle_items: &angle_items,
            refractive_index: record.refractive_index.as_deref(),
            // The remote counterpart of `load_diagram_detail`'s own
            // `cached_curve_material` -- `record.preview_material` is exactly the
            // value this function already read into `TiltModel.cached_curve_material`
            // a few lines up.
            preview_material: record.preview_material.as_deref(),
            // A remote `DesignRecord` never carries attachment
            // BYTES (see this function's own doc comment) -- there is no real `.asc`
            // text here to resolve a `Design` from, so this route stays on the
            // placeholder reconstruction. Fixing that needs a wire-protocol change
            // (`indicatrix-net`/`indicatrix-worker`), out of scope for this crate
            // alone.
            real_design: None,
        },
    );

    // The remote counterpart of the local selection callback's own resubmit
    // (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`) --
    // needed here specifically because THIS path is async: the local branch's planes
    // land synchronously before its caller returns, but this closure is where a
    // remote design's planes actually arrive. Only fires when the Live Render tab is
    // both showing (`render_view_tab == 0`) and in Solid mode (`live_view_mode == 0`)
    // -- the same condition the local path's own resubmit checks, so a remote
    // selection redraws the solid immediately instead of leaving the previous
    // design's stale raster on screen until the next camera drag.
    if ui.get_render_view_tab() == 0 && ui.global::<ViewportModel>().get_live_view_mode() == 0 {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resubmit_live_solid(ui, &ctx, preview_state);
    }
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
/// everything else untouched; see that method's own doc comment for the full story.
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
                    // than reaching into another module's private function. The
                    // `_preserving_filters` variant: a metadata correction
                    // is not a request to clear whatever range filters the cutter had
                    // already narrowed the catalogue to.
                    sync_range_bounds_to_ui_preserving_filters(&ui, &db_meta);
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

/// [`crate::gui::editor::resolve_catalogue_planes`], `None`-ified
/// on any failure -- a resolution failure (no attached `.asc` and no angle-settings
/// row at all) and "the design has no valid anchor" (`Ok(None)`) both mean the same
/// thing to this route: fall back to [`reconstruct_planes`].
///
/// `gui::editor` (`mod editor;`, `gui/mod.rs`) is declared unconditionally -- this
/// crate's `library` module already depends on it elsewhere (e.g.
/// `editor::material_lookup`, imported at the top of this file).
/// [`crate::gui::editor::resolve_catalogue_planes`]'s own signature still avoids
/// naming `indicatrix_cut_core::Design` so this module never needs one either.
fn resolve_catalogue_planes_for_entry(
    full: &FullDiagramRecord,
) -> Option<(Vec<GpuFacetPlane>, u32, f32)> {
    crate::gui::editor::resolve_catalogue_planes(full)
        .ok()
        .flatten()
}

/// Builds the catalogue detail view's 3D planes/gear/reference
/// angle from `real_design` -- the SAME real `Design`'s planes/gear-teeth/reference-
/// angle [`crate::gui::editor::resolve_catalogue_planes`] resolves for the editor's
/// own "Load Selected" (via `Design::planes()`, the exact pipeline
/// `gui::editor::state::design_to_gpu_planes` uses) -- instead of this route's own
/// placeholder-only [`reconstruct_planes`] guess, whenever that resolution succeeded.
///
/// Falls back to [`reconstruct_planes`] (gear reference angle `0.0`, matching this
/// route's long-standing "library records carry no reference angle" note) whenever
/// `real_design` is `None` -- [`crate::gui::editor::resolve_catalogue_planes`] itself
/// returned `None`/`Err`: no attached `.asc`/angle-settings row to resolve a `Design`
/// from at all, a resolved design with no valid `ScaleReference` anchor for
/// `Design::planes()` to place its tiers against, or (`apply_design_record_to_ui`'s
/// own call site) the remote route, which has no attached `.asc` bytes to parse yet
/// and so never even calls it.
fn planes_gear_and_reference_angle(
    real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
    shape: Option<&str>,
    index_gear: Option<&str>,
    angle_items: &[AngleItem],
) -> (Vec<GpuFacetPlane>, u32, f32) {
    if let Some(resolved) = real_design {
        return resolved;
    }

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
    // Library records carry no reference angle, so the diagram view's index wheel
    // uses the gear tooth count alone (96 when the record has none).
    let gear_teeth: u32 = index_gear
        .and_then(|g| g.trim().parse().ok())
        .filter(|&g| g > 0)
        .unwrap_or(96);
    (planes, gear_teeth, 0.0)
}

/// Shared by [`load_diagram_detail`] (local) and [`apply_design_record_to_ui`] (remote):
/// rebuilds the 3D viewport's facet planes from a design's shape/gear/angle-settings --
/// exactly one reconstruction implementation for both sources, so they can never drift
/// apart.
///
/// This claims the shared plane slot exactly
/// like `gui::editor::view::refresh_viewport` does for an in-editor edit -- if the
/// Tilt Performance dialog is open over a PREVIOUS design's curves when a cutter
/// picks a different catalogue entry, those curves and summary badges would
/// otherwise go on describing geometry that no longer exists. `ui` is only needed
/// for that re-sweep request (`TiltModel.dialog_open`/
/// `invoke_request_tilt_profile_axes`), not for anything else this function does.
///
/// [`apply_reconstructed_planes`]'s bundled input -- see that function's own doc
/// comment on `real_design` for what each field means.
struct ReconstructedPlanesInput<'a> {
    shape: Option<&'a str>,
    index_gear: Option<&'a str>,
    angle_items: &'a [AngleItem],
    refractive_index: Option<&'a str>,
    /// This design's own persisted preview material (`Database::
    /// get_preview_images(entry_id).material` locally, `DesignRecord::preview_material`
    /// remotely) -- already read by both callers into `TiltModel.cached_curve_material`
    /// -- tried by [`apply_catalogue_material`] ahead of the refractive-index guess.
    preview_material: Option<&'a str>,
    real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
}

/// `real_design` is the SAME `Design`'s already-converted
/// planes/gear-teeth/reference-angle the editor's own "Load Selected" would show for
/// `entry_id` (`resolve_catalogue_planes_for_entry`, `None` on the remote route --
/// see that function's own call site), preferred over this module's placeholder-only
/// `reconstruct_planes` guess whenever it resolved to something. See
/// [`planes_gear_and_reference_angle`] for exactly when each path is taken.
///
/// Bundled into [`ReconstructedPlanesInput`] purely to keep this function's argument
/// count under clippy's `too_many_arguments` limit -- the `real_design` field is what
/// pushed it over.
fn apply_reconstructed_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    input: ReconstructedPlanesInput<'_>,
) {
    let ReconstructedPlanesInput {
        shape,
        index_gear,
        angle_items,
        refractive_index,
        preview_material,
        real_design,
    } = input;
    let (planes, gear_teeth, gear_reference_angle) =
        planes_gear_and_reference_angle(real_design, shape, index_gear, angle_items);

    info!(
        "Reconstructed {} 3D facet planes for diagram #{}",
        planes.len(),
        entry_id
    );

    let mut ctx = RenderContext::lock(render_ctx);
    // A catalogue click must not steal the viewport out from
    // under a design being edited. `claim_active_planes` refuses when the editor
    // owns the planes; say so rather than appearing to work and changing nothing.
    let claimed = ctx.claim_active_planes(
        std::sync::Arc::new(planes),
        Some((gear_teeth, gear_reference_angle)),
        PlanesOwner::Catalogue { entry_id },
    );
    let resolved_material = if claimed {
        let resolved = apply_catalogue_material(&mut ctx, refractive_index, preview_material);
        ctx.dirty = true;
        resolved
    } else {
        None
    };
    drop(ctx);
    // The dropdown's own displayed selection, not just the material being traced: the
    // two are separate pieces of state, and writing only `RenderContext` left the
    // toolbar reading (say) "Quartz" while the stone on screen was already the newly
    // selected design's sapphire. Same treatment `editor::view::refresh_design_settings`
    // gives its own material sync, via the same helper. `find_option_index` returning
    // `None` leaves the selection alone rather than guessing -- it cannot happen for a
    // built-in (`refresh_material_options` lists every one of them), so that case only
    // covers an options model not pushed yet.
    if let Some(name) = resolved_material {
        let options = ui.global::<ViewportModel>().get_material_options();
        if let Some(index) = crate::gui::startup_settings::find_option_index(&options, &name) {
            ui.global::<ViewportModel>()
                .set_selected_material_index(index);
        }
    }
    if !claimed {
        show_toast(
            ui,
            "The 3D view is showing the design you are editing -- it was left alone. \
             Switch to the Library tab's own view to preview this row.",
            "info",
        );
        return;
    }

    // See this function's own doc comment --
    // `AxesCacheKey` (`gui::tilt::tilt_profile`) already hashes the planes, so this
    // is a no-op resweep whenever nothing about them actually moved.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
}

/// Resolves `name` against the render context's own loaded custom materials first,
/// falling back to the built-in table -- the same custom-over-built-in precedence
/// `gui::editor::material_lookup::EditorMaterialLookup::lookup` already applies (that
/// type isn't reachable from this module -- `pub(super)` to `gui::editor` -- so this is
/// a narrower, local equivalent), kept in step so a persisted preview-material name
/// never resolves differently depending on which of the two lookups happens to see it.
/// Used only by [`apply_catalogue_material`]'s preview-material fast path.
fn lookup_named_material(ctx: &RenderContext, name: &str) -> Option<GemMaterial> {
    ctx.custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .cloned()
        .or_else(|| BuiltinMaterials.lookup(name))
}

/// Points the viewport's material at whichever built-in this catalogue row's own
/// refractive index names -- the material half of a catalogue selection. Without it the
/// preview would keep whatever the last editor refresh, the last Render Material pick or
/// the PREVIOUS catalogue row left behind, so clicking through the library would change
/// the shape on screen and never the stone.
///
/// Both halves are written, not just the name: `material_override` BEATS `material_name`
/// in `render_thread::context::resolve_material_with_override`, so leaving a stale
/// override in place would make the name below purely decorative.
///
/// A row whose refractive index is absent, unparsable, or within
/// [`MATERIAL_MATCH_TOLERANCE`] of no built-in at all sets `material_unresolved` instead:
/// the viewport then says why it will not trace rather than borrowing some other
/// design's optics.
///
/// `preview_material` -- this design's own persisted preview/tilt-curve
/// material, when one is on file -- is tried FIRST, via [`lookup_named_material`],
/// ahead of the refractive-index nearest-match below. It is a recorded FACT (the exact
/// material the cached thumbnail and tilt curves were actually rendered under), not a
/// ±[`MATERIAL_MATCH_TOLERANCE`] nearest-preset GUESS that can tie-break two different
/// ways in two different code paths (`gui::editor::material_lookup::
/// material_for_refractive_index` here vs. `indicatrix_vault::model::material_match`'s
/// own nearest-with-random-tie rule, which decided what the persisted preview material
/// actually IS) -- preferring the fact closes the "unresolved in the viewport while the
/// thumbnail was rendered as a real preset" seam that let those two rules disagree.
///
/// Returns the material's name on success, so the caller can point the Render Material
/// dropdown at it once the `RenderContext` lock is released; `None` on either refusal,
/// where there is no name to show.
fn apply_catalogue_material(
    ctx: &mut RenderContext,
    refractive_index: Option<&str>,
    preview_material: Option<&str>,
) -> Option<String> {
    if let Some(name) = preview_material.map(str::trim).filter(|s| !s.is_empty())
        && let Some(gem) = lookup_named_material(ctx, name)
    {
        ctx.material_unresolved = None;
        ctx.material_name = name.to_string();
        ctx.material_override = Some(gem);
        return Some(name.to_string());
    }

    let n_d = refractive_index.and_then(|text| text.trim().parse::<f64>().ok());
    let Some(n_d) = n_d.filter(|v| v.is_finite() && *v > 1.0) else {
        ctx.material_unresolved = Some(
            "This design records no usable refractive index, so there is no honest \
             material to render it in. Pick one in the Render Material dropdown above."
                .to_string(),
        );
        return None;
    };
    let Some((name, gem)) = material_for_refractive_index(n_d) else {
        ctx.material_unresolved = Some(format!(
            "This design's refractive index ({n_d:.4}) matches no built-in preset within \
             {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in the Render Material dropdown \
             above -- rendering it as something else would give you the optics of a \
             different stone."
        ));
        return None;
    };
    ctx.material_unresolved = None;
    ctx.material_name.clone_from(&name);
    ctx.material_override = Some(gem);
    Some(name)
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
/// which reports through `ui.status_message`/a toast) or a background remote fetch,
/// depending on which library is active.
///
/// Unlike [`export_diagram_file`] this reports its own result directly onto `ui` rather
/// than returning one -- the remote path is unavoidably asynchronous (a `FetchAttachment`
/// round trip), so both branches report the same way for one consistent call
/// convention at the single call site (`gui::diagram_list::setup_diagram_selection_and_export_callbacks`).
///
/// Prompts for a destination with a native Save As dialog, seeded with the
/// attachment's own `file_name` under the existing `./exports/` default directory (if
/// it exists yet), BEFORE dispatching to either branch below -- one prompt shared by
/// both, so cancelling never writes a file either way.
///
/// The picker runs off the UI thread via `gui::pickers::pick`; this function reports
/// through `ui`/a toast rather than a return value, so its one caller,
/// `gui::diagram_list::setup_diagram_selection_and_export_callbacks`, needs no special
/// handling for that.
pub fn export_diagram_file_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    entry_id: i64,
    file_name: &str,
) {
    let default_dir = std::path::Path::new("exports");
    let extension_filter = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_string);
    let db_mutex = Arc::clone(db_mutex);
    let source = Arc::clone(source);
    let entry_id_owned = entry_id;
    let file_name_owned = file_name.to_string();
    crate::gui::pickers::pick(
        ui,
        crate::gui::pickers::PickerRequest {
            kind: crate::gui::pickers::PickerKind::SaveFile,
            title: None,
            filters: extension_filter
                .map(|ext| vec![crate::gui::pickers::PickerFilter::single(ext)])
                .unwrap_or_default(),
            default_file_name: Some(file_name_owned.clone()),
            starting_dir: default_dir.is_dir().then(|| default_dir.to_path_buf()),
        },
        move |ui, dest_path| {
            let Some(dest_path) = dest_path else {
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
                    match export_diagram_file(
                        &db_mutex,
                        entry_id_owned,
                        &file_name_owned,
                        &dest_path,
                    ) {
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
                    export_diagram_file_remote(
                        ui,
                        worker,
                        entry_id_owned,
                        &file_name_owned,
                        &dest_path,
                    );
                }
            }
        },
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_catalogue_angle_deg / sides_from_angle_sequence ---

    /// The real catalogue's own shape: 50,809 of 50,817 stored `angle_settings.angle`
    /// values end in `\u{b0}`, which a bare `str::parse` chokes on.
    #[test]
    fn parse_catalogue_angle_deg_strips_the_degree_sign() {
        assert_eq!(parse_catalogue_angle_deg("42.70\u{b0}"), Some(42.70));
    }

    #[test]
    fn parse_catalogue_angle_deg_also_accepts_plain_text_with_no_degree_sign() {
        assert_eq!(parse_catalogue_angle_deg("42.70"), Some(42.70));
    }

    #[test]
    fn parse_catalogue_angle_deg_is_none_for_unparsable_text() {
        assert_eq!(parse_catalogue_angle_deg("not a number"), None);
    }

    /// A girdle-ordered schedule -- crown facets, then the girdle row(s) at ~90
    /// degrees, then pavilion facets -- exactly the real catalogue's own row order and
    /// unsigned, degree-sign-suffixed angle text.
    #[test]
    fn sides_from_angle_sequence_classifies_a_girdle_ordered_schedule() {
        let angles = [
            "34.50\u{b0}",
            "42.70\u{b0}",
            "90.00\u{b0}",
            "41.00\u{b0}",
            "43.10\u{b0}",
        ];
        let sides = sides_from_angle_sequence(angles.iter().copied());
        assert_eq!(sides, vec![1, 1, 0, -1, -1]);
    }

    /// More than one row can sit at/above the girdle threshold (e.g. two girdle
    /// facets listed back to back) -- every one of them reports `0`, not just the
    /// first.
    #[test]
    fn sides_from_angle_sequence_reports_zero_for_every_girdle_row_not_only_the_first() {
        let angles = ["40.00\u{b0}", "89.80\u{b0}", "90.00\u{b0}", "41.00\u{b0}"];
        let sides = sides_from_angle_sequence(angles.iter().copied());
        assert_eq!(sides, vec![1, 0, 0, -1]);
    }

    #[test]
    fn sides_from_angle_sequence_reports_zero_for_unparsable_text() {
        let angles = ["not a number", "41.00\u{b0}"];
        let sides = sides_from_angle_sequence(angles.iter().copied());
        assert_eq!(sides[0], 0);
    }

    /// A schedule with no girdle-magnitude row at all (nothing >= the threshold) has
    /// no boundary to cross, so every row stays crown -- never guesses a pavilion
    /// split with no marker to place it at.
    #[test]
    fn sides_from_angle_sequence_treats_every_row_as_crown_with_no_girdle_marker() {
        let angles = ["34.50\u{b0}", "42.70\u{b0}"];
        let sides = sides_from_angle_sequence(angles.iter().copied());
        assert_eq!(sides, vec![1, 1]);
    }

    /// With no `real_design` resolved (the remote route, or a
    /// local resolution failure), the catalogue view must still fall back to the
    /// existing placeholder [`reconstruct_planes`] path -- gear read from the text
    /// column, reference angle `0.0` (library records carry none).
    #[test]
    fn falls_back_to_the_placeholder_reconstruction_with_no_real_design() {
        let angle_items = [AngleItem {
            order_idx: 0,
            side: -1,
            facet: "P1".into(),
            angle: "-41.0".into(),
            index_val: "0, 24, 48, 72".into(),
            notes: "".into(),
        }];
        let (planes, gear_teeth, reference_angle) =
            planes_gear_and_reference_angle(None, Some("Round"), Some("96"), &angle_items);
        assert_ne!(planes.len(), 0);
        assert_eq!(gear_teeth, 96);
        assert_eq!(reference_angle, 0.0);
    }

    /// With a resolved `real_design` tuple, the
    /// catalogue view must use exactly that (already-converted) planes/gear/reference-
    /// angle, not the placeholder guess or a hardcoded `0.0` -- regardless of what the
    /// (deliberately wrong, "999") shape/gear text arguments would otherwise have
    /// produced.
    #[test]
    fn prefers_the_real_designs_own_planes_and_reference_angle_when_resolved() {
        let resolved_planes = vec![GpuFacetPlane::new(glam::Vec3::Y, -1.0)];
        let (planes, gear_teeth, reference_angle) = planes_gear_and_reference_angle(
            Some((resolved_planes.clone(), 12, 2.5)),
            Some("Round"),
            Some("999"),
            &[],
        );

        assert_eq!(planes.len(), resolved_planes.len());
        assert_eq!(gear_teeth, 12);
        assert_eq!(reference_angle, 2.5);
    }

    // --- count_angles_for_mode ---

    fn angle_item(side: i32) -> AngleItem {
        AngleItem {
            order_idx: 0,
            side,
            facet: "P1".into(),
            angle: "0.0".into(),
            index_val: "".into(),
            notes: "".into(),
        }
    }

    /// Mode 0 (All) counts every row regardless of side, including girdle/unparsed
    /// rows (`side == 0`).
    #[test]
    fn mode_all_counts_every_row() {
        let angles = ModelRc::new(VecModel::from(vec![
            angle_item(-1),
            angle_item(1),
            angle_item(0),
        ]));
        assert_eq!(count_angles_for_mode(&angles, 0), 3);
    }

    /// Mode 1 (Pavilion) counts only `side < 0` rows -- exactly
    /// `cutting_table.slint`'s own `row_shown` predicate.
    #[test]
    fn mode_pavilion_counts_only_negative_side_rows() {
        let angles = ModelRc::new(VecModel::from(vec![
            angle_item(-1),
            angle_item(-1),
            angle_item(1),
            angle_item(0),
        ]));
        assert_eq!(count_angles_for_mode(&angles, 1), 2);
    }

    /// Mode 2 (Crown) counts only `side > 0` rows.
    #[test]
    fn mode_crown_counts_only_positive_side_rows() {
        let angles = ModelRc::new(VecModel::from(vec![
            angle_item(-1),
            angle_item(1),
            angle_item(1),
            angle_item(0),
        ]));
        assert_eq!(count_angles_for_mode(&angles, 2), 2);
    }

    /// An empty angle list counts to zero under every mode, rather than panicking.
    #[test]
    fn empty_angle_list_counts_to_zero_under_every_mode() {
        let angles: ModelRc<AngleItem> = ModelRc::new(VecModel::from(Vec::<AngleItem>::new()));
        for mode in [0, 1, 2] {
            assert_eq!(count_angles_for_mode(&angles, mode), 0);
        }
    }
}

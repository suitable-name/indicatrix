//! The window's shared state: the design library, the Solid viewport's handles, the
//! persisted settings and the background render thread with its UI-thread receivers.

use super::DB_PATH;
use crate::{
    MainWindow, RemoteWorkerModel, SolidPreviewModel, TiltModel, ViewportModel,
    bridge::render_thread::{RenderContext, spawn_render_thread},
    gui::{
        custom_material_startup::initial_custom_materials_and_specific_gravity,
        editor_layout,
        optics::curve_path::tilt_curve_path,
        remote::refresh_remote_ui,
        show_toast,
        solid_preview::{
            diagram_wiring::{self, DiagramFacetTier, DiagramHoverText, DiagramPick},
            preview_state::{
                DEFAULT_MESH_BOUNDING_RADIUS, FrameGeometry, PickBuffer, SolidLastSolved,
                SolidPreviewState,
            },
        },
        solid_sink::SlintSolidSink,
        startup_settings::{
            apply_loaded_settings, refresh_lighting_preset_options, refresh_material_options,
        },
    },
    settings::{self, SettingsPersister},
};
use indicatrix::geometry::plane::GpuFacetPlane;
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// The path tracer's or the solid worker's last-published plane arrangement, compared by
/// `Arc` pointer identity.
type ActivePlanes = Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>;

/// The Solid viewport's shared handles, created together by [`create_solid_view`] and
/// handed to the callbacks that read or write them.
pub(super) struct SolidView {
    /// The last rendered frame's per-pixel facet-picking buffer.
    pub(super) pick: Arc<Mutex<Option<PickBuffer>>>,
    /// The most-recent-resolved-mast cache.
    pub(super) last_solved: SolidLastSolved,
    /// The Solid view's facet id -> hover-text table.
    pub(super) hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's facet id -> owning-tier table.
    pub(super) facet_tier: Arc<Mutex<Vec<Option<usize>>>>,
    /// The frame on screen's mesh geometry, camera pose and raster size.
    pub(super) geometry: Arc<Mutex<Option<FrameGeometry>>>,
    /// The solid worker's own last-published plane arrangement.
    active_planes: ActivePlanes,
    /// The path tracer's own last-published plane arrangement.
    trace_active_planes: ActivePlanes,
    /// The current solid's own bounding radius.
    pub(super) mesh_bounding_radius: Arc<Mutex<f64>>,
    /// The Edit tab's solid-inspection preview controller.
    pub(super) preview_state: Arc<SolidPreviewState>,
}

/// Opens the on-disk design library, or -- when that fails -- a throwaway in-memory one.
///
/// # Errors
///
/// Returns an error only when the in-memory fallback fails to open too.
pub(super) fn open_design_library(ui: &MainWindow) -> anyhow::Result<Arc<Mutex<Database>>> {
    // Connect to SQLite DB. If the on-disk database can't be opened (locked, corrupt,
    // unwritable directory, ...), fall back to a throwaway in-memory database rather
    // than panicking at startup -- `Database::new` runs the exact same
    // create-tables-if-not-exist + migration chain against `:memory:` as it does
    // against a real file (see `Database::new`'s own doc comment), so the fallback
    // yields a fully-functional, just non-persistent, library. The user is told via a
    // persistent error toast (never auto-dismisses -- see `show_toast`'s doc comment)
    // rather than a status-bar message, since a status bar can be set and then never
    // seen if a later step panics before the next redraw.
    let db = match Database::new(Some(DB_PATH)) {
        Ok(db_inst) => Arc::new(Mutex::new(db_inst)),
        Err(e) => {
            match Database::new(Some(":memory:")) {
                Ok(memory_db) => {
                    show_toast(
                        ui,
                        &format!(
                            "Could not open the design library ({e}) -- running in a \
                             temporary, unsaved library for this session."
                        ),
                        "error",
                    );
                    Arc::new(Mutex::new(memory_db))
                }
                Err(memory_err) => {
                    // Both the real database AND an in-memory SQLite connection
                    // failed to open -- SQLite itself is unusable in this process
                    // (e.g. no SQLite support compiled in, or the process is out of
                    // memory). There is no further degraded mode to fall back to, so
                    // report both errors and bail out of window construction instead
                    // of panicking.
                    return Err(anyhow::anyhow!(
                        "failed to open design library at {DB_PATH:?} ({e}), and the \
                         in-memory fallback database also failed to open ({memory_err})"
                    ));
                }
            }
        }
    };
    Ok(db)
}

/// Creates the Solid viewport's shared handles and its preview controller, and wires the
/// Diagram mode's hover/click callbacks onto them.
pub(super) fn create_solid_view(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> SolidView {
    // The Edit tab's solid-inspection preview controller (see
    // `gui::solid_preview::preview_state`'s own module doc comment). Built right
    // after `render_ctx` -- `setup_camera_and_lighting_callbacks` (wired later)
    // needs it unconditionally (the Live Render viewport's own orbit/zoom drag also
    // re-issues the last solid planes at the new pose). Its worker thread is spawned
    // lazily on the first `request_redraw`, so a session that never opens the Edit
    // tab's Solid view never pays for one.
    //
    // The last rendered frame's per-pixel facet-picking buffer, and the
    // most-recent-resolved-mast cache -- both written by `SlintSolidSink::apply` (the
    // worker thread's callback) and read by `gui::editor`'s hover/click callbacks and
    // edit callbacks respectively. See `solid_preview::preview_state`'s own module doc
    // comment ("Where `last_solved` lives") for why these live here, as plain
    // `Arc<Mutex<..>>`s, rather than on `EditorState` itself.
    let solid_pick: Arc<Mutex<Option<PickBuffer>>> = Arc::new(Mutex::new(None));
    let solid_last_solved: SolidLastSolved = Arc::new(Mutex::new(None));
    // Diagram mode's (view_mode 3) own pick buffer/hover-text/tier tables -- see
    // `SlintSolidSink`'s own doc comment for why these are separate from
    // `solid_pick`/`solid_last_solved` above.
    let diagram_pick: DiagramPick = Arc::new(Mutex::new(None));
    // The index wheel's own tooth pick buffer -- same shape/lifetime as
    // `diagram_pick` above, see `SlintSolidSink::diagram_tooth_pick`'s own doc
    // comment.
    let diagram_tooth_pick: DiagramPick = Arc::new(Mutex::new(None));
    // Which panel painted each diagram pixel -- read by the double-click that
    // enlarges a panel (`diagram_wiring::setup_diagram_double_click_callback`).
    let diagram_panel_pick: DiagramPick = Arc::new(Mutex::new(None));
    let diagram_hover_text: DiagramHoverText = Arc::new(Mutex::new(None));
    let diagram_facet_tier: DiagramFacetTier = Arc::new(Mutex::new(None));
    // The Solid view's own facet id -> hover-text/owning-tier tables, from
    // every frame's `PreviewFrame::hover_text`/`facet_tier` -- the Solid-mode
    // counterpart to `diagram_hover_text`/`diagram_facet_tier` above. Not yet
    // threaded into `editor::setup_editor_callbacks` below (that call site would
    // need two more parameters, and the hover/click handlers that would index
    // these live in `gui::editor::callbacks::tier_actions`) -- see
    // `SlintSolidSink::hover_text`'s doc comment for the full handoff.
    let solid_hover_text: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let solid_facet_tier: Arc<Mutex<Vec<Option<usize>>>> = Arc::new(Mutex::new(Vec::new()));
    // The path tracer's and the solid worker's own last-published plane
    // arrangements (by `Arc` pointer identity) -- see `planes_generations_match`'s
    // doc comment. `trace_active_planes` is also written from the render thread's
    // own frame-push closure (see `spawn_viewport_render_thread`).
    let solid_active_planes: ActivePlanes = Arc::new(Mutex::new(None));
    let trace_active_planes: ActivePlanes = Arc::new(Mutex::new(None));
    // The current solid's own bounding radius, written by every frame
    // (`SlintSolidSink::apply`) and read by `render::camera_lighting`'s orbit-zoom
    // clamp and "Fit" pose -- see `SlintSolidSink::mesh_bounding_radius`'s own doc
    // comment. Defaults to the same fallback a frame carries before anything has
    // ever closed.
    let mesh_bounding_radius: Arc<Mutex<f64>> = Arc::new(Mutex::new(DEFAULT_MESH_BOUNDING_RADIUS));
    // The frame on screen's mesh geometry, camera pose and raster size -- written by
    // `SlintSolidSink::apply` in the same UI-thread closure as `solid_pick`, read by
    // the manipulation handles through `SolidPickState::geometry`.
    let solid_geometry: Arc<Mutex<Option<FrameGeometry>>> = Arc::new(Mutex::new(None));
    let solid_preview_state = SolidPreviewState::new(Arc::new(SlintSolidSink {
        ui: ui.as_weak(),
        pick: Arc::clone(&solid_pick),
        last_solved: Arc::clone(&solid_last_solved),
        diagram_pick: Arc::clone(&diagram_pick),
        diagram_tooth_pick: Arc::clone(&diagram_tooth_pick),
        diagram_panel_pick: Arc::clone(&diagram_panel_pick),
        diagram_hover_text: Arc::clone(&diagram_hover_text),
        diagram_facet_tier: Arc::clone(&diagram_facet_tier),
        hover_text: Arc::clone(&solid_hover_text),
        facet_tier: Arc::clone(&solid_facet_tier),
        render_ctx: Arc::clone(render_ctx),
        solid_active_planes: Arc::clone(&solid_active_planes),
        trace_active_planes: Arc::clone(&trace_active_planes),
        mesh_bounding_radius: Arc::clone(&mesh_bounding_radius),
        geometry: Arc::clone(&solid_geometry),
    }));
    // Diagram mode's own hover/click callbacks -- see
    // `solid_preview::diagram_wiring`'s own module doc comment for why this needs
    // no `gui::editor`/`Design` access, unlike the ordinary Solid view's hover/
    // click callbacks (`gui::editor::callbacks::tier_actions::
    // setup_solid_facet_hover_callback`/`setup_solid_facet_click_callback`).
    // `&solid_preview_state` lets the diagram's own hover/click
    // drive `SolidPreviewState::request_facet_overlay` -- see that function's own
    // doc comment for what each callback now does with it.
    diagram_wiring::setup_diagram_hover_and_click_callbacks(
        ui,
        &diagram_pick,
        &diagram_tooth_pick,
        &diagram_hover_text,
        &diagram_facet_tier,
        &solid_preview_state,
    );
    // Double-click on a diagram panel: enlarge it, or go back to all three.
    diagram_wiring::setup_diagram_double_click_callback(ui, &diagram_panel_pick);
    SolidView {
        pick: solid_pick,
        last_solved: solid_last_solved,
        hover_text: solid_hover_text,
        facet_tier: solid_facet_tier,
        geometry: solid_geometry,
        active_planes: solid_active_planes,
        trace_active_planes,
        mesh_bounding_radius,
        preview_state: solid_preview_state,
    }
}

/// Loads the custom gemstone materials from the library into the render context and the
/// material picker.
pub(super) fn apply_custom_materials(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // Load custom gemstone materials from SQLite. `indicatrix-vault` returns plain
    // `CustomMaterialRow`s (it must not depend on `indicatrix`), so convert them into
    // `indicatrix::optics::materials::GemMaterial` here at the boundary.
    let custom_material_rows = db
        .lock()
        .unwrap()
        .get_custom_materials()
        .unwrap_or_default();
    let (initial_custom_mats, initial_custom_sg) =
        initial_custom_materials_and_specific_gravity(&custom_material_rows);
    {
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ctx.custom_materials = Arc::new(initial_custom_mats.clone());
        ctx.custom_material_specific_gravity = Arc::new(initial_custom_sg);
    }
    refresh_material_options(ui, &initial_custom_mats);
}

/// Loads the persisted settings, applies them to the render context and the UI, and
/// starts the debounced background writer every settings-changing callback feeds.
pub(super) fn load_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Arc<SettingsPersister> {
    // Settings persistence + lighting presets. `load_or_default` never fails/panics --
    // a missing, unreadable, or corrupt file just logs and yields defaults. Applied
    // into `render_ctx` and the UI's mirrored properties before the render thread
    // starts, then handed to a debounced background writer that every
    // settings-changing callback below feeds.
    let settings_path = settings::store::default_settings_path();
    // One-time carry-over from the old public viewer's ("diagram-gui", deleted in the
    // 2026-09-07 rename) settings file -- must run before `load_with_outcome` reads
    // `settings_path`, and only ever copies into a genuinely absent file. See
    // `migrate_legacy_settings_if_needed`'s own doc comment for the
    // copy-never-move/never-overwrite/never-blocks-startup guarantees.
    settings::store::migrate_legacy_settings_if_needed(&settings_path);
    let (loaded_settings, settings_load_outcome) =
        settings::store::load_with_outcome(&settings_path);
    // A corrupt settings file used to be silently replaced with defaults (a
    // `tracing::warn!` nobody watching a release build's console ever sees) and then
    // permanently overwritten by the very next save -- the cutter lost their remote
    // worker, cert directory, export directory and presets with no idea why. Now the
    // corrupt file is renamed aside before the fallback (`load_with_outcome`'s own doc
    // comment), and this toast is what tells the cutter it happened at all. `"warning"`
    // (persistent, not auto-dismissing) rather than `"error"`: the app itself started
    // up fine, but something the cutter should know about and act on did happen.
    if let settings::store::SettingsLoadOutcome::Corrupt { renamed_to } = &settings_load_outcome {
        show_toast(
            ui,
            &format!(
                "Your settings file could not be read and was reset to defaults. The \
                 previous file was kept at {}.",
                renamed_to.display()
            ),
            "warning",
        );
    }
    apply_loaded_settings(ui, render_ctx, &loaded_settings);
    // The Edit sub-tab's resizable dock width / inspector height / collapsed
    // sections -- see `editor_layout::apply_editor_layout_from_settings`'s own doc
    // comment for why this runs before `setup_editor_layout_callbacks` below.
    editor_layout::apply_editor_layout_from_settings(ui, &loaded_settings.settings);
    refresh_lighting_preset_options(ui, &loaded_settings.presets);
    // Remote rendering: the "Remote coordinator" form and every transfer default,
    // restored from the last saved configuration (a legacy multi-worker list was already
    // folded into the single endpoint by `load_or_default`).
    refresh_remote_ui(ui, loaded_settings.settings.remote.as_ref());
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(loaded_settings.settings.denoise_enabled);
    let settings_store = Arc::new(SettingsPersister::spawn(settings_path, loaded_settings));
    // Registered before any editor callback is wired: the editor's recent-file list and
    // "don't ask again" choices are written from many call sites that hold no handle of
    // their own, and every write of the settings file must go through the persister --
    // `flush()` on close writes its snapshot over the file, erasing anything written
    // around it. See `SettingsPersister::install_for_this_thread`.
    SettingsPersister::install_for_this_thread(&settings_store);
    settings_store
}

/// Spawns the background spectral ray tracer and the UI-thread closures that receive its
/// frames, metrics and status texts.
pub(super) fn spawn_viewport_render_thread(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid: &SolidView,
) {
    // Spawn Background Multi-Threaded Physically Based Spectral Raytracer
    let ui_weak_render = ui.as_weak();
    let render_ctx_frame_push = render_ctx.clone();
    let trace_active_planes_push = Arc::clone(&solid.trace_active_planes);
    let solid_active_planes_push = Arc::clone(&solid.active_planes);
    spawn_render_thread(
        ui_weak_render,
        render_ctx.clone(),
        move |ui: &MainWindow, img: slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            ui.global::<ViewportModel>()
                .set_render_image(slint::Image::from_rgba8(img));
            ui.global::<ViewportModel>().set_has_render(true);
            // This path-traced frame reflects whatever `RenderContext::
            // active_planes` currently is -- remember it (by `Arc` pointer
            // identity) and recompute `SolidPreviewModel.trace_matches_solid`
            // against the solid worker's own last-published arrangement. Already
            // running on the UI thread (this closure is `spawn_render_thread`'s
            // `update_image` callback, itself only ever invoked via
            // `Weak::upgrade_in_event_loop` -- see `bridge::render_thread::
            // frame_helpers::push_frame_to_ui`), so locking `render_ctx` here is
            // exactly as safe as every other UI-thread callback in this file
            // already locking it.
            let planes_now = render_ctx_frame_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active_planes
                .clone();
            *trace_active_planes_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(planes_now);
            ui.global::<SolidPreviewModel>().set_trace_matches_solid(
                crate::gui::solid_sink::planes_generations_match(
                    &trace_active_planes_push,
                    &solid_active_planes_push,
                ),
            );
        },
        |ui: &MainWindow,
         brilliance: f32,
         fire: f32,
         scintillation: f32,
         windowing: f32,
         extinction: f32,
         graph_brilliance: [f32; 19],
         graph_extinction: [f32; 19],
         graph_windowing: [f32; 19],
         cam_pitch_deg: f32| {
            ui.global::<ViewportModel>().set_brilliance_pct(brilliance);
            ui.global::<ViewportModel>().set_fire_index(fire);
            ui.global::<ViewportModel>()
                .set_scintillation_pct(scintillation);
            ui.global::<ViewportModel>().set_windowing_pct(windowing);
            ui.global::<ViewportModel>().set_extinction_pct(extinction);
            ui.global::<TiltModel>()
                .set_graph_brilliance(slint::ModelRc::new(slint::VecModel::from(
                    graph_brilliance.to_vec(),
                )));
            ui.global::<TiltModel>()
                .set_graph_extinction(slint::ModelRc::new(slint::VecModel::from(
                    graph_extinction.to_vec(),
                )));
            ui.global::<TiltModel>()
                .set_graph_windowing(slint::ModelRc::new(slint::VecModel::from(
                    graph_windowing.to_vec(),
                )));
            // Tilt-curve `Path::commands` strings for the performance graph dialog's
            // line chart -- derived here rather than threading three more arguments
            // through this closure (see `curve_path::tilt_curve_path`'s doc comment).
            ui.global::<TiltModel>()
                .set_graph_brilliance_path(tilt_curve_path(&graph_brilliance).into());
            ui.global::<TiltModel>()
                .set_graph_extinction_path(tilt_curve_path(&graph_extinction).into());
            ui.global::<TiltModel>()
                .set_graph_windowing_path(tilt_curve_path(&graph_windowing).into());
            ui.global::<TiltModel>().set_cam_pitch_deg(cam_pitch_deg);
        },
        // Pushes `ViewportGpu`'s self-healing status text
        // (empty while healthy) to `ViewportModel.gpu_status_text` -- see
        // `spawn_render_thread`'s own `update_gpu_status` doc comment.
        |ui: &MainWindow, text: slint::SharedString| {
            ui.global::<ViewportModel>().set_gpu_status_text(text);
        },
        // Pushes `RenderContext::material_unresolved`'s cutter-facing reason (empty
        // once resolved) to `ViewportModel.trace_refusal` -- see `spawn_render_thread`'s
        // own `update_trace_refusal` doc comment.
        |ui: &MainWindow, text: slint::SharedString| {
            ui.global::<ViewportModel>().set_trace_refusal(text);
        },
    );
}

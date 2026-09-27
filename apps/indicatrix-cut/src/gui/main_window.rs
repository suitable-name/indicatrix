//! Building and fully wiring a `MainWindow`, and the RAII handle that keeps its
//! render thread/settings-autosave/remote-rendering polling alive.

use crate::{
    EditorModel, LibraryModel, MainWindow, RemoteWorkerModel, SettingsModel, SolidPreviewModel,
    TiltModel, ViewportModel,
    bridge::{
        library::source::LibrarySource,
        render_thread::{RenderContext, spawn_render_thread},
    },
    gui::{
        batch,
        custom_material_startup::initial_custom_materials_and_specific_gravity,
        editor, editor_layout,
        external_links::{setup_reveal_last_saved_callback, setup_user_manual_callback},
        library, optics,
        optics::curve_path::tilt_curve_path,
        remote::{refresh_remote_ui, setup_remote_rendering, setup_worker_callbacks},
        render, show_toast,
        solid_preview::{
            diagram_wiring::{self, DiagramFacetTier, DiagramHoverText, DiagramPick},
            preview_state::{
                DEFAULT_MESH_BOUNDING_RADIUS, PickBuffer, SolidLastSolved, SolidPickState,
                SolidPreviewState,
            },
        },
        solid_sink::SlintSolidSink,
        startup_settings::{
            apply_loaded_settings, refresh_lighting_preset_options, refresh_material_options,
        },
        tilt,
        window_close::setup_close_confirm_callbacks,
    },
    settings::{self, SettingsPersister},
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// The on-disk design library's default path -- also used by
/// `gui::library::search::fetch` to open a read-only connection for background
/// search, hence `pub(in crate::gui)` rather than private (re-exported at
/// `gui::DB_PATH` by `mod.rs` so that existing call site keeps resolving).
pub(in crate::gui) const DB_PATH: &str = "facet_diagrams.sqlite";

/// A fully constructed and wired [`MainWindow`], not yet shown or running. Built by
/// [`build_main_window`].
///
/// Exists (rather than `run_gui` doing everything and blocking on `ui.run()`) so a
/// second binary that wants this *same*, already-wired public window -- the private
/// edition's GUI, which shows this window alongside its own separate sync window --
/// can call [`build_main_window`], `.show()` both windows, and drive a single shared
/// event loop itself, without duplicating any of this crate's rendering/settings/
/// remote-rendering wiring.
///
/// `ui` is the only public field. The rest (render context, settings persister,
/// remote-rendering poll timer) exist purely to be kept alive for the life of the
/// window -- dropping any of them early would stop the thing it drives (the render
/// thread, settings autosave, remote-rendering handoff polling) -- so the caller only
/// needs to keep the whole `MainWindowHandle` alive, never reach into its internals.
pub struct MainWindowHandle {
    pub ui: MainWindow,
    // `render_ctx`/`remote_rendering_timer` are never read again after construction --
    // both exist purely so dropping `MainWindowHandle` is what stops the render thread/
    // remote-rendering handoff polling, not so anything downstream can inspect them.
    // `#[expect(dead_code)]` rather than removing them: removing a field would drop it
    // at the end of `build_main_window` instead of at the end of the caller's
    // `MainWindowHandle`'s lifetime, which is the entire point of holding onto them
    // here. `settings_store` is the same kind of RAII guard (keeps the debounced
    // settings writer alive) but is also read once, by `run_gui` right below, hence no
    // `#[expect(dead_code)]` on it -- see that field's own comment.
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    render_ctx: Arc<Mutex<RenderContext>>,
    // Not `#[expect(dead_code)]` like its two siblings above/below: `run_gui` reads
    // this field directly (`handle.settings_store.snapshot()`) to learn whether the
    // Edit sub-tab's layout was ever touched, for `window_sizing::
    // fit_initial_window_size`'s small-screen default. Still exists primarily as the
    // same kind of RAII guard -- kept alive so the debounced settings writer keeps
    // running for the life of the window -- but, unlike its siblings, it is also
    // read directly by `run_gui`, per the paragraph above.
    pub(super) settings_store: Arc<SettingsPersister>,
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    remote_rendering_timer: slint::Timer,
}

/// Builds and fully wires a `MainWindow`, up to but not including actually
/// running the event loop. See [`MainWindowHandle`]'s doc comment for why this
/// is split out from `super::run_gui`.
///
/// # Errors
///
/// Returns an error if constructing the `MainWindow` itself fails (a Slint platform
/// initialization failure), or if the on-disk design library fails to open AND the
/// in-memory fallback database also fails to open (SQLite itself unusable in this
/// process -- see the database-open block below). A plain on-disk open failure is
/// handled internally: it falls back to a throwaway in-memory database and reports
/// the failure via a persistent error toast rather than propagating it or panicking.
///
/// # Panics
///
/// Panics if any of this function's several `Mutex::lock().unwrap()` calls (`db`,
/// `render_ctx`) observes a poisoned lock -- which only happens if some earlier code
/// already panicked while holding that same lock, so this is a symptom of a prior
/// panic elsewhere, not a new failure mode this function introduces.
// Long by one function's worth of setup calls (every one of them already split out
// and individually under the limit) -- this is `run_gui`'s original body, unchanged
// in shape by the `MainWindowHandle` split; splitting it further would just move the
// line count into an equally long "call every setup_* function" wrapper.
#[expect(
    clippy::too_many_lines,
    reason = "a flat sequence of already-extracted setup_* calls (see comment above) -- \
              splitting further just moves the same line count into a wrapper that \
              calls them all, not a real reduction"
)]
pub fn build_main_window() -> anyhow::Result<MainWindowHandle> {
    let ui = MainWindow::new()?;

    // Local Compute (Feature: a local CPU/CPU+GPU/GPU switch): whether this build was
    // compiled with the `gpu` feature -- Rust-driven, set once here (Slint has no
    // `cfg!` of its own), never touched again. `settings_dialog.slint`'s "Local
    // Compute" row binds its whole `visible:` to this, so a CPU-only build never
    // offers a choice it has no GPU path to act on.
    ui.global::<SettingsModel>()
        .set_gpu_build(cfg!(feature = "gpu"));

    // The editor is not optional (there is no `editor` Cargo feature gating it),
    // so `EditorModel.enabled` is unconditionally `true` -- kept as a real
    // property (rather than deleted) because `app.slint`'s "Edit" sub-tab and
    // several other components still read it as their enablement guard. Slint
    // markup has no `#[cfg]` of its own, so this flag is how they would detect a
    // gated build if one ever existed.
    ui.global::<EditorModel>().set_enabled(true);

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
                        &ui,
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

    // Shared Render Context for 3D Viewport
    let render_ctx = Arc::new(Mutex::new(RenderContext::default()));

    // The Edit tab's solid-inspection preview controller (see
    // `gui::solid_preview::preview_state`'s own module doc comment). Built here,
    // alongside `render_ctx` above -- `setup_camera_and_lighting_callbacks` below
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
    // own frame-push closure below.
    let solid_active_planes: Arc<
        Mutex<Option<Arc<Vec<indicatrix::geometry::plane::GpuFacetPlane>>>>,
    > = Arc::new(Mutex::new(None));
    let trace_active_planes: Arc<
        Mutex<Option<Arc<Vec<indicatrix::geometry::plane::GpuFacetPlane>>>>,
    > = Arc::new(Mutex::new(None));
    // The current solid's own bounding radius, written by every frame
    // (`SlintSolidSink::apply`) and read by `render::camera_lighting`'s orbit-zoom
    // clamp and "Fit" pose -- see `SlintSolidSink::mesh_bounding_radius`'s own doc
    // comment. Defaults to the same fallback a frame carries before anything has
    // ever closed.
    let mesh_bounding_radius: Arc<Mutex<f64>> = Arc::new(Mutex::new(DEFAULT_MESH_BOUNDING_RADIUS));
    let solid_preview_state = SolidPreviewState::new(Arc::new(SlintSolidSink {
        ui: ui.as_weak(),
        pick: Arc::clone(&solid_pick),
        last_solved: Arc::clone(&solid_last_solved),
        diagram_pick: Arc::clone(&diagram_pick),
        diagram_tooth_pick: Arc::clone(&diagram_tooth_pick),
        diagram_hover_text: Arc::clone(&diagram_hover_text),
        diagram_facet_tier: Arc::clone(&diagram_facet_tier),
        hover_text: Arc::clone(&solid_hover_text),
        facet_tier: Arc::clone(&solid_facet_tier),
        render_ctx: Arc::clone(&render_ctx),
        solid_active_planes: Arc::clone(&solid_active_planes),
        trace_active_planes: Arc::clone(&trace_active_planes),
        mesh_bounding_radius: Arc::clone(&mesh_bounding_radius),
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
        &ui,
        &diagram_pick,
        &diagram_tooth_pick,
        &diagram_hover_text,
        &diagram_facet_tier,
        &solid_preview_state,
    );

    // Which library (local database, or a remote worker) the viewer is
    // currently browsing. Starts `LibrarySource::Local` -- see that type's own doc
    // comment on why every local-only call site keeps behaving as plain local-only
    // until a user actually switches.
    let library_source: Arc<Mutex<LibrarySource>> = Arc::new(Mutex::new(LibrarySource::default()));

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
    refresh_material_options(&ui, &initial_custom_mats);

    // Settings persistence + lighting presets. `load_or_default` never fails/panics --
    // a missing, unreadable, or corrupt file just logs and yields defaults. Applied
    // into `render_ctx` and the UI's mirrored properties before the render thread
    // starts, then handed to a debounced background writer that every
    // settings-changing callback below feeds.
    let settings_path = settings::store::default_settings_path();
    // One-time carry-over from the public `indicatrix-cut`'s settings file -- must run
    // before `load_or_default` reads `settings_path`, and only ever copies into a
    // genuinely absent file. See `migrate_legacy_settings_if_needed`'s own doc comment
    // for the copy-never-move/never-overwrite/never-blocks-startup guarantees.
    settings::store::migrate_legacy_settings_if_needed(&settings_path);
    let loaded_settings = settings::store::load_or_default(&settings_path);
    apply_loaded_settings(&ui, &render_ctx, &loaded_settings);
    // The Edit sub-tab's resizable dock width / inspector height / collapsed
    // sections -- see `editor_layout::apply_editor_layout_from_settings`'s own doc
    // comment for why this runs before `setup_editor_layout_callbacks` below.
    editor_layout::apply_editor_layout_from_settings(&ui, &loaded_settings.settings);
    refresh_lighting_preset_options(&ui, &loaded_settings.presets);
    // Remote rendering: the "Remote coordinator" form and every transfer default,
    // restored from the last saved configuration (a legacy multi-worker list was already
    // folded into the single endpoint by `load_or_default`).
    refresh_remote_ui(&ui, loaded_settings.settings.remote.as_ref());
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(loaded_settings.settings.denoise_enabled);
    let settings_store = Arc::new(SettingsPersister::spawn(settings_path, loaded_settings));

    // Detachable Live Render window: sub-tab/detach-toggle wiring, plus the shared
    // frame-routing target the render thread's own frame-push closure below also
    // mirrors into -- see `detached_render`'s module doc comment.
    let detached_frame_target = render::detached_render::setup_live_render_visibility_callbacks(
        &ui,
        &render_ctx,
        &solid_preview_state,
    );
    // The settings load above (`apply_loaded_settings`/`apply_loaded_ui_mirrors`)
    // already pushed the restored `ViewportModel.live_view_mode`/`SolidPreviewModel.
    // view_mode` into the UI, but nothing had recomputed `render_ctx.tab_visible`
    // against them yet -- it was still sitting at `RenderContext::default()`'s `true`.
    // Without this, a session restored into Solid mode would keep tracing (wasting
    // GPU/remote time, the very thing this gate exists to stop) until the user
    // happened to touch a tab. One-time sync, now that every signal
    // `detached_render::render_is_visible` reads is in place.
    render::detached_render::recompute_tab_visible(&ui, &render_ctx);

    // Spawn Background Multi-Threaded Physically Based Spectral Raytracer
    let ui_weak_render = ui.as_weak();
    let render_ctx_frame_push = render_ctx.clone();
    let trace_active_planes_push = Arc::clone(&trace_active_planes);
    let solid_active_planes_push = Arc::clone(&solid_active_planes);
    spawn_render_thread(
        ui_weak_render,
        render_ctx.clone(),
        move |ui: &MainWindow, img: slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            // Both destinations are updated on EVERY frame regardless of which one is
            // currently visible -- see `detached_render`'s module doc comment's "Frame
            // routing" section for why that's what keeps either side of a dock/undock
            // from ever showing a stale or frozen frame. `slint::Image::from_rgba8`
            // wraps `img` in a cheap, atomically-refcounted handle, so cloning it for
            // the (usually absent) second destination costs a pointer bump, not a
            // pixel copy.
            let slint_img = slint::Image::from_rgba8(img);
            ui.global::<ViewportModel>()
                .set_render_image(slint_img.clone());
            ui.global::<ViewportModel>().set_has_render(true);
            // The lock guard is dropped (via this `let`) before the `if let` below --
            // holding it across the scrutinee would keep it alive for the whole `if`
            // body, including the `set_render_image`/`set_has_render` calls, which
            // don't need it at all.
            let detached_weak = detached_frame_target.lock().unwrap().clone();
            if let Some(detached) = detached_weak.and_then(|weak| weak.upgrade()) {
                detached.set_render_image(slint_img);
                detached.set_has_render(true);
            }
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
    );

    // Persist the library panel's collapsed/expanded state -- same "debounced
    // background writer" persistence every other durable setting in this file already
    // goes through, just a one-off `bool` with no dedicated `gui::*` module of its own.
    let settings_store_panel = settings_store.clone();
    ui.global::<LibraryModel>()
        .on_panel_collapsed_changed(move |collapsed: bool| {
            settings_store_panel.update(|s| s.settings.library_panel_collapsed = collapsed);
        });
    // Persist the Edit sub-tab's resizable dock width / inspector height /
    // collapsed sections the same way -- see `editor_layout::
    // setup_editor_layout_callbacks`'s own doc comment.
    editor_layout::setup_editor_layout_callbacks(&ui, &settings_store);

    setup_user_manual_callback(&ui);
    setup_reveal_last_saved_callback(&ui);
    render::camera_lighting::setup_camera_and_lighting_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
        &solid_preview_state,
        &mesh_bounding_radius,
    );
    render::material_quality::setup_material_changed_callback(&ui, &render_ctx, &settings_store);
    render::material_quality::setup_backdrop_callback(&ui, &render_ctx, &settings_store);
    render::material_quality::setup_material_and_quality_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
    );
    render::material_quality::setup_material_effect_override_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
    );
    render::camera_lighting::setup_environment_map_callbacks(&ui, &render_ctx, &settings_store);
    render::lighting_presets::setup_lighting_preset_callbacks(&ui, &render_ctx, &settings_store);
    render::render_export::setup_render_export_callbacks(&ui, &render_ctx, &settings_store);
    optics::custom_materials::setup_custom_material_callbacks(&ui, &render_ctx, &db);
    library::clipboard::setup_copy_callbacks(&ui);
    tilt::tilt_profile::setup_tilt_profile_callback(&ui, &render_ctx, &settings_store);
    tilt::tilt_hover_preview::setup_tilt_hover_preview_callback(&ui, &render_ctx);
    library::diagram_list::load_filter_options_and_initial_list(&ui, &db);
    library::diagram_list::setup_search_and_filter_callbacks(&ui, &db, &library_source);
    // The owner's "regenerate library previews/tilt curves for the filtered set"
    // request -- see `setup_regenerate_filtered_set_callbacks`'s own doc comment.
    library::diagram_list::setup_regenerate_filtered_set_callbacks(&ui, &db, &library_source);
    library::diagram_list::setup_diagram_selection_and_export_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &solid_preview_state,
    );
    // Tilt-performance filter rows (add/remove/clear-all) -- Rust-mediated but
    // UI-owned state.
    library::diagram_list::setup_performance_filter_callbacks(&ui);
    library::local::setup_import_callback(&ui, &db, &library_source, &settings_store);
    library::local::setup_rename_callback(&ui, &db, &library_source);
    library::local::setup_set_shape_callback(&ui, &db, &library_source);
    library::local::setup_delete_callback(&ui, &db, &library_source, &render_ctx);
    library::local::setup_export_asc_callback(&ui, &db, &library_source);
    // Per-design ignore/un-ignore toggle -- grouped with the other library CRUD
    // callbacks above since it's the same shape (local-only write, then
    // `refresh_after_library_change`).
    library::local::setup_ignore_toggle_callback(&ui, &db, &library_source);
    library::local::setup_add_tag_callback(&ui, &db, &library_source);
    library::local::setup_remove_tag_callback(&ui, &db, &library_source);
    library::detail::setup_save_metadata_callback(&ui, &db, &library_source, &render_ctx);
    // The cutting-table's per-mode row-count pills.
    library::detail::setup_filtered_row_count_callback(&ui);
    // The Edit sub-tab's own callbacks (tier add/remove/modify, preform controls,
    // undo/redo, loading the selected design, `.asc` export).
    // Bundles the three Solid-viewport handles above into the one value
    // `editor::setup_editor_callbacks` takes for them -- see [`SolidPickState`]'s
    // own doc comment for why they travel together.
    let solid_pick_state = SolidPickState {
        pick: Arc::clone(&solid_pick),
        hover_text: Arc::clone(&solid_hover_text),
        facet_tier: Arc::clone(&solid_facet_tier),
    };
    editor::setup_editor_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &solid_preview_state,
        &solid_last_solved,
        &solid_pick_state,
    );
    // Persist the Solid viewport's view mode through the same debounced writer every
    // other durable setting in this file goes through -- see
    // `AppSettings::solid_view_mode`'s own doc comment. Also recomputes
    // `render_ctx.tab_visible`: this mode is one of the five signals
    // `render::detached_render::render_is_visible` reads (the Edit tab traces live
    // only in Path-traced/Both), so flipping it can turn the tracer on or off.
    //
    // Also re-requests a redraw at the new mode via `resubmit_at_current_pose` --
    // otherwise switching TO Diagram or Both would never ask the worker for
    // anything, so both would stay on whatever `has_diagram`/`has_solid_edges` a
    // PREVIOUS mode happened to leave set (usually `false`, showing the "Solve to
    // preview" placeholder until the user jiggled the mouse or made an edit).
    let settings_store_solid_view_mode = settings_store.clone();
    let render_ctx_solid_view_mode = render_ctx.clone();
    let preview_state_solid_view_mode = Arc::clone(&solid_preview_state);
    let ui_weak_solid_view_mode = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_view_mode_changed(move |mode: i32| {
            settings_store_solid_view_mode.update(|s| {
                s.settings.solid_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            if let Some(ui) = ui_weak_solid_view_mode.upgrade() {
                render::detached_render::recompute_tab_visible(&ui, &render_ctx_solid_view_mode);
                let mut ctx = render_ctx_solid_view_mode
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Path-traced/Both (modes 1/2) must resume accumulation from
                // scratch on entry, mirroring `on_live_view_mode_changed` below --
                // otherwise an accumulator already at `target_samples` for the
                // current planes leaves the render thread's worker loop sleeping
                // instead of sending a display cycle (`bridge/render_thread/mod.rs`),
                // so flipping the Edit tab into Path-traced/Both would resume
                // tracing but never force a visible restart: the newly revealed
                // surface would show whatever was last pushed, reading as "this is
                // current" even when it isn't.
                if mode == 1 || mode == 2 {
                    ctx.dirty = true;
                }
                render::camera_lighting::resubmit_at_current_pose(
                    &ui,
                    &ctx,
                    &preview_state_solid_view_mode,
                );
                // Explicit: releases the `RenderContext` mutex right after its
                // last use rather than leaving it held until this block's
                // closing brace.
                drop(ctx);
            }
        });
    // Handles the debounced `SolidPreviewModel.viewport_resized()` signal
    // (`solid_viewport.slint`'s `resize_pulse` timer) -- without it, dragging the
    // dock splitter or maximising the window would leave the shown image
    // rescaled/letterboxed and the pick buffer misaligned from the cursor until the
    // next orbit/zoom redraw. Deferred by one event-loop turn (`slint::Timer::
    // single_shot`) so the resubmit reads the size AFTER Slint's own layout pass
    // for this resize has actually run, not whatever was current the instant the
    // debounce timer fired.
    let render_ctx_viewport_resized = render_ctx.clone();
    let preview_state_viewport_resized = Arc::clone(&solid_preview_state);
    let ui_weak_viewport_resized = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_viewport_resized(move || {
            let ui_weak = ui_weak_viewport_resized.clone();
            let render_ctx = render_ctx_viewport_resized.clone();
            let preview_state = Arc::clone(&preview_state_viewport_resized);
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let ctx = render_ctx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                render::camera_lighting::resubmit_at_current_pose(&ui, &ctx, &preview_state);
            });
        });
    // Persist the Live Render tab's own view mode the same way -- see
    // `AppSettings::live_view_mode`'s own doc comment. Same `recompute_tab_visible`
    // reasoning as `solid_view_mode` above (the Live tab traces live only in
    // Path-traced mode), plus two effects specific to this toggle:
    // - Flipping TO Path-traced (`1`) must resume accumulation from the current
    //   planes rather than showing whatever sample count was left over from the last
    //   time this mode was visible (`ctx.dirty = true` forces a fresh restart, same as
    //   every other scene-invalidating write to `RenderContext`).
    // - Flipping TO Solid (`0`) must show the solid raster immediately, not wait for
    //   the next camera drag or library selection to trigger a redraw
    //   (`camera_lighting::resubmit_live_solid`).
    let settings_store_live_view_mode = settings_store.clone();
    let render_ctx_live_view_mode = render_ctx.clone();
    let preview_state_live_view_mode = Arc::clone(&solid_preview_state);
    let ui_weak_live_view_mode = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_live_view_mode_changed(move |mode: i32| {
            settings_store_live_view_mode.update(|s| {
                s.settings.live_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            let Some(ui) = ui_weak_live_view_mode.upgrade() else {
                return;
            };
            render::detached_render::recompute_tab_visible(&ui, &render_ctx_live_view_mode);
            let mut ctx = render_ctx_live_view_mode
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if mode == 1 {
                ctx.dirty = true;
            } else if mode == 0 {
                render::camera_lighting::resubmit_live_solid(
                    &ui,
                    &ctx,
                    &preview_state_live_view_mode,
                );
            }
            drop(ctx);
        });
    // Persist the Edit sub-tab's auto-solve budget the same way -- see
    // `AppSettings::editor_auto_solve_budget_ms`'s own doc comment and
    // `gui::editor::auto_solve`'s module doc comment for the setting's effect.
    let settings_store_auto_solve_budget = settings_store.clone();
    ui.global::<EditorModel>()
        .on_auto_solve_budget_ms_changed(move |budget_ms: i32| {
            settings_store_auto_solve_budget.update(|s| {
                s.settings.editor_auto_solve_budget_ms =
                    u32::try_from(budget_ms).unwrap_or_default();
            });
        });
    library::remote::setup_library_source_callbacks(&ui, &db, &library_source, &settings_store);
    library::remote::setup_mirror_sync_callbacks(&ui, &db, &settings_store);
    setup_worker_callbacks(&ui, &render_ctx, &settings_store);
    // Cached design-catalogue preview thumbnails (front/top renders) -- decoded and
    // cached off the UI thread (see `preview_cache`'s own doc comment), and the
    // background batch job that generates them (two-level progress, cancellation,
    // per-item panic isolation, local/remote dispatch over a shared work queue -- see
    // `preview_batch`'s own doc comment). The batch-callbacks setup also kicks off the
    // once-per-session "designs with no previews yet" scan/offer (trigger: library
    // scan) as part of its own wiring.
    batch::preview_cache::setup_preview_thumbnail_callback(&ui, &db);
    batch::preview::setup_preview_batch_callbacks(&ui, &db, &render_ctx, &settings_store);
    // Tilt-curve batch computation -- same shape as the preview batch above,
    // deliberately never auto-scanned at startup (see `tilt_batch`'s own module doc
    // comment for why: the cost per catalogue is roughly an order of magnitude
    // higher).
    batch::tilt::setup_tilt_batch_callbacks(&ui, &db, &render_ctx, &settings_store);
    // Preview-then-handoff remote rendering: a repeating `slint::Timer` that polls
    // camera/light pose and drives the handoff -- must be kept alive for the life of
    // the window (a dropped `Timer` simply stops firing), hence the binding held all
    // the way to `ui.run()` below rather than being dropped immediately.
    let remote_rendering_timer = setup_remote_rendering(&ui, &render_ctx, &settings_store);

    // Signal the render thread to exit when the window closes. `running` is the thread's
    // shutdown flag -- it's only ever set to false here, once, since the thread has no way to
    // be restarted. Without this the render thread would silently outlive the window.
    //
    // Gated on `EditorModel.is_dirty` so an unsaved Edit-tab design is never
    // discarded by one click on the window's own close button with no prompt.
    // A dirty design keeps the window open and shows `MainWindow.close_confirm_open`'s
    // Save/Discard/Cancel guard instead (see `close_confirm_open`'s own doc comment in
    // `app.slint`); `close_confirm_save`/`close_confirm_discard` below do the actual
    // shutdown once the user picks one.
    let render_ctx_close = render_ctx.clone();
    let settings_store_close = settings_store.clone();
    let ui_weak_close = ui.as_weak();
    ui.window().on_close_requested(move || {
        let is_dirty = ui_weak_close
            .upgrade()
            .is_some_and(|ui| ui.global::<EditorModel>().get_is_dirty());
        if !is_dirty {
            render_ctx_close
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .running = false;
            // Bypasses the debounce window -- a change made in the last `DEBOUNCE`
            // interval before quitting must not be silently lost.
            settings_store_close.flush();
            return slint::CloseRequestResponse::HideWindow;
        }
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.set_close_confirm_open(true);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    setup_close_confirm_callbacks(&ui, &render_ctx, &settings_store);

    Ok(MainWindowHandle {
        ui,
        render_ctx,
        settings_store,
        remote_rendering_timer,
    })
}

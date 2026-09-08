// Only `search` is `pub`: `refresh_diagram_list` is reused by a downstream binary's
// sync-complete handler (see `search::refresh_diagram_list`'s doc comment). The rest
// are wiring `build_main_window` uses internally.
mod batch;
// The `indicatrix-cut-core`-backed Edit sub-tab. Only compiled with the `editor`
// feature -- see that feature's own `Cargo.toml` comment for why (it is also what
// makes the `indicatrix-cut-core` dependency itself optional).
#[cfg(feature = "editor")]
mod editor;
mod library;
mod optics;
mod remote;
mod render;
pub mod solid_preview;
// Startup settings-application and the settings-value <-> UI-pill-index conversions
// that go with it -- moved out of this file purely to keep it from growing further.
// Several of its items are used well beyond startup (e.g. by callbacks that need the
// same index<->value mapping later), so this module re-exports them flatly below
// rather than requiring every caller to spell out `gui::startup_settings::`.
mod startup_settings;
mod tilt;

// `sync_range_bounds_to_ui` is defined in `library::diagram_list` but re-exported here
// so its public path (`gui::sync_range_bounds_to_ui`) is unchanged for a downstream
// binary's sync-complete handler and this crate's own `gui::library`, both of which
// call it at that spelling.
pub use library::diagram_list::sync_range_bounds_to_ui;
// `search` used to be a direct top-level `gui` submodule (`pub mod search;`) -- now
// grouped into `gui::library` with the rest of the library UI (see that module's own
// doc comment), but re-exported here under its original name/path so `gui::search`
// (the one path a downstream binary's sync-complete handler is documented to use --
// see `search::refresh_diagram_list`'s doc comment) keeps resolving unchanged.
pub use library::search;
use startup_settings::apply_loaded_settings;
// Flat re-exports so every existing call site elsewhere in `gui` (which all spell
// these as `gui::X`, never `gui::startup_settings::X`) keeps resolving unchanged.
pub(in crate::gui) use startup_settings::{
    color_space_from_index, env_map_status_text, is_c_axis_override_available,
    local_compute_target_from_index, local_preview_scale_from_index,
    refresh_lighting_preset_options, refresh_material_options,
};

use crate::{
    EditorModel, LibraryModel, MainWindow, RemoteWorkerModel, SettingsModel, SolidPreviewModel,
    TiltModel, ViewportModel,
    bridge::{
        library::source::LibrarySource,
        render_thread::{RenderContext, spawn_render_thread},
    },
    gui::{
        optics::{crystal_optics::gem_material_from_row, curve_path::tilt_curve_path},
        remote::{refresh_worker_options, setup_remote_rendering, setup_worker_callbacks},
        solid_preview::{
            diagram_wiring::{self, DiagramFacetTier, DiagramHoverText, DiagramPick},
            preview_state::{
                PickBuffer, PreviewFrame, PreviewSink, SolidLastSolved, SolidPreviewState,
            },
        },
    },
    settings::{self, SettingsPersister},
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::sync::{Arc, Mutex};

const DB_PATH: &str = "facet_diagrams.sqlite";

/// This crate's binary entry point (`apps/indicatrix-cut/src/main.rs` calls
/// straight through to this).
///
/// # Errors
///
/// See [`run_gui`].
pub fn main() -> anyhow::Result<()> {
    run_gui()
}

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
    // Never read again after construction -- these three exist purely so dropping
    // `MainWindowHandle` is what stops the render thread/settings autosave/handoff
    // polling, not so anything downstream can inspect them. `#[expect(dead_code)]`
    // rather than removing them: removing a field would drop it at the end of
    // `build_main_window` instead of at the end of the caller's `MainWindowHandle`'s
    // lifetime, which is the entire point of holding onto them here.
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    render_ctx: Arc<Mutex<RenderContext>>,
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    settings_store: Arc<SettingsPersister>,
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    remote_rendering_timer: slint::Timer,
}

/// Runs this crate's window standalone: builds it (see [`build_main_window`])
/// and runs it to completion.
///
/// # Errors
///
/// Returns an error if [`build_main_window`] or the window's own event loop
/// (`MainWindow::run`) fails -- see [`build_main_window`]'s doc comment for what that
/// covers.
pub fn run_gui() -> anyhow::Result<()> {
    let handle = build_main_window()?;
    handle.ui.run()?;
    Ok(())
}

/// The real [`PreviewSink`] -- hops back to the UI thread exactly like
/// `bridge::render_thread::frame_helpers::push_frame_to_ui` already does for the
/// path-traced view, pushing a finished solid-preview frame into `MainWindow`'s
/// `editor_solid_image`/`editor_has_solid`/`editor_solid_status`/`editor_solid_stale`/
/// `editor_solid_edges_image`/`editor_has_solid_edges`/`editor_solid_diagram_image`/
/// `editor_has_diagram` properties. See `gui::solid_preview::preview_state`'s own
/// module doc comment ("`PreviewSink`: kept generic over Slint on purpose") for why
/// this hop lives here rather than inside `SolidPreviewState` itself.
///
/// `pick`/`last_solved`/`diagram_pick`/`diagram_hover_text`/`diagram_facet_tier` are
/// plain `Arc<Mutex<..>>` state, not Slint properties, so `gui::editor`'s hover/click
/// callbacks, `solid_preview::diagram_wiring`'s Diagram-mode hover/click callbacks,
/// and the next edit's [`solid_preview::preview_state::ReplanRequest::last_solved`]
/// can all read them without needing a Slint window handle of their own. See
/// `preview_state`'s own module doc comment ("Where `last_solved` lives") for why
/// these live here rather than on `gui::editor::state::EditorState`.
///
/// They are stored inside [`PreviewSink::apply`]'s `upgrade_in_event_loop` closure,
/// on the UI thread, together with the image swap that closure already performs --
/// NOT immediately on the calling worker thread. See that `impl`'s own doc comment
/// ("Atomicity guarantee") for why: a click landing between an immediate worker-
/// thread store and a deferred image swap used to resolve against a pick buffer
/// newer than the image still on screen -- see `docs/history/indicatrix-cut.md`.
struct SlintSolidSink {
    ui: Weak<MainWindow>,
    pick: Arc<Mutex<Option<PickBuffer>>>,
    last_solved: SolidLastSolved,
    // Diagram mode's (view_mode 3) own pick buffer/hover-text/tier tables -- a
    // separate `Arc<Mutex<..>>` triple from `pick`/`last_solved` above, since the
    // diagram is an entirely different pixel layout resolved without ever needing
    // `Design`/`FacetMap` access on the UI thread (see
    // `solid_preview::diagram_wiring`'s own module doc comment).
    diagram_pick: DiagramPick,
    diagram_hover_text: DiagramHoverText,
    diagram_facet_tier: DiagramFacetTier,
}

impl PreviewSink for SlintSolidSink {
    /// # Atomicity guarantee
    ///
    /// Every piece of this frame's state -- `pick`/`last_solved`/the Diagram-mode
    /// side tables AND the displayed image -- is swapped together, inside this one
    /// `upgrade_in_event_loop` closure, on the UI thread. Before this was true, the
    /// pick buffers were stored immediately on the WORKER thread while the image swap
    /// was deferred to the UI thread's event loop; a click landing on the OLD
    /// (still-displayed) image in that gap would resolve against the NEW pick buffer
    /// already stashed for the next frame, picking the wrong facet.
    /// Deferring every store into the closure removes that window: whichever frame's
    /// image is on screen, that SAME frame's pick/solved/diagram data is what a click
    /// arriving right after the swap will see -- never a newer buffer paired with an
    /// older image, or vice versa.
    fn apply(&self, frame: PreviewFrame) {
        let PreviewFrame {
            image,
            has_solid,
            status,
            solved,
            stale,
            pick,
            edges_image,
            diagram_image,
            has_diagram,
            diagram_pick,
            diagram_hover_text,
            diagram_facet_tier,
        } = frame;

        // Cloned Arcs (cheap: one atomic increment each), not `self` itself -- `self`
        // borrows for only the duration of this call, but the closure below is queued
        // to run later on the UI thread's event loop and must be `'static`. These are
        // plain `Arc<Mutex<..>>` state (`Send`, unlike `slint::Image` -- see the
        // comment further down), so cloning them across the boundary is exactly as
        // sound as it was for the payload fields already being moved in below.
        let pick_state = Arc::clone(&self.pick);
        let last_solved_state = Arc::clone(&self.last_solved);
        let diagram_pick_state = Arc::clone(&self.diagram_pick);
        let diagram_hover_text_state = Arc::clone(&self.diagram_hover_text);
        let diagram_facet_tier_state = Arc::clone(&self.diagram_facet_tier);

        // `slint::Image::from_rgba8` runs HERE, inside the UI-thread closure, not on
        // the worker thread that built `image` -- see `gui::solid_preview::
        // to_pixel_buffer`'s own doc comment: `slint::Image` is not `Send`, so it
        // cannot be the payload crossing into `upgrade_in_event_loop` itself.
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            *pick_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pick);
            *last_solved_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = solved;
            // Diagram mode's side tables are only ever `Some` for a view_mode-3
            // request (see `PreviewFrame::diagram_hover_text`'s doc comment) -- a
            // frame from another view mode leaves the last-known diagram state alone
            // rather than clobbering it with `None`, so switching back to Diagram mode
            // between edits does not lose the tooltip/click table before the next
            // view_mode-3 frame lands.
            if let Some(diagram_pick) = diagram_pick {
                *diagram_pick_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(diagram_pick);
            }
            if let Some(diagram_hover_text) = diagram_hover_text {
                *diagram_hover_text_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(diagram_hover_text);
            }
            if let Some(diagram_facet_tier) = diagram_facet_tier {
                *diagram_facet_tier_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(diagram_facet_tier);
            }

            ui.global::<SolidPreviewModel>()
                .set_image(slint::Image::from_rgba8(image));
            ui.global::<SolidPreviewModel>().set_has_solid(has_solid);
            ui.global::<SolidPreviewModel>().set_status(status.into());
            ui.global::<SolidPreviewModel>().set_stale(stale);
            if let Some(edges_image) = edges_image {
                ui.global::<SolidPreviewModel>()
                    .set_edges_image(slint::Image::from_rgba8(edges_image));
                ui.global::<SolidPreviewModel>().set_has_solid_edges(true);
            } else {
                ui.global::<SolidPreviewModel>().set_has_solid_edges(false);
            }
            if let Some(diagram_image) = diagram_image {
                ui.global::<SolidPreviewModel>()
                    .set_diagram_image(slint::Image::from_rgba8(diagram_image));
            }
            ui.global::<SolidPreviewModel>()
                .set_has_diagram(has_diagram);
        });
    }
}

/// Builds and fully wires a `MainWindow` -- everything [`run_gui`] used to do
/// inline, up to but not including actually running the event loop. See
/// [`MainWindowHandle`]'s doc comment for why this is split out.
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

    // Editor behind a feature flag: whether this build was compiled with the `editor`
    // feature -- Rust-driven, set once here, same treatment `gpu_build` above already
    // gets. `app.slint`'s "Edit" sub-tab (button and body both) is gated on this
    // property rather than the feature itself, since Slint markup has no `#[cfg]` --
    // see this crate's `Cargo.toml` `editor` feature comment for the full asymmetry
    // (the markup always compiles in; this flag keeps it unreachable at runtime when
    // the feature is off).
    ui.global::<EditorModel>()
        .set_enabled(cfg!(feature = "editor"));

    // Connect to SQLite DB. If the on-disk database can't be opened (locked, corrupt,
    // unwritable directory, ...), fall back to a throwaway in-memory database rather
    // than panicking at startup -- `Database::new` runs the exact same
    // create-tables-if-not-exist + migration chain against `:memory:` as it does
    // against a real file (see `Database::new`'s own doc comment), so the fallback
    // yields a fully-functional, just non-persistent, library. The user is told via a
    // persistent error toast (never auto-dismisses -- see `show_toast`'s doc comment)
    // rather than the old status-bar message, since that message was set right before
    // an `unwrap()` that could panic before it was ever seen.
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
    // alongside `render_ctx` above, regardless of the `editor` feature -- unlike
    // `gui::editor`, this module has no `indicatrix-cut-core` dependency to make
    // optional, and `setup_camera_and_lighting_callbacks` below needs it
    // unconditionally (the Live Render viewport's own orbit/zoom drag also re-issues
    // the last solid planes at the new pose). Its worker thread is spawned lazily on
    // the first `request_redraw`, so a build that never opens the Edit tab's Solid
    // view never pays for one.
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
    let diagram_hover_text: DiagramHoverText = Arc::new(Mutex::new(None));
    let diagram_facet_tier: DiagramFacetTier = Arc::new(Mutex::new(None));
    let solid_preview_state = SolidPreviewState::new(Arc::new(SlintSolidSink {
        ui: ui.as_weak(),
        pick: Arc::clone(&solid_pick),
        last_solved: Arc::clone(&solid_last_solved),
        diagram_pick: Arc::clone(&diagram_pick),
        diagram_hover_text: Arc::clone(&diagram_hover_text),
        diagram_facet_tier: Arc::clone(&diagram_facet_tier),
    }));
    // Diagram mode's own hover/click callbacks -- see
    // `solid_preview::diagram_wiring`'s own module doc comment for why this needs
    // no `gui::editor`/`Design` access, unlike the ordinary Solid view's hover/
    // click callbacks (`gui::editor::callbacks::tier_actions::
    // setup_solid_facet_hover_callback`/`setup_solid_facet_click_callback`).
    diagram_wiring::setup_diagram_hover_and_click_callbacks(
        &ui,
        &diagram_pick,
        &diagram_hover_text,
        &diagram_facet_tier,
    );

    // Which library (local database, or a remote worker) the viewer is
    // currently browsing. Starts `LibrarySource::Local` -- see that type's own doc
    // comment on why every local-only call site behaves exactly as it did before this
    // existed until a user actually switches.
    let library_source: Arc<Mutex<LibrarySource>> = Arc::new(Mutex::new(LibrarySource::default()));

    // Load custom gemstone materials from SQLite. `indicatrix-vault` returns plain
    // `CustomMaterialRow`s (it must not depend on `indicatrix`), so convert them into
    // `indicatrix::optics::materials::GemMaterial` here at the boundary.
    let initial_custom_mats: Vec<GemMaterial> = db
        .lock()
        .unwrap()
        .get_custom_materials()
        .unwrap_or_default()
        .iter()
        .map(gem_material_from_row)
        .collect();
    render_ctx.lock().unwrap().custom_materials = Arc::new(initial_custom_mats.clone());
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
    refresh_lighting_preset_options(&ui, &loaded_settings.presets);
    // Remote rendering: the worker-list panel's rows, restored from the last saved
    // configuration.
    refresh_worker_options(&ui, &loaded_settings.settings.remote_workers);
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(loaded_settings.settings.denoise_enabled);
    let settings_store = Arc::new(SettingsPersister::spawn(settings_path, loaded_settings));

    // Detachable Live Render window: sub-tab/detach-toggle wiring, plus the shared
    // frame-routing target the render thread's own frame-push closure below also
    // mirrors into -- see `detached_render`'s module doc comment.
    let detached_frame_target =
        render::detached_render::setup_live_render_visibility_callbacks(&ui, &render_ctx);

    // Spawn Background Multi-Threaded Physically Based Spectral Raytracer
    let ui_weak_render = ui.as_weak();
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
    );

    // Persist the library panel's collapsed/expanded state -- same "debounced
    // background writer" persistence every other durable setting in this file already
    // goes through, just a one-off `bool` with no dedicated `gui::*` module of its own.
    let settings_store_panel = settings_store.clone();
    ui.global::<LibraryModel>()
        .on_panel_collapsed_changed(move |collapsed: bool| {
            settings_store_panel.update(|s| s.settings.library_panel_collapsed = collapsed);
        });

    setup_user_manual_callback(&ui);
    render::camera_lighting::setup_camera_and_lighting_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
        &solid_preview_state,
    );
    render::material_quality::setup_material_changed_callback(&ui, &render_ctx, &settings_store);
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
    tilt::tilt_profile::setup_tilt_profile_callback(&ui, &render_ctx);
    tilt::tilt_hover_preview::setup_tilt_hover_preview_callback(&ui, &render_ctx);
    library::diagram_list::load_filter_options_and_initial_list(&ui, &db);
    library::diagram_list::setup_search_and_filter_callbacks(&ui, &db, &library_source);
    library::diagram_list::setup_diagram_selection_and_export_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
    );
    // Tilt-performance filter rows (add/remove/clear-all) -- Rust-mediated but
    // UI-owned state.
    library::diagram_list::setup_performance_filter_callbacks(&ui);
    library::local::setup_import_callback(&ui, &db, &library_source);
    library::local::setup_rename_callback(&ui, &db, &library_source);
    library::local::setup_set_shape_callback(&ui, &db, &library_source);
    library::local::setup_delete_callback(&ui, &db, &library_source, &render_ctx);
    library::local::setup_export_asc_callback(&ui, &db, &library_source);
    // Per-design ignore/un-ignore toggle -- grouped with the other library CRUD
    // callbacks above since it's the same shape (local-only write, then
    // `refresh_after_library_change`).
    library::local::setup_ignore_toggle_callback(&ui, &db, &library_source);
    library::detail::setup_save_metadata_callback(&ui, &db, &library_source, &render_ctx);
    // The Edit sub-tab's own callbacks (tier add/remove/modify, preform controls,
    // undo/redo, loading the selected design, `.asc` export). Feature-gated exactly
    // like `editor::setup_editor_callbacks`'s module declaration above --
    // `MainWindow.editor_enabled` already keeps the tab itself unreachable at runtime
    // without this feature, but wiring these callbacks at all would require linking
    // `indicatrix-cut-core`, which is the dependency this feature exists to make optional.
    #[cfg(feature = "editor")]
    editor::setup_editor_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &solid_preview_state,
        &solid_last_solved,
        &solid_pick,
    );
    // Persist the Solid viewport's view mode through the same debounced writer every
    // other durable setting in this file goes through -- see
    // `AppSettings::solid_view_mode`'s own doc comment.
    let settings_store_solid_view_mode = settings_store.clone();
    ui.global::<SolidPreviewModel>()
        .on_view_mode_changed(move |mode: i32| {
            settings_store_solid_view_mode.update(|s| {
                s.settings.solid_view_mode = u8::try_from(mode).unwrap_or_default();
            });
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
    let render_ctx_close = render_ctx.clone();
    let settings_store_close = settings_store.clone();
    ui.window().on_close_requested(move || {
        render_ctx_close.lock().unwrap().running = false;
        // Bypasses the debounce window -- a change made in the last `DEBOUNCE`
        // interval before quitting must not be silently lost.
        settings_store_close.flush();
        slint::CloseRequestResponse::HideWindow
    });

    Ok(MainWindowHandle {
        ui,
        render_ctx,
        settings_store,
        remote_rendering_timer,
    })
}

/// Resolves the starting directory for a native `rfd` file/folder picker from a path
/// text field's current value: the field's own value if it already names an existing
/// directory, that path's parent if it names an existing file, otherwise `None`
/// (callers then leave `rfd`'s directory unset). Shared by every picker call site that
/// still fills a text field this way (`camera_lighting`'s HDR environment-map path,
/// `remote::worker_callbacks`'s certificate-bundle-folder field) so "what counts as a
/// path already in the field" stays one rule.
fn starting_dir_from_picker_field(current: &str) -> Option<std::path::PathBuf> {
    if current.is_empty() {
        return None;
    }
    let path = std::path::Path::new(current);
    if path.is_dir() {
        Some(path.to_path_buf())
    } else if path.is_file() {
        path.parent().map(std::path::Path::to_path_buf)
    } else {
        None
    }
}

/// Candidate `docs/manual/README.md` locations for [`locate_user_manual`], relative to
/// a base directory (the running executable's own directory in production; see that
/// function for how each candidate is built from it).
///
/// Order matters: earlier candidates are preferred, and the first one that exists on
/// disk wins. The first three match how the manual is actually laid out relative to
/// the installed/built binary (bundled alongside it, one level up, or in the `cargo
/// build` target-directory layout); the last is a dev-only fallback for running
/// straight out of a `cargo run` checkout, where none of those relative layouts apply.
fn user_manual_candidates(exe_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![
        exe_dir.join("docs/manual/README.md"),
        exe_dir.join("../docs/manual/README.md"),
        // `cargo build` layout: `<target>/<profile>/<exe>` -> the crate root is two
        // directories up from the executable's directory.
        exe_dir.join("../../apps/indicatrix-cut/docs/manual/README.md"),
        // Dev fallback: running via `cargo run` from within this crate's own checkout,
        // where the exe-relative candidates above don't line up with the source tree.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md"),
    ]
}

/// Resolves the bundled user manual's path at runtime: tries each of
/// [`user_manual_candidates`], relative to the running executable's own directory, in
/// order, and returns the first one that exists on disk. `None` if none of them do
/// (e.g. a build that never bundled the manual at all).
///
/// Runtime-resolved rather than the `CARGO_MANIFEST_DIR` compile-time path this used
/// to be: that path is baked in at compile time and only ever resolves correctly for a
/// build run from within this crate's own checkout, never for a distributed binary.
fn locate_user_manual() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))?;
    user_manual_candidates(&exe_dir)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Help menu: opens the bundled `docs/manual/README.md` via the same per-platform
/// `xdg-open`/`cmd /C start`/`open` dispatch
/// `library::diagram_list::setup_diagram_selection_and_export_callbacks`'s
/// `on_open_diagram_url` uses for URLs (all three also open a plain file path). The
/// path itself is resolved at runtime by [`locate_user_manual`] (see its doc comment
/// for the candidate search order). If none of those candidates exist on disk, this
/// shows an error toast instead of silently doing nothing.
fn setup_user_manual_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.on_open_user_manual(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some(manual_path) = locate_user_manual() else {
            show_toast(&ui, "User manual not found", "error");
            return;
        };
        let path_str = manual_path.to_string_lossy().into_owned();
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open")
            .arg(&path_str)
            .spawn();
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path_str])
            .spawn();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&path_str).spawn();
    });
}

/// Shows a toast message.
///
/// Informational/success toasts auto-dismiss after 3.5s; errors stay on screen
/// until the user dismisses them (via `Toast.dismiss` -> `root.toast_visible =
/// false` in `app.slint`), since an error the user needed to act on could otherwise
/// disappear before they read it -- see `docs/history/indicatrix-cut.md` for the
/// review this asymmetry came out of.
///
/// A plain function (not a closure) since it captures nothing from `run_gui` --
/// every `ui.on_X` callback across this module's submodules that needs it just calls
/// it directly on its own `ui: &MainWindow`. `pub` because a downstream binary
/// reusing this window needs to surface its own results through the same toast.
pub fn show_toast(ui: &MainWindow, msg: &str, toast_type: &str) {
    ui.set_toast_message(msg.into());
    ui.set_toast_type(toast_type.into());
    ui.set_toast_visible(true);

    // Every toast bumps this counter; a scheduled dismiss below only acts if it's
    // still THIS call's generation by the time it fires -- otherwise a second toast
    // shown while the first one's timer is still running would get hidden early by
    // that stale timer (see `MainWindow.toast_generation`'s own doc comment).
    let generation = ui.get_toast_generation() + 1;
    ui.set_toast_generation(generation);

    // Errors stay until the user dismisses them -- see this function's own doc
    // comment.
    if toast_type == "error" {
        return;
    }

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(3500), move || {
        if let Some(ui) = ui_weak.upgrade()
            && ui.get_toast_generation() == generation
        {
            ui.set_toast_visible(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::user_manual_candidates;

    /// The candidate list is built entirely from the given base directory (plus one
    /// fixed dev-only `CARGO_MANIFEST_DIR` fallback) -- no hidden dependence on the
    /// current working directory or environment. Also pins down the order (exe dir,
    /// one level up, two levels up under the `cargo build` layout, then the dev
    /// fallback) since [`locate_user_manual`](super::locate_user_manual) relies on
    /// earlier candidates being preferred.
    #[test]
    fn user_manual_candidates_are_relative_to_exe_dir_in_order() {
        let exe_dir = std::path::Path::new("/opt/indicatrix-cut/bin");
        let candidates = user_manual_candidates(exe_dir);

        assert_eq!(
            candidates[0],
            std::path::Path::new("/opt/indicatrix-cut/bin/docs/manual/README.md")
        );
        assert_eq!(
            candidates[1],
            std::path::Path::new("/opt/indicatrix-cut/bin/../docs/manual/README.md")
        );
        assert_eq!(
            candidates[2],
            std::path::Path::new(
                "/opt/indicatrix-cut/bin/../../apps/indicatrix-cut/docs/manual/README.md"
            )
        );
        // Last candidate is the fixed dev-only fallback, independent of `exe_dir`.
        assert_eq!(
            candidates[3],
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md")
        );
        assert_eq!(candidates.len(), 4);
    }
}

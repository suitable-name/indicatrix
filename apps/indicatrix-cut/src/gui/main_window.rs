//! Building and fully wiring a `MainWindow`, and the RAII handle that keeps its
//! render thread/settings-autosave/remote-rendering polling alive.

mod callbacks;
mod state;

use crate::{
    EditorModel, MainWindow, SettingsModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::render,
    settings::SettingsPersister,
};
use callbacks::{
    setup_editor, setup_library_callbacks, setup_live_view_mode_callbacks, setup_remote_and_batch,
    setup_render_callbacks, setup_solid_view_mode_callbacks, setup_window_close,
};
use slint::ComponentHandle;
use state::{
    apply_custom_materials, create_solid_view, load_settings, open_design_library,
    spawn_viewport_render_thread,
};
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

    let db = open_design_library(&ui)?;

    // Shared Render Context for 3D Viewport
    let render_ctx = Arc::new(Mutex::new(RenderContext::default()));

    let solid = create_solid_view(&ui, &render_ctx);

    // Which library (local database, or a remote worker) the viewer is
    // currently browsing. Starts `LibrarySource::Local` -- see that type's own doc
    // comment on why every local-only call site keeps behaving as plain local-only
    // until a user actually switches.
    let library_source: Arc<Mutex<LibrarySource>> = Arc::new(Mutex::new(LibrarySource::default()));

    apply_custom_materials(&ui, &db, &render_ctx);
    let settings_store = load_settings(&ui, &render_ctx);

    // Live Render visibility: outer tab / sub-tab change wiring -- see
    // `render_visibility`'s module doc comment.
    render::render_visibility::setup_live_render_visibility_callbacks(
        &ui,
        &render_ctx,
        &solid.preview_state,
    );
    // The settings load above (`apply_loaded_settings`/`apply_loaded_ui_mirrors`)
    // already pushed the restored `ViewportModel.live_view_mode`/`SolidPreviewModel.
    // view_mode` into the UI, but nothing had recomputed `render_ctx.tab_visible`
    // against them yet -- it was still sitting at `RenderContext::default()`'s `true`.
    // Without this, a session restored into Solid mode would keep tracing (wasting
    // GPU/remote time, the very thing this gate exists to stop) until the user
    // happened to touch a tab. One-time sync, now that every signal
    // `render_visibility::render_is_visible` reads is in place.
    render::render_visibility::recompute_tab_visible(&ui, &render_ctx);

    spawn_viewport_render_thread(&ui, &render_ctx, &solid);

    setup_render_callbacks(&ui, &db, &render_ctx, &settings_store, &solid);
    setup_library_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &settings_store,
        &solid.preview_state,
    );
    setup_editor(&ui, &db, &library_source, &render_ctx, &solid);
    // The lighting a design remembers for itself (settings dialog buttons + the hook the
    // editor calls when a design opens, see `render::design_lighting`).
    render::design_lighting::setup_design_lighting_callbacks(
        &ui,
        &db,
        &render_ctx,
        &settings_store,
    );
    setup_solid_view_mode_callbacks(&ui, &render_ctx, &settings_store, &solid.preview_state);
    setup_live_view_mode_callbacks(&ui, &render_ctx, &settings_store, &solid.preview_state);
    let remote_rendering_timer =
        setup_remote_and_batch(&ui, &db, &library_source, &render_ctx, &settings_store);
    setup_window_close(&ui, &render_ctx, &settings_store);

    // The app always opens on the solid renderer view: the 3D Gem tab's Edit
    // sub-tab in Solid view mode (`startup_settings` forces the view mode to 0).
    // Goes through the same path as a user's click on the pill (set the property,
    // then fire `render_view_tab_changed`), and runs only now that every handler
    // is connected, so the visibility gate (`recompute_tab_visible`) keeps the
    // live tracer idle and `edit_view_entered` reclaims the viewport for the
    // editor. `EditorModel.enabled` is set unconditionally above, so there is no
    // "enabled later" case to wait for.
    crate::gui::commands::show_edit_view(&ui);

    Ok(MainWindowHandle {
        ui,
        render_ctx,
        settings_store,
        remote_rendering_timer,
    })
}

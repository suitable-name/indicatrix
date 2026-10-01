//! The window's callback wiring: layout persistence, render, optics, tilt, library, editor,
//! view-mode, remote-rendering and window-close callbacks.

use super::state::SolidView;
use crate::{
    EditorModel, LibraryModel, MainWindow, SolidPreviewModel, ViewportModel,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        batch, editor, editor_layout,
        external_links::{setup_reveal_last_saved_callback, setup_user_manual_callback},
        library, optics,
        remote::{setup_remote_rendering, setup_worker_callbacks},
        render, rough_plan,
        solid_preview::preview_state::{SolidPickState, SolidPreviewState},
        tilt,
        window_close::setup_close_confirm_callbacks,
    },
    settings::SettingsPersister,
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Wires the persisted-layout, render, optics and tilt callbacks.
pub(super) fn setup_render_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    solid: &SolidView,
) {
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
    editor_layout::setup_editor_layout_callbacks(ui, settings_store);

    setup_user_manual_callback(ui);
    setup_reveal_last_saved_callback(ui);
    render::camera_lighting::setup_camera_and_lighting_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.preview_state,
        &solid.mesh_bounding_radius,
    );
    render::camera_lighting::setup_camera_drag_callbacks(ui, render_ctx);
    render::material_quality::setup_material_changed_callback(ui, render_ctx, settings_store);
    render::material_quality::setup_backdrop_callback(ui, render_ctx, settings_store);
    render::material_quality::setup_material_and_quality_callbacks(ui, render_ctx, settings_store);
    render::material_quality::setup_material_effect_override_callbacks(
        ui,
        render_ctx,
        settings_store,
    );
    render::camera_lighting::setup_environment_map_callbacks(ui, render_ctx, settings_store);
    render::lighting_presets::setup_lighting_preset_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.mesh_bounding_radius,
    );
    render::render_export::setup_render_export_callbacks(
        ui,
        render_ctx,
        settings_store,
        &solid.mesh_bounding_radius,
    );
    optics::custom_materials::setup_custom_material_callbacks(ui, render_ctx, db);
    library::clipboard::setup_copy_callbacks(ui);
    tilt::tilt_profile::setup_tilt_profile_callback(ui, render_ctx, settings_store);
    tilt::tilt_hover_preview::setup_tilt_hover_preview_callback(ui, render_ctx);
}

/// Wires the library list, search, import, rename, delete, export and detail callbacks.
pub(super) fn setup_library_callbacks(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    library::diagram_list::load_filter_options_and_initial_list(ui, db);
    library::diagram_list::setup_search_and_filter_callbacks(ui, db, library_source);
    // The owner's "regenerate library previews/tilt curves for the filtered set"
    // request -- see `setup_regenerate_filtered_set_callbacks`'s own doc comment.
    library::diagram_list::setup_regenerate_filtered_set_callbacks(ui, db, library_source);
    // The Library menu's whole-catalogue counterparts.
    batch::regenerate_all::setup_regenerate_all_callbacks(ui, db, library_source);
    // The Library menu's "Plan Rough..." dialog.
    rough_plan::setup_rough_plan_callbacks(ui, db, library_source);
    library::diagram_list::setup_diagram_selection_and_export_callbacks(
        ui,
        db,
        library_source,
        render_ctx,
        preview_state,
    );
    // Tilt-performance filter rows (add/remove/clear-all) -- Rust-mediated but
    // UI-owned state.
    library::diagram_list::setup_performance_filter_callbacks(ui);
    library::local::setup_import_callback(ui, db, library_source, settings_store);
    library::local::setup_rename_callback(ui, db, library_source);
    library::local::setup_set_shape_callback(ui, db, library_source);
    library::local::setup_delete_callback(ui, db, library_source, render_ctx);
    library::local::setup_export_asc_callback(ui, db, library_source);
    // Per-design ignore/un-ignore toggle -- grouped with the other library CRUD
    // callbacks above since it's the same shape (local-only write, then
    // `refresh_after_library_change`).
    library::local::setup_ignore_toggle_callback(ui, db, library_source);
    // Per-design "Exclude from planner" toggle: a local-only write that keeps one
    // design out of the Rough Planner's candidates without hiding it from the library.
    library::local::setup_planner_exclusion_toggle_callback(ui, db, library_source);
    library::local::setup_add_tag_callback(ui, db, library_source);
    library::local::setup_remove_tag_callback(ui, db, library_source);
    library::detail::setup_save_metadata_callback(ui, db, library_source, render_ctx);
    // The cutting-table's per-mode row-count pills.
    library::detail::setup_filtered_row_count_callback(ui);
}

/// Wires the Edit sub-tab's callbacks against the Solid viewport's shared handles.
pub(super) fn setup_editor(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid: &SolidView,
) {
    // The Edit sub-tab's own callbacks (tier add/remove/modify, preform controls,
    // undo/redo, loading the selected design, `.asc` export).
    // Bundles the three Solid-viewport handles above into the one value
    // `editor::setup_editor_callbacks` takes for them -- see [`SolidPickState`]'s
    // own doc comment for why they travel together.
    let solid_pick_state = SolidPickState {
        pick: Arc::clone(&solid.pick),
        hover_text: Arc::clone(&solid.hover_text),
        facet_tier: Arc::clone(&solid.facet_tier),
        geometry: Arc::clone(&solid.geometry),
    };
    editor::setup_editor_callbacks(
        ui,
        db,
        library_source,
        render_ctx,
        &solid.preview_state,
        &solid.last_solved,
        &solid_pick_state,
    );
}

/// Persists the Solid viewport's view mode and redraws on a mode change or a viewport
/// resize.
pub(super) fn setup_solid_view_mode_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
    // Persist the Solid viewport's view mode through the same debounced writer every
    // other durable setting in this file goes through -- see
    // `AppSettings::solid_view_mode`'s own doc comment. Also recomputes
    // `render_ctx.tab_visible`: this mode is one of the signals
    // `render::render_visibility::render_is_visible` reads (the Edit tab traces live
    // only in Path-traced/Both), so flipping it can turn the tracer on or off.
    //
    // Also re-requests a redraw at the new mode via `resubmit_at_current_pose` --
    // otherwise switching TO Diagram or Both would never ask the worker for
    // anything, so both would stay on whatever `has_diagram`/`has_solid_edges` a
    // PREVIOUS mode happened to leave set (usually `false`, showing the "Solve to
    // preview" placeholder until the user jiggled the mouse or made an edit).
    let settings_store_solid_view_mode = settings_store.clone();
    let render_ctx_solid_view_mode = render_ctx.clone();
    let preview_state_solid_view_mode = Arc::clone(preview_state);
    let ui_weak_solid_view_mode = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_view_mode_changed(move |mode: i32| {
            settings_store_solid_view_mode.update(|s| {
                s.settings.solid_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            if let Some(ui) = ui_weak_solid_view_mode.upgrade() {
                render::render_visibility::recompute_tab_visible(&ui, &render_ctx_solid_view_mode);
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
    let preview_state_viewport_resized = Arc::clone(preview_state);
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
}

/// Persists the Live Render tab's view mode and the Edit sub-tab's auto-solve budget.
pub(super) fn setup_live_view_mode_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    preview_state: &Arc<SolidPreviewState>,
) {
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
    let preview_state_live_view_mode = Arc::clone(preview_state);
    let ui_weak_live_view_mode = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_live_view_mode_changed(move |mode: i32| {
            settings_store_live_view_mode.update(|s| {
                s.settings.live_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            let Some(ui) = ui_weak_live_view_mode.upgrade() else {
                return;
            };
            render::render_visibility::recompute_tab_visible(&ui, &render_ctx_live_view_mode);
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
}

/// Wires the remote library, the remote-rendering handoff and the preview/tilt batch
/// jobs. Returns the handoff poll timer, which the caller must keep alive.
pub(super) fn setup_remote_and_batch(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    library_source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) -> slint::Timer {
    library::remote::setup_library_source_callbacks(ui, db, library_source, settings_store);
    library::remote::setup_mirror_sync_callbacks(ui, db, settings_store);
    // Preview-then-handoff remote rendering: a repeating `slint::Timer` that polls
    // camera/light pose and drives the handoff -- must be kept alive for the life of
    // the window (a dropped `Timer` simply stops firing), hence the binding held all
    // the way to `ui.run()` below rather than being dropped immediately. Set up
    // BEFORE `setup_worker_callbacks`: the denoise toggle it wires needs the
    // `RemoteOrchestratorHandle` this also returns.
    let (remote_rendering_timer, remote_orchestrator) =
        setup_remote_rendering(ui, render_ctx, settings_store);
    setup_worker_callbacks(ui, render_ctx, settings_store, &remote_orchestrator);
    // Cached design-catalogue preview thumbnails (front/top renders) -- decoded and
    // cached off the UI thread (see `preview_cache`'s own doc comment), and the
    // background batch job that generates them (two-level progress, cancellation,
    // per-item panic isolation, local/remote dispatch over a shared work queue -- see
    // `preview_batch`'s own doc comment). The batch-callbacks setup also kicks off the
    // once-per-session "designs with no previews yet" scan/offer (trigger: library
    // scan) as part of its own wiring. The batch invalidates the thumbnail cache per
    // saved design, so the cache handle is passed on.
    let thumbnail_cache = batch::preview_cache::setup_preview_thumbnail_callback(ui, db);
    batch::preview::setup_preview_batch_callbacks(
        ui,
        db,
        render_ctx,
        settings_store,
        &thumbnail_cache,
    );
    // Tilt-curve batch computation -- same shape as the preview batch above,
    // deliberately never auto-scanned at startup (see `tilt_batch`'s own module doc
    // comment for why: the cost per catalogue is roughly an order of magnitude
    // higher).
    batch::tilt::setup_tilt_batch_callbacks(ui, db, render_ctx, settings_store);
    remote_rendering_timer
}

/// Wires the window-close handling: the dirty-design guard and the render thread's
/// shutdown.
pub(super) fn setup_window_close(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
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
    //
    // Closing the main window also closes the Rough Planner, so a clean design does not
    // close it while the planner holds results that were never saved or a running plan:
    // the same guard asks, without its Save button (there is nothing to save), and its
    // Discard closes everything.
    let render_ctx_close = render_ctx.clone();
    let settings_store_close = settings_store.clone();
    let ui_weak_close = ui.as_weak();
    ui.window().on_close_requested(move || {
        let is_dirty = ui_weak_close
            .upgrade()
            .is_some_and(|ui| ui.global::<EditorModel>().get_is_dirty());
        let planner_risk = rough_plan::planner_work_at_risk();
        if !is_dirty && planner_risk.is_none() {
            render_ctx_close
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .running = false;
            // Bypasses the debounce window -- a change made in the last `DEBOUNCE`
            // interval before quitting must not be silently lost.
            settings_store_close.flush();
            // A still-open compare window would otherwise keep the event loop
            // (and the process) running after the main window is gone.
            editor::close_compare_window();
            rough_plan::close_planner_window();
            return slint::CloseRequestResponse::HideWindow;
        }
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.set_close_confirm_message(
                close_confirm_message(is_dirty, planner_risk.as_deref()).into(),
            );
            ui.set_close_confirm_can_save(is_dirty);
            ui.set_close_confirm_open(true);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    setup_close_confirm_callbacks(ui, render_ctx, settings_store);
}

/// What the design-dirty message of the close guard says.
const DIRTY_DESIGN_MESSAGE: &str =
    "Closing the window will discard the current design's unsaved changes.";

/// The message of the close guard: the dirty design's sentence, the planner's sentence
/// (see `rough_plan::planner_work_at_risk`), or both, design first.
fn close_confirm_message(design_dirty: bool, planner_risk: Option<&str>) -> String {
    match (design_dirty, planner_risk) {
        (true, Some(risk)) => format!("{DIRTY_DESIGN_MESSAGE} {risk}"),
        (true, None) => DIRTY_DESIGN_MESSAGE.to_string(),
        (false, Some(risk)) => risk.to_string(),
        (false, None) => String::new(),
    }
}

#[cfg(test)]
mod close_message_tests {
    use super::*;

    #[test]
    fn the_guard_names_what_is_at_risk() {
        assert_eq!(
            close_confirm_message(true, None),
            "Closing the window will discard the current design's unsaved changes."
        );
        assert_eq!(
            close_confirm_message(false, Some("A rough plan is still running.")),
            "A rough plan is still running."
        );
        assert_eq!(
            close_confirm_message(true, Some("A rough plan is still running.")),
            "Closing the window will discard the current design's unsaved changes. A rough plan is still running."
        );
    }
}

//! The window-close unsaved-changes guard's Save/Discard callbacks.

use crate::{
    EditorModel, MainWindow, bridge::render_thread::RenderContext, settings::SettingsPersister,
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// `MainWindow.close_confirm_save`/`close_confirm_discard` -- the window-close
/// unsaved-changes guard's two ways past itself (`close_confirm_open`'s own doc
/// comment in `app.slint`; Cancel needs no Rust handler at all, see that same
/// comment). Both finish with the same `render_ctx.running = false` +
/// `settings_store.flush()` + hide sequence that `on_close_requested` runs
/// directly when the design is already clean (not dirty).
///
/// "Save" invokes `EditorModel.save_native` and only actually closes once that save
/// really lands -- via the [`crate::gui::editor::native_io::AfterSave::CloseWindow`]
/// listener registered below, not a synchronous `is_dirty` check run right after
/// `invoke_save_native` returns: Save is asynchronous end to end (the
/// design is resolved and the file written on a spawned thread), so that check used
/// to read the state from BEFORE the save even started, closing the window out from
/// under whatever the save was still writing, or leaving it open with no message when
/// the save had, in fact, already succeeded. A cancelled or failed save (already
/// toasted by `save_native` itself, and always clearing `after_save` -- see that
/// enum's own doc comment) simply never fires the listener, leaving the window open.
pub(super) fn setup_close_confirm_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // Runs once the save this guard's own "Save" triggered actually completes --
    // see [`crate::gui::editor::native_io::AfterSave::CloseWindow`]'s own doc
    // comment. A listener that finds `after_save` holding the OTHER guard's own
    // `Resume(..)` (or `None`) must leave it untouched -- see
    // `crate::gui::editor::native_io::on_save_completed`'s own doc comment; only
    // one listener may ever actually consume a given value.
    let render_ctx_listener = render_ctx.clone();
    let settings_store_listener = settings_store.clone();
    crate::gui::editor::native_io::on_save_completed(move |ui, state| {
        {
            let mut st = state.borrow_mut();
            match st.after_save.take() {
                Some(crate::gui::editor::native_io::AfterSave::CloseWindow) => {}
                // Not this guard's own save (or none pending) -- put it back
                // exactly as found; see this function's own doc comment for
                // why only one listener may ever consume a given value.
                other => {
                    st.after_save = other;
                    return;
                }
            }
        }
        render_ctx_listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
        settings_store_listener.flush();
        // See `main_window`'s `on_close_requested`: the compare window must not
        // outlive the main one.
        crate::gui::editor::close_compare_window();
        crate::gui::rough_plan::close_planner_window();
        let _ = ui.hide();
    });

    let ui_weak_save = ui.as_weak();
    ui.on_close_confirm_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        // Marks `after_save` so the listener above runs the actual close once
        // this save really lands -- see this function's own doc comment.
        crate::gui::editor::native_io::request_save_then_close();
        ui.global::<EditorModel>().invoke_save_native();
    });

    let render_ctx_discard = render_ctx.clone();
    let settings_store_discard = settings_store.clone();
    let ui_weak_discard = ui.as_weak();
    ui.on_close_confirm_discard(move || {
        let Some(ui) = ui_weak_discard.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        render_ctx_discard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
        settings_store_discard.flush();
        crate::gui::editor::close_compare_window();
        crate::gui::rough_plan::close_planner_window();
        let _ = ui.hide();
    });
}

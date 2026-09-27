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
/// "Save" invokes `EditorModel.save_native` and only proceeds to actually close once
/// that save left the design clean -- a cancelled or failed save (already toasted by
/// `save_native` itself) leaves the window open instead of closing out from under an
/// unsaved design anyway.
pub(super) fn setup_close_confirm_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.on_close_confirm_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        ui.global::<EditorModel>().invoke_save_native();
        if ui.global::<EditorModel>().get_is_dirty() {
            return;
        }
        render_ctx_save
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
        settings_store_save.flush();
        let _ = ui.hide();
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
        let _ = ui.hide();
    });
}

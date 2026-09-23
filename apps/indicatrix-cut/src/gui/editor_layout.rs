//! The Edit sub-tab's resizable-dock/collapsible-section layout: applying the
//! persisted values to `EditorModel` at startup ([`apply_editor_layout_from_settings`])
//! and writing them back on every change ([`setup_editor_layout_callbacks`]). Split out
//! of `gui::mod`/`gui::startup_settings` purely to keep this one layout-persistence
//! concern in its own file -- the same reasoning `window_sizing` and `startup_settings`
//! themselves already document.
//!
//! `EditorModel` itself is a plain Slint global, always compiled in.

use crate::{
    EditorModel, MainWindow,
    settings::{SettingsPersister, model::AppSettings},
};
use slint::ComponentHandle;
use std::sync::Arc;

/// Raised floor for the inspector's height while the user has never touched the Edit
/// sub-tab's layout -- taller than `AppSettings::DEFAULT_EDITOR_INSPECTOR_HEIGHT`
/// (260px) so the Tier tab's Save/Add Tier button row lands inside the `ScrollView`'s
/// initial fold instead of landing roughly 75px below it. Applied
/// in [`apply_editor_layout_from_settings`] only while `editor_layout_touched` is
/// `false`; once a user has ever dragged the table|inspector split themselves (or a
/// smaller screen has narrowed it, see `window_sizing`), their own value is trusted
/// verbatim and never raised.
const FIRST_RUN_INSPECTOR_HEIGHT: f32 = 340.0;

/// Applies the Edit sub-tab's persisted dock width / inspector height / section
/// collapsed-states to `EditorModel`. Called once at startup, right after
/// `startup_settings::apply_loaded_settings` and before
/// [`setup_editor_layout_callbacks`] wires `on_layout_changed` -- so this initial push
/// can never race a save of the very values it is restoring.
pub(super) fn apply_editor_layout_from_settings(ui: &MainWindow, s: &AppSettings) {
    let editor = ui.global::<EditorModel>();
    editor.set_dock_width(s.editor_dock_width);
    let inspector_height = if s.editor_layout_touched {
        s.editor_inspector_height
    } else {
        s.editor_inspector_height.max(FIRST_RUN_INSPECTOR_HEIGHT)
    };
    editor.set_inspector_height(inspector_height);
    editor.set_settings_collapsed(s.editor_settings_collapsed);
    editor.set_inspector_collapsed(s.editor_inspector_collapsed);
    editor.set_remap_collapsed(s.editor_remap_collapsed);
}

/// Wires `EditorModel.layout_changed` (fired by `ui/models/editor.slint` on every
/// change to `dock_width`/`inspector_height`/`settings_collapsed`/
/// `inspector_collapsed`/`remap_collapsed`) to persist all five through the same
/// debounced writer every other durable setting in `gui::mod::build_main_window` goes
/// through -- the same shape as that function's own `on_panel_collapsed_changed`.
///
/// Also marks `AppSettings::editor_layout_touched`, so `window_sizing`'s per-screen
/// inspector-collapse default never overrides a layout the user has ever adjusted
/// themselves -- once touched, it stays touched (nothing in this crate ever resets it
/// back to `false`).
pub(super) fn setup_editor_layout_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_layout_changed(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let editor = ui.global::<EditorModel>();
        let dock_width = editor.get_dock_width();
        let inspector_height = editor.get_inspector_height();
        let settings_collapsed = editor.get_settings_collapsed();
        let inspector_collapsed = editor.get_inspector_collapsed();
        let remap_collapsed = editor.get_remap_collapsed();
        settings_store.update(|s| {
            s.settings.editor_dock_width = dock_width;
            s.settings.editor_inspector_height = inspector_height;
            s.settings.editor_settings_collapsed = settings_collapsed;
            s.settings.editor_inspector_collapsed = inspector_collapsed;
            s.settings.editor_remap_collapsed = remap_collapsed;
            s.settings.editor_layout_touched = true;
        });
    });
}

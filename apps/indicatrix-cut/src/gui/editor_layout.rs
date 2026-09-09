//! The Edit sub-tab's resizable-dock/collapsible-section layout: applying the
//! persisted values to `EditorModel` at startup ([`apply_editor_layout_from_settings`])
//! and writing them back on every change ([`setup_editor_layout_callbacks`]). Split out
//! of `gui::mod`/`gui::startup_settings` purely to keep this one layout-persistence
//! concern in its own file -- the same reasoning `window_sizing` and `startup_settings`
//! themselves already document.
//!
//! Not behind the `editor` feature: `EditorModel` itself is a plain Slint global,
//! always compiled in regardless of whether `indicatrix-cut-core` is linked (see
//! `gui::mod::build_main_window`'s own `EditorModel.enabled` comment). Gating this
//! module would mean a non-`editor` build silently drops the dock-width/
//! inspector-height/collapsed-state settings on every save, corrupting them for a
//! later `editor`-enabled run against the same settings file.

use crate::{
    EditorModel, MainWindow,
    settings::{SettingsPersister, model::AppSettings},
};
use slint::ComponentHandle;
use std::sync::Arc;

/// Applies the Edit sub-tab's persisted dock width / inspector height / section
/// collapsed-states to `EditorModel`. Called once at startup, right after
/// `startup_settings::apply_loaded_settings` and before
/// [`setup_editor_layout_callbacks`] wires `on_layout_changed` -- so this initial push
/// can never race a save of the very values it is restoring.
pub(super) fn apply_editor_layout_from_settings(ui: &MainWindow, s: &AppSettings) {
    let editor = ui.global::<EditorModel>();
    editor.set_dock_width(s.editor_dock_width);
    editor.set_inspector_height(s.editor_inspector_height);
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

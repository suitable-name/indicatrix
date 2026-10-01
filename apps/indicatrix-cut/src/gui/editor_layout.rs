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
/// (340px) so the Tier tab's `ScrollView` fold holds the title plus the Angle, Meets,
/// Name and Indices rows: the Save/Add Tier row is pinned OUTSIDE that `ScrollView`, so
/// it costs the fold roughly 36px on top of the tab header and metrics strip. Applied
/// in [`apply_editor_layout_from_settings`] only while `editor_layout_touched` is
/// `false`; once a user has ever dragged the table|inspector split themselves (or a
/// smaller screen has narrowed it, see `window_sizing`), their own value is trusted
/// verbatim and never raised.
const FIRST_RUN_INSPECTOR_HEIGHT: f32 = 380.0;

/// Applies the Edit sub-tab's persisted dock width / inspector height / section
/// collapsed-states to `EditorModel`. Called once at startup, right after
/// `startup_settings::apply_loaded_settings` and before
/// [`setup_editor_layout_callbacks`] wires `on_layout_changed`.
///
/// This does NOT keep this initial push from racing a save of the very values it
/// is restoring -- it does, whenever [`FIRST_RUN_INSPECTOR_HEIGHT`] actually
/// raises `inspector_height` above the on-disk value (a fresh settings file):
/// the assignments below only QUEUE `EditorModel`'s own `changed` handlers, so the resulting
/// `EditorModel.layout_changed()` does not actually fire until the main window's
/// first `show()`/tree flush, indistinguishable, from `on_layout_changed`'s own
/// point of view, from the cutter dragging a split handle in the first instant
/// the window was visible. [`setup_editor_layout_callbacks`] guards against this
/// directly (see its own doc comment) rather than relying on call order here.
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
///
/// Skips this entirely the FIRST time `on_layout_changed` fires: that first firing
/// is always the flush of [`apply_editor_layout_from_settings`]'s own startup push
/// (see that function's own doc comment), never a real user action -- no user
/// input reaches this window before its first `show()`, and that flush is
/// guaranteed to happen at or before it. Without this, a from-disk
/// `editor_layout_touched: false` (a fresh install, or one that has never had its
/// layout touched) would flip permanently `true` on every single launch, purely
/// because [`FIRST_RUN_INSPECTOR_HEIGHT`] differs from the compiled default --
/// never recording a real user action, but persisting as if it had, and
/// permanently defeating `window_sizing`'s small-screen default from the second
/// launch on.
pub(super) fn setup_editor_layout_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    let startup_flush_pending = std::cell::Cell::new(true);
    ui.global::<EditorModel>().on_layout_changed(move || {
        if startup_flush_pending.replace(false) {
            return;
        }
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

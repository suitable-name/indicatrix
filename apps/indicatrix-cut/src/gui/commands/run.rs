//! What the palette commands do beyond calling one Slint callback.
//!
//! Each function here mirrors what a menu item, a button or a key arm in the `.slint` files
//! does, so a command and its button can never drift apart. Plain "invoke this callback"
//! commands are written inline in `table.rs`.
//!
//! Every function runs on the UI thread from the palette's deferred run, never from inside a
//! Slint handler, and none of them holds a borrow of the editor state.

use crate::{
    EditorModel, ExportModel, LibraryModel, MainWindow, ManipulateModel, PreferencesModel,
    SettingsModel, SolidPreviewModel, VariantsModel, ViewportModel,
};
use slint::ComponentHandle;

/// The 3D Spectral Preview tab.
pub const TAB_3D: i32 = 0;
/// The Cutting Instructions tab.
pub const TAB_CUTTING: i32 = 1;
/// The Files and Downloads tab.
pub const TAB_FILES: i32 = 2;

/// The 3D tab's Live Render view.
const VIEW_LIVE: i32 = 0;
/// The 3D tab's Edit view.
const VIEW_EDIT: i32 = 1;

/// Switches the window to `tab`, firing the same callback the tab pill does.
pub fn show_tab(ui: &MainWindow, tab: i32) {
    if ui.get_active_tab() != tab {
        ui.set_active_tab(tab);
        ui.invoke_active_tab_changed(tab);
    }
}

/// Switches the 3D tab to `view`, firing the same callback the view pill does.
fn show_view(ui: &MainWindow, view: i32) {
    show_tab(ui, TAB_3D);
    if ui.get_render_view_tab() != view {
        ui.set_render_view_tab(view);
        ui.invoke_render_view_tab_changed(view);
    }
}

/// Shows the Edit view of the 3D tab, where the tier table, inspector and viewport live.
pub fn show_edit_view(ui: &MainWindow) {
    show_view(ui, VIEW_EDIT);
}

/// Shows the Live Render view of the 3D tab.
pub fn show_live_render_view(ui: &MainWindow) {
    show_view(ui, VIEW_LIVE);
}

/// Opens the inspector's Tier tab ready to author a new tier.
pub fn add_tier(ui: &MainWindow) {
    show_edit_view(ui);
    let editor = ui.global::<EditorModel>();
    editor.set_selected_tier_index(-1);
    editor.set_inspector_tab(0);
    editor.set_inspector_collapsed(false);
    // The new tier appears in the table, so keep it in view as well.
    editor.set_tier_table_collapsed(false);
}

/// Shows inspector tab `tab` (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History), expanded.
pub fn show_inspector_tab(ui: &MainWindow, tab: i32) {
    show_edit_view(ui);
    let editor = ui.global::<EditorModel>();
    editor.set_inspector_tab(tab);
    editor.set_inspector_collapsed(false);
}

/// `EditorModel.inspector_tab` of the History tab, which hosts the Variants view.
const HISTORY_TAB: i32 = 4;

/// Shows the History tab's Variants view (the list of saved variants), expanded.
pub fn show_variants(ui: &MainWindow) {
    show_inspector_tab(ui, HISTORY_TAB);
    ui.global::<VariantsModel>().set_view(1);
}

/// Opens the form that saves the open design as a variant, like the Variants view's
/// "Save as variant..." button.
pub fn save_variant(ui: &MainWindow) {
    show_variants(ui);
    ui.global::<VariantsModel>().invoke_begin_save(-1);
}

/// Duplicates the selected tier, like the command bar's Duplicate button.
pub fn duplicate_selected_tier(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_duplicate_tier(index);
    }
}

/// Removes the selected tier, like the command bar's Delete button.
pub fn delete_selected_tier(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_remove_tier(index);
    }
}

/// Moves the selected tier by `step` places in cutting order (-1 earlier, 1 later).
pub fn move_selected_tier(ui: &MainWindow, step: i32) {
    let editor = ui.global::<EditorModel>();
    let index = editor.get_selected_tier_index();
    if index >= 0 {
        editor.invoke_move_tier(index, step);
    }
}

/// Clears the tier selection, like Escape does.
pub fn clear_tier_selection(ui: &MainWindow) {
    ui.global::<EditorModel>().set_selected_tier_index(-1);
}

/// Moves the keyboard focus to the tier filter box, like Ctrl+F on the Edit view.
pub fn focus_tier_filter(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    // A pulse counter: the filter box watches it change (see `EditorModel.filter_focus_pulse`).
    editor.set_filter_focus_pulse(editor.get_filter_focus_pulse().wrapping_add(1));
}

/// Starts Optimize with the weights currently typed into the Optimize tab, like its button.
pub fn optimize(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    editor.invoke_optimize(
        editor.get_optimize_weight_windowing(),
        editor.get_optimize_weight_extinction(),
        editor.get_optimize_weight_tilt_brilliance(),
        editor.get_optimize_weight_yield(),
        editor.get_optimize_weight_tone(),
    );
}

/// Sets the Edit view's viewport mode (0 Solid, 1 Path-traced, 2 Both, 3 Diagram).
pub fn set_solid_view_mode(ui: &MainWindow, mode: i32) {
    show_edit_view(ui);
    ui.global::<SolidPreviewModel>().set_view_mode(mode);
}

/// Sets the Live Render view's mode (0 Solid, 1 Path-traced).
pub fn set_live_view_mode(ui: &MainWindow, mode: i32) {
    show_live_render_view(ui);
    ui.global::<ViewportModel>().set_live_view_mode(mode);
}

/// Turns the Solid viewport's Slice tool on or off, like its pill and the S key.
pub fn toggle_slice_mode(ui: &MainWindow) {
    show_edit_view(ui);
    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_slice_mode(!manipulate.get_slice_mode());
    manipulate.invoke_slice_toggled();
}

/// Turns snapping of the angle and depth handles on or off, like the Snap pill.
pub fn toggle_snap(ui: &MainWindow) {
    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_snap_off(!manipulate.get_snap_off());
    manipulate.invoke_snap_toggled();
}

/// Collapses or expands the library panel, like its rail toggle.
pub fn toggle_library_panel(ui: &MainWindow) {
    let library = ui.global::<LibraryModel>();
    let collapsed = !library.get_panel_collapsed();
    library.set_panel_collapsed(collapsed);
    library.invoke_panel_collapsed_changed(collapsed);
}

/// Switches between the Simple and Advanced interface, like the header pill.
pub fn set_simple_interface(ui: &MainWindow, simple: bool) {
    let preferences = ui.global::<PreferencesModel>();
    preferences.set_simple_mode(simple);
    preferences.invoke_simple_mode_changed(simple);
}

/// Opens the Live Render view's render settings panel.
pub fn open_render_settings(ui: &MainWindow) {
    show_live_render_view(ui);
    ui.global::<SettingsModel>().set_is_open(true);
}

/// Opens the Live Render view's high-resolution export dialog.
pub fn open_render_export(ui: &MainWindow) {
    show_live_render_view(ui);
    ui.global::<ExportModel>().set_is_open(true);
}

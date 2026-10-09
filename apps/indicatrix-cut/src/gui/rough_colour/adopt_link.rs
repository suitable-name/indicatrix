//! What adopting a planned stone does to the editor (`zoning` feature only).
//!
//! The new "<rough> colour" material is put into the material list, selected, and the editor's
//! stone width is set to the stone's real width.
//!
//! The planner's adopt action (`gui::rough_plan::zoning_hooks`) writes the vault rows with
//! `preview::adopt` and then calls [`apply_to_editor`]. The render context belongs to the main
//! window's wiring, which registers it here when it builds the window
//! ([`register_render_context`]).

use super::store::AdoptOutcome;
use crate::{
    MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{show_toast, startup_settings::find_option_index},
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The main window's render context, once the window is built.
    static RENDER_CONTEXT: RefCell<Option<Arc<Mutex<RenderContext>>>> = const { RefCell::new(None) };
}

/// Remembers the main window's render context (called when the window is built).
pub fn register_render_context(render_ctx: &Arc<Mutex<RenderContext>>) {
    RENDER_CONTEXT.with(|cell| *cell.borrow_mut() = Some(Arc::clone(render_ctx)));
}

/// Makes the adopted stone the editor's material.
///
/// The custom-material list is rebuilt from the vault (so the new material and its zones are in
/// it), the render selects the material (like "Save and apply" in the material editor), the
/// material picker follows, and the stone width setting becomes the stone's real width, which
/// also persists it like the settings dialog does.
///
/// # Errors
///
/// A sentence when the editor is not ready (no render context registered).
pub fn apply_to_editor(
    main: &MainWindow,
    db: &Arc<Mutex<Database>>,
    outcome: &AdoptOutcome,
) -> Result<(), String> {
    let render_ctx = RENDER_CONTEXT
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| "The editor is not ready yet.".to_owned())?;
    crate::gui::main_window::reload_custom_materials(main, db, &render_ctx);
    {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        ctx.material_name.clone_from(&outcome.material_name);
        // `material_override` would win over the name just written; naming a real material by
        // hand is also the way out of a refusal (as in the material editor's apply).
        ctx.material_override = None;
        ctx.material_unresolved = None;
        ctx.dirty = true;
    }
    let viewport = main.global::<ViewportModel>();
    if viewport.get_viewport_material_linked() {
        viewport.set_viewport_material_linked(false);
    }
    if let Some(index) = find_option_index(&viewport.get_material_options(), &outcome.material_name)
    {
        viewport.set_selected_material_index(index);
    }
    // The settings dialog's own path: the handler sets the context's width, persists the setting
    // and refreshes the colour swatches. Called with the context unlocked.
    let width = outcome.stone_width_mm as f32;
    let settings = main.global::<SettingsModel>();
    settings.set_stone_width_mm(width);
    settings.invoke_stone_width_changed(width);
    show_toast(
        main,
        &format!(
            "Using '{}' at a stone width of {:.2} mm",
            outcome.material_name, outcome.stone_width_mm
        ),
        "success",
    );
    Ok(())
}

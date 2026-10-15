//! What adopting a planned stone does to the editor's render.
//!
//! The new "<plan> - <design> color" material is put into the material list and the render is
//! pointed at it ([`point_render_at`]); the design's material, stone width (girdle diameter) and
//! planned preform are then applied as ONE undoable design edit by
//! `gui::editor::planned_stone::apply_pending`, which hands the render context back through
//! [`restore`] if the design refuses the apply.
//!
//! The planner's two actions ("Use Color" and "Open in Editor", `gui::rough_plan`) write the vault
//! rows with `preview::adopt`, load the design they were computed for into the editor and only
//! then call these. The render context belongs to the main window's wiring, which registers it
//! here when it builds the window ([`register_render_context`]).

use super::store::AdoptOutcome;
use crate::{MainWindow, bridge::render_thread::RenderContext};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// The main window's render context, once the window is built.
    static RENDER_CONTEXT: RefCell<Option<Arc<Mutex<RenderContext>>>> = const { RefCell::new(None) };
}

/// Forgets the render context a previous main window registered. See `gui::window_release`.
pub fn release_window_state() {
    let old = RENDER_CONTEXT.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Remembers the main window's render context (called when the window is built).
pub fn register_render_context(render_ctx: &Arc<Mutex<RenderContext>>) {
    RENDER_CONTEXT.with(|cell| *cell.borrow_mut() = Some(Arc::clone(render_ctx)));
}

/// What the render context traced before [`point_render_at`] pointed it at an adopted material.
///
/// Handed back to [`restore`] when the design refuses the apply, so the picture keeps tracing the
/// material the design still has.
pub struct RenderPrevious {
    /// The context both calls act on.
    render_ctx: Arc<Mutex<RenderContext>>,
    /// The traced material's name.
    material_name: String,
    /// The design's own material, when the render followed one.
    material_override: Option<GemMaterial>,
    /// The note that the design names no usable material, when it did.
    material_unresolved: Option<String>,
}

/// Points the render at the adopted material, ready for the design to take it.
///
/// The custom-material list is rebuilt from the vault (so the new material and its zones are in
/// it) and the render context traces the material by name, like "Save and apply" in the material
/// editor. Returns what the context held before, for [`restore`]. It starts no preview and edits
/// no design: the caller applies the material, the width and the preform to the design on the
/// bench as one undoable edit.
///
/// # Errors
///
/// A sentence when the editor is not ready (no render context registered).
pub fn point_render_at(
    main: &MainWindow,
    db: &Arc<Mutex<Database>>,
    outcome: &AdoptOutcome,
) -> Result<RenderPrevious, String> {
    let render_ctx = RENDER_CONTEXT
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| "The editor is not ready yet.".to_owned())?;
    crate::gui::main_window::reload_custom_materials(main, db, &render_ctx);
    let previous = {
        let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
        // A same-named override is the design's copy of the adopted material; the reload above
        // put the fresh definition in the list, so capture that rather than the stale copy.
        let fresh_override = ctx.material_override.clone().map(|m| {
            if m.name.eq_ignore_ascii_case(&outcome.material_name) {
                ctx.custom_materials
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(&outcome.material_name))
                    .cloned()
                    .unwrap_or(m)
            } else {
                m
            }
        });
        let previous = RenderPrevious {
            render_ctx: Arc::clone(&render_ctx),
            material_name: ctx.material_name.clone(),
            material_override: fresh_override,
            material_unresolved: ctx.material_unresolved.clone(),
        };
        ctx.material_name.clone_from(&outcome.material_name);
        // `material_override` would win over the name just written; naming a real material by
        // hand is also the way out of a refusal (as in the material editor's apply).
        ctx.material_override = None;
        ctx.material_unresolved = None;
        ctx.dirty = true;
        previous
    };
    Ok(previous)
}

/// Puts the render context back as [`point_render_at`] found it.
///
/// The design refused the apply (it was busy), so reporting the adopted material now would be false, and the picture must
/// keep tracing the material the design still has.
pub fn restore(previous: RenderPrevious) {
    let RenderPrevious {
        render_ctx,
        material_name,
        material_override,
        material_unresolved,
    } = previous;
    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
    ctx.material_name = material_name;
    ctx.material_override = material_override;
    ctx.material_unresolved = material_unresolved;
    ctx.dirty = true;
}

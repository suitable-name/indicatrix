//! The leave-the-page guard: while the design has unsaved changes, closing or reloading
//! the tab makes the browser ask first (`beforeunload`).
//!
//! The session's own copy in `sessionStorage` (`super::persist`) survives a reload, but it
//! is gone when the tab closes, and a download is the only copy that outlives the tab -- so
//! the guard asks whenever the design is dirty, not only when closing.

use super::{Ctx, diagnostics::console_warn};
use wasm_bindgen::{JsCast, closure::Closure};

/// Whether the design has changes no save has captured. `false` when the state happens to
/// be borrowed (the event then asks nothing rather than panicking).
fn has_unsaved_changes(ctx: &Ctx) -> bool {
    ctx.state
        .try_borrow()
        .is_ok_and(|app| app.design.as_ref().is_some_and(|d| d.session.is_dirty()))
}

/// Installs the `beforeunload` listener once, for the page's lifetime (the closure is
/// leaked with `forget()`).
pub fn install(ctx: &Ctx) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let ctx = ctx.clone();
    let on_unload = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if !has_unsaved_changes(&ctx) {
            return;
        }
        // Standard browsers ask on `preventDefault`; older ones need a `returnValue`.
        event.prevent_default();
        if let Some(event) = event.dyn_ref::<web_sys::BeforeUnloadEvent>() {
            event.set_return_value("This design has unsaved changes.");
        }
    });
    if window
        .add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref())
        .is_err()
    {
        console_warn("could not install the unsaved-changes guard");
    }
    on_unload.forget();
}

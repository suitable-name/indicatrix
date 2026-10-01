//! The app shell: the wasm entry point, the shared [`Ctx`] every callback holds,
//! and the submodules that hold state, refresh the UI and persist it.
//!
//! # Single-threaded by construction
//!
//! `wasm32-unknown-unknown` (no `atomics`) has one thread: every callback and every
//! `.await` runs on it, so `Rc<RefCell<..>>` is the right container. The one rule
//! that still matters: never hold a `RefCell` borrow across an `.await` or across
//! anything that pumps the Slint event loop -- the `push_*` functions take `&WebApp`
//! and are called after the mutation's borrow ends.
//!
//! # "Using exceptions for control flow"
//!
//! winit's web backend prints this notice once at start-up on every Slint web app:
//! `EventLoop::run` never returns on the web, and winit unwinds out of it with a
//! JS exception on purpose. It is expected, not an error.

pub mod callbacks;
pub mod diagnostics;
pub mod persist;
pub mod push;
pub mod settings;
pub mod solve;
pub mod state;
pub mod unload;

use crate::{AppWindow, GalleryModel, TemplateCard};
use slint::{ComponentHandle, Timer};
use state::WebApp;
use std::{cell::RefCell, rc::Rc, time::Duration};
use wasm_bindgen::prelude::*;

/// The timers the shell restarts rather than recreates (restarting the SAME
/// `Timer` is what makes a debounce debounce).
#[derive(Default)]
pub struct Timers {
    /// Hides an info/success toast after a few seconds.
    pub toast: Timer,
    /// Debounces `sessionStorage` writes (see [`persist::PERSIST_DEBOUNCE`]).
    pub persist: Timer,
    /// Debounces render-view resizes (see
    /// [`crate::render::viewport::RESIZE_DEBOUNCE`]).
    pub resize: Timer,
}

/// What every callback holds: the window (weakly), the state, the timers.
#[derive(Clone)]
pub struct Ctx {
    /// The window; `upgrade()` fails only once the page is going away.
    pub ui: slint::Weak<AppWindow>,
    /// The one [`WebApp`].
    pub state: Rc<RefCell<WebApp>>,
    /// Shared timers.
    pub timers: Rc<Timers>,
}

/// Milliseconds since the page's time origin as a [`Duration`] -- the monotonic
/// clock `EditorSession::apply_coalescing`/`nudge_angles` take for undo coalescing
/// (`std::time::Instant` panics on this target). Zero if `performance` is missing.
#[must_use]
pub fn coalesce_now() -> Duration {
    let ms = web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now());
    Duration::from_secs_f64((ms / 1000.0).max(0.0))
}

/// Fills the New Design gallery from `indicatrix_editor::templates`.
fn push_template_cards(ui: &AppWindow) {
    let cards: Vec<TemplateCard> = indicatrix_editor::templates::template_cards()
        .into_iter()
        .map(|card| TemplateCard {
            name: card.name.into(),
            shape: card.shape.into(),
            description: card.description.into(),
        })
        .collect();
    ui.global::<GalleryModel>()
        .set_templates(Rc::new(slint::VecModel::from(cards)).into());
}

/// The wasm entry point, run once when the module has instantiated.
///
/// # Errors
///
/// The window could not be created or its event loop could not start (no canvas
/// or WebGL context); surfaced to `console.error` by wasm-bindgen.
#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    diagnostics::install();

    let ui = AppWindow::new().map_err(|e| {
        diagnostics::show_fatal(&format!(
            "Indicatrix Web could not start: {e}. It needs a browser with WebGL2 enabled \
             (hardware acceleration on); try a current Chrome, Edge, Firefox or Safari."
        ));
        JsValue::from_str(&e.to_string())
    })?;
    let settings = persist::restore_settings();
    let mut web_app = WebApp::new(settings);
    web_app.auto_solve_budget_ms = persist::restore_auto_solve_budget();
    let ctx = Ctx {
        ui: ui.as_weak(),
        state: Rc::new(RefCell::new(web_app)),
        timers: Rc::new(Timers::default()),
    };

    push_template_cards(&ui);
    callbacks::wire(&ui, &ctx);
    crate::editor::wire(&ui, &ctx);
    crate::views::wire(&ui, &ctx);
    crate::render::wire(&ui, &ctx);
    crate::metrics::wire(&ui, &ctx);
    crate::io::drop::install_drop_target(&ctx);
    unload::install(&ctx);
    persist::restore_design(&ctx);
    push::push_all(&ui, &ctx.state.borrow());
    // After the design is back: the restored step's goal is judged against it.
    crate::editor::guide::restore(&ctx);
    solve::auto_solve(&ctx);

    // After `wire`: sizing the window fires `render-size-changed`, whose handler
    // must already be registered.
    crate::render::viewport::track_browser_viewport(&ui);

    ui.run().map_err(|e| {
        diagnostics::show_fatal(&format!("Indicatrix Web stopped: {e}. Reload the page."));
        JsValue::from_str(&e.to_string())
    })?;
    Ok(())
}

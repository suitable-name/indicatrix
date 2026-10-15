//! The Stone, Light and Checks tabs: their Slint callbacks (`wire`) and the state pushed
//! into `PropsModel` (`push`). The logic lives in `indicatrix_mobile_core::design_props`;
//! this module only converts between its plain structs and the Slint structs.
//!
//! `push` runs on every refresh of the Design page (selection, camera, a finished solve),
//! so it remembers what it pushed last and writes only what changed. A text field or a
//! slider is rewritten only when its *design value* changed, never because something else
//! did, so a refresh cannot overwrite what the user is typing or dragging. After an edit
//! that was refused (or normalized), [`finish`] forgets everything so the fields show the
//! design's values again.
//!
//! Children: `stone` (the Stone tab and its pickers), `light` (the Light tab), `hdr` (the
//! Light tab's HDR Map section), `checks` (the Checks tab, the fixes), `metrics` (the Checks
//! tab's Metrics section and the Tilt Curves sheet), `number` (the number keypad).

mod checks;
mod hdr;
mod light;
mod metrics;
mod number;
mod stone;

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::{
    app_state::AppState,
    design_props::{
        ChecksView, LightingView, PreformField, StoneView, set_preform_field,
        set_refractive_index_override, set_stone_width,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Shared, after_edit, refresh};
use crate::{AppWindow, PropsModel, push::toast};

/// What the last [`push`] wrote, per surface; `None` means "write everything next time".
struct Pushed {
    stone: Option<StoneView>,
    lighting: Option<LightingView>,
    checks: Option<ChecksView>,
    /// What the material picker's rows were built for: the search text, the design's
    /// material and the number of custom materials.
    materials_key: Option<(String, Option<String>, usize)>,
    /// The Metrics section and the Tilt Curves sheet.
    analysis: metrics::AnalysisPushed,
}

impl Pushed {
    /// Nothing pushed yet.
    const EMPTY: Self = Self {
        stone: None,
        lighting: None,
        checks: None,
        materials_key: None,
        analysis: metrics::AnalysisPushed::EMPTY,
    };
}

thread_local! {
    /// The app state, for work that comes back to the UI thread from a worker (a fix).
    static HANDLE: RefCell<Option<Shared>> = const { RefCell::new(None) };
    /// The last pushed values.
    static PUSHED: RefCell<Pushed> = const { RefCell::new(Pushed::EMPTY) };
}

/// Runs `edit` on the remembered last-pushed values.
fn with_pushed<R>(edit: impl FnOnce(&mut Pushed) -> R) -> R {
    PUSHED.with(|pushed| edit(&mut pushed.borrow_mut()))
}

/// Forgets everything pushed, so the next push writes every field again.
fn invalidate() {
    with_pushed(|pushed| *pushed = Pushed::EMPTY);
}

/// A callback handler without arguments that runs `run` with the window and the state.
fn bind0(window: &AppWindow, state: &Shared, run: fn(&AppWindow, &Shared)) -> impl Fn() + 'static {
    let (state, weak) = (Rc::clone(state), window.as_weak());
    move || {
        if let Some(window) = weak.upgrade() {
            run(&window, &state);
        }
    }
}

/// A callback handler with one argument that runs `run` with the window, the state and
/// the argument.
fn bind1<A: 'static>(
    window: &AppWindow,
    state: &Shared,
    run: fn(&AppWindow, &Shared, A),
) -> impl Fn(A) + 'static {
    let (state, weak) = (Rc::clone(state), window.as_weak());
    move |argument| {
        if let Some(window) = weak.upgrade() {
            run(&window, &state, argument);
        }
    }
}

/// A Slint string array from `items`.
fn string_model(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// What to do after a call that may have edited the design: an edit re-solves and
/// refreshes everything, a no-op or a refusal restores the fields from the design (a
/// refusal also says why).
fn finish(window: &AppWindow, state: &Shared, result: Result<bool, String>) {
    match result {
        Ok(true) => after_edit(window, state),
        Ok(false) => {
            invalidate();
            refresh(window, state);
        }
        Err(message) => {
            toast::show(window, &message);
            invalidate();
            refresh(window, state);
        }
    }
}

/// Forgets the state (see `design::shutdown`).
pub fn forget() {
    // The analysis thread must not call back into a window that is going away.
    if let Some(state) = HANDLE.with(|handle| handle.borrow_mut().take()) {
        metrics::unwire(&state);
    }
}

/// Connects the three tabs' callbacks.
pub fn wire(window: &AppWindow, state: &Shared) {
    HANDLE.with(|handle| *handle.borrow_mut() = Some(Rc::clone(state)));
    stone::wire(window, state);
    light::wire(window, state);
    hdr::wire(window, state);
    checks::wire(window, state);
    metrics::wire(window, state);
    number::wire(window);

    window.global::<PropsModel>().on_field_committed({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |id, text| {
            if let Some(window) = weak.upgrade() {
                field_committed(&window, &state, &id, &text);
            }
        }
    });
}

/// A number field was committed. The field ids are shared by the Stone and Checks tabs:
/// `ri`, `width` (the Stone tab), `half-width`, `length-width`, `depth`, `girdle-mm` and
/// `sg` (the Checks tab; `girdle-mm` is the same value as `width`).
fn field_committed(window: &AppWindow, state: &Shared, id: &str, text: &str) {
    let result = match id {
        "ri" => set_refractive_index_override(&mut state.borrow_mut(), text),
        "width" => set_stone_width(&mut state.borrow_mut(), text),
        other => {
            let Some(field) = PreformField::from_id(other) else {
                tracing::warn!(id = other, "an unknown number field was committed");
                return;
            };
            set_preform_field(&mut state.borrow_mut(), field, text)
        }
    };
    finish(window, state, result);
}

/// Pushes the three tabs from the state.
pub fn push(window: &AppWindow, state: &AppState) {
    let props = window.global::<PropsModel>();
    with_pushed(|pushed| {
        stone::push(&props, state, pushed);
        light::push(&props, state, pushed);
        checks::push(&props, state, pushed);
        metrics::push(&props, state, pushed);
    });
}

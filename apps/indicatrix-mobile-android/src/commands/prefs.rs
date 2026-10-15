//! Preferences: each switch and the theme selector write the core settings.

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::{app_state::AppState, settings::MobileSettings};
use slint::{ComponentHandle, Weak};

use crate::{AppWindow, Prefs, push};

/// Wires `Prefs`' callbacks.
pub(super) fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let prefs = window.global::<Prefs>();

    prefs.on_set_theme({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |index| {
            apply(&state, &weak, |settings| {
                settings.theme = push::prefs::theme_from_index(index);
            });
        }
    });

    prefs.on_set_large_handles({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |on| {
            apply(&state, &weak, |settings| settings.large_handles = on);
        }
    });

    prefs.on_set_snap_to_gear({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |on| {
            apply(&state, &weak, |settings| settings.snap_to_gear = on);
        }
    });

    prefs.on_set_battery_saver({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move |on| {
            apply(&state, &weak, |settings| settings.battery_saver = on);
        }
    });
}

/// Applies `edit` to the settings (the core saves them, debounced) and pushes the
/// result to the UI.
fn apply(
    state: &Rc<RefCell<AppState>>,
    weak: &Weak<AppWindow>,
    edit: impl FnOnce(&mut MobileSettings),
) {
    state.borrow_mut().update_settings(edit);
    if let Some(window) = weak.upgrade() {
        let state = state.borrow();
        push::prefs::push(&window, &state.settings);
        push::shell::push_theme(&window, &state.settings);
    }
}

//! Core state -> Slint properties.
//!
//! One module per surface (shell, preferences, about, toast). Each `push` function
//! only writes properties; none of them read the UI back.

use indicatrix_mobile_core::app_state::AppState;

use crate::AppWindow;

pub mod about;
pub mod prefs;
pub mod shell;
pub mod toast;

/// Pushes every surface from `state`. Called once after the callbacks are wired.
pub fn all(window: &AppWindow, state: &AppState) {
    shell::push(window, state);
    prefs::push(window, &state.settings);
    about::push(window, state);
    crate::design::push_all(window, state);
}

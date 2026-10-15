//! The navigation shell: window class, destination, theme and the design summary.

use indicatrix_mobile_core::{app_state::AppState, settings::MobileSettings};
use slint::ComponentHandle;

use super::prefs::theme_index;
use crate::{AppWindow, Shell};

/// Pushes everything `Shell` shows from `state`.
pub fn push(window: &AppWindow, state: &AppState) {
    let shell = window.global::<Shell>();
    shell.set_window_class(state.window_class.index());
    shell.set_destination(state.destination.index());
    shell.set_design_title(state.design_title().into());
    shell.set_design_empty(state.session.design.tiers.is_empty());
    push_theme(window, &state.settings);
}

/// Pushes only the window class (after a resize or a rotation).
pub fn push_window_class(window: &AppWindow, state: &AppState) {
    window
        .global::<Shell>()
        .set_window_class(state.window_class.index());
}

/// Pushes the theme choice, which `ui/theme.slint` turns into dark or light.
pub fn push_theme(window: &AppWindow, settings: &MobileSettings) {
    window
        .global::<Shell>()
        .set_theme_choice(theme_index(settings.theme));
}

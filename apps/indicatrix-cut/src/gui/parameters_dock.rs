//! The Parameters dock's persisted state: whether the panel is expanded and which tab shows.
//!
//! The dock itself is Slint (`ui/components/parameters_panel/`, state in
//! `ui/models/parameters.slint`'s `ParametersModel`). This module pushes the saved state into
//! `ParametersModel` at start-up and writes it back through the debounced settings writer on
//! every change, the same shape as `gui::editor_layout`. The tabs' own controls keep their
//! existing wiring (`gui::render::*`); nothing here touches them.

use crate::{MainWindow, ParametersModel, settings::SettingsPersister};
use slint::ComponentHandle;
use std::sync::Arc;

/// The number of tabs of the dock: Stone, Lighting & Environment, Render & Output.
const TAB_COUNT: u8 = 3;

/// The Slint tab index for a persisted value: anything past the last tab is the Stone tab.
const fn tab_from_saved(saved: u8) -> i32 {
    match saved {
        1 => 1,
        2 => 2,
        _ => 0,
    }
}

/// The persisted value for a Slint tab index (out-of-range indices are saved as the Stone tab).
fn tab_to_saved(index: i32) -> u8 {
    u8::try_from(index)
        .ok()
        .filter(|tab| *tab < TAB_COUNT)
        .unwrap_or(0)
}

/// Applies the saved dock state to `ParametersModel` and wires `layout_changed` to save every
/// later change. Called once from `main_window::callbacks::setup_render_callbacks`, before the
/// window is shown.
///
/// The assignments below queue `ParametersModel`'s own `changed` handlers, so `layout_changed`
/// fires once after the first layout pass with the values just applied. That write stores what
/// is already stored; it is harmless, so no first-firing guard is needed (unlike
/// `editor_layout`, whose flag marks a layout as touched).
pub(super) fn setup_parameters_dock(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let saved = settings_store.snapshot().settings;
    let model = ui.global::<ParametersModel>();
    model.set_active_tab(tab_from_saved(saved.parameters_dock_tab));
    model.set_open(saved.parameters_dock_open);

    let settings_store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_layout_changed(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<ParametersModel>();
        let open = model.get_open();
        let tab = tab_to_saved(model.get_active_tab());
        settings_store.update(|s| {
            s.settings.parameters_dock_open = open;
            s.settings.parameters_dock_tab = tab;
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_tab_maps_to_its_slint_index_and_back() {
        for tab in 0..TAB_COUNT {
            assert_eq!(tab_to_saved(tab_from_saved(tab)), tab);
        }
    }

    #[test]
    fn a_tab_out_of_range_is_the_stone_tab() {
        assert_eq!(tab_from_saved(3), 0);
        assert_eq!(tab_from_saved(200), 0);
        assert_eq!(tab_to_saved(-1), 0);
        assert_eq!(tab_to_saved(3), 0);
    }
}

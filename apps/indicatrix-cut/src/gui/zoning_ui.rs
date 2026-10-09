//! The `zoning` feature's UI switch: rough colour and colour zoning from rig photos.
//!
//! Slint has no `cfg!`, so the `zoning` Cargo feature reaches the UI as one `bool` pushed
//! into the Slint global `Zoning` (`ui/models/zoning.slint`) per window. Every zoning
//! element sits under `if Zoning.enabled`. A default build pushes `false`, so nothing shows.

#[cfg(feature = "zoning")]
use crate::RoughColourWindow;
use crate::{MainWindow, RoughPlannerWindow, Zoning};
use slint::ComponentHandle;

/// Whether this build shows the rough colour and zoning UI: the `zoning` cargo feature.
///
/// Pushed to Slint once per window as `Zoning.enabled` ([`apply_to_main`],
/// [`apply_to_planner`]). Everything behind it is additionally `#[cfg(feature = "zoning")]`
/// on the Rust side; this const only drives what the `.slint` files show.
pub const ZONING_UI: bool = cfg!(feature = "zoning");

/// Sets `Zoning.enabled` on the main window from [`ZONING_UI`].
pub fn apply_to_main(ui: &MainWindow) {
    ui.global::<Zoning>().set_enabled(ZONING_UI);
}

/// Sets `Zoning.enabled` on the Rough Planner window from [`ZONING_UI`]. A global is per
/// window, so the planner window needs its own call.
pub fn apply_to_planner(window: &RoughPlannerWindow) {
    window.global::<Zoning>().set_enabled(ZONING_UI);
}

/// Sets `Zoning.enabled` on the Rough colour wizard window from [`ZONING_UI`].
#[cfg(feature = "zoning")]
pub fn apply_to_rough_colour(window: &RoughColourWindow) {
    window.global::<Zoning>().set_enabled(ZONING_UI);
}

/// Whether the wizard shows the host-species picker (the chromophore model): only when both the
/// zoning UI and the physical colour UI are on. Without the physical colour feature the only
/// spectral model offered is the smooth spectrum.
#[cfg(feature = "zoning")]
#[must_use]
pub const fn host_picker_visible() -> bool {
    ZONING_UI && crate::gui::optics::physics_state::PHYSICS_COLOR_UI
}

#[cfg(test)]
mod tests {
    use super::ZONING_UI;

    #[test]
    fn ui_switch_follows_the_cargo_feature() {
        assert_eq!(ZONING_UI, cfg!(feature = "zoning"));
    }

    #[cfg(feature = "zoning")]
    #[test]
    fn the_host_picker_needs_both_features() {
        assert_eq!(
            super::host_picker_visible(),
            cfg!(feature = "zoning") && cfg!(feature = "physical-color")
        );
    }
}

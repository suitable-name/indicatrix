//! The zoning UI's one remaining build switch: rough color and color zoning from rig photos are
//! always shown; only the host-species picker (the chromophore model) follows the physical color
//! feature.

/// Whether the wizard shows the host-species picker (the chromophore model): only when the
/// physical color UI is on. Without the physical color feature the only spectral model offered
/// is the smooth spectrum.
#[must_use]
pub const fn host_picker_visible() -> bool {
    crate::gui::optics::physics_state::PHYSICS_COLOR_UI
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_host_picker_follows_the_physical_color_feature() {
        assert_eq!(
            super::host_picker_visible(),
            cfg!(feature = "physical-color")
        );
    }
}

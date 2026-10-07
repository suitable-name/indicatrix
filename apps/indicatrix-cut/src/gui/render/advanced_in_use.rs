//! The "Some advanced settings are in use" note of the render settings dialog.
//!
//! Simple mode hides the render settings most designs never touch: the catalogue preview size
//! and samples, the motion-preview scale, the live and local compute choices, the live
//! transfer, the switch that lets this machine share a final-picture render, and the
//! ray-bounce cap. A hidden control keeps its value and still applies, so while Simple mode is
//! on the dialog says so when any of them is off its default ("Switch to Advanced to see
//! them"), instead of leaving a render slower, or a remote unused, for a reason nobody can
//! see.
//!
//! The decision is [`HiddenRenderSettings::in_use`], a pure comparison with the dialog's own
//! "Reset to Defaults" values. The dialog (`settings_dialog.slint`) asks for it through the
//! pure callback `SettingsModel.advanced_in_use`, which [`setup_advanced_in_use_callback`]
//! answers; the tests keep the default values equal to the persisted ones
//! (`AppSettings::default()`), so a changed default cannot leave the note wrong.

use crate::{MainWindow, SettingsModel};
use slint::ComponentHandle;

/// What the controls Simple mode hides hold, as the dialog sends them: slider values and pill
/// indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenRenderSettings {
    /// Catalogue preview thumbnail size in pixels.
    pub preview_size: i32,
    /// Catalogue preview samples per pixel.
    pub preview_spp: i32,
    /// Motion-preview pill: 0 off, 1 half, 2 quarter.
    pub motion_preview_index: i32,
    /// Live Compute pill: 0 local only, 1 remote only, 2 local plus remote.
    pub live_compute_index: i32,
    /// Live Transfer pill: 0 full data, 1 final picture.
    pub live_transfer_index: i32,
    /// Whether this machine renders a share of a final-picture export.
    pub contribute_to_final_picture: bool,
    /// Local Compute pill: 0 CPU, 1 CPU and GPU, 2 GPU only.
    pub local_compute_index: i32,
    /// Max Ray Bounces pill: 0 to 5 for 4, 8, 12, 24, 64 and 128 bounces.
    pub bounce_index: i32,
}

impl HiddenRenderSettings {
    /// The values the dialog's "Reset to Defaults" puts back.
    pub const DEFAULT: Self = Self {
        preview_size: 160,
        preview_spp: 256,
        motion_preview_index: 0,
        live_compute_index: 2,
        live_transfer_index: 0,
        contribute_to_final_picture: true,
        local_compute_index: 1,
        bounce_index: 2,
    };

    /// Whether any hidden setting is off its default.
    #[must_use]
    pub fn in_use(&self) -> bool {
        *self != Self::DEFAULT
    }
}

/// Answers `SettingsModel.advanced_in_use`, the pure callback behind the dialog's note.
pub(in crate::gui) fn setup_advanced_in_use_callback(ui: &MainWindow) {
    ui.global::<SettingsModel>().on_advanced_in_use(
        |preview_size,
         preview_spp,
         motion_preview_index,
         live_compute_index,
         live_transfer_index,
         contribute_to_final_picture,
         local_compute_index,
         bounce_index| {
            HiddenRenderSettings {
                preview_size,
                preview_spp,
                motion_preview_index,
                live_compute_index,
                live_transfer_index,
                contribute_to_final_picture,
                local_compute_index,
                bounce_index,
            }
            .in_use()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        gui::{
            local_compute_target_from_index, local_preview_scale_from_index,
            remote::live_compute_target_from_index,
        },
        settings::model::{AppSettings, LiveTransfer},
    };

    const DEFAULT: HiddenRenderSettings = HiddenRenderSettings::DEFAULT;

    /// The ray-bounce caps of the dialog's pill ladder, by pill index (`bounces_picker.slint`).
    const BOUNCE_LADDER: [u32; 6] = [4, 8, 12, 24, 64, 128];

    #[test]
    fn the_default_settings_are_not_advanced() {
        assert!(!DEFAULT.in_use());
    }

    #[test]
    fn every_hidden_setting_off_its_default_counts_as_in_use() {
        let changed = [
            HiddenRenderSettings {
                preview_size: 224,
                ..DEFAULT
            },
            HiddenRenderSettings {
                preview_spp: 512,
                ..DEFAULT
            },
            HiddenRenderSettings {
                motion_preview_index: 1,
                ..DEFAULT
            },
            HiddenRenderSettings {
                live_compute_index: 0,
                ..DEFAULT
            },
            HiddenRenderSettings {
                live_transfer_index: 1,
                ..DEFAULT
            },
            HiddenRenderSettings {
                contribute_to_final_picture: false,
                ..DEFAULT
            },
            HiddenRenderSettings {
                local_compute_index: 2,
                ..DEFAULT
            },
            HiddenRenderSettings {
                bounce_index: 4,
                ..DEFAULT
            },
        ];
        for settings in changed {
            assert!(settings.in_use(), "{settings:?}");
        }
    }

    /// The numbers above are the dialog's, copied: they must be what a fresh install holds,
    /// or the note would show on every start (or never).
    #[test]
    fn the_default_values_are_the_persisted_defaults() {
        let persisted = AppSettings::default();
        assert_eq!(
            u32::try_from(DEFAULT.preview_size).ok(),
            Some(persisted.preview_size)
        );
        assert_eq!(
            u32::try_from(DEFAULT.preview_spp).ok(),
            Some(persisted.preview_spp)
        );
        assert_eq!(
            local_preview_scale_from_index(DEFAULT.motion_preview_index),
            persisted.local_preview_scale
        );
        assert_eq!(
            live_compute_target_from_index(DEFAULT.live_compute_index),
            persisted.live_compute_target
        );
        assert_eq!(
            local_compute_target_from_index(DEFAULT.local_compute_index),
            persisted.local_compute_target
        );
        assert_eq!(
            DEFAULT.contribute_to_final_picture,
            persisted.contribute_to_final_picture
        );
        assert_eq!(
            LiveTransfer::from_index(DEFAULT.live_transfer_index),
            LiveTransfer::default()
        );
        let bounces = usize::try_from(DEFAULT.bounce_index)
            .ok()
            .and_then(|index| BOUNCE_LADDER.get(index));
        assert_eq!(bounces, Some(&persisted.max_bounces));
    }

    /// The dialog sends the pill's index, not the cap: six pills, rising.
    #[test]
    fn the_bounce_ladder_has_the_six_caps_the_dialog_draws() {
        assert_eq!(BOUNCE_LADDER.len(), 6);
        assert!(BOUNCE_LADDER.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

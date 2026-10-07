//! The "Some advanced settings are in use" note of the export dialog.
//!
//! Simple mode leaves four things out of the export dialog: the compute choice, the ray-bounce
//! cap, the colour space and the lighting-preset fan-out. The first two always start from their
//! defaults (every open re-derives them), and the fan-out list starts unticked at every open, so
//! none of those can be "in use" while Simple mode hides them. Two hidden values can be: a
//! wide-gamut colour space chosen earlier in the session, and a remote that returns only the
//! finished picture (a hand-set endpoint default), which stops this machine from helping.
//! While Simple mode is on the dialog says so, with the same sentence every panel uses, instead of
//! leaving a file in an unexpected colour space for a reason nobody can see.
//!
//! The decision is [`HiddenExportSettings::in_use`], a pure comparison with the dialog's defaults.
//! The dialog (`export_dialog.slint`) asks for it through the pure callback
//! `ExportModel.advanced_in_use`, which [`setup_advanced_in_use_callback`] answers.

use crate::{ExportModel, MainWindow};
use slint::ComponentHandle;

/// What the controls Simple mode hides hold, as the dialog sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenExportSettings {
    /// Colour-space pill: 0 sRGB (the default), 1 Display P3, 2 Rec.2020.
    pub color_space_index: i32,
    /// Transfer pill: 0 full data (the default), 1 final picture only.
    pub transfer_index: i32,
    /// Whether a remote worker is reachable. The transfer choice only matters then: without a
    /// remote there is nothing to transfer.
    pub remote_available: bool,
}

impl HiddenExportSettings {
    /// Whether any hidden setting is off its default.
    #[must_use]
    pub const fn in_use(&self) -> bool {
        self.color_space_index != 0 || (self.remote_available && self.transfer_index == 1)
    }
}

/// Answers `ExportModel.advanced_in_use`, the pure callback behind the dialog's note.
pub(super) fn setup_advanced_in_use_callback(ui: &MainWindow) {
    ui.global::<ExportModel>().on_advanced_in_use(
        |color_space_index, transfer_index, remote_available| {
            HiddenExportSettings {
                color_space_index,
                transfer_index,
                remote_available,
            }
            .in_use()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gui::color_space_from_index, settings::ExportTransfer};
    use indicatrix::color::ColorSpace;

    const DEFAULT: HiddenExportSettings = HiddenExportSettings {
        color_space_index: 0,
        transfer_index: 0,
        remote_available: true,
    };

    #[test]
    fn the_default_export_is_not_advanced() {
        assert!(!DEFAULT.in_use());
        assert!(
            !HiddenExportSettings {
                remote_available: false,
                ..DEFAULT
            }
            .in_use()
        );
    }

    #[test]
    fn a_wide_gamut_colour_space_counts_as_in_use() {
        for color_space_index in [1, 2] {
            let settings = HiddenExportSettings {
                color_space_index,
                ..DEFAULT
            };
            assert!(settings.in_use(), "{settings:?}");
        }
    }

    #[test]
    fn a_final_picture_transfer_counts_only_while_a_remote_is_reachable() {
        let with_remote = HiddenExportSettings {
            transfer_index: 1,
            ..DEFAULT
        };
        assert!(with_remote.in_use());
        let without_remote = HiddenExportSettings {
            remote_available: false,
            ..with_remote
        };
        assert!(!without_remote.in_use());
    }

    /// The defaults above are the dialog's: pill 0 is sRGB and the full-data transfer, so a
    /// reordered pill row would make the note wrong.
    #[test]
    fn the_default_pills_are_srgb_and_full_data() {
        assert_eq!(
            color_space_from_index(DEFAULT.color_space_index),
            ColorSpace::Srgb
        );
        assert_eq!(
            ExportTransfer::from_index(DEFAULT.transfer_index),
            ExportTransfer::default()
        );
        assert_eq!(ExportTransfer::default().index(), DEFAULT.transfer_index);
    }
}

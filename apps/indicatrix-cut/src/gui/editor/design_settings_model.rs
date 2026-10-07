//! The Design settings panel's "Some advanced settings are in use" note
//! (`ui/models/design_settings.slint`, `DesignSettingsModel`).
//!
//! The Simple interface hides the RI override, the extra header lines, the footnotes, the gear
//! reference angle, the printed proportions and the symmetry Mirror switch. A hidden field keeps
//! its value and still applies, so while Simple is on the panel says so when one of them is off
//! its default ("Switch to Advanced to see them") instead of leaving a stone priced, saved or
//! mirrored for a reason nobody can see. The symmetry order itself is not counted: every design
//! has one, so it would make the note show on every design.
//!
//! The decision is [`HiddenDesignSettings::in_use`], a pure function of the fields' texts. The
//! panel asks for it through the pure callback `DesignSettingsModel.advanced_in_use`, which
//! [`setup_design_settings_model`] answers.

use crate::{DesignSettingsModel, MainWindow};
use indicatrix_editor::loading::eval_number;
use slint::ComponentHandle;

/// What the controls the Simple interface hides hold, as the panel sends them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HiddenDesignSettings<'a> {
    /// The RI Override field: blank means the material's own index.
    pub ri_override: &'a str,
    /// The Extra Header Lines field.
    pub extra_headers: &'a str,
    /// The Footnotes field.
    pub footnotes: &'a str,
    /// The Gear Ref. Angle field, in degrees: zero is the default.
    pub gear_reference_angle: &'a str,
    /// The five Printed Proportions fields, joined: blank means none typed.
    pub printed: &'a str,
    /// The symmetry Mirror switch.
    pub mirror: bool,
}

/// Whether a text field holds anything but blanks.
fn typed(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Whether the Gear Ref. Angle text is anything but a zero angle. Text that is no number counts
/// as typed (the field does not accept it, but it is not the default either).
fn reference_angle_set(text: &str) -> bool {
    if !typed(text) {
        return false;
    }
    eval_number(text, None).map_or(true, |degrees| degrees.abs() > 1e-12)
}

impl HiddenDesignSettings<'_> {
    /// Whether any hidden setting is off its default.
    pub(super) fn in_use(&self) -> bool {
        typed(self.ri_override)
            || typed(self.extra_headers)
            || typed(self.footnotes)
            || reference_angle_set(self.gear_reference_angle)
            || typed(self.printed)
            || self.mirror
    }
}

/// Answers `DesignSettingsModel.advanced_in_use`, the pure callback behind the panel's note.
/// Called once from `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_design_settings_model(ui: &MainWindow) {
    ui.global::<DesignSettingsModel>().on_advanced_in_use(
        |ri_override, extra_headers, footnotes, gear_reference_angle, printed, mirror| {
            HiddenDesignSettings {
                ri_override: &ri_override,
                extra_headers: &extra_headers,
                footnotes: &footnotes,
                gear_reference_angle: &gear_reference_angle,
                printed: &printed,
                mirror,
            }
            .in_use()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh design: every hidden field blank or zero, Mirror off.
    const DEFAULT: HiddenDesignSettings<'static> = HiddenDesignSettings {
        ri_override: "",
        extra_headers: "",
        footnotes: "",
        gear_reference_angle: "0",
        printed: "",
        mirror: false,
    };

    #[test]
    fn a_fresh_design_has_no_advanced_settings_in_use() {
        assert!(!DEFAULT.in_use());
    }

    #[test]
    fn blank_and_spaces_count_as_not_typed() {
        let blanks = HiddenDesignSettings {
            ri_override: "  ",
            extra_headers: "\t",
            footnotes: " ",
            gear_reference_angle: "",
            printed: "   ",
            ..DEFAULT
        };
        assert!(!blanks.in_use());
    }

    #[test]
    fn every_hidden_setting_off_its_default_counts_as_in_use() {
        let changed = [
            HiddenDesignSettings {
                ri_override: "1.76",
                ..DEFAULT
            },
            HiddenDesignSettings {
                extra_headers: "Cut by A",
                ..DEFAULT
            },
            HiddenDesignSettings {
                footnotes: "Use a 3000 lap",
                ..DEFAULT
            },
            HiddenDesignSettings {
                gear_reference_angle: "7.5",
                ..DEFAULT
            },
            HiddenDesignSettings {
                printed: "0.62",
                ..DEFAULT
            },
            HiddenDesignSettings {
                mirror: true,
                ..DEFAULT
            },
        ];
        for settings in changed {
            assert!(settings.in_use(), "{settings:?}");
        }
    }

    #[test]
    fn a_zero_reference_angle_is_the_default_however_it_is_written() {
        for zero in ["0", "0.0", " 0 ", "-0", "1-1", "0*5"] {
            let settings = HiddenDesignSettings {
                gear_reference_angle: zero,
                ..DEFAULT
            };
            assert!(!settings.in_use(), "{zero:?}");
        }
    }

    #[test]
    fn a_reference_angle_that_is_no_number_counts_as_typed() {
        let settings = HiddenDesignSettings {
            gear_reference_angle: "abc",
            ..DEFAULT
        };
        assert!(settings.in_use());
    }

    #[test]
    fn a_negative_or_calculated_reference_angle_is_in_use() {
        for angle in ["-3", "90/4", "0.001"] {
            let settings = HiddenDesignSettings {
                gear_reference_angle: angle,
                ..DEFAULT
            };
            assert!(settings.in_use(), "{angle:?}");
        }
    }
}

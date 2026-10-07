//! The "Some advanced settings are in use" note of the Retarget dialog.
//!
//! Simple mode hides the crown handling (the follow-the-pavilion switch, the fraction of the
//! pavilion's shift the crown follows, and the scale-by-ratio switch) and the Optimize
//! objective, range, effort and keep-the-look switch. A hidden control
//! keeps its value and still applies, so while Simple mode is on the dialog says so when any of
//! them is off the value the dialog opens with ("Switch to Advanced to see them"), instead of
//! leaving an Optimize search or a crown move for a reason nobody can see.
//!
//! The decision is [`HiddenRetargetSettings::in_use`], a pure comparison with the dialog's
//! opening values. The dialog asks for it through the pure callback
//! `RetargetModel.advanced_in_use`, which [`setup_advanced_in_use_callback`] answers. The tests
//! keep the defaults equal to what `setup_retarget_open_callback` and `push_options` put in the
//! dialog, so a changed default cannot leave the note wrong.

use crate::{
    MainWindow, RetargetModel,
    gui::editor::retarget::search::{DEFAULT_EFFORT_INDEX, DEFAULT_RANGE_INDEX, SearchSettings},
};
use slint::ComponentHandle;

/// The crown fraction below which the field counts as untouched (the slider steps by 5 %, the
/// percent box takes whole numbers).
const CROWN_FRACTION_EPSILON: f32 = 1e-4;

/// What the controls Simple mode hides hold, as the dialog sends them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HiddenRetargetSettings {
    /// Fraction (0.0 to 1.0) of the pavilion's shift the crown follows.
    pub crown_fraction: f32,
    /// The crown is scaled by the ratio of the critical angles instead.
    pub scale_crown_by_ratio: bool,
    /// The crown follows the pavilion's vertical stretch (the default); off means one of the
    /// older rules is chosen.
    pub crown_follows_pavilion: bool,
    /// Objective combo index.
    pub objective_index: i32,
    /// Range combo index.
    pub range_index: i32,
    /// Effort combo index.
    pub effort_index: i32,
    /// The search penalises options that drift from the design's table size and
    /// crown-to-pavilion ratio (the default); off means the cutter switched it off.
    pub keep_look: bool,
    /// The girdle may thicken by up to 10 % to keep its corners (the default); off means the
    /// cutter switched the allowance off.
    pub girdle_allowance: bool,
}

impl HiddenRetargetSettings {
    /// The values the dialog opens with.
    #[must_use]
    pub(super) fn defaults() -> Self {
        Self {
            crown_fraction: 0.0,
            scale_crown_by_ratio: false,
            crown_follows_pavilion: true,
            objective_index: i32::try_from(SearchSettings::default().preset.index()).unwrap_or(0),
            range_index: i32::try_from(DEFAULT_RANGE_INDEX).unwrap_or(0),
            effort_index: i32::try_from(DEFAULT_EFFORT_INDEX).unwrap_or(0),
            keep_look: SearchSettings::default().keep_look,
            girdle_allowance: SearchSettings::default().girdle.is_some(),
        }
    }

    /// Whether any hidden setting is off its default.
    #[must_use]
    pub(super) fn in_use(&self) -> bool {
        let default = Self::defaults();
        self.crown_fraction.abs() > CROWN_FRACTION_EPSILON
            || self.scale_crown_by_ratio != default.scale_crown_by_ratio
            || self.crown_follows_pavilion != default.crown_follows_pavilion
            || self.objective_index != default.objective_index
            || self.range_index != default.range_index
            || self.effort_index != default.effort_index
            || self.keep_look != default.keep_look
            || self.girdle_allowance != default.girdle_allowance
    }
}

/// Answers `RetargetModel.advanced_in_use`, the pure callback behind the dialog's note.
pub(super) fn setup_advanced_in_use_callback(ui: &MainWindow) {
    ui.global::<RetargetModel>().on_advanced_in_use(
        |crown_fraction,
         scale_crown_by_ratio,
         objective_index,
         range_index,
         effort_index,
         crown_follows_pavilion,
         keep_look,
         girdle_allowance| {
            HiddenRetargetSettings {
                crown_fraction,
                scale_crown_by_ratio,
                crown_follows_pavilion,
                objective_index,
                range_index,
                effort_index,
                keep_look,
                girdle_allowance,
            }
            .in_use()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::editor::retarget::search::{EFFORT_CHOICES, RANGE_CHOICES_DEG};

    fn default() -> HiddenRetargetSettings {
        HiddenRetargetSettings::defaults()
    }

    #[test]
    fn the_opening_values_are_not_advanced() {
        assert!(!default().in_use());
    }

    #[test]
    fn every_hidden_setting_off_its_default_counts_as_in_use() {
        let changed = [
            HiddenRetargetSettings {
                crown_fraction: 0.25,
                ..default()
            },
            HiddenRetargetSettings {
                scale_crown_by_ratio: true,
                ..default()
            },
            HiddenRetargetSettings {
                crown_follows_pavilion: false,
                ..default()
            },
            HiddenRetargetSettings {
                objective_index: default().objective_index + 1,
                ..default()
            },
            HiddenRetargetSettings {
                range_index: default().range_index + 1,
                ..default()
            },
            HiddenRetargetSettings {
                effort_index: default().effort_index + 1,
                ..default()
            },
            HiddenRetargetSettings {
                keep_look: false,
                ..default()
            },
            HiddenRetargetSettings {
                girdle_allowance: false,
                ..default()
            },
        ];
        for settings in changed {
            assert!(settings.in_use(), "{settings:?}");
        }
    }

    /// A crown fraction of a rounding error is the default, not a setting.
    #[test]
    fn a_crown_fraction_of_zero_within_rounding_is_not_in_use() {
        let settings = HiddenRetargetSettings {
            crown_fraction: 1e-6,
            ..default()
        };
        assert!(!settings.in_use());
    }

    /// The dialog's combo indices name the same choices the search starts from: the default
    /// range and effort indices point at the default settings' own numbers, and the objective
    /// is the first preset.
    #[test]
    fn the_default_indices_name_the_default_search_settings() {
        let settings = SearchSettings::default();
        let range = usize::try_from(default().range_index)
            .ok()
            .and_then(|index| RANGE_CHOICES_DEG.get(index));
        assert_eq!(range, Some(&settings.range_deg));
        let effort = usize::try_from(default().effort_index)
            .ok()
            .and_then(|index| EFFORT_CHOICES.get(index));
        assert_eq!(effort, Some(&settings.evaluations));
        assert_eq!(default().objective_index, 0);
    }
}

//! The interface preferences: [`UiMode`] (Simple or Advanced controls) and the UI scale
//! choices, with the pure rules the settings file, the start-up code and the Preferences
//! dialog share.
//!
//! No Slint here, so every rule is unit-tested directly.

use serde::{Deserialize, Deserializer, Serialize};

/// Which set of controls the interface shows.
///
/// One global switch, shown as the "Simple | Advanced" pill in the header and in the
/// Preferences dialog. [`UiMode::Advanced`] is the type's default because it is the
/// interface exactly as it was before the switch existed: a settings file without the key
/// (an existing install) must keep showing everything. Only a brand-new install starts in
/// [`UiMode::Simple`] -- see `AppSettings::default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiMode {
    /// The controls most designs need.
    Simple,
    /// Every control.
    #[default]
    Advanced,
}

impl UiMode {
    /// Whether this is the Simple mode (what `PreferencesModel.simple_mode` holds).
    #[must_use]
    pub const fn is_simple(self) -> bool {
        matches!(self, Self::Simple)
    }

    /// The mode for the `PreferencesModel.simple_mode` switch.
    #[must_use]
    pub const fn from_simple(simple: bool) -> Self {
        if simple { Self::Simple } else { Self::Advanced }
    }

    /// Reads the mode from the text written in the settings file: `"simple"` (any
    /// capitalisation) is Simple, anything else -- including a value a newer build wrote
    /// -- is Advanced, so an unfamiliar word never costs the user their controls.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.trim().eq_ignore_ascii_case("simple") {
            Self::Simple
        } else {
            Self::Advanced
        }
    }

    /// `serde` hook: [`Self::parse`] on a string value, so an unfamiliar word loads as
    /// Advanced instead of failing the whole settings file.
    ///
    /// # Errors
    ///
    /// Fails only when the value is not a string at all.
    pub fn deserialize_lenient<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Ok(Self::parse(&text))
    }
}

/// The UI scale percentages the Preferences dialog offers, besides `0` (Automatic: follow
/// the operating system).
pub const UI_SCALE_CHOICES: [u16; 8] = [75, 90, 100, 110, 125, 150, 175, 200];

/// The value `SLINT_SCALE_FACTOR` takes for a chosen scale, or `None` for Automatic (`0`)
/// and for any percentage that is not one of [`UI_SCALE_CHOICES`].
///
/// Spelled as a table, not computed, so the text is exactly what a person would type.
#[must_use]
pub const fn ui_scale_factor_text(percent: u16) -> Option<&'static str> {
    match percent {
        75 => Some("0.75"),
        90 => Some("0.9"),
        100 => Some("1"),
        110 => Some("1.1"),
        125 => Some("1.25"),
        150 => Some("1.5"),
        175 => Some("1.75"),
        200 => Some("2"),
        _ => None,
    }
}

/// Whether `percent` is a value the setting may hold: `0` (Automatic) or one of
/// [`UI_SCALE_CHOICES`].
#[must_use]
pub const fn is_allowed_ui_scale_percent(percent: u16) -> bool {
    percent == 0 || ui_scale_factor_text(percent).is_some()
}

/// `raw` as a stored scale: itself when allowed, else `0` (Automatic). The one rule a
/// loaded, hand-edited or dialog-chosen value passes through.
#[must_use]
pub fn normalize_ui_scale_percent(raw: i64) -> u16 {
    u16::try_from(raw)
        .ok()
        .filter(|percent| is_allowed_ui_scale_percent(*percent))
        .unwrap_or(0)
}

/// `serde` hook for `AppSettings::ui_scale_percent`: reads a signed integer and
/// normalises it, so a hand-edited value never fails the whole settings file.
///
/// # Errors
///
/// Fails only when the value is not an integer at all.
pub fn deserialize_ui_scale_percent<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = i64::deserialize(deserializer)?;
    Ok(normalize_ui_scale_percent(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};

    #[test]
    fn the_mode_round_trips_through_the_simple_switch() {
        assert!(UiMode::Simple.is_simple());
        assert!(!UiMode::Advanced.is_simple());
        assert_eq!(UiMode::from_simple(true), UiMode::Simple);
        assert_eq!(UiMode::from_simple(false), UiMode::Advanced);
    }

    #[test]
    fn the_mode_text_is_read_leniently() {
        assert_eq!(UiMode::parse("simple"), UiMode::Simple);
        assert_eq!(UiMode::parse(" Simple "), UiMode::Simple);
        assert_eq!(UiMode::parse("advanced"), UiMode::Advanced);
        assert_eq!(UiMode::parse("expert"), UiMode::Advanced);
        assert_eq!(UiMode::parse(""), UiMode::Advanced);
    }

    #[test]
    fn the_type_default_is_advanced_the_interface_before_the_switch_existed() {
        assert_eq!(UiMode::default(), UiMode::Advanced);
    }

    #[test]
    fn every_offered_scale_has_a_factor_text_and_is_allowed() {
        for percent in UI_SCALE_CHOICES {
            assert!(
                ui_scale_factor_text(percent).is_some(),
                "{percent} % has no factor text"
            );
            assert!(is_allowed_ui_scale_percent(percent));
        }
        assert!(is_allowed_ui_scale_percent(0));
    }

    #[test]
    fn the_factor_text_is_the_percent_divided_by_one_hundred() {
        for percent in UI_SCALE_CHOICES {
            let text = ui_scale_factor_text(percent).expect("offered scale");
            let parsed: f32 = text.parse().expect("a plain decimal");
            assert!(
                (parsed - f32::from(percent) / 100.0).abs() < 1e-6,
                "{percent} % -> {text}"
            );
        }
        assert_eq!(ui_scale_factor_text(75), Some("0.75"));
        assert_eq!(ui_scale_factor_text(100), Some("1"));
        assert_eq!(ui_scale_factor_text(200), Some("2"));
    }

    #[test]
    fn automatic_and_unlisted_percentages_have_no_factor_text() {
        assert_eq!(ui_scale_factor_text(0), None);
        assert_eq!(ui_scale_factor_text(133), None);
        assert_eq!(ui_scale_factor_text(u16::MAX), None);
    }

    #[test]
    fn a_stored_scale_is_kept_when_allowed_and_automatic_otherwise() {
        for percent in UI_SCALE_CHOICES {
            assert_eq!(normalize_ui_scale_percent(i64::from(percent)), percent);
        }
        assert_eq!(normalize_ui_scale_percent(0), 0);
        assert_eq!(normalize_ui_scale_percent(133), 0);
        assert_eq!(normalize_ui_scale_percent(-100), 0);
        assert_eq!(normalize_ui_scale_percent(65_636), 0);
        assert_eq!(normalize_ui_scale_percent(i64::MAX), 0);
    }

    /// A settings document holding only `[settings]` plus `lines`.
    fn parse(lines: &str) -> SettingsFile {
        toml::from_str(&format!("[settings]\n{lines}\n")).expect("settings parse")
    }

    #[test]
    fn an_existing_file_without_the_keys_keeps_the_full_interface_and_skips_the_tour() {
        let settings = parse("exposure = 1.2").settings;
        assert_eq!(settings.ui_mode, UiMode::Advanced);
        assert!(settings.first_run_tour_done);
        assert!(settings.tutorials_completed.is_empty());
        assert_eq!(settings.ui_scale_percent, 0);
        assert!(!settings.high_contrast);
        assert!(!settings.large_handles);
        assert!(!settings.manipulate_snap_off);
        assert!(settings.slice_symmetric);
    }

    #[test]
    fn a_brand_new_install_starts_simple_with_the_tour_pending() {
        let settings = AppSettings::default();
        assert_eq!(settings.ui_mode, UiMode::Simple);
        assert!(!settings.first_run_tour_done);
        assert!(settings.tutorials_completed.is_empty());
        assert_eq!(settings.ui_scale_percent, 0);
        assert!(!settings.high_contrast);
        assert!(!settings.large_handles);
        assert!(!settings.manipulate_snap_off);
        assert!(settings.slice_symmetric);
    }

    #[test]
    fn treating_a_default_as_an_existing_install_keeps_everything_visible() {
        let mut settings = AppSettings::default();
        settings.treat_as_existing_install();
        assert_eq!(settings.ui_mode, UiMode::Advanced);
        assert!(settings.first_run_tour_done);
    }

    #[test]
    fn every_new_setting_round_trips_through_toml() {
        let mut file = SettingsFile::default();
        file.settings.ui_mode = UiMode::Advanced;
        file.settings.first_run_tour_done = true;
        file.settings.tutorials_completed.insert("tour".to_owned());
        file.settings.tutorials_completed.insert("slice".to_owned());
        file.settings.ui_scale_percent = 125;
        file.settings.high_contrast = true;
        file.settings.large_handles = true;
        file.settings.manipulate_snap_off = true;
        file.settings.slice_symmetric = false;

        let text = toml::to_string_pretty(&file).expect("serialize");
        assert!(text.contains("ui_mode = \"advanced\""), "{text}");
        assert!(text.contains("ui_scale_percent = 125"), "{text}");
        let parsed: SettingsFile = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, file);

        let mut simple = SettingsFile::default();
        simple.settings.ui_mode = UiMode::Simple;
        let text = toml::to_string_pretty(&simple).expect("serialize");
        assert!(text.contains("ui_mode = \"simple\""), "{text}");
        let parsed: SettingsFile = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.settings.ui_mode, UiMode::Simple);
        assert!(!parsed.settings.first_run_tour_done);
    }

    #[test]
    fn an_unfamiliar_mode_word_loads_as_advanced_without_failing_the_file() {
        let settings = parse("ui_mode = \"expert\"\nexposure = 1.5").settings;
        assert_eq!(settings.ui_mode, UiMode::Advanced);
        assert!((settings.exposure - 1.5).abs() < 1e-6);
    }

    #[test]
    fn a_hand_edited_scale_outside_the_choices_loads_as_automatic() {
        for (written, expected) in [
            ("0", 0),
            ("75", 75),
            ("125", 125),
            ("200", 200),
            ("133", 0),
            ("-5", 0),
            ("9999999999", 0),
        ] {
            let settings = parse(&format!("ui_scale_percent = {written}")).settings;
            assert_eq!(settings.ui_scale_percent, expected, "wrote {written}");
        }
    }

    #[test]
    fn setting_the_scale_goes_through_the_same_rule() {
        let mut settings = AppSettings::default();
        settings.set_ui_scale_percent(150);
        assert_eq!(settings.ui_scale_percent, 150);
        settings.set_ui_scale_percent(133);
        assert_eq!(settings.ui_scale_percent, 0);
        settings.set_ui_scale_percent(-1);
        assert_eq!(settings.ui_scale_percent, 0);
    }

    #[test]
    fn resetting_tutorials_clears_the_finished_list_only() {
        let mut settings = AppSettings {
            first_run_tour_done: true,
            ..AppSettings::default()
        };
        settings.tutorials_completed.insert("slice".to_owned());
        settings.tutorials_completed.insert("slice".to_owned());
        settings.tutorials_completed.insert("handles".to_owned());
        assert_eq!(settings.tutorials_completed.len(), 2);
        settings.reset_tutorials();
        assert!(settings.tutorials_completed.is_empty());
        assert!(
            settings.first_run_tour_done,
            "the welcome tour has its own switch"
        );
    }

    #[test]
    fn showing_the_tour_again_clears_its_done_flag() {
        let mut settings = AppSettings::default();
        settings.treat_as_existing_install();
        assert!(settings.first_run_tour_done);
        settings.show_tour_again();
        assert!(!settings.first_run_tour_done);
    }
}

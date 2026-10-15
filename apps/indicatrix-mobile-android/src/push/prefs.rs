//! The Preferences page: theme and the three switches.

use indicatrix_mobile_core::settings::{MobileSettings, ThemeChoice};
use slint::ComponentHandle;

use crate::{AppWindow, Prefs};

/// The Slint index of a theme choice: 0 system, 1 dark, 2 light.
#[must_use]
pub const fn theme_index(theme: ThemeChoice) -> i32 {
    match theme {
        ThemeChoice::System => 0,
        ThemeChoice::Dark => 1,
        ThemeChoice::Light => 2,
    }
}

/// The theme choice for a Slint index; anything unknown means "follow the system".
#[must_use]
pub const fn theme_from_index(index: i32) -> ThemeChoice {
    match index {
        1 => ThemeChoice::Dark,
        2 => ThemeChoice::Light,
        _ => ThemeChoice::System,
    }
}

/// Pushes the preference values from `settings`.
pub fn push(window: &AppWindow, settings: &MobileSettings) {
    let prefs = window.global::<Prefs>();
    prefs.set_theme(theme_index(settings.theme));
    prefs.set_large_handles(settings.large_handles);
    prefs.set_snap_to_gear(settings.snap_to_gear);
    prefs.set_battery_saver(settings.battery_saver);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_indices_round_trip() {
        for index in 0..=2 {
            assert_eq!(theme_index(theme_from_index(index)), index);
        }
    }

    #[test]
    fn unknown_theme_index_follows_the_system() {
        assert_eq!(theme_index(theme_from_index(-1)), 0);
        assert_eq!(theme_index(theme_from_index(9)), 0);
    }
}

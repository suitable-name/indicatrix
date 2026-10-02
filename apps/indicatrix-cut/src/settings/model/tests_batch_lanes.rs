//! Tests for `AppSettings::remote_batch_lanes`: its default, its `1..=32` limit on load
//! and on set, and its round trip through the settings file.

use super::{
    AppSettings, SettingsFile,
    app_settings::{
        DEFAULT_REMOTE_BATCH_LANES, MAX_REMOTE_BATCH_LANES, MIN_REMOTE_BATCH_LANES,
        clamp_remote_batch_lanes,
    },
};

/// A settings document holding only `remote_batch_lanes = value`.
fn file_with_lanes(value: &str) -> String {
    format!("[settings]\nremote_batch_lanes = {value}\n")
}

#[test]
fn a_fresh_install_keeps_four_pictures_in_flight() {
    assert_eq!(DEFAULT_REMOTE_BATCH_LANES, 4);
    assert_eq!(AppSettings::default().remote_batch_lanes, 4);
}

#[test]
fn a_settings_file_without_the_key_loads_the_default() {
    let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
    assert_eq!(
        parsed.settings.remote_batch_lanes,
        DEFAULT_REMOTE_BATCH_LANES
    );
}

#[test]
fn the_limit_is_one_through_thirty_two() {
    assert_eq!(clamp_remote_batch_lanes(0), MIN_REMOTE_BATCH_LANES);
    assert_eq!(clamp_remote_batch_lanes(1), 1);
    assert_eq!(clamp_remote_batch_lanes(17), 17);
    assert_eq!(clamp_remote_batch_lanes(32), 32);
    assert_eq!(clamp_remote_batch_lanes(33), MAX_REMOTE_BATCH_LANES);
    assert_eq!(clamp_remote_batch_lanes(u32::MAX), MAX_REMOTE_BATCH_LANES);
}

/// A hand-edited value outside the range -- even a negative or an absurd one -- loads as
/// the nearest valid count and never fails the whole document.
#[test]
fn an_out_of_range_value_in_the_file_is_limited_on_load() {
    for (written, expected) in [
        ("0", 1),
        ("-7", 1),
        ("1", 1),
        ("8", 8),
        ("32", 32),
        ("33", 32),
        ("9999999999999", 32),
    ] {
        let parsed: SettingsFile =
            toml::from_str(&file_with_lanes(written)).expect("an integer always parses");
        assert_eq!(
            parsed.settings.remote_batch_lanes, expected,
            "wrote {written}"
        );
    }
}

#[test]
fn setting_the_lane_count_limits_it() {
    let mut settings = AppSettings::default();
    settings.set_remote_batch_lanes(0);
    assert_eq!(settings.remote_batch_lanes, 1);
    settings.set_remote_batch_lanes(12);
    assert_eq!(settings.remote_batch_lanes, 12);
    settings.set_remote_batch_lanes(500);
    assert_eq!(settings.remote_batch_lanes, 32);
}

#[test]
fn the_lane_count_round_trips_through_toml() {
    let mut file = SettingsFile::default();
    file.settings.set_remote_batch_lanes(11);
    let text = toml::to_string_pretty(&file).expect("serialize");
    assert!(text.contains("remote_batch_lanes = 11"), "{text}");
    let parsed: SettingsFile = toml::from_str(&text).expect("deserialize");
    assert_eq!(parsed.settings.remote_batch_lanes, 11);
}

//! Tests for `AppSettings::remote_preview_batch_size` (its default, the `1..=32` limit on
//! load and on set) and for the GPU-worker default of `remote_batch_lanes`.

use super::{
    AppSettings, SettingsFile, app_settings::GPU_REMOTE_BATCH_LANES,
    clamp_remote_preview_batch_size, effective_remote_batch_lanes,
};

fn file_with(value: i64) -> String {
    format!("[settings]\nremote_preview_batch_size = {value}\n")
}

#[test]
fn the_default_batch_size_is_ten_and_an_old_file_loads_it() {
    assert_eq!(AppSettings::default().remote_preview_batch_size, 10);
    let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").unwrap();
    assert_eq!(parsed.settings.remote_preview_batch_size, 10);
    assert!(!parsed.settings.remote_batch_lanes_user_set);
}

#[test]
fn the_batch_size_is_limited_to_one_through_thirty_two() {
    assert_eq!(clamp_remote_preview_batch_size(0), 1);
    assert_eq!(clamp_remote_preview_batch_size(1), 1);
    assert_eq!(clamp_remote_preview_batch_size(10), 10);
    assert_eq!(clamp_remote_preview_batch_size(32), 32);
    assert_eq!(clamp_remote_preview_batch_size(33), 32);
    for (written, expected) in [(-5, 1), (0, 1), (7, 7), (32, 32), (99, 32)] {
        let parsed: SettingsFile = toml::from_str(&file_with(written)).unwrap();
        assert_eq!(parsed.settings.remote_preview_batch_size, expected);
    }
    let mut settings = AppSettings::default();
    settings.set_remote_preview_batch_size(500);
    assert_eq!(settings.remote_preview_batch_size, 32);
}

#[test]
fn a_gpu_worker_gets_two_dispatchers_until_the_user_sets_a_lane_count() {
    let mut settings = AppSettings::default();
    let effective = |settings: &AppSettings, remote_is_gpu: bool| {
        effective_remote_batch_lanes(
            settings.remote_batch_lanes,
            settings.remote_batch_lanes_user_set,
            remote_is_gpu,
        )
    };
    assert_eq!(effective(&settings, true), GPU_REMOTE_BATCH_LANES);
    assert_eq!(effective(&settings, false), 4);
    settings.set_remote_batch_lanes(4);
    assert!(settings.remote_batch_lanes_user_set);
    assert_eq!(effective(&settings, true), 4);
    settings.set_remote_batch_lanes(9);
    assert_eq!(effective(&settings, false), 9);
}

#[test]
fn the_user_set_flag_and_batch_size_round_trip_through_toml() {
    let mut file = SettingsFile::default();
    file.settings.set_remote_batch_lanes(6);
    file.settings.set_remote_preview_batch_size(12);
    let text = toml::to_string_pretty(&file).unwrap();
    let parsed: SettingsFile = toml::from_str(&text).unwrap();
    assert!(parsed.settings.remote_batch_lanes_user_set);
    assert_eq!(parsed.settings.remote_preview_batch_size, 12);
}

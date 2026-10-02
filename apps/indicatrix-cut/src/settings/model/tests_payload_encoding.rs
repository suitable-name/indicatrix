//! Tests for `AppSettings::payload_encoding`: the default, the tolerant load of an absent
//! or unreadable value, and the round trip through the settings file.

use super::{AppSettings, SettingsFile};
use indicatrix_net::messages::{PayloadEncoding, adaptive::PayloadChoice};

#[test]
fn a_fresh_install_follows_the_link() {
    assert_eq!(AppSettings::default().payload_encoding, PayloadChoice::Auto);
}

#[test]
fn a_settings_file_without_the_key_loads_auto() {
    let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
    assert_eq!(parsed.settings.payload_encoding, PayloadChoice::Auto);
}

#[test]
fn a_pinned_encoding_round_trips_through_the_file() {
    let mut file = SettingsFile::default();
    file.settings.payload_encoding =
        PayloadChoice::Fixed(PayloadEncoding::ShuffleZstd { level: 5 });
    let text = toml::to_string_pretty(&file).expect("serialise");
    assert!(text.contains("payload_encoding = \"zstd:5\""), "{text}");
    let reloaded: SettingsFile = toml::from_str(&text).expect("parse");
    assert_eq!(
        reloaded.settings.payload_encoding,
        file.settings.payload_encoding
    );
}

#[test]
fn an_unreadable_value_loads_auto_instead_of_failing_the_file() {
    let parsed: SettingsFile =
        toml::from_str("[settings]\npayload_encoding = \"gzip\"\nexposure = 1.5\n").expect("parse");
    assert_eq!(parsed.settings.payload_encoding, PayloadChoice::Auto);
    assert!((parsed.settings.exposure - 1.5).abs() < f32::EPSILON);
}

use super::*;

#[test]
fn record_recent_native_file_inserts_at_the_front() {
    let mut settings = AppSettings::default();
    settings.record_recent_native_file("a.indicatrix.toml".to_string());
    settings.record_recent_native_file("b.indicatrix.toml".to_string());
    assert_eq!(
        settings.recent_native_files,
        vec![
            "b.indicatrix.toml".to_string(),
            "a.indicatrix.toml".to_string()
        ]
    );
}

#[test]
fn record_recent_native_file_moves_an_existing_entry_to_the_front_without_duplicating() {
    let mut settings = AppSettings::default();
    settings.record_recent_native_file("a.indicatrix.toml".to_string());
    settings.record_recent_native_file("b.indicatrix.toml".to_string());
    settings.record_recent_native_file("a.indicatrix.toml".to_string());
    assert_eq!(
        settings.recent_native_files,
        vec![
            "a.indicatrix.toml".to_string(),
            "b.indicatrix.toml".to_string()
        ]
    );
}

#[test]
fn recording_a_design_file_replaces_the_older_sidecar_entry_of_the_same_design() {
    let mut settings = AppSettings::default();
    settings.record_recent_native_file("d/round.indicatrix.toml".to_string());
    settings.record_recent_native_file("d/other.gemcut.toml".to_string());
    settings.record_recent_native_file("d/round.indicatrix".to_string());
    assert_eq!(
        settings.recent_native_files,
        vec![
            "d/round.indicatrix".to_string(),
            "d/other.gemcut.toml".to_string()
        ],
        "both kinds are accepted; the older entry of the saved design is gone"
    );
    // Opening an older sidecar again afterwards is still recorded.
    settings.record_recent_native_file("d/legacy.indicatrix.toml".to_string());
    assert_eq!(settings.recent_native_files[0], "d/legacy.indicatrix.toml");
    assert_eq!(settings.recent_native_files.len(), 3);
}

#[test]
fn record_recent_native_file_caps_at_max_recent_native_files() {
    let mut settings = AppSettings::default();
    for i in 0..(MAX_RECENT_NATIVE_FILES + 3) {
        settings.record_recent_native_file(format!("design-{i}.indicatrix.toml"));
    }
    assert_eq!(settings.recent_native_files.len(), MAX_RECENT_NATIVE_FILES);
    // Most recent first: the last one recorded is still at the front, and the
    // oldest entries fell off the back rather than the front.
    assert_eq!(
        settings.recent_native_files[0],
        format!("design-{}.indicatrix.toml", MAX_RECENT_NATIVE_FILES + 2)
    );
}

// --- Suppressed confirmations: AppSettings::suppressed_confirmations ---

#[test]
fn a_fresh_settings_file_suppresses_nothing() {
    let settings = AppSettings::default();
    assert!(!settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
}

#[test]
fn suppress_confirm_is_reflected_immediately() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert!(settings.is_confirm_suppressed("write_confirm.not_closed_solid"));
}

#[test]
fn suppressing_one_key_does_not_suppress_a_different_one() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert!(!settings.is_confirm_suppressed("write_confirm.overwrite_unrelated"));
}

#[test]
fn suppress_confirm_is_idempotent() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    settings.suppress_confirm("write_confirm.not_closed_solid");
    assert_eq!(settings.suppressed_confirmations.len(), 1);
}

/// A round trip through TOML -- the actual persistence mechanism
/// (`settings::store`) -- so a regression that drops `#[serde(default)]` or
/// breaks `BTreeSet<String>` serialisation is caught here rather than only in
/// a live app.
#[test]
fn suppressed_confirmations_round_trips_through_toml() {
    let mut settings = AppSettings::default();
    settings.suppress_confirm("write_confirm.not_closed_solid");
    settings.suppress_confirm("write_confirm.overwrite_unrelated");
    let toml_text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(
        restored.suppressed_confirmations,
        settings.suppressed_confirmations
    );
}

/// A settings file saved BEFORE this field existed (no `suppressed_confirmations`
/// key at all) must still load, with nothing suppressed -- the idempotent
/// migration this field's own `#[serde(default)]` provides.
#[test]
fn a_settings_file_predating_this_field_loads_with_nothing_suppressed() {
    let settings = AppSettings::default();
    let mut toml_text = toml::to_string(&settings).expect("settings must serialize");
    // Strip the field this test is specifically about, simulating an
    // old-format file that never had it.
    let filtered: String = toml_text
        .lines()
        .filter(|line| !line.contains("suppressed_confirmations"))
        .collect::<Vec<_>>()
        .join("\n");
    toml_text = filtered;
    let restored: AppSettings =
        toml::from_str(&toml_text).expect("a settings file missing this field must still parse");
    assert!(restored.suppressed_confirmations.is_empty());
}

// --- Live Render colour: AppSettings::render_body_color_override ---

/// The yellow body-colour preset's absorption triple: values with a fractional part that
/// does not survive a lossy text round trip, so a bit-exact comparison is meaningful.
const YELLOW: [f32; 3] = [0.2, 0.4, 2.8];

/// Loads a settings document that carries `value` for the key and nothing else changed.
fn load_with_color_value(value: &str) -> AppSettings {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    toml::from_str(&format!("render_body_color_override = {value}\n{text}"))
        .expect("a bad colour value must not fail the whole settings file")
}

#[test]
fn the_render_colour_defaults_to_the_material_default() {
    assert_eq!(AppSettings::default().render_body_color_override, None);
}

#[test]
fn the_render_colour_round_trips_through_toml_bit_for_bit() {
    let settings = AppSettings {
        render_body_color_override: Some(YELLOW),
        ..AppSettings::default()
    };
    let toml_text = toml::to_string(&settings).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, Some(YELLOW));

    // "Material default" writes no key at all and reads back as None.
    let cleared = AppSettings {
        render_body_color_override: None,
        ..settings
    };
    let toml_text = toml::to_string(&cleared).expect("settings must serialize");
    assert!(!toml_text.contains("render_body_color_override"));
    let restored: AppSettings = toml::from_str(&toml_text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, None);
}

#[test]
fn a_settings_file_predating_the_render_colour_loads_with_the_material_default() {
    let text = toml::to_string(&AppSettings::default()).expect("settings must serialize");
    let restored: AppSettings = toml::from_str(&text).expect("settings must parse");
    assert_eq!(restored.render_body_color_override, None);
}

#[test]
fn a_hand_edited_render_colour_that_is_not_three_numbers_loads_as_the_default() {
    for bad in [
        "[1.0, 2.0]",
        "[1.0, 2.0, 3.0, 4.0]",
        "\"yellow\"",
        "[\"a\", \"b\", \"c\"]",
        "42",
    ] {
        assert_eq!(
            load_with_color_value(bad).render_body_color_override,
            None,
            "value {bad}"
        );
    }
    assert_eq!(
        load_with_color_value("[nan, 0.0, 0.0]").render_body_color_override,
        None,
        "a non-finite channel must not reach the renderer"
    );
}

#[test]
fn a_hand_edited_render_colour_is_limited_to_the_supported_range() {
    let loaded = load_with_color_value("[50.0, -1.0, 2.0]");
    assert_eq!(
        loaded.render_body_color_override,
        Some([MAX_RENDER_BODY_COLOR_CHANNEL, 0.0, 2.0])
    );
}

use super::*;
use crate::settings::model::{AppSettings, LightingPreset, UiMode};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, unique scratch directory under the OS temp dir, cleaned up when the
/// returned guard drops. Avoids adding a `tempfile` dependency for a handful of
/// small round-trip tests.
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-cut-settings-test-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_file_falls_back_to_defaults_without_panicking() {
    let dir = TempDir::new("missing");
    let path = dir.path().join("does-not-exist.toml");
    let loaded = load_or_default(&path);
    assert_eq!(loaded, SettingsFile::default());
}

#[test]
fn corrupt_file_falls_back_to_defaults_without_panicking() {
    let dir = TempDir::new("corrupt");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "this is { not [ valid toml at all").unwrap();
    let loaded = load_or_default(&path);
    assert_eq!(loaded, defaults_for_existing_install());
}

#[test]
fn unreadable_directory_in_place_of_file_falls_back_to_defaults() {
    // A directory in place of a file makes `read_to_string` fail with something
    // other than `NotFound`, exercising the third fallback arm.
    let dir = TempDir::new("isdir");
    let path = dir.path().join("settings.toml");
    std::fs::create_dir_all(&path).unwrap();
    let loaded = load_or_default(&path);
    assert_eq!(loaded, defaults_for_existing_install());
}

/// Only an ABSENT file is a brand-new install: it starts in the Simple interface with
/// the welcome tour still to come.
#[test]
fn a_missing_file_is_a_new_install_that_starts_simple_with_the_tour_pending() {
    let dir = TempDir::new("new-install");
    let loaded = load_or_default(&dir.path().join("does-not-exist.toml"));
    assert_eq!(loaded.settings.ui_mode, UiMode::Simple);
    assert!(!loaded.settings.first_run_tour_done);
}

/// A file that existed but could not be used belongs to someone who already uses the
/// app: they keep every control and get no tour.
#[test]
fn a_corrupt_or_unreadable_file_keeps_the_full_interface_and_skips_the_tour() {
    let dir = TempDir::new("existing-install-fallback");
    let corrupt = dir.path().join("corrupt.toml");
    std::fs::write(&corrupt, "this is { not [ valid toml at all").unwrap();
    let unreadable = dir.path().join("a-directory.toml");
    std::fs::create_dir_all(&unreadable).unwrap();
    for path in [corrupt, unreadable] {
        let loaded = load_or_default(&path);
        assert_eq!(loaded.settings.ui_mode, UiMode::Advanced, "{path:?}");
        assert!(loaded.settings.first_run_tour_done, "{path:?}");
    }
}

/// A settings file from before the interface switch existed: its owner is an existing
/// user, so it loads Advanced with the tour done -- not the newcomer's defaults.
#[test]
fn an_existing_file_without_the_interface_keys_loads_advanced_with_the_tour_done() {
    let dir = TempDir::new("existing-install");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "[settings]\nexposure = 1.0\n").unwrap();
    let loaded = load_or_default(&path);
    assert_eq!(loaded.settings.ui_mode, UiMode::Advanced);
    assert!(loaded.settings.first_run_tour_done);
}

#[test]
fn the_interface_preferences_round_trip_through_save_and_load() {
    let dir = TempDir::new("interface-preferences");
    let path = dir.path().join("settings.toml");
    let mut file = SettingsFile::default();
    file.settings.ui_mode = UiMode::Advanced;
    file.settings.first_run_tour_done = true;
    file.settings.ui_scale_percent = 150;
    file.settings.high_contrast = true;
    file.settings.large_handles = true;
    file.settings.manipulate_snap_off = true;
    file.settings.slice_symmetric = false;
    file.settings.tutorials_completed.insert("slice".to_owned());
    save(&path, &file).unwrap();
    assert_eq!(load_or_default(&path), file);
}

#[test]
fn peeking_the_scale_reads_the_saved_percentage() {
    let dir = TempDir::new("peek-scale");
    let path = dir.path().join("settings.toml");
    let mut file = SettingsFile::default();
    file.settings.ui_scale_percent = 125;
    save(&path, &file).unwrap();
    assert_eq!(peek_ui_scale_percent_at(&path), 125);
}

#[test]
fn peeking_the_scale_never_fails() {
    let dir = TempDir::new("peek-scale-failures");
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    // No file, a directory, broken TOML, a missing key, an unoffered value, a
    // negative one, a value of the wrong type and a key in the wrong table.
    assert_eq!(
        peek_ui_scale_percent_at(&dir.path().join("does-not-exist.toml")),
        0
    );
    assert_eq!(peek_ui_scale_percent_at(dir.path()), 0);
    assert_eq!(
        peek_ui_scale_percent_at(&write("broken.toml", "not { toml")),
        0
    );
    assert_eq!(
        peek_ui_scale_percent_at(&write("no-key.toml", "[settings]\nexposure = 1.0\n")),
        0
    );
    assert_eq!(
        peek_ui_scale_percent_at(&write("odd.toml", "[settings]\nui_scale_percent = 133\n")),
        0
    );
    assert_eq!(
        peek_ui_scale_percent_at(&write(
            "negative.toml",
            "[settings]\nui_scale_percent = -125\n"
        )),
        0
    );
    assert_eq!(
        peek_ui_scale_percent_at(&write(
            "text.toml",
            "[settings]\nui_scale_percent = \"125\"\n"
        )),
        0
    );
    assert_eq!(
        peek_ui_scale_percent_at(&write("top-level.toml", "ui_scale_percent = 125\n")),
        0
    );
}

#[test]
fn partially_corrupt_file_keeps_valid_fields_and_defaults_missing_ones() {
    let dir = TempDir::new("partial");
    let path = dir.path().join("settings.toml");
    // Valid TOML, missing most fields, with an extra unknown key -- must still
    // parse, defaulting everything not present.
    std::fs::write(
        &path,
        "[settings]\nexposure = 1.75\nsomething_unknown = true\n",
    )
    .unwrap();
    let loaded = load_or_default(&path);
    assert_eq!(loaded.settings.exposure, 1.75);
    assert_eq!(
        loaded.settings.target_samples,
        AppSettings::default().target_samples
    );
    // Built-ins still get filled in even with no [[presets]] at all.
    assert_eq!(
        loaded.presets.len(),
        super::super::model::built_in_presets().len()
    );
}

#[test]
fn library_panel_collapsed_state_round_trips_through_save_and_load() {
    let dir = TempDir::new("library-panel-collapsed");
    let path = dir.path().join("settings.toml");

    let mut collapsed = SettingsFile::default();
    collapsed.settings.library_panel_collapsed = true;
    save(&path, &collapsed).unwrap();
    assert!(load_or_default(&path).settings.library_panel_collapsed);

    let mut expanded = SettingsFile::default();
    expanded.settings.library_panel_collapsed = false;
    save(&path, &expanded).unwrap();
    assert!(!load_or_default(&path).settings.library_panel_collapsed);
}

#[test]
fn missing_library_panel_collapsed_key_defaults_to_expanded() {
    let dir = TempDir::new("library-panel-collapsed-missing-key");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "[settings]\nexposure = 1.0\n").unwrap();
    assert!(!load_or_default(&path).settings.library_panel_collapsed);
}

#[test]
fn editor_layout_settings_round_trip_through_save_and_load() {
    let dir = TempDir::new("editor-layout");
    let path = dir.path().join("settings.toml");

    let mut custom = SettingsFile::default();
    custom.settings.editor_dock_width = 620.0;
    custom.settings.editor_inspector_height = 310.0;
    custom.settings.editor_settings_collapsed = true;
    custom.settings.editor_inspector_collapsed = true;
    custom.settings.editor_remap_collapsed = true;
    custom.settings.editor_layout_touched = true;
    save(&path, &custom).unwrap();

    let loaded = load_or_default(&path);
    assert_eq!(loaded.settings.editor_dock_width, 620.0);
    assert_eq!(loaded.settings.editor_inspector_height, 310.0);
    assert!(loaded.settings.editor_settings_collapsed);
    assert!(loaded.settings.editor_inspector_collapsed);
    assert!(loaded.settings.editor_remap_collapsed);
    assert!(loaded.settings.editor_layout_touched);
}

#[test]
fn missing_editor_layout_keys_default_to_the_old_fixed_layout() {
    let dir = TempDir::new("editor-layout-missing-key");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "[settings]\nexposure = 1.0\n").unwrap();

    let loaded = load_or_default(&path);
    assert_eq!(
        loaded.settings.editor_dock_width,
        AppSettings::default().editor_dock_width
    );
    assert_eq!(
        loaded.settings.editor_inspector_height,
        AppSettings::default().editor_inspector_height
    );
    assert!(!loaded.settings.editor_settings_collapsed);
    assert!(!loaded.settings.editor_inspector_collapsed);
    assert!(!loaded.settings.editor_remap_collapsed);
    assert!(!loaded.settings.editor_layout_touched);
}

#[test]
fn solid_view_mode_round_trips_through_save_and_load() {
    let dir = TempDir::new("solid-view-mode");
    let path = dir.path().join("settings.toml");

    let mut both = SettingsFile::default();
    both.settings.solid_view_mode = 2;
    save(&path, &both).unwrap();
    assert_eq!(load_or_default(&path).settings.solid_view_mode, 2);

    let mut solid = SettingsFile::default();
    solid.settings.solid_view_mode = 0;
    save(&path, &solid).unwrap();
    assert_eq!(load_or_default(&path).settings.solid_view_mode, 0);
}

#[test]
fn missing_solid_view_mode_key_defaults_to_solid() {
    let dir = TempDir::new("solid-view-mode-missing-key");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "[settings]\nexposure = 1.0\n").unwrap();
    assert_eq!(load_or_default(&path).settings.solid_view_mode, 0);
}

#[test]
fn live_view_mode_round_trips_through_save_and_load() {
    let dir = TempDir::new("live-view-mode");
    let path = dir.path().join("settings.toml");

    let mut solid = SettingsFile::default();
    solid.settings.live_view_mode = 0;
    save(&path, &solid).unwrap();
    assert_eq!(load_or_default(&path).settings.live_view_mode, 0);

    let mut traced = SettingsFile::default();
    traced.settings.live_view_mode = 1;
    save(&path, &traced).unwrap();
    assert_eq!(load_or_default(&path).settings.live_view_mode, 1);
}

#[test]
fn missing_live_view_mode_key_defaults_to_path_traced() {
    let dir = TempDir::new("live-view-mode-missing-key");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "[settings]\nexposure = 1.0\n").unwrap();
    assert_eq!(load_or_default(&path).settings.live_view_mode, 1);
}

#[test]
fn settings_with_a_lit_model_lighting_rig_round_trip() {
    let dir = TempDir::new("lit-model");
    let path = dir.path().join("settings.toml");

    let mut custom = SettingsFile::default();
    custom.settings.lighting_rig = "ISO hemisphere".to_string();
    save(&path, &custom).unwrap();

    let loaded = load_or_default(&path);
    assert_eq!(loaded.settings.lighting_rig, "ISO hemisphere");
}

#[test]
fn save_then_load_round_trips_settings_and_presets() {
    let dir = TempDir::new("roundtrip");
    let path = dir.path().join("nested").join("settings.toml");

    let mut original = SettingsFile::default();
    original.settings.exposure = 1.65;
    original.settings.max_bounces = 20;
    original.settings.selected_material = "Ruby".to_string();
    original
        .upsert_user_preset(LightingPreset {
            name: "Golden Hour".to_string(),
            built_in: false,
            light_yaw_deg: 88.0,
            light_pitch_deg: 33.0,
            exposure: 1.15,
            lighting_rig: "D65 Daylight (5500K)".to_string(),
            camera_distance: 2.9,
            camera_yaw: None,
            camera_pitch: None,
            env_map_path: None,
            export_usable: false,
        })
        .unwrap();

    save(&path, &original).expect("save should succeed, creating parent dirs");
    assert!(path.exists());

    let loaded = load_or_default(&path);
    assert_eq!(loaded, original);
}

#[test]
fn save_overwrites_existing_file_atomically() {
    let dir = TempDir::new("overwrite");
    let path = dir.path().join("settings.toml");

    let first = SettingsFile::default();
    save(&path, &first).unwrap();

    let mut second = SettingsFile::default();
    second.settings.exposure = 2.0;
    save(&path, &second).unwrap();

    let loaded = load_or_default(&path);
    assert_eq!(loaded.settings.exposure, 2.0);
    // No leftover temp file.
    assert!(!path.with_extension("toml.tmp").exists());
}

/// An old settings FILE holding two workers loads with the first as the single
/// remote endpoint, and the next save no longer carries the retired list.
#[test]
fn an_old_file_with_two_workers_migrates_on_load_and_saves_without_the_list() {
    let dir = TempDir::new("two-workers");
    let path = dir.path().join("settings.toml");
    std::fs::write(
        &path,
        "[settings]\nexposure = 1.0\n\n\
             [[settings.remote_workers]]\nname = \"A\"\naddress = \"a.lan:7878\"\n\n\
             [[settings.remote_workers]]\nname = \"B\"\naddress = \"b.lan:7878\"\n",
    )
    .unwrap();
    let loaded = load_or_default(&path);
    let remote = loaded.settings.remote.as_ref().expect("first worker kept");
    assert_eq!(remote.connection.address, "a.lan:7878");
    assert_eq!(loaded.settings.legacy_remote_workers.len(), 0);

    save(&path, &loaded).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("remote_workers"), "{text}");
    assert!(!text.contains("b.lan"), "{text}");
    assert_eq!(
        load_or_default(&path).settings.remote,
        loaded.settings.remote
    );
}

#[test]
fn default_settings_path_is_non_empty_and_ends_with_settings_file_name() {
    let path = default_settings_path();
    assert!(path.to_string_lossy().ends_with("settings.toml"));
    assert!(path.components().any(|c| c.as_os_str() == APP_DIR_NAME));
}

/// [`migrate_legacy_settings_if_needed`] takes an explicit `new_path` precisely so
/// these tests can point it at a temp directory rather than a real `%APPDATA%`.
mod migration {
    use super::*;

    #[test]
    fn migrates_when_editor_settings_are_absent() {
        let dir = TempDir::new("migrate-absent");
        let old_path = dir.path().join("old-settings.toml");
        let new_path = dir.path().join("new-settings.toml");
        std::fs::write(&old_path, "[settings]\nexposure = 2.5\n").unwrap();

        copy_legacy_settings_if_absent(&old_path, &new_path);

        assert!(new_path.exists());
        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap(),
            std::fs::read_to_string(&old_path).unwrap()
        );
        // Copy, never move: the legacy file must still be there afterward.
        assert!(old_path.exists());
    }

    #[test]
    fn does_not_fire_when_editor_settings_already_exist() {
        let dir = TempDir::new("migrate-existing");
        let old_path = dir.path().join("old-settings.toml");
        let new_path = dir.path().join("new-settings.toml");
        std::fs::write(&old_path, "[settings]\nexposure = 2.5\n").unwrap();
        std::fs::write(&new_path, "[settings]\nexposure = 9.9\n").unwrap();

        copy_legacy_settings_if_absent(&old_path, &new_path);

        // Untouched -- the editor's own file must never be clobbered by a legacy
        // one, even though the legacy file exists and differs.
        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap(),
            "[settings]\nexposure = 9.9\n"
        );
    }

    #[test]
    fn survives_a_missing_legacy_file() {
        let dir = TempDir::new("migrate-no-legacy");
        let old_path = dir.path().join("does-not-exist.toml");
        let new_path = dir.path().join("new-settings.toml");

        copy_legacy_settings_if_absent(&old_path, &new_path);

        assert!(
            !new_path.exists(),
            "nothing to migrate from -> nothing written"
        );
    }

    #[test]
    fn survives_an_unreadable_legacy_file() {
        // A directory where a file is expected fails `fs::copy`, exercising the
        // "exists but unreadable" path distinctly from "absent".
        let dir = TempDir::new("migrate-unreadable-legacy");
        let old_path = dir.path().join("old-settings.toml");
        std::fs::create_dir_all(&old_path).unwrap();
        let new_path = dir.path().join("new-settings.toml");

        copy_legacy_settings_if_absent(&old_path, &new_path);

        assert!(
            !new_path.exists(),
            "a failed copy must not leave a partial/empty destination file"
        );
    }

    #[test]
    fn creates_the_destination_directory_when_missing() {
        let dir = TempDir::new("migrate-nested-dest");
        let old_path = dir.path().join("old-settings.toml");
        std::fs::write(&old_path, "[settings]\nexposure = 3.0\n").unwrap();
        let new_path = dir.path().join("nested").join("new-settings.toml");

        copy_legacy_settings_if_absent(&old_path, &new_path);

        assert!(new_path.exists());
    }
}

//! Load/save the settings TOML file.
//!
//! A settings file must never prevent the app from starting: every failure mode here
//! (missing file, unreadable file, corrupt/unparseable TOML) is caught and logged,
//! falling back to `SettingsFile::default()` rather than propagating an error.

use super::model::SettingsFile;
use std::{
    io,
    path::{Path, PathBuf},
};
use tracing::{info, warn};

// Named for THIS crate, not the public app this crate was copied from -- the two are
// separate binaries that can be installed side by side, and sharing one settings
// directory would mean whichever app last saved silently overwrites the other's file.
const APP_DIR_NAME: &str = "indicatrix-cut";
/// The app-dir name this crate's settings used to share with the public app before
/// `APP_DIR_NAME` was split off. Kept for exactly one purpose:
/// [`migrate_legacy_settings_if_needed`]'s one-time copy, so a machine that already
/// had the old app configured doesn't have the editor start completely blank.
const LEGACY_APP_DIR_NAME: &str = "indicatrix-cut";
const SETTINGS_FILE_NAME: &str = "settings.toml";

/// Resolves `app_dir_name/settings.toml` inside the platform config directory (not
/// next to the executable, to avoid requiring write access to the install directory).
/// Shared by [`default_settings_path`] and [`legacy_settings_path`] so the two paths
/// can never disagree about where the platform config directory itself lives.
///
/// - Windows: `%APPDATA%\<app_dir_name>\settings.toml`
/// - macOS: `~/Library/Application Support/<app_dir_name>/settings.toml`
/// - Linux/other Unix: `$XDG_CONFIG_HOME/<app_dir_name>/settings.toml`, falling back to
///   `~/.config/<app_dir_name>/settings.toml`
///
/// If none of the expected environment variables are set (unusual, but not
/// impossible), falls back to a `<app_dir_name>/settings.toml` path relative to the
/// current working directory rather than failing outright.
fn settings_path_under(app_dir_name: &str) -> PathBuf {
    platform_config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(app_dir_name)
        .join(SETTINGS_FILE_NAME)
}

/// This crate's own settings file path -- see [`settings_path_under`].
#[must_use]
pub fn default_settings_path() -> PathBuf {
    settings_path_under(APP_DIR_NAME)
}

/// Where the public app's settings file lives, in the SAME platform config directory
/// this crate's settings resolve under -- the one-time migration source. See
/// [`migrate_legacy_settings_if_needed`].
fn legacy_settings_path() -> PathBuf {
    settings_path_under(LEGACY_APP_DIR_NAME)
}

/// One-time carry-over of the public app's settings file into this crate's own,
/// now-separate one. Without this, the editor would start completely blank on a
/// machine that already had the old app configured -- losing remote workers, cert
/// directories, export paths, and presets.
///
/// Called once, right before [`load_or_default`], with `new_path` the editor's own
/// resolved settings path, threaded through rather than recomputed so tests can point
/// it at a temp directory.
///
/// - **Copies, never moves.** The legacy app is a separate, still-actively-used binary
///   reading the SAME file this copies FROM -- deleting or renaming it would break
///   that app. `std::fs::copy` duplicates bytes; it never touches the source.
/// - **Never overwrites an existing editor settings file.** Only fires into a
///   genuinely absent destination (checked first) -- a one-time bootstrap, not an
///   ongoing sync.
/// - **A failed copy must not stop the app starting.** Every failure mode is caught
///   and logged with `warn!`, falling through to whatever [`load_or_default`] does
///   with the still-absent destination.
pub fn migrate_legacy_settings_if_needed(new_path: &Path) {
    copy_legacy_settings_if_absent(&legacy_settings_path(), new_path);
}

/// The actual copy logic behind [`migrate_legacy_settings_if_needed`], with `old_path`
/// passed in explicitly so this module's tests can exercise every branch with
/// temp-directory paths rather than touching a real `%APPDATA%`.
fn copy_legacy_settings_if_absent(old_path: &Path, new_path: &Path) {
    if new_path.exists() {
        // Already has its own settings -- never overwrite it.
        return;
    }
    if !old_path.exists() {
        // The common case: nothing to migrate. Not even worth a `warn!`.
        return;
    }
    if let Some(parent) = new_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        warn!(
            "Could not create {} to migrate legacy indicatrix-cut settings into; \
             starting with defaults instead: {e}",
            parent.display()
        );
        return;
    }
    match std::fs::copy(old_path, new_path) {
        Ok(_) => info!(
            "Migrated settings from the legacy config at {} to {}.",
            old_path.display(),
            new_path.display()
        ),
        Err(e) => warn!(
            "Could not copy legacy settings from {} to {}; starting with defaults \
             instead: {e}",
            old_path.display(),
            new_path.display()
        ),
    }
}

#[cfg(target_os = "windows")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
}

/// Loads settings from `path`, falling back to defaults (with built-in presets) on
/// any failure -- missing file, unreadable file, or corrupt/unparseable TOML. Never
/// panics and never propagates an error: this is deliberately infallible so a broken
/// settings file can never block startup.
#[must_use]
pub fn load_or_default(path: &Path) -> SettingsFile {
    let mut file = match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(file) => file,
            Err(e) => {
                warn!(
                    "Settings file at {} is corrupt ({e}); falling back to defaults.",
                    path.display()
                );
                SettingsFile::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            info!(
                "No settings file at {} yet; using defaults.",
                path.display()
            );
            SettingsFile::default()
        }
        Err(e) => {
            warn!(
                "Could not read settings file at {} ({e}); falling back to defaults.",
                path.display()
            );
            SettingsFile::default()
        }
    };
    file.ensure_built_in_presets();
    file
}

/// Writes `settings` to `path` as pretty-printed TOML, creating the parent directory
/// if needed. Writes to a temporary sibling file and renames it into place so a crash
/// or power loss mid-write can never leave a half-written, corrupt settings file
/// behind -- the rename is the only step that can make the new content visible, and
/// `std::fs::rename` replaces an existing destination atomically on both Windows
/// (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`) and Unix.
pub fn save(path: &Path, settings: &SettingsFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(settings)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, toml_str)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, LightingPreset};
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
        assert_eq!(loaded, SettingsFile::default());
    }

    #[test]
    fn unreadable_directory_in_place_of_file_falls_back_to_defaults() {
        // A directory in place of a file makes `read_to_string` fail with something
        // other than `NotFound`, exercising the third fallback arm.
        let dir = TempDir::new("isdir");
        let path = dir.path().join("settings.toml");
        std::fs::create_dir_all(&path).unwrap();
        let loaded = load_or_default(&path);
        assert_eq!(loaded, SettingsFile::default());
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
}

//! Remembered settings: a small JSON file in the platform's configuration directory.
//!
//! Windows: `%APPDATA%\indicatrix-trainer\settings.json`; elsewhere `$XDG_CONFIG_HOME` or
//! `~/.config`. Loading is tolerant (a missing, unreadable or foreign file gives the defaults and
//! every value is range-checked), saving is atomic (write a temporary file, then rename).

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use indicatrix_update::UpdatePreferences;
use serde::{Deserialize, Serialize};

use super::models::is_known_tier;
use crate::{dataset::find_spectrum_entry, trainer_worker::TrainerCommand};

/// The window's minimum size (matches the Slint window).
pub const MIN_WINDOW: (f32, f32) = (1060.0, 720.0);
/// No remembered size is accepted above this (a corrupt file must not create a giant window).
const MAX_WINDOW: f32 = 16_384.0;
/// The mode a fresh worker starts in (Retarget).
const DEFAULT_MODE: i32 = 1;
/// The tier a fresh worker starts with.
const DEFAULT_TIER: &str = "M";
/// Directory name inside the configuration directory.
const APP_DIR: &str = "indicatrix-trainer";
/// Settings file name.
const FILE_NAME: &str = "settings.json";

/// Everything remembered between runs. A missing value means "use the default".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Logical window size `[width, height]`.
    pub window_size: Option<[f32; 2]>,
    /// Folder of the last weights checkpoint dialog.
    pub weights_dir: Option<String>,
    /// Folder of the last run-file dialog.
    pub stats_dir: Option<String>,
    /// Database the user picked (the default lookup is used when absent).
    pub db_path: Option<String>,
    /// 0 = Optimizer, 1 = Retarget.
    pub mode: Option<i32>,
    /// Model size code.
    pub tier: Option<String>,
    /// Fixed target material.
    pub target_material: Option<String>,
    /// Sample the target from the whole spectrum.
    pub universal_spectrum: Option<bool>,
    /// Cycle through the training split.
    pub corpus_mode: Option<bool>,
    /// Console filter: 0 = everything, 1 = warnings and errors, 2 = errors.
    pub log_filter: i32,
    /// The new-version check: consent, last check and skipped version (an older file without
    /// this key loads as "not asked yet").
    pub update: UpdatePreferences,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

impl Settings {
    /// Parses settings text; anything unreadable gives the defaults, bad values are dropped.
    pub fn parse(text: &str) -> Self {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitised)
            .unwrap_or_default()
    }

    /// Drops every value outside its range.
    fn sanitised(mut self) -> Self {
        self.window_size = self.window_size.filter(|[width, height]| {
            width.is_finite()
                && height.is_finite()
                && (MIN_WINDOW.0..=MAX_WINDOW).contains(width)
                && (MIN_WINDOW.1..=MAX_WINDOW).contains(height)
        });
        self.weights_dir = non_empty(self.weights_dir);
        self.stats_dir = non_empty(self.stats_dir);
        self.db_path = non_empty(self.db_path);
        self.mode = self.mode.filter(|mode| (0..=1).contains(mode));
        self.tier = self.tier.filter(|tier| is_known_tier(tier));
        self.target_material =
            non_empty(self.target_material).filter(|name| find_spectrum_entry(name).is_ok());
        self.log_filter = self.log_filter.clamp(0, 2);
        self
    }

    /// The settings as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Loads the settings file; the defaults when there is none.
    pub fn load() -> Self {
        settings_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Writes the settings file atomically.
    ///
    /// # Errors
    ///
    /// No configuration directory, or the file could not be written.
    pub fn save(&self) -> io::Result<()> {
        let path = settings_path()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
        write_atomic(&path, &self.to_json())
    }

    /// The commands that bring a fresh worker to the remembered configuration. `exists` tells
    /// whether a remembered database file is still there.
    pub fn startup_commands(&self, exists: impl Fn(&Path) -> bool) -> Vec<TrainerCommand> {
        let mut commands = Vec::new();
        if let Some(path) = self
            .db_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| exists(p))
        {
            commands.push(TrainerCommand::LoadDatabase(Some(path)));
        }
        if let Some(mode) = self.mode.filter(|&mode| mode != DEFAULT_MODE) {
            commands.push(TrainerCommand::SwitchMode(
                usize::try_from(mode).unwrap_or_default(),
            ));
        }
        if let Some(tier) = self.tier.as_deref().filter(|&tier| tier != DEFAULT_TIER) {
            commands.push(TrainerCommand::SwitchTier(tier.to_owned()));
        }
        if let Some(name) = &self.target_material {
            commands.push(TrainerCommand::SetTargetMaterial(name.clone()));
        }
        if self.universal_spectrum == Some(false) {
            commands.push(TrainerCommand::ToggleSpectrumMode);
        }
        if self.corpus_mode == Some(false) {
            commands.push(TrainerCommand::ToggleCorpusMode);
        }
        commands
    }
}

/// The application's configuration directory for the given platform and environment values.
pub fn config_dir_from(
    windows: bool,
    appdata: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let non_empty_path =
        |value: Option<OsString>| value.filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if windows {
        non_empty_path(appdata)?
    } else {
        non_empty_path(xdg_config_home)
            .or_else(|| non_empty_path(home).map(|h| h.join(".config")))?
    };
    Some(base.join(APP_DIR))
}

/// The settings file of this machine, when a configuration directory exists.
pub fn settings_path() -> Option<PathBuf> {
    config_dir_from(
        cfg!(windows),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .map(|dir| dir.join(FILE_NAME))
}

/// Writes `text` to `path` through a temporary sibling file, creating the folder first.
///
/// # Errors
///
/// Any I/O failure; the previous file is left untouched in that case.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        Settings {
            window_size: Some([1280.0, 840.0]),
            weights_dir: Some("C:/weights".into()),
            stats_dir: Some("C:/runs".into()),
            db_path: Some("C:/db/facet_diagrams.sqlite".into()),
            mode: Some(0),
            tier: Some("L".into()),
            target_material: Some("Diamond".into()),
            universal_spectrum: Some(false),
            corpus_mode: Some(true),
            log_filter: 1,
            update: UpdatePreferences {
                enabled: Some(true),
                last_check_unix: 1_790_000_000,
                skipped_version: Some("0.7.3".into()),
            },
        }
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = sample();
        assert_eq!(Settings::parse(&settings.to_json()), settings);
    }

    #[test]
    fn an_older_file_without_the_update_record_is_not_asked_yet() {
        let parsed = Settings::parse(r#"{"tier": "L", "log_filter": 1}"#);
        assert_eq!(parsed.update, UpdatePreferences::default());
        assert!(parsed.update.needs_consent());
        let partial = Settings::parse(r#"{"update": {"enabled": false}}"#);
        assert_eq!(partial.update.enabled, Some(false));
        assert_eq!(partial.update.last_check_unix, 0);
    }

    #[test]
    fn unreadable_text_gives_the_defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("not json"), Settings::default());
        assert_eq!(Settings::parse("[1, 2]"), Settings::default());
        assert_eq!(Settings::parse(r#"{"mode": "wide"}"#), Settings::default());
    }

    #[test]
    fn unknown_and_missing_keys_are_tolerated() {
        let parsed = Settings::parse(r#"{"tier": "XL", "future_key": 7}"#);
        assert_eq!(parsed.tier.as_deref(), Some("XL"));
        assert_eq!(parsed.mode, None);
    }

    #[test]
    fn out_of_range_values_are_dropped() {
        let parsed = Settings::parse(
            r#"{"window_size": [10, 10], "mode": 7, "tier": "XS", "target_material": "Unobtainium", "log_filter": 9, "db_path": "  "}"#,
        );
        assert_eq!(parsed.window_size, None);
        assert_eq!(parsed.mode, None);
        assert_eq!(parsed.tier, None);
        assert_eq!(parsed.target_material, None);
        assert_eq!(parsed.log_filter, 2);
        assert_eq!(parsed.db_path, None);
        let huge = Settings::parse(r#"{"window_size": [99999, 800]}"#);
        assert_eq!(huge.window_size, None);
    }

    #[test]
    fn config_dir_by_platform() {
        let some = |text: &str| Some(OsString::from(text));
        assert_eq!(
            config_dir_from(
                true,
                some("C:/Users/a/AppData/Roaming"),
                some("/x"),
                some("/h")
            ),
            Some(PathBuf::from("C:/Users/a/AppData/Roaming").join(APP_DIR))
        );
        assert_eq!(config_dir_from(true, None, some("/x"), some("/h")), None);
        assert_eq!(
            config_dir_from(false, None, some("/x"), some("/h")),
            Some(PathBuf::from("/x").join(APP_DIR))
        );
        assert_eq!(
            config_dir_from(false, None, some(""), some("/h")),
            Some(PathBuf::from("/h").join(".config").join(APP_DIR))
        );
        assert_eq!(config_dir_from(false, None, None, None), None);
    }

    #[test]
    fn startup_commands_cover_only_what_differs_from_the_defaults() {
        assert!(Settings::default().startup_commands(|_| true).is_empty());
        let commands = sample().startup_commands(|_| true);
        assert!(matches!(commands[0], TrainerCommand::LoadDatabase(Some(_))));
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::SwitchMode(0)))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::SwitchTier(t) if t == "L"))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::SetTargetMaterial(_)))
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::ToggleSpectrumMode))
        );
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::ToggleCorpusMode))
        );
    }

    #[test]
    fn a_vanished_database_is_not_requested() {
        let commands = sample().startup_commands(|_| false);
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, TrainerCommand::LoadDatabase(_)))
        );
    }

    #[test]
    fn atomic_write_creates_the_folder_and_replaces_the_file() {
        let dir =
            std::env::temp_dir().join(format!("indicatrix-trainer-test-{}", std::process::id()));
        let path = dir.join("nested").join(FILE_NAME);
        write_atomic(&path, "first").expect("first write");
        write_atomic(&path, "second").expect("second write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "second");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

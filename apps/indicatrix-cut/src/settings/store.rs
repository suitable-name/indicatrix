//! Load/save the settings TOML file.
//!
//! A settings file must never prevent the app from starting: every failure mode here
//! (missing file, unreadable file, corrupt/unparseable TOML) is caught and logged,
//! falling back to `SettingsFile::default()` rather than propagating an error.

use super::model::{SettingsFile, normalize_ui_scale_percent};
use std::{
    io,
    path::{Path, PathBuf},
};
use tracing::{info, warn};

// Named for THIS crate, not the public app this crate was copied from -- the two are
// separate binaries that can be installed side by side, and sharing one settings
// directory would mean whichever app last saved silently overwrites the other's file.
const APP_DIR_NAME: &str = "indicatrix-cut";
/// The public app's own app-dir name, kept for exactly one purpose:
/// [`migrate_legacy_settings_if_needed`]'s one-time copy, so a machine that already
/// has the public app configured doesn't have the editor start completely blank.
///
/// `"diagram-gui"` -- the old public viewer this editor (`indicatrix-cut`, née
/// `private/apps/diagram-editor`) superseded, before the 2026-09-07 suite-wide rename
/// to Indicatrix deleted it outright (see the rename map in project memory:
/// `apps/diagram-gui (old viewer) -> deleted; superseded by the editor`). This used
/// to be a no-op (both constants read `"indicatrix-cut"`), which made the migration
/// below silently copy a path onto itself and orphan any real `diagram-gui`
/// installation's settings on upgrade.
const LEGACY_APP_DIR_NAME: &str = "diagram-gui";
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

/// What happened while loading the settings file -- returned by [`load_with_outcome`]
/// alongside the (possibly-defaulted) [`SettingsFile`] so `gui::main_window` can toast
/// a corrupt-file recovery instead of the cutter's saved settings silently vanishing
/// with only a `tracing::warn!` nobody watching a release build's console ever sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsLoadOutcome {
    /// Loaded normally, or the file was genuinely absent (first run) -- nothing to
    /// report.
    Ok,
    /// The file existed but its content was unreadable or failed to parse as valid
    /// `SettingsFile` TOML. Renamed aside to `renamed_to`
    /// (`settings.toml.corrupt-<unix-seconds>`, see [`rename_aside`]) before this load
    /// fell back to defaults, so the broken file is never silently overwritten by the
    /// very next save -- it stays on disk for the cutter (or a bug report) to inspect.
    Corrupt {
        /// Where the corrupt file was renamed to, for the toast to name.
        renamed_to: PathBuf,
    },
}

/// Renames `path` aside to `<path>.corrupt-<unix-seconds>` so the next save never
/// silently overwrites the one piece of evidence explaining what went wrong. Returns
/// the new path on success; logs and returns `None` on failure (e.g. no write
/// permission on the containing directory) rather than blocking startup on a rename
/// that isn't the load itself.
fn rename_aside(path: &Path) -> Option<PathBuf> {
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut renamed = path.as_os_str().to_owned();
    renamed.push(format!(".corrupt-{unix_secs}"));
    let renamed = PathBuf::from(renamed);
    match std::fs::rename(path, &renamed) {
        Ok(()) => Some(renamed),
        Err(e) => {
            warn!(
                "Could not rename the corrupt settings file at {} aside to {}: {e}. \
                 Continuing with defaults; the corrupt file is still at its original path.",
                path.display(),
                renamed.display()
            );
            None
        }
    }
}

/// The defaults for a settings file that EXISTED but could not be used (corrupt, or
/// unreadable): [`SettingsFile::default`], except that its owner is no newcomer, so they
/// keep the full interface and get no welcome tour. Only an absent file is a new install.
fn defaults_for_existing_install() -> SettingsFile {
    let mut file = SettingsFile::default();
    file.settings.treat_as_existing_install();
    file
}

/// The saved interface scale in percent (`0` = Automatic), read straight from the settings
/// file before anything else starts.
///
/// The scale has to reach `SLINT_SCALE_FACTOR` before the first window exists, which is
/// earlier than the normal load (`load_with_outcome` runs while the main window is being
/// built). This reads only that one field, so it needs no model and never fails: a
/// missing, unreadable or unparseable file, a missing key or an unoffered value all
/// give `0`.
#[must_use]
pub fn peek_ui_scale_percent() -> u16 {
    peek_ui_scale_percent_at(&default_settings_path())
}

/// [`peek_ui_scale_percent`] for an explicit `path`, so the tests can point it anywhere.
fn peek_ui_scale_percent_at(path: &Path) -> u16 {
    std::fs::read_to_string(path).map_or(0, |text| ui_scale_percent_in(&text))
}

/// The `[settings] ui_scale_percent` value in the settings document `text`, normalised
/// (`0` when absent or not an integer).
fn ui_scale_percent_in(text: &str) -> u16 {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|document| {
            document
                .get("settings")
                .and_then(toml::Value::as_table)
                .and_then(|settings| settings.get("ui_scale_percent"))
                .and_then(toml::Value::as_integer)
        })
        .map_or(0, normalize_ui_scale_percent)
}

/// [`load_or_default`], plus what actually happened -- see [`SettingsLoadOutcome`].
/// Never panics and never propagates an error: this is deliberately infallible so a
/// broken settings file can never block startup.
#[must_use]
pub fn load_with_outcome(path: &Path) -> (SettingsFile, SettingsLoadOutcome) {
    let (mut file, outcome) = match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(file) => (file, SettingsLoadOutcome::Ok),
            Err(e) => {
                warn!(
                    "Settings file at {} is corrupt ({e}); falling back to defaults.",
                    path.display()
                );
                // Renamed BEFORE falling back to defaults: the very next save (which
                // happens on almost any interaction) would otherwise overwrite the
                // corrupt file with a fresh default one, losing the one piece of
                // evidence explaining what went wrong -- see this constructor's own
                // doc comment.
                let outcome = rename_aside(path).map_or(SettingsLoadOutcome::Ok, |renamed_to| {
                    SettingsLoadOutcome::Corrupt { renamed_to }
                });
                (defaults_for_existing_install(), outcome)
            }
        },
        // The ONE place a brand-new install is recognised: `SettingsFile::default()` is
        // the newcomer's start (the Simple interface, the welcome tour still to come).
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            info!(
                "No settings file at {} yet; using defaults.",
                path.display()
            );
            (SettingsFile::default(), SettingsLoadOutcome::Ok)
        }
        Err(e) => {
            warn!(
                "Could not read settings file at {} ({e}); falling back to defaults.",
                path.display()
            );
            (defaults_for_existing_install(), SettingsLoadOutcome::Ok)
        }
    };
    file.ensure_built_in_presets();
    // Files from before the single-remote-endpoint rule carry a `remote_workers`
    // list; fold it into the single endpoint (logged inside). The next save writes only `remote`.
    file.settings.migrate_legacy_remote_workers();
    (file, outcome)
}

/// Loads settings from `path`, falling back to defaults (with built-in presets) on
/// any failure -- missing file, unreadable file, or corrupt/unparseable TOML. Never
/// panics and never propagates an error: this is deliberately infallible so a broken
/// settings file can never block startup.
///
/// A thin wrapper over [`load_with_outcome`] for callers that only want the settings
/// themselves. The application's only load, `gui::main_window`'s startup one, wants the
/// full [`SettingsLoadOutcome`] to toast a corrupt-file recovery, so this is a
/// convenience for tests.
///
/// Reading the file belongs to startup only: once the `SettingsPersister` is seeded
/// from it, the persister's in-memory snapshot is the source of truth, and a caller
/// that re-read the file mid-session would miss changes still inside the persister's
/// debounce window.
#[cfg(test)]
#[must_use]
pub fn load_or_default(path: &Path) -> SettingsFile {
    load_with_outcome(path).0
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
mod tests;

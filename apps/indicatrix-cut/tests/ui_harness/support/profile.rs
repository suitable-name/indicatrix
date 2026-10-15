//! Indicatrix's side of the throw-away profile.
//!
//! The profile itself (fresh directory, environment redirect, working directory, the
//! guard that proves the settings file and the database lie inside it) is `slint-harness`'s
//! [`ProfileDir`], configured in `main.rs`. This module adds what is Indicatrix-specific on top
//! of it: what `settings.toml` looks like when a scenario starts ([`SettingsSeed`],
//! [`ProfileExt::seed_settings`]) and how the app's persisted data is wiped between launches
//! ([`ProfileExt::wipe_app_data`], including the move to a fresh working directory when an
//! earlier launch left the design library database locked).

use indicatrix_cut::harness_support;
use slint_harness::profile::{ProfileDir, abort_run};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

/// The live throw-away profile of the run.
pub type Profile = ProfileDir;

/// How many times [`ProfileExt::wipe_app_data`] had to move to a fresh working directory.
static FRESH_CWDS: AtomicUsize = AtomicUsize::new(0);

/// The working directory name the crate's profile starts in; fresh ones are `cwd-N`.
const CWD_DIR: &str = "cwd";

/// What `settings.toml` looks like when a scenario starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsSeed {
    /// A file holding `[settings]` and the answered update question: an existing install
    /// (Advanced UI, tour done, new-version check declined).
    ExistingInstall,
    /// No settings file: a first run (Simple UI, Welcome dialog).
    FirstRun,
    /// This exact file content.
    Raw(String),
}

/// Settings seeding and app-data wiping on the crate's [`ProfileDir`].
pub trait ProfileExt {
    /// Writes (or removes) `settings.toml` as `seed` says.
    fn seed_settings(&self, seed: &SettingsSeed);

    /// Deletes everything the app persisted (settings dir with autosaves and caches, the design
    /// library with its `-wal`/`-shm` files) so the next launch starts from nothing. The app must
    /// be shut down first. Only paths inside the profile are ever touched.
    fn wipe_app_data(&self);
}

impl ProfileExt for ProfileDir {
    fn seed_settings(&self, seed: &SettingsSeed) {
        let path = harness_support::settings_path();
        if !self.contains(&path) {
            abort_run(&format!(
                "refusing to seed settings outside the profile: {}",
                path.display()
            ));
        }
        let content = match seed {
            SettingsSeed::FirstRun => {
                let _ = fs::remove_file(&path);
                return;
            }
            // The new-version question is already answered with "Don't Check": no dialog, and
            // no network request in a test run.
            SettingsSeed::ExistingInstall => {
                "[settings]\n\n[settings.update_check]\nenabled = false\n".to_owned()
            }
            SettingsSeed::Raw(text) => text.clone(),
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| {
                abort_run(&format!("cannot create {}: {e}", parent.display()));
            });
        }
        fs::write(&path, content)
            .unwrap_or_else(|e| abort_run(&format!("cannot write {}: {e}", path.display())));
    }

    fn wipe_app_data(&self) {
        let settings = harness_support::settings_path();
        if let Some(dir) = settings.parent() {
            // Refuses a directory outside the profile and the profile root itself.
            let _ = self.wipe_dir(dir);
        }
        let database = harness_support::database_path();
        if self.contains(&database) {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut name = database.clone().into_os_string();
                name.push(suffix);
                let _ = self.wipe_file(&PathBuf::from(name));
            }
            // A connection a previous launch leaked (a thread or a timer still holding it) keeps
            // the file locked on Windows, and the next scenario would silently inherit its rows
            // (run 4: a catalogue at id 145, designs left ignored or deleted by earlier
            // scenarios). Move to a fresh working directory instead, so the next launch opens a
            // new, empty database, and say so.
            if database.exists() {
                let n = FRESH_CWDS.fetch_add(1, Ordering::Relaxed) + 1;
                let fresh = self.root().join(format!("{CWD_DIR}-{n}"));
                match fs::create_dir_all(&fresh).and_then(|()| std::env::set_current_dir(&fresh)) {
                    // Said once; `print_locked_summary` prints how often it happened.
                    Ok(()) if n == 1 => println!(
                        "slint-harness:   the database {} is still locked by an earlier launch; \
                         continuing in {} (later moves are counted at the end)",
                        database.display(),
                        fresh.display()
                    ),
                    Ok(()) => {}
                    Err(e) => abort_run(&format!(
                        "the database {} is locked and no fresh working directory could be \
                         entered: {e}",
                        database.display()
                    )),
                }
            }
        }
    }
}

/// The survivors / locked-database summary the old `Profile::finish` printed; `main.rs` calls it
/// from the harness teardown, before the crate deletes the profile.
pub fn print_shutdown_summary() {
    let survivors = harness_support::shutdowns_with_survivors();
    if survivors > 0 {
        println!(
            "slint-harness: {survivors} shutdown(s) left the main window or the design library \
             handle alive (see the 'after shutdown' lines above for the first ones)"
        );
    }
    let moves = FRESH_CWDS.load(Ordering::Relaxed);
    if moves > 0 {
        println!(
            "slint-harness: the database was still locked after {moves} shutdown(s); each time \
             the run continued in a fresh working directory"
        );
    }
}

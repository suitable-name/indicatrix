//! Native file dialogs (`rfd`), awaited on the Slint event loop so they stay on the main thread
//! (macOS requires it) without blocking the loop. One dialog at a time; no Slint object and no
//! `RefCell` borrow is held across the await.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use rfd::AsyncFileDialog;

use super::{Gui, UiMessage, worker_handle::ClosePhase};
use crate::{
    stats::TrainingRunStats,
    trainer_worker::{TrainerCommand, runner::checkpoint_file_name},
    ui_state::ProgressSnapshot,
};

/// File name offered by the Save Weights dialog.
pub fn default_weights_name(snapshot: Option<&ProgressSnapshot>) -> String {
    snapshot.map_or_else(
        || "indicatrix-weights.json".to_owned(),
        |s| checkpoint_file_name(s.mode, s.tier, s.iteration),
    )
}

/// File name offered by the Export Run dialog.
pub fn default_run_name(snapshot: Option<&ProgressSnapshot>) -> String {
    snapshot.map_or_else(
        || "indicatrix-run.json".to_owned(),
        |s| {
            format!(
                "indicatrix-run-{}-{}-iter{}.json",
                s.mode,
                s.tier.code_str().to_ascii_lowercase(),
                s.iteration
            )
        },
    )
}

/// The path with a `.json` extension when the user typed none.
pub fn ensure_json(path: PathBuf) -> PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension("json")
    }
}

/// The folder of a chosen file as text, for the settings.
pub fn parent_dir_text(path: &Path) -> Option<String> {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.to_string_lossy().into_owned())
}

impl Gui {
    /// Runs one dialog on the event loop and hands the chosen path to `done`.
    fn run_dialog(
        self: &Rc<Self>,
        dialog: AsyncFileDialog,
        save: bool,
        done: impl FnOnce(&Rc<Self>, PathBuf) + 'static,
    ) {
        if self.dialog_busy.replace(true) {
            return;
        }
        let gui = self.clone();
        let spawned = slint::spawn_local(async move {
            let picked = if save {
                dialog.save_file().await
            } else {
                dialog.pick_file().await
            };
            gui.dialog_busy.set(false);
            if let Some(file) = picked {
                done(&gui, file.path().to_path_buf());
            }
        });
        // Dropping the handle detaches the task; it runs to completion on the event loop.
        if spawned.is_err() {
            self.dialog_busy.set(false);
        }
    }

    /// The remembered folder of the weights (`true`) or run-file (`false`) dialogs, if it exists.
    fn start_dir(&self, weights: bool) -> Option<PathBuf> {
        let settings = self.settings.borrow();
        let dir = if weights {
            settings.weights_dir.as_deref()
        } else {
            settings.stats_dir.as_deref()
        };
        dir.map(PathBuf::from).filter(|p| p.is_dir())
    }

    fn remember_dir(&self, path: &Path, weights: bool) {
        if let Some(dir) = parent_dir_text(path) {
            self.update_settings(|s| {
                if weights {
                    s.weights_dir = Some(dir);
                } else {
                    s.stats_dir = Some(dir);
                }
            });
        }
    }

    fn dialog(&self, title: &str, weights_dir: bool) -> AsyncFileDialog {
        let dialog = AsyncFileDialog::new()
            .set_title(title)
            .add_filter("JSON file", &["json"]);
        match self.start_dir(weights_dir) {
            Some(dir) => dialog.set_directory(dir),
            None => dialog,
        }
    }

    /// Save Weights; `then_close` is the "Save and Close" path of the close prompt.
    pub(super) fn save_weights(self: &Rc<Self>, then_close: bool) {
        let name = default_weights_name(self.live.borrow().snapshot.as_ref());
        let dialog = self.dialog("Save Weights", true).set_file_name(name);
        self.run_dialog(dialog, true, move |gui, path| {
            gui.remember_dir(&path, true);
            if then_close {
                gui.phase.set(ClosePhase::SavingFirst);
                gui.send(TrainerCommand::Pause);
            }
            gui.send(TrainerCommand::SaveWeights(ensure_json(path)));
        });
    }

    pub(super) fn load_weights(self: &Rc<Self>) {
        let dialog = self.dialog("Load Weights", true);
        self.run_dialog(dialog, false, |gui, path| {
            gui.remember_dir(&path, true);
            gui.send(TrainerCommand::LoadWeights(
                path.to_string_lossy().into_owned(),
            ));
        });
    }

    pub(super) fn export_run(self: &Rc<Self>) {
        let name = default_run_name(self.live.borrow().snapshot.as_ref());
        let dialog = self.dialog("Export Run", false).set_file_name(name);
        self.run_dialog(dialog, true, |gui, path| {
            gui.remember_dir(&path, false);
            gui.send(TrainerCommand::ExportStats(ensure_json(path)));
        });
    }

    /// Import Run: the file is read on a background thread; the result arrives as a message.
    pub(super) fn import_run(self: &Rc<Self>) {
        let dialog = self.dialog("Import Run", false);
        self.run_dialog(dialog, false, |gui, path| {
            gui.remember_dir(&path, false);
            gui.with_c(|c| c.set_busy_text("Reading the run file…".into()));
            let sender = gui.msg_tx.clone();
            std::thread::spawn(move || {
                let result = TrainingRunStats::load_from_file(&path.to_string_lossy())
                    .map(Box::new)
                    .map_err(|error| error.to_string());
                let _ = sender.send(UiMessage::ImportLoaded { path, result });
            });
        });
    }

    pub(super) fn pick_database(self: &Rc<Self>) {
        let dialog = AsyncFileDialog::new()
            .set_title("Choose Library Database")
            .add_filter("SQLite database", &["sqlite", "sqlite3", "db"]);
        self.run_dialog(dialog, false, |gui, path| {
            let text = path.to_string_lossy().into_owned();
            gui.update_settings(|s| s.db_path = Some(text.clone()));
            gui.with_c(|c| {
                c.set_db_state(0);
                c.set_db_path(text.into());
            });
            gui.send(TrainerCommand::LoadDatabase(Some(path)));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weights_name_follows_the_checkpoint_convention() {
        let snapshot = ProgressSnapshot::default();
        let name = default_weights_name(Some(&snapshot));
        assert_eq!(
            name,
            checkpoint_file_name(snapshot.mode, snapshot.tier, snapshot.iteration)
        );
        assert_eq!(default_weights_name(None), "indicatrix-weights.json");
    }

    #[test]
    fn the_run_name_carries_mode_tier_and_iteration() {
        let snapshot = ProgressSnapshot {
            iteration: 12,
            ..ProgressSnapshot::default()
        };
        let name = default_run_name(Some(&snapshot));
        assert!(name.starts_with("indicatrix-run-"));
        assert!(name.ends_with("-m-iter12.json"));
        assert_eq!(default_run_name(None), "indicatrix-run.json");
    }

    #[test]
    fn json_is_added_only_when_no_extension_was_typed() {
        assert_eq!(ensure_json(PathBuf::from("a/b")), PathBuf::from("a/b.json"));
        assert_eq!(
            ensure_json(PathBuf::from("a/b.json")),
            PathBuf::from("a/b.json")
        );
        assert_eq!(
            ensure_json(PathBuf::from("a/b.bak")),
            PathBuf::from("a/b.bak")
        );
    }

    #[test]
    fn the_parent_folder_is_remembered_as_text() {
        let text = parent_dir_text(Path::new("some/dir/file.json")).expect("parent");
        assert!(text.contains("dir"));
        assert_eq!(parent_dir_text(Path::new("file.json")), None);
    }
}

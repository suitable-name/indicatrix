//! The commands of the training worker: how each [`TrainerCommand`] changes the run and which
//! events answers it, plus the loading of the design library.
//!
//! A child module of `trainer_worker.rs`, so it sees the worker's private state.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Instant,
};

use indicatrix_ai::model::{ModelTier, TrainingMode};

use super::{
    Flow, TrainerCommand, Worker,
    runner::{CPU_LABEL, DeviceChoice, GPU_LABEL, Hyperparams, RunConfig, TrainingRunner},
};
use crate::{
    dataset::{
        DEFAULT_MAX_TIERS_CAP, DEFAULT_SPLIT_SEED, DatasetSplit, find_db_path, load_db_designs_from,
    },
    ui_state::{DatasetInfo, DesignSplit, WorkerEvent, design_infos},
};

/// Longest list of skipped designs written to the log individually.
const SKIPPED_LOGGED: usize = 5;
/// Configuration commands remembered while no data set is loaded.
const PENDING_CAP: usize = 64;
/// The reason given to a file command that arrives before any data set loaded.
const NO_DATASET: &str = "no data set is loaded";

/// A design library read and split, ready to build a run on.
pub(super) struct Loaded {
    pub(super) info: DatasetInfo,
    pub(super) split: DatasetSplit,
}

/// Where the worker gets a data set from; production reads the database, tests use a fixture.
pub(super) type DatasetLoader = fn(Option<PathBuf>) -> Result<Loaded, String>;

/// The path as an absolute, display-ready string (no symlink resolution, no `\\?\` prefix).
pub(super) fn absolute_text(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Reads the design library at `path` (`None` = the default lookup) and splits it.
pub(super) fn load_dataset(path: Option<PathBuf>) -> Result<Loaded, String> {
    let path = match path {
        Some(path) => path,
        None => find_db_path().map_err(|e| e.to_string())?,
    };
    let records = load_db_designs_from(&path).map_err(|e| e.to_string())?;
    let split = DatasetSplit::from_records(&records, DEFAULT_SPLIT_SEED, DEFAULT_MAX_TIERS_CAP)
        .map_err(|e| format!("cannot build the data set: {e}"))?;
    let info = DatasetInfo {
        db_path: absolute_text(&path),
        designs: design_infos(&records, &split),
        train: split.train.len(),
        val: split.val.len(),
        test: split.test.len(),
    };
    Ok(Loaded { info, split })
}

/// Loads the first data set and builds the worker on it. On failure the error goes to the log
/// and into [`WorkerEvent::Dataset`], and the caller keeps waiting for a database.
pub(super) fn open_worker<'a>(
    path: Option<PathBuf>,
    events: &'a Sender<WorkerEvent>,
    is_active: &'a AtomicBool,
    loader: DatasetLoader,
    config: &RunConfig,
) -> Option<Worker<'a>> {
    let _ = events.send(WorkerEvent::Busy(Some("Loading the data set…".to_owned())));
    let built = loader(path).and_then(|loaded| {
        let runner = TrainingRunner::new(config.clone(), loaded.split)
            .map_err(|e| format!("cannot start: {e}"))?;
        Ok((loaded.info, runner))
    });
    let worker = match built {
        Ok((info, runner)) => {
            let mut worker = Worker {
                runner,
                events,
                is_active,
                loader,
                step_once: false,
                selected_entry: None,
                designs: info.designs.clone(),
                steps_per_sec: 0.0,
                rate_start: Instant::now(),
                rate_steps: 0,
                history: Vec::new(),
                logs: Vec::new(),
                weights_dirty: false,
                run_generation: 0,
            };
            worker.announce_dataset(info);
            Some(worker)
        }
        Err(reason) => {
            let _ = events.send(WorkerEvent::log(
                "ERROR",
                format!("Could not load the data set: {reason}"),
            ));
            let _ = events.send(WorkerEvent::Dataset(Err(reason)));
            None
        }
    };
    let _ = events.send(WorkerEvent::Busy(None));
    worker
}

/// Handles a command that arrives while no data set is loaded: a start is refused with one
/// error, file commands are answered with a failure, configuration commands wait in `pending`
/// for the data set, the rest is ignored with a warning.
pub(super) fn defer_without_dataset(
    command: TrainerCommand,
    pending: &mut Vec<TrainerCommand>,
    events: &Sender<WorkerEvent>,
) {
    let log = |level: &str, text: String| {
        let _ = events.send(WorkerEvent::log(level, text));
    };
    match command {
        TrainerCommand::Start | TrainerCommand::StepOnce => log(
            "ERROR",
            "Cannot train: no data set is loaded. Choose a database first.".to_owned(),
        ),
        TrainerCommand::SwitchMode(_)
        | TrainerCommand::SwitchTier(_)
        | TrainerCommand::SetTargetMaterial(_)
        | TrainerCommand::ToggleCorpusMode
        | TrainerCommand::ToggleSpectrumMode
        | TrainerCommand::SetHyperparams { .. } => {
            if pending.len() < PENDING_CAP {
                pending.push(command);
            }
        }
        TrainerCommand::SaveWeights(_) => {
            let _ = events.send(WorkerEvent::WeightsSaved(Err(NO_DATASET.to_owned())));
        }
        TrainerCommand::LoadWeights(_) => {
            let _ = events.send(WorkerEvent::WeightsLoaded(Err(NO_DATASET.to_owned())));
        }
        TrainerCommand::ExportStats(_) => {
            let _ = events.send(WorkerEvent::StatsExported(Err(NO_DATASET.to_owned())));
        }
        TrainerCommand::Pause
        | TrainerCommand::Reset
        | TrainerCommand::SelectDbDesign(_)
        | TrainerCommand::ToggleDevice => log(
            "WARN",
            format!("Ignored {command:?}: no data set is loaded"),
        ),
        // The caller consumes these.
        TrainerCommand::LoadDatabase(_) | TrainerCommand::Shutdown => {}
    }
}

const fn mode_from_index(index: usize) -> Option<TrainingMode> {
    match index {
        0 => Some(TrainingMode::Optimizer),
        1 => Some(TrainingMode::Retarget),
        _ => None,
    }
}

const REBUILD: &str = "Rebuilding the environments…";

impl Worker<'_> {
    pub(super) fn handle(&mut self, command: TrainerCommand) -> Flow {
        match command {
            TrainerCommand::Start => {
                self.is_active.store(true, Ordering::SeqCst);
                self.log("INFO", "Training started");
                self.send_snapshot();
            }
            TrainerCommand::Pause => {
                self.is_active.store(false, Ordering::SeqCst);
                self.log("INFO", "Training paused");
                self.send_snapshot();
            }
            TrainerCommand::StepOnce => self.step_once = true,
            TrainerCommand::Reset => self.busy(REBUILD, Self::reset),
            TrainerCommand::SaveWeights(path) => {
                self.busy("Saving weights…", |w| w.save_weights(&path));
            }
            TrainerCommand::LoadWeights(path) => {
                self.busy("Loading weights…", |w| w.load_weights(&path));
            }
            TrainerCommand::ExportStats(path) => {
                self.busy("Exporting the run…", |w| w.export_stats(&path));
            }
            TrainerCommand::SwitchMode(index) => {
                self.busy(REBUILD, |w| w.switch_mode(index));
            }
            TrainerCommand::SelectDbDesign(index) => self.select_db_design(index),
            TrainerCommand::ToggleDevice => {
                self.busy("Switching the compute device…", Self::toggle_device);
            }
            TrainerCommand::SetTargetMaterial(name) => {
                self.busy(REBUILD, |w| w.set_target_material(&name));
            }
            TrainerCommand::ToggleCorpusMode => self.busy(REBUILD, Self::toggle_corpus_mode),
            TrainerCommand::ToggleSpectrumMode => self.busy(REBUILD, Self::toggle_spectrum_mode),
            TrainerCommand::SwitchTier(text) => {
                self.busy(format!("Switching to size {text}…"), |w| {
                    w.switch_tier(&text);
                });
            }
            TrainerCommand::SetHyperparams {
                workers,
                rollout_size,
                mini_batch,
                ppo_epochs,
                learning_rate,
                clip_epsilon,
                gae_lambda,
            } => {
                let hp = Hyperparams {
                    workers,
                    rollout_size,
                    mini_batch,
                    ppo_epochs,
                    learning_rate,
                    clip_epsilon,
                    gae_lambda,
                };
                // Only a changed worker count rebuilds the environments.
                if workers == self.runner.hyperparams().workers {
                    self.set_hyperparams(&hp);
                } else {
                    self.busy(REBUILD, |w| w.set_hyperparams(&hp));
                }
            }
            TrainerCommand::LoadDatabase(path) => {
                self.busy("Loading the data set…", |w| w.load_database(path));
            }
            TrainerCommand::Shutdown => {
                self.is_active.store(false, Ordering::SeqCst);
                return Flow::Stop;
            }
        }
        Flow::Continue
    }

    /// Tells the UI about the data set the runner was just built on: the `Dataset` event, the log
    /// lines, the first design as the selected one, the first snapshot and the backend.
    pub(super) fn announce_dataset(&mut self, info: DatasetInfo) {
        let (train, val, test) = (info.train, info.val, info.test);
        let first = info
            .designs
            .iter()
            .position(|d| d.split != DesignSplit::Skipped);
        let db_path = info.db_path.clone();
        let _ = self.events.send(WorkerEvent::Dataset(Ok(info)));

        let split = self.runner.split();
        let max_tiers = split.max_tiers;
        let skipped = split.skipped.clone();
        self.log(
            "DB",
            format!(
                "Data set {db_path}: {train} train / {val} val / {test} test designs, padded to \
                 {max_tiers} tiers"
            ),
        );
        if val == 0 {
            self.log(
                "WARN",
                "validation split is empty: no validation will run and no generalization gap is reported",
            );
        }
        if !skipped.is_empty() {
            self.log(
                "WARN",
                format!("{} designs were skipped at load time", skipped.len()),
            );
            for s in skipped.iter().take(SKIPPED_LOGGED) {
                self.log("WARN", format!("skipped '{}': {}", s.title, s.reason));
            }
        }
        let hp = self.runner.hyperparams();
        self.log(
            "DB",
            format!(
                "Algorithm {} | pool: {} parallel envs | rollout {} | mini-batch {} | {} epochs | {}",
                self.runner.config().algo,
                hp.workers,
                hp.rollout_size,
                hp.mini_batch,
                hp.ppo_epochs,
                self.runner.device_label()
            ),
        );
        self.flush_warnings();
        match first {
            Some(index) => self.select_db_design(index),
            None => self.send_snapshot(),
        }
        // Tell the GUI which backend it actually got, so the device button never guesses.
        let label = self.runner.device_label().to_owned();
        let _ = self.events.send(WorkerEvent::DeviceChanged(Ok(label)));
    }

    /// Replaces the data set and with it the run: same configuration, a freshly initialised
    /// model. A failed load changes nothing.
    fn load_database(&mut self, path: Option<PathBuf>) {
        self.is_active.store(false, Ordering::SeqCst);
        let rebuilt = (self.loader)(path).and_then(|loaded| {
            let runner = TrainingRunner::new(self.runner.config().clone(), loaded.split)
                .map_err(|e| format!("cannot start: {e}"))?;
            Ok((loaded.info, runner))
        });
        match rebuilt {
            Ok((info, runner)) => {
                self.runner = runner;
                self.designs.clone_from(&info.designs);
                self.selected_entry = None;
                self.step_once = false;
                self.weights_dirty = false;
                self.restart_bookkeeping();
                self.log(
                    "MODEL",
                    "Model re-initialised for the new data set; statistics restarted",
                );
                self.announce_dataset(info);
            }
            Err(reason) => {
                self.log("ERROR", format!("Could not load the data set: {reason}"));
                let _ = self.events.send(WorkerEvent::Dataset(Err(reason)));
                self.send_snapshot();
            }
        }
    }

    fn reset(&mut self) {
        self.is_active.store(false, Ordering::SeqCst);
        match self.runner.reset() {
            Ok(()) => {
                self.restart_bookkeeping();
                self.log("RESET", "Environments and statistics reset; weights kept");
            }
            Err(e) => self.report_error("Reset failed", &e),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn load_weights(&mut self, path: &str) {
        let result = self.runner.load_checkpoint(Path::new(path));
        match &result {
            Ok(()) => {
                // The statistics belong to the model: the runner started them over, so the
                // metric history and rate counters of the replaced model start over too.
                self.restart_bookkeeping();
                self.weights_dirty = false;
                self.log(
                    "CHECKPOINT",
                    format!("Loaded weights from {path}; run statistics restarted"),
                );
            }
            Err(e) => self.log("ERROR", format!("Failed to load weights: {e}")),
        }
        let _ = self
            .events
            .send(WorkerEvent::weights_loaded_outcome(&result, path));
        self.send_snapshot();
    }

    fn save_weights(&mut self, path: &Path) {
        let outcome = self
            .runner
            .save_checkpoint(path)
            .map(|()| absolute_text(path))
            .map_err(|e| e.to_string());
        match &outcome {
            Ok(saved) => {
                self.weights_dirty = false;
                self.log("CHECKPOINT", format!("Saved weights checkpoint to {saved}"));
            }
            Err(e) => self.log(
                "ERROR",
                format!("Failed to save weights to {}: {e}", path.display()),
            ),
        }
        let _ = self.events.send(WorkerEvent::WeightsSaved(outcome));
        self.send_snapshot();
    }

    fn export_stats(&mut self, path: &Path) {
        let stats = self.runner.export_stats(&self.history, &self.logs);
        let outcome = stats
            .save_to_path(path)
            .map(|()| absolute_text(path))
            .map_err(|e| e.to_string());
        match &outcome {
            Ok(saved) => self.log("EXPORT", format!("Saved training telemetry to {saved}")),
            Err(e) => self.log(
                "ERROR",
                format!("Failed to export stats to {}: {e}", path.display()),
            ),
        }
        let _ = self.events.send(WorkerEvent::StatsExported(outcome));
    }

    fn select_db_design(&mut self, index: usize) {
        match self.runner.select_record(index) {
            Ok(entry) => {
                self.selected_entry = Some(entry);
                let selected = &self.runner.split().entries[entry];
                let (title, tier_count) = (selected.title.clone(), selected.design.tiers.len());
                self.log("INSPECTOR", format!("Active design: {title}"));
                let split = self
                    .designs
                    .get(index)
                    .map_or(DesignSplit::Train, |d| d.split);
                let _ = self.events.send(WorkerEvent::DesignSelected {
                    index,
                    title,
                    tier_count,
                    split,
                });
            }
            Err(e) => {
                self.selected_entry = None;
                self.log("WARN", format!("Cannot select design #{index}: {e}"));
                if let Some(info) = self.designs.get(index) {
                    let _ = self.events.send(WorkerEvent::DesignSelected {
                        index,
                        title: info.title.clone(),
                        tier_count: 0,
                        split: info.split,
                    });
                }
            }
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn set_target_material(&mut self, name: &str) {
        match self.runner.set_fixed_target(name) {
            Ok(()) => self.log("MATERIAL", format!("Target material set to {name}")),
            Err(e) => self.log("WARN", format!("Target material not changed: {e}")),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn toggle_corpus_mode(&mut self) {
        let on = !self.runner.config().corpus_mode;
        match self.runner.set_corpus_mode(on) {
            Ok(()) => self.log(
                "CONFIG",
                if on {
                    "Full corpus: the train split is cycled across workers"
                } else {
                    "Single stone focus: all workers train on the selected design"
                },
            ),
            Err(e) => self.report_error("Mode change failed", &e),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn toggle_spectrum_mode(&mut self) {
        let on = !self.runner.config().universal_spectrum;
        match self.runner.set_universal_spectrum(on) {
            Ok(()) => self.log(
                "CONFIG",
                if on {
                    format!(
                        "Universal spectrum: targets sampled from {}",
                        crate::ui_state::spectrum_text()
                    )
                } else {
                    "Fixed target material".to_owned()
                },
            ),
            Err(e) => self.report_error("Mode change failed", &e),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn set_hyperparams(&mut self, hp: &Hyperparams) {
        let result = self.runner.set_hyperparams(hp);
        match &result {
            Ok(()) => self.log("CONFIG", format!("Hyperparameters applied: {hp:?}")),
            Err(e) => self.log("WARN", format!("Hyperparameters refused: {e}")),
        }
        let _ = self.events.send(WorkerEvent::hyperparams_outcome(&result));
        self.flush_warnings();
        self.send_snapshot();
    }

    fn switch_mode(&mut self, index: usize) {
        let Some(mode) = mode_from_index(index) else {
            self.log("WARN", format!("Unknown model mode {index}"));
            return;
        };
        match self.runner.set_mode(mode) {
            Ok(()) => {
                self.restart_bookkeeping();
                self.log(
                    "MODE",
                    format!("Switched to the {mode} model; statistics restarted, weights kept"),
                );
            }
            Err(e) => self.report_error("Mode switch failed", &e),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn switch_tier(&mut self, text: &str) {
        let Some(tier) = ModelTier::from_str_loose(text) else {
            self.log("WARN", format!("Unknown model tier '{text}'"));
            return;
        };
        match self.runner.set_tier(tier) {
            Ok(()) => {
                self.restart_bookkeeping();
                // The model is initialised from the seed again: nothing is left to save.
                self.weights_dirty = false;
                self.log(
                    "MODEL",
                    format!(
                        "Model re-initialised as size {} (~{} params); statistics restarted",
                        tier.code_str(),
                        tier.param_count_approx()
                    ),
                );
            }
            Err(e) => self.report_error("Tier switch failed", &e),
        }
        self.flush_warnings();
        self.send_snapshot();
    }

    fn toggle_device(&mut self) {
        let target = if self.runner.device_label() == GPU_LABEL {
            DeviceChoice::Cpu
        } else {
            DeviceChoice::Gpu
        };
        match self.runner.set_device(target) {
            Ok(label) => {
                self.log(
                    "DEVICE",
                    format!("Backend is now {label}; optimizer state restarted"),
                );
                let _ = self
                    .events
                    .send(WorkerEvent::DeviceChanged(Ok(label.to_owned())));
            }
            Err(e) => {
                let current = if self.runner.device_label() == CPU_LABEL {
                    CPU_LABEL
                } else {
                    GPU_LABEL
                };
                self.log(
                    "ERROR",
                    format!("Backend switch failed, staying on {current}: {e}"),
                );
                let _ = self
                    .events
                    .send(WorkerEvent::DeviceChanged(Err(e.to_string())));
            }
        }
        self.flush_warnings();
    }
}

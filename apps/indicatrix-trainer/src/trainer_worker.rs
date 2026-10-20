//! The training worker thread of the GUI.
//!
//! The thread owns a [`TrainingRunner`] (see `runner.rs`, shared with the command-line trainer),
//! executes [`TrainerCommand`]s from the UI thread between training iterations, and reports
//! through [`WorkerEvent`]s. Every [`WorkerEvent::Metrics`] is a complete snapshot of the live run
//! state; every finished iteration also sends its [`WorkerEvent::Point`] for the charts.
//!
//! The worker trains with the runner's default algorithm (imitation of the finite-difference
//! teacher). The PPO figures of a snapshot stay at zero in an imitation run; its loss, direction
//! cosine, label count and evaluation rate travel in the metric points and go to the console log
//! (the periodic status line).
//!
//! Start-up: the thread loads the design library itself (the default lookup, see
//! `dataset::find_db_path`) and announces the result with [`WorkerEvent::Dataset`]; the window
//! does not wait for it. When the load fails the worker stays alive and idle, answering a start
//! with one error, until a [`TrainerCommand::LoadDatabase`] succeeds. The configuration commands
//! that arrive meanwhile (mode, tier, material, toggles, hyperparameters) are replayed in order
//! once the data set is there.
//!
//! Long commands (tier switch, environment rebuilds, device switch, checkpoint and telemetry
//! files, validation) are bracketed by [`WorkerEvent::Busy`] events.
//!
//! Lifecycle: the thread ends on [`TrainerCommand::Shutdown`] (clean) or when the UI drops its
//! command sender; an error or panic is caught and reported as [`WorkerEvent::Died`]. To make a
//! shutdown prompt the UI should clear the `is_active` flag before sending `Shutdown`: the
//! rollout loop checks the flag between environment steps, a validation pass that would start is
//! skipped once the flag is clear, and an update already in progress must finish before shutdown.

#[path = "runner.rs"]
pub mod runner;

#[path = "trainer_worker_commands.rs"]
mod commands;

use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use indicatrix_ai::model::{ModelTier, TrainingMode};

use crate::{
    stats::{IterationMetricPoint, describe_drops, rfc3339_utc_now},
    ui_state::{DesignInfo, ProgressSnapshot, ValidationSnapshot, WorkerEvent},
};
use commands::{DatasetLoader, defer_without_dataset, load_dataset};
use runner::{Algorithm, IterationReport, RunConfig, RunnerError, TrainingRunner};

/// Status line every this many iterations.
const STATUS_EVERY: usize = 20;
/// Window over which the throughput is measured.
const RATE_WINDOW: Duration = Duration::from_millis(500);
/// Metric points and log lines kept for the stats export.
const HISTORY_CAP: usize = 5000;

/// Command messages sent from the UI thread to the training worker thread.
#[derive(Debug)]
pub enum TrainerCommand {
    /// Resume / start continuous training.
    Start,
    /// Pause continuous training (takes effect between environment steps).
    Pause,
    /// Execute a single training iteration then halt.
    StepOnce,
    /// Reset the environments and statistics; the weights are kept.
    Reset,
    /// Save the network weights as a checkpoint at this path (answered by
    /// [`WorkerEvent::WeightsSaved`]; an existing file is replaced, the UI asks first).
    SaveWeights(PathBuf),
    /// Load a checkpoint; refused when its mode, tier or observation layout differs.
    LoadWeights(String),
    /// Write the full run telemetry (`TrainingRunStats`) to this path (answered by
    /// [`WorkerEvent::StatsExported`]).
    ExportStats(PathBuf),
    /// Switch between Optimizer (0) and Retarget (1).
    SwitchMode(usize),
    /// Select the design with this index in the record list of the loaded data set.
    SelectDbDesign(usize),
    /// Move the model between GPU and CPU, keeping the weights. Answered by
    /// [`WorkerEvent::DeviceChanged`].
    ToggleDevice,
    /// Set the fixed target material by name (used when the universal spectrum is off).
    SetTargetMaterial(String),
    /// Toggle between the full train split and single-stone focus.
    ToggleCorpusMode,
    /// Toggle between the universal material spectrum and the fixed target material.
    ToggleSpectrumMode,
    /// Switch the model size tier ("S", "M", "L", "XL", "XXL"); the model is re-initialised.
    SwitchTier(String),
    /// Replace the hyperparameters; validated as a whole, refused with a warning when invalid.
    SetHyperparams {
        /// Parallel environments (1..=[`runner::MAX_WORKERS`]).
        workers: usize,
        /// Environment steps per update.
        rollout_size: usize,
        /// Mini-batch size (imitation or PPO, whichever the run trains with).
        mini_batch: usize,
        /// Epochs per update (per round for imitation).
        ppo_epochs: usize,
        /// Imitation mini-batches trained per round; 0 trains `ppo_epochs` full epochs over the
        /// whole aggregate instead. No effect in a PPO run.
        updates_per_round: usize,
        /// Adam learning rate.
        learning_rate: f32,
        /// PPO clipping threshold (no effect in an imitation run).
        clip_epsilon: f32,
        /// GAE lambda (no effect in an imitation run).
        gae_lambda: f32,
    },
    /// Load a design library: `Some(path)` of a database file, `None` for the default lookup.
    /// Answered by [`WorkerEvent::Dataset`]. A new data set replaces the run (the model is
    /// re-initialised, the statistics start over); a failed load changes nothing.
    LoadDatabase(Option<PathBuf>),
    /// End the worker thread.
    Shutdown,
}

/// Spawns the dedicated training thread. It loads the design library itself.
pub fn spawn_trainer_worker(
    cmd_rx: Receiver<TrainerCommand>,
    event_tx: Sender<WorkerEvent>,
    is_active: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run_worker(
                &cmd_rx,
                &event_tx,
                &is_active,
                load_dataset,
                &RunConfig::new(TrainingMode::Retarget, ModelTier::Medium),
            )
        }));
        let death = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(message)) => Some(message),
            Err(payload) => Some(format!("panic: {}", panic_text(payload.as_ref()))),
        };
        if let Some(message) = death {
            is_active.store(false, Ordering::SeqCst);
            let _ = event_tx.send(WorkerEvent::Died(message));
        }
    })
}

/// The periodic status line: smoothed step reward, throughput, evaluation rate and the share of
/// proposals that changed the design, then the figures of the update from the iteration's metric
/// `point`. An `imitation` run shows the loss, the cosine between the composed direction and the
/// measured one, the labels of the round, the aggregate size and the label support of each of
/// the seven components; a PPO run the KL of the update and the critic's explained variance
/// (`-` when undefined).
fn status_line(
    iteration: usize,
    smoothed_reward: f32,
    steps_per_sec: f32,
    point: &IterationMetricPoint,
    imitation: bool,
) -> String {
    let rate = point
        .evaluations_per_sec()
        .map_or_else(String::new, |evals| format!(", {evals:.0} evals/s"));
    let accepted = point.accepted_frac * 100.0;
    if imitation {
        let cosine = point
            .train_cosine
            .map_or_else(|| "-".to_owned(), |c| format!("{c:+.2}"));
        // No update installed yet (the first iteration): the loss is a default, not a figure.
        let loss = if point.has_update() {
            format!("{:.3}", point.imitation_loss)
        } else {
            "-".to_owned()
        };
        let component_support = point
            .component_support
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("/");
        return format!(
            "Iter #{iteration}: step reward {smoothed_reward:+.3}, \
             {steps_per_sec:.0} steps/s{rate}, loss {loss}, cosine {cosine}, \
             {} labels this round ({} in the aggregate; support {component_support}), host ms pack/update/export/fwd {:.0}/{:.0}/{:.0}/{:.0}, accepted {accepted:.0}%",
            point.labels_this_round,
            point.labelled_samples,
            point.packing_ms,
            point.training_host_ms,
            point.weight_export_ms,
            point.rollout_forward_ms
        );
    }
    let explained = point
        .explained_variance
        .map_or_else(|| "-".to_owned(), |ev| format!("{ev:+.2}"));
    format!(
        "Iter #{iteration}: step reward {smoothed_reward:+.3}, {steps_per_sec:.0} steps/s{rate}, \
         KL {:.4}{}, accepted {accepted:.0}%, explained variance {explained}",
        point.approx_kl,
        if point.early_stopped {
            " (stopped early)"
        } else {
            ""
        }
    )
}

/// The tail of the status line that says which update the loss figures belong to and how long it
/// took: the update installed during the iteration is the one of the previous iteration in an
/// imitation run (it trained while this rollout ran), so the line names it. `waited` is how long
/// the worker stood still for it: near 0 the rollout is the bottleneck, large the update is.
/// Empty when no update was installed (the first iteration of a run).
fn update_note(report: &IterationReport) -> String {
    report.update_of_iteration.map_or_else(String::new, |of| {
        format!(
            ", update of iteration #{of} took {:.1}s (waited {:.1}s)",
            report.update_secs, report.update_wait_secs
        )
    })
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "unknown panic payload".to_owned())
}

/// Whether the worker loop goes on.
#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

struct Worker<'a> {
    runner: TrainingRunner,
    events: &'a Sender<WorkerEvent>,
    is_active: &'a AtomicBool,
    /// Where `LoadDatabase` gets its data set from.
    loader: DatasetLoader,
    step_once: bool,
    /// Data-set entry of the design the user selected in the library.
    selected_entry: Option<usize>,
    /// Every record of the loaded library with its split (the design picker's rows).
    designs: Vec<DesignInfo>,
    steps_per_sec: f32,
    rate_start: Instant,
    rate_steps: usize,
    history: Vec<IterationMetricPoint>,
    logs: Vec<(String, String, String)>,
    /// An update changed the weights since the last save, load or re-initialisation.
    weights_dirty: bool,
    /// Incremented whenever the run statistics start over.
    run_generation: u64,
}

fn run_worker(
    cmd_rx: &Receiver<TrainerCommand>,
    events: &Sender<WorkerEvent>,
    is_active: &AtomicBool,
    loader: DatasetLoader,
    config: &RunConfig,
) -> Result<(), String> {
    // Without a data set the worker only waits for `LoadDatabase`; configuration commands wait
    // in `pending` and run once the data set is there.
    let mut pending = Vec::new();
    let mut wanted = Some(None);
    let mut worker = loop {
        if let Some(path) = wanted.take()
            && let Some(worker) = commands::open_worker(path, events, is_active, loader, config)
        {
            break worker;
        }
        match cmd_rx.recv() {
            Ok(TrainerCommand::LoadDatabase(path)) => wanted = Some(path),
            Ok(TrainerCommand::Shutdown) | Err(_) => return Ok(()),
            Ok(command) => defer_without_dataset(command, &mut pending, events),
        }
    };
    for command in pending {
        if worker.handle(command) == Flow::Stop {
            return Ok(());
        }
    }

    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(command) => {
                    if worker.handle(command) == Flow::Stop {
                        return Ok(());
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if worker.is_active.load(Ordering::SeqCst) || worker.step_once {
            worker.run_iteration();
        } else {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Worker<'_> {
    fn log(&mut self, level: &str, message: impl Into<String>) {
        let message = message.into();
        if self.logs.len() >= HISTORY_CAP {
            self.logs.remove(0);
        }
        self.logs
            .push((rfc3339_utc_now(), level.to_owned(), message.clone()));
        let _ = self.events.send(WorkerEvent::log(level, message));
    }

    fn flush_warnings(&mut self) {
        for warning in self.runner.take_warnings() {
            self.log("WARN", warning);
        }
    }

    fn report_error(&mut self, what: &str, error: &RunnerError) {
        self.is_active.store(false, Ordering::SeqCst);
        self.log("ERROR", format!("{what}: {error}"));
    }

    /// Runs `work` between a `Busy(Some(text))` and a `Busy(None)` event.
    fn busy<T>(&mut self, text: impl Into<String>, work: impl FnOnce(&mut Self) -> T) -> T {
        let _ = self.events.send(WorkerEvent::Busy(Some(text.into())));
        let result = work(self);
        let _ = self.events.send(WorkerEvent::Busy(None));
        result
    }

    fn snapshot(&self) -> ProgressSnapshot {
        let runner = &self.runner;
        let stats = runner.stats();
        let config = runner.config();
        let hp = runner.hyperparams();
        let split = runner.split();
        let optics = stats.current.unwrap_or_default();
        let baseline = stats.baseline.unwrap_or_default();
        let gap = runner.generalization_gap();
        let validation = stats
            .last_validation
            .as_ref()
            .filter(|v| v.designs > 0)
            .map(|v| ValidationSnapshot {
                score: v.mean_final_score,
                brilliance: v.final_optics.brilliance,
                windowing: v.final_optics.windowing,
                gap,
            });
        let (src, tgt) = runner.display_materials().unzip();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "refractive index is stored as f64 and shown as f32; values are around 1 to 4"
        )]
        let src_n_d = src.map_or(0.0, |m| m.n_d as f32);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "refractive index is stored as f64 and shown as f32; values are around 1 to 4"
        )]
        let tgt_n_d = tgt.map_or(0.0, |m| m.n_d as f32);
        let tier_count = self
            .selected_entry
            .or_else(|| runner.display_entry())
            .map_or(0, |e| split.entries[e].design.tiers.len());
        let display_design_title = runner
            .display_entry()
            .map(|e| split.entries[e].title.clone())
            .unwrap_or_default();
        ProgressSnapshot {
            running: self.is_active.load(Ordering::SeqCst),
            mode: config.mode,
            tier: config.tier,
            iteration: stats.iteration,
            episode: stats.episodes,
            total_steps: stats.total_steps,
            steps_per_sec: self.steps_per_sec,
            mean_return: stats.mean_step_reward,
            best_score: stats.best.as_ref().map(|b| b.final_score),
            solid_closed: stats.solid_closed,
            brilliance: optics.brilliance,
            windowing: optics.windowing,
            extinction: optics.extinction,
            yield_pct: optics.yield_pct,
            baseline_brilliance: baseline.brilliance,
            baseline_windowing: baseline.windowing,
            baseline_extinction: baseline.extinction,
            baseline_yield_pct: baseline.yield_pct,
            validation,
            is_corpus_mode: config.corpus_mode,
            is_universal_spectrum: config.universal_spectrum,
            train_count: split.train.len(),
            val_count: split.val.len(),
            test_count: split.test.len(),
            tier_count,
            display_design_title,
            src_material_name: src.map(|m| m.name.clone()).unwrap_or_default(),
            src_n_d,
            tgt_material_name: tgt.map(|m| m.name.clone()).unwrap_or_default(),
            tgt_n_d,
            learning_rate: hp.learning_rate,
            ppo_clip: hp.clip_epsilon,
            gae_lambda: hp.gae_lambda,
            rollout_size: hp.rollout_size,
            batch_size: hp.mini_batch,
            ppo_epochs: hp.ppo_epochs,
            updates_per_round: config.imitation.updates_per_round,
            worker_count: hp.workers,
            weights_dirty: self.weights_dirty,
            run_generation: self.run_generation,
        }
    }

    fn send_snapshot(&self) {
        let _ = self
            .events
            .send(WorkerEvent::Metrics(Box::new(self.snapshot())));
    }

    fn restart_bookkeeping(&mut self) {
        self.history.clear();
        self.steps_per_sec = 0.0;
        self.rate_steps = 0;
        self.rate_start = Instant::now();
        self.run_generation += 1;
    }

    fn run_iteration(&mut self) {
        let force = std::mem::take(&mut self.step_once);
        let is_active = self.is_active;
        let mut keep_going = || force || is_active.load(Ordering::SeqCst);
        match self.runner.iterate(&mut keep_going) {
            Ok(Some(report)) => {
                self.weights_dirty = true;
                self.rate_steps += report.steps;
                let elapsed = self.rate_start.elapsed();
                if elapsed >= RATE_WINDOW {
                    self.steps_per_sec = self.rate_steps as f32 / elapsed.as_secs_f32();
                    self.rate_steps = 0;
                    self.rate_start = Instant::now();
                }
                let mut point = self.runner.metric_point(self.steps_per_sec);
                // The report has the exact iteration; the runner's point rebuilds it.
                point.update_of_iteration = report.update_of_iteration;
                self.flush_warnings();

                let iteration = self.runner.stats().iteration;
                // The record that matters is the largest score drop: the lowest final score
                // mostly says which design happened to start lowest (review TQ-19).
                if let Some(best) = &report.new_best_improvement {
                    let title = self.runner.split().entries[best.entry].title.clone();
                    self.log(
                        "BEST",
                        format!(
                            "New largest score drop {:.2} (from {:.2} to {:.2}) on '{title}'",
                            best.improvement(),
                            best.base_score,
                            best.final_score
                        ),
                    );
                }
                if iteration.is_multiple_of(self.runner.config().validation_every) {
                    // A pass that would only delay a pause or a close is left for the next
                    // interval; a single step validates as asked.
                    if force || self.is_active.load(Ordering::SeqCst) {
                        self.busy("Validating…", Self::validate);
                        // The point carries the fresh validation figures.
                        point = self.runner.metric_point(self.steps_per_sec);
                        point.update_of_iteration = report.update_of_iteration;
                    }
                } else if iteration.is_multiple_of(STATUS_EVERY) {
                    let reward = self.runner.stats().mean_step_reward;
                    let line = status_line(
                        iteration,
                        reward,
                        self.steps_per_sec,
                        &point,
                        self.runner.config().algo == Algorithm::Imitation,
                    );
                    self.log("OPT", format!("{line}{}", update_note(&report)));
                }
                if self.history.len() >= HISTORY_CAP {
                    self.history.remove(0);
                }
                self.history.push(point.clone());
                self.send_snapshot();
                let _ = self.events.send(WorkerEvent::Point(Box::new(point)));
            }
            Ok(None) => self.send_snapshot(),
            Err(error) => {
                self.report_error("Training iteration failed, training paused", &error);
                self.send_snapshot();
            }
        }
    }

    fn validate(&mut self) {
        let designs = self.runner.config().validation_designs;
        match self.runner.validate(designs) {
            Ok(Some(report)) if report.designs > 0 => {
                let gap = self
                    .runner
                    .generalization_gap()
                    .map_or_else(|| "n/a".to_owned(), |g| format!("{g:+.2}"));
                let drops = describe_drops(
                    report.mean_improvement,
                    report.mean_improvement_oracle,
                    report.mean_improvement_sampled,
                );
                self.log(
                    "VAL",
                    format!(
                        "Validation cohort: {} attempted ({} evaluated, {} failed), score {:.2}, {drops}, {:.2} evals/step, \
                         brilliance {:.1}%, windowing {:.1}%, generalization gap {gap}",
                        report.cases.len(),
                        report.designs,
                        report.failed,
                        report.mean_final_score,
                        report.mean_evaluations_per_step,
                        report.final_optics.brilliance,
                        report.final_optics.windowing,
                    ),
                );
                if let Some(reason) = &report.first_failure {
                    self.log("WARN", format!("validation skipped a design: {reason}"));
                }
            }
            Ok(Some(report)) => {
                self.log(
                    "WARN",
                    format!(
                        "validation could not evaluate any design ({})",
                        report.first_failure.as_deref().unwrap_or_default()
                    ),
                );
            }
            // An empty split is reported once when the data set loads.
            Ok(None) => {}
            Err(error) => self.report_error("Validation failed", &error),
        }
    }
}

#[cfg(test)]
#[path = "trainer_worker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "trainer_worker_protocol_tests.rs"]
mod protocol_tests;

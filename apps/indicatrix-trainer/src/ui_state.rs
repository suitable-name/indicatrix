//! Messages and snapshot types exchanged between the training worker and the Slint UI.
//!
//! The worker thread owns all training state. It talks to the UI thread only through
//! [`WorkerEvent`]: complete [`ProgressSnapshot`]s (every field is filled from the live run
//! state, never from placeholder defaults, so a snapshot sent as the acknowledgement of a command
//! cannot overwrite the display with invented numbers), log lines, the result of a device switch,
//! and the worker's death notice.
//!
//! Conventions: optical scores are `ObjectiveWeights::score` figures, `0..=100`, LOWER is better.
//! A figure that does not exist yet (no finished episode, no validation pass) is `None`, never
//! `0.0`: a zero score is a perfect stone and must not be shown as a measurement.

use indicatrix_ai::model::{ModelTier, TrainingMode};

use crate::{
    dataset::{
        DatasetSplit, DbDesignRecord, MINERAL_SPECTRUM, SPLIT_TEST_PERCENT, SPLIT_TRAIN_PERCENT,
        SPLIT_VAL_PERCENT,
    },
    stats::IterationMetricPoint,
};

/// A generalization gap above this reads as overfitting (a rule of thumb, not a test).
pub const OVERFIT_GAP_WARN: f32 = 3.0;

/// Goal of the validation figure of merit: the model's score drop divided by the oracle's.
pub const RATIO_GOAL: f32 = 0.8;

/// The split rule as one line, e.g. `75 / 15 / 10 % train / validation / test`.
#[must_use]
pub fn split_text() -> String {
    format!(
        "{SPLIT_TRAIN_PERCENT} / {SPLIT_VAL_PERCENT} / {SPLIT_TEST_PERCENT} % train / validation / test"
    )
}

/// The refractive-index range of the material spectrum, e.g. `n 1.43 – 2.90, 13 species`.
#[must_use]
pub fn spectrum_text() -> String {
    let (min, max) = MINERAL_SPECTRUM
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), e| {
            (lo.min(e.refractive_index), hi.max(e.refractive_index))
        });
    format!("n {min:.2} – {max:.2}, {} species", MINERAL_SPECTRUM.len())
}

/// Which part of the data set a library design belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignSplit {
    /// Trained on.
    Train,
    /// Validated on.
    Validation,
    /// Held out for the final test.
    Test,
    /// Left out at load time (unparsable, no tiers, too many tiers).
    Skipped,
}

impl DesignSplit {
    /// The word the UI shows (`DesignItem::split` of the Slint contract).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Train => "Train",
            Self::Validation => "Validation",
            Self::Test => "Test",
            Self::Skipped => "Skipped",
        }
    }
}

/// One library design as the design picker lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignInfo {
    /// Index in the record list (the argument of `TrainerCommand::SelectDbDesign`).
    pub index: usize,
    /// Library title.
    pub title: String,
    /// Attachment file name.
    pub filename: String,
    /// The part of the data set the design is in.
    pub split: DesignSplit,
}

/// The data set the worker loaded: where from, every design with its split, the split sizes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetInfo {
    /// Absolute path of the database file.
    pub db_path: String,
    /// Every record of the library, in record order.
    pub designs: Vec<DesignInfo>,
    /// Designs in the train split.
    pub train: usize,
    /// Designs in the validation split.
    pub val: usize,
    /// Designs in the test split.
    pub test: usize,
}

/// The picker rows for `records`: each record's split, or `Skipped` when it was not loaded.
/// A design listed in several splits (the smoke-test fixture) counts as the first of
/// train, validation, test.
#[must_use]
pub fn design_infos(records: &[DbDesignRecord], split: &DatasetSplit) -> Vec<DesignInfo> {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let part = match split.by_record.get(index).copied().flatten() {
                Some(entry) if split.train.contains(&entry) => DesignSplit::Train,
                Some(entry) if split.val.contains(&entry) => DesignSplit::Validation,
                Some(entry) if split.test.contains(&entry) => DesignSplit::Test,
                _ => DesignSplit::Skipped,
            };
            DesignInfo {
                index,
                title: record.title.clone(),
                filename: record.filename.clone(),
                split: part,
            }
        })
        .collect()
}

/// The first validation pass's figures; absent from the snapshot until a pass evaluated a design.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValidationSnapshot {
    /// Mean final score on the held-out designs (lower is better).
    pub score: f32,
    /// Mean final brilliance on the validation designs.
    pub brilliance: f32,
    /// Mean final windowing on the validation designs.
    pub windowing: f32,
    /// Mean training-episode score drop minus the mean drop of the sampled validation episodes
    /// (both sides sample their actions); `None` while no training episode has finished.
    pub gap: Option<f32>,
}

/// A point-in-time snapshot of the training run: counters, losses, optics, validation, config.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the Slint TrainingController properties; running, solid_closed and the two mode flags are independent switches"
)]
pub struct ProgressSnapshot {
    /// Whether the worker is training continuously right now (its `is_active` flag). The worker
    /// clears the flag itself when an error pauses it, so this is the only truthful source for
    /// the status pill and the toolbar button.
    pub running: bool,
    /// Model mode the worker is really running.
    pub mode: TrainingMode,
    /// Model size tier the worker is really running.
    pub tier: ModelTier,
    /// Training iterations done (one rollout and one update each).
    pub iteration: usize,
    /// Completed episode counter.
    pub episode: usize,
    /// Environment transition steps collected.
    pub total_steps: usize,
    /// Rollout throughput in environment steps per second.
    pub steps_per_sec: f32,
    /// Exponential moving average of the mean reward PER STEP (not an episode return).
    pub mean_return: f32,
    /// Lowest final score over all finished episodes (lower is better); `None` until one finished.
    pub best_score: Option<f32>,
    /// Whether the displayed stone's current state is a closed solid.
    pub solid_closed: bool,
    /// Tilt brilliance of the displayed stone's current state (higher is better).
    pub brilliance: f32,
    /// Windowing percentage of the displayed stone's current state.
    pub windowing: f32,
    /// Extinction percentage of the displayed stone's current state.
    pub extinction: f32,
    /// Volumetric yield percentage of the displayed stone's current state.
    pub yield_pct: f32,
    /// Brilliance of the displayed stone at the start of its episode.
    pub baseline_brilliance: f32,
    /// Windowing of the displayed stone at the start of its episode.
    pub baseline_windowing: f32,
    /// Extinction of the displayed stone at the start of its episode.
    pub baseline_extinction: f32,
    /// Yield of the displayed stone at the start of its episode.
    pub baseline_yield_pct: f32,
    /// Latest validation pass on held-out designs (deterministic policy); `None` until one
    /// evaluated a design.
    pub validation: Option<ValidationSnapshot>,
    /// Whether the full train split is cycled across workers (vs one focus stone).
    pub is_corpus_mode: bool,
    /// Whether the target material is sampled from the whole spectrum (vs one fixed material).
    pub is_universal_spectrum: bool,
    /// Designs in the train split.
    pub train_count: usize,
    /// Designs in the validation split.
    pub val_count: usize,
    /// Designs in the test split.
    pub test_count: usize,
    /// Tier count of the design the user selected (0 while unknown).
    pub tier_count: usize,
    /// Title of the design the optics panel describes (worker 0's stone). In corpus mode this is
    /// not the design the user selected, so the panel captions itself with it.
    pub display_design_title: String,
    /// Name of the displayed stone's source material.
    pub src_material_name: String,
    /// Refractive index (sodium D) of the source material.
    pub src_n_d: f32,
    /// Name of the displayed stone's target material.
    pub tgt_material_name: String,
    /// Refractive index (sodium D) of the target material.
    pub tgt_n_d: f32,
    /// Adam learning rate in effect.
    pub learning_rate: f32,
    /// PPO clipping threshold in effect.
    pub ppo_clip: f32,
    /// GAE lambda in effect.
    pub gae_lambda: f32,
    /// Environment steps per update.
    pub rollout_size: usize,
    /// Mini-batch size in effect.
    pub batch_size: usize,
    /// Epochs per update in effect (per round for imitation).
    pub ppo_epochs: usize,
    /// Imitation mini-batches trained per round in effect (0: full epochs over the aggregate,
    /// see `ppo_epochs`).
    pub updates_per_round: usize,
    /// Parallel environments.
    pub worker_count: usize,
    /// True once an update changed the weights since the last save, load or re-initialisation
    /// (closing the window then asks first).
    pub weights_dirty: bool,
    /// Counts the restarts of the run statistics (reset, mode or tier switch, weights load, new
    /// data set). The UI clears its chart series when the value changes.
    pub run_generation: u64,
}

impl Default for ProgressSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            mode: TrainingMode::Retarget,
            tier: ModelTier::Medium,
            iteration: 0,
            episode: 0,
            total_steps: 0,
            steps_per_sec: 0.0,
            mean_return: 0.0,
            best_score: None,
            solid_closed: true,
            brilliance: 0.0,
            windowing: 0.0,
            extinction: 0.0,
            yield_pct: 0.0,
            baseline_brilliance: 0.0,
            baseline_windowing: 0.0,
            baseline_extinction: 0.0,
            baseline_yield_pct: 0.0,
            validation: None,
            is_corpus_mode: true,
            is_universal_spectrum: true,
            train_count: 0,
            val_count: 0,
            test_count: 0,
            tier_count: 0,
            display_design_title: String::new(),
            src_material_name: String::new(),
            src_n_d: 0.0,
            tgt_material_name: String::new(),
            tgt_n_d: 0.0,
            learning_rate: 0.0,
            ppo_clip: 0.0,
            gae_lambda: 0.0,
            rollout_size: 0,
            batch_size: 0,
            ppo_epochs: 0,
            updates_per_round: 0,
            worker_count: 0,
            weights_dirty: false,
            run_generation: 0,
        }
    }
}

impl ProgressSnapshot {
    /// The text of the status pill: "Training" while the worker runs, "Paused" once it has done
    /// work and stopped (by the user or by an error), "Idle" before the first iteration.
    #[must_use]
    pub const fn status_text(&self) -> &'static str {
        if self.running {
            "Training"
        } else if self.iteration > 0 {
            "Paused"
        } else {
            "Idle"
        }
    }

    /// The model-mode index the header uses: 0 Optimizer, 1 Retarget.
    #[must_use]
    pub const fn mode_index(&self) -> i32 {
        match self.mode {
            TrainingMode::Optimizer => 0,
            TrainingMode::Retarget => 1,
        }
    }
}

/// Everything the worker thread sends to the UI thread.
#[derive(Debug)]
pub enum WorkerEvent {
    /// The complete current run state. The UI applies the latest one it received. Boxed: the
    /// snapshot is far larger than every other variant.
    Metrics(Box<ProgressSnapshot>),
    /// One log line. The UI appends EVERY one it received.
    Log {
        /// Level tag (`INFO`, `WARN`, `ERROR`, ...).
        level: String,
        /// Message text.
        message: String,
    },
    /// Result of `TrainerCommand::ToggleDevice`: `Ok(label)` with the backend now in use, or
    /// `Err(reason)` when the switch failed and the previous backend stays active.
    DeviceChanged(Result<String, String>),
    /// A `TrainerCommand::SetHyperparams` was accepted (the worker logs the values now in effect).
    HyperparamsApplied,
    /// A `TrainerCommand::SetHyperparams` was refused; the text is the reason. Nothing changed.
    HyperparamsRefused(String),
    /// Result of `TrainerCommand::LoadWeights`: `Ok(path)` that was loaded, or `Err(reason)`.
    WeightsLoaded(Result<String, String>),
    /// The worker thread has ended abnormally (error or panic); the text says why.
    Died(String),
    /// The metric point of the iteration that the preceding `Metrics` snapshot describes (sent
    /// after it, once per finished iteration). Boxed like the snapshot.
    Point(Box<IterationMetricPoint>),
    /// `Some(text)` before a long command starts, `None` after it ended (and its snapshot and
    /// result events were sent).
    Busy(Option<String>),
    /// Result of `TrainerCommand::SaveWeights`: `Ok(absolute path)` or `Err(reason)`.
    WeightsSaved(Result<String, String>),
    /// Result of `TrainerCommand::ExportStats`: `Ok(absolute path)` or `Err(reason)`.
    StatsExported(Result<String, String>),
    /// Result of loading the data set (at start-up and for `TrainerCommand::LoadDatabase`).
    Dataset(Result<DatasetInfo, String>),
    /// The design selected in the library changed (also sent once after a data set loaded, for
    /// its first loadable design). A design that cannot be selected (it was skipped at load time)
    /// reports `tier_count` 0 and its split, [`DesignSplit::Skipped`].
    DesignSelected {
        /// Index in the record list.
        index: usize,
        /// Library title.
        title: String,
        /// Tier count of the design.
        tier_count: usize,
        /// The part of the data set the design is in.
        split: DesignSplit,
    },
}

impl WorkerEvent {
    /// The event answering a hyperparameter change: applied or refused (with the reason).
    #[must_use]
    pub fn hyperparams_outcome<E: std::fmt::Display>(result: &Result<(), E>) -> Self {
        match result {
            Ok(()) => Self::HyperparamsApplied,
            Err(e) => Self::HyperparamsRefused(e.to_string()),
        }
    }

    /// The event answering a weights load: the path on success, the reason on failure.
    #[must_use]
    pub fn weights_loaded_outcome<E: std::fmt::Display>(
        result: &Result<(), E>,
        path: &str,
    ) -> Self {
        Self::WeightsLoaded(match result {
            Ok(()) => Ok(path.to_owned()),
            Err(e) => Err(e.to_string()),
        })
    }

    /// A log event.
    #[must_use]
    pub fn log(level: &str, message: impl Into<String>) -> Self {
        Self::Log {
            level: level.to_owned(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_follows_the_worker_state() {
        let mut snapshot = ProgressSnapshot::default();
        assert_eq!(snapshot.status_text(), "Idle");
        snapshot.running = true;
        assert_eq!(snapshot.status_text(), "Training");
        // An error pauses the worker itself: running is false again after real work was done.
        snapshot.running = false;
        snapshot.iteration = 7;
        assert_eq!(snapshot.status_text(), "Paused");
    }

    #[test]
    fn mode_index_matches_the_header_buttons() {
        let optimizer = ProgressSnapshot {
            mode: TrainingMode::Optimizer,
            ..ProgressSnapshot::default()
        };
        assert_eq!(optimizer.mode_index(), 0);
        let retarget = ProgressSnapshot {
            mode: TrainingMode::Retarget,
            ..ProgressSnapshot::default()
        };
        assert_eq!(retarget.mode_index(), 1);
    }

    #[test]
    fn a_fresh_snapshot_claims_no_measurements() {
        let snapshot = ProgressSnapshot::default();
        assert!(snapshot.best_score.is_none());
        assert!(snapshot.validation.is_none());
    }

    #[test]
    fn accepted_hyperparams_are_reported_as_applied() {
        let ok: Result<(), String> = Ok(());
        assert!(matches!(
            WorkerEvent::hyperparams_outcome(&ok),
            WorkerEvent::HyperparamsApplied
        ));
    }

    #[test]
    fn refused_hyperparams_carry_the_reason() {
        let err: Result<(), String> = Err("mini-batch exceeds rollout".to_owned());
        match WorkerEvent::hyperparams_outcome(&err) {
            WorkerEvent::HyperparamsRefused(reason) => {
                assert_eq!(reason, "mini-batch exceeds rollout");
            }
            other => panic!("expected HyperparamsRefused, got {other:?}"),
        }
    }

    fn record(title: &str) -> DbDesignRecord {
        DbDesignRecord {
            id: 1,
            title: title.to_owned(),
            filename: format!("{title}.asc"),
            asc_bytes: Vec::new(),
        }
    }

    #[test]
    fn the_picker_rows_carry_each_designs_split() {
        let split = DatasetSplit {
            by_record: vec![Some(0), None, Some(1), Some(2)],
            train: vec![0],
            val: vec![1],
            test: vec![2],
            ..DatasetSplit::fixture()
        };
        let records = [record("a"), record("b"), record("c"), record("d")];
        let rows = design_infos(&records, &split);
        let parts: Vec<_> = rows.iter().map(|r| r.split).collect();
        assert_eq!(
            parts,
            [
                DesignSplit::Train,
                DesignSplit::Skipped,
                DesignSplit::Validation,
                DesignSplit::Test
            ]
        );
        assert_eq!(rows[2].index, 2);
        assert_eq!(rows[2].title, "c");
        assert_eq!(rows[2].filename, "c.asc");
        // The fixture is in all three splits and counts as train.
        let fixture = design_infos(&[record("f")], &DatasetSplit::fixture());
        assert_eq!(fixture[0].split, DesignSplit::Train);
    }

    #[test]
    fn split_labels_match_the_slint_contract() {
        assert_eq!(DesignSplit::Train.label(), "Train");
        assert_eq!(DesignSplit::Validation.label(), "Validation");
        assert_eq!(DesignSplit::Test.label(), "Test");
        assert_eq!(DesignSplit::Skipped.label(), "Skipped");
    }

    #[test]
    fn the_split_and_spectrum_texts_come_from_the_tables() {
        assert_eq!(split_text(), "75 / 15 / 10 % train / validation / test");
        assert_eq!(spectrum_text(), "n 1.43 – 2.90, 13 species");
    }

    #[test]
    fn a_fresh_snapshot_is_clean_and_generation_zero() {
        let snapshot = ProgressSnapshot::default();
        assert!(!snapshot.weights_dirty);
        assert_eq!(snapshot.run_generation, 0);
    }

    #[test]
    fn weights_outcome_maps_both_branches() {
        let ok: Result<(), String> = Ok(());
        assert!(matches!(
            WorkerEvent::weights_loaded_outcome(&ok, "a.json"),
            WorkerEvent::WeightsLoaded(Ok(p)) if p == "a.json"
        ));
        let err: Result<(), String> = Err("tier differs".to_owned());
        assert!(matches!(
            WorkerEvent::weights_loaded_outcome(&err, "a.json"),
            WorkerEvent::WeightsLoaded(Err(e)) if e == "tier differs"
        ));
    }
}

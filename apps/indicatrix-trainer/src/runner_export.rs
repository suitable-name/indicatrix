//! What leaves the runner: the telemetry records and the checkpoints.

use std::path::Path;

use indicatrix_ai::{
    env::OBS_LAYOUT_VERSION,
    model::{Checkpoint, TrainingMode, bounded_log_std},
};

use super::{
    Algorithm, EpisodeSummary, RunnerError, RunnerStats, TrainingRunner, resumed_schedule,
};
use crate::stats::{IterationMetricPoint, STATS_VERSION, TrainingRunStats, rfc3339_utc_now};

impl TrainingRunner {
    /// The metrics point of the iteration that just finished.
    ///
    /// The loss figures are the imitation update's in imitation mode (the PPO figures are then
    /// zero) and the PPO update's otherwise; `grad_norm` follows the algorithm. `train_cosine`
    /// is the direction cosine of the last imitation update and `None` when none ran;
    /// `val_improvement_oracle` is the cached oracle drop of the last validation.
    #[must_use]
    pub fn metric_point(&self, steps_per_sec: f32) -> IterationMetricPoint {
        let s = &self.stats;
        let optics = s.current.unwrap_or_default();
        let val = s.last_validation.as_ref();
        let imitation = s.last_imitation.unwrap_or_default();
        let log_std = &self.net.log_std_raw;
        let mean_log_std =
            log_std.iter().map(|&raw| bounded_log_std(raw)).sum::<f32>() / log_std.len() as f32;
        IterationMetricPoint {
            iteration: s.iteration,
            episode: s.episodes,
            total_steps: s.total_steps,
            steps_per_sec,
            mean_step_reward: s.mean_step_reward,
            best_score: s.best.as_ref().map(|b| b.final_score),
            policy_loss: s.last_train.policy_loss,
            value_loss: s.last_train.value_loss,
            entropy: s.last_train.entropy,
            clip_fraction: s.last_train.clip_fraction,
            approx_kl: s.last_train.approx_kl,
            solid_closed: s.solid_closed,
            brilliance: optics.brilliance,
            windowing: optics.windowing,
            extinction: optics.extinction,
            yield_pct: optics.yield_pct,
            val_score: val.map(|v| v.mean_final_score),
            val_brilliance: val.map(|v| v.final_optics.brilliance),
            val_windowing: val.map(|v| v.final_optics.windowing),
            mean_step_reward_raw: s.mean_step_reward_raw,
            reward_scale: s.reward_scale,
            explained_variance: s.explained_variance,
            grad_norm: if s.last_imitation.is_some() {
                imitation.grad_norm
            } else {
                s.last_train.grad_norm
            },
            mean_log_std,
            early_stopped: s.last_train.early_stopped,
            accepted_frac: s.accepted_frac,
            episodes_finished: s.episodes_finished,
            episodes_improved_frac: s.episodes_improved_frac,
            mean_improvement: s.mean_improvement(),
            rej_unsafe: s.rej_unsafe,
            rej_unsolvable: s.rej_unsolvable,
            rej_not_closed: s.rej_not_closed,
            rej_girdle: s.rej_girdle,
            rej_manufacturability: s.rej_manufacturability,
            rej_not_improved: s.rej_not_improved,
            imitation_loss: imitation.loss,
            sens_mse: imitation.sens_mse,
            train_cosine: s.last_imitation.filter(|m| m.updates > 0).map(|m| m.cosine),
            gain_loss: imitation.gain_loss,
            labelled_samples: s.labelled_samples,
            labels_this_round: s.labels_this_round,
            oracle_gain_mean: s.oracle_gain_mean,
            val_improvement: val.map(|v| v.mean_improvement),
            val_improvement_sampled: val.and_then(|v| v.mean_improvement_sampled),
            val_improvement_oracle: s.val_oracle,
            val_evaluations_per_step: val
                .filter(|v| v.designs > 0)
                .map(|v| v.mean_evaluations_per_step),
            val_designs: val.map_or(0, |v| v.designs),
            rollout_secs: s.rollout_secs,
            label_secs: s.label_secs,
            update_secs: s.update_secs,
            evaluations_this_round: s.evaluations_this_round,
            workers: self.cfg.workers,
        }
    }

    /// The full telemetry record of the run so far.
    #[must_use]
    pub fn export_stats(
        &self,
        history: &[IterationMetricPoint],
        event_logs: &[(String, String, String)],
    ) -> TrainingRunStats {
        let s = &self.stats;
        let baseline = s.baseline.unwrap_or_default();
        let current = s.current.unwrap_or_default();
        let (title, filename) = self
            .display_entry()
            .map(|e| {
                let entry = &self.split.entries[e];
                (entry.title.clone(), entry.filename.clone())
            })
            .unwrap_or_default();
        let target_material = if self.cfg.mode == TrainingMode::Optimizer {
            "Own material (optimizer)".to_owned()
        } else if self.cfg.universal_spectrum {
            "Universal Spectrum (n=1.43-2.90)".to_owned()
        } else {
            self.cfg.fixed_target.clone()
        };
        TrainingRunStats {
            version: STATS_VERSION.to_owned(),
            timestamp_rfc3339: rfc3339_utc_now(),
            mode: match self.cfg.mode {
                TrainingMode::Optimizer => "Optimizer".to_owned(),
                TrainingMode::Retarget => "Retarget".to_owned(),
            },
            model_tier: self.cfg.tier.code_str().to_owned(),
            device: self.device_label.to_owned(),
            active_design_title: title,
            active_design_filename: filename,
            target_material,
            total_iterations: s.iteration,
            total_episodes: s.episodes,
            total_steps: s.total_steps,
            best_score: s.best.as_ref().map(|b| b.final_score),
            best_improvement: s.best_improvement.as_ref().map(EpisodeSummary::improvement),
            final_return: s.mean_step_reward,
            baseline_brilliance: baseline.brilliance,
            baseline_windowing: baseline.windowing,
            baseline_extinction: baseline.extinction,
            baseline_yield_pct: baseline.yield_pct,
            final_brilliance: current.brilliance,
            final_windowing: current.windowing,
            final_extinction: current.extinction,
            final_yield_pct: current.yield_pct,
            val_score: s.last_validation.as_ref().map(|v| v.mean_final_score),
            generalization_gap: self.generalization_gap(),
            train_designs_count: self.split.train.len(),
            val_designs_count: self.split.val.len(),
            test_designs_count: self.split.test.len(),
            split_seed: self.split.split_seed,
            algo: self.cfg.algo.to_string(),
            label_every: self.cfg.label_every,
            objectives: self.cfg.objectives.to_string(),
            explore_std: self.cfg.explore_std,
            max_step_deg: self.max_step_deg(),
            recent_rewards: s.reward_history.iter().copied().collect(),
            metrics_history: history.to_vec(),
            event_logs: event_logs.to_vec(),
        }
    }

    // ---- checkpoints ------------------------------------------------------------------------

    /// The current network with its metadata.
    #[must_use]
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint::new(
            self.cfg.mode,
            self.cfg.tier,
            OBS_LAYOUT_VERSION,
            self.split.max_tiers,
            self.stats.iteration as u64,
            self.cfg.seed,
            self.net.clone(),
        )
    }

    /// Writes a checkpoint to `path` (temporary file, then rename). Only the network is
    /// written: the aggregate of labelled states of an imitation run is not part of a
    /// checkpoint, so a resumed run starts with an empty one.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Weights`] on IO failure.
    pub fn save_checkpoint(&self, path: &Path) -> Result<(), RunnerError> {
        self.checkpoint().save(path)?;
        Ok(())
    }

    /// Loads a checkpoint, refusing a different mode, tier or observation layout. The iteration
    /// counter continues from the checkpoint. A PPO run also continues the learning-rate
    /// schedule position (a linear schedule is read as the steps still to run, so its end moves
    /// out by the steps already done); the Adam moments and the reward scale start over (the
    /// scale is re-estimated from the first rollout of the resumed run). An imitation run keeps
    /// its schedule as configured: the number of optimizer steps a round takes depends on the
    /// size of the aggregate, which is not saved. The run statistics (episodes, best score,
    /// validation) start over too, because they describe the replaced model, and the
    /// environments are rebuilt so that no episode mixes the two models. A differing
    /// `max_tiers` or seed is raised as a warning.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] for any mismatch or an unreadable file; nothing changes then.
    pub fn load_checkpoint(&mut self, path: &Path) -> Result<(), RunnerError> {
        let checkpoint = Checkpoint::load(path, OBS_LAYOUT_VERSION)?;
        checkpoint.ensure_mode(self.cfg.mode)?;
        if checkpoint.tier != self.cfg.tier {
            return Err(RunnerError::Data(format!(
                "checkpoint is for model tier {}, the run uses {}",
                checkpoint.tier.code_str(),
                self.cfg.tier.code_str()
            )));
        }
        self.trainer.load_weights(&checkpoint.network)?;
        self.net = checkpoint.network;
        self.refresh_rollout_model();
        let iteration = usize::try_from(checkpoint.iteration).unwrap_or(usize::MAX);
        // The statistics belong to the model: the previous model's episodes, best score, reward
        // history and validation are dropped. The displayed stone's optics describe the live
        // environments, not the model, so they stay.
        self.stats = RunnerStats {
            iteration,
            baseline: self.stats.baseline,
            current: self.stats.current,
            solid_closed: self.stats.solid_closed,
            ..RunnerStats::new()
        };
        // Fresh environments, so no episode mixes the previous weights with the loaded ones. The
        // rebuild is transactional: on failure the old slots stay usable and only a warning is
        // raised, because the cause is unrelated to the checkpoint.
        if let Err(e) = self.rebuild_slots() {
            self.warnings.push(format!(
                "environments could not be rebuilt after loading weights ({e}); the current \
                 episodes finish under the loaded model"
            ));
        }
        // The reward scale is not part of a checkpoint (review TQ-21): the resumed run
        // re-estimates it from its own first rollout, which its first update already uses. The
        // rebuild above resets it too; this covers the failed rebuild.
        self.normalizer.reset();
        if self.cfg.algo == Algorithm::Ppo {
            self.resume_ppo_schedule(iteration);
        }
        if checkpoint.max_tiers != self.split.max_tiers {
            self.warnings.push(format!(
                "checkpoint was trained with a {}-tier layout, this run pads to {}",
                checkpoint.max_tiers, self.split.max_tiers
            ));
        }
        if checkpoint.seed != self.cfg.seed {
            self.warnings.push(format!(
                "checkpoint seed {} differs from the run seed {}; sampling and shuffling \
                 continue from the run seed",
                checkpoint.seed, self.cfg.seed
            ));
        }
        Ok(())
    }

    /// The learning-rate schedule of a resumed PPO run continues where the checkpointed run
    /// stopped: one optimizer step per mini-batch, `epochs_per_batch` passes per iteration (an
    /// upper bound when the KL stop ended epochs early). A linear schedule counts the steps
    /// still to run, so its end moves out by the steps done; otherwise the resumed run would
    /// start at (or past) the end of the decay.
    fn resume_ppo_schedule(&mut self, iteration: usize) {
        let mini_batches = self
            .cfg
            .rollout_size
            .div_ceil(self.cfg.ppo.mini_batch_size.max(1));
        let steps = (iteration as u64)
            .saturating_mul(self.cfg.ppo.epochs_per_batch as u64)
            .saturating_mul(mini_batches as u64);
        self.trainer.set_update_count(steps);
        let schedule = resumed_schedule(self.cfg.ppo.lr_schedule, steps);
        self.cfg.ppo.lr_schedule = schedule;
        self.trainer.config.lr_schedule = schedule;
    }
}

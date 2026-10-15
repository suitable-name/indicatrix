//! The rollout and the update of one training iteration, for both algorithms.
//!
//! A rollout is a series of passes; a pass steps the first `n` slots once each (`n` is the slot
//! count, or what is left of the rollout size). Every pass has the same shape:
//!
//! 1. **Batched forward** (serial). The flat observations of the `n` slots go through the
//!    rollout copy of the Burn model in ONE call (`BurnSetAttention::forward_host`), on the
//!    training device. The numbers a rollout acts on are therefore the device's; validation and
//!    the checkpoints use the pure-Rust twin, which is exported after every update.
//! 2. **Sampling** (serial, in slot order, with the runner's own generator, so the random stream
//!    does not depend on anything else): the policy's own draw in PPO mode, the mean plus
//!    exploration noise on the free tiers in imitation mode; then squash and projection.
//! 3. **Phase 1, probes** (parallel). Every slot flagged for a label contributes one task per
//!    free tier (the finite-difference probe of that tier). Threads take the tasks from a shared
//!    counter, so a 24-tier slot no longer holds up a 4-tier one.
//! 4. **Phase 2, labels and steps** (parallel). Each slot, taken one at a time from a shared
//!    iterator, finishes its label (the oracle step along the probed slopes, a pure computation
//!    on the state BEFORE the step) and then takes its line-search step.
//! 5. **Finish** (serial, in slot order): rollout buffer or label aggregate, statistics, the
//!    seed of the next episode of a slot that ended.
//! 6. **Respawn** (parallel): the next episodes of the slots that ended are built on the pool
//!    (an environment costs a base evaluation plus the solid and girdle measurements, as much
//!    as a step) and installed serially in slot order.
//!
//! Each task and each slot writes its own result and the serial parts run in slot order, so the
//! outcome of a rollout does not depend on the thread count.

use std::time::Instant;

use indicatrix_ai::{
    env::{
        ContinuousTierAction, GemstoneObservation, Step, TeacherLabel, TierSensitivity,
        assemble_label, score_slopes,
    },
    model::{GaussianPolicy, GemstoneSetAttentionNetwork},
    training::{
        EpisodeEnd, ImitationMetrics, TrainBatch, TrainError, TrainingMetrics, TrajectoryStep,
    },
};

use super::{
    Algorithm, EpisodeSummary, IterationReport, Optics, RolloutStats, RunConfig, RunnerError, Slot,
    TrainingRunner, build_slot,
    imitation::{LabelledSample, explore_raw},
    pool,
    teacher::wants_label,
};

/// Everything the parallel stepping of a slot needs from the serial sampling.
struct PendingStep {
    /// Which real tiers the environment moves.
    free: Vec<bool>,
    /// The raw (pre-squash) action that was drawn.
    raw: Vec<f32>,
    /// Log-probability of `raw` under the sampling policy (PPO mode only; 0 otherwise).
    log_prob: f32,
    /// The critic's value of the state (PPO mode only).
    value: f32,
    /// The squashed and projected action the environment is stepped with.
    action: ContinuousTierAction,
    /// The flat observation the action was chosen in.
    state: Vec<f32>,
    /// Whether this sample gets a finite-difference label (see `wants_label`).
    labelled: bool,
}

/// What phase 1 measured for one labelled slot.
struct ProbeSet {
    /// One entry per tier of the design; `None` for a tier that is not free or whose probes
    /// both failed.
    tiers: Vec<Option<TierSensitivity>>,
    /// Candidate evaluations the probes cost.
    evaluations: usize,
}

/// What phase 2 returns for one slot.
struct SlotOutcome {
    step: Step<GemstoneObservation>,
    /// The finished label of a flagged slot.
    label: Option<TeacherLabel>,
}

/// A slot whose episode ended in this pass: where its next episode starts in the train split
/// and the seed it is drawn from (drawn serially, in slot order, so the random stream does not
/// depend on how the respawn is scheduled).
struct Respawn {
    w: usize,
    start: usize,
    seed: u64,
}

/// The probe size of `slot`: the configured one, or the step limit.
fn probe_size(cfg: &RunConfig, slot: &Slot) -> f64 {
    cfg.label_h_deg.unwrap_or_else(|| slot.env.max_step_deg())
}

/// Phase 1: probes every free tier of every flagged slot as one task each, on a pool of threads
/// pulling tasks from a shared counter. Returns the probes of each flagged slot (`None` for a
/// slot that is not flagged), grouped by tier index.
fn probe_phase(slots: &[Slot], pending: &[PendingStep], cfg: &RunConfig) -> Vec<Option<ProbeSet>> {
    let tasks: Vec<(usize, usize)> = pending
        .iter()
        .enumerate()
        .filter(|(_, p)| p.labelled)
        .flat_map(|(w, _)| slots[w].env.free_tiers().iter().map(move |&tier| (w, tier)))
        .collect();
    let sizes: Vec<f64> = slots.iter().map(|slot| probe_size(cfg, slot)).collect();
    let probes = pool::run_indexed(tasks.len(), pool::pass_threads(tasks.len()), |task| {
        let (w, tier) = tasks[task];
        slots[w].env.probe_tier(tier, sizes[w])
    });
    let mut sets: Vec<Option<ProbeSet>> = pending
        .iter()
        .zip(slots)
        .map(|(p, slot)| {
            p.labelled.then(|| ProbeSet {
                tiers: vec![None; slot.obs.tiers.len()],
                evaluations: 0,
            })
        })
        .collect();
    for (&(w, tier), probe) in tasks.iter().zip(probes) {
        if let Some(set) = &mut sets[w] {
            set.tiers[tier] = probe.sensitivity;
            set.evaluations += probe.evaluations;
        }
    }
    sets
}

/// Phase 2: for every slot, finishes the label of a flagged one (the oracle step along the
/// probed score slopes, computed on the state before the step) and takes the line-search step.
/// Slots are handed out one at a time to the pool, so the cost of a slot (a labelled many-tier
/// slot costs several times an unlabelled one) does not unbalance the threads.
fn step_phase(
    slots: &mut [Slot],
    pending: &[PendingStep],
    sets: &[Option<ProbeSet>],
    scales: &[f64],
) -> Vec<SlotOutcome> {
    let threads = pool::pass_threads(slots.len());
    pool::run_items(slots, threads, |w, slot| {
        let label = sets[w].as_ref().map(|set| {
            let oracle = slot.env.oracle_outcome(&score_slopes(&set.tiers), scales);
            assemble_label(set.tiers.clone(), set.evaluations, oracle)
        });
        let step = slot.env.step_scaled(&pending[w].action, scales);
        SlotOutcome { step, label }
    })
}

impl TrainingRunner {
    /// Steps 1 and 2 of a pass: the batched forward over the first `n` slots, then the serial
    /// sampling in slot order. `base` is the number of samples collected before the pass.
    fn prepare_pass(&mut self, n: usize, base: usize) -> Result<Vec<PendingStep>, RunnerError> {
        let max_tiers = self.split.max_tiers;
        let mut states = Vec::with_capacity(n);
        let mut n_tiers = Vec::with_capacity(n);
        for slot in &self.slots[..n] {
            states.push(slot.obs.try_flatten(max_tiers)?);
            n_tiers.push(slot.obs.tiers.len());
        }
        let batch = self.rollout_model.forward_host(&states, &n_tiers)?;
        if batch.means.len() != n || batch.log_stds.len() != n || batch.values.len() != n {
            return Err(RunnerError::Train(TrainError::ShapeMismatch(format!(
                "the batched forward returned {} means, {} log-stds and {} values for {n} slots",
                batch.means.len(),
                batch.log_stds.len(),
                batch.values.len()
            ))));
        }
        let rows = batch
            .means
            .into_iter()
            .zip(batch.log_stds)
            .zip(batch.values);
        let mut pending = Vec::with_capacity(n);
        for (w, (state, ((mean, log_std), value))) in states.into_iter().zip(rows).enumerate() {
            let labelled = wants_label(self.cfg.label_every, base, w, self.label_phase);
            let policy = GaussianPolicy { mean, log_std };
            pending.push(self.sample_step(w, state, &policy, value, labelled));
        }
        Ok(pending)
    }

    /// Draws, squashes and projects the action of slot `w` from the batched `policy`.
    fn sample_step(
        &mut self,
        w: usize,
        state: Vec<f32>,
        policy: &GaussianPolicy,
        value: f32,
        labelled: bool,
    ) -> PendingStep {
        let slot = &self.slots[w];
        // Pinned and flat tiers never reach the environment: they are neither sampled nor scored.
        let free = slot.obs.free_tier_mask();
        let (raw, log_prob) = match self.cfg.algo {
            Algorithm::Ppo => policy.sample_raw_masked(&mut self.rng, &free),
            Algorithm::Imitation => (
                explore_raw(&policy.mean, &free, self.cfg.explore_std, &mut self.rng),
                0.0,
            ),
        };
        let max_step = slot.env.max_step_deg();
        let mut action = GaussianPolicy::squash(&raw, max_step);
        GemstoneSetAttentionNetwork::project_actions(
            &mut action,
            &slot.obs.tiers,
            slot.obs.global.target_critical_angle_deg,
            max_step,
        );
        PendingStep {
            free,
            raw,
            log_prob,
            value,
            action,
            state,
            labelled,
        }
    }

    /// Steps 3 and 4 of a pass: the probes of the flagged slots, then every slot's label and
    /// step. The wall-clock time of the probes goes into `rollout.label_secs`.
    fn step_pass(
        &mut self,
        pending: &[PendingStep],
        rollout: &mut RolloutStats,
    ) -> Vec<SlotOutcome> {
        let n = pending.len();
        let started = Instant::now();
        let sets = probe_phase(&self.slots[..n], pending, &self.cfg);
        if sets.iter().any(Option::is_some) {
            rollout.label_secs += started.elapsed().as_secs_f32();
        }
        step_phase(&mut self.slots[..n], pending, &sets, &self.cfg.line_search)
    }

    /// The PPO half of [`Self::finish_step`]: the sample goes into the rollout buffer, with the
    /// bootstrap value a truncated episode needs.
    fn push_ppo_step(
        &mut self,
        w: usize,
        pending: PendingStep,
        step: &Step<GemstoneObservation>,
        ended: bool,
    ) -> Result<(), RunnerError> {
        let PendingStep {
            free,
            raw,
            log_prob,
            value,
            state,
            ..
        } = pending;
        let end = if step.terminated {
            EpisodeEnd::Terminated
        } else if step.truncated {
            let (_, next_value) = self.net.forward_with_scratch(
                &step.observation.tier_tokens(),
                &step.observation.global_token(),
                &mut self.scratch,
            )?;
            EpisodeEnd::Truncated { next_value }
        } else {
            EpisodeEnd::Continue
        };
        self.buffer.push_worker_step(
            w,
            TrajectoryStep {
                observation: state,
                action: raw.iter().map(|&x| f64::from(x)).collect(),
                reward: step.reward,
                value,
                log_prob,
            },
            end,
        );
        self.normalizer
            .track(w, step.reward, self.trainer.config.gamma, ended);
        self.n_tiers.push(free.len());
        self.free_masks.push(free);
        Ok(())
    }

    /// Step 5 of a pass for slot `w`: the sample goes into the rollout buffer (PPO) or, with its
    /// label, into the aggregate (imitation); statistics; and, when this episode ended, the
    /// [`Respawn`] of the next one (built in step 6).
    fn finish_step(
        &mut self,
        w: usize,
        pending: PendingStep,
        outcome: SlotOutcome,
        rollout: &mut RolloutStats,
    ) -> Result<Option<Respawn>, RunnerError> {
        let SlotOutcome { step, label } = outcome;
        let ended = step.terminated || step.truncated;
        rollout.evaluations += usize::from(step.info.evaluations);
        let label = label.filter(|label| rollout.record_label(label));
        match self.cfg.algo {
            Algorithm::Imitation => {
                if let Some(label) = &label {
                    self.aggregate
                        .push(LabelledSample::new(pending.state, pending.free, label));
                }
            }
            Algorithm::Ppo => self.push_ppo_step(w, pending, &step, ended)?,
        }
        rollout.steps += 1;
        rollout.reward_sum += step.reward;
        rollout.count_rejection(step.info.rejection);

        let optics = Optics::from_global(&step.observation.global);
        if w == 0 {
            self.stats.current = Some(optics);
            self.stats.solid_closed = step.info.is_closed;
        }
        if ended {
            return Ok(Some(self.end_episode(w, optics, rollout)));
        }
        self.slots[w].obs = step.observation;
        Ok(None)
    }

    /// Records the episode slot `w` just finished and draws the seed of the next one.
    fn end_episode(&mut self, w: usize, optics: Optics, rollout: &mut RolloutStats) -> Respawn {
        let slot = &self.slots[w];
        let summary = EpisodeSummary {
            entry: slot.entry,
            base_score: slot.env.base_score(),
            final_score: slot.env.current_score(),
            optics,
        };
        rollout.episodes_improved += usize::from(summary.improvement() > 0.0);
        rollout.finished.push(summary);
        let start = if self.cfg.corpus_mode {
            slot.pos + self.cfg.workers
        } else {
            0
        };
        Respawn {
            w,
            start,
            seed: self.rng.next_u64(),
        }
    }

    /// Step 6 of a pass: the next episodes of the slots that ended, built in parallel and
    /// installed serially in slot order. The designs are chosen serially first (so the set of
    /// skipped entries is read in slot order); a design whose environment fails to build is
    /// skipped exactly as the serial spawn skips it (a hard error in single-stone mode) and its
    /// slot takes the next design.
    fn respawn_slots(&mut self, ended: Vec<Respawn>) -> Result<(), RunnerError> {
        if ended.is_empty() {
            return Ok(());
        }
        let mut chosen = Vec::with_capacity(ended.len());
        for respawn in &ended {
            let (pos, entry) = self.choose(respawn.start)?;
            chosen.push((pos, entry, respawn.seed));
        }
        let (cfg, split) = (&self.cfg, &self.split);
        let built = pool::run_indexed(chosen.len(), pool::pass_threads(chosen.len()), |i| {
            let (pos, entry, seed) = chosen[i];
            build_slot(cfg, split, pos, entry, seed)
        });
        for (respawn, ((pos, entry, seed), built)) in
            ended.into_iter().zip(chosen.into_iter().zip(built))
        {
            let slot = match built? {
                Ok(slot) => slot,
                Err(error) => {
                    let start = self.skip_entry(pos, entry, &error)?;
                    self.spawn_slot(start, seed)?
                }
            };
            self.install_slot(respawn.w, slot);
        }
        Ok(())
    }

    /// Collects `rollout_size` steps round-robin over the workers. `keep_going` is asked before
    /// every environment step; when it returns `false` the partial rollout is discarded (the
    /// labelled states it already added to the aggregate stay: a label is exact for its state)
    /// and `None` is returned.
    fn collect_rollout(
        &mut self,
        keep_going: &mut dyn FnMut() -> bool,
    ) -> Result<Option<RolloutStats>, RunnerError> {
        self.clear_rollout();
        let mut rollout = RolloutStats::default();
        while rollout.steps < self.cfg.rollout_size {
            // One round-robin pass: the slots that still fit into the rollout.
            let n = self.slots.len().min(self.cfg.rollout_size - rollout.steps);
            for _ in 0..n {
                if !keep_going() {
                    self.clear_rollout();
                    return Ok(None);
                }
            }
            let pending = self.prepare_pass(n, rollout.steps)?;
            let outcomes = self.step_pass(&pending, &mut rollout);
            let mut ended = Vec::new();
            for (w, (p, outcome)) in pending.into_iter().zip(outcomes).enumerate() {
                ended.extend(self.finish_step(w, p, outcome, &mut rollout)?);
            }
            self.respawn_slots(ended)?;
            self.label_phase = self.label_phase.wrapping_add(1);
        }
        Ok(Some(rollout))
    }

    /// The PPO update: GAE over the rollout buffer, then the epochs of the Burn trainer.
    fn update_ppo(&mut self) -> Result<TrainingMetrics, RunnerError> {
        // Only the slots that produced a step have a rollout-tail value to bootstrap from.
        let used = self
            .buffer
            .worker_ids
            .iter()
            .copied()
            .max()
            .map_or(0, |last| last + 1);
        let mut last_values = Vec::with_capacity(used);
        for slot in &self.slots[..used] {
            let (_, v) = self
                .net
                .forward(&slot.obs.tier_tokens(), &slot.obs.global_token())?;
            last_values.push(v);
        }
        // The rewards enter GAE divided by the running std of the discounted return (the stored
        // steps keep the raw reward), so the critic regresses on O(1) targets.
        let reward_scale = self.normalizer.scale();
        self.buffer.compute_gae_per_worker(
            &last_values,
            self.trainer.config.gamma,
            self.trainer.config.gae_lambda,
            reward_scale,
        )?;
        self.stats.reward_scale = reward_scale;
        self.stats.explained_variance = self.buffer.explained_variance();

        let states: Vec<Vec<f32>> = self
            .buffer
            .steps
            .iter()
            .map(|s| s.observation.clone())
            .collect();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "actions are stored as f64 and fed to the f32 network; values are in the normal float range"
        )]
        let actions: Vec<Vec<f32>> = self
            .buffer
            .steps
            .iter()
            .map(|s| s.action.iter().map(|&a| a as f32).collect())
            .collect();
        let old_log_probs: Vec<f32> = self.buffer.steps.iter().map(|s| s.log_prob).collect();
        let batch = TrainBatch {
            states: &states,
            actions: &actions,
            old_log_probs: &old_log_probs,
            advantages: &self.buffer.advantages,
            returns: &self.buffer.returns,
            n_tiers: &self.n_tiers,
            free_tiers: &self.free_masks,
        };
        let metrics = self.trainer.train_rollout_epochs(&batch)?;
        self.net = self.trainer.model.export_to_set_attention()?;
        self.refresh_rollout_model();
        Ok(metrics)
    }

    /// The imitation update: a few epochs over the WHOLE aggregate of labelled states. An empty
    /// aggregate (a rollout without a usable label) leaves the model as it is.
    fn update_imitation(&mut self) -> Result<ImitationMetrics, RunnerError> {
        let batch = self.aggregate.batch();
        let metrics = self
            .trainer
            .train_imitation_epochs(&batch, &self.cfg.imitation)?;
        if metrics.updates > 0 {
            self.net = self.trainer.model.export_to_set_attention()?;
            self.refresh_rollout_model();
        }
        Ok(metrics)
    }

    /// One update of the configured algorithm: the PPO metrics (default in imitation mode) and
    /// the imitation metrics (`None` in PPO mode).
    fn update(&mut self) -> Result<(TrainingMetrics, Option<ImitationMetrics>), RunnerError> {
        match self.cfg.algo {
            Algorithm::Ppo => self.update_ppo().map(|metrics| (metrics, None)),
            Algorithm::Imitation => self
                .update_imitation()
                .map(|metrics| (TrainingMetrics::default(), Some(metrics))),
        }
    }

    /// One iteration: collect a rollout, update the network (PPO or imitation, as configured).
    ///
    /// Returns `None` when `keep_going` ended the rollout early (nothing was trained).
    ///
    /// # Errors
    ///
    /// [`RunnerError`] from the environments, the network or the trainer (a diverged loss
    /// included; the weights are then unchanged by that step).
    pub fn iterate(
        &mut self,
        keep_going: &mut dyn FnMut() -> bool,
    ) -> Result<Option<IterationReport>, RunnerError> {
        let started = Instant::now();
        let Some(rollout) = self.collect_rollout(keep_going)? else {
            return Ok(None);
        };
        let rollout_secs = started.elapsed().as_secs_f32();
        let started = Instant::now();
        let (train, imitation) = self.update()?;
        let update_secs = started.elapsed().as_secs_f32();

        let steps = rollout.steps;
        let mean_step_reward = rollout.reward_sum / steps.max(1) as f32;
        let labels_this_round = rollout.labels;
        let oracle_gain_mean = rollout.mean_oracle_gain();
        let label_secs = rollout.label_secs;
        let evaluations_this_round = rollout.total_evaluations();
        let (new_best, new_best_improvement) = self.record_rollout(rollout, &train);
        let stats = &mut self.stats;
        stats.last_imitation = imitation;
        stats.labelled_samples = self.aggregate.len();
        stats.rollout_secs = rollout_secs;
        stats.update_secs = update_secs;
        Ok(Some(IterationReport {
            train,
            mean_step_reward,
            steps,
            new_best,
            new_best_improvement,
            explained_variance: stats.explained_variance,
            accepted_frac: stats.accepted_frac,
            imitation,
            labels_this_round,
            oracle_gain_mean,
            rollout_secs,
            label_secs,
            update_secs,
            evaluations_this_round,
        }))
    }

    /// Folds a finished rollout and its update into the statistics. Returns the episodes that set
    /// a new lowest final score and a new largest score drop, if any.
    fn record_rollout(
        &mut self,
        rollout: RolloutStats,
        train: &TrainingMetrics,
    ) -> (Option<EpisodeSummary>, Option<EpisodeSummary>) {
        let steps = rollout.steps.max(1) as f32;
        let mean_step_reward = rollout.reward_sum / steps;
        let finished = rollout.finished.len();
        let stats = &mut self.stats;
        stats.iteration += 1;
        stats.total_steps += rollout.steps;
        stats.mean_step_reward = if stats.reward_history.is_empty() {
            mean_step_reward
        } else {
            super::REWARD_EMA.mul_add(
                mean_step_reward,
                (1.0 - super::REWARD_EMA) * stats.mean_step_reward,
            )
        };
        stats.reward_history.push_back(mean_step_reward);
        if stats.reward_history.len() > super::REWARD_HISTORY_LEN {
            stats.reward_history.pop_front();
        }
        stats.last_train = train.clone();
        stats.mean_step_reward_raw = mean_step_reward;
        stats.accepted_frac = rollout.accepted as f32 / steps;
        stats.episodes_finished = finished;
        stats.episodes_improved_frac = if finished == 0 {
            0.0
        } else {
            rollout.episodes_improved as f32 / finished as f32
        };
        stats.rej_unsafe = rollout.rej_unsafe;
        stats.rej_unsolvable = rollout.rej_unsolvable;
        stats.rej_not_closed = rollout.rej_not_closed;
        stats.rej_girdle = rollout.rej_girdle;
        stats.rej_manufacturability = rollout.rej_manufacturability;
        stats.rej_not_improved = rollout.rej_not_improved;
        stats.labels_this_round = rollout.labels;
        stats.oracle_gain_mean = rollout.mean_oracle_gain();
        stats.label_secs = rollout.label_secs;
        stats.evaluations_this_round = rollout.total_evaluations();

        let (mut new_best, mut new_best_improvement) = (None, None);
        for episode in rollout.finished {
            let improvement = episode.improvement();
            stats.episodes += 1;
            stats.recent_improvements.push_back(improvement);
            if stats.recent_improvements.len() > super::IMPROVEMENT_WINDOW {
                stats.recent_improvements.pop_front();
            }
            // A drop of zero is no record: the first episodes of a run would all "win".
            if improvement > 0.0
                && stats
                    .best_improvement
                    .as_ref()
                    .is_none_or(|b| improvement > b.improvement())
            {
                stats.best_improvement = Some(episode.clone());
                new_best_improvement = Some(episode.clone());
            }
            if stats
                .best
                .as_ref()
                .is_none_or(|b| episode.final_score < b.final_score)
            {
                stats.best = Some(episode.clone());
                new_best = Some(episode);
            }
        }
        (new_best, new_best_improvement)
    }
}

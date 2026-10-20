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
//!
//! The update that follows a rollout lives in `runner_update.rs`: an imitation update trains on
//! the update thread while the next rollout runs, so [`TrainingRunner::iterate`] reports the
//! figures of the update installed during it, which is the one of the previous iteration.

use std::time::Instant;

use indicatrix_ai::{
    env::{
        ContinuousTierAction, GemstoneObservation, OracleOutcome, Step, TeacherLabel,
        TierSensitivity, assemble_label, score_slopes,
    },
    model::{GaussianPolicy, GemstoneSetAttentionNetwork},
    training::{EpisodeEnd, TrainError, TrainingMetrics, TrajectoryStep},
};

use super::{
    Algorithm, EpisodeSummary, IterationReport, Optics, RolloutStats, RunConfig, RunnerError, Slot,
    TrainingRunner, build_slot,
    envs::EnvKind,
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
    // A probe costs about what one evaluation of its design costs, which grows with the tier
    // count: the biggest designs start first.
    let costs: Vec<u64> = tasks
        .iter()
        .map(|&(w, _)| slots[w].obs.tiers.len() as u64)
        .collect();
    let probes = pool::run_indexed_ranked(&costs, pool::pass_threads(tasks.len()), |task| {
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

/// The step of `env` along the line search; with `parallel` the candidates are evaluated
/// concurrently (bit-identical result, see `OptimizerEnv::step_scaled_with`).
fn step_env(
    env: &mut EnvKind,
    action: &ContinuousTierAction,
    scales: &[f64],
    parallel: bool,
) -> Step<GemstoneObservation> {
    match env {
        EnvKind::Optimizer(e) => e.step_scaled_with(action, scales, parallel),
        EnvKind::Retarget(e) => e.step_scaled_with(action, scales, parallel),
    }
}

/// The oracle step of `env` along `slopes`; with `parallel` the candidates are evaluated
/// concurrently (bit-identical result, see `OptimizerEnv::oracle_outcome_with`).
fn oracle_env(
    env: &EnvKind,
    slopes: &[Option<f32>],
    scales: &[f64],
    parallel: bool,
) -> OracleOutcome {
    match env {
        EnvKind::Optimizer(e) => e.oracle_outcome_with(slopes, scales, parallel),
        EnvKind::Retarget(e) => e.oracle_outcome_with(slopes, scales, parallel),
    }
}

/// The cost proxy of one slot in phase 2, for the longest-first order: the evaluations of its
/// step and its label, `free tiers x (1 + labelled)` (a labelled slot also runs the oracle line
/// search, and its probes were paid for in phase 1), scaled by the tier count because one
/// evaluation grows with it. Only the ranking matters, not the unit.
fn step_cost(slot: &Slot, labelled: bool) -> u64 {
    let free = slot.env.free_tiers().len() as u64;
    free.max(1) * (1 + u64::from(labelled)) * slot.obs.tiers.len() as u64
}

/// Phase 2: for every slot, finishes the label of a flagged one (the oracle step along the
/// probed score slopes, computed on the state before the step) and takes the line-search step.
/// Slots are handed out one at a time to the pool, the costliest first ([`step_cost`]), so the
/// cost of a slot (a labelled many-tier slot costs several times an unlabelled one) does not
/// unbalance the threads. Inner candidate line searches stay serial, so each slot is bounded by
/// one pool worker and native thread count cannot multiply by the number of line-search scales.
fn step_phase(
    slots: &mut [Slot],
    pending: &[PendingStep],
    sets: &[Option<ProbeSet>],
    scales: &[f64],
) -> Vec<SlotOutcome> {
    let threads = pool::pass_threads(slots.len());
    let costs: Vec<u64> = slots
        .iter()
        .zip(pending)
        .map(|(slot, p)| step_cost(slot, p.labelled))
        .collect();
    pool::run_items_ranked(slots, &costs, threads, |w, slot, tail| {
        let label = sets[w].as_ref().map(|set| {
            let slopes = score_slopes(&set.tiers);
            let oracle = oracle_env(&slot.env, &slopes, scales, tail.is_active());
            assemble_label(set.tiers.clone(), set.evaluations, oracle)
        });
        let step = step_env(&mut slot.env, &pending[w].action, scales, tail.is_active());
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
        let forward_started = Instant::now();
        let batch = self.rollout_model.forward_host(&states, &n_tiers)?;
        self.stats.rollout_forward_ms = forward_started
            .elapsed()
            .as_secs_f32()
            .mul_add(1_000.0, self.stats.rollout_forward_ms);
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
            .track(w, step.reward, self.cfg.ppo.gamma, ended);
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
            slot.pos + self.cfg.effective_workers()
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
        // The tier count of the design ranks the builds: the biggest start first.
        let costs: Vec<u64> = chosen
            .iter()
            .map(|&(_, entry, _)| split.entries[entry].design.tiers.len() as u64)
            .collect();
        let built = pool::run_indexed_ranked(&costs, pool::pass_threads(chosen.len()), |i| {
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
    pub(super) fn collect_rollout(
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

    /// One iteration: collect a rollout, then update the network (PPO or imitation, as
    /// configured).
    ///
    /// The update is not necessarily the one of this rollout. PPO updates on this rollout and
    /// waits for it. Imitation installs the update of the previous iteration (which trained
    /// while this rollout ran), then starts the update on this rollout's aggregate and returns
    /// without waiting for it: [`IterationReport::update_of_iteration`] says which iteration the
    /// reported update belongs to (`None` when none was installed, as in iteration 0). The
    /// schedule and its determinism contract are in the module docs of `runner_update.rs`.
    ///
    /// Returns `None` when `keep_going` ended the rollout early (nothing was trained; an update
    /// still training is finished and installed first, so the weights are never half applied).
    ///
    /// # Errors
    ///
    /// [`RunnerError`] from the environments, the network or the trainer (a diverged loss
    /// included; the weights are then unchanged by that step, and the error surfaces at the
    /// wait point after the iteration the update belongs to).
    pub fn iterate(
        &mut self,
        keep_going: &mut dyn FnMut() -> bool,
    ) -> Result<Option<IterationReport>, RunnerError> {
        self.stats.rollout_forward_ms = 0.0;
        let started = Instant::now();
        let Some(rollout) = self.collect_rollout(keep_going)? else {
            self.sync_update()?;
            self.refresh_rollout_model()?;
            return Ok(None);
        };
        let rollout_secs = started.elapsed().as_secs_f32();
        let iteration = self.stats.iteration;
        let update_wait_secs = self.update_step(iteration)?;
        let installed = self.installed_update.take();
        let (train, imitation, update_secs, weight_export_secs, update_of_iteration) =
            match installed {
                Some(update) => (
                    update.train,
                    update.imitation,
                    update.secs,
                    update.export_secs,
                    Some(update.iteration),
                ),
                None => (TrainingMetrics::default(), None, 0.0, 0.0, None),
            };

        let steps = rollout.steps;
        let mean_step_reward = rollout.reward_sum / steps.max(1) as f32;
        let labels_this_round = rollout.labels;
        let oracle_gain_mean = rollout.mean_oracle_gain();
        let label_secs = rollout.label_secs;
        let evaluations_this_round = rollout.total_evaluations();
        let (new_best, new_best_improvement) = self.record_rollout(rollout, &train);
        let stats = &mut self.stats;
        if imitation.is_some() {
            stats.last_imitation = imitation;
        }
        stats.labelled_samples = self.aggregate.len();
        stats.rollout_secs = rollout_secs;
        stats.update_secs = update_secs;
        stats.weight_export_secs = weight_export_secs;
        stats.update_wait_secs = update_wait_secs;
        stats.update_of_iteration = update_of_iteration;
        let packing_ms = imitation.map_or(train.packing_ms, |metrics| metrics.packing_ms);
        let training_host_ms = imitation.map_or(train.update_ms, |metrics| metrics.update_ms);
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
            packing_ms,
            training_host_ms,
            weight_export_ms: weight_export_secs * 1_000.0,
            rollout_forward_ms: stats.rollout_forward_ms,
            update_wait_secs,
            update_of_iteration,
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

//! Changes to a live run: the task, the model, the backend, the data and the hyperparameters.
//!
//! Every change is atomic: when the environments cannot be rebuilt for the new setting, the
//! setting, the model and the statistics are all as they were.

use indicatrix_ai::{
    model::{GemstoneSetAttentionNetwork, ModelTier, TrainingMode},
    training::BurnTrainer,
};

use super::{
    Algorithm, BuildArgs, DeviceChoice, Hyperparams, RunnerError, TrainingRunner, build_trainer,
    device_of_label,
};
use crate::dataset::{find_spectrum_entry, resolve_material};

impl TrainingRunner {
    /// Starts over: counters, statistics, the labelled aggregate and environments are reset; the
    /// weights are kept.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when no environment can be built.
    pub fn reset(&mut self) -> Result<(), RunnerError> {
        self.restart_run()
    }

    /// Switches the task. The weights are kept, the statistics and the labelled aggregate start
    /// over.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when no environment can be built (the mode is then unchanged).
    pub fn set_mode(&mut self, mode: TrainingMode) -> Result<(), RunnerError> {
        let previous = self.cfg.mode;
        self.cfg.mode = mode;
        self.restart_run().inspect_err(|_| {
            self.cfg.mode = previous;
        })
    }

    /// Switches the model size: the model is re-initialised and the statistics and the labelled
    /// aggregate start over.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when the trainer cannot be built or no environment can be built; the model,
    /// backend and statistics are then all as they were.
    pub fn set_tier(&mut self, tier: ModelTier) -> Result<(), RunnerError> {
        self.sync_update()?;
        let choice = device_of_label(self.device_label);
        let net = tier.init_network(self.cfg.seed);
        // The update thread builds the replacement trainer and holds it; the old one serves until
        // the commit below.
        let built = self.updater.stage(BuildArgs {
            ppo: self.cfg.ppo.clone(),
            tier,
            max_tiers: self.split.max_tiers,
            device: choice,
            seed: self.cfg.seed,
            net: net.clone(),
        })?;
        let rollout_trainer = match self.build_rollout_trainer(tier, &net, built.label) {
            Ok(trainer) => trainer,
            Err(error) => {
                self.updater.discard();
                return Err(error);
            }
        };
        // Environments do not depend on the model, so restart on the old model first: a failure
        // there leaves everything untouched, and nothing can fail after the swap.
        if let Err(error) = self.restart_run() {
            self.updater.discard();
            return Err(error);
        }
        self.updater.commit();
        self.rollout_trainer = rollout_trainer;
        self.net = net;
        self.rollout_model = self.rollout_trainer.rollout_model();
        self.cfg.tier = tier;
        self.device_label = built.label;
        self.warnings.extend(built.warning);
        Ok(())
    }

    /// A rollout trainer of model size `tier` on the device `label` names, loaded with `net`.
    fn build_rollout_trainer(
        &self,
        tier: ModelTier,
        net: &GemstoneSetAttentionNetwork,
        label: &str,
    ) -> Result<BurnTrainer, RunnerError> {
        let (mut trainer, _, _) = build_trainer(
            &self.cfg.ppo,
            tier,
            self.split.max_tiers,
            device_of_label(label),
            self.cfg.seed,
        )?;
        trainer.load_weights(net)?;
        Ok(trainer)
    }

    /// Moves the model to another backend, keeping the trained weights (the Adam moments start
    /// over). Returns the new backend label. An explicit WGPU or CUDA backend that cannot start
    /// is an error and leaves the current backend in place.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when the trainer cannot be built or the weights cannot be loaded.
    pub fn set_device(&mut self, device: DeviceChoice) -> Result<&'static str, RunnerError> {
        self.sync_update()?;
        let built = self.updater.stage(BuildArgs {
            ppo: self.cfg.ppo.clone(),
            tier: self.cfg.tier,
            max_tiers: self.split.max_tiers,
            device,
            seed: self.cfg.seed,
            net: self.net.clone(),
        })?;
        let rollout_trainer =
            match self.build_rollout_trainer(self.cfg.tier, &self.net, built.label) {
                Ok(trainer) => trainer,
                Err(error) => {
                    self.updater.discard();
                    return Err(error);
                }
            };
        self.updater.commit();
        self.rollout_trainer = rollout_trainer;
        self.rollout_model = self.rollout_trainer.rollout_model();
        self.device_label = built.label;
        self.cfg.device = device;
        self.warnings.extend(built.warning);
        Ok(built.label)
    }

    /// Cycle the train split across workers, or train on the focus stone only.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when no environment can be built (the setting is then unchanged).
    pub fn set_corpus_mode(&mut self, on: bool) -> Result<(), RunnerError> {
        let previous = self.cfg.corpus_mode;
        self.cfg.corpus_mode = on;
        self.rebuild_slots().inspect_err(|_| {
            self.cfg.corpus_mode = previous;
        })
    }

    /// Sample the retarget target from the whole spectrum, or use the fixed material.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when no environment can be built (the setting is then unchanged).
    pub fn set_universal_spectrum(&mut self, on: bool) -> Result<(), RunnerError> {
        let previous = self.cfg.universal_spectrum;
        self.cfg.universal_spectrum = on;
        self.rebuild_slots().inspect_err(|_| {
            self.cfg.universal_spectrum = previous;
        })
    }

    /// Sets the fixed retarget target by name (the UI labels are accepted).
    ///
    /// # Errors
    ///
    /// [`RunnerError::Dataset`] for an unknown material, or [`RunnerError`] when the environments
    /// cannot be rebuilt for it (nothing changes then).
    pub fn set_fixed_target(&mut self, name: &str) -> Result<(), RunnerError> {
        resolve_material(name)?;
        let canonical = find_spectrum_entry(name)?.name.to_owned();
        let previous = std::mem::replace(&mut self.cfg.fixed_target, canonical);
        if self.cfg.mode == TrainingMode::Retarget
            && !self.cfg.universal_spectrum
            && let Err(error) = self.rebuild_slots()
        {
            self.cfg.fixed_target = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Selects the focus stone by index into the records the data set was built from. In
    /// single-stone mode the workers switch to it at once.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Data`] when the record was skipped at load time or does not exist.
    pub fn select_record(&mut self, record: usize) -> Result<usize, RunnerError> {
        let entry = match self.split.by_record.get(record) {
            Some(Some(entry)) => *entry,
            Some(None) => {
                return Err(RunnerError::Data(
                    "that design was skipped at load time and cannot be trained".into(),
                ));
            }
            None => return Err(RunnerError::Data(format!("no design with index {record}"))),
        };
        let previous = self.focus_entry;
        self.focus_entry = entry;
        if !self.cfg.corpus_mode {
            self.rebuild_slots().inspect_err(|_| {
                self.focus_entry = previous;
            })?;
        }
        Ok(entry)
    }

    /// Validates and applies new hyperparameters atomically. In imitation mode the learning
    /// rate, mini-batch size and epoch count are the imitation ones (see [`Hyperparams`]).
    ///
    /// # Errors
    ///
    /// [`RunnerError`] naming the offending value; nothing changes then.
    pub fn set_hyperparams(&mut self, h: &Hyperparams) -> Result<(), RunnerError> {
        self.set_hyperparams_with_updates(h, None)
    }

    /// [`Self::set_hyperparams`] plus, in imitation mode, the mini-batches trained per round
    /// (`imitation.updates_per_round`; `Some(0)` selects the full-epoch mode, `None` keeps the
    /// current value). A PPO run ignores it. One atomic change: when validation refuses any value,
    /// nothing changes. The next imitation update takes it with its job; one in flight keeps the
    /// value it started with.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] naming the offending value; nothing changes then.
    pub fn set_hyperparams_with_updates(
        &mut self,
        h: &Hyperparams,
        updates_per_round: Option<usize>,
    ) -> Result<(), RunnerError> {
        let mut candidate = self.cfg.clone();
        candidate.workers = h.workers;
        candidate.rollout_size = h.rollout_size;
        match candidate.algo {
            Algorithm::Ppo => {
                candidate.ppo.mini_batch_size = h.mini_batch;
                candidate.ppo.epochs_per_batch = h.ppo_epochs;
                candidate.ppo.learning_rate = h.learning_rate;
            }
            Algorithm::Imitation => {
                candidate.imitation.mini_batch_size = h.mini_batch;
                candidate.imitation.epochs_per_round = h.ppo_epochs;
                candidate.imitation.learning_rate = h.learning_rate;
                if let Some(updates) = updates_per_round {
                    candidate.imitation.updates_per_round = updates;
                }
            }
        }
        candidate.ppo.clip_epsilon = h.clip_epsilon;
        candidate.ppo.gae_lambda = h.gae_lambda;
        candidate.validate()?;
        let workers_changed = candidate.workers != self.cfg.workers;
        let ppo = candidate.ppo.clone();
        let previous = std::mem::replace(&mut self.cfg, candidate);
        if workers_changed && let Err(e) = self.rebuild_slots() {
            self.cfg = previous;
            return Err(e);
        }
        // Queued behind any update in flight, which keeps the configuration it started with. An
        // imitation update takes its settings with the job; the PPO one reads the trainer's.
        self.updater.set_ppo_config(ppo);
        Ok(())
    }
}

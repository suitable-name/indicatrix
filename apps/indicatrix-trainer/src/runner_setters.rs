//! Changes to a live run: the task, the model, the backend, the data and the hyperparameters.
//!
//! Every change is atomic: when the environments cannot be rebuilt for the new setting, the
//! setting, the model and the statistics are all as they were.

use indicatrix_ai::model::{ModelTier, TrainingMode};

use super::{
    Algorithm, DeviceChoice, GPU_LABEL, Hyperparams, RunnerError, TrainingRunner, build_trainer,
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
        let choice = self.current_device_choice();
        let (mut trainer, label, warning) = build_trainer(
            &self.cfg.ppo,
            tier,
            self.split.max_tiers,
            choice,
            self.cfg.seed,
        )?;
        let net = tier.init_network(self.cfg.seed);
        trainer.load_weights(&net)?;
        // Environments do not depend on the model, so restart on the old model first: a failure
        // there leaves everything untouched, and nothing can fail after the swap.
        self.restart_run()?;
        self.trainer = trainer;
        self.net = net;
        self.refresh_rollout_model();
        self.cfg.tier = tier;
        self.device_label = label;
        self.warnings.extend(warning);
        Ok(())
    }

    fn current_device_choice(&self) -> DeviceChoice {
        if self.device_label == GPU_LABEL {
            DeviceChoice::Gpu
        } else {
            DeviceChoice::Cpu
        }
    }

    /// Moves the model to another backend, keeping the trained weights (the Adam moments start
    /// over). Returns the new backend label. An explicit `Gpu` that cannot start is an error and
    /// leaves the current backend in place.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when the trainer cannot be built or the weights cannot be loaded.
    pub fn set_device(&mut self, device: DeviceChoice) -> Result<&'static str, RunnerError> {
        let (mut trainer, label, warning) = build_trainer(
            &self.cfg.ppo,
            self.cfg.tier,
            self.split.max_tiers,
            device,
            self.cfg.seed,
        )?;
        trainer.load_weights(&self.net)?;
        self.trainer = trainer;
        self.refresh_rollout_model();
        self.device_label = label;
        self.cfg.device = device;
        self.warnings.extend(warning);
        Ok(label)
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
        self.trainer.config = ppo;
        Ok(())
    }
}

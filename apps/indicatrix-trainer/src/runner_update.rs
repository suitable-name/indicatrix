//! The update thread: imitation updates train while the next rollout collects.
//!
//! A long-lived thread owns the Burn trainer (it builds the trainer itself, so no Burn object
//! ever crosses a thread boundary except the pure-Rust twin it exports). The runner talks to it
//! through two `mpsc` channels: requests go in, replies come out in the order the requests that
//! reply were sent. At most one update is in flight.
//!
//! # The schedule, and why it is deterministic
//!
//! Iteration `i` of an imitation run is: rollout `i`, then the wait point, then the start of
//! update `i`. Update `i` trains on a **snapshot** of the aggregate taken at the end of rollout
//! `i` ([`OwnedImitationBatch`]), so rollout `i + 1` can keep pushing labels into the live
//! aggregate while the update trains.
//!
//! - **Wait point.** At the end of rollout `i` the runner waits for update `i - 1` (which had the
//!   whole rollout to finish), installs its twin as `net`, refreshes the rollout copy of the
//!   model from it and only then starts update `i`.
//! - **Rollout weights.** Rollout `i + 1` therefore acts with the weights of update `i - 1`: one
//!   update stale, which is fine for imitation (`DAgger` trains on labels of states the older
//!   policy visited anyway). It never sees a partly finished update.
//! - **Iteration 0** has nothing to wait for: rollout 0 acts with the initial weights, update 0
//!   starts at its end and trains during rollout 1, which still acts with the initial weights;
//!   update 1 starts at the end of rollout 1, after update 0 was installed.
//! - **Other readers** (validation, checkpoints, setters) call
//!   [`TrainingRunner::sync_update`], which waits for the update in flight and installs it into
//!   `net`. That does **not** touch the rollout copy of the model, which is refreshed only at the
//!   wait point above. So *when* somebody syncs (a GUI click, a validation) changes nothing the
//!   rollouts see: the sequence of weights and rollouts is fixed by the iteration index alone.
//! - **PPO** is on-policy: its rollout must use the freshly updated weights. It sends its update
//!   to the same thread and waits for it at once, so there is one code path and the result is
//!   the serial one.
//!
//! One Burn device is shared by the two threads (the update thread's trainer and the rollout
//! trainer the runner keeps for the batched forward both target the same default device). Burn's
//! compute clients are thread-safe handles to one server per device; this is the one thing the
//! code cannot show by reading, see the lane report.

use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

use indicatrix_ai::{
    env::N_COMPONENTS,
    model::{GemstoneSetAttentionNetwork, ModelTier},
    training::{
        BurnTrainer, ImitationBatch, ImitationConfig, ImitationMetrics, PpoConfig, TrainBatch,
        TrainError, TrainingMetrics,
    },
};

use super::{Algorithm, DeviceChoice, RunnerError, TrainingRunner, build_trainer};

/// Hard cap for an owned replay snapshot. The budget is smaller of this, the update's possible
/// minibatch draws, and the byte cap below; very large configured update counts cannot copy the
/// complete replay buffer by accident.
const MAX_REPLAY_SNAPSHOT_ROWS: usize = 65_536;
const MAX_REPLAY_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

/// An owned copy of an [`ImitationBatch`]: the aggregate as it stood at the end of a rollout.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OwnedImitationBatch {
    states: Vec<Vec<f32>>,
    n_tiers: Vec<usize>,
    free_tiers: Vec<Vec<bool>>,
    sensitivities: Vec<Vec<[f32; N_COMPONENTS]>>,
    score_slopes: Vec<Vec<f32>>,
    oracle_gain: Vec<f32>,
}

impl OwnedImitationBatch {
    /// Copies a deterministic, uniformly ranked subset of `batch` within the update's work and
    /// memory budgets. The subsequent trainer sampling remains stochastic over these rows.
    pub(super) fn snapshot(
        batch: &ImitationBatch<'_>,
        seed: u64,
        row_budget: Option<usize>,
    ) -> Self {
        let selected = row_budget.map_or_else(
            || (0..batch.len()).collect(),
            |budget| {
                let max_row_bytes = (0..batch.len())
                    .map(|index| replay_row_bytes(batch, index))
                    .max()
                    .unwrap_or(0);
                let byte_rows = MAX_REPLAY_SNAPSHOT_BYTES
                    .checked_div(max_row_bytes)
                    .unwrap_or(MAX_REPLAY_SNAPSHOT_ROWS);
                let count = budget
                    .min(batch.len())
                    .min(MAX_REPLAY_SNAPSHOT_ROWS)
                    .min(byte_rows);
                let mut rng = indicatrix_ai::model::Prng::from_stream(seed, 0x534e_4150_5348_4f54);
                let mut reservoir: Vec<usize> = (0..count).collect();
                for index in count..batch.len() {
                    let slot = rng.next_index(index + 1);
                    if slot < count {
                        reservoir[slot] = index;
                    }
                }
                reservoir
            },
        );
        let mut selected = selected;
        selected.sort_unstable();
        Self {
            states: selected.iter().map(|&i| batch.states[i].clone()).collect(),
            n_tiers: selected.iter().map(|&i| batch.n_tiers[i]).collect(),
            free_tiers: selected
                .iter()
                .map(|&i| batch.free_tiers[i].clone())
                .collect(),
            sensitivities: selected
                .iter()
                .map(|&i| batch.sensitivities[i].clone())
                .collect(),
            score_slopes: selected
                .iter()
                .map(|&i| batch.score_slopes[i].clone())
                .collect(),
            oracle_gain: selected.iter().map(|&i| batch.oracle_gain[i]).collect(),
        }
    }

    /// The borrowed view the trainer takes.
    pub(super) fn as_batch(&self) -> ImitationBatch<'_> {
        ImitationBatch {
            states: &self.states,
            n_tiers: &self.n_tiers,
            free_tiers: &self.free_tiers,
            sensitivities: &self.sensitivities,
            score_slopes: &self.score_slopes,
            oracle_gain: &self.oracle_gain,
        }
    }
}

const fn replay_row_bytes(batch: &ImitationBatch<'_>, index: usize) -> usize {
    // Payload plus the per-row Vec headers retained by the owned representation.
    batch.states[index]
        .len()
        .saturating_mul(std::mem::size_of::<f32>())
        .saturating_add(
            batch.free_tiers[index]
                .len()
                .saturating_mul(std::mem::size_of::<bool>()),
        )
        .saturating_add(
            batch.sensitivities[index]
                .len()
                .saturating_mul(std::mem::size_of::<[f32; N_COMPONENTS]>()),
        )
        .saturating_add(
            batch.score_slopes[index]
                .len()
                .saturating_mul(std::mem::size_of::<f32>()),
        )
        .saturating_add(5 * std::mem::size_of::<Vec<u8>>())
        .saturating_add(2 * std::mem::size_of::<usize>())
        .saturating_add(std::mem::size_of::<f32>())
}

/// An owned PPO batch (the borrowed [`TrainBatch`] cannot cross to the update thread).
#[derive(Debug)]
pub(super) struct OwnedPpoBatch {
    pub(super) states: Vec<Vec<f32>>,
    pub(super) actions: Vec<Vec<f32>>,
    pub(super) old_log_probs: Vec<f32>,
    pub(super) advantages: Vec<f32>,
    pub(super) returns: Vec<f32>,
    pub(super) n_tiers: Vec<usize>,
    pub(super) free_tiers: Vec<Vec<bool>>,
}

impl OwnedPpoBatch {
    pub(super) fn as_batch(&self) -> TrainBatch<'_> {
        TrainBatch {
            states: &self.states,
            actions: &self.actions,
            old_log_probs: &self.old_log_probs,
            advantages: &self.advantages,
            returns: &self.returns,
            n_tiers: &self.n_tiers,
            free_tiers: &self.free_tiers,
        }
    }
}

/// One update to run.
pub(super) enum UpdateJob {
    /// A few epochs over a snapshot of the labelled aggregate.
    Imitation {
        batch: OwnedImitationBatch,
        config: ImitationConfig,
    },
    /// The PPO epochs over one rollout.
    Ppo(OwnedPpoBatch),
}

/// What a finished update hands back.
#[derive(Debug)]
pub(super) struct UpdateDone {
    /// The iteration whose rollout the update trained on.
    pub(super) iteration: usize,
    /// Wall-clock seconds of the update on the update thread (training and export).
    pub(super) secs: f32,
    /// Host time exporting the updated weights to the inference twin.
    pub(super) export_secs: f32,
    /// PPO metrics (default for an imitation update).
    pub(super) train: TrainingMetrics,
    /// Imitation metrics (`None` for a PPO update).
    pub(super) imitation: Option<ImitationMetrics>,
    /// The exported twin; `None` when the update took no optimizer step (an empty aggregate),
    /// so the weights are unchanged.
    pub(super) net: Option<GemstoneSetAttentionNetwork>,
}

/// An installed update whose figures the next [`super::IterationReport`] carries.
#[derive(Debug)]
pub(super) struct InstalledUpdate {
    pub(super) iteration: usize,
    pub(super) secs: f32,
    pub(super) export_secs: f32,
    pub(super) train: TrainingMetrics,
    pub(super) imitation: Option<ImitationMetrics>,
}

/// Everything the update thread needs to build a trainer.
pub(super) struct BuildArgs {
    pub(super) ppo: PpoConfig,
    pub(super) tier: ModelTier,
    pub(super) max_tiers: usize,
    pub(super) device: DeviceChoice,
    pub(super) seed: u64,
    /// The weights the new trainer starts from.
    pub(super) net: GemstoneSetAttentionNetwork,
}

/// What building a trainer reports.
#[derive(Debug)]
pub(super) struct Built {
    /// Backend label of the trainer (WGPU, CUDA, or CPU).
    pub(super) label: &'static str,
    /// A GPU fallback warning, if any.
    pub(super) warning: Option<String>,
}

enum Request {
    Update {
        iteration: usize,
        job: UpdateJob,
    },
    LoadWeights(Box<GemstoneSetAttentionNetwork>),
    SetPpoConfig(Box<PpoConfig>),
    SetUpdateCount(u64),
    /// Builds a replacement trainer and holds it until [`Request::Commit`] or
    /// [`Request::Discard`]; the current trainer keeps serving meanwhile.
    Stage(Box<BuildArgs>),
    Commit,
    Discard,
}

enum Reply {
    Built(Result<Built, TrainError>),
    Done(Box<Result<UpdateDone, TrainError>>),
    Ack(Result<(), TrainError>),
}

fn stopped() -> RunnerError {
    RunnerError::Train(TrainError::Tensor(
        "the update thread stopped unexpectedly".into(),
    ))
}

fn out_of_step() -> RunnerError {
    RunnerError::Train(TrainError::Tensor(
        "the update thread answered out of order".into(),
    ))
}

/// `build_trainer` reports only `TrainError`s; anything else is folded into one.
fn into_train_error(error: RunnerError) -> TrainError {
    match error {
        RunnerError::Train(error) => error,
        other => TrainError::InvalidConfig(other.to_string()),
    }
}

/// A trainer on the requested device, loaded with `args.net`.
fn build(args: &BuildArgs) -> Result<(BurnTrainer, Built), TrainError> {
    let (mut trainer, label, warning) =
        build_trainer(&args.ppo, args.tier, args.max_tiers, args.device, args.seed)
            .map_err(into_train_error)?;
    trainer.load_weights(&args.net)?;
    Ok((trainer, Built { label, warning }))
}

/// Runs one update on `trainer` and exports the twin.
fn run_update(
    trainer: &mut BurnTrainer,
    iteration: usize,
    job: UpdateJob,
) -> Result<UpdateDone, TrainError> {
    let started = Instant::now();
    let (train, imitation, stepped) = match job {
        UpdateJob::Imitation { batch, config } => {
            let metrics = trainer.train_imitation_epochs(&batch.as_batch(), &config)?;
            (
                TrainingMetrics::default(),
                Some(metrics),
                metrics.updates > 0,
            )
        }
        UpdateJob::Ppo(batch) => {
            let metrics = trainer.train_rollout_epochs(&batch.as_batch())?;
            (metrics, None, true)
        }
    };
    let export_started = Instant::now();
    let net = if stepped {
        Some(trainer.model.export_to_set_attention()?)
    } else {
        None
    };
    let export_secs = if stepped {
        export_started.elapsed().as_secs_f32()
    } else {
        0.0
    };
    Ok(UpdateDone {
        iteration,
        secs: started.elapsed().as_secs_f32(),
        export_secs,
        train,
        imitation,
        net,
    })
}

/// The body of the update thread: builds the first trainer, then serves requests until the
/// runner drops its sender (queued requests, an update in flight included, run to the end).
fn serve(requests: &Receiver<Request>, replies: &Sender<Reply>, first: &BuildArgs) {
    let mut trainer = match build(first) {
        Ok((trainer, built)) => {
            if replies.send(Reply::Built(Ok(built))).is_err() {
                return;
            }
            trainer
        }
        Err(error) => {
            let _ = replies.send(Reply::Built(Err(error)));
            return;
        }
    };
    let mut staged: Option<BurnTrainer> = None;
    while let Ok(request) = requests.recv() {
        let reply = match request {
            Request::Update { iteration, job } => Some(Reply::Done(Box::new(run_update(
                &mut trainer,
                iteration,
                job,
            )))),
            Request::LoadWeights(net) => Some(Reply::Ack(trainer.load_weights(&net))),
            Request::SetPpoConfig(config) => {
                trainer.config = *config;
                None
            }
            Request::SetUpdateCount(updates) => {
                trainer.set_update_count(updates);
                None
            }
            Request::Stage(args) => Some(Reply::Built(build(&args).map(|(next, built)| {
                staged = Some(next);
                built
            }))),
            Request::Commit => {
                if let Some(next) = staged.take() {
                    trainer = next;
                }
                None
            }
            Request::Discard => {
                staged = None;
                None
            }
        };
        if let Some(reply) = reply
            && replies.send(reply).is_err()
        {
            return;
        }
    }
}

/// The runner's handle to the update thread.
pub(super) struct Updater {
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    handle: Option<JoinHandle<()>>,
    /// The iteration of the update in flight.
    in_flight: Option<usize>,
}

impl Updater {
    /// Starts the thread, which builds its trainer from `first`, and waits for it.
    ///
    /// # Errors
    ///
    /// [`RunnerError::Train`] when the trainer cannot be built (for example an explicit GPU that
    /// does not start) or the thread cannot be spawned.
    pub(super) fn spawn(first: BuildArgs) -> Result<(Self, Built), RunnerError> {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("trainer-update".into())
            .spawn(move || serve(&request_rx, &reply_tx, &first))
            .map_err(|e| {
                RunnerError::Train(TrainError::Tensor(format!(
                    "cannot start the update thread: {e}"
                )))
            })?;
        let updater = Self {
            requests: Some(request_tx),
            replies: reply_rx,
            handle: Some(handle),
            in_flight: None,
        };
        let built = updater.recv_built()?;
        Ok((updater, built))
    }

    /// The iteration of the update in flight, if any.
    pub(super) const fn in_flight(&self) -> Option<usize> {
        self.in_flight
    }

    fn send(&self, request: Request) -> Result<(), RunnerError> {
        self.requests
            .as_ref()
            .ok_or_else(stopped)?
            .send(request)
            .map_err(|_| stopped())
    }

    /// Calls that expect a reply of their own must not run while an update is in flight (its
    /// reply would come first).
    fn idle(&self) -> Result<(), RunnerError> {
        if self.in_flight.is_some() {
            return Err(RunnerError::Config(
                "internal error: the update thread is busy, sync before changing the trainer"
                    .into(),
            ));
        }
        Ok(())
    }

    fn recv_built(&self) -> Result<Built, RunnerError> {
        match self.replies.recv() {
            Ok(Reply::Built(built)) => built.map_err(RunnerError::Train),
            Ok(_) => Err(out_of_step()),
            Err(_) => Err(stopped()),
        }
    }

    /// Starts `job` as the update of `iteration`. Nothing may be in flight.
    ///
    /// # Errors
    ///
    /// [`RunnerError`] when an update is in flight or the thread is gone.
    pub(super) fn start(&mut self, iteration: usize, job: UpdateJob) -> Result<(), RunnerError> {
        self.idle()?;
        self.send(Request::Update { iteration, job })?;
        self.in_flight = Some(iteration);
        Ok(())
    }

    /// Waits for the update in flight; `None` when there is none. The update is consumed either
    /// way: after an error the trainer's weights are those before it.
    ///
    /// # Errors
    ///
    /// The update's own [`RunnerError`] (a diverged loss included), or one for a dead thread.
    pub(super) fn wait(&mut self) -> Result<Option<UpdateDone>, RunnerError> {
        if self.in_flight.take().is_none() {
            return Ok(None);
        }
        match self.replies.recv() {
            Ok(Reply::Done(done)) => (*done).map(Some).map_err(RunnerError::Train),
            Ok(_) => Err(out_of_step()),
            Err(_) => Err(stopped()),
        }
    }

    /// Replaces the weights of the trainer (the Adam moments start over).
    ///
    /// # Errors
    ///
    /// [`RunnerError::Train`] when the network does not fit the model; nothing changes then.
    pub(super) fn load_weights(&self, net: GemstoneSetAttentionNetwork) -> Result<(), RunnerError> {
        self.idle()?;
        self.send(Request::LoadWeights(Box::new(net)))?;
        match self.replies.recv() {
            Ok(Reply::Ack(result)) => result.map_err(RunnerError::Train),
            Ok(_) => Err(out_of_step()),
            Err(_) => Err(stopped()),
        }
    }

    /// Builds a replacement trainer on the thread without installing it; follow with
    /// [`Self::commit`] or [`Self::discard`].
    ///
    /// # Errors
    ///
    /// [`RunnerError::Train`] when the trainer cannot be built.
    pub(super) fn stage(&self, args: BuildArgs) -> Result<Built, RunnerError> {
        self.idle()?;
        self.send(Request::Stage(Box::new(args)))?;
        self.recv_built()
    }

    /// Installs the staged trainer.
    pub(super) fn commit(&self) {
        let _ = self.send(Request::Commit);
    }

    /// Drops the staged trainer.
    pub(super) fn discard(&self) {
        let _ = self.send(Request::Discard);
    }

    /// Hands the trainer a new PPO configuration (it applies after the update in flight).
    pub(super) fn set_ppo_config(&self, config: PpoConfig) {
        let _ = self.send(Request::SetPpoConfig(Box::new(config)));
    }

    /// Restores the optimizer-step count of a resumed run.
    pub(super) fn set_update_count(&self, updates: u64) {
        let _ = self.send(Request::SetUpdateCount(updates));
    }
}

impl Drop for Updater {
    /// Closes the request channel and joins the thread: a queued or running update finishes
    /// first, the thread is never abandoned.
    fn drop(&mut self) {
        self.requests = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl TrainingRunner {
    /// Waits for the update in flight and installs its weights into the network that validation
    /// and the checkpoints read. A no-op when none is in flight. It does **not** change what the
    /// rollouts act with (see the module docs), so calling it at any time leaves a run
    /// reproducible. The figures of the installed update go into the next
    /// [`super::IterationReport`].
    ///
    /// # Errors
    ///
    /// The update's own [`RunnerError`] (a diverged loss included; the weights are then those
    /// before that update).
    pub fn sync_update(&mut self) -> Result<(), RunnerError> {
        let Some(done) = self.updater.wait()? else {
            return Ok(());
        };
        if let Some(net) = done.net {
            self.net = net;
        }
        self.installed_update = Some(InstalledUpdate {
            iteration: done.iteration,
            secs: done.secs,
            export_secs: done.export_secs,
            train: done.train,
            imitation: done.imitation,
        });
        Ok(())
    }

    /// The wait point: installs the update in flight, then points the rollout copy of the model
    /// at the installed weights. Returns the seconds spent waiting.
    fn install_pending(&mut self) -> Result<f32, RunnerError> {
        let started = Instant::now();
        self.sync_update()?;
        let waited = started.elapsed().as_secs_f32();
        self.refresh_rollout_model()?;
        Ok(waited)
    }

    /// Ends an iteration's update step for the configured algorithm and returns the seconds the
    /// runner waited for the update thread. Imitation: snapshot, wait point, start (the update
    /// then trains during the next rollout). PPO: wait point, start, wait point at once.
    pub(super) fn update_step(&mut self, iteration: usize) -> Result<f32, RunnerError> {
        match self.cfg.algo {
            Algorithm::Imitation => {
                let row_budget = (self.cfg.imitation.updates_per_round > 0).then(|| {
                    self.cfg
                        .imitation
                        .updates_per_round
                        .saturating_mul(self.cfg.imitation.mini_batch_size)
                });
                let selection_seed =
                    self.cfg.seed ^ (iteration as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                let batch = OwnedImitationBatch::snapshot(
                    &self.aggregate.batch(),
                    selection_seed,
                    row_budget,
                );
                let job = UpdateJob::Imitation {
                    batch,
                    config: self.cfg.imitation.clone(),
                };
                let waited = self.install_pending()?;
                self.updater.start(iteration, job)?;
                Ok(waited)
            }
            Algorithm::Ppo => {
                let first = self.install_pending()?;
                let job = self.ppo_job()?;
                self.updater.start(iteration, job)?;
                Ok(first + self.install_pending()?)
            }
        }
    }

    /// The PPO update's batch: GAE over the rollout buffer, then the owned samples.
    pub(super) fn ppo_job(&mut self) -> Result<UpdateJob, RunnerError> {
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
            self.cfg.ppo.gamma,
            self.cfg.ppo.gae_lambda,
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
        Ok(UpdateJob::Ppo(OwnedPpoBatch {
            states,
            actions,
            old_log_probs,
            advantages: self.buffer.advantages.clone(),
            returns: self.buffer.returns.clone(),
            n_tiers: std::mem::take(&mut self.n_tiers),
            free_tiers: std::mem::take(&mut self.free_masks),
        }))
    }
}

#[cfg(test)]
mod tests {
    use indicatrix_ai::env::{TeacherLabel, TierSensitivity, flat_len};

    use super::super::imitation::{LabelledSample, LabelledStates};
    use super::*;

    fn sample(marker: f32) -> LabelledSample {
        let mut state = vec![0.0; flat_len(4)];
        state[0] = marker;
        let tier = TierSensitivity {
            components: [marker; N_COMPONENTS],
            score: marker,
            forward_probe: true,
        };
        let label = TeacherLabel {
            tiers: vec![Some(tier), None],
            oracle_gain: Some(marker),
            oracle_scale: Some(1.0),
            evaluations: 0,
        };
        LabelledSample::new(state, vec![true, false], &label)
    }

    #[test]
    fn a_snapshot_is_a_copy_the_aggregate_can_outgrow() {
        let mut aggregate = LabelledStates::new(10);
        aggregate.push(sample(1.0));
        aggregate.push(sample(2.0));
        let snapshot = OwnedImitationBatch::snapshot(&aggregate.batch(), 31, Some(10));
        assert_eq!(snapshot.as_batch().len(), 2);

        // The live aggregate moves on; the snapshot keeps the state of the moment it was taken.
        aggregate.push(sample(3.0));
        aggregate.clear();
        let view = snapshot.as_batch();
        assert_eq!(view.oracle_gain, [1.0, 2.0]);
        assert_eq!(view.n_tiers, [2, 2]);
        assert_eq!(view.states[1][0], 2.0);
        view.validate(
            indicatrix_ai::env::GLOBAL_DIM,
            indicatrix_ai::env::TIER_DIM,
            4,
        )
        .expect("a snapshot of a valid aggregate is a valid batch");
    }

    /// Whether two snapshots hold the same rows, NaN included: an unlabelled tier is all NaN by
    /// contract and `PartialEq` says `NaN != NaN`, so `assert_eq!` could never hold.
    fn same_rows(a: &OwnedImitationBatch, b: &OwnedImitationBatch) -> bool {
        let (a, b) = (a.as_batch(), b.as_batch());
        let bits = |value: &f32| value.to_bits();
        let same_rows_of = |x: &[Vec<f32>], y: &[Vec<f32>]| {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|(x, y)| x.iter().map(bits).eq(y.iter().map(bits)))
        };
        a.states == b.states
            && a.n_tiers == b.n_tiers
            && a.free_tiers == b.free_tiers
            && a.sensitivities.len() == b.sensitivities.len()
            && a.sensitivities.iter().zip(b.sensitivities).all(|(x, y)| {
                x.len() == y.len()
                    && x.iter()
                        .flatten()
                        .map(bits)
                        .eq(y.iter().flatten().map(bits))
            })
            && same_rows_of(a.score_slopes, b.score_slopes)
            && a.oracle_gain
                .iter()
                .map(bits)
                .eq(b.oracle_gain.iter().map(bits))
    }

    #[test]
    fn bounded_snapshot_is_seeded_uniform_subset_not_a_prefix() {
        let mut aggregate = LabelledStates::new(256);
        for marker in 0..200 {
            aggregate.push(sample(marker as f32));
        }
        let first = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x1234, Some(32));
        let repeat = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x1234, Some(32));
        let other = OwnedImitationBatch::snapshot(&aggregate.batch(), 0x5678, Some(32));
        // The unlabelled second tier of every sample is NaN, so the rows are compared bitwise.
        assert!(same_rows(&first, &repeat));
        assert!(!same_rows(&first, &other));
        assert_eq!(first.as_batch().len(), 32);
        let selected: Vec<f32> = first.as_batch().oracle_gain.to_vec();
        assert!(selected.iter().any(|&i| i < 50.0));
        assert!(selected.iter().any(|&i| i >= 150.0));
    }

    #[test]
    fn full_epoch_mode_keeps_the_entire_aggregate() {
        let mut aggregate = LabelledStates::new(100);
        for marker in 0..80 {
            aggregate.push(sample(marker as f32));
        }
        let snapshot = OwnedImitationBatch::snapshot(&aggregate.batch(), 7, None);
        assert_eq!(snapshot.as_batch().len(), 80);
    }
}

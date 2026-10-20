//! Parallel evaluation of the model on a set of designs, in up to three passes (review TQ-16).
//!
//! Every design is played from its base state by the same machinery, [`EvalJob::play`], with a
//! different [`Pilot`] choosing the action of each step:
//!
//! - the **model** (always): the policy mean, projected, stepped with the run's line search;
//! - the **sampled** pass: one stochastic episode, the exploration policy of the run (the lower
//!   bound a trained model must beat);
//! - the **oracle** pass: per step the finite-difference slopes of the current design, the same
//!   direction rule the model composes from its sensitivities, projected and stepped like the
//!   model's action (a reference direction, not a guaranteed upper bound).
//!
//! All three score the same stone against the same target under the same objective weights, so
//! `model / oracle` is the figure to watch. The end-of-run test evaluation and every in-training
//! validation pass previously played designs one after another on one core. Designs are
//! independent, so they are handed to the persistent bounded CPU pool (a design with many free
//! tiers costs many times a small one) and every outcome is written into the slot of its entry;
//! the report is then reduced serially in entry order. Nothing is shared but immutable data (the network, the
//! data set, the configuration), so the result is identical for any thread count.

use std::time::Instant;

use indicatrix_ai::inference::score_design_at_fidelity;
use indicatrix_ai::{
    env::{
        ContinuousTierAction, EnvError, GemstoneObservation, action_from_direction,
        direction_from_slopes, score_slopes,
    },
    model::{GaussianPolicy, GemstoneSetAttentionNetwork, InferenceScratch, Prng, TrainingMode},
};
use indicatrix_cut_core::{
    Design, ObjectiveWeights,
    optimize::{
        ObjectiveFidelity, OptimizeConfig, OptimizeOptions, SearchHooks, optimize_design_with,
    },
};

use super::{
    Algorithm, EvalCase, EvalFidelity, EvalPasses, EvalReport, ObjectiveSampling, Optics,
    RunConfig, RunnerError, build_env, draw_weights,
    envs::{EnvKind, Target, sampled_context, sampled_source_material},
    imitation::explore_raw,
    pick_target, pool,
};
use crate::dataset::{DatasetSplit, DesignEntry};

/// Everything the evaluation threads read. All of it is shared immutably, so the job is `Copy`
/// and `Send`; the trainer, the optimizer state and the environments of the rollout never enter.
#[derive(Clone, Copy)]
pub(super) struct EvalJob<'a> {
    /// The network that plays (its `forward` is pure).
    pub(super) net: &'a GemstoneSetAttentionNetwork,
    /// The data set the entry indices refer to.
    pub(super) split: &'a DatasetSplit,
    /// Task, target selection, objective sampling, line search and step-limit override.
    pub(super) cfg: &'a RunConfig,
    /// Scoring fidelity of the reported figures.
    pub(super) fidelity: EvalFidelity,
    /// Run seed: fixes the targets, the weights and the stochastic episodes.
    pub(super) seed: u64,
    /// The passes besides the model's own.
    pub(super) passes: EvalPasses,
}

/// What the cached oracle drop depends on: the validation designs, the seed that draws their
/// targets and weights, and every setting of the run that shapes the oracle episode. The oracle
/// does not depend on the model, so while the key is unchanged the drop is too.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OracleKey {
    entries: Vec<usize>,
    seed: u64,
    max_step_deg: f64,
    mode: TrainingMode,
    universal_spectrum: bool,
    fixed_target: String,
    line_search: Vec<f64>,
    label_h_deg: Option<f64>,
    objectives: ObjectiveSampling,
    same_material_frac: f32,
    train_fidelity: ObjectiveFidelity,
    validation_fidelity: EvalFidelity,
    objective_sweep: bool,
}

impl OracleKey {
    /// The key of an oracle pass over `entries` with `seed`, under the step limit `max_step_deg`
    /// that is in effect.
    pub(super) fn new(cfg: &RunConfig, entries: &[usize], seed: u64, max_step_deg: f64) -> Self {
        Self {
            entries: entries.to_vec(),
            seed,
            max_step_deg,
            mode: cfg.mode,
            universal_spectrum: cfg.universal_spectrum,
            fixed_target: cfg.fixed_target.clone(),
            line_search: cfg.line_search.clone(),
            label_h_deg: cfg.label_h_deg,
            objectives: cfg.objectives,
            same_material_frac: cfg.same_material_frac,
            train_fidelity: cfg.train_fidelity,
            validation_fidelity: cfg.validation_fidelity,
            objective_sweep: cfg.objective_sweep,
        }
    }
}

/// The oracle's mean score drop on the validation designs, kept for as long as its [`OracleKey`]
/// matches.
#[derive(Clone, Debug)]
pub(super) struct OracleCache {
    pub(super) key: OracleKey,
    pub(super) drop: f32,
}

/// Figures of one evaluated design.
#[derive(Clone)]
struct DesignFigures {
    case: EvalCase,
    /// Score before the episode.
    base_score: f32,
    /// Score after the model's episode.
    final_score: f32,
    /// Optics before the episode.
    base: Optics,
    /// Optics after the model's episode.
    fin: Optics,
    /// Score after the stochastic episode (same fidelity); `None` when it was not played.
    sampled_final_score: Option<f32>,
    /// Score after the oracle episode (same fidelity); `None` when it was not played.
    oracle_final_score: Option<f32>,
    /// Candidate evaluations the model's episode spent.
    model_evaluations: usize,
    /// Environment steps the model's episode took.
    model_steps: usize,
}

/// What one design came to: figures, or the reason it could not be evaluated.
enum DesignResult {
    Figures(DesignFigures),
    Failed(EvalCase),
}

/// A design's outcome: its result, or the error that stops the evaluation.
type DesignOutcome = Result<DesignResult, RunnerError>;

/// Who chooses the action of each step of an episode.
enum Pilot {
    /// The policy mean.
    Model,
    /// One draw per step. `explore_std` is the exploration noise around the mean (imitation
    /// runs); `None` draws from the policy's own distribution (PPO runs).
    Sampled { rng: Prng, explore_std: Option<f32> },
    /// The finite-difference oracle.
    Oracle,
}

/// How an episode went.
struct Played {
    /// The observation after the last step.
    obs: GemstoneObservation,
    /// Environment steps taken.
    steps: usize,
    /// Candidates the steps scored.
    evaluations: usize,
}

/// What every pass of one design shares: the target it is judged against and the objective
/// weights it is scored with.
struct DesignContext {
    target: Option<Target>,
    weights: ObjectiveWeights,
    context_seed: u64,
}

impl EvalJob<'_> {
    /// The exploration noise of the sampled pass: the run's `explore_std` in imitation mode,
    /// `None` (the policy's own distribution) in PPO mode.
    fn explore_std(&self) -> Option<f32> {
        (self.cfg.algo == Algorithm::Imitation).then_some(self.cfg.explore_std)
    }

    /// The oracle's action at the current design: the direction of the true score slopes, at
    /// the step limit.
    fn oracle_action(&self, env: &EnvKind, max_step: f64) -> ContinuousTierAction {
        let h = self.cfg.label_h_deg.unwrap_or(max_step);
        let slopes = score_slopes(&env.teacher_label(h, &[]).tiers);
        action_from_direction(&direction_from_slopes(&slopes), max_step)
    }

    /// The (unprojected) action `pilot` proposes in `obs`.
    fn propose(
        &self,
        env: &EnvKind,
        obs: &GemstoneObservation,
        pilot: &mut Pilot,
        scratch: &mut InferenceScratch,
    ) -> Result<ContinuousTierAction, RunnerError> {
        let max_step = env.max_step_deg();
        let raw = match pilot {
            Pilot::Oracle => return Ok(self.oracle_action(env, max_step)),
            Pilot::Model => {
                self.net
                    .forward_with_scratch(&obs.tier_tokens(), &obs.global_token(), scratch)?
                    .0
                    .mean
            }
            Pilot::Sampled { rng, explore_std } => {
                let (policy, _) = self.net.forward_with_scratch(
                    &obs.tier_tokens(),
                    &obs.global_token(),
                    scratch,
                )?;
                let free = obs.free_tier_mask();
                match *explore_std {
                    Some(std) => explore_raw(&policy.mean, &free, std, rng),
                    None => policy.sample_raw_masked(rng, &free).0,
                }
            }
        };
        Ok(GaussianPolicy::squash(&raw, max_step))
    }

    /// Plays one episode from `obs` to its end and returns how it went: every step is proposed
    /// by `pilot`, projected as the rollout projects, and stepped with the run's line search.
    ///
    /// The oracle stops as soon as a step is not accepted: the environment is greedy and
    /// deterministic, a refused step leaves the design as it was, and the oracle's action
    /// depends on the design alone, so every later step would repeat the refusal. The final
    /// score is the same as if the episode were played out, at a fraction of the cost.
    fn play(
        &self,
        env: &mut EnvKind,
        mut obs: GemstoneObservation,
        mut pilot: Pilot,
        scratch: &mut InferenceScratch,
    ) -> Result<Played, RunnerError> {
        let (mut steps, mut evaluations) = (0, 0);
        loop {
            let max_step = env.max_step_deg();
            let mut action = self.propose(env, &obs, &mut pilot, scratch)?;
            GemstoneSetAttentionNetwork::project_actions(
                &mut action,
                &obs.tiers,
                obs.global.target_critical_angle_deg,
                max_step,
            );
            let step = env.step_scaled(&action, &self.cfg.line_search);
            steps += 1;
            evaluations += usize::from(step.info.evaluations);
            let done = step.terminated || step.truncated;
            let stalled = matches!(pilot, Pilot::Oracle) && step.info.scale.is_none();
            obs = step.observation;
            if done || stalled {
                return Ok(Played {
                    obs,
                    steps,
                    evaluations,
                });
            }
        }
    }

    /// Plays `pilot` from the base design again (the same environment, reset) and returns the
    /// final score at the evaluation fidelity. An `Err` inside is a design whose full-fidelity
    /// re-score could not be built, which fails the design rather than the evaluation.
    fn replay(
        &self,
        env: &mut EnvKind,
        ctx: &DesignContext,
        pilot: Pilot,
        scratch: &mut InferenceScratch,
    ) -> Result<Result<f32, EnvError>, RunnerError> {
        let obs = env.reset(self.seed);
        self.play(env, obs, pilot, scratch)?;
        Ok(match (self.fidelity, self.cfg.train_fidelity) {
            (EvalFidelity::Fast, ObjectiveFidelity::Fast) => Ok(env.current_score()),
            (fidelity, _) => {
                rescore_final_score(env.current_design(), ctx, objective_fidelity(fidelity))
            }
        })
    }

    /// Evaluates one design: build the environment, play the model's episode, optionally
    /// re-score at full fidelity, then the requested extra passes. A design whose environment
    /// (or full-fidelity re-score) cannot be built is a [`DesignResult::Failed`], not an error;
    /// a material or network failure is an error.
    fn evaluate_design(
        &self,
        entry: usize,
        objective_override: Option<ObjectiveWeights>,
    ) -> DesignOutcome {
        let started = Instant::now();
        let item = &self.split.entries[entry];
        let salted = self.seed ^ (entry as u64).wrapping_mul(0x9E37_79B9);
        let target = pick_target(self.cfg, salted, &item.design)?;
        let weights =
            objective_override.unwrap_or_else(|| draw_weights(self.cfg.objectives, salted));
        let mut case = new_case(item, target.as_ref(), weights, salted)?;
        let mut env = match build_env(
            self.cfg,
            &item.design,
            target.as_ref(),
            weights,
            None,
            salted,
        ) {
            Ok(env) => env,
            Err(e) => {
                case.failure = Some(format!("'{}': {e}", item.title));
                return Ok(DesignResult::Failed(case));
            }
        };
        let ctx = DesignContext {
            target,
            weights,
            context_seed: salted,
        };
        let mut scratch = InferenceScratch::default();

        let obs = env.reset(self.seed);
        let base_fast = Optics::from_global(&obs.global);
        let model_started = Instant::now();
        let model = self.play(&mut env, obs, Pilot::Model, &mut scratch)?;
        let final_fast = Optics::from_global(&model.obs.global);
        let (base_score, final_score, base, fin) = if self.fidelity == EvalFidelity::Fast
            && self.cfg.train_fidelity == ObjectiveFidelity::Fast
        {
            (env.base_score(), env.current_score(), base_fast, final_fast)
        } else {
            match rescore_figures(
                &item.design,
                env.current_design(),
                &ctx,
                objective_fidelity(self.fidelity),
            ) {
                Ok(figures) => figures,
                Err(e) => return Ok(full_failed(case, item, &e)),
            }
        };
        let mut figures = DesignFigures {
            case: case.clone(),
            base_score,
            final_score,
            base,
            fin,
            sampled_final_score: None,
            oracle_final_score: None,
            model_evaluations: model.evaluations,
            model_steps: model.steps,
        };
        figures.case.model_seconds = Some(model_started.elapsed().as_secs_f32());

        // The same environment, back at the base design: every extra pass starts from the very
        // stone, target and weights the model's episode did.
        if self.passes.sampled {
            let pilot = Pilot::Sampled {
                rng: Prng::new(salted),
                explore_std: self.explore_std(),
            };
            match self.replay(&mut env, &ctx, pilot, &mut scratch)? {
                Ok(score) => figures.sampled_final_score = Some(score),
                Err(e) => return Ok(full_failed(case, item, &e)),
            }
        }
        if self.passes.oracle {
            match self.replay(&mut env, &ctx, Pilot::Oracle, &mut scratch)? {
                Ok(score) => figures.oracle_final_score = Some(score),
                Err(e) => return Ok(full_failed(case, item, &e)),
            }
        }
        figures.case.base_score = Some(figures.base_score);
        figures.case.final_score = Some(figures.final_score);
        figures.case.score_drop = Some(figures.base_score - figures.final_score);
        figures.case.accepted = Some(figures.final_score < figures.base_score);
        figures.case.evaluations = figures.model_evaluations;
        figures.case.steps = figures.model_steps;
        // Keep model throughput separate from the optional comparator's CPU time.
        figures.case.elapsed_seconds = Some(started.elapsed().as_secs_f32());
        if let Some(budget) = self.cfg.classical_budget {
            self.classical_comparison(item, &ctx, budget, &mut figures.case)?;
        }
        Ok(DesignResult::Figures(figures))
    }

    /// Runs the classical optimizer on `item` with the same weights, lighting and shape target as
    /// the model's episode and records its figures in `case`.
    fn classical_comparison(
        &self,
        item: &DesignEntry,
        ctx: &DesignContext,
        budget: usize,
        case: &mut EvalCase,
    ) -> Result<(), RunnerError> {
        let salted = ctx.context_seed;
        let weights = ctx.weights;
        let (shape_target, lighting) = sampled_context(salted);
        let started = Instant::now();
        let material = if let Some(target) = &ctx.target {
            target.material.clone()
        } else {
            sampled_source_material(salted)?.1
        };
        let optimizer_config = OptimizeConfig {
            weights,
            seed: salted,
            max_evaluations: budget,
            polish_start_step_deg: None,
            lighting,
            ..OptimizeConfig::default()
        };
        let mut options = OptimizeOptions {
            shape_target,
            ..OptimizeOptions::default()
        };
        if self.cfg.mode == TrainingMode::Retarget {
            options.preservation_policy =
                indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET;
            options.min_girdle_fraction = Some(0.5);
            options.min_girdle_thinnest_fraction = Some(0.5);
        }
        match optimize_design_with(
            &item.design,
            &material.gem,
            &optimizer_config,
            &options,
            &SearchHooks::default(),
        ) {
            Ok(result) => {
                let report_fidelity = match self.fidelity {
                    EvalFidelity::Fast => ObjectiveFidelity::Fast,
                    EvalFidelity::Full => ObjectiveFidelity::Full,
                };
                let baseline = score_design_at_fidelity(
                    &item.design,
                    &material.gem,
                    &weights,
                    report_fidelity,
                    lighting,
                    shape_target,
                )
                .map_err(|error| error.to_string());
                let candidate = if result.outcome.changes.is_empty() {
                    Ok(item.design.clone())
                } else {
                    indicatrix_cut_core::optimize::materialize_outcome(
                        &item.design,
                        &result.outcome,
                    )
                    .map(|(design, _, _)| design)
                    .map_err(|error| error.to_string())
                };
                match (baseline, candidate) {
                    (Ok(baseline), Ok(design)) => match score_design_at_fidelity(
                        &design,
                        &material.gem,
                        &weights,
                        report_fidelity,
                        lighting,
                        shape_target,
                    ) {
                        Ok(final_score) => {
                            case.classical_base_score = Some(baseline.score);
                            case.classical_final_score = Some(final_score.score);
                        }
                        Err(error) => case.classical_failure = Some(error.to_string()),
                    },
                    (Err(error), _) | (_, Err(error)) => case.classical_failure = Some(error),
                }
                case.classical_evaluations = result.outcome.evaluations;
            }
            Err(error) => case.classical_failure = Some(error.to_string()),
        }
        case.classical_seconds = Some(started.elapsed().as_secs_f32());

        Ok(())
    }
}

/// The report row of `item` before it is played: which material, objective and context it
/// draws.
fn new_case(
    item: &DesignEntry,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    salted: u64,
) -> Result<EvalCase, RunnerError> {
    let (shape_target, lighting) = sampled_context(salted);
    let (source_name, _, augmentation) = sampled_source_material(salted)?;
    Ok(EvalCase {
        design_id: item.id,
        design_title: item.title.clone(),
        objective: objective_name(weights),
        target_material: target.map_or_else(|| source_name.clone(), |target| target.name.clone()),
        source_material: source_name,
        seed: salted,
        material_augmentation: augmentation,
        target_augmentation: target
            .map_or_else(|| "none".to_owned(), |target| target.augmentation.clone()),
        lighting: format!("{lighting:?}"),
        shape_target: shape_target.map_or_else(
            || "none".to_owned(),
            |shape| {
                format!(
                    "table={:?}, crown/pavilion={:?}, weight={:.2}",
                    shape.table_percent, shape.crown_to_pavilion, shape.weight
                )
            },
        ),
        ..EvalCase::default()
    })
}

/// The failure text of a design whose full-fidelity re-score could not be built.
fn full_failure(item: &DesignEntry, error: &EnvError) -> String {
    format!("'{}' (full fidelity): {error}", item.title)
}

/// `case` marked as failed because its full-fidelity re-score could not be built.
fn full_failed(mut case: EvalCase, item: &DesignEntry, error: &EnvError) -> DesignResult {
    case.failure = Some(full_failure(item, error));
    DesignResult::Failed(case)
}

fn objective_name(weights: ObjectiveWeights) -> String {
    indicatrix_cut_core::ObjectivePreset::ALL
        .into_iter()
        .find(|preset| preset.weights() == weights)
        .map_or_else(|| "weighted".to_owned(), |preset| format!("{preset:?}"))
}

/// Full-sweep scores and optics of `before` and `after` (each about 1.3 s of tracing).
fn rescore_figures(
    before: &Design,
    after: &Design,
    ctx: &DesignContext,
    fidelity: ObjectiveFidelity,
) -> Result<(f32, f32, Optics, Optics), EnvError> {
    let material = if let Some(target) = &ctx.target {
        target.material.clone()
    } else {
        sampled_source_material(ctx.context_seed)
            .map_err(|_| EnvError::InvalidConfig("trainer source material could not be resolved"))?
            .1
    };
    let (shape, lighting) = sampled_context(ctx.context_seed);
    let score = |design: &Design| {
        score_design_at_fidelity(
            design,
            &material.gem,
            &ctx.weights,
            fidelity,
            lighting,
            shape,
        )
        .map_err(|_| EnvError::InvalidConfig("full-fidelity endpoint could not be scored"))
    };
    let b = score(before)?;
    let a = score(after)?;
    let optics = |readout: &indicatrix_ai::inference::ScoreReadout| Optics {
        brilliance: readout.components.tilt_brilliance_pct,
        windowing: readout.components.windowing_pct,
        extinction: readout.components.extinction_pct,
        yield_pct: readout.yield_pct.unwrap_or(0.0),
    };
    Ok((b.score, a.score, optics(&b), optics(&a)))
}

/// Full-sweep score of `design`.
fn rescore_final_score(
    design: &Design,
    ctx: &DesignContext,
    fidelity: ObjectiveFidelity,
) -> Result<f32, EnvError> {
    let (shape, lighting) = sampled_context(ctx.context_seed);
    let material = if let Some(target) = &ctx.target {
        target.material.clone()
    } else {
        sampled_source_material(ctx.context_seed)
            .map_err(|_| EnvError::InvalidConfig("trainer source material could not be resolved"))?
            .1
    };
    score_design_at_fidelity(
        design,
        &material.gem,
        &ctx.weights,
        fidelity,
        lighting,
        shape,
    )
    .map(|readout| readout.score)
    .map_err(|_| EnvError::InvalidConfig("full-fidelity endpoint could not be scored"))
}

const fn objective_fidelity(fidelity: EvalFidelity) -> ObjectiveFidelity {
    match fidelity {
        EvalFidelity::Fast => ObjectiveFidelity::Fast,
        EvalFidelity::Full => ObjectiveFidelity::Full,
    }
}

/// Adds the readings of `v` to the running sums in `acc`.
fn add_optics(acc: &mut Optics, v: Optics) {
    acc.brilliance += v.brilliance;
    acc.windowing += v.windowing;
    acc.extinction += v.extinction;
    acc.yield_pct += v.yield_pct;
}

/// The sums in `o` divided by the design count `n`.
fn mean_optics(o: Optics, n: f32) -> Optics {
    Optics {
        brilliance: o.brilliance / n,
        windowing: o.windowing / n,
        extinction: o.extinction / n,
        yield_pct: o.yield_pct / n,
    }
}

/// Evaluates `entries` (indices into the data set) as `job` describes, in parallel, and reduces
/// the outcomes in entry order.
pub(super) fn evaluate_designs(
    job: EvalJob<'_>,
    entries: &[usize],
) -> Result<EvalReport, RunnerError> {
    let tasks: Vec<_> = if job.cfg.objective_sweep {
        entries
            .iter()
            .flat_map(|&entry| {
                indicatrix_cut_core::ObjectivePreset::ALL
                    .into_iter()
                    .map(move |preset| (entry, Some(preset.weights())))
            })
            .collect()
    } else {
        entries.iter().map(|&entry| (entry, None)).collect()
    };
    let results = pool::run_indexed(tasks.len(), pool::pass_threads(tasks.len()), |i| {
        let (entry, weights) = tasks[i];
        job.evaluate_design(entry, weights)
    });
    reduce(job.fidelity, job.passes, results)
}

/// Serial reduction in entry order: sums, failure count and the first failure (the first failing
/// entry), or the first hard error. The order is the entry order whatever the thread count, so
/// the sums round identically.
fn reduce(
    fidelity: EvalFidelity,
    passes: EvalPasses,
    results: Vec<DesignOutcome>,
) -> Result<EvalReport, RunnerError> {
    let mut report = EvalReport {
        full_fidelity: fidelity == EvalFidelity::Full,
        ..EvalReport::default()
    };
    let (mut base_score, mut final_score) = (0.0f32, 0.0f32);
    let (mut sampled_drop, mut oracle_drop) = (0.0f32, 0.0f32);
    let (mut model_evaluations, mut model_steps) = (0usize, 0usize);
    let (mut base, mut fin) = (Optics::default(), Optics::default());
    for result in results {
        match result? {
            DesignResult::Failed(case) => {
                report.failed += 1;
                report
                    .first_failure
                    .get_or_insert_with(|| case.failure.clone().unwrap_or_default());
                report.cases.push(case);
            }
            DesignResult::Figures(f) => {
                report.designs += 1;
                base_score += f.base_score;
                final_score += f.final_score;
                sampled_drop += f.sampled_final_score.map_or(0.0, |s| f.base_score - s);
                oracle_drop += f.oracle_final_score.map_or(0.0, |s| f.base_score - s);
                model_evaluations += f.model_evaluations;
                model_steps += f.model_steps;
                add_optics(&mut base, f.base);
                add_optics(&mut fin, f.fin);
                report.cases.push(f.case);
            }
        }
    }
    if report.designs > 0 {
        let n = report.designs as f32;
        report.mean_base_score = base_score / n;
        report.mean_final_score = final_score / n;
        report.mean_improvement = report.mean_base_score - report.mean_final_score;
        report.mean_improvement_sampled = passes.sampled.then_some(sampled_drop / n);
        report.mean_improvement_oracle = passes.oracle.then_some(oracle_drop / n);
        report.mean_evaluations_per_step = if model_steps == 0 {
            0.0
        } else {
            model_evaluations as f32 / model_steps as f32
        };
        report.base_optics = mean_optics(base, n);
        report.final_optics = mean_optics(fin, n);
    }
    Ok(report)
}

#[cfg(test)]
#[path = "runner_eval_tests.rs"]
mod tests;

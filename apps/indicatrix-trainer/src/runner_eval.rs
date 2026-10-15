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
//!   model's action (the upper bound of what this policy class does per step).
//!
//! All three score the same stone against the same target under the same objective weights, so
//! `model / oracle` is the figure to watch. The end-of-run test evaluation and every in-training
//! validation pass used to play their designs one after another on one core (the 570 test
//! designs took minutes at `Fast` and hours at `Full`). Designs are independent, so they are
//! handed to a pool of scoped threads one at a time (a design with many free tiers costs many
//! times a small one) and every outcome is written into the slot of its entry; the report is
//! then reduced serially in entry order. Nothing is shared but immutable data (the network, the
//! data set, the configuration), so the result is identical for any thread count.

use indicatrix_ai::{
    env::{
        ContinuousTierAction, EnvError, GemstoneObservation, action_from_direction,
        direction_from_slopes, score_slopes,
    },
    model::{GaussianPolicy, GemstoneSetAttentionNetwork, InferenceScratch, Prng, TrainingMode},
};
use indicatrix_cut_core::{Design, ObjectiveWeights, optimize::ObjectiveFidelity};

use super::{
    Algorithm, EvalFidelity, EvalPasses, EvalReport, ObjectiveSampling, Optics, RunConfig,
    RunnerError, build_env, draw_weights,
    envs::{EnvKind, Target},
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
#[derive(Clone, Copy)]
struct DesignFigures {
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
    Failed(String),
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
        Ok(match self.fidelity {
            EvalFidelity::Fast => Ok(env.current_score()),
            EvalFidelity::Full => full_final_score(self.cfg, env.current_design(), ctx),
        })
    }

    /// Evaluates one design: build the environment, play the model's episode, optionally
    /// re-score at full fidelity, then the requested extra passes. A design whose environment
    /// (or full-fidelity re-score) cannot be built is a [`DesignResult::Failed`], not an error;
    /// a material or network failure is an error.
    fn evaluate_design(&self, entry: usize) -> DesignOutcome {
        let item = &self.split.entries[entry];
        let salted = self.seed ^ (entry as u64).wrapping_mul(0x9E37_79B9);
        let target = pick_target(self.cfg, salted, &item.design)?;
        let weights = draw_weights(self.cfg.objectives, salted);
        let mut env = match build_env(self.cfg, &item.design, target.as_ref(), weights, None) {
            Ok(env) => env,
            Err(e) => return Ok(DesignResult::Failed(format!("'{}': {e}", item.title))),
        };
        let ctx = DesignContext { target, weights };
        let mut scratch = InferenceScratch::default();

        let obs = env.reset(self.seed);
        let base_fast = Optics::from_global(&obs.global);
        let model = self.play(&mut env, obs, Pilot::Model, &mut scratch)?;
        let final_fast = Optics::from_global(&model.obs.global);
        let (base_score, final_score, base, fin) = match self.fidelity {
            EvalFidelity::Fast => (env.base_score(), env.current_score(), base_fast, final_fast),
            EvalFidelity::Full => {
                match full_figures(self.cfg, &item.design, env.current_design(), &ctx) {
                    Ok(figures) => figures,
                    Err(e) => return Ok(DesignResult::Failed(full_failure(item, &e))),
                }
            }
        };
        let mut figures = DesignFigures {
            base_score,
            final_score,
            base,
            fin,
            sampled_final_score: None,
            oracle_final_score: None,
            model_evaluations: model.evaluations,
            model_steps: model.steps,
        };

        // The same environment, back at the base design: every extra pass starts from the very
        // stone, target and weights the model's episode did.
        if self.passes.sampled {
            let pilot = Pilot::Sampled {
                rng: Prng::new(salted),
                explore_std: self.explore_std(),
            };
            match self.replay(&mut env, &ctx, pilot, &mut scratch)? {
                Ok(score) => figures.sampled_final_score = Some(score),
                Err(e) => return Ok(DesignResult::Failed(full_failure(item, &e))),
            }
        }
        if self.passes.oracle {
            match self.replay(&mut env, &ctx, Pilot::Oracle, &mut scratch)? {
                Ok(score) => figures.oracle_final_score = Some(score),
                Err(e) => return Ok(DesignResult::Failed(full_failure(item, &e))),
            }
        }
        Ok(DesignResult::Figures(figures))
    }
}

/// The failure text of a design whose full-fidelity re-score could not be built.
fn full_failure(item: &DesignEntry, error: &EnvError) -> String {
    format!("'{}' (full fidelity): {error}", item.title)
}

/// Full-sweep scores and optics of `before` and `after` (each about 1.3 s of tracing).
fn full_figures(
    cfg: &RunConfig,
    before: &Design,
    after: &Design,
    ctx: &DesignContext,
) -> Result<(f32, f32, Optics, Optics), EnvError> {
    let full = Some(ObjectiveFidelity::Full);
    let b = build_env(cfg, before, ctx.target.as_ref(), ctx.weights, full)?;
    let a = build_env(cfg, after, ctx.target.as_ref(), ctx.weights, full)?;
    Ok((
        b.base_score(),
        a.base_score(),
        Optics::from_global(&b.build_observation().global),
        Optics::from_global(&a.build_observation().global),
    ))
}

/// Full-sweep score of `design`.
fn full_final_score(
    cfg: &RunConfig,
    design: &Design,
    ctx: &DesignContext,
) -> Result<f32, EnvError> {
    build_env(
        cfg,
        design,
        ctx.target.as_ref(),
        ctx.weights,
        Some(ObjectiveFidelity::Full),
    )
    .map(|env| env.base_score())
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
    let results = pool::run_indexed(entries.len(), pool::pass_threads(entries.len()), |i| {
        job.evaluate_design(entries[i])
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
            DesignResult::Failed(text) => {
                report.failed += 1;
                report.first_failure.get_or_insert(text);
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
mod tests {
    use super::*;

    fn optics(brilliance: f32) -> Optics {
        Optics {
            brilliance,
            ..Optics::default()
        }
    }

    /// Scores of a finished design.
    struct Scores {
        base: f32,
        fin: f32,
        sampled: Option<f32>,
        oracle: Option<f32>,
    }

    /// A finished design: its scores, the brilliance before (it gains 10 points in the model's
    /// episode) and the model's (evaluations, steps).
    #[expect(
        clippy::unnecessary_wraps,
        reason = "builds the Ok entries of the DesignOutcome list that reduce consumes"
    )]
    fn done(scores: &Scores, brilliance: f32, model: (usize, usize)) -> DesignOutcome {
        Ok(DesignResult::Figures(DesignFigures {
            base_score: scores.base,
            final_score: scores.fin,
            base: optics(brilliance),
            fin: optics(brilliance + 10.0),
            sampled_final_score: scores.sampled,
            oracle_final_score: scores.oracle,
            model_evaluations: model.0,
            model_steps: model.1,
        }))
    }

    fn scores(base: f32, fin: f32, sampled: Option<f32>, oracle: Option<f32>) -> Scores {
        Scores {
            base,
            fin,
            sampled,
            oracle,
        }
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "builds the Ok entries of the DesignOutcome list that reduce consumes"
    )]
    fn failed(text: &str) -> DesignOutcome {
        Ok(DesignResult::Failed(text.to_owned()))
    }

    const ALL_PASSES: EvalPasses = EvalPasses {
        sampled: true,
        oracle: true,
    };

    #[test]
    fn the_reduction_averages_the_designs_and_keeps_the_first_failure_in_entry_order() {
        let results = vec![
            failed("'a': no light"),
            done(&scores(10.0, 6.0, Some(8.0), Some(2.0)), 40.0, (90, 60)),
            failed("'c': unsolvable"),
            done(&scores(20.0, 12.0, Some(18.0), Some(8.0)), 60.0, (60, 60)),
        ];
        let report = reduce(EvalFidelity::Fast, ALL_PASSES, results).unwrap();
        assert_eq!((report.designs, report.failed), (2, 2));
        assert_eq!(report.first_failure.as_deref(), Some("'a': no light"));
        // Scores: base (10 + 20) / 2 = 15, final (6 + 12) / 2 = 9, drop 6.
        assert!((report.mean_base_score - 15.0).abs() < 1e-6);
        assert!((report.mean_final_score - 9.0).abs() < 1e-6);
        assert!((report.mean_improvement - 6.0).abs() < 1e-6);
        // Sampled drops 10 - 8 = 2 and 20 - 18 = 2.
        let sampled = report.mean_improvement_sampled.unwrap();
        assert!((sampled - 2.0).abs() < 1e-6);
        // Oracle drops 10 - 2 = 8 and 20 - 8 = 12.
        let oracle = report.mean_improvement_oracle.unwrap();
        assert!((oracle - 10.0).abs() < 1e-6);
        // Brilliance before (40 + 60) / 2 = 50, after 10 more.
        assert!((report.base_optics.brilliance - 50.0).abs() < 1e-4);
        assert!((report.final_optics.brilliance - 60.0).abs() < 1e-4);
        assert!(!report.full_fidelity);
        // The oracle is never marked as cached by the reduction: that is the runner's call.
        assert!(!report.oracle_cached);
    }

    #[test]
    fn without_the_oracle_figures_there_is_no_oracle_mean() {
        let results = vec![
            done(&scores(10.0, 6.0, Some(8.0), None), 40.0, (60, 60)),
            done(&scores(20.0, 12.0, Some(18.0), None), 60.0, (60, 60)),
        ];
        let report = reduce(
            EvalFidelity::Fast,
            EvalPasses {
                sampled: true,
                oracle: false,
            },
            results,
        )
        .unwrap();
        assert!(report.mean_improvement_oracle.is_none());
        assert!(report.mean_improvement_sampled.is_some());
        assert_eq!(report.designs, 2);
    }

    #[test]
    fn without_a_sampled_pass_there_is_no_sampled_mean() {
        let results = vec![done(&scores(10.0, 6.0, None, None), 40.0, (60, 60))];
        let report = reduce(EvalFidelity::Full, EvalPasses::default(), results).unwrap();
        assert!(report.mean_improvement_sampled.is_none());
        assert!(report.mean_improvement_oracle.is_none());
        assert!(report.full_fidelity);
        assert!((report.mean_improvement - 4.0).abs() < 1e-6);
    }

    #[test]
    fn the_evaluations_per_step_are_the_totals_over_the_designs() {
        // 120 evaluations over 70 steps, not the mean of the per-design ratios (1.5 and 3.0
        // would give 2.25).
        let results = vec![
            done(&scores(10.0, 6.0, None, None), 40.0, (90, 60)),
            done(&scores(20.0, 12.0, None, None), 60.0, (30, 10)),
        ];
        let report = reduce(EvalFidelity::Fast, EvalPasses::default(), results).unwrap();
        assert!((report.mean_evaluations_per_step - 120.0 / 70.0).abs() < 1e-6);
        // No steps, no ratio.
        let none = vec![done(&scores(1.0, 1.0, None, None), 0.0, (0, 0))];
        let report = reduce(EvalFidelity::Fast, EvalPasses::default(), none).unwrap();
        assert_eq!(report.mean_evaluations_per_step, 0.0);
    }

    #[test]
    fn nothing_evaluated_leaves_the_means_empty() {
        let report = reduce(EvalFidelity::Fast, ALL_PASSES, vec![failed("'x': dead")]).unwrap();
        assert_eq!((report.designs, report.failed), (0, 1));
        assert!(report.mean_improvement_sampled.is_none());
        assert!(report.mean_improvement_oracle.is_none());
        assert!(
            reduce(EvalFidelity::Fast, ALL_PASSES, Vec::new())
                .unwrap()
                .first_failure
                .is_none()
        );
    }

    #[test]
    fn a_hard_error_is_the_first_one_in_entry_order() {
        let results = vec![
            done(&scores(10.0, 6.0, None, None), 40.0, (60, 60)),
            Err(RunnerError::Data("first".into())),
            Err(RunnerError::Data("second".into())),
        ];
        let error = reduce(EvalFidelity::Fast, EvalPasses::default(), results).unwrap_err();
        assert_eq!(error.to_string(), "first");
    }

    #[test]
    fn the_oracle_key_changes_with_everything_the_oracle_depends_on() {
        let cfg = RunConfig::new(
            TrainingMode::Retarget,
            indicatrix_ai::model::ModelTier::Small,
        );
        let key = OracleKey::new(&cfg, &[0, 1], 7, 0.5);
        assert_eq!(key, OracleKey::new(&cfg, &[0, 1], 7, 0.5));
        assert_ne!(key, OracleKey::new(&cfg, &[0], 7, 0.5), "entries");
        assert_ne!(key, OracleKey::new(&cfg, &[0, 1], 8, 0.5), "seed");
        assert_ne!(key, OracleKey::new(&cfg, &[0, 1], 7, 0.25), "step limit");
        let changed = |edit: &dyn Fn(&mut RunConfig)| {
            let mut other = cfg.clone();
            edit(&mut other);
            OracleKey::new(&other, &[0, 1], 7, 0.5)
        };
        assert_ne!(key, changed(&|c| c.mode = TrainingMode::Optimizer), "mode");
        assert_ne!(key, changed(&|c| c.universal_spectrum = false), "spectrum");
        assert_ne!(key, changed(&|c| c.fixed_target = "Ruby".into()), "target");
        assert_ne!(key, changed(&|c| c.line_search = vec![1.0]), "line search");
        assert_ne!(key, changed(&|c| c.label_h_deg = Some(1.0)), "probe size");
        assert_ne!(
            key,
            changed(&|c| c.objectives = ObjectiveSampling::Balanced),
            "objectives"
        );
        assert_ne!(
            key,
            changed(&|c| c.same_material_frac = 0.0),
            "same material"
        );
        // Settings the oracle does not depend on leave the key alone.
        assert_eq!(key, changed(&|c| c.workers += 1));
        assert_eq!(key, changed(&|c| c.explore_std += 0.1));
        assert_eq!(key, changed(&|c| c.corpus_mode = !c.corpus_mode));
    }
}

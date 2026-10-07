//! The multi-start search on a real [`Design`]: the [`StartEngine`] that scores, descends
//! and polishes with the same machinery the single-start search uses, and
//! [`run_multi_start`], which hands it to the generic driver in
//! [`crate::optimize::multistart`].

use super::{
    Baseline, CoordinateStageOutcome, OptimizeConfig, SearchHooks, SearchRunSummary, SearchStage,
    stages::{StartRun, run_polish_stage, run_search},
};
use crate::{
    design::Design,
    optimize::{
        candidate::{
            CandidateOutcome, SearchContext, apply_free_angles, build_free_angle_candidate,
            evaluate_in_context,
        },
        multistart::{
            DescentRun, DriverHooks, MultiSpec, StartEngine, StartPoint, StartState, run_multistart,
        },
        options::OptimizeOptions,
        pool::CandidatePool,
    },
};
use std::sync::atomic::AtomicBool;

/// Everything [`run_multi_start`] needs from [`optimize_design_with`](super::optimize_design_with),
/// bundled to keep the argument count down.
pub(super) struct MultiRun<'a> {
    pub(super) design: &'a Design,
    pub(super) config: &'a OptimizeConfig,
    pub(super) options: &'a OptimizeOptions,
    pub(super) ctx: &'a SearchContext<'a>,
    pub(super) baseline: &'a Baseline,
    /// The effective number of starts (at least two).
    pub(super) starts: usize,
}

/// The [`StartEngine`] over one design and its search context.
struct DesignEngine<'a> {
    run: &'a MultiRun<'a>,
    /// The design's own free angles, the reference every drawn point is safety-checked
    /// against.
    incumbent: &'a [f64],
    cancel: Option<&'a AtomicBool>,
}

impl DesignEngine<'_> {
    fn free_angles(&self, design: &Design) -> Vec<f64> {
        self.run
            .baseline
            .free
            .iter()
            .map(|&tier| design.tiers[tier].angle_deg)
            .collect()
    }

    fn hooks<'h>(&'h self, progress: &'h dyn Fn(usize, SearchStage)) -> SearchHooks<'h> {
        SearchHooks {
            cancel: self.cancel,
            on_progress: Some(progress),
            on_start: None,
        }
    }

    /// A start that could not be built (a tier has no usable mast): no evaluations, an
    /// infinite score, so it never wins and never offers a pool entry.
    fn dead(angles: &[f64]) -> StartState {
        StartState {
            angles: angles.to_vec(),
            score: f32::INFINITY,
            evaluations: 0,
            polish_evaluations: 0,
            polish_improvement: 0.0,
            cancelled: false,
            pool: CandidatePool::new(0, 0.0),
        }
    }
}

impl StartEngine for DesignEngine<'_> {
    fn score(&self, angles: &[f64]) -> Option<f32> {
        let run = self.run;
        let candidate = build_free_angle_candidate(
            run.design,
            &run.baseline.free,
            self.incumbent,
            angles,
            run.ctx.space,
        )?;
        match evaluate_in_context(&candidate, run.ctx) {
            CandidateOutcome::Rejected => None,
            CandidateOutcome::Accepted { score } => Some(score),
        }
    }

    fn descend(
        &self,
        descent: &DescentRun<'_>,
        progress: &dyn Fn(usize, SearchStage),
    ) -> StartState {
        let run = self.run;
        // Start 0 is the incumbent itself: hand over the design untouched, exactly as the
        // single-start path does, instead of re-writing every angle and mast.
        let is_incumbent = descent.angles.len() == self.incumbent.len()
            && descent
                .angles
                .iter()
                .zip(self.incumbent)
                .all(|(a, b)| a.to_bits() == b.to_bits());
        let start = if is_incumbent {
            run.design.clone()
        } else {
            let Some(start) = apply_free_angles(
                run.design,
                &run.baseline.free,
                descent.angles,
                run.ctx.space,
            ) else {
                return Self::dead(descent.angles);
            };
            start
        };
        let coord = run_search(
            &start,
            run.config,
            run.ctx,
            run.baseline,
            self.new_pool(),
            &self.hooks(progress),
            &StartRun {
                max_evaluations: descent.max_evaluations,
                seed: descent.seed,
                initial_score: descent.score,
            },
        );
        StartState {
            angles: self.free_angles(&coord.design),
            score: coord.score,
            evaluations: coord.evaluations,
            polish_evaluations: 0,
            polish_improvement: 0.0,
            cancelled: coord.cancelled,
            pool: coord.pool,
        }
    }

    fn polish(&self, state: &StartState, progress: &dyn Fn(usize, SearchStage)) -> StartState {
        let run = self.run;
        let Some(design) =
            apply_free_angles(run.design, &run.baseline.free, &state.angles, run.ctx.space)
        else {
            return state.clone();
        };
        let coord = CoordinateStageOutcome {
            design,
            score: state.score,
            evaluations: state.evaluations,
            cancelled: false,
            pool: state.pool.clone(),
        };
        let polished = run_polish_stage(
            run.design,
            run.baseline,
            run.config,
            run.ctx,
            &self.hooks(progress),
            coord,
        );
        StartState {
            angles: self.free_angles(&polished.current),
            score: state.score - polished.improvement,
            evaluations: state.evaluations,
            polish_evaluations: polished.evaluations,
            polish_improvement: polished.improvement,
            cancelled: polished.cancelled,
            pool: polished.pool,
        }
    }

    fn new_pool(&self) -> CandidatePool {
        CandidatePool::new(
            self.run.options.candidate_capacity(),
            self.run
                .options
                .candidate_separation_deg
                .unwrap_or(self.run.config.min_step_deg),
        )
    }
}

/// The multi-start search: [`MultiRun::starts`] descents from the design itself and from
/// screened points of the search box, the best few polished, merged into one result.
/// Returns the design the best start ended on, the merged candidate pool and the run
/// summary, the same triple the single-start search hands to `build_result`.
pub(super) fn run_multi_start(
    run: &MultiRun<'_>,
    hooks: &SearchHooks<'_>,
) -> (Design, CandidatePool, SearchRunSummary) {
    let incumbent_angles: Vec<f64> = run
        .baseline
        .free
        .iter()
        .map(|&tier| run.design.tiers[tier].angle_deg)
        .collect();
    let boxes: Vec<(f64, f64)> = run
        .baseline
        .free
        .iter()
        .zip(&incumbent_angles)
        .map(|(&tier, &angle)| run.ctx.space.start_box(tier, angle))
        .collect();
    let engine = DesignEngine {
        run,
        incumbent: &incumbent_angles,
        cancel: hooks.cancel,
    };
    let spec = MultiSpec {
        starts: run.starts,
        max_lanes: run.config.max_lanes,
        budget: run.config.max_evaluations,
        polish_keep: run.options.candidate_capacity(),
        seed: run.config.seed,
        separation_deg: run
            .options
            .candidate_separation_deg
            .unwrap_or(run.config.min_step_deg),
        cancel: hooks.cancel,
    };
    let outcome = run_multistart(
        &engine,
        &spec,
        &StartPoint {
            angles: incumbent_angles.clone(),
            score: run.baseline.before_score_fast,
        },
        &boxes,
        &DriverHooks {
            report: hooks.on_progress,
            on_start: hooks.on_start,
        },
    );
    let best = apply_free_angles(
        run.design,
        &run.baseline.free,
        &outcome.best_angles,
        run.ctx.space,
    )
    .unwrap_or_else(|| run.design.clone());
    let summary = SearchRunSummary {
        evaluations: outcome.evaluations,
        cancelled: outcome.cancelled,
        polish_evaluations: outcome.polish_evaluations,
        polish_improvement: outcome.polish_improvement,
        starts_run: outcome.starts_run,
        best_start: outcome.best_start,
    };
    (best, outcome.pool, summary)
}

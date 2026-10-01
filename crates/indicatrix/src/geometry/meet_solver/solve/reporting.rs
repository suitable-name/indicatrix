//! Turns a finished [`PipelineResult`] into caller-facing output: per-tier
//! [`SolvedTier`] reports ([`SolveContext::to_solved`]) and the external
//! verification score against printed proportions
//! ([`SolveContext::config_score`]).

use glam::DVec3;

use super::{
    super::{SolveStrategy, SolvedTier},
    context::SolveContext,
    pipeline::{Origin, PipelineResult},
};
use crate::geometry::stone_metrics::{ExternalProportions, measure_solid};

impl SolveContext<'_> {
    /// Maps one pipeline run's raw result to the per-tier [`SolvedTier`] reports.
    pub(in crate::geometry::meet_solver) fn to_solved(
        &self,
        result: &PipelineResult,
    ) -> Vec<SolvedTier> {
        let variant = "file-order";
        let refine_sweeps = result.refine_sweeps;
        let conv = if result.converged {
            "converged"
        } else {
            "sweep cap"
        };
        // `result.mast` is in `run_pipeline`'s normalised (order-1)
        // units (see `SolveContext::scale_norm`'s doc comment); this is the
        // one place that scales every solved mast back to the design's real
        // absolute units before it reaches a caller. A bit-exact no-op when
        // `scale_norm == 1.0`.
        let scale_norm = self.scale_norm;
        (0..self.tiers.len())
            .map(|i| {
                let mast = result.mast[i] * scale_norm;
                let pick_str = result.last_pick[i].map_or_else(
                    || "no vertex pick".to_string(),
                    |(li, nl)| format!("level {li} of {nl}"),
                );
                match result.origin[i] {
                    Origin::Anchor => SolvedTier {
                        mast,
                        strategy: SolveStrategy::ScaleReference,
                        detail: "given (scale reference)".to_string(),
                    },
                    Origin::ConstructiveNamed => SolvedTier {
                        mast,
                        strategy: SolveStrategy::DependencyOrder,
                        detail: format!(
                            "constructive vertex incidence ({variant} pipeline) with {} resolved \
                             named reference(s), then {pick_str} after {refine_sweeps} refinement \
                             sweep(s) ({conv})",
                            self.resolved_named[i].len()
                        ),
                    },
                    Origin::ConstructiveRank1 => SolvedTier {
                        mast,
                        strategy: SolveStrategy::DependencyOrder,
                        detail: {
                            let cause = result.named_cause[i]
                                .map_or_else(String::new, |c| format!(" [named fallback: {c}]"));
                            format!(
                                "constructive vertex incidence ({variant} pipeline, rank-1 \
                                 prior{cause}), then {pick_str} after {refine_sweeps} refinement \
                                 sweep(s) ({conv})"
                            )
                        },
                    },
                    Origin::Refined => SolvedTier {
                        mast,
                        strategy: SolveStrategy::JointGroup,
                        detail: format!(
                            "mutually-dependent remainder settled by nearest-level refinement \
                            ({variant} pipeline): {pick_str} after {refine_sweeps} sweep(s) ({conv})"
                        ),
                    },
                    Origin::Estimated => SolvedTier {
                        mast,
                        strategy: SolveStrategy::LeastSquaresFallback,
                        detail: "no usable candidate vertex; mast is the per-block \
                                 a*cos(theta)+b*sin(theta) estimate"
                            .to_string(),
                    },
                    Origin::Unset => SolvedTier {
                        mast,
                        strategy: SolveStrategy::Failed,
                        detail: "no vertex and no estimate; mast is the scale prior".to_string(),
                    },
                }
            })
            .collect()
    }

    /// External combined-deviation score of one mast configuration against the
    /// design's printed proportions: [`measure_solid`] over the configuration's
    /// full plane arrangement, then
    /// [`ExternalProportions::combined_deviation`]. `INFINITY` when the solid is
    /// unbounded/degenerate or no printed figure overlaps the measurement --
    /// such a configuration can never be verified.
    pub(in crate::geometry::meet_solver) fn config_score(
        &self,
        masts: &[f64],
        targets: &ExternalProportions,
    ) -> f64 {
        let planes: Vec<(DVec3, f64)> = self
            .normals
            .iter()
            .zip(masts)
            .flat_map(|(ns, &m)| ns.iter().map(move |&n| (n, m)))
            .collect();
        measure_solid(&planes)
            .and_then(|m| targets.combined_deviation(&m))
            .unwrap_or(f64::INFINITY)
    }
}

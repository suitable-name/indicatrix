//! [`SolveContext`]: precomputed, immutable per-design solve state shared by
//! every pipeline run.

use glam::DVec3;

use super::super::{
    BLANK_DOMINATION_FACTOR, DEFAULT_PLAUSIBLE_SCALE, MAX_PLANES, MeetConstraint, MeetTierInput,
    SolveStrategy, SolvedTier,
    blocks::{Block, classify_blocks, tier_sides},
    candidates::tier_normals,
    names::MeetNameResolver,
};

/// Precomputed, immutable per-design solve state: everything every pipeline run
/// shares (normals, block classification, name resolution, anchors, priors).
///
/// Built once per design; [`Self::run_pipeline`](super::pipeline) can then run
/// any number of times -- with different decision overrides -- without
/// re-deriving any of it. That reuse is what makes
/// [`solve_meet_points_verified`](super::super::solve_meet_points_verified)'s
/// externally-scored repair search affordable.
pub(in crate::geometry::meet_solver) struct SolveContext<'a> {
    pub(in crate::geometry::meet_solver) tiers: &'a [MeetTierInput],
    pub(super) normals: Vec<Vec<DVec3>>,
    pub(super) blocks: Vec<Block>,
    pub(super) resolved_named: Vec<Vec<usize>>,
    pub(in crate::geometry::meet_solver) is_anchor: Vec<bool>,
    pub(super) scale_prior: f64,
    pub(super) domination_limit: f64,
    pub(in crate::geometry::meet_solver) total_planes: usize,
}

impl<'a> SolveContext<'a> {
    /// Builds a new context, precomputing per-tier instance normals, block
    /// classification, resolved `"Meet <names>"` references, anchor flags, and
    /// the scale prior/domination limit -- see the type doc for why this is
    /// done once and reused.
    pub(in crate::geometry::meet_solver) fn new(
        gear_teeth_abs: u32,
        tiers: &'a [MeetTierInput],
    ) -> Self {
        let sides = tier_sides(tiers);
        let normals: Vec<Vec<DVec3>> = tiers
            .iter()
            .zip(&sides)
            .map(|(t, &crown)| tier_normals(gear_teeth_abs, t.angle_deg, &t.indices, crown))
            .collect();

        let blocks = classify_blocks(tiers);

        // Resolve every MeetNamed tier's references up front via the shared
        // [`MeetNameResolver`] rule set (see its doc comment).
        let resolver = MeetNameResolver::new(tiers);
        let resolved_named: Vec<Vec<usize>> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| match &t.constraint {
                MeetConstraint::MeetNamed(names) => {
                    let mut refs = resolver.resolve_names(names).refs;
                    refs.retain(|&r| r != i);
                    refs
                }
                _ => Vec::new(),
            })
            .collect();

        let is_anchor: Vec<bool> = tiers
            .iter()
            .map(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_)))
            .collect();
        let scale_prior = tiers
            .iter()
            .filter_map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => Some(v.abs()),
                _ => None,
            })
            .fold(0.0_f64, f64::max);
        let scale_prior = if scale_prior > 1e-9 {
            scale_prior
        } else {
            DEFAULT_PLAUSIBLE_SCALE
        };
        let domination_limit = BLANK_DOMINATION_FACTOR * scale_prior;

        let total_planes = 6 + normals.iter().map(Vec::len).sum::<usize>();
        Self {
            tiers,
            normals,
            blocks,
            resolved_named,
            is_anchor,
            scale_prior,
            domination_limit,
            total_planes,
        }
    }

    /// The over-[`MAX_PLANES`] early-out result: anchors keep their given masts,
    /// everything else is a flagged placeholder.
    pub(in crate::geometry::meet_solver) fn failed_solved(&self) -> Vec<SolvedTier> {
        let total_planes = self.total_planes;
        self.tiers
            .iter()
            .map(|t| match &t.constraint {
                MeetConstraint::ScaleReference(v) => SolvedTier {
                    mast: v.abs(),
                    strategy: SolveStrategy::ScaleReference,
                    detail: "given (scale reference)".to_string(),
                },
                _ => SolvedTier {
                    mast: self.scale_prior,
                    strategy: SolveStrategy::Failed,
                    detail: format!(
                        "design has {total_planes} planes, above the {MAX_PLANES}-plane cap \
                         for candidate-vertex enumeration"
                    ),
                },
            })
            .collect()
    }
}

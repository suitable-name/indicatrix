//! [`Design::stone_proportions`]: the design-context wrapper over
//! [`indicatrix::geometry::stone_metrics::StoneProportions`], the same shape
//! [`super::fit::exceeds_preform`] and [`Design::yield_report`] already use
//! for [`indicatrix::geometry::stone_metrics::SolidMetrics`]/[`super::PreformFit`].

use crate::design::Design;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
};

impl Design {
    /// This design's proportion readouts -- table size as a percentage of
    /// width, crown height, pavilion depth, girdle thickness, total depth,
    /// and the length/crown/pavilion/girdle-to-width ratios -- computed from
    /// `solved`, an already-
    /// [`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list, rather
    /// than solving again. `None` when the design isn't currently a closed
    /// solid (the same condition [`Self::status`] reports as anything other
    /// than [`SolidStatus::Closed`]).
    ///
    /// See [`StoneProportions`]'s own doc comment for the unit convention
    /// (mast units; multiply by [`super::mm_per_unit`] or call
    /// [`StoneProportions::to_mm`] for a real millimetre figure).
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`]: `solved`
    /// must have one entry per tier `self` currently has, in the same order.
    #[must_use]
    pub fn stone_proportions(&self, solved: &[SolvedTier]) -> Option<StoneProportions> {
        let planes = self.planes_from_solved(solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return None;
        };
        let metrics = measure_solid(&planes)?;
        Some(StoneProportions::from_solid(&metrics, &mesh, &planes))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        design::{ConstraintTier, Design, ScheduleMeta},
        preform::PreformSpec,
    };

    /// A solved standard round brilliant must report a table percent
    /// between 0 and 100 and a length-to-width ratio near 1.0 (a round
    /// stone).
    #[test]
    fn stone_proportions_reads_plausible_figures_for_a_round_brilliant() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let proportions = design
            .stone_proportions(&solved)
            .expect("round brilliant template must close");
        let table_percent = proportions.table_percent.expect("table tier is present");
        assert!(
            table_percent > 0.0 && table_percent < 100.0,
            "{table_percent}"
        );
        let lw = proportions.length_to_width.expect("positive width");
        assert!((lw - 1.0).abs() < 1e-6, "{lw}");
        assert!(proportions.crown_height.expect("girdle present") > 0.0);
        assert!(proportions.pavilion_depth.expect("girdle present") > 0.0);
        assert!(proportions.crown_to_width_percent.expect("girdle present") > 0.0);
        assert!(
            proportions
                .pavilion_to_width_percent
                .expect("girdle present")
                > 0.0
        );
        assert!(proportions.girdle_to_width_percent.expect("girdle present") > 0.0);
    }

    /// An empty design (no tiers, just the preform) never closes on its own
    /// facets alone in a way that has a meaningful table -- but here it's
    /// the preform box itself that closes, which trivially reports a 100%
    /// table (its own flat top). The real behavior under test is that
    /// `stone_proportions` never panics on a solved-but-tierless design.
    #[test]
    fn stone_proportions_handles_a_tierless_design() {
        let design = Design::new(
            PreformSpec::block(1.0, 1.0, 1.0),
            ScheduleMeta::default(),
            vec![],
        );
        let solved = design.solve().expect("no tiers to solve");
        assert!(design.stone_proportions(&solved).is_some());
    }
}

//! Resolve tests for [`crate::design::TierTarget`]: `GirdleThicknessMm`,
//! `TableWidthMm` and `DepthMm` must each actually resolve on a real design --
//! not `CannotBracket` for every value, and not a self-inconsistent one-pass
//! depth guess -- and an incremental resolve must re-derive a target exactly
//! like a full solve would. See `crate::design::targets`'s own module doc
//! comment for the algorithm these exercise.

use crate::{
    design::{ConstraintTier, Design, ScheduleMeta, TierTarget},
    preform::PreformSpec,
};
use indicatrix::geometry::{
    meet_solver::MeetConstraint,
    stone_metrics::{SolidStatus, StoneProportions, build_solid_mesh, measure_solid},
};

/// Tier indices in [`ConstraintTier::standard_round_brilliant`]'s own table.
const TABLE: usize = 0;
const GIRDLE: usize = 4;
const PAVILION_MAIN: usize = 5;

/// [`ConstraintTier::standard_round_brilliant`] over a block preform, with a
/// real 6.5 mm girdle diameter set -- the fixture every test below targets.
fn standard_round_brilliant_6_5mm_girdle() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.girdle_diameter_mm = Some(6.5);
    design
}

/// Solves `design` (through [`Design::solve`], which resolves any
/// [`TierTarget`] first) and returns `extract`'s measured mm figure for the
/// finished solid -- the same measurement `crate::design::targets::bisect_tier_mast`
/// itself checks against, built here from public API only.
fn measured_mm(design: &Design, extract: impl Fn(&StoneProportions, f64) -> Option<f64>) -> f64 {
    let solved = design
        .solve()
        .expect("design with a resolved target must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("must measure as a solid");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("must close into a real solid");
    };
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let scale = crate::yield_metrics::mm_per_unit(
        design
            .girdle_diameter_mm
            .expect("fixture sets a girdle diameter"),
        metrics.width_axis,
    )
    .expect("must have a real width_axis to scale from");
    extract(&proportions, metrics.width_axis)
        .expect("the requested figure must be measurable on this solid")
        * scale
}

/// `GirdleThicknessMm` used to `CannotBracket` for every value tried --
/// bisecting from mast `0.0` (measurably degenerate for a girdle facet) rather
/// than around the tier's own currently authored mast.
#[test]
fn girdle_thickness_mm_target_resolves_within_one_percent() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(GIRDLE).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::GirdleThicknessMm(0.3));

    let measured = measured_mm(&design, |p, _width_axis| p.girdle_thickness);
    assert!(
        (measured - 0.3).abs() / 0.3 <= 0.01,
        "measured girdle thickness {measured:.4} mm not within 1% of the 0.3 mm target"
    );
}

/// `TableWidthMm` used to `CannotBracket` for every value tried, same
/// root cause as the girdle-thickness case above.
#[test]
fn table_width_mm_target_resolves_within_one_percent() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(TABLE).expect("tier must exist");
    design
        .tier_targets
        .insert(id, TierTarget::TableWidthMm(3.5));

    let measured = measured_mm(&design, |p, width_axis| {
        p.table_percent.map(|pct| pct / 100.0 * width_axis)
    });
    assert!(
        (measured - 3.5).abs() / 3.5 <= 0.01,
        "measured table width {measured:.4} mm not within 1% of the 3.5 mm target"
    );
}

/// `DepthMm` used to resolve to roughly HALF the requested depth -- the
/// bootstrap's single correction pass measured `width_axis` with the target
/// tier still pinned at mast `0.0`, not at its own (larger) resolved mast.
/// Checked as a self-consistency (fixed-point) property: the tier's own
/// resolved mast, converted back to mm via the SAME solid's own measured
/// `width_axis`, must reproduce the original target.
#[test]
fn depth_mm_target_resolves_within_one_percent() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(PAVILION_MAIN).expect("tier must exist");
    design.tier_targets.insert(id, TierTarget::DepthMm(2.8));

    let solved = design
        .solve()
        .expect("design with a resolved target must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("must measure as a solid");
    let scale = crate::yield_metrics::mm_per_unit(
        design
            .girdle_diameter_mm
            .expect("fixture sets a girdle diameter"),
        metrics.width_axis,
    )
    .expect("must have a real width_axis to scale from");
    let measured = solved[PAVILION_MAIN].mast * scale;
    assert!(
        (measured - 2.8).abs() / 2.8 <= 0.01,
        "Pavilion Main's resolved mast measures {measured:.4} mm, not within 1% of the 2.8 mm \
         depth target -- the fixed-point iteration must converge, not stop after one \
         mast-0 bootstrap pass"
    );
}

/// Regression: `resolve_dirty` must re-derive a `TierTarget`'s mast when
/// something the target's own conversion depends on changes (here, the
/// girdle's own mast, which `width_axis` -- and so the depth-to-mast scale --
/// depends on) -- not keep serving the pre-edit resolved mast because a
/// resolved target tier looks exactly like an ordinary `ScaleReference` anchor
/// to `affected_tiers`.
#[test]
fn resolve_dirty_reresolves_a_depth_mm_target_when_the_girdle_changes() {
    let mut design = standard_round_brilliant_6_5mm_girdle();
    let id = design.tier_id_at(PAVILION_MAIN).expect("tier must exist");
    design.tier_targets.insert(id, TierTarget::DepthMm(2.8));

    let baseline = design.solve().expect("must solve");

    let mut edited = design.clone();
    let MeetConstraint::ScaleReference(mast) = edited.tiers[GIRDLE].constraint else {
        panic!("Girdle must be a ScaleReference");
    };
    edited.tiers[GIRDLE].constraint = MeetConstraint::ScaleReference(mast * 1.2);

    let dirty = std::collections::BTreeSet::from([GIRDLE]);
    let incremental = edited
        .resolve_dirty(&baseline, &dirty)
        .expect("must resolve");
    let full = edited.solve().expect("must solve");

    assert_eq!(
        incremental[PAVILION_MAIN].mast.to_bits(),
        full[PAVILION_MAIN].mast.to_bits(),
        "an incremental resolve must re-derive the DepthMm target exactly like a full solve, \
         not keep serving the pre-edit mast"
    );
}

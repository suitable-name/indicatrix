//! Tests for [`super::super::ShapeTarget`] and the shape penalty the optimizer adds to
//! every score when [`super::super::OptimizeOptions::shape_target`] is set.
//!
//! The runs score yield only (all three optical weights zero), like `search_results`:
//! yield is pure geometry, so a run is certain to want to drift, which is what the
//! penalty has to hold back.

use super::{
    super::{
        ObjectiveWeights, OptimizeConfig, OptimizeOptions, OptimizeResult, SearchHooks,
        ShapeTarget, candidate, free_tier_indices_with, optimize_design_with,
    },
    anchored::anchored_options,
    fixtures::imported_rbc,
};
use crate::design::Design;
use indicatrix::{
    geometry::{
        meet_solver::MeetConstraint,
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};

fn yield_only_config(max_evaluations: usize) -> OptimizeConfig {
    OptimizeConfig {
        weights: ObjectiveWeights {
            windowing: 0.0,
            extinction: 0.0,
            tilt_brilliance: 0.0,
            yield_weight: 1.0,
            ..ObjectiveWeights::default()
        },
        seed: 0,
        max_evaluations,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    }
}

/// The table percent and crown-to-pavilion ratio of `design`'s solid.
fn figures_of(design: &Design) -> (Option<f64>, Option<f64>) {
    let solved = design.solve().expect("the design must solve");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("the design must measure");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("the design must close into a solid");
    };
    candidate::shape_figures(&metrics, &mesh, &planes)
}

/// `design` with the best candidate of `result` applied (angles and masts).
fn with_best_applied(design: &Design, result: &OptimizeResult) -> Design {
    let mut applied = design.clone();
    if let Some(best) = result.best_candidate() {
        for change in &best.changes {
            applied.tiers[change.index].angle_deg = change.to_deg;
        }
        for change in &best.mast_changes {
            applied.tiers[change.index].constraint = MeetConstraint::ScaleReference(change.to_mast);
        }
    }
    applied
}

fn run(design: &Design, options: &OptimizeOptions, budget: usize) -> OptimizeResult {
    optimize_design_with(
        design,
        &GemMaterial::diamond(),
        &yield_only_config(budget),
        options,
        &SearchHooks::default(),
    )
    .expect("the imported design must solve")
}

/// The relative drift of `design`'s two figures from `start`'s, as the penalty reads it.
fn drift(start: (Option<f64>, Option<f64>), design: &Design) -> f64 {
    let target = ShapeTarget {
        table_percent: start.0,
        crown_to_pavilion: start.1,
        weight: 1.0,
    };
    let (table, ratio) = figures_of(design);
    target.distance(table, ratio)
}

#[test]
fn a_shape_target_of_none_changes_nothing() {
    let design = imported_rbc();
    let plain = anchored_options(&design);
    assert!(plain.shape_target.is_none());
    let (table, ratio) = figures_of(&design);
    let zero_weight = OptimizeOptions {
        shape_target: Some(ShapeTarget {
            table_percent: table,
            crown_to_pavilion: ratio,
            weight: 0.0,
        }),
        ..plain.clone()
    };
    let without = run(&design, &plain, 12);
    let with_zero = run(&design, &zero_weight, 12);
    assert_eq!(without.outcome, with_zero.outcome);
    assert_eq!(without.mast_changes, with_zero.mast_changes);
    assert_eq!(without.candidates.len(), with_zero.candidates.len());
    for (a, b) in without.candidates.iter().zip(&with_zero.candidates) {
        assert_eq!(a.score.to_bits(), b.score.to_bits());
        assert_eq!(a.changes, b.changes);
    }
}

#[test]
fn the_distance_is_zero_on_the_original_and_grows_with_drift() {
    let target = ShapeTarget {
        table_percent: Some(55.0),
        crown_to_pavilion: Some(0.4),
        weight: 2.0,
    };
    assert_eq!(target.distance(Some(55.0), Some(0.4)), 0.0);
    assert_eq!(target.penalty(Some(55.0), Some(0.4)), 0.0);
    // One figure unknown on the candidate's side: only the other counts.
    assert_eq!(target.distance(None, Some(0.4)), 0.0);
    assert!((target.distance(Some(60.5), None) - 0.1).abs() < 1e-12);
    // 10 % off on one figure, on target on the other.
    assert!((target.distance(Some(60.5), Some(0.4)) - 0.1).abs() < 1e-12);
    // Both 10 % off: the root of the sum of squares.
    let both = target.distance(Some(60.5), Some(0.44));
    assert!((both - 0.02_f64.sqrt()).abs() < 1e-12, "{both}");
    // weight * 100 * d.
    let penalty = target.penalty(Some(60.5), Some(0.4));
    assert!((penalty - 20.0).abs() < 1e-3, "{penalty}");
    // A target figure that is None or not positive contributes nothing.
    let blind = ShapeTarget {
        table_percent: None,
        crown_to_pavilion: Some(0.0),
        weight: 2.0,
    };
    assert_eq!(blind.distance(Some(10.0), Some(10.0)), 0.0);
    // A weight of zero or less is no penalty at all.
    let off = ShapeTarget {
        weight: 0.0,
        ..target
    };
    assert_eq!(off.penalty(Some(99.0), Some(9.0)), 0.0);
}

#[test]
fn a_heavy_shape_weight_keeps_the_table_and_ratio_near_the_start() {
    let design = imported_rbc();
    let start = figures_of(&design);
    let mut options = anchored_options(&design);
    options.min_girdle_fraction = Some(0.5);
    assert_ne!(free_tier_indices_with(&design, &options).len(), 0);

    let free_run = run(&design, &options, 60);
    options.shape_target = Some(ShapeTarget {
        table_percent: start.0,
        crown_to_pavilion: start.1,
        weight: 5.0,
    });
    let held_run = run(&design, &options, 60);

    let drift_none = drift(start, &with_best_applied(&design, &free_run));
    let drift_shape = drift(start, &with_best_applied(&design, &held_run));
    assert!(
        drift_shape <= 0.03,
        "the table size and the ratio must stay within 3 % of the start, drifted {drift_shape}"
    );
    assert!(
        drift_none >= drift_shape,
        "the run without the penalty ({drift_none}) must drift at least as far as the held one ({drift_shape})"
    );
}

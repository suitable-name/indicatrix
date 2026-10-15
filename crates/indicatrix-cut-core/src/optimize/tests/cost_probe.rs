//! Cost probe: ignored by default, run explicitly --
//! `cargo test -p indicatrix-cut-core --release --ignored --nocapture cost_probe`
//!
//! Measures one full objective evaluation (`Design::solve` + `evaluate_objective`)
//! end to end, at both fidelities, on a small real meet-derived design (RBC-445, 12
//! tiers) and the large one `design.rs`'s own benchmark already measured a 5.9s full
//! solve against (CrackOtto-Step, 103 tiers) -- see the parent module doc comment's
//! "Cost first" section for the numbers from the run this was written against.

use super::{
    super::{ObjectiveFidelity, evaluate_objective, objective},
    fixtures::{design_with_real_meet_structure, rbc_445},
};
use crate::{
    design::Design,
    manufacturability::{
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_manufacturability_with_geometry,
    },
};
use indicatrix::{
    geometry::{
        meet_solver::SolvedTier,
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use std::time::Instant;

const CRACKOTTO_STEP: &str = include_str!("../../optimize_cost_probe_crackotto_step.asc");

fn report(name: &str, design: &Design) {
    let material = GemMaterial::diamond();

    let start = Instant::now();
    let solved = design.solve().expect("must solve");
    let solve_time = start.elapsed();

    let planes = design.planes_from_solved(&solved);
    let gpu_planes = objective::to_gpu_planes(&planes);

    let start = Instant::now();
    let _ = evaluate_objective(&gpu_planes, &material, ObjectiveFidelity::Fast);
    let fast_metrics_time = start.elapsed();

    let start = Instant::now();
    let _ = evaluate_objective(&gpu_planes, &material, ObjectiveFidelity::Full);
    let full_metrics_time = start.elapsed();

    println!(
        "{name} ({} tiers): solve={solve_time:?} fast_metrics={fast_metrics_time:?} \
         full_metrics={full_metrics_time:?} | one Fast eval (solve+fast)={:?} | \
         one Full eval (solve+full, e.g. the before/after report)={:?}",
        design.tiers.len(),
        solve_time + fast_metrics_time,
        solve_time + full_metrics_time,
    );
    report_geometry(name, design, &solved);
}

/// One line per call `evaluate_candidate` makes around the metrics, each timed alone:
/// the plane list, the mesh, the manufacturability check given that geometry, the solid
/// measurement and the (per-run) preform measure. Before the candidate shared its
/// geometry the first four ran 2-3 times per evaluation; the sum of one of each is what
/// the evaluation pays now.
fn report_geometry(name: &str, design: &Design, solved: &[SolvedTier]) {
    let start = Instant::now();
    let planes = design.planes_from_solved(solved);
    println!("{name}: planes_from_solved={:?}", start.elapsed());

    let start = Instant::now();
    let status = build_solid_mesh(&planes);
    println!("{name}: build_solid_mesh={:?}", start.elapsed());

    let start = Instant::now();
    let metrics = measure_solid(&planes);
    println!("{name}: measure_solid={:?}", start.elapsed());

    if let SolidStatus::Closed(mesh) = &status {
        let start = Instant::now();
        let _ = check_manufacturability_with_geometry(
            design,
            solved,
            Some(mesh),
            metrics.as_ref(),
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        );
        println!(
            "{name}: check_manufacturability_with_geometry={:?}",
            start.elapsed()
        );
    }

    let start = Instant::now();
    let _ = measure_solid(&design.preform.planes());
    println!("{name}: preform measure_solid={:?}", start.elapsed());
}

#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture, see module doc comment"]
fn cost_probe_small_real_meet_derived_design() {
    report("RBC-445", &rbc_445());
}

#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture, see module doc comment"]
fn cost_probe_large_real_meet_derived_design() {
    let design = design_with_real_meet_structure(CRACKOTTO_STEP);
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );
    report("CrackOtto-Step", &design);
}

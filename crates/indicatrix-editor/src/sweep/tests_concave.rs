//! The angle sweep scores the stone WITH its concave tools (the Compare window's stone):
//! its table-up figures and its tilt averages are those of the carved stone. A flat
//! design is scored as before (`tests::the_current_row_matches_a_direct_score_of_the_design`).

use super::*;
use indicatrix::optics::{materials::GemMaterial, raytracer::LightingPreset};
use std::f32::consts::FRAC_PI_2;

/// The concave fixture's pavilion tier (-42 degrees, pinned): flat tier 1.
const FIXTURE_PAVILION: usize = 1;

fn environment() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

/// A one-angle sweep (the tier's own angle) of the concave fixture.
fn current_row(tilt_average: bool) -> SweepRow {
    let design = Design::concave_fixture();
    let plan = plan_sweep(
        &design,
        FIXTURE_PAVILION,
        SweepRange {
            from_deg: 42.0,
            to_deg: 42.0,
            step_deg: 1.0,
        },
    )
    .expect("a pavilion sweep");
    let material = GemMaterial::diamond();
    let outcome = sweep_tier_angle(
        &design,
        &plan,
        &SweepScene {
            material: &material,
            environment: environment(),
        },
        &SweepOptions {
            tilt_average,
            workers: 1,
        },
        &AtomicBool::new(false),
        &|_, _| {},
    );
    assert_eq!(outcome.rows.len(), 1);
    outcome.rows[0].clone()
}

#[test]
fn the_figures_of_a_concave_design_are_those_of_the_stone_with_its_tools() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let gpu = crate::solve_policy::design_to_gpu_planes_from_solved(&design, &solved);
    let tools = design.tools_for_scoring(&planes);
    assert_eq!(tools.len(), design.concave_placement_count());

    let material = GemMaterial::diamond();
    let carved = evaluate_gem_optical_metrics_geom(
        StoneGeometry {
            planes: &gpu,
            tools: &tools,
        },
        &material,
        0.0,
        FRAC_PI_2,
        environment(),
    );
    let shell = evaluate_gem_optical_metrics_geom(
        StoneGeometry::planes_only(&gpu),
        &material,
        0.0,
        FRAC_PI_2,
        environment(),
    );

    let row = current_row(false);
    assert!(row.is_current && row.is_valid(), "{:?}", row.notes);
    let swept = row.metrics.expect("scored");
    assert_eq!(
        swept.brilliance_pct.to_bits(),
        carved.brilliance_pct.to_bits()
    );
    assert_eq!(
        swept.windowing_pct.to_bits(),
        carved.windowing_pct.to_bits()
    );
    assert_eq!(
        swept.extinction_pct.to_bits(),
        carved.extinction_pct.to_bits()
    );
    assert_eq!(swept.fire_index.to_bits(), carved.fire_index.to_bits());
    assert_eq!(
        swept.scintillation_pct.to_bits(),
        carved.scintillation_pct.to_bits()
    );
    assert!(
        swept.brilliance_pct.to_bits() != shell.brilliance_pct.to_bits()
            || swept.windowing_pct.to_bits() != shell.windowing_pct.to_bits()
            || swept.extinction_pct.to_bits() != shell.extinction_pct.to_bits(),
        "the tools change the figures: carved {carved:?}, flat shell {shell:?}"
    );
}

#[test]
fn the_yield_of_a_concave_design_reads_the_carved_volume() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let tools = design.tools_for_scoring(&planes);
    let flat = measure_solid(&planes).expect("the flat stone measures");
    let carved = Design::carved_metrics(Some(flat), &planes, &tools)
        .expect("the tools leave a closed stone");
    assert!(
        carved.volume < flat.volume,
        "the tools remove material: carved {} flat {}",
        carved.volume,
        flat.volume
    );

    let preform = measure_solid(&design.preform.planes())
        .expect("the preform measures")
        .volume;
    let expected = volumetric_yield(carved.volume, preform).map(|fraction| fraction * 100.0);
    let flat_yield = volumetric_yield(flat.volume, preform).map(|fraction| fraction * 100.0);
    let row = current_row(false);
    let swept = row.metrics.expect("scored").yield_pct;
    assert_eq!(swept.map(f64::to_bits), expected.map(f64::to_bits));
    assert_ne!(swept.map(f64::to_bits), flat_yield.map(f64::to_bits));
}

#[test]
fn the_tilt_averages_of_a_concave_design_are_those_of_the_stone_with_its_tools() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let gpu = crate::solve_policy::design_to_gpu_planes_from_solved(&design, &solved);
    let tools = design.tools_for_scoring(&planes);
    let material = GemMaterial::diamond();
    let direct = measure_tilt_average_geom(
        StoneGeometry {
            planes: &gpu,
            tools: &tools,
        },
        &material,
        environment(),
        &mut |_| true,
    )
    .expect("the sweep is not stopped");

    let swept = current_row(true)
        .metrics
        .expect("scored")
        .tilt
        .expect("the tilt averages were asked for");
    assert_eq!(swept, direct);
}

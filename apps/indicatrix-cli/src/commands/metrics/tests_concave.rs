//! `metrics` measures the stone WITH its concave tools (the Compare window's stone): the optics
//! see the grooves and dimples and the volume is the carved volume. A design without concave
//! tiers is measured on exactly its planes, bit for bit as before.

use super::*;
use crate::{args::MaterialArg, outcome::EXIT_OK, testing};
use indicatrix::color::metrics::evaluate_gem_optical_metrics;
use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, evaluate_objective_under};

/// Measures `design` under refractive index 1.76 at the canonical light.
fn measured_of(design: &Design, tilt: bool) -> Measured {
    let scoring = resolve_scoring(design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
        .expect("scoring");
    let analysis = analyze(design);
    let solved = analysis.require_stone(design).expect("a usable stone");
    measure(
        design,
        &analysis,
        solved,
        &scoring,
        CANONICAL_LIGHTING_PRESET,
        tilt,
    )
    .expect("measures")
}

/// The five table-up figures as exact bits.
fn table_bits(metrics: &GemOpticalMetrics) -> [u32; 5] {
    [
        metrics.brilliance_pct.to_bits(),
        metrics.fire_index.to_bits(),
        metrics.scintillation_pct.to_bits(),
        metrics.windowing_pct.to_bits(),
        metrics.extinction_pct.to_bits(),
    ]
}

/// A design with no concave tiers is measured on exactly its planes: the figures are what the
/// planes-only entry points give, bit for bit.
#[test]
fn a_flat_design_is_measured_on_exactly_its_planes() {
    let design = testing::template();
    let measured = measured_of(&design, true);

    let analysis = analyze(&design);
    let solved = analysis.require_stone(&design).expect("a usable stone");
    let planes = design_to_gpu_planes_from_solved(&design, solved);
    let scoring = resolve_scoring(&design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
        .expect("scoring");
    let gem = scoring.sized_gem(&design, &planes);
    let environment =
        CANONICAL_LIGHTING_PRESET.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let direct = evaluate_gem_optical_metrics(&planes, &gem, 0.0, TABLE_UP_PITCH, environment);
    assert_eq!(table_bits(&measured.table_up), table_bits(&direct));
    let tilt = evaluate_objective_under(
        &planes,
        &gem,
        ObjectiveFidelity::Full,
        CANONICAL_LIGHTING_PRESET,
    );
    assert_eq!(measured.tilt, Some(tilt));

    let flat = measure_solid(&design.planes_from_solved(solved)).expect("measures");
    let width = flat.width_axis;
    assert_eq!(
        measured.vol_w3.map(f64::to_bits),
        Some((flat.volume / (width * width * width)).to_bits())
    );
    let rough = measure_solid(&design.preform.planes()).expect("the rough measures");
    assert_eq!(
        measured.yield_pct.map(f64::to_bits),
        volumetric_yield(flat.volume, rough.volume).map(|y| (y * 100.0).to_bits())
    );
}

/// A design with concave tiers is measured as the stone with its tools: the optics are the
/// tracer's figures for the carved stone, the volume is the carved volume, and both differ from
/// the flat shell of the same design.
#[test]
fn a_concave_design_is_measured_with_its_tools() {
    let design = testing::concave();
    let measured = measured_of(&design, false);
    let shell = measured_of(&testing::concave_shell(), false);

    let analysis = analyze(&design);
    let solved = analysis.require_stone(&design).expect("a usable stone");
    let planes64 = design.planes_from_solved(solved);
    let tools = design.tools_for_scoring(&planes64);
    assert_eq!(tools.len(), design.concave_placement_count());
    assert!(!tools.is_empty(), "the fixture carves");
    let planes = design_to_gpu_planes_from_solved(&design, solved);
    let scoring = resolve_scoring(&design, &Catalogue::default(), Some(&MaterialArg::Ri(1.76)))
        .expect("scoring");
    let gem = scoring.sized_gem(&design, &planes);
    let environment =
        CANONICAL_LIGHTING_PRESET.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let carved = evaluate_gem_optical_metrics_geom(
        StoneGeometry {
            planes: &planes,
            tools: &tools,
        },
        &gem,
        0.0,
        TABLE_UP_PITCH,
        environment,
    );
    assert_eq!(table_bits(&measured.table_up), table_bits(&carved));
    assert_ne!(
        table_bits(&measured.table_up),
        table_bits(&shell.table_up),
        "the grooves and dimples change the optics"
    );

    let (Some(carved_vol), Some(flat_vol)) = (measured.vol_w3, shell.vol_w3) else {
        panic!("both stones have a width");
    };
    assert!(carved_vol < flat_vol, "{carved_vol} against {flat_vol}");
    let (Some(carved_yield), Some(flat_yield)) = (measured.yield_pct, shell.yield_pct) else {
        panic!("both stones have a rough");
    };
    assert!(
        carved_yield < flat_yield,
        "{carved_yield} against {flat_yield}"
    );
    // The widths, heights and the table are the flat stone's.
    assert_eq!(
        measured.depth_pct.map(f64::to_bits),
        shell.depth_pct.map(f64::to_bits)
    );
    assert_eq!(measured.facets, shell.facets);
    assert_eq!(measured.tiers, shell.tiers);
}

#[test]
fn end_to_end_metrics_reads_the_concave_tools_of_a_file() {
    let carved = testing::write_design("metrics-e2e-concave.isdd", &testing::concave());
    let shell = testing::write_design("metrics-e2e-shell.isdd", &testing::concave_shell());
    let figures = |input: &str| -> Value {
        let outcome = testing::run(&["metrics", input, "--ri", "1.76", "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        value["figures"].clone()
    };
    let with_tools = figures(&carved);
    let without = figures(&shell);
    assert_ne!(with_tools, without, "the tools are part of the figures");
    let volume = |figures: &Value| {
        figures["volume_over_width_cubed"]
            .as_f64()
            .expect("a number")
    };
    assert!(volume(&with_tools) < volume(&without));
}

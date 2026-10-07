//! Tests of the compare window's optical figures and sentences.

use super::*;
use indicatrix::{geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset};

fn figures(
    brilliance: f32,
    windowing: f32,
    extinction: f32,
    fire: f32,
    scint: f32,
) -> OpticalFigures {
    OpticalFigures {
        brilliance_pct: brilliance,
        windowing_pct: windowing,
        extinction_pct: extinction,
        fire_index: fire,
        scintillation_pct: scint,
    }
}

fn base() -> OpticalFigures {
    figures(60.0, 20.0, 20.0, 20.0, 50.0)
}

fn tilt(brilliance: f32, windowing: f32, extinction: f32) -> TiltAverages {
    TiltAverages {
        brilliance_pct: brilliance,
        windowing_pct: windowing,
        extinction_pct: extinction,
    }
}

#[test]
fn the_same_stone_twice_has_no_clear_difference() {
    assert_eq!(
        describe_metric_deltas(&base(), &base(), None),
        vec!["No clear optical difference."]
    );
}

#[test]
fn a_brighter_stone_with_less_windowing_reads_like_the_example() {
    let after = figures(64.0, 17.0, 20.0, 20.0, 50.0);
    assert_eq!(
        describe_metric_deltas(&base(), &after, None),
        vec![
            "The after design is brighter face-up (+4 %) and shows less windowing (-3 %).",
            "Extinction, fire and scintillation are about the same.",
        ]
    );
}

#[test]
fn a_change_exactly_at_the_threshold_is_noise_and_just_over_it_is_not() {
    let at = figures(62.0, 22.0, 18.0, 20.0, 53.0);
    assert_eq!(
        describe_metric_deltas(&base(), &at, None),
        vec!["No clear optical difference."],
        "2 points of brilliance, windowing and extinction and 3 of scintillation are noise"
    );
    let over = figures(62.01, 20.0, 20.0, 20.0, 50.0);
    let lines = describe_metric_deltas(&base(), &over, None);
    assert!(lines[0].starts_with("The after design is brighter face-up (+2 %)"));
}

#[test]
fn every_figure_has_its_own_direction_and_words() {
    let after = figures(56.0, 25.0, 26.0, 16.0, 40.0);
    assert_eq!(
        describe_metric_deltas(&base(), &after, None),
        vec![
            "The after design is less bright face-up (-4 %), shows more windowing (+5 %) and has more extinction (+6 %).",
            "It also shows less fire (-20 %) and sparkles less (-10 %).",
        ]
    );
}

#[test]
fn the_changes_are_ordered_by_importance_not_by_size() {
    // Windowing moves most, but brilliance still leads.
    let after = figures(63.0, 10.0, 20.0, 20.0, 50.0);
    let lines = describe_metric_deltas(&base(), &after, None);
    assert!(lines[0].contains("brighter face-up (+3 %) and shows less windowing (-10 %)"));
}

#[test]
fn fire_is_judged_by_a_relative_threshold_with_a_floor() {
    // 4.5 % of 20 is under the 5 % threshold; 6 % is over it.
    assert_eq!(
        judge(DeltaMetric::Fire, 20.0, 20.9, false).map(|c| c.tone),
        Some(Tone::Same)
    );
    let over = judge(DeltaMetric::Fire, 20.0, 21.2, false).expect("finite");
    assert_eq!(over.tone, Tone::Better);
    assert!((over.change - 6.0).abs() < 1e-3, "{}", over.change);
    // A stone with next to no fire: a jump of 0.3 is under the floor of 0.5.
    assert_eq!(
        judge(DeltaMetric::Fire, 0.1, 0.4, false).map(|c| c.tone),
        Some(Tone::Same)
    );
    // Less fire is worse.
    assert_eq!(
        judge(DeltaMetric::Fire, 20.0, 15.0, false).map(|c| c.tone),
        Some(Tone::Worse)
    );
}

#[test]
fn better_and_worse_follow_each_figures_direction() {
    let tone = |metric, before, after| judge(metric, before, after, false).map(|c| c.tone);
    assert_eq!(
        tone(DeltaMetric::Brilliance, 50.0, 60.0),
        Some(Tone::Better)
    );
    assert_eq!(tone(DeltaMetric::Brilliance, 60.0, 50.0), Some(Tone::Worse));
    assert_eq!(tone(DeltaMetric::Windowing, 20.0, 10.0), Some(Tone::Better));
    assert_eq!(tone(DeltaMetric::Windowing, 10.0, 20.0), Some(Tone::Worse));
    assert_eq!(
        tone(DeltaMetric::Extinction, 20.0, 10.0),
        Some(Tone::Better)
    );
    assert_eq!(
        tone(DeltaMetric::Scintillation, 40.0, 50.0),
        Some(Tone::Better)
    );
}

#[test]
fn a_figure_that_is_not_a_number_is_left_out_of_the_sentences() {
    let after = figures(f32::NAN, 17.0, 20.0, 20.0, 50.0);
    let lines = describe_metric_deltas(&base(), &after, None);
    assert_eq!(
        lines,
        vec![
            "The after design shows less windowing (-3 %).",
            "Extinction, fire and scintillation are about the same.",
        ]
    );
    let all_nan = figures(f32::NAN, f32::NAN, f32::NAN, f32::NAN, f32::NAN);
    assert_eq!(
        describe_metric_deltas(&base(), &all_nan, None),
        vec!["The optical figures could not be compared."]
    );
}

#[test]
fn fire_and_scintillation_alone_lead_with_the_subject() {
    let after = figures(60.0, 20.0, 20.0, 25.0, 50.0);
    assert_eq!(
        describe_metric_deltas(&base(), &after, None),
        vec![
            "The after design shows more fire (+25 %).",
            "Brilliance, windowing, extinction and scintillation are about the same.",
        ]
    );
}

#[test]
fn another_subject_names_the_thing_that_changed() {
    let after = figures(64.0, 20.0, 20.0, 20.0, 50.0);
    let lines = describe_metric_deltas_for(
        "The current design",
        &base(),
        &after,
        Some((&tilt(50.0, 20.0, 20.0), &tilt(53.0, 20.0, 20.0))),
    );
    assert!(lines[0].starts_with("The current design is brighter face-up (+4 %)"));
    assert_eq!(
        lines.last().map(String::as_str),
        Some("Averaged over all tilts, the current design returns more light (+3 %).")
    );
}

#[test]
fn the_tilt_sentence_names_changes_over_one_point_and_otherwise_says_none() {
    let flat = describe_metric_deltas(
        &base(),
        &base(),
        Some((&tilt(50.0, 20.0, 20.0), &tilt(50.8, 19.2, 20.9))),
    );
    assert_eq!(
        flat.last().map(String::as_str),
        Some("Averaged over all tilts, there is no clear difference."),
        "0.8 and 0.9 points are under the tilt threshold of 1"
    );
    let moved = describe_metric_deltas(
        &base(),
        &base(),
        Some((&tilt(50.0, 20.0, 20.0), &tilt(48.0, 23.0, 20.0))),
    );
    assert_eq!(
        moved.last().map(String::as_str),
        Some(
            "Averaged over all tilts, the after design returns less light (-2 %) and shows more windowing (+3 %)."
        )
    );
    assert_eq!(moved.len(), 2, "one table-up sentence plus the tilt one");
}

#[test]
fn the_rows_show_the_figures_the_change_and_the_verdict() {
    let after = figures(64.0, 17.0, 20.2, 21.2, 50.0);
    let rows = metric_rows(&base(), &after, None);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].label, "Brilliance");
    assert_eq!(rows[0].before, "60.0 %");
    assert_eq!(rows[0].after, "64.0 %");
    assert_eq!(rows[0].change, "+4.0 %");
    assert_eq!(rows[0].tone, Tone::Better);
    assert_eq!(rows[1].change, "-3.0 %");
    assert_eq!(rows[1].tone, Tone::Better, "less windowing is better");
    assert_eq!(rows[2].change, "+0.2 %");
    assert_eq!(rows[2].tone, Tone::Same);
    assert_eq!(rows[3].before, "20.0", "fire has no percent sign");
    assert_eq!(rows[3].change, "+6 %");
    assert_eq!(rows[4].change, "0.0 %", "no sign on a zero change");
}

#[test]
fn a_change_that_rounds_to_zero_never_reads_minus_zero() {
    assert_eq!(signed(-0.04, 1), "0.0");
    assert_eq!(signed(0.04, 1), "0.0");
    assert_eq!(signed(-0.4, 0), "0");
    assert_eq!(signed(-3.04, 1), "-3.0");
    assert_eq!(signed(2.96, 1), "+3.0");
}

#[test]
fn tilt_rows_follow_the_table_up_rows_with_their_own_labels() {
    let rows = metric_rows(
        &base(),
        &base(),
        Some((&tilt(50.0, 20.0, 20.0), &tilt(52.0, 19.0, 20.0))),
    );
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[5].label, "Tilt brilliance");
    assert_eq!(rows[5].change, "+2.0 %");
    assert_eq!(rows[5].tone, Tone::Better);
    assert_eq!(rows[6].label, "Tilt windowing");
    assert_eq!(
        rows[6].tone,
        Tone::Same,
        "one point of windowing is exactly the threshold"
    );
    assert_eq!(rows[7].label, "Tilt extinction");
}

#[test]
fn a_figure_that_is_not_a_number_shows_as_not_available() {
    let after = figures(f32::NAN, 20.0, 20.0, 20.0, 50.0);
    let rows = metric_rows(&base(), &after, None);
    assert_eq!(rows[0].after, "n/a");
    assert_eq!(rows[0].change, "n/a");
    assert_eq!(rows[0].tone, Tone::Same);
}

#[test]
fn the_tilt_average_is_the_mean_of_every_point_of_every_axis() {
    let axis = |b: f32, w: f32, e: f32| AxisProfile {
        brilliance: [b; 181],
        extinction: [e; 181],
        windowing: [w; 181],
    };
    let averages = average_axis_profiles(&[
        axis(40.0, 10.0, 30.0),
        axis(60.0, 20.0, 10.0),
        axis(50.0, 30.0, 20.0),
    ]);
    assert!((averages.brilliance_pct - 50.0).abs() < 1e-4);
    assert!((averages.windowing_pct - 20.0).abs() < 1e-4);
    assert!((averages.extinction_pct - 20.0).abs() < 1e-4);
    let none = average_axis_profiles(&[]);
    assert!(none.brilliance_pct.abs() < 1e-6, "no axes is zero, not NaN");
}

#[test]
fn the_table_up_measurement_of_a_round_brilliant_is_in_range_and_repeatable() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let first = measure_table_up(&planes, &material, environment);
    let second = measure_table_up(&planes, &material, environment);
    assert_eq!(first, second);
    for percent in [
        first.brilliance_pct,
        first.windowing_pct,
        first.extinction_pct,
        first.scintillation_pct,
    ] {
        assert!((0.0..=100.0).contains(&percent), "{percent}");
    }
    assert!(first.fire_index > 0.0);
}

#[test]
fn the_geometry_measurements_agree_bit_for_bit_with_the_planar_ones_without_tools() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    assert_eq!(
        measure_table_up(&planes, &material, environment),
        measure_table_up_geom(StoneGeometry::planes_only(&planes), &material, environment)
    );
    let mut plain_steps = 0_usize;
    let plain = measure_tilt_average(&planes, &material, environment, &mut |_| {
        plain_steps += 1;
        plain_steps < 40
    });
    let mut geom_steps = 0_usize;
    let geom = measure_tilt_average_geom(
        StoneGeometry::planes_only(&planes),
        &material,
        environment,
        &mut |_| {
            geom_steps += 1;
            geom_steps < 40
        },
    );
    assert_eq!(
        plain, geom,
        "both stop at the same step, with the same answer"
    );
    assert_eq!(plain_steps, geom_steps);
}

#[test]
fn a_tilt_average_that_is_stopped_is_none() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let mut calls = 0_usize;
    let stopped = measure_tilt_average(&planes, &material, environment, &mut |_| {
        calls += 1;
        calls < 5
    });
    assert!(stopped.is_none());
    assert_eq!(calls, 5, "nothing is asked for after the refusal");
}

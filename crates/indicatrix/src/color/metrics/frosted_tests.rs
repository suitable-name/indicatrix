//! Facet finishes in the tilt sweeps: no frosting is the polished sweep, bit for bit, and a
//! frosted facet moves the curves.

use super::{
    FinishedStone,
    evaluate::{MetricsWanted, evaluate_pose},
    evaluate_all_axes_profiles_finished_threaded, evaluate_all_axes_profiles_geom_threaded,
    evaluate_full_axis_profile_at_azimuth_finished_threaded,
    evaluate_full_axis_profile_at_azimuth_geom_threaded,
    prepared::PreparedEval,
};
use crate::{
    geometry::{cuts::StandardGemCuts, girdle_facet_finishes, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish, LightingPreset},
    },
};

type Curves = ([f32; 181], [f32; 181], [f32; 181]);

/// The light pose the finish tests run under.
const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

fn bits(curves: &Curves) -> Vec<u32> {
    curves
        .0
        .iter()
        .chain(&curves.1)
        .chain(&curves.2)
        .map(|v| v.to_bits())
        .collect()
}

fn axis(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    azimuth_deg: f32,
    threads: usize,
) -> Curves {
    evaluate_full_axis_profile_at_azimuth_finished_threaded(
        stone,
        material,
        azimuth_deg,
        studio(),
        Some(threads),
    )
}

/// An empty finish list and a list of only polished facets are the polished sweep: the same
/// bits as the plain-geometry entry point, and the frosted code is never reached (the prepared
/// stone drops the list).
#[test]
fn an_empty_or_all_polished_finish_list_is_the_polished_sweep() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let plain = evaluate_full_axis_profile_at_azimuth_geom_threaded(
        geom,
        &material,
        45.0,
        studio(),
        Some(1),
    );
    let empty: &[FacetFinish] = &[];
    let all_polished = vec![FacetFinish::Polished; planes.len()];
    for finishes in [empty, all_polished.as_slice()] {
        let finished = axis(FinishedStone::new(geom, finishes), &material, 45.0, 1);
        assert_eq!(bits(&finished), bits(&plain));
    }
    assert_eq!(
        bits(&axis(FinishedStone::polished(geom), &material, 45.0, 1)),
        bits(&plain)
    );
    let prepared = PreparedEval::new_finished(geom, &all_polished, &material, studio())
        .expect("a stone with planes prepares");
    assert_eq!(prepared.stone().finishes.len(), 0);
}

/// A frosted girdle changes what the sweep sees: the curves differ from the polished stone's
/// somewhere, stay percentages, and are the same bits on a second run and for any thread
/// count (the frosted draws are a pure function of each ray).
#[test]
fn a_frosted_girdles_tilt_profile_differs_from_the_polished_one() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let finishes = girdle_facet_finishes(&planes);
    assert!(finishes.contains(&FacetFinish::Frosted));

    let polished = axis(FinishedStone::polished(geom), &material, 0.0, 1);
    let frosted = axis(FinishedStone::new(geom, &finishes), &material, 0.0, 1);
    assert_ne!(bits(&frosted), bits(&polished));
    for value in frosted.0.iter().chain(&frosted.1).chain(&frosted.2) {
        assert!(
            value.is_finite() && (0.0..=100.0).contains(value),
            "{value}"
        );
    }

    let again = axis(FinishedStone::new(geom, &finishes), &material, 0.0, 1);
    let threaded = axis(FinishedStone::new(geom, &finishes), &material, 0.0, 3);
    assert_eq!(bits(&frosted), bits(&again));
    assert_eq!(bits(&frosted), bits(&threaded));
}

/// Frosting the whole stone moves the curves much further than the girdle alone, and the four
/// axis sweep of a frosted stone is the per-axis sweep at each azimuth.
#[test]
fn frosting_every_facet_moves_the_curves_and_the_all_axes_sweep_agrees() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let everything = vec![FacetFinish::Frosted; planes.len()];
    let stone = FinishedStone::new(geom, &everything);

    let polished = evaluate_all_axes_profiles_geom_threaded(geom, &material, studio(), Some(2));
    let frosted = evaluate_all_axes_profiles_finished_threaded(stone, &material, studio(), Some(2));
    assert_eq!(polished.len(), frosted.len());
    assert!(
        polished.iter().zip(&frosted).all(|(p, f)| p != f),
        "every axis of a fully frosted stone differs from the polished one"
    );
    let first = &frosted[0];
    let direct = axis(stone, &material, 0.0, 2);
    assert_eq!(
        bits(&direct),
        bits(&(first.brilliance, first.extinction, first.windowing))
    );
}

/// The full metrics pass (Fire and Scintillation as well) runs on a frosted stone and
/// differs from the polished pass: the frosted facets reach the Fire companion traces and the
/// scintillation sub-poses too, not only the three percentages the tilt sweep reads.
#[test]
fn the_full_metrics_pass_sees_frosted_facets() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let finishes = vec![FacetFinish::Frosted; planes.len()];
    let polished = PreparedEval::new(geom, &material, studio()).expect("planes");
    let frosted = PreparedEval::new_finished(geom, &finishes, &material, studio()).expect("planes");
    let metrics = |prepared: &PreparedEval<'_>| {
        let m = evaluate_pose(prepared, 0.6, 0.45, MetricsWanted::ALL, None);
        [
            m.brilliance_pct,
            m.fire_index,
            m.scintillation_pct,
            m.windowing_pct,
            m.extinction_pct,
        ]
    };
    let (p, f) = (metrics(&polished), metrics(&frosted));
    assert!(f.iter().all(|v| v.is_finite()), "{f:?}");
    assert_ne!(p.map(f32::to_bits), f.map(f32::to_bits));
}

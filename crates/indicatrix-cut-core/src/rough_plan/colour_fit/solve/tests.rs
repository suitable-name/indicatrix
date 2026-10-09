//! Tests of the inverse solver (written, not run, by lane S1).
//!
//! Synthetic photos are made from
//! the forward tracer's own records (so the physics agrees with the fit by construction) with a
//! seeded Gaussian noise; the checks are on what the plan asks for: recovered amplitudes and
//! amounts, the bicolour, the cross-validation, the calibration of the covariance, determinism
//! across thread counts, cancellation and the zoning suggestion.

use std::sync::atomic::AtomicBool;

use glam::DVec3;
use indicatrix::{
    color::body_color::{Illuminant, delta_e_2000},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
};

use super::{
    ChromophoreModel, FitConfig, FitError, ModelKind, ObservedView, SmoothBasisModel,
    diagnostics::moran_i,
    fit_records,
    linalg::{inverse_spd, solve_spd},
    predict::{face_up_path_mm, zone_lab},
    select::chi_square_quantile,
};
use crate::rough_plan::{
    camera_spectral::{BacklightSpectrum, CameraResponse},
    colour_fit::{
        ColourRig,
        forward::{
            ForwardInput, ForwardOptions, ForwardRecords, PanelGeom, RigLighting, SpectralModel,
            StoneIndex, SurfaceMap, ViewTraceInput, evaluate, trace_rig,
        },
    },
    locate::{Projection, RigProfile, Rigid, ViewPose, box_mesh},
    photometry::WorkingGrid,
    shape::RoughMesh,
};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// Camera positions around a 10 mm cube at the origin (up is +Z; none looks along it).
const POSITIONS: [(f64, f64, f64); 4] = [
    (0.0, -100.0, 0.0),
    (-70.0, -70.0, 30.0),
    (70.0, -70.0, -30.0),
    (0.0, -80.0, 60.0),
];

fn ortho_pose(position: DVec3) -> ViewPose {
    ViewPose::look_at(
        "test",
        position,
        DVec3::ZERO,
        DVec3::Z,
        Projection::Orthographic { px_per_mm: 10.0 },
        [200, 200],
    )
}

/// A 10 x 10 working grid over the central 8 mm square of the image.
fn window_grid() -> WorkingGrid {
    WorkingGrid::fit([60, 60, 140, 140], 10).expect("a valid region")
}

struct Scene {
    mesh: RoughMesh,
    rig: ColourRig,
    surfaces: SurfaceMap,
    index: StoneIndex,
    zones: Option<ZonedAbsorption>,
    camera: CameraResponse,
    backlight: BacklightSpectrum,
    options: ForwardOptions,
    views: Vec<ViewTraceInput<'static>>,
}

impl Scene {
    fn new(view_count: usize, zones: Option<ZonedAbsorption>, threads: usize) -> Self {
        let poses: Vec<ViewPose> = POSITIONS[..view_count]
            .iter()
            .map(|&(x, y, z)| ortho_pose(DVec3::new(x, y, z)))
            .collect();
        let panels: Vec<PanelGeom> = poses
            .iter()
            .map(|pose| PanelGeom::facing_camera(pose, 200.0, [400.0, 400.0]))
            .collect();
        let lighting = RigLighting::backlight(panels, (0..view_count).map(|_| None).collect());
        let mut profile = RigProfile::new("test rig", poses, 1.6);
        profile.surround_n = 1.0;
        Self {
            mesh: box_mesh(DVec3::splat(5.0)).expect("a cube"),
            rig: ColourRig::new(profile, lighting),
            surfaces: SurfaceMap::polished(),
            index: StoneIndex::Rig,
            zones,
            camera: CameraResponse::from_srgb().expect("the sRGB fallback"),
            backlight: BacklightSpectrum::from_cct_k(5000.0).expect("an LED spectrum"),
            options: ForwardOptions {
                samples: 64,
                max_sample_factor: 1,
                threads,
                ..ForwardOptions::default()
            },
            views: (0..view_count)
                .map(|v| ViewTraceInput::new(v, window_grid()))
                .collect(),
        }
    }

    fn input(&self) -> ForwardInput<'_> {
        ForwardInput {
            mesh: &self.mesh,
            alignment: Rigid::IDENTITY,
            rig: &self.rig,
            surfaces: &self.surfaces,
            index: &self.index,
            zones: self.zones.as_ref(),
            inclusions: &[],
            camera: &self.camera,
            backlight: &self.backlight,
            views: &self.views,
            options: &self.options,
            cache_dir: None,
        }
    }

    fn trace(&self) -> ForwardRecords {
        trace_rig(&self.input(), &AtomicBool::new(false), &mut |_| {}).expect("the trace runs")
    }
}

/// A zone absorption whose bands do not matter (the geometry is what the tracer reads).
fn placeholder(peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        550.0, 40.0, peak,
    )]))
}

/// The base zone plus the half space `x >= 0`.
fn bicolour_geometry() -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(placeholder(0.1));
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: placeholder(0.3),
    });
    zoned
}

/// `SplitMix64` with Box-Muller normals: the seeded noise of the synthetic photos.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1_u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Photos of `records` for the absorption `alpha(zone, lambda)`, the per-view `gains` and the
/// noise standard deviation `sigma` (also the stated variance).
fn synthesize(
    records: &ForwardRecords,
    alpha: &dyn Fn(usize, f64) -> f64,
    gains: &[f64],
    sigma: f64,
    seed: u64,
) -> Vec<ObservedView> {
    evaluate(records, alpha)
        .iter()
        .enumerate()
        .map(|(slot, prediction)| {
            let mut noise = Noise(seed.wrapping_mul(0x1000_0001).wrapping_add(slot as u64 + 1));
            let n = prediction.rgb.len();
            let mut values = vec![[0.0_f32; 3]; n];
            for p in 0..n {
                if !prediction.valid[p] {
                    continue;
                }
                for c in 0..3 {
                    let clean = gains[slot] * f64::from(prediction.rgb[p][c]);
                    values[p][c] = (clean + sigma * noise.gauss()) as f32;
                }
            }
            ObservedView {
                view: prediction.view,
                width: prediction.grid.width,
                height: prediction.grid.height,
                values,
                variance: vec![[(sigma * sigma) as f32; 3]; n],
            }
        })
        .collect()
}

fn basis_alpha<'a>(
    model: &'a SmoothBasisModel,
    params: &'a [f64],
) -> impl Fn(usize, f64) -> f64 + 'a {
    move |zone, lambda| model.alpha(zone, lambda, params)
}

/// A configuration that keeps the tests quick.
fn quick_config() -> FitConfig {
    FitConfig {
        seeds: 4,
        stage1_iterations: 10,
        finalists: 1,
        smoothness_sigma: 0.25,
        threads: 2,
        ..FitConfig::default()
    }
}

fn run(records: &ForwardRecords, photos: &[ObservedView], config: &FitConfig) -> super::ColourFit {
    fit_records(
        records,
        photos,
        config,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .expect("the fit runs")
}

/// Log-linear amplitudes about `a0` per mm (a null direction of the smoothness prior).
fn log_linear(a0: f64, slope: f64) -> Vec<f64> {
    (0..7)
        .map(|k| a0.ln() + slope * (f64::from(k) - 3.0))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------------------------

fn recovers_basis_amplitudes(a0: f64, tolerance: f64) {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(a0, 0.15);
    let gains = [1.0, 0.95, 1.05, 1.0];
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &gains, 0.002, 1);
    let fit = run(&records, &photos, &quick_config());
    let chosen = fit.chosen_fit();
    assert_eq!(chosen.kind, ModelKind::SmoothBasis);
    for k in 0..7 {
        let ratio = (chosen.params[k] - truth[k]).exp();
        assert!(
            (ratio - 1.0).abs() <= tolerance,
            "band {k}: fitted / true amplitude = {ratio}"
        );
    }
    let lovo = fit
        .lovo
        .as_ref()
        .expect("four views allow a cross-validation");
    assert!(
        lovo.median_delta_e <= 2.0,
        "leave-one-view-out median {} (max {})",
        lovo.median_delta_e,
        lovo.max_delta_e
    );
    assert_eq!(lovo.entries.len(), 4);
}

#[test]
fn recovers_the_amplitudes_of_a_saturated_stone_within_5_percent() {
    recovers_basis_amplitudes(0.12, 0.05);
}

#[test]
fn recovers_the_amplitudes_of_a_pale_stone_within_10_percent() {
    recovers_basis_amplitudes(0.012, 0.10);
}

#[test]
fn recovers_a_tourmaline_manganese_iron_recipe_by_colour() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let truth_model =
        ChromophoreModel::new("tourmaline", &[], 1).expect("tourmaline is in the catalogue");
    let ids = truth_model.elements().to_vec();
    let index_of = |id: &str| {
        ids.iter()
            .position(|e| e == id)
            .expect("a selectable element")
    };
    // Absent elements sit at the floor; Mn and Fe (weight percent of the oxide) give peaks of a
    // few tenths per mm.
    let mut truth: Vec<f64> = (0..ids.len())
        .map(|i| truth_model.log_amount(i, 0.0))
        .collect();
    truth[index_of("Mn")] = truth_model.log_amount(index_of("Mn"), 0.05);
    truth[index_of("Fe")] = truth_model.log_amount(index_of("Fe"), 1.5);
    let alpha = |zone: usize, lambda: f64| truth_model.alpha(zone, lambda, &truth);
    let photos = synthesize(&records, &alpha, &[1.0, 1.0, 1.0, 1.0], 0.002, 2);

    let config = FitConfig {
        host_id: Some("tourmaline".to_owned()),
        preferred_model: Some(ModelKind::Chromophore),
        seeds: 8,
        finalists: 2,
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    assert_eq!(fit.chosen_fit().kind, ModelKind::Chromophore);
    let path = face_up_path_mm(7.0, 1.0);
    let truth_lab = zone_lab(&truth_model, 0, &truth, path, Illuminant::D65);
    let fitted = fit
        .prediction(0, 7.0, Illuminant::D65)
        .expect("a prediction for the base zone at 7 mm");
    let de = delta_e_2000(truth_lab, fitted.lab);
    assert!(de <= 3.0, "face-up colour error {de}");
    assert!(fitted.delta_e_radius.is_finite());
}

#[test]
fn recovers_a_two_zone_bicolour_with_the_geometry_fixed() {
    let scene = Scene::new(4, Some(bicolour_geometry()), 2);
    let records = scene.trace();
    assert_eq!(records.n_zones, 2);
    let model = SmoothBasisModel::new(2);
    let mut truth = log_linear(0.03, 0.2);
    truth.extend(log_linear(0.12, -0.2));
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], 0.002, 3);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    let path = face_up_path_mm(7.0, 1.0);
    for zone in 0..2 {
        let truth_lab = zone_lab(&model, zone, &truth, path, Illuminant::D65);
        let fitted = fit
            .prediction(zone, 7.0, Illuminant::D65)
            .expect("a prediction per zone");
        let de = delta_e_2000(truth_lab, fitted.lab);
        assert!(de <= 3.0, "zone {zone}: colour error {de}");
    }
    assert_eq!(fit.chosen_fit().zone_absorptions().len(), 2);
}

#[test]
fn the_covariance_covers_the_truth_at_about_one_sigma() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.06, 0.1);
    let path = face_up_path_mm(7.0, 1.0);
    let truth_lab = zone_lab(&model, 0, &truth, path, Illuminant::D65);
    let config = FitConfig {
        seeds: 3,
        stage1_iterations: 8,
        finalists: 1,
        lovo: false,
        smoothness_sigma: 0.25,
        threads: 2,
        ..FitConfig::default()
    };
    let (mut hits, mut total) = (0_usize, 0_usize);
    for seed in 0..20_u64 {
        let photos = synthesize(
            &records,
            &basis_alpha(&model, &truth),
            &[1.0; 4],
            0.01,
            100 + seed,
        );
        let fit = run(&records, &photos, &config);
        let p = fit
            .prediction(0, 7.0, Illuminant::D65)
            .expect("a prediction");
        for c in 0..3 {
            total += 1;
            if (p.lab[c] - truth_lab[c]).abs() <= p.lab_sigma[c] {
                hits += 1;
            }
        }
    }
    let coverage = hits as f64 / total as f64;
    assert!(
        (0.55..=0.80).contains(&coverage),
        "1 sigma coverage {coverage} ({hits} of {total})"
    );
}

// ---------------------------------------------------------------------------------------------
// Behaviour
// ---------------------------------------------------------------------------------------------

#[test]
fn the_result_is_bitwise_the_same_for_any_thread_count() {
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.1);
    let mut first: Option<super::ColourFit> = None;
    for (trace_threads, fit_threads) in [(1, 1), (4, 3)] {
        let scene = Scene::new(3, None, trace_threads);
        let records = scene.trace();
        let photos = synthesize(
            &records,
            &basis_alpha(&model, &truth),
            &[1.0, 0.9, 1.1],
            0.003,
            7,
        );
        let config = FitConfig {
            threads: fit_threads,
            ..quick_config()
        };
        let fit = run(&records, &photos, &config);
        match &first {
            None => first = Some(fit),
            Some(reference) => assert_eq!(
                reference, &fit,
                "{trace_threads}/{fit_threads} threads differ"
            ),
        }
    }
}

#[test]
fn a_raised_cancel_flag_stops_the_fit() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let cancel = AtomicBool::new(true);
    let result = fit_records(&records, &photos, &quick_config(), &cancel, &mut |_| {});
    assert_eq!(result.err(), Some(FitError::Cancelled));
}

#[test]
fn zoning_is_suggested_for_a_bicolour_fitted_with_one_zone_and_not_for_a_uniform_stone() {
    let zoned_scene = Scene::new(4, Some(bicolour_geometry()), 2);
    let zoned_records = zoned_scene.trace();
    let plain_scene = Scene::new(4, None, 2);
    let plain_records = plain_scene.trace();
    let two = SmoothBasisModel::new(2);
    let mut truth = log_linear(0.01, 0.0);
    truth.extend(log_linear(0.25, 0.0));
    let bicolour_photos = synthesize(
        &zoned_records,
        &basis_alpha(&two, &truth),
        &[1.0; 4],
        0.002,
        11,
    );
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let one_zone_fit = run(&plain_records, &bicolour_photos, &config);
    assert!(
        one_zone_fit.suggest_zoning,
        "score {} rms {}",
        one_zone_fit.structured_score, one_zone_fit.residual_rms
    );

    let one = SmoothBasisModel::new(1);
    let uniform = log_linear(0.08, 0.1);
    let uniform_photos = synthesize(
        &plain_records,
        &basis_alpha(&one, &uniform),
        &[1.0; 4],
        0.002,
        12,
    );
    let uniform_fit = run(&plain_records, &uniform_photos, &config);
    assert!(
        !uniform_fit.suggest_zoning,
        "score {} rms {}",
        uniform_fit.structured_score, uniform_fit.residual_rms
    );
}

#[test]
fn a_view_without_a_photo_is_an_error() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let mut photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    photos.pop();
    let result = fit_records(
        &records,
        &photos,
        &quick_config(),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert_eq!(result.err(), Some(FitError::MissingObservation(1)));
}

#[test]
fn an_unknown_host_is_an_error() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let config = FitConfig {
        host_id: Some("no such host".to_owned()),
        ..quick_config()
    };
    let result = fit_records(
        &records,
        &photos,
        &config,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(matches!(result.err(), Some(FitError::UnknownHost(_))));
}

#[test]
fn the_fit_round_trips_through_json() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    let json = serde_json::to_string(&fit).expect("serialises");
    let back: super::ColourFit = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(fit.version, back.version);
    assert_eq!(
        fit.chosen_fit().params.len(),
        back.chosen_fit().params.len()
    );
    assert_eq!(back.chosen_fit().zone_absorptions().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------------------------

#[test]
fn the_basis_derivative_matches_a_central_difference() {
    let model = SmoothBasisModel::new(2);
    let mut params = log_linear(0.05, 0.1);
    params.extend(log_linear(0.2, -0.1));
    let mut out = vec![0.0; 14];
    for zone in 0..2 {
        for lambda in [430.0, 520.0, 610.0] {
            model.dalpha(zone, lambda, &params, &mut out);
            for p in 0..14 {
                let h = 1e-6;
                let mut up = params.clone();
                up[p] += h;
                let mut down = params.clone();
                down[p] -= h;
                let numeric =
                    (model.alpha(zone, lambda, &up) - model.alpha(zone, lambda, &down)) / (2.0 * h);
                assert!(
                    (out[p] - numeric).abs() <= 1e-7 + 1e-6 * numeric.abs(),
                    "zone {zone} param {p}"
                );
            }
        }
    }
}

#[test]
fn the_chromophore_derivative_of_a_linear_element_is_the_absorption_itself() {
    let model = ChromophoreModel::new("tourmaline", &[], 1).expect("tourmaline");
    let ids = model.elements().to_vec();
    let mn = ids
        .iter()
        .position(|e| e == "Mn")
        .expect("Mn is selectable");
    let mut params: Vec<f64> = (0..ids.len()).map(|i| model.log_amount(i, 0.0)).collect();
    params[mn] = model.log_amount(mn, 0.05);
    let mut out = vec![0.0; ids.len()];
    model.dalpha(0, 520.0, &params, &mut out);
    let alpha = model.alpha(0, 520.0, &params);
    assert!(alpha > 1e-3, "the Mn band must absorb at 520 nm: {alpha}");
    // The Mn band is linear in the amount, so d alpha / d ln(amount) = alpha minus what the
    // other (floor) elements contribute, which is negligible; the central difference has an
    // error of order step^2 / 6.
    assert!(
        (out[mn] / alpha - 1.0).abs() < 5e-3,
        "{} against {alpha}",
        out[mn]
    );
}

#[test]
fn the_chromophore_model_resolves_the_same_tensor_as_the_recipe() {
    let model = ChromophoreModel::new("tourmaline", &[], 2).expect("tourmaline");
    let n = model.elements().len();
    assert_eq!(model.n_params(), 2 * n);
    let params: Vec<f64> = (0..2 * n).map(|p| model.log_amount(p % n, 0.02)).collect();
    let tensors = super::models::FitModel::tensors(&model, &params);
    assert_eq!(tensors.len(), 2);
    assert_eq!(tensors[0], tensors[1], "equal parameters give equal zones");
}

#[test]
fn linear_algebra_helpers_solve_and_invert() {
    let a = [4.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 2.0];
    let x = solve_spd(&a, 3, &[1.0, 2.0, 3.0]).expect("positive definite");
    for row in 0..3 {
        let sum: f64 = (0..3).map(|c| a[row * 3 + c] * x[c]).sum();
        assert!((sum - [1.0, 2.0, 3.0][row]).abs() < 1e-12);
    }
    let inverse = inverse_spd(&a, 3).expect("positive definite");
    for i in 0..3 {
        for j in 0..3 {
            let sum: f64 = (0..3).map(|k| a[i * 3 + k] * inverse[k * 3 + j]).sum();
            assert!((sum - f64::from(u8::from(i == j))).abs() < 1e-12);
        }
    }
    assert!(
        solve_spd(&[1.0, 2.0, 2.0, 1.0], 2, &[1.0, 1.0]).is_none(),
        "indefinite"
    );
}

#[test]
fn the_chi_square_quantile_matches_the_tables() {
    let z99 = 2.326_347_874;
    for (df, table) in [(1.0, 6.635), (2.0, 9.210), (5.0, 15.086), (10.0, 23.209)] {
        let q = chi_square_quantile(z99, df);
        assert!(
            (q / table - 1.0).abs() < 0.03,
            "df {df}: {q} against {table}"
        );
    }
}

#[test]
fn morans_i_separates_blocks_from_a_checkerboard_and_noise() {
    let (w, h) = (8, 8);
    let valid = vec![true; w * h];
    let blocks: Vec<f64> = (0..w * h)
        .map(|i| if i % w < w / 2 { 1.0 } else { -1.0 })
        .collect();
    let checker: Vec<f64> = (0..w * h)
        .map(|i| {
            if (i % w + i / w).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            }
        })
        .collect();
    assert!(moran_i(&blocks, &valid, w, h) > 0.7);
    assert!(moran_i(&checker, &valid, w, h) < -0.9);
    let mut noise = Noise(3);
    let random: Vec<f64> = (0..400).map(|_| noise.gauss()).collect();
    assert!(moran_i(&random, &vec![true; 400], 20, 20).abs() < 0.2);
    assert_eq!(moran_i(&blocks, &vec![false; w * h], w, h), 0.0);
}

#[test]
fn bad_settings_are_rejected() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    for config in [
        FitConfig {
            seeds: 0,
            ..quick_config()
        },
        FitConfig {
            gain_sigma: 0.0,
            ..quick_config()
        },
        FitConfig {
            gain_mean_sigma: 0.0,
            ..quick_config()
        },
        FitConfig {
            roughness: Some(super::RoughnessSearch {
                min: 0.5,
                max: 0.1,
                evaluations: 5,
            }),
            ..quick_config()
        },
    ] {
        let result = fit_records(
            &records,
            &photos,
            &config,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(result.err(), Some(FitError::BadConfig(_))));
    }
}

// ---------------------------------------------------------------------------------------------
// Round 2: the gain split, the objective at a given absorption, the metamer spread
// ---------------------------------------------------------------------------------------------

#[test]
fn the_gain_prior_pins_the_common_mode_harder_than_the_deviations() {
    use super::problem::Problem;
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let config = quick_config();
    let problem = Problem::new(&records, &photos, &config).expect("the problem builds");
    let np = 7;
    let n = np + 2;
    let h = problem.gain_prior_hessian(np);
    let at = |s: usize, t: usize| h[(np + s) * n + np + t];
    // P (1, 1) = sigma_m^-2 / views, P (1, -1) = sigma_d^-2.
    let common = at(0, 0) + at(0, 1);
    let deviation = at(0, 0) - at(0, 1);
    let expected_common = 1.0 / (config.gain_mean_sigma * config.gain_mean_sigma) / 2.0;
    let expected_deviation = 1.0 / (config.gain_sigma * config.gain_sigma);
    assert!(
        (common - expected_common).abs() < 1e-6 * expected_common,
        "{common}"
    );
    assert!(
        (deviation - expected_deviation).abs() < 1e-6 * expected_deviation,
        "{deviation}"
    );
    // The model block is untouched.
    assert!(h[..np * n].iter().all(|v| *v == 0.0));
}

#[test]
fn a_fixed_common_gain_stays_at_one_whatever_the_photos_say() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    // Every photo is 12 percent brighter than the model: a pure common gain.
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.12; 4], 0.002, 6);
    let config = FitConfig {
        fix_common_gain: true,
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    let mean: f64 = fit.chosen_fit().log_gains.iter().sum::<f64>() / 4.0;
    assert!(mean.abs() < 1e-3, "mean log gain {mean}");
}

#[test]
fn the_objective_at_the_truth_is_at_the_noise_floor_and_at_a_wrong_absorption_it_is_not() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.08, 0.1);
    let gains = [1.0, 0.98, 1.02, 1.0];
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &gains, 0.002, 7);
    let config = FitConfig {
        mc_variance_scale: 0.0,
        ..quick_config()
    };
    let log_gains: Vec<f64> = gains.iter().map(|g: &f64| g.ln()).collect();
    let alpha = basis_alpha(&model, &truth);
    let at_truth = super::objective_at(&records, &photos, &config, &alpha, &log_gains)
        .expect("the objective evaluates");
    let per = at_truth.chi2 / at_truth.n_data as f64;
    assert!((0.8..1.25).contains(&per), "chi2/n at the truth {per}");
    let wrong = |zone: usize, lambda: f64| 1.5 * alpha(zone, lambda);
    let at_wrong = super::objective_at(&records, &photos, &config, &wrong, &log_gains)
        .expect("the objective evaluates");
    assert!(at_wrong.chi2 > 10.0 * at_truth.chi2, "{}", at_wrong.chi2);
}

#[test]
fn the_metamer_spread_is_reported_and_grows_with_the_noise() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.06, 0.1);
    let spread_at = |sigma: f64| {
        let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], sigma, 8);
        let config = FitConfig {
            lovo: false,
            ..quick_config()
        };
        let fit = run(&records, &photos, &config);
        fit.chosen_fit()
            .predictions
            .iter()
            .map(|p| p.metamer_spread)
            .collect::<Vec<f64>>()
    };
    let quiet = spread_at(0.002);
    let noisy = spread_at(0.03);
    assert!(
        quiet
            .iter()
            .chain(&noisy)
            .all(|v| v.is_finite() && *v >= 0.0)
    );
    assert!(noisy.iter().sum::<f64>() >= quiet.iter().sum::<f64>());
}

#[test]
fn the_jacobi_eigen_decomposition_reproduces_a_small_symmetric_matrix() {
    use super::linalg::symmetric_eigen;
    let a = [4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 1.0];
    let (values, vectors) = symmetric_eigen(&a, 3);
    assert!(values[0] <= values[1] && values[1] <= values[2]);
    for k in 0..3 {
        let v = &vectors[k * 3..(k + 1) * 3];
        let norm: f64 = v.iter().map(|x| x * x).sum();
        assert!((norm - 1.0).abs() < 1e-12);
        for i in 0..3 {
            let av: f64 = (0..3).map(|j| a[i * 3 + j] * v[j]).sum();
            assert!(
                (av - values[k] * v[i]).abs() < 1e-10,
                "A v = lambda v failed"
            );
        }
    }
    let trace: f64 = values.iter().sum();
    assert!((trace - 8.0).abs() < 1e-12);
}

// ---------------------------------------------------------------------------------------------
// Round 3: the staged fit, the objective terms, the Birge ratio, the verified metamer spread
// ---------------------------------------------------------------------------------------------

#[test]
fn the_staged_fit_keeps_the_gains_near_one_for_photos_that_need_none() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.03, 0.2);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], 0.002, 11);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    let chosen = fit.chosen_fit();
    let mean: f64 = chosen.log_gains.iter().sum::<f64>() / 4.0;
    assert!(mean.abs() < config.gain_mean_sigma, "mean log gain {mean}");
    assert!(
        chosen.converged,
        "stopped after {} iterations",
        chosen.iterations
    );
    let per = chosen.chi2 / chosen.n_data as f64;
    assert!(per < 1.4, "chi2/n {per}");
    // The cost splits into the data and the priors, and the prior terms are named.
    let prior: f64 = chosen.prior_terms.iter().map(|(_, v)| *v).sum();
    assert!((chosen.cost - chosen.data_cost - prior).abs() < 1e-6 * chosen.cost.abs().max(1.0));
    let names: Vec<&str> = chosen.prior_terms.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["gain", "smoothness", "magnitude", "l1", "zone pull"]
    );
}

#[test]
fn the_objective_at_the_truth_reports_its_prior_terms_and_a_good_projection() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.08, 0.1);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], 0.002, 12);
    let config = FitConfig {
        mc_variance_scale: 0.0,
        ..quick_config()
    };
    let alpha = basis_alpha(&model, &truth);
    let at_truth = super::objective_at(&records, &photos, &config, &alpha, &[0.0; 4])
        .expect("the objective evaluates");
    // The truth is in the basis: the projection finds it.
    assert!(
        at_truth.projection_max_error < 1e-3,
        "projection error {}",
        at_truth.projection_max_error
    );
    let gain = at_truth
        .prior_terms
        .iter()
        .find(|(n, _)| n == "gain")
        .map(|(_, v)| *v)
        .expect("a gain term");
    assert_eq!(gain, 0.0, "gains of exactly 1 cost nothing");
    assert!(
        (at_truth.total - at_truth.data_cost - at_truth.prior_cost).abs()
            < 1e-9 * at_truth.total.abs().max(1.0)
    );
    // A Huber data cost never exceeds half the chi-square.
    assert!(at_truth.data_cost <= 0.5 * at_truth.chi2 + 1e-9);
}

#[test]
fn an_understated_noise_model_is_caught_by_the_birge_ratio() {
    use crate::rough_plan::colour_fit::solve::WarningKind;
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.1);
    let config = FitConfig {
        lovo: false,
        mc_variance_scale: 0.0,
        ..quick_config()
    };
    let radius_of = |fit: &super::ColourFit| {
        fit.chosen_fit()
            .predictions
            .iter()
            .map(|p| p.delta_e_radius)
            .fold(0.0_f64, f64::max)
    };
    // Honest: the stated variance is the real one.
    let honest_photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], 0.006, 13);
    let honest = run(&records, &honest_photos, &config);
    let r_honest = honest.chosen_fit().birge_ratio;
    assert!((0.7..1.3).contains(&r_honest), "honest R {r_honest}");
    assert!(
        !honest
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::NoiseUnderestimated)
    );
    // Understated: the same noise, but the photos claim a sigma three times smaller.
    let mut claimed = honest_photos;
    for view in &mut claimed {
        view.variance.fill([(0.002_f32 * 0.002); 3]);
    }
    let understated = run(&records, &claimed, &config);
    let r = understated.chosen_fit().birge_ratio;
    assert!(r > 5.0, "R {r}");
    assert!(
        understated
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::NoiseUnderestimated)
    );
    // The inflation gives back about the honest radius (it would be three times smaller).
    assert!(
        radius_of(&understated) > 0.6 * radius_of(&honest),
        "inflated radius {} against honest {}",
        radius_of(&understated),
        radius_of(&honest)
    );
}

#[test]
fn the_verified_metamer_spread_is_reported_next_to_the_unverified_one() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.06, 0.1);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 4], 0.01, 14);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    for p in &fit.chosen_fit().predictions {
        assert!(p.metamer_spread.is_finite() && p.metamer_spread >= 0.0);
        assert!(p.metamer_spread_unverified.is_finite() && p.metamer_spread_unverified >= 0.0);
    }
}

#[test]
fn the_structured_starts_are_full_parameter_vectors_inside_the_bounds() {
    use super::models::FitModel;
    let model = SmoothBasisModel::new(2);
    let seed = model.seed(0);
    let starts = model.structured_starts(&seed);
    assert_eq!(starts.len(), 2);
    let (lo, hi) = model.bounds();
    for start in &starts {
        assert_eq!(start.len(), model.n_params());
        for (i, v) in start.iter().enumerate() {
            assert!(v.is_finite() && *v >= lo[i] && *v <= hi[i], "{i}: {v}");
        }
    }
    // The flat component is removed: a flat spectrum leaves only the floor.
    let flat_removed = &starts[0];
    let largest = flat_removed.iter().map(|v| v.exp()).fold(0.0_f64, f64::max);
    assert!(largest < 0.05, "a flat seed keeps {largest} per mm");
}

#[test]
fn projecting_a_basis_absorption_onto_the_basis_returns_it() {
    let model = SmoothBasisModel::new(2);
    let truth: Vec<f64> = log_linear(0.04, 0.15)
        .into_iter()
        .chain(log_linear(0.1, -0.2))
        .collect();
    let alpha = |zone: usize, lambda: f64| model.alpha(zone, lambda, &truth);
    let (projected, worst) = SmoothBasisModel::project(2, &alpha);
    assert!(worst < 1e-4, "largest misfit {worst} per mm");
    for (a, b) in projected.iter().zip(&truth) {
        assert!((a - b).abs() < 0.05, "{a} against {b}");
    }
}

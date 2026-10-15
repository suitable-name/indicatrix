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
    ChromophoreModel, FIT_BASIS_NM, FitConfig, FitError, FitStage, ModelKind, ObservedView,
    SmoothBasisModel,
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
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "synthetic observation is stored as f32 like the real observed values"
                    )]
                    let observed = (clean + sigma * noise.gauss()) as f32;
                    values[p][c] = observed;
                }
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "synthetic variance is stored as f32 like the real observed variances"
            )]
            let variance = vec![[(sigma * sigma) as f32; 3]; n];
            ObservedView {
                view: prediction.view,
                width: prediction.grid.width,
                height: prediction.grid.height,
                values,
                variance,
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

/// A configuration that keeps the tests quick. Every photo in this module is synthesised from the
/// very records the fit evaluates (`synthesize`), so the residual carries no Monte-Carlo error of
/// the records and `mc_variance_scale` must be 0, as `tests/zoning_validation/recovery.rs` states:
/// with the shipped 1.0 the records' ~5 percent relative Monte-Carlo variance (64 samples) swamps
/// the 0.002 photo sigma, the data term goes flat (chi2 4 over 1200 residuals, measured
/// 2026-10-12) and the priors alone pick the spectrum.
fn quick_config() -> FitConfig {
    FitConfig {
        seeds: 4,
        stage1_iterations: 10,
        finalists: 1,
        threads: 2,
        mc_variance_scale: 0.0,
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

/// Log-linear amplitudes about `a0` per mm at 540 nm, with the log-slope `slope` per 40 nm, on
/// the bands of the fit basis.
fn log_linear(a0: f64, slope: f64) -> Vec<f64> {
    FIT_BASIS_NM
        .iter()
        .map(|&(centre, _)| a0.ln() + slope * (f64::from(centre) - 540.0) / 40.0)
        .collect()
}

const BANDS: usize = SmoothBasisModel::BANDS;

// ---------------------------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------------------------

/// The common body of the two single-zone recovery tests (round 7, 2026-10-12).
///
/// A three-channel camera under one LED identifies the colour of a stone and, through the path
/// lengths of the rig, a little more; it does not identify a fourteen-band spectrum. Measured on
/// this fixture: the fit of the pale stone reproduces the photos BETTER than the truth (data term
/// 621.7 against 626.8, chi2/n 1.056 against 1.064) with an absorption of 0.36 of the truth at
/// 500 nm and 0.14 at 650 nm, because the magnitude prior, which settles the camera's null
/// space, prefers that spectrum by 1.9; the saturated stone likewise (data 622.7 against 626.8,
/// magnitude 24.1 against 38.5, nothing left at 395 nm or beyond 670 nm). The former criterion,
/// "absorption within 5 or 10 % of the truth at 450 to 650 nm", asked the priors to know the
/// truth: a scale-free shape prior that recovers these log-linear truths charges the two-band
/// validation truth and an isolated band far beyond the pins of round 5 (see the notes of
/// `models`). Owner decision O2 of the 2026-10-12 analysis: the plan's criteria are colour-based.
/// What is asserted is what the data support: the optimiser reaches at least the truth's
/// objective, the noise model is honest, the predicted face-up colour differs from the truth's
/// by no more than the reported metamer spread, and the cross-validation holds. The spectrum
/// ratios are printed for the record.
fn recovers_as_camera_metamer(a0: f64) {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(a0, 0.15);
    // The log-gains have zero mean because the common mode is pinned (`fix_common_gain`): the
    // old 0.95 / 1.05 pair had a mean log-gain of -0.0006 that the pinned fit could not
    // reproduce.
    let log_gains: [f64; 4] = [0.0, -0.05, 0.05, 0.0];
    let gains: Vec<f64> = log_gains.iter().map(|g| g.exp()).collect();
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &gains, 0.002, 1);
    let config = quick_config();
    let fit = run(&records, &photos, &config);
    let chosen = fit.chosen_fit();
    assert_eq!(chosen.kind, ModelKind::SmoothBasis);
    assert!(
        chosen.converged,
        "stopped after {} iterations",
        chosen.iterations
    );
    let at_truth = super::objective_at(
        &records,
        &photos,
        &config,
        &basis_alpha(&model, &truth),
        &log_gains,
    )
    .expect("the objective at the truth");
    assert!(
        chosen.cost <= at_truth.total * (1.0 + 1e-9),
        "the fit's objective {} is above the truth's {}",
        chosen.cost,
        at_truth.total
    );
    let per = chosen.chi2 / chosen.n_data as f64;
    assert!((0.9..=1.25).contains(&per), "chi2/n {per}");
    let ratios: Vec<String> = (450..=650)
        .step_by(25)
        .map(|lambda| {
            let lambda = f64::from(lambda);
            let ratio = model.alpha(0, lambda, &chosen.params) / model.alpha(0, lambda, &truth);
            format!("{lambda:.0} nm {ratio:.3}")
        })
        .collect();
    eprintln!(
        "a0 {a0} per mm: fitted / true absorption {}",
        ratios.join(", ")
    );
    let path = face_up_path_mm(7.0, 1.0);
    let truth_lab = zone_lab(&model, 0, &truth, path, Illuminant::D65);
    let p = fit
        .prediction(0, 7.0, Illuminant::D65)
        .expect("a prediction");
    let distance = delta_e_2000(truth_lab, p.lab);
    assert!(
        distance <= p.metamer_spread + 0.25,
        "truth {distance} CIEDE2000 from the fit, spread {}",
        p.metamer_spread
    );
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
fn recovers_a_saturated_stone_as_its_camera_metamer() {
    recovers_as_camera_metamer(0.12);
}

#[test]
fn recovers_a_pale_stone_as_its_camera_metamer() {
    recovers_as_camera_metamer(0.012);
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
#[ignore = "slow colour-fit test (over 60 s); run with --ignored"]
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
    let chosen = fit.chosen_fit();
    let at_truth = super::objective_at(
        &records,
        &photos,
        &config,
        &basis_alpha(&model, &truth),
        &[0.0; 4],
    )
    .expect("the objective at the truth");
    assert!(
        chosen.cost <= at_truth.total * (1.0 + 1e-9),
        "the fit's objective {} is above the truth's {}",
        chosen.cost,
        at_truth.total
    );
    // Round 7, 2026-10-12: the former "3 CIEDE2000 per zone" asked for colours the rig does not
    // identify (the fit sat 4.5 and 7.3 dE from the truth with a data term below the truth's):
    // the null space of three channels under one LED is a metamer limit the spread reports, not
    // an error of the fit (owner decision O2, see `recovers_as_camera_metamer`). What the
    // search must do is reach the farthest feasible point of its constraint set towards the
    // truth for every zone, as `the_spread_reaches_the_farthest_feasible_point_towards_a_known_
    // metamer` states for one zone. The colour errors are printed for the record.
    let mut truth_x = truth.clone();
    truth_x.extend_from_slice(&[0.0; 4]);
    let reached = farthest_feasible_towards(&records, &photos, &config, &model, chosen, &truth_x);
    let path = face_up_path_mm(7.0, 1.0);
    for zone in 0..2 {
        let truth_lab = zone_lab(&model, zone, &truth, path, Illuminant::D65);
        let fitted = fit
            .prediction(zone, 7.0, Illuminant::D65)
            .expect("a prediction per zone");
        let de = delta_e_2000(truth_lab, fitted.lab);
        eprintln!(
            "zone {zone}: colour error {de:.2}, spread {:.2}",
            fitted.metamer_spread
        );
        for (name, t, labs) in &reached {
            let feasible = delta_e_2000(fitted.lab, labs[zone]);
            assert!(
                fitted.metamer_spread >= 0.8 * feasible,
                "zone {zone}: the point {t} of the way from {name} to the truth is feasible and \
                 {feasible} CIEDE2000 from the fit, but the verified spread is {}",
                fitted.metamer_spread
            );
        }
    }
    assert_eq!(chosen.zone_absorptions().len(), 2);
}

/// The posterior covariance is the one WITH the priors (`predict`): in the directions the camera
/// does not see it is the priors' width about the priors' choice, and it covers a truth there
/// only if the priors are neutral about that truth. The log-linear stone is not such a truth
/// (its absorption beyond 670 nm is invisible to the rig and the magnitude prior puts the fit
/// 5 dE away, so the old form of this test covered 16 of 60 components). Round 7, 2026-10-12:
/// the truth is therefore the fit of one noise realisation of that stone, a spectrum the priors
/// are neutral about by construction; the twenty fits of fresh realisations must then cover it
/// at about one sigma. That tests the covariance, not the priors' choice (owner decision O2).
#[test]
#[ignore = "slow colour-fit test (over 60 s); run with --ignored"]
fn the_covariance_covers_the_truth_at_about_one_sigma() {
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let config = FitConfig {
        seeds: 3,
        stage1_iterations: 8,
        finalists: 1,
        lovo: false,
        threads: 2,
        // Photos from the records themselves, as in `quick_config`: no Monte-Carlo error.
        mc_variance_scale: 0.0,
        ..FitConfig::default()
    };
    let truth = {
        let stone = log_linear(0.06, 0.1);
        let photos = synthesize(&records, &basis_alpha(&model, &stone), &[1.0; 4], 0.01, 100);
        run(&records, &photos, &config).chosen_fit().params.clone()
    };
    let path = face_up_path_mm(7.0, 1.0);
    let truth_lab = zone_lab(&model, 0, &truth, path, Illuminant::D65);
    let (mut hits, mut total) = (0_usize, 0_usize);
    for seed in 1..=20_u64 {
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
        (0.55..=1.00).contains(&coverage),
        "1 sigma coverage {coverage} ({hits} of {total})"
    );
}

// ---------------------------------------------------------------------------------------------
// Behaviour
// ---------------------------------------------------------------------------------------------

#[test]
#[ignore = "slow colour-fit test (over 60 s); run with --ignored"]
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
fn the_fitting_stage_reports_a_moving_fraction() {
    // ZC-20: the multi-start and the spread search both report, so the fraction of the fitting
    // stage moves while the model is being fitted and never goes back.
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let mut fractions: Vec<f32> = Vec::new();
    fit_records(
        &records,
        &photos,
        &quick_config(),
        &AtomicBool::new(false),
        &mut |p| {
            if p.stage == FitStage::Fitting {
                fractions.push(p.fraction);
            }
        },
    )
    .expect("the fit runs");
    assert!(fractions.len() >= 3, "{fractions:?}");
    assert!(
        fractions.windows(2).all(|w| w[0] <= w[1]),
        "the fraction went back: {fractions:?}"
    );
    assert!(
        fractions.iter().all(|f| (0.0..=1.0).contains(f)),
        "{fractions:?}"
    );
    assert!(
        fractions.last().is_some_and(|f| *f > 0.9),
        "the stage ends near 1: {fractions:?}"
    );
}

#[test]
fn the_per_stage_iterations_add_up_to_the_total() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let fit = run(&records, &photos, &quick_config());
    let chosen = fit.chosen_fit();
    assert!(chosen.iterations_per_stage.len() >= 2);
    assert_eq!(
        chosen.iterations_per_stage.iter().sum::<usize>(),
        chosen.iterations
    );
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
    let mut out = vec![0.0; 2 * BANDS];
    for zone in 0..2 {
        for lambda in [430.0, 520.0, 610.0] {
            model.dalpha(zone, lambda, &params, &mut out);
            for p in 0..2 * BANDS {
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
    // other (floor) elements contribute. With the GaussianEnergy basis, the tails of other
    // chromophores (like Fe IVCT) are wider and contribute more at 520 nm (~2.6%).
    assert!(
        (out[mn] / alpha - 1.0).abs() < 0.03,
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
    // The common mode is pinned by default (ZC-5); this test is about the free prior.
    let config = FitConfig {
        fix_common_gain: false,
        ..quick_config()
    };
    let problem = Problem::new(&records, &photos, &config).expect("the problem builds");
    let np = BANDS;
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
    let config = quick_config();
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
#[ignore = "slow colour-fit test (over 60 s); run with --ignored"]
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
    let config = quick_config();
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

/// The photos claim a sigma three times smaller than the noise they carry. Since ZC-18 the Birge
/// ratio is Huber-weighted, `R = sum min(u^2, k|u|) / (erf(k / sqrt 2) (n - p_eff))`, so for
/// `u ~ N(0, s^2)` with `s = 3`, `k = 2` its expectation is not the plain `s^2 = 9` but
/// `(E[u^2; |u| < 2] + E[2|u|; |u| >= 2]) / erf(sqrt 2)` = `(0.622 + 3.833) / 0.9545` = 4.67
/// (round 6, re-captured from the former `R > 5`, which was written for the plain ratio). The
/// plain `chi2 / dof` is asserted next to it and stays at 9; the honest case is 1 by the `erf`
/// divisor. The inflation by 4.67 in variance is 2.16 in sigma against the 3x shrink: 0.72 of the
/// honest radius, above the 0.6 asserted.
#[test]
fn an_understated_noise_model_is_caught_by_the_birge_ratio() {
    use crate::rough_plan::colour_fit::solve::WarningKind;
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.1);
    let config = FitConfig {
        lovo: false,
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
    assert!(
        (4.2..=5.2).contains(&r),
        "R {r}: a 3x understated sigma gives the Huber-weighted ratio 4.67 (plain ratio 9)"
    );
    // The plain ratio sees the same understatement at full strength.
    let fit = understated.chosen_fit();
    let plain = fit.chi2 / (fit.n_data as f64 - fit.effective_params);
    assert!(
        (7.5..=10.5).contains(&plain),
        "plain chi2/dof {plain}: a 3x understated sigma gives 9"
    );
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

/// The spread is the largest colour distance over `Delta chi2 <= 1` of the data alone: a one
/// sigma statement about one direction, so a three-component truth lies inside it in somewhere
/// between a fifth and two thirds of the noise realisations, never in every one. The former
/// single-seed form of this test (`the_truth_of_an_identifiable_stone_lies_inside_the_
/// constrained_spread`, seed 15: truth 5.50 from the fit, spread 4.56) asserted that as a
/// certainty (owner decision O4 of the 2026-10-12 analysis); measured over the eight seeds
/// below, the truth is inside in 2 of 8. What the search must do in every realisation is reach
/// the farthest feasible point of its own constraint set towards the truth, as
/// `the_spread_reaches_the_farthest_feasible_point_towards_a_known_metamer` states for two
/// views; this is the same statement for the four-view rig over eight realisations. The
/// containment count is printed for the record.
#[test]
fn the_spread_reaches_the_farthest_feasible_point_towards_the_truth_in_every_realisation() {
    const SEEDS: u64 = 8;
    let scene = Scene::new(4, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.06, 0.1);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let path = face_up_path_mm(7.0, 1.0);
    let truth_lab = zone_lab(&model, 0, &truth, path, Illuminant::D65);
    let mut truth_x = truth.clone();
    truth_x.extend_from_slice(&[0.0; 4]);
    let mut inside = 0_u64;
    let mut report = Vec::new();
    for seed in 15..15 + SEEDS {
        let photos = synthesize(
            &records,
            &basis_alpha(&model, &truth),
            &[1.0; 4],
            0.002,
            seed,
        );
        let fit = run(&records, &photos, &config);
        let p = fit
            .prediction(0, 7.0, Illuminant::D65)
            .expect("a prediction");
        let distance = delta_e_2000(truth_lab, p.lab);
        if distance <= p.metamer_spread + 0.25 {
            inside += 1;
        }
        report.push(format!(
            "seed {seed}: truth {distance:.2} from the fit, spread {:.2}",
            p.metamer_spread
        ));
        let reached = farthest_feasible_towards(
            &records,
            &photos,
            &config,
            &model,
            fit.chosen_fit(),
            &truth_x,
        );
        for (name, t, labs) in reached {
            let feasible = delta_e_2000(p.lab, labs[0]);
            assert!(
                p.metamer_spread >= 0.8 * feasible,
                "seed {seed}: the point {t} of the way from {name} to the truth is feasible and \
                 {feasible} CIEDE2000 from the fit, but the verified spread is {}",
                p.metamer_spread
            );
        }
    }
    eprintln!(
        "the truth lies inside the spread in {inside} of {SEEDS} realisations: {}",
        report.join("; ")
    );
}

#[test]
fn the_objective_at_a_given_absorption_needs_one_log_gain_per_view() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let alpha = basis_alpha(&model, &truth);
    let result = super::objective_at(&records, &photos, &quick_config(), &alpha, &[0.0]);
    assert!(matches!(result, Err(FitError::BadConfig(_))));
}

// ---------------------------------------------------------------------------------------------
// Round 5: the priors, the basis and the export
// ---------------------------------------------------------------------------------------------

/// The absorption of `(centre nm, FWHM cm^-1, peak per mm)` energy-domain bands.
fn band_alpha(bands: &[(f32, f32, f32)]) -> impl Fn(usize, f64) -> f64 + '_ {
    move |_zone, lambda| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "wavelength in nm is a few hundred; the band evaluator takes f32"
        )]
        let lambda_nm = lambda as f32;
        bands
            .iter()
            .map(|&(c, w, p)| f64::from(AbsorptionBand::energy(c, w, p).evaluate(lambda_nm)))
            .sum()
    }
}

/// The prior costs `(smoothness, magnitude)` of one zone at the log-amplitudes `theta`.
fn model_prior_cost(theta: &[f64], config: &FitConfig) -> (f64, f64) {
    use super::models::{FitModel, PriorContext, PriorTag};
    let model = SmoothBasisModel::new(1);
    let ones = vec![1.0; theta.len()];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: true,
    };
    let mut rows = Vec::new();
    model.prior_rows(theta, &ctx, &mut rows);
    let sum = |tag: PriorTag| -> f64 {
        rows.iter()
            .filter(|r| r.tag == tag)
            .map(|r| 0.5 * r.value * r.value)
            .sum()
    };
    (sum(PriorTag::Smoothness), sum(PriorTag::Magnitude))
}

#[test]
fn the_priors_cost_the_validation_truths_almost_nothing() {
    // The saturated and pale broad bands of the validation suite (tests/zoning_validation/synth.rs)
    // and the narrow pair of its model-class case (the 700 cm^-1 bands of the bicolour base and
    // the watermelon core). The old prior charged 2462 for the saturated pair; the bound here is the
    // review's "twice the band count per zone".
    const SATURATED: [(f32, f32, f32); 2] = [(545.0, 2600.0, 0.2), (390.0, 3500.0, 0.16)];
    const PALE: [(f32, f32, f32); 2] = [(545.0, 2600.0, 0.02), (390.0, 3500.0, 0.016)];
    const NARROW_BASE: [(f32, f32, f32); 3] = [
        (580.0, 700.0, 0.04),
        (625.0, 700.0, 0.03),
        (540.0, 700.0, 0.024),
    ];
    const NARROW_CORE: [(f32, f32, f32); 3] = [
        (580.0, 700.0, 0.08),
        (625.0, 700.0, 0.06),
        (540.0, 700.0, 0.048),
    ];
    let config = FitConfig::default();
    for (name, bands) in [
        ("saturated", &SATURATED[..]),
        ("pale", &PALE[..]),
        ("narrow base", &NARROW_BASE[..]),
        ("narrow core", &NARROW_CORE[..]),
    ] {
        let alpha = band_alpha(bands);
        let (theta, _) = SmoothBasisModel::project(1, &alpha);
        let (smooth, magnitude) = model_prior_cost(&theta, &config);
        assert!(
            smooth + magnitude <= 2.0 * BANDS as f64,
            "{name}: smoothness {smooth}, magnitude {magnitude}"
        );
    }
}

#[test]
fn zero_absorption_is_free_and_an_isolated_band_is_cheap() {
    let config = FitConfig::default();
    let empty = vec![-13.8; BANDS];
    let (smooth, magnitude) = model_prior_cost(&empty, &config);
    assert!(smooth + magnitude < 1e-3, "{smooth} + {magnitude}");
    let mut isolated = empty;
    isolated[BANDS / 2] = 0.2_f64.ln();
    let (smooth, magnitude) = model_prior_cost(&isolated, &config);
    assert!(
        smooth + magnitude < 2.0 * BANDS as f64,
        "{smooth} + {magnitude}"
    );
    // L1: 0.2 per mm at the scale 0.05 costs 4.0 (round 6, R2; it was 0.4 at the scale 0.5).
    assert!((magnitude - 4.0).abs() < 0.05, "magnitude {magnitude}");
}

/// A piecewise-linear absorption through `values` at the centres of the fit basis (constant
/// beyond the ends), so the test does not depend on the basis width.
fn through_centres(values: [f64; 14]) -> impl Fn(usize, f64) -> f64 {
    move |_zone, lambda| {
        if lambda <= f64::from(FIT_BASIS_NM[0].0) {
            return values[0];
        }
        for (pair, v) in FIT_BASIS_NM.windows(2).zip(values.windows(2)) {
            let (c0, c1) = (f64::from(pair[0].0), f64::from(pair[1].0));
            if lambda <= c1 {
                return v[0] + (lambda - c0) / (c1 - c0) * (v[1] - v[0]);
            }
        }
        values[values.len() - 1]
    }
}

#[test]
fn the_priors_prefer_the_truth_to_its_camera_metamer() {
    // The saturated truth of the validation suite and the spectrum the main-tracer fit landed
    // on (round 6, R2): the same colour to the camera, a red hump of 0.38 per mm instead of the
    // 395 to 420 nm absorption. At the old scales (sigma_a 1, b 0.5) the metamer cost only 1.9
    // more than the truth, less than the data's own noise preference; at sigma_a 0.3 and b 0.05
    // it costs about 20 more. Both are projected onto the basis, so the figure does not depend
    // on its width.
    const TRUTH: [f64; 14] = [
        0.1562, 0.0749, 0.0167, 0.0081, 0.0491, 0.1454, 0.2000, 0.1533, 0.0754, 0.0265, 0.0072,
        0.0016, 0.0003, 0.0001,
    ];
    const METAMER: [f64; 14] = [
        0.0000, 0.0005, 0.0073, 0.0348, 0.0719, 0.1503, 0.2125, 0.1299, 0.0388, 0.0308, 0.1661,
        0.3772, 0.3794, 0.1847,
    ];
    let config = FitConfig::default();
    let cost = |values: [f64; 14]| -> f64 {
        let alpha = through_centres(values);
        let (theta, _) = SmoothBasisModel::project(1, &alpha);
        let (smooth, magnitude) = model_prior_cost(&theta, &config);
        smooth + magnitude
    };
    let (truth, metamer) = (cost(TRUTH), cost(METAMER));
    assert!(
        metamer - truth >= 10.0,
        "the metamer costs {metamer}, the truth {truth}: the priors do not separate them"
    );
}

/// The farthest points of the data-only constraint set `C(x) <= C(x_data) + 1` (the set of
/// `extremes`, with `x_data` the optimum without the model priors, computed as the constrained
/// search computes it) on the straight segments from the fit and from the data optimum towards
/// `truth_x` (the model parameters followed by the log-gains), found by bisection: `(start,
/// fraction of the way, the face-up D65 Lab at 7 mm of every zone there)`. A start outside the
/// set is skipped.
fn farthest_feasible_towards(
    records: &ForwardRecords,
    photos: &[ObservedView],
    config: &FitConfig,
    model: &SmoothBasisModel,
    chosen: &super::ModelFit,
    truth_x: &[f64],
) -> Vec<(&'static str, f64, Vec<[f64; 3]>)> {
    use super::{
        lm::{LmSettings, run_lm},
        models::{FitModel, PriorContext},
        problem::Problem,
    };
    let np = chosen.params.len();
    let active = vec![true; chosen.log_gains.len()];
    let problem = Problem::new(records, photos, config).expect("the problem builds");
    let search = model.search_space().unwrap_or_else(|| model.duplicate());
    let mut fit_x = chosen.params.clone();
    fit_x.extend_from_slice(&chosen.log_gains);
    let mut start = model.to_search_space(&chosen.params);
    start.extend_from_slice(&chosen.log_gains);
    let ones = vec![1.0; np];
    let ctx = PriorContext {
        config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: false,
    };
    let optimum = run_lm(
        &problem,
        &*search,
        &start,
        &active,
        &ctx,
        &LmSettings::new(config.max_iterations, config.rel_cost_tol),
        &AtomicBool::new(false),
    )
    .expect("the data-only optimum");
    let limit = optimum.normal.chi2 + 2.0 * optimum.normal.prior.gain + 1.0;
    let mut optimum_x = model.from_search_space(&optimum.x[..np]);
    optimum_x.extend_from_slice(&optimum.x[np..]);
    let constraint = |x: &[f64]| problem.constraint_value(model, x).expect("the constraint");
    let path = face_up_path_mm(7.0, 1.0);
    let mut reached = Vec::new();
    for (name, from) in [("the fit", &fit_x), ("the data optimum", &optimum_x)] {
        if constraint(from) > limit {
            continue;
        }
        let at = |t: f64| -> Vec<f64> {
            from.iter()
                .zip(truth_x)
                .map(|(a, b)| a + t * (b - a))
                .collect()
        };
        let t = if constraint(&at(1.0)) <= limit {
            1.0
        } else {
            let (mut low, mut high) = (0.0_f64, 1.0_f64);
            for _ in 0..30 {
                let middle = f64::midpoint(low, high);
                if constraint(&at(middle)) <= limit {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            low
        };
        let point = at(t);
        let labs = (0..model.zone_count())
            .map(|zone| zone_lab(model, zone, &point[..np], path, Illuminant::D65))
            .collect();
        reached.push((name, t, labs));
    }
    reached
}

#[test]
fn the_spread_reaches_the_farthest_feasible_point_towards_a_known_metamer() {
    // Round 6, R3. The verified spread must contain every point of the constraint set it is
    // supposed to cover: take the segments from the fit and from the data optimum towards the
    // truth, bisect for the farthest point with `C <= C(x_data) + 1` (the set of `extremes`),
    // and require the reported spread to reach 80 % of the colour distance to it. Two views
    // leave a real null space, so the distance is not zero.
    let scene = Scene::new(2, None, 2);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.15, 0.2);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 16);
    let config = FitConfig {
        lovo: false,
        ..quick_config()
    };
    let fit = run(&records, &photos, &config);
    let mut truth_x = truth;
    truth_x.extend_from_slice(&[0.0; 2]);
    let p = fit
        .prediction(0, 7.0, Illuminant::D65)
        .expect("a prediction");
    let reached = farthest_feasible_towards(
        &records,
        &photos,
        &config,
        &model,
        fit.chosen_fit(),
        &truth_x,
    );
    for (name, t, labs) in reached {
        let distance = delta_e_2000(p.lab, labs[0]);
        assert!(
            p.metamer_spread >= 0.8 * distance,
            "the point {t} of the way from {name} to the truth is feasible and {distance} \
             CIEDE2000 from the fit, but the verified spread is {}",
            p.metamer_spread
        );
    }
}

#[test]
fn the_smoothness_prior_rows_match_a_numeric_derivative() {
    use super::models::{FitModel, PriorContext};
    let config = FitConfig::default();
    let model = SmoothBasisModel::new(1);
    let ones = vec![1.0; BANDS];
    let ctx = PriorContext {
        config: &config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: true,
    };
    let theta = log_linear(0.07, -0.15);
    let mut rows = Vec::new();
    model.prior_rows(&theta, &ctx, &mut rows);
    for p in [0, BANDS / 2, BANDS - 1] {
        let h = 1e-6;
        let mut up = theta.clone();
        up[p] += h;
        let mut down = theta.clone();
        down[p] -= h;
        let (mut rows_up, mut rows_down) = (Vec::new(), Vec::new());
        model.prior_rows(&up, &ctx, &mut rows_up);
        model.prior_rows(&down, &ctx, &mut rows_down);
        for (i, row) in rows.iter().enumerate() {
            let analytic: f64 = row
                .entries
                .iter()
                .filter(|(q, _)| *q == p)
                .map(|(_, d)| *d)
                .sum();
            let numeric = (rows_up[i].value - rows_down[i].value) / (2.0 * h);
            assert!(
                (analytic - numeric).abs() <= 1e-6 + 1e-5 * numeric.abs(),
                "row {i} parameter {p}: {analytic} against {numeric}"
            );
        }
    }
}

#[test]
fn the_fit_basis_has_fourteen_bands_and_the_model_priors_can_be_switched_off() {
    use super::models::{FitModel, PriorContext};
    assert_eq!(FIT_BASIS_NM.len(), 14);
    assert_eq!(BANDS, 14);
    assert!((FIT_BASIS_NM[0].0 - 395.0).abs() < 1e-6);
    assert!((FIT_BASIS_NM[1].0 - FIT_BASIS_NM[0].0 - 25.0).abs() < 1e-6);
    let config = FitConfig::default();
    let model = SmoothBasisModel::new(2);
    let ones = vec![1.0; 2 * BANDS];
    let ctx = PriorContext {
        config: &config,
        l1_weights: &ones,
        pin_common_gain: false,
        model_priors: false,
    };
    let mut rows = Vec::new();
    model.prior_rows(&model.seed(0), &ctx, &mut rows);
    assert!(rows.is_empty());
}

#[test]
fn the_exported_tensors_of_a_broad_spectrum_stay_within_three_percent() {
    use super::models::FitModel;
    use indicatrix::optics::zoning::ZoneAbsorption;
    let model = SmoothBasisModel::new(1);
    let theta: Vec<f64> = FIT_BASIS_NM
        .iter()
        .map(|&(c, _)| {
            (0.2 * (-((f64::from(c) - 560.0) / 80.0).powi(2)).exp())
                .max(1e-6)
                .ln()
        })
        .collect();
    let tensors = model.tensors(&theta);
    assert_eq!(tensors.len(), 1);
    assert!(
        tensors[0].o_ray.len() <= 7,
        "the GPU carries at most eight bands"
    );
    let exported = ZoneAbsorption::per_mm(tensors[0].clone());
    let peak = (400..=700)
        .step_by(5)
        .map(|l| model.alpha(0, f64::from(l), &theta))
        .fold(0.0_f64, f64::max);
    for l in (400..=700).step_by(5) {
        let l = f64::from(l);
        let difference = (model.alpha(0, l, &theta) - exported.alpha(l, None)).abs();
        assert!(
            difference <= 0.03 * peak,
            "{l} nm: {difference} against peak {peak}"
        );
    }
}

#[test]
fn an_unreasonably_long_smoothness_length_is_rejected() {
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.0);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let config = FitConfig {
        smoothness_length_nm: 1.0e9,
        ..quick_config()
    };
    let result = fit_records(
        &records,
        &photos,
        &config,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(matches!(result.err(), Some(FitError::BadConfig(_))));
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
    assert!(largest < 0.08, "a flat seed keeps {largest} per mm");
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

#[test]
fn the_chunked_normal_pass_equals_one_serial_sum() {
    use super::{
        models::FitModel,
        problem::{NormalPart, PixelPass, Problem},
    };
    use crate::rough_plan::colour_fit::forward::for_each_pixel_jacobian;
    // The parallel, chunked data pass of `Problem::normal` against a single serial fold over the
    // same pixels with the same per-pixel code: only the summation order may differ.
    let scene = Scene::new(2, None, 1);
    let records = scene.trace();
    let model = SmoothBasisModel::new(1);
    let truth = log_linear(0.05, 0.1);
    let photos = synthesize(&records, &basis_alpha(&model, &truth), &[1.0; 2], 0.002, 5);
    let config = quick_config();
    let problem = Problem::new(&records, &photos, &config).expect("the problem builds");
    let np = model.n_params();
    let mut x = truth;
    x.extend_from_slice(&[0.1, -0.05]);
    let n = x.len();
    let active = [true; 2];
    let chunked = problem
        .data_pass(&model, &x, &active, 4)
        .expect("the chunked pass");
    let gains: Vec<f64> = x[np..].iter().map(|g| g.exp()).collect();
    let pass = PixelPass {
        views: &problem.views,
        gains: &gains,
        active: &active,
        k: config.huber_k,
        np,
        n,
    };
    let mut serial = NormalPart::new(np, n);
    for_each_pixel_jacobian(&records, &x[..np], model.as_spectral(), &mut |px| {
        pass.add(&mut serial, &px);
    })
    .expect("the serial pass");
    let close = |a: f64, b: f64, what: &str| {
        let scale = a.abs().max(b.abs()).max(1e-300);
        assert!((a - b).abs() <= 1e-9 * scale, "{what}: {a} against {b}");
    };
    assert_eq!(chunked.count, serial.count);
    assert!(serial.count > 0, "the fixture has data");
    close(chunked.data_cost, serial.data_cost, "data_cost");
    close(chunked.chi2, serial.chi2, "chi2");
    close(chunked.chi2_robust, serial.chi2_robust, "chi2_robust");
    for (i, (a, b)) in chunked.h.iter().zip(&serial.h).enumerate() {
        close(*a, *b, &format!("h[{i}]"));
    }
    for (i, (a, b)) in chunked.g.iter().zip(&serial.g).enumerate() {
        close(*a, *b, &format!("g[{i}]"));
    }
}

//! Recovery of the face-up colour (plan section 10.1): the predicted colour of a 7 mm and a 12 mm
//! stone cut from the rough, under D65 and A, within CIEDE2000 1.5 (RAW tier) or 3.0 (JPEG tier);
//! the leave-one-view-out median within 2.0.
//!
//! Everything runs by default: the full matrix (six roughs times five colour cases, the JPEG tier,
//! and photos rendered by the independent main CPU tracer).

use std::sync::atomic::AtomicBool;

use indicatrix::optics::absorption::BODY_COLOR_BASIS_NM;
use indicatrix_cut_core::rough_plan::colour_fit::{
    forward::ForwardRecords,
    solve::{ColourFit, FitConfig, ObjectiveAt, ObservedView, fit_records, objective_at},
};

use crate::synth::{
    ColourCase, Light, MAIN_TRACER_DEPTH, MainView, NoiseSpec, RoughKind, Setup, SetupSpec,
    Spectrum, Tier, face_up_errors, illuminant_label, jpeg_round_trip, photos_forward, photos_jpeg,
    quick_fit_config, render_main, truth_log_gains,
};

/// The limit for photos from the independent main tracer: the camera, the light spectrum and the
/// wavelength grid are matched on purpose, but a different transport code and Monte-Carlo noise
/// stay in the photos, so the JPEG-tier figure applies.
const MAIN_TRACER_LIMIT: f64 = 3.0;

/// The camera-ray samples per pixel of the forward trace for the main-tracer photos (round 3, D5).
///
/// The records' own Monte-Carlo noise is NOT in the weights of these fits (the records' term is
/// the relative error at the reference absorption, which overstates a pale stone ten-fold). Noise
/// in the PREDICTION is errors-in-variables: it biases the fitted gain below 1 by about
/// `var(noise) / var(signal)` (regression dilution; the first runs found 0.82 for the saturated
/// stone, 0.95 for the pale one, in the order of their relative noise) and raises chi2/n above 1
/// by `1 + var(forward noise) / var(photo noise)`. Tracing 32 times more paths than the photos
/// are made of (4096 against 1024 rays per pixel) puts that excess at a quarter, and the pixels
/// are few (10 x 10 per view), so it costs seconds.
const MAIN_FORWARD_SAMPLES: usize = 4096;
/// The rays per working pixel of the main tracer's photos.
const MAIN_TRACER_SPP: u32 = 1024;

/// A fit together with the objective evaluated at the TRUTH (the truth absorption and the truth
/// gains of the photos, with the very weights the fit used): the diagnostic of round 2, C2.
struct Run {
    fit: ColourFit,
    at_truth: ObjectiveAt,
    /// The median relative Monte-Carlo error of the records and of the photos (round 3, D5).
    noise: String,
}

/// The median of `values` (0 when empty).
fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The relative Monte-Carlo error of the forward records (at the reference absorption 0.3 per mm,
/// which overstates it for a pale stone) next to the relative noise of the photos' green channel,
/// medians over the used pixels. When the first is not clearly below the second, the unmodelled
/// noise of the records dilutes the fitted gain (see [`MAIN_FORWARD_SAMPLES`]).
fn noise_summary(records: &ForwardRecords, photos: &[ObservedView]) -> String {
    let (mut forward, mut photo) = (Vec::new(), Vec::new());
    for view in &records.views {
        let Some(observed) = photos.iter().find(|o| o.view == view.view) else {
            continue;
        };
        for p in 0..view.pixel_count() {
            if !view.is_valid(p) {
                continue;
            }
            let mean = f64::from(view.mc_mean[p]);
            if mean > 1e-6 {
                forward.push(f64::from(view.mc_variance[p]).sqrt() / mean);
            }
            let value = f64::from(observed.values[p][1]);
            let variance = f64::from(observed.variance[p][1]);
            if value > 1e-6 && variance.is_finite() {
                photo.push(variance.sqrt() / value);
            }
        }
    }
    format!(
        "median relative noise: forward records {:.4} (at the reference absorption), photos (green) {:.4}",
        median(forward),
        median(photo)
    )
}

/// The prior term whose value differs most between the fit and the truth (`(name, fit, truth)`).
fn dominant_prior_term(fit: &[(String, f64)], truth: &[(String, f64)]) -> (String, f64, f64) {
    let mut best = (String::from("none"), 0.0, 0.0);
    let mut best_gap = f64::NEG_INFINITY;
    for (name, fit_value) in fit {
        let truth_value = truth
            .iter()
            .find(|(n, _)| n == name)
            .map_or(0.0, |(_, v)| *v);
        let gap = fit_value - truth_value;
        if gap > best_gap {
            best_gap = gap;
            best = (name.clone(), *fit_value, truth_value);
        }
    }
    best
}

/// `name value` pairs on one line.
fn terms_line(terms: &[(String, f64)]) -> String {
    terms
        .iter()
        .map(|(name, value)| format!("{name} {value:.1}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A report of the fit next to the truth: per zone the face-up Lab (truth, fitted, the fit's own
/// CIEDE2000 posterior radius and its metamer spread), the absorption at the basis centres, the
/// gains and chi-square, and the chi-square and Moran's I at the truth.
///
/// It is part of every failure message, so a failed run explains itself:
///
/// * `chi2 at truth` about the number of residuals: the photos and the records agree. If the fit
///   is clearly above it, the optimiser or a degeneracy (gain against absorption) lost the truth.
/// * `chi2 at truth` far above the number of residuals: the truth itself does not fit, a
///   generator or tracer mismatch.
/// * an error well inside `radius` is a data limit (identifiability), one far outside it is a bias
///   (prior, model or a bug); an error inside the `metamer spread` is a metamer limit of the
///   camera data and not a defect of the fit.
fn report(run: &Run, setup: &Setup) -> String {
    use std::fmt::Write as _;
    let fit = &run.fit;
    let mut out = String::new();
    let chosen = fit.chosen_fit();
    let _ = writeln!(
        out,
        "model {:?}, chi2 {:.1} over {} residuals (chi2/dof {:.3}, {:.1} effective parameters), iterations {} (converged {}), roughness {:?}, Moran {:.3}, residual rms {:.3}",
        chosen.kind,
        chosen.chi2,
        chosen.n_data,
        chosen.chi2 / (chosen.n_data as f64 - chosen.effective_params).max(1.0),
        chosen.effective_params,
        chosen.iterations,
        chosen.converged,
        fit.roughness.as_ref().map(|r| r.roughness),
        fit.structured_score,
        fit.residual_rms
    );
    let truth = &run.at_truth;
    let n = truth.n_data.max(1) as f64;
    let per_truth = truth.chi2 / n;
    let per_fit = chosen.chi2 / chosen.n_data.max(1) as f64;
    let _ = writeln!(
        out,
        "chi2 at truth {:.1} over {} residuals (chi2/n {:.3}), Moran at truth {:.3}, rms at truth {:.3}; at the fit chi2/n {:.3}, Moran {:.3}",
        truth.chi2, truth.n_data, per_truth, truth.moran, truth.rms, per_fit, fit.structured_score
    );
    let _ = writeln!(
        out,
        "Birge ratio R {:.3} (the stone region: chi2 / (n - effective parameters); above {} the covariance and radii are inflated by it); {}",
        chosen.birge_ratio,
        indicatrix_cut_core::rough_plan::colour_fit::solve::BIRGE_THRESHOLD,
        run.noise
    );
    // Round 3, D1.3: the TOTAL objective (data plus priors) at the fit and at the truth.
    let _ = writeln!(
        out,
        "objective at the fit {:.1} = data {:.1} + priors {:.1} [{}]",
        chosen.cost,
        chosen.data_cost,
        chosen.cost - chosen.data_cost,
        terms_line(&chosen.prior_terms)
    );
    let _ = writeln!(
        out,
        "objective at the truth {:.1} = data {:.1} + priors {:.1} [{}] (priors of the truth projected onto the basis, largest alpha difference {:.4}/mm)",
        truth.total,
        truth.data_cost,
        truth.prior_cost,
        terms_line(&truth.prior_terms),
        truth.projection_max_error
    );
    // Round 3, D2: the classification uses the chi2 difference, not the two chi2/n against 1.
    let delta_chi2 = chosen.chi2 - truth.chi2;
    let threshold = (2.0 * chosen.effective_params).max(10.0);
    let floor = 3.0f64.mul_add((2.0 / n).sqrt(), 1.0);
    let _ = writeln!(
        out,
        "delta chi2 = fit - truth = {delta_chi2:.1} against the threshold max(2 * {:.1} effective parameters, 10) = {threshold:.1}; chi2/n at truth {per_truth:.3} against {floor:.3}",
        chosen.effective_params
    );
    let verdict = if delta_chi2 > threshold {
        if truth.total < chosen.cost {
            format!(
                "the fit is not at the truth optimum, and the TOTAL objective is lower at the truth ({:.1}) than at the fit ({:.1}): OPTIMISER FAILURE",
                truth.total, chosen.cost
            )
        } else {
            let (name, at_fit, at_truth) =
                dominant_prior_term(&chosen.prior_terms, &truth.prior_terms);
            format!(
                "the fit is not at the truth optimum, but its total objective ({:.1}) is not above the truth's ({:.1}): PRIOR BIAS, dominated by the {name} prior ({at_fit:.1} at the fit against {at_truth:.1} at the truth)",
                chosen.cost, truth.total
            )
        }
    } else if per_truth > floor {
        "the fit is as good as the truth, but both are above the noise floor: the photos and the records disagree (generator, tracer or noise-model mismatch)".to_owned()
    } else {
        "identifiability: fit and truth are both at the noise floor (see the metamer spread)"
            .to_owned()
    };
    let _ = writeln!(out, "diagnosis: {verdict}");
    let gains: Vec<String> = chosen
        .view_ids
        .iter()
        .zip(&chosen.log_gains)
        .map(|(v, g)| format!("view {v}: {:.4}", g.exp()))
        .collect();
    let _ = writeln!(
        out,
        "gains {} (truth {:?})",
        gains.join(", "),
        &crate::synth::GAINS[..gains.len().min(8)]
    );
    let fitted = fit.zone_absorptions();
    for zone in 0..setup.truth.zone_count() {
        let _ = writeln!(out, "zone {zone}:");
        for size_mm in crate::synth::SIZES_MM {
            for illuminant in crate::synth::illuminants() {
                let truth = setup.truth.face_up_lab(zone, size_mm, illuminant);
                if let Some(p) = fit.prediction(zone, size_mm, illuminant) {
                    let _ = writeln!(
                        out,
                        "  {size_mm} mm {}: truth Lab [{:.1} {:.1} {:.1}] fitted [{:.1} {:.1} {:.1}]                          dE2000 {:.2} (posterior radius {:.2}, verified metamer spread {:.2}, unverified {:.2})",
                        illuminant_label(illuminant),
                        truth[0],
                        truth[1],
                        truth[2],
                        p.lab[0],
                        p.lab[1],
                        p.lab[2],
                        indicatrix::color::body_color::delta_e_2000(truth, p.lab),
                        p.delta_e_radius,
                        p.metamer_spread,
                        p.metamer_spread_unverified
                    );
                }
            }
        }
        let truth_alpha: Vec<String> = BODY_COLOR_BASIS_NM
            .iter()
            .map(|&(c, _)| format!("{:.4}", setup.truth.alpha(zone, f64::from(c))))
            .collect();
        let fitted_alpha: Vec<String> = BODY_COLOR_BASIS_NM
            .iter()
            .map(|&(c, _)| {
                format!(
                    "{:.4}",
                    fitted
                        .get(zone)
                        .map_or(f64::NAN, |z| z.alpha(f64::from(c), None))
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "  alpha/mm at 420..660 nm step 40: truth [{}] fitted [{}]",
            truth_alpha.join(" "),
            fitted_alpha.join(" ")
        );
    }
    out
}

/// Checks every zone of the fit against the truth of `setup` and the leave-one-view-out median.
/// All misses are collected into one message together with [`report`]. A face-up miss names the
/// metamer spread of its prediction (`limit: metamer spread X`), so the owner can tell a metamer
/// limit of the camera data (the spread reaches the error and `chi2 at truth` is about the `chi2`
/// of the fit) from a defect. The limits themselves are never changed here.
fn check(run: &Run, setup: &Setup, face_up_limit: f64, lovo_limit: f64, label: &str) {
    let fit = &run.fit;
    let mut misses = Vec::new();
    for e in face_up_errors(fit, &setup.truth) {
        if e.delta_e > face_up_limit {
            let spread = fit
                .prediction(e.zone, e.size_mm, e.illuminant)
                .map_or(f64::NAN, |p| p.metamer_spread);
            misses.push(format!(
                "zone {} at {} mm under {}: face-up dE2000 {:.3} exceeds {face_up_limit} (limit: metamer spread {:.2}{})",
                e.zone,
                e.size_mm,
                illuminant_label(e.illuminant),
                e.delta_e,
                spread,
                if spread >= e.delta_e {
                    ", the error is inside it: a metamer limit of the camera data"
                } else {
                    ""
                }
            ));
        }
    }
    let lovo = fit
        .lovo
        .as_ref()
        .expect("four views allow the leave-one-view-out check");
    if lovo.median_delta_e > lovo_limit {
        misses.push(format!(
            "leave-one-view-out median {:.3} (max {:.3}) exceeds {lovo_limit}",
            lovo.median_delta_e, lovo.max_delta_e
        ));
    }
    assert!(
        misses.is_empty(),
        "{label}: {} criteria missed:\n  {}\n{}",
        misses.len(),
        misses.join("\n  "),
        report(run, setup)
    );
}

/// Fits the photos and evaluates the objective at the truth.
///
/// The records' Monte-Carlo variance is NOT added to the photo variance for either kind of photo
/// (`mc_variance_scale` 0). The photos from `photos_forward` and `photos_jpeg` are computed from
/// the very records the fit evaluates, so they carry no Monte-Carlo error of the records. The
/// photos of the independent main tracer carry their own per-pixel variance (the spread of its
/// samples over the pixel footprint, `MainView::variance_of_mean`), which is what the weights use
/// (round 2, C5). The records' term is the relative error of a path sum at the reference
/// absorption of 0.3 per mm, where the transmittance is about `e^-3` and the relative error is
/// large, so for a pale stone it overstates the real error by a factor of ten or more (chi2/dof
/// 0.003 to 0.25 in the first run). The model-side noise of the records then shows as a chi2/dof
/// above 1 in the report instead of being hidden in the weights.
///
/// `truth_gains` are the log-gains of the photos (the nominal exposure factors of the generator;
/// zeros for photos that carry none), for the objective at the truth.
fn fit_photos(
    setup: &Setup,
    records: &ForwardRecords,
    photos: &[ObservedView],
    label: &str,
    truth_gains: &[f64],
) -> Run {
    let config = FitConfig {
        mc_variance_scale: 0.0,
        ..quick_fit_config(2)
    };
    let fit = fit_records(
        records,
        photos,
        &config,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_or_else(|e| {
        panic!(
            "{label}: the fit failed: {e} (zones {})",
            setup.truth.zone_count()
        )
    });
    let at_truth = objective_at(
        records,
        photos,
        &config,
        &|zone, lambda| setup.truth.alpha(zone, lambda),
        truth_gains,
    )
    .unwrap_or_else(|e| panic!("{label}: the objective at the truth failed: {e}"));
    Run {
        fit,
        at_truth,
        noise: noise_summary(records, photos),
    }
}

/// Traces `rough` with `colour`, makes the photos of `tier` and checks the recovery. Returns the
/// fit for further checks.
///
/// # Panics
///
/// When a criterion is missed.
pub fn run_recovery(rough: RoughKind, colour: ColourCase, tier: Tier) -> ColourFit {
    let samples = match rough {
        RoughKind::Frosted | RoughKind::PolishedWindows => 128,
        _ => 64,
    };
    let setup = Setup::new(&SetupSpec {
        rough,
        colour,
        grid_px: tier.grid_px(),
        samples,
        ..SetupSpec::default()
    });
    let label = format!("{} / {} / {tier:?}", rough.label(), colour.label());
    let records = setup.trace();
    let grid = setup.views[0].grid;
    let total = setup.views.len() * grid.width * grid.height;
    assert!(
        records.valid_pixels() * 2 >= total,
        "{label}: only {} of {total} pixels are usable",
        records.valid_pixels()
    );
    let photos = match tier {
        Tier::Raw => photos_forward(&records, &setup.truth, &NoiseSpec::RAW, 11),
        Tier::Jpeg => photos_jpeg(&records, &setup.truth, &NoiseSpec::RAW, 11, 90),
    };
    let run = fit_photos(
        &setup,
        &records,
        &photos,
        &label,
        &truth_log_gains(records.views.len()),
    );
    check(
        &run,
        &setup,
        tier.face_up_limit(),
        tier.lovo_limit(),
        &label,
    );
    run.fit
}

/// Recovers the colour of a cube in the white furnace from photos RENDERED BY THE MAIN CPU TRACER
/// (the forward model only supplies the path records the fit evaluates).
///
/// # Panics
///
/// When a criterion is missed.
pub fn run_main_tracer_recovery(colour: ColourCase) -> ColourFit {
    let setup = Setup::new(&SetupSpec {
        rough: RoughKind::Cube,
        colour,
        light: Light::Surround,
        spectrum: Spectrum::EnvironmentWhite,
        samples: MAIN_FORWARD_SAMPLES,
        max_depth: MAIN_TRACER_DEPTH as usize,
        ..SetupSpec::default()
    });
    let label = format!("main tracer photos / {}", colour.label());
    let records = setup.trace();
    let photos: Vec<ObservedView> = render_main(&setup, MAIN_TRACER_SPP, 0xBEEF_0000)
        .iter()
        .map(MainView::observed)
        .collect();
    // The main tracer's photos carry no exposure drift: the truth gains are all 1.
    let truth_gains = vec![0.0; records.views.len()];
    let run = fit_photos(&setup, &records, &photos, &label, &truth_gains);
    check(&run, &setup, MAIN_TRACER_LIMIT, MAIN_TRACER_LIMIT, &label);
    run.fit
}

// ---------------------------------------------------------------------------------------------
// The generator's own checks
// ---------------------------------------------------------------------------------------------

/// Round 2, C4: the JPEG generator must hand back a flat field within the 8-bit quantisation (one
/// and a half codes, propagated through the sRGB curve), or the common gain of the JPEG tier is a
/// generator defect and not a property of the photometry.
#[test]
fn a_flat_field_survives_the_jpeg_round_trip_within_quantisation() {
    let (w, h) = (16, 16);
    for level in [0.05_f32, 0.2, 0.35, 0.5, 0.7] {
        let back = jpeg_round_trip(w, h, &vec![[level; 3]; w * h], 90);
        let encoded = indicatrix_cut_core::rough_plan::photometry::linear_to_srgb(level);
        let tolerance = 2.4 * (1.5 / 255.0) / (f64::from(encoded) + 0.055);
        let worst = back
            .pixels
            .iter()
            .flat_map(|p| p.iter())
            .map(|v| (f64::from(*v) - f64::from(level)).abs() / f64::from(level))
            .fold(0.0_f64, f64::max);
        assert!(
            worst <= tolerance,
            "flat {level}: worst relative error {worst:.4} exceeds {tolerance:.4}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The representative case (runs by default)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_saturated_cube_recovers_its_face_up_colour_in_the_raw_tier() {
    let fit = run_recovery(RoughKind::Cube, ColourCase::UniformSaturated, Tier::Raw);
    assert_eq!(fit.n_zones, 1);
}

// ---------------------------------------------------------------------------------------------
// The full matrix
// ---------------------------------------------------------------------------------------------

macro_rules! matrix {
    ($tier:ident: $($name:ident => ($rough:ident, $colour:ident)),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_recovery(RoughKind::$rough, ColourCase::$colour, Tier::$tier);
            }
        )+
    };
}

matrix!(Raw:
    raw_cube_pale => (Cube, UniformPale),
    raw_cube_saturated => (Cube, UniformSaturated),
    raw_cube_bicolour => (Cube, Bicolour),
    raw_cube_watermelon => (Cube, WatermelonPrism),
    raw_cube_sector => (Cube, Sector),
    raw_irregular_pale => (Irregular, UniformPale),
    raw_irregular_saturated => (Irregular, UniformSaturated),
    raw_irregular_bicolour => (Irregular, Bicolour),
    raw_irregular_watermelon => (Irregular, WatermelonPrism),
    raw_irregular_sector => (Irregular, Sector),
    raw_frosted_pale => (Frosted, UniformPale),
    raw_frosted_saturated => (Frosted, UniformSaturated),
    raw_frosted_bicolour => (Frosted, Bicolour),
    raw_frosted_watermelon => (Frosted, WatermelonPrism),
    raw_frosted_sector => (Frosted, Sector),
    raw_windows_pale => (PolishedWindows, UniformPale),
    raw_windows_saturated => (PolishedWindows, UniformSaturated),
    raw_windows_bicolour => (PolishedWindows, Bicolour),
    raw_windows_watermelon => (PolishedWindows, WatermelonPrism),
    raw_windows_sector => (PolishedWindows, Sector),
    raw_holder_pale => (WithHolder, UniformPale),
    raw_holder_saturated => (WithHolder, UniformSaturated),
    raw_holder_bicolour => (WithHolder, Bicolour),
    raw_holder_watermelon => (WithHolder, WatermelonPrism),
    raw_holder_sector => (WithHolder, Sector),
    raw_immersion_pale => (Immersion, UniformPale),
    raw_immersion_saturated => (Immersion, UniformSaturated),
    raw_immersion_bicolour => (Immersion, Bicolour),
    raw_immersion_watermelon => (Immersion, WatermelonPrism),
    raw_immersion_sector => (Immersion, Sector),
);

matrix!(Jpeg:
    jpeg_cube_pale => (Cube, UniformPale),
    jpeg_cube_saturated => (Cube, UniformSaturated),
    jpeg_cube_bicolour => (Cube, Bicolour),
    jpeg_cube_watermelon => (Cube, WatermelonPrism),
    jpeg_cube_sector => (Cube, Sector),
    jpeg_windows_bicolour => (PolishedWindows, Bicolour),
    jpeg_immersion_bicolour => (Immersion, Bicolour),
);

macro_rules! main_tracer {
    ($($name:ident => $colour:ident),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                run_main_tracer_recovery(ColourCase::$colour);
            }
        )+
    };
}

main_tracer!(
    main_tracer_photos_pale => UniformPale,
    main_tracer_photos_saturated => UniformSaturated,
    main_tracer_photos_bicolour => Bicolour,
    main_tracer_photos_watermelon => WatermelonPrism,
    main_tracer_photos_sector => Sector,
);

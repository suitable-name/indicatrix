//! What the Compare step shows besides the pictures.
//!
//! The per-pixel colour difference, the
//! leave-one-view-out table, the model comparison, the predicted colour chips with their
//! uncertainty, the fit's warnings and the "This looks zoned" prompt. All of it is derived from
//! the fit result and is window-free.

use indicatrix::color::body_color::{Illuminant, delta_e_2000, srgb_to_lab};
use indicatrix_cut_core::rough_plan::{
    colour_fit::solve::{
        ChoiceReason, ColourFit, LovoReport, ModelComparison, ModelKind, WarningKind,
    },
    photometry::linear_to_srgb,
};

/// One line of a list in the window; `warn` shows it in amber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow {
    /// The words.
    pub text: String,
    /// Whether to draw it as a warning.
    pub warn: bool,
}

impl TextRow {
    const fn plain(text: String) -> Self {
        Self { text, warn: false }
    }
}

/// A colour difference above this many CIEDE2000 units is shown as a warning.
pub const WARN_DELTA_E: f64 = 3.0;

// --- Colours ---------------------------------------------------------------------------------

/// The CIE L*a*b* colour (D65) as sRGB bytes, clipped to the gamut.
#[must_use]
pub fn lab_to_srgb8(lab: [f64; 3]) -> [u8; 3] {
    const WHITE: [f64; 3] = [0.950_47, 1.0, 1.088_83];
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;
    let inverse = |t: f64| {
        let cube = t * t * t;
        if cube > 216.0 / 24389.0 {
            cube
        } else {
            116.0_f64.mul_add(t, -16.0) / (24389.0 / 27.0)
        }
    };
    let (x, y, z) = (
        WHITE[0] * inverse(fx),
        WHITE[1] * inverse(fy),
        WHITE[2] * inverse(fz),
    );
    let linear = [
        f64::mul_add(
            -0.498_531_4,
            z,
            f64::mul_add(-1.537_138_5, y, 3.240_454_2 * x),
        ),
        f64::mul_add(
            0.041_556_0,
            z,
            f64::mul_add(1.876_010_8, y, -0.969_266_0 * x),
        ),
        f64::mul_add(
            1.057_225_2,
            z,
            f64::mul_add(-0.204_025_9, y, 0.055_643_4 * x),
        ),
    ];
    let encode = |channel: f64| {
        let channel = channel.clamp(0.0, 1.0);
        let encoded = if channel <= 0.003_130_8 {
            12.92 * channel
        } else {
            f64::mul_add(1.055, channel.powf(1.0 / 2.4), -0.055)
        };
        (encoded * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [encode(linear[0]), encode(linear[1]), encode(linear[2])]
}

/// The Lab colour of a linear camera value, read as linear sRGB relative to the white backlight.
#[must_use]
pub fn linear_to_lab(rgb: [f32; 3]) -> [f64; 3] {
    let encode = |v: f32| {
        if v.is_finite() {
            f64::from(linear_to_srgb(v.clamp(0.0, 1.0)))
        } else {
            0.0
        }
    };
    srgb_to_lab([encode(rgb[0]), encode(rgb[1]), encode(rgb[2])])
}

/// The CIEDE2000 difference between an observed and a predicted linear value.
#[must_use]
pub fn delta_e_pixel(observed: [f32; 3], predicted: [f32; 3]) -> f32 {
    delta_e_2000(linear_to_lab(observed), linear_to_lab(predicted)) as f32
}

/// Summary of a difference map.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DeltaStats {
    /// Compared pixels.
    pub count: usize,
    /// Mean difference.
    pub mean: f32,
    /// Median difference.
    pub median: f32,
    /// 95th percentile.
    pub p95: f32,
    /// Largest difference.
    pub max: f32,
}

/// The per-pixel CIEDE2000 difference between `observed` and `gain * predicted` where `valid`
/// is true and both are finite; other pixels get 0 and are not counted.
///
/// The second value is the
/// validity actually used.
#[must_use]
pub fn delta_e_map(
    observed: &[[f32; 3]],
    predicted: &[[f32; 3]],
    valid: &[bool],
    gain: f32,
) -> (Vec<f32>, Vec<bool>, DeltaStats) {
    let count = observed.len().min(predicted.len());
    let mut delta = vec![0.0_f32; observed.len()];
    let mut used = vec![false; observed.len()];
    let mut values = Vec::new();
    for i in 0..count {
        if !valid.get(i).copied().unwrap_or(false) {
            continue;
        }
        let o = observed[i];
        let p = predicted[i].map(|v| v * gain);
        if o.iter().chain(p.iter()).any(|v| !v.is_finite()) {
            continue;
        }
        let d = delta_e_pixel(o, p);
        delta[i] = d;
        used[i] = true;
        values.push(d);
    }
    (delta, used, stats(values))
}

/// The statistics of a list of differences.
#[must_use]
pub fn stats(mut values: Vec<f32>) -> DeltaStats {
    if values.is_empty() {
        return DeltaStats::default();
    }
    values.sort_by(f32::total_cmp);
    let n = values.len();
    let at = |q: f64| values[(((n - 1) as f64) * q).round() as usize];
    DeltaStats {
        count: n,
        mean: values.iter().sum::<f32>() / n as f32,
        median: at(0.5),
        p95: at(0.95),
        max: values[n - 1],
    }
}

/// The line under a view's pictures: `"median 1.8, 95 % under 4.2, worst 9.1 (23 456 pixels)"`.
#[must_use]
pub fn stats_text(stats: &DeltaStats) -> String {
    if stats.count == 0 {
        return "Nothing to compare in this view.".to_owned();
    }
    format!(
        "Colour difference (CIEDE2000): median {:.1}, 95 % under {:.1}, worst {:.1} ({} pixels).",
        stats.median, stats.p95, stats.max, stats.count
    )
}

// --- Tables ----------------------------------------------------------------------------------

/// The leave-one-view-out table: one line per held-out view and a summary line.
#[must_use]
pub fn lovo_rows(report: &LovoReport, view_names: &[String]) -> Vec<TextRow> {
    let mut rows = Vec::with_capacity(report.entries.len() + 1);
    for entry in &report.entries {
        let name = view_names
            .get(entry.view)
            .cloned()
            .unwrap_or_else(|| format!("View {}", entry.view + 1));
        rows.push(TextRow {
            text: format!(
                "{name}: predicted from the other views, off by {:.1} dE ({} pixels)",
                entry.delta_e, entry.pixels
            ),
            warn: entry.delta_e > WARN_DELTA_E,
        });
    }
    rows.push(TextRow {
        text: format!(
            "Median {:.1} dE, worst {:.1} dE. This is the honest accuracy of the colour.",
            report.median_delta_e, report.max_delta_e
        ),
        warn: report.max_delta_e > 2.0 * WARN_DELTA_E,
    });
    rows
}

/// Why the model was chosen, in words.
#[must_use]
pub const fn reason_text(reason: ChoiceReason) -> &'static str {
    match reason {
        ChoiceReason::UserPreference => "chosen by you",
        ChoiceReason::OnlyOneModel => "the only model fitted",
        ChoiceReason::ChromophoreAcceptable => {
            "the chromophore model explains the photos about as well"
        }
        ChoiceReason::ChromophoreSignificantlyWorse => {
            "the chromophore model explains the photos significantly worse"
        }
    }
}

/// The name of a spectral model for the window.
#[must_use]
pub const fn model_name(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Chromophore => "Chromophore model (A)",
        ModelKind::SmoothBasis => "Smooth spectrum model (B)",
    }
}

/// The model comparison: one line per model fitted and the choice.
#[must_use]
pub fn model_rows(comparison: &ModelComparison) -> Vec<TextRow> {
    let mut rows: Vec<TextRow> = comparison
        .scores
        .iter()
        .map(|score| {
            TextRow::plain(format!(
                "{}: misfit {:.0}, {:.1} free parameters, AIC {:.0}",
                model_name(score.kind),
                score.chi2,
                score.effective_params,
                score.aic
            ))
        })
        .collect();
    rows.push(TextRow::plain(format!(
        "Shown: {} ({}).",
        model_name(comparison.chosen),
        reason_text(comparison.reason)
    )));
    rows
}

/// A predicted colour with its uncertainty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipRow {
    /// What it is: `"Zone 1, 7 mm, D65"`.
    pub label: String,
    /// The colour.
    pub srgb: [u8; 3],
    /// The numbers: `"Lab 62.1, 41.0, 55.2 +/- 1.8 dE (metamer spread 3.2 dE)"`.
    pub text: String,
}

/// The name of an illuminant for a chip.
#[must_use]
pub fn illuminant_name(illuminant: Illuminant) -> String {
    match illuminant {
        Illuminant::D65 => "D65".to_owned(),
        Illuminant::Planckian(t) if (t - 2856.0).abs() < 1.0 => "A".to_owned(),
        Illuminant::Planckian(t) => format!("{t:.0} K"),
        other => format!("{other:?}"),
    }
}

/// The name of a zone: the base zone or `"Zone k"`.
#[must_use]
pub fn zone_name(zone: usize) -> String {
    if zone == 0 {
        "Base zone".to_owned()
    } else {
        format!("Zone {zone}")
    }
}

/// The predicted colours of the shown model: every zone, at the reference size and the planned
/// size, under D65 and A, with the ± CIEDE2000 radius of the fit's uncertainty.
#[must_use]
pub fn chip_rows(fit: &ColourFit) -> Vec<ChipRow> {
    let mut predictions: Vec<_> = fit.chosen_fit().predictions.iter().collect();
    predictions.sort_by(|a, b| {
        a.zone
            .cmp(&b.zone)
            .then(a.size_mm.total_cmp(&b.size_mm))
            .then_with(|| illuminant_name(a.illuminant).cmp(&illuminant_name(b.illuminant)))
    });
    predictions
        .into_iter()
        .map(|p| ChipRow {
            label: format!(
                "{}, {:.0} mm, {}",
                zone_name(p.zone),
                p.size_mm,
                illuminant_name(p.illuminant)
            ),
            srgb: lab_to_srgb8(p.lab),
            text: format!(
                "Lab {:.1}, {:.1}, {:.1}  \u{b1} {:.1} dE (metamer spread {:.1} dE)",
                p.lab[0], p.lab[1], p.lab[2], p.delta_e_radius, p.metamer_spread
            ),
        })
        .collect()
}

/// The fit's warnings as rows (amber), plus a line for a spatially structured residual.
#[must_use]
pub fn warning_rows(fit: &ColourFit) -> Vec<TextRow> {
    let mut rows: Vec<TextRow> = fit
        .warnings
        .iter()
        .map(|w| TextRow {
            text: w.message.clone(),
            warn: !matches!(w.kind, WarningKind::PreferenceIgnored),
        })
        .collect();
    let chosen = fit.chosen_fit();
    if !chosen.at_bound.is_empty() {
        rows.push(TextRow {
            text: format!(
                "These amounts ran into their limits: {}.",
                chosen.at_bound.join(", ")
            ),
            warn: true,
        });
    }
    rows
}

/// The prompt shown when the residual is spatially structured: the stone is probably zoned.
#[must_use]
pub fn zoned_prompt(fit: &ColourFit) -> Option<String> {
    fit.suggest_zoning.then(|| {
        format!(
            "This looks zoned. The difference between photos and render is not random noise (structure score {:.2}). Add zones?",
            fit.structured_score
        )
    })
}

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;
    use indicatrix_cut_core::rough_plan::colour_fit::solve::{
        FitWarning, LikelihoodRatio, LovoEntry, ModelFit, ModelScore, ZonePrediction,
    };

    fn prediction(zone: usize, size: f64, illuminant: Illuminant) -> ZonePrediction {
        ZonePrediction {
            zone,
            size_mm: size,
            path_mm: size,
            illuminant,
            lab: [60.0, 30.0, 40.0],
            lab_sigma: [1.0; 3],
            delta_e_radius: 1.8,
            metamer_spread: 3.2,
            metamer_spread_unverified: 3.5,
        }
    }

    pub fn sample_fit() -> ColourFit {
        let model_fit = ModelFit {
            kind: ModelKind::SmoothBasis,
            host_id: None,
            param_names: vec![],
            params: vec![],
            view_ids: vec![0, 1],
            log_gains: vec![0.0, 0.0],
            covariance: vec![],
            zone_tensors: vec![],
            cost: 1.0,
            data_cost: 0.8,
            prior_terms: vec![],
            birge_ratio: 1.0,
            chi2: 10.0,
            n_data: 100,
            effective_params: 7.0,
            aic: 24.0,
            iterations: 5,
            converged: true,
            seed_costs: vec![],
            at_bound: vec!["Zone 1 amplitude 3".to_owned()],
            predictions: vec![
                prediction(1, 12.0, Illuminant::Planckian(2856.0)),
                prediction(0, 7.0, Illuminant::D65),
                prediction(1, 7.0, Illuminant::D65),
                prediction(0, 7.0, Illuminant::Planckian(2856.0)),
            ],
        };
        ColourFit {
            version: 1,
            n_zones: 2,
            fits: vec![model_fit],
            chosen_index: 0,
            comparison: ModelComparison {
                scores: vec![ModelScore {
                    kind: ModelKind::SmoothBasis,
                    chi2: 10.0,
                    effective_params: 7.0,
                    aic: 24.0,
                }],
                noise_scale2: 1.0,
                likelihood_ratio: None::<LikelihoodRatio>,
                chosen: ModelKind::SmoothBasis,
                reason: ChoiceReason::OnlyOneModel,
            },
            roughness: None,
            alignment: None,
            lovo: Some(LovoReport {
                entries: vec![
                    LovoEntry {
                        view: 0,
                        pixels: 1000,
                        delta_e: 1.2,
                        delta_e_full_fit_gain: 1.0,
                        gain: 1.0,
                    },
                    LovoEntry {
                        view: 1,
                        pixels: 900,
                        delta_e: 4.5,
                        delta_e_full_fit_gain: 3.0,
                        gain: 1.0,
                    },
                ],
                median_delta_e: 2.85,
                max_delta_e: 4.5,
            }),
            residuals: vec![],
            structured_score: 0.4,
            residual_rms: 1.5,
            suggest_zoning: true,
            warnings: vec![FitWarning {
                kind: WarningKind::NotConverged,
                message: "The fit did not converge.".to_owned(),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::sample_fit, *};

    #[test]
    fn lab_and_srgb_agree() {
        for srgb in [
            [0.8, 0.2, 0.1],
            [0.1, 0.5, 0.9],
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
        ] {
            let lab = srgb_to_lab(srgb);
            let bytes = lab_to_srgb8(lab);
            for c in 0..3 {
                let want = (srgb[c] * 255.0_f64).round() as i32;
                assert!(
                    (i32::from(bytes[c]) - want).abs() <= 1,
                    "{srgb:?} {bytes:?}"
                );
            }
        }
        // Out of gamut is clipped, not wrapped.
        let vivid = lab_to_srgb8([50.0, 120.0, 100.0]);
        assert_eq!(vivid[0], 255);
    }

    #[test]
    fn equal_colours_differ_by_zero_and_the_gain_matters() {
        let a = [0.4_f32, 0.3, 0.2];
        assert!(delta_e_pixel(a, a) < 1e-4);
        assert!(delta_e_pixel(a, [0.2, 0.3, 0.4]) > 5.0);
        let observed = vec![a; 4];
        let predicted = vec![[0.2_f32, 0.15, 0.1]; 4];
        let valid = vec![true; 4];
        let (off, _, off_stats) = delta_e_map(&observed, &predicted, &valid, 1.0);
        let (on, _, on_stats) = delta_e_map(&observed, &predicted, &valid, 2.0);
        assert!(off[0] > 5.0);
        assert!(on[0] < 1e-3);
        assert!(on_stats.mean < off_stats.mean);
    }

    #[test]
    fn the_map_skips_invalid_and_non_finite_pixels() {
        let observed = vec![[0.5_f32; 3], [0.5; 3], [f32::NAN, 0.5, 0.5]];
        let predicted = vec![[0.4_f32; 3]; 3];
        let (delta, used, s) = delta_e_map(&observed, &predicted, &[true, false, true], 1.0);
        assert_eq!(used, vec![true, false, false]);
        assert!(delta[0] > 0.0);
        assert_eq!(delta[1], 0.0);
        assert_eq!(s.count, 1);
    }

    #[test]
    fn statistics_of_a_known_list() {
        let s = stats(vec![4.0, 1.0, 2.0, 3.0, 10.0]);
        assert_eq!(s.count, 5);
        assert!((s.mean - 4.0).abs() < 1e-6);
        assert_eq!(s.median, 3.0);
        assert_eq!(s.max, 10.0);
        assert_eq!(stats(Vec::new()), DeltaStats::default());
        assert!(stats_text(&s).contains("median 3.0"));
        assert!(stats_text(&DeltaStats::default()).contains("Nothing"));
    }

    #[test]
    fn the_lovo_table_flags_large_misses() {
        let fit = sample_fit();
        let names = vec!["+X upper".to_owned()];
        let rows = lovo_rows(fit.lovo.as_ref().unwrap(), &names);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].text.starts_with("+X upper:"));
        assert!(!rows[0].warn);
        assert!(rows[1].text.starts_with("View 2:"));
        assert!(rows[1].warn);
        assert!(rows[2].text.contains("Median 2.9"));
    }

    #[test]
    fn the_model_table_names_the_choice() {
        let rows = model_rows(&sample_fit().comparison);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].text.starts_with("Smooth spectrum model (B)"));
        assert!(rows[1].text.contains("the only model fitted"));
    }

    #[test]
    fn chips_are_sorted_and_labelled() {
        let chips = chip_rows(&sample_fit());
        let labels: Vec<_> = chips.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Base zone, 7 mm, A",
                "Base zone, 7 mm, D65",
                "Zone 1, 7 mm, D65",
                "Zone 1, 12 mm, A",
            ]
        );
        assert!(chips[0].text.contains("1.8 dE"));
        assert_ne!(chips[0].srgb, [0, 0, 0]);
    }

    #[test]
    fn warnings_and_the_zoned_prompt() {
        let fit = sample_fit();
        let rows = warning_rows(&fit);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].warn);
        assert!(rows[1].text.contains("limits"));
        assert!(zoned_prompt(&fit).unwrap().starts_with("This looks zoned."));
        let mut calm = fit;
        calm.suggest_zoning = false;
        assert_eq!(zoned_prompt(&calm), None);
    }

    #[test]
    fn illuminant_names() {
        assert_eq!(illuminant_name(Illuminant::D65), "D65");
        assert_eq!(illuminant_name(Illuminant::Planckian(2856.0)), "A");
        assert_eq!(illuminant_name(Illuminant::Planckian(4000.0)), "4000 K");
        assert_eq!(zone_name(0), "Base zone");
        assert_eq!(zone_name(2), "Zone 2");
    }
}

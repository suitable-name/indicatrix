//! Fluorescence of a color recipe: the emitters of its chromophores, their quenching, and the
//! analytic glow under a UV lamp (`docs/fluorescence-plan.md` sections 3 and 5).
//!
//! "What absorbs is what glows": an emitter's excitation bands are the chromophore's own
//! resolved absorption bands scaled by its effective concentration (strength and treatment
//! removals included, polarisation averaged), so the body color and the glow share one
//! concentration. The quantum yield is the catalogue's `Phi_0` times, per quencher, the
//! concentration law `1 / (1 + (c_q / c_half)^n)`, with `c_q` the recipe amount of the quencher
//! converted into the law's unit through the host's number densities.
//!
//! Limits: at most [`MAX_EMITTERS`] emitters (the strongest are kept) of at most [`MAX_BANDS`]
//! excitation bands (the nearest bands are merged by moment matching) and [`MAX_BANDS`] emission
//! bands (checked when the catalogue loads), so the result always passes
//! [`Fluorescence::validate`]. Emission lines narrower than [`MIN_EMISSION_FWHM_NM`] are widened
//! to it, keeping their area.

use std::collections::BTreeMap;

use glam::Vec3;

use super::{
    catalogue::{ChromophoreCatalogue, ChromophoreData, FluorescenceData, HostData, QuenchData},
    recipe::{ResolveError, colorRecipe},
    resolve::{
        MAX_PEAK_PER_MM, MAX_STRENGTH, Treated, apply_treatments, clamped_amounts,
        effective_concentration, energy_params, merged_pair,
    },
};
use crate::{
    color::{
        body_color::xyz_to_lab,
        cie1931::cie_1931_cmf,
        space::{ColorSpace, TransferFunction},
    },
    optics::{
        absorption::AbsorptionBand,
        fluorescence::{
            EmissionBand, Fluorescence, FluorescentEmitter, MAX_BANDS, MAX_EMITTERS,
            MIN_EMISSION_FWHM_NM,
        },
    },
};

/// Wavelength of the 365 nm UV lamp preset.
pub const UV365_NM: f32 = 365.0;
/// Wavelength of the 395 nm UV lamp preset.
pub const UV395_NM: f32 = 395.0;

/// Relative luminance the hue of a glow is evaluated at (CIELAB hue depends weakly on it).
const HUE_REFERENCE_Y: f32 = 0.4;
/// Photon yield (emitted photons per lamp photon at the reference path) from which a glow is
/// called strong.
const STRONG_YIELD: f32 = 0.2;
/// Photon yield below which a glow is called none.
const WEAK_YIELD: f32 = 0.01;
/// `Phi_eff / Phi_0` below which the readout names the quencher.
const QUENCHED_BELOW: f32 = 0.5;

/// One emitter of a recipe with its quenching bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitterReport {
    /// Id of the emitting chromophore.
    pub chromophore: String,
    /// Quantum yield of the isolated chromophore, `Phi_0`.
    pub quantum_yield_0: f32,
    /// Effective quantum yield after quenching, `Phi_eff`.
    pub quantum_yield: f32,
    /// Each quencher with its yield factor in `(0, 1]` (element symbol, factor); factors of 1
    /// (absent quenchers) are left out.
    pub quenchers: Vec<(String, f32)>,
}

/// The fluorescence of a recipe: the renderer's [`Fluorescence`] plus what the editor shows.
#[derive(Debug, Clone, PartialEq)]
pub struct FluorescenceReport {
    /// The emitters, ready for the renderer.
    pub fluorescence: Fluorescence,
    /// One entry per emitter, in the order of `fluorescence.emitters()`.
    pub emitters: Vec<EmitterReport>,
    /// Absorption bands (per mm, polarisation averaged) of the active chromophores that do not
    /// emit: they compete with the emitters for lamp photons.
    pub background: Vec<AbsorptionBand>,
}

/// The analytic glow of a stone under a monochromatic UV lamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvGlow {
    /// Emitted photons per incident lamp photon at the reference path
    /// (`absorbed fraction * Sum_c Phi_c alpha_c / alpha_total`).
    pub photon_yield: f32,
    /// CIE XYZ of the emitted photon flux per incident lamp photon
    /// (`Sum_c yield_c Int f_c(l) cmf(l) dl`); `Y` is the relative luminance.
    pub xyz: [f32; 3],
    /// CIELAB hue angle in degrees (at a fixed relative luminance), `None` without a glow.
    pub hue_deg: Option<f32>,
    /// Swatch color, sRGB 0..1: the glow's color (gamut-clipped, full brightness) darkened
    /// with the photon yield.
    pub srgb: [f32; 3],
}

/// How strong a glow reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlowStrength {
    /// No visible glow.
    None,
    /// A faint glow.
    Weak,
    /// A clear glow.
    Strong,
}

impl UvGlow {
    /// The strength class of the glow.
    #[must_use]
    pub fn strength(&self) -> GlowStrength {
        if self.photon_yield >= STRONG_YIELD {
            GlowStrength::Strong
        } else if self.photon_yield >= WEAK_YIELD {
            GlowStrength::Weak
        } else {
            GlowStrength::None
        }
    }

    /// A color name from the hue angle of the glow, `None` without a glow.
    #[must_use]
    pub fn color_name(&self) -> Option<&'static str> {
        self.hue_deg.map(hue_name)
    }
}

/// color name of a CIELAB hue angle.
fn hue_name(h: f32) -> &'static str {
    match h {
        h if !(55.0..345.0).contains(&h) => "red",
        h if h < 80.0 => "orange",
        h if h < 105.0 => "yellow",
        h if h < 135.0 => "yellow-green",
        h if h < 180.0 => "green",
        h if h < 230.0 => "cyan",
        h if h < 320.0 => "blue",
        _ => "magenta",
    }
}

impl FluorescenceReport {
    /// The strongest quencher of the weakest-yielding quenched emitter, if any emitter lost more
    /// than half of its yield (`Phi_eff / Phi_0 < 0.5`).
    #[must_use]
    pub fn dominant_quencher(&self) -> Option<&str> {
        self.emitters
            .iter()
            .filter(|e| {
                e.quantum_yield_0 > 0.0 && e.quantum_yield / e.quantum_yield_0 < QUENCHED_BELOW
            })
            .flat_map(|e| e.quenchers.iter())
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(by, _)| by.as_str())
    }

    /// The glow under a lamp of `lamp_nm` for a stone `path_mm` long.
    #[must_use]
    pub fn glow(&self, lamp_nm: f32, path_mm: f32) -> UvGlow {
        let background: f32 = self.background.iter().map(|b| b.evaluate(lamp_nm)).sum();
        uv_glow(&self.fluorescence, background, lamp_nm, path_mm)
    }
}

/// The analytic glow of `fluorescence` under a lamp of `lamp_nm`:
/// `Sum_c Phi_eff,c * alpha_c(lamp) / alpha_total(lamp) * f_c(lambda)`, weighted by the absorbed
/// fraction `1 - exp(-alpha_total * path_mm)`, integrated against the CIE 1931 observer.
///
/// `background_per_mm` is the lamp absorption of everything that does not emit. Pure and
/// deterministic.
#[must_use]
pub fn uv_glow(
    fluorescence: &Fluorescence,
    background_per_mm: f32,
    lamp_nm: f32,
    path_mm: f32,
) -> UvGlow {
    let alphas: Vec<f32> = fluorescence
        .emitters()
        .iter()
        .map(|e| e.excitation_absorption(lamp_nm))
        .collect();
    let total = alphas.iter().sum::<f32>() + background_per_mm.max(0.0);
    let mut none = UvGlow {
        photon_yield: 0.0,
        xyz: [0.0; 3],
        hue_deg: None,
        srgb: [0.0; 3],
    };
    if !(total.is_finite() && total > 0.0) {
        return none;
    }
    let absorbed = 1.0 - (-total * path_mm.max(0.0)).exp();
    let mut xyz = Vec3::ZERO;
    let mut photon_yield = 0.0f32;
    for (e, alpha) in fluorescence.emitters().iter().zip(&alphas) {
        let yield_c = absorbed * e.quantum_yield * alpha / total;
        if yield_c <= 0.0 {
            continue;
        }
        photon_yield += yield_c;
        let mut flux = Vec3::ZERO;
        for i in 0..=400 {
            let l = 380.0 + i as f32;
            let f = e.emission_density(l);
            flux += f * Vec3::from(cie_1931_cmf(l));
        }
        xyz += yield_c * flux;
    }
    if !(photon_yield.is_finite() && photon_yield > 0.0 && xyz.y > 0.0) {
        return none;
    }
    none.photon_yield = photon_yield;
    none.xyz = xyz.to_array();
    none.hue_deg = Some(lab_hue_deg(xyz));
    none.srgb = swatch_srgb(xyz, photon_yield);
    none
}

/// CIELAB hue angle (degrees) of `xyz` scaled to a relative luminance of [`HUE_REFERENCE_Y`]
/// against the D65 white.
#[expect(clippy::many_single_char_names, reason = "CIELAB's own X, Y, Z, a, b")]
fn lab_hue_deg(xyz: Vec3) -> f32 {
    let s = f64::from(HUE_REFERENCE_Y / xyz.y);
    let lab = xyz_to_lab(
        [
            f64::from(xyz.x) * s,
            f64::from(xyz.y) * s,
            f64::from(xyz.z) * s,
        ],
        [0.950_47, 1.0, 1.088_83],
    );
    (lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0)) as f32
}

/// sRGB swatch of a glow: the chromaticity gamut-clipped to full brightness, darkened with the
/// photon yield (a quenched stone gets a dark swatch).
fn swatch_srgb(xyz: Vec3, photon_yield: f32) -> [f32; 3] {
    let linear = ColorSpace::Srgb.xyz_to_linear(xyz).max(Vec3::ZERO);
    let peak = linear.max_element();
    if peak <= 0.0 {
        return [0.0; 3];
    }
    let gain = 0.1 + 0.9 * (photon_yield / STRONG_YIELD).min(1.0);
    let lit = linear / peak * gain;
    [
        TransferFunction::Srgb.encode(lit.x),
        TransferFunction::Srgb.encode(lit.y),
        TransferFunction::Srgb.encode(lit.z),
    ]
}

/// Resolves the fluorescence of `recipe`: one [`FluorescentEmitter`] per emitting active
/// chromophore, with its quenched yield. Empty (no fluorescence) for an unknown host, a
/// non-finite or inconsistent recipe, and for recipes without emitters.
#[must_use]
pub fn resolve_fluorescence(
    catalogue: &ChromophoreCatalogue,
    recipe: &colorRecipe,
) -> Fluorescence {
    fluorescence_report(catalogue, recipe).fluorescence
}

/// [`resolve_fluorescence`] with the per-emitter quenching and the competing background
/// absorption, for the editor's glow swatches and readout.
#[must_use]
pub fn fluorescence_report(
    catalogue: &ChromophoreCatalogue,
    recipe: &colorRecipe,
) -> FluorescenceReport {
    report_impl(catalogue, recipe).unwrap_or_else(|_| FluorescenceReport {
        fluorescence: Fluorescence::new(Vec::new()),
        emitters: Vec::new(),
        background: Vec::new(),
    })
}

fn report_impl(
    catalogue: &ChromophoreCatalogue,
    recipe: &colorRecipe,
) -> Result<FluorescenceReport, ResolveError> {
    let host = catalogue
        .host(&recipe.host)
        .ok_or_else(|| ResolveError::HostNotFound(recipe.host.clone()))?;
    if !recipe.strength.is_finite() {
        return Err(ResolveError::NonFinite("strength".to_string()));
    }
    if let Some(e) = recipe.entries.iter().find(|e| !e.amount.is_finite()) {
        return Err(ResolveError::NonFinite(format!("amount of {}", e.id)));
    }
    let strength = recipe.strength.clamp(0.0, MAX_STRENGTH);

    let mut warnings = Vec::new();
    let amounts = clamped_amounts(catalogue, host, recipe, &mut warnings);
    if !host.end_members.is_empty() {
        let sum: f64 = host
            .end_members
            .iter()
            .filter(|m| !m.colorless)
            .map(|m| amounts.get(&m.id).copied().unwrap_or(0.0))
            .sum();
        if sum > 1.0 + 1e-9 {
            return Err(ResolveError::EndMemberSumExceeded(sum));
        }
    }
    let treated = apply_treatments(host, recipe, &amounts, &mut warnings);

    let mut candidates: Vec<(f32, FluorescentEmitter, EmitterReport)> = Vec::new();
    let mut background: Vec<AbsorptionBand> = Vec::new();
    for chromo in &host.chromophores {
        if !chromo.is_offered() {
            continue;
        }
        let c_eff =
            effective_concentration(host, chromo, &amounts, &treated.split, &treated.created);
        if !c_eff.is_finite() || c_eff <= 1e-12 {
            continue;
        }
        let bands = excitation_bands(host, chromo, c_eff, strength, &treated);
        let Some(entry) = chromo.fluorescence.iter().find(|f| f.is_emitting()) else {
            background.extend(bands);
            continue;
        };
        if bands.is_empty() {
            continue;
        }
        let (phi, quenchers) = effective_yield(host, entry, &amounts);
        let emitter = FluorescentEmitter {
            excitation: limit_bands(bands),
            emission: emission_bands(entry),
            quantum_yield: phi,
        };
        let weight = emitter.excitation.iter().map(|b| b.peak).sum::<f32>() * phi;
        let report = EmitterReport {
            chromophore: chromo.id.clone(),
            quantum_yield_0: entry.quantum_yield.unwrap_or(0.0) as f32,
            quantum_yield: phi,
            quenchers,
        };
        candidates.push((weight, emitter, report));
    }
    if candidates.len() > MAX_EMITTERS {
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        candidates.truncate(MAX_EMITTERS);
    }
    let (emitters, emitter_reports): (Vec<_>, Vec<_>) =
        candidates.into_iter().map(|(_, e, r)| (e, r)).unzip();
    Ok(FluorescenceReport {
        fluorescence: Fluorescence::new(emitters),
        emitters: emitter_reports,
        background,
    })
}

/// The chromophore's absorption bands in mm^-1 as one polarisation-averaged set (uniaxial
/// `(2 o + e) / 3`, biaxial `(a + b + g) / 3`, isotropic `o`), the way `resolve` scales them.
fn excitation_bands(
    host: &HostData,
    chromo: &ChromophoreData,
    c_eff: f64,
    strength: f64,
    treated: &Treated,
) -> Vec<AbsorptionBand> {
    let removals = treated.removals.get(&chromo.id);
    let mut out = Vec::new();
    for band in chromo.usable_bands() {
        let (Some(fwhm), Some(peak_coeff)) = (band.fwhm_cm1, band.peak_coeff) else {
            continue;
        };
        let mut factor = 1.0;
        for (bands_nm, f) in removals.into_iter().flatten() {
            if bands_nm.is_empty() || bands_nm.iter().any(|c| (c - band.centre_nm).abs() < 1.0) {
                factor *= f;
            }
        }
        let peak_cm =
            peak_coeff.mul_add(c_eff, band.quadratic_coeff.unwrap_or(0.0) * c_eff * c_eff);
        let coeff_mm = (peak_cm * strength * factor / 10.0).min(MAX_PEAK_PER_MM);
        let weight = |keys: &[&str], default: f64| -> f64 {
            keys.iter()
                .find_map(|k| band.pol.get(*k).copied())
                .unwrap_or(default)
                .max(0.0)
        };
        let w = match host.optical.as_str() {
            "biaxial" => (weight(&["a"], 0.0) + weight(&["b"], 0.0) + weight(&["g"], 0.0)) / 3.0,
            "uniaxial" => (2.0 * weight(&["o"], 0.0) + weight(&["e"], 0.0)) / 3.0,
            _ => weight(&["o"], 1.0),
        };
        let peak = coeff_mm * w;
        if peak.is_finite() && peak > 1e-12 {
            out.push(AbsorptionBand::energy(
                band.centre_nm as f32,
                fwhm as f32,
                peak as f32,
            ));
        }
    }
    out
}

/// Reduces `bands` to at most [`MAX_BANDS`] by repeatedly merging the two nearest ones (distance
/// in energy over the sum of their widths) into one energy Gaussian of the same area.
fn limit_bands(mut bands: Vec<AbsorptionBand>) -> Vec<AbsorptionBand> {
    bands.sort_by(|a, b| a.center_nm.total_cmp(&b.center_nm));
    while bands.len() > MAX_BANDS {
        let closeness = |a: &AbsorptionBand, b: &AbsorptionBand| {
            let (na, sa, _) = energy_params(a);
            let (nb, sb, _) = energy_params(b);
            (na - nb).abs() / (sa + sb).max(1e-9)
        };
        let i = (0..bands.len() - 1)
            .min_by(|&i, &j| {
                closeness(&bands[i], &bands[i + 1]).total_cmp(&closeness(&bands[j], &bands[j + 1]))
            })
            .unwrap_or(0);
        let merged = merged_pair(&bands[i], &bands[i + 1]);
        bands[i] = merged;
        bands.remove(i + 1);
    }
    bands
}

/// The emission bands of a catalogue entry: the lines (equal weights unless given) sharing
/// `1 - sideband.weight` of the photons, plus the sideband; narrow lines widened to 1 nm.
fn emission_bands(entry: &FluorescenceData) -> Vec<EmissionBand> {
    let n = entry.emission_nm.len();
    let line_weights: Vec<f64> = if entry.emission_weight.len() == n {
        entry.emission_weight.clone()
    } else {
        vec![1.0; n]
    };
    let total: f64 = line_weights.iter().sum();
    let sideband = entry.sideband.map_or(0.0, |s| s.weight);
    let line_share = (1.0 - sideband) / if total > 0.0 { total } else { 1.0 };
    let mut out: Vec<EmissionBand> = entry
        .emission_nm
        .iter()
        .enumerate()
        .map(|(i, &centre)| {
            let fwhm = entry
                .emission_fwhm_nm
                .get(i)
                .or_else(|| entry.emission_fwhm_nm.first())
                .copied()
                .unwrap_or(MIN_EMISSION_FWHM_NM.into());
            EmissionBand::new(
                centre as f32,
                (fwhm as f32).max(MIN_EMISSION_FWHM_NM),
                (line_weights[i] * line_share) as f32,
            )
        })
        .collect();
    if let Some(s) = entry.sideband {
        out.push(EmissionBand::new(
            s.centre_nm as f32,
            (s.fwhm_nm as f32).max(MIN_EMISSION_FWHM_NM),
            s.weight as f32,
        ));
    }
    out
}

/// `Phi_eff` of an entry in `amounts`, and its quenchers with their factors.
fn effective_yield(
    host: &HostData,
    entry: &FluorescenceData,
    amounts: &BTreeMap<String, f64>,
) -> (f32, Vec<(String, f32)>) {
    let phi0 = entry.quantum_yield.unwrap_or(0.0).clamp(0.0, 1.0);
    let mut phi = phi0;
    let mut quenchers = Vec::new();
    for q in &entry.quench {
        let factor = quench_factor(host, q, amounts);
        phi *= factor;
        if factor < 1.0 - 1e-6 {
            quenchers.push((q.by.clone(), factor as f32));
        }
    }
    (phi.clamp(0.0, 1.0) as f32, quenchers)
}

/// `1 / (1 + (c_q / c_half)^n)` for one quencher: `c_q` is the recipe amount of the element
/// converted from its input unit into the law's unit through the host's number densities.
fn quench_factor(host: &HostData, q: &QuenchData, amounts: &BTreeMap<String, f64>) -> f64 {
    let amount = amounts.get(&q.by).copied().unwrap_or(0.0);
    let (Some(unit), n_law) = (host.element_unit(&q.by), host.n_site_for_unit(&q.unit)) else {
        return 1.0;
    };
    if amount <= 0.0 || n_law <= 0.0 {
        return 1.0;
    }
    let c_q = amount * host.n_site_for_unit(&unit) / n_law;
    1.0 / (1.0 + (c_q / q.c_half).powf(q.n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> &'static ChromophoreCatalogue {
        ChromophoreCatalogue::global()
    }

    fn recipe(host: &str, amounts: &[(&str, f64)]) -> colorRecipe {
        let mut r = colorRecipe::new(host, cat().data_version);
        for (id, a) in amounts {
            r.set_amount(id, *a);
        }
        r
    }

    /// The recipe amount of `element` that equals `wt_pct` of `FeO`-style oxide `unit` (the
    /// element's input unit differs per host, so convert through the number densities).
    fn amount_of_wt_pct(host_id: &str, element: &str, unit: &str, wt_pct: f64) -> f64 {
        let host = cat().host(host_id).expect("host");
        let own = host.element_unit(element).expect("element unit");
        wt_pct * host.n_site_for_unit(unit) / host.n_site_for_unit(&own)
    }

    #[test]
    fn ruby_has_one_red_emitter_with_the_isolated_yield() {
        let fl = resolve_fluorescence(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        assert_eq!(fl.emitters().len(), 1);
        let e = &fl.emitters()[0];
        assert!((e.quantum_yield - 0.9).abs() < 1e-3, "{}", e.quantum_yield);
        assert!(
            e.emission
                .iter()
                .any(|b| (692.0..=695.0).contains(&b.centre_nm)),
            "{:?}",
            e.emission
        );
        assert!(!e.excitation.is_empty() && e.excitation.len() <= MAX_BANDS);
        fl.validate().expect("valid");
        // Excitation reaches the 365 nm lamp through the chromophore's own bands.
        assert!(e.excitation_absorption(365.0) > 0.01);
    }

    #[test]
    fn iron_quenches_ruby_below_a_tenth_of_its_yield() {
        let fe = amount_of_wt_pct("corundum", "Fe", "wt_pct_oxide:FeO", 1.0);
        let report = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5), ("Fe", fe)]));
        let e = &report.emitters[0];
        assert!(
            e.quantum_yield < 0.1 * e.quantum_yield_0,
            "Phi_eff {} of {}",
            e.quantum_yield,
            e.quantum_yield_0
        );
        assert_eq!(report.dominant_quencher(), Some("Fe"));
        // Without iron nothing is quenched and nothing is reported.
        let clean = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        assert_eq!(clean.dominant_quencher(), None);
    }

    #[test]
    fn recipes_without_emitters_are_empty() {
        for r in [
            recipe("corundum", &[("Fe", 1000.0), ("Ti", 100.0)]),
            recipe("corundum", &[]),
            recipe("unknown host", &[("Cr", 0.5)]),
        ] {
            assert!(resolve_fluorescence(cat(), &r).is_empty(), "{r:?}");
        }
    }

    #[test]
    fn every_host_at_maximum_concentration_validates() {
        for host in &cat().hosts {
            let mut r = colorRecipe::new(&host.id, cat().data_version);
            let members: Vec<_> = host.end_members.iter().filter(|m| !m.colorless).collect();
            for id in cat().selectable_elements(&host.id) {
                if members.iter().any(|m| m.id == id) {
                    r.set_amount(&id, 1.0 / members.len() as f64);
                } else {
                    r.set_amount(&id, host.element_conc_max(&id));
                }
            }
            r.treatments = host.treatments.iter().map(|t| t.id.clone()).collect();
            let fl = resolve_fluorescence(cat(), &r);
            fl.validate().unwrap_or_else(|e| panic!("{}: {e}", host.id));
            assert!(fl.emitters().len() <= MAX_EMITTERS);
        }
    }

    #[test]
    fn ruby_glows_red_under_the_365_lamp() {
        let report = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        let glow = report.glow(UV365_NM, 5.0);
        let hue = glow.hue_deg.expect("a glow");
        assert!((0.0..=40.0).contains(&hue), "hue {hue}");
        assert_eq!(glow.strength(), GlowStrength::Strong);
        assert_eq!(glow.color_name(), Some("red"));
    }

    #[test]
    fn quenched_ruby_reads_dimmer() {
        let fe = amount_of_wt_pct("corundum", "Fe", "wt_pct_oxide:FeO", 1.0);
        let clean = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        let iron = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5), ("Fe", fe)]));
        let (a, b) = (clean.glow(UV365_NM, 5.0), iron.glow(UV365_NM, 5.0));
        assert!(b.photon_yield < 0.1 * a.photon_yield, "{b:?} vs {a:?}");
    }

    /// Emerald: iron halves the R-line yield at 660 ppm Fe (0.0849 wt% FeO; a DERIVED estimate
    /// from the GIA Fall 2017 abstract, `research-2026-10-primary-data` section 8) and the n = 2
    /// law gives the abstract's tenfold fall (12-fold) at 2200 ppm Fe.
    #[test]
    fn emerald_iron_quench_halves_the_yield_at_660_ppm_fe() {
        let fe_amount = |ppmw: f64| {
            // ppm by mass of Fe -> wt% FeO (x 71.84 / 55.845) -> the recipe's Fe unit.
            amount_of_wt_pct(
                "beryl",
                "Fe",
                "wt_pct_oxide:FeO",
                ppmw * 1e-4 * 71.84 / 55.845,
            )
        };
        let ratio = |ppmw: f64| {
            let report = fluorescence_report(
                cat(),
                &recipe("beryl", &[("Cr", 0.3), ("Fe", fe_amount(ppmw))]),
            );
            let e = &report.emitters[0];
            f64::from(e.quantum_yield / e.quantum_yield_0)
        };
        assert!((ratio(660.0) - 0.5).abs() < 0.01, "{}", ratio(660.0));
        assert!((0.07..=0.10).contains(&ratio(2200.0)), "{}", ratio(2200.0));
        assert!(ratio(100.0) > 0.9, "a clean emerald is not quenched");
    }

    #[test]
    fn diamond_n3_glows_blue() {
        let report = fluorescence_report(cat(), &recipe("diamond", &[("N", 500.0)]));
        assert_eq!(report.fluorescence.emitters().len(), 1);
        let hue = report.glow(UV365_NM, 5.0).hue_deg.expect("a glow");
        assert!((230.0..=290.0).contains(&hue), "hue {hue}");
    }

    #[test]
    fn irradiation_and_anneal_create_the_h3_and_nv_emitters() {
        let mut r = recipe("diamond", &[("N", 500.0)]);
        r.treatments = vec!["irradiation_anneal_h3".to_string()];
        let h3 = resolve_fluorescence(cat(), &r);
        assert_eq!(h3.emitters().len(), 2, "N3 and H3");
        let mut r = recipe("diamond", &[("N", 500.0)]);
        r.treatments = vec!["irradiation_anneal_nv".to_string()];
        let nv = fluorescence_report(cat(), &r);
        assert_eq!(nv.fluorescence.emitters().len(), 2, "N3 and NV");
        assert!(
            nv.fluorescence
                .emitters()
                .iter()
                .any(|e| e.emission.iter().any(|b| b.centre_nm == 637.0))
        );
    }

    #[test]
    fn emission_spectra_are_normalised_and_narrow_lines_widened() {
        let fl = resolve_fluorescence(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        let e = &fl.emitters()[0];
        let total: f32 = e.emission.iter().map(|b| b.weight).sum();
        assert!((total - 1.0).abs() < 1e-5, "{total}");
        assert!(e.emission.iter().all(|b| b.fwhm_nm >= MIN_EMISSION_FWHM_NM));
    }

    #[test]
    fn the_glow_without_emitters_is_none() {
        let glow = uv_glow(&Fluorescence::new(Vec::new()), 0.1, UV365_NM, 5.0);
        assert_eq!(glow.strength(), GlowStrength::None);
        assert_eq!(glow.hue_deg, None);
        assert_eq!(glow.photon_yield, 0.0);
    }

    #[test]
    fn a_competing_background_dims_the_glow() {
        let report = fluorescence_report(cat(), &recipe("corundum", &[("Cr", 0.5)]));
        let bare = uv_glow(&report.fluorescence, 0.0, UV365_NM, 5.0);
        let crowded = uv_glow(&report.fluorescence, 10.0, UV365_NM, 5.0);
        assert!(crowded.photon_yield < 0.5 * bare.photon_yield);
        assert_eq!(report.background.len(), 0, "Cr is the only absorber here");
    }
}

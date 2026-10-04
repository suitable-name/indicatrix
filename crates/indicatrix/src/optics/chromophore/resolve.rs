//! Forward model resolution from [`colorRecipe`] to [`AbsorptionTensor`].
//!
//! Dispatch is on the chromophore `kind` and its data fields (elements, partners, valence,
//! end member), never on id strings. Enforces:
//! - amounts clamped to `[0, conc_max]`, non-finite input rejected;
//! - explicit unit conversion between concentration bases through number densities;
//! - valence splits, treatments and centre intensities from the catalogue data;
//! - pairing laws selectable per pair (`clustered_min`: `min`, `product`: `z * cA * cB * 1e-6` in
//!   `ppm_site`; `clustered`/`random` are aliases), with an optional data-driven compensator
//!   (`max(0, partner - compensator)`), active only when both partner elements are present;
//! - a data-driven quadratic term per band (`peak = k c + q c^2`, pair-enhanced Fe3+);
//! - end-member fractions with sum <= 1 and a colorless remainder;
//! - cm^-1 to mm^-1 unit conversion (divide by 10);
//! - GPU band budget of <= 8 bands per eigenmode: the cheapest pairs of energy Gaussians are
//!   merged first (color-weighted moment matching, `BandsMerged` with its dE00), whatever still
//!   exceeds the budget is dropped (CMF-weighted absorbance, `BandsDropped`); bands are emitted
//!   in `(centre, width, id)` order;
//! - pleochroic tensor construction according to host optical class (a missing pol key is
//!   weight 0, except for isotropic hosts).

use std::collections::BTreeMap;

use super::{
    catalogue::{
        ChromophoreCatalogue, ChromophoreData, HostData, TreatmentEffectData, species_element,
    },
    recipe::{ResolveError, ResolveWarning, colorRecipe},
};
use crate::{
    color::{
        body_color::{Illuminant, body_color, delta_e_2000},
        cie1931::cie_1931_cmf,
    },
    optics::absorption::{AbsorptionBand, AbsorptionTensor},
};

/// Upper bound on the `strength` multiplier.
pub(super) const MAX_STRENGTH: f64 = 1000.0;
/// Upper bound on a single band's peak in mm^-1 (keeps every `f32` cast finite).
pub(super) const MAX_PEAK_PER_MM: f64 = 1.0e4;

/// A band tagged with the id of the chromophore it came from.
type TaggedBand = (AbsorptionBand, String);

/// Per-chromophore multipliers from `remove_centre` effects: `(bands_nm filter, factor)`.
pub(super) type Removal = (Vec<f64>, f64);

/// Resolves a [`colorRecipe`] into an [`AbsorptionTensor`].
///
/// # Errors
///
/// - [`ResolveError::HostNotFound`] if the recipe's host is not in the catalogue;
/// - [`ResolveError::NonFinite`] for a NaN/infinite amount, strength or reference path;
/// - [`ResolveError::EndMemberSumExceeded`] if end-member fractions sum to more than 1.
pub fn resolve(
    recipe: &colorRecipe,
    catalogue: &ChromophoreCatalogue,
) -> Result<(AbsorptionTensor, Vec<ResolveWarning>), ResolveError> {
    resolve_impl(recipe, catalogue, true)
}

/// [`resolve`] without the <= 8 band budget (exact duplicates are still merged): the tensor
/// the solver's linear forward model corresponds to. Not for rendering (more than 8 bands).
#[cfg(test)]
pub(super) fn resolve_unbudgeted(
    recipe: &colorRecipe,
    catalogue: &ChromophoreCatalogue,
) -> Result<AbsorptionTensor, ResolveError> {
    resolve_impl(recipe, catalogue, false).map(|(t, _)| t)
}

fn resolve_impl(
    recipe: &colorRecipe,
    catalogue: &ChromophoreCatalogue,
    budget: bool,
) -> Result<(AbsorptionTensor, Vec<ResolveWarning>), ResolveError> {
    let host = catalogue
        .host(&recipe.host)
        .ok_or_else(|| ResolveError::HostNotFound(recipe.host.clone()))?;

    if !recipe.strength.is_finite() {
        return Err(ResolveError::NonFinite("strength".to_string()));
    }
    if !recipe.reference_path_mm.is_finite() {
        return Err(ResolveError::NonFinite("reference_path_mm".to_string()));
    }
    if let Some(e) = recipe.entries.iter().find(|e| !e.amount.is_finite()) {
        return Err(ResolveError::NonFinite(format!("amount of {}", e.id)));
    }
    let strength = recipe.strength.clamp(0.0, MAX_STRENGTH);

    let mut warnings = Vec::new();

    // 1. Clamped amounts keyed by selectable id.
    let amounts = clamped_amounts(catalogue, host, recipe, &mut warnings);

    // 2. End members: fractions sum <= 1, colorless remainder (not absorbing).
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

    // 3. Valence split and treatments, all from data.
    let treated = apply_treatments(host, recipe, &amounts, &mut warnings);

    // 4. Chromophores -> tagged bands per eigenmode.
    let (raw_o, raw_e, raw_b) = collect_bands(host, &amounts, &treated, strength);
    let is_biaxial = host.optical == "biaxial";
    let is_uniaxial = host.optical == "uniaxial";

    warnings.extend(relation_warnings(host, &amounts, &treated));

    // Step 2 merge: exact same centre and width; step 3: budget <= 8 per mode.
    let ref_path = f64::from(recipe.reference_path_mm);
    let fit = |raw: Vec<TaggedBand>, mode: &str| {
        let merged = merge_bands(sorted(raw));
        if budget {
            budget_bands(merged, mode, ref_path)
        } else {
            (merged, Vec::new())
        }
    };
    let (budgeted_o, warn_o) = fit(raw_o, "o_ray");
    let (budgeted_e, warn_e) = fit(raw_e, "e_ray");
    let (budgeted_b, warn_b) = fit(raw_b, "beta_ray");
    warnings.extend(warn_o.into_iter().chain(warn_e).chain(warn_b));

    let tensor = if is_biaxial {
        AbsorptionTensor::biaxial(strip(budgeted_o), strip(budgeted_b), strip(budgeted_e))
    } else if is_uniaxial {
        AbsorptionTensor::uniaxial(strip(budgeted_o), strip(budgeted_e))
    } else {
        AbsorptionTensor::isotropic(strip(budgeted_o))
    };

    Ok((tensor, warnings))
}

/// The chromophore of `host` that a catalogue `requires` / `excludes` entry names, if any.
///
/// The entries are free text; one names a chromophore when it equals the id or one is a prefix of
/// the other (`"H3 / H4"` names `"H3 / H4 (N-V-N, 2N+V; 4N+2V)"`). Entries that name no
/// chromophore (`"oxidising melt"`) are documentation only.
fn named_chromophore<'a>(host: &'a HostData, text: &str) -> Option<&'a ChromophoreData> {
    host.chromophores
        .iter()
        .find(|c| c.id == text || c.id.starts_with(text) || text.starts_with(c.id.as_str()))
}

/// Reports `requires` / `excludes` relations of the catalogue that the recipe violates: an
/// active chromophore whose excluded partner is also active, or whose required partner is absent.
/// Advisory only (the bands are untouched), because the relations are free text in the data.
fn relation_warnings(
    host: &HostData,
    amounts: &BTreeMap<String, f64>,
    treated: &Treated,
) -> Vec<ResolveWarning> {
    let active = |c: &ChromophoreData| {
        c.is_offered() && {
            let c_eff = effective_concentration(host, c, amounts, &treated.split, &treated.created);
            c_eff.is_finite() && c_eff > 1e-12
        }
    };
    let mut out = Vec::new();
    for chromo in host.chromophores.iter().filter(|c| active(c)) {
        for text in &chromo.excludes {
            if let Some(other) = named_chromophore(host, text)
                && other.id > chromo.id
                && active(other)
            {
                out.push(ResolveWarning::Relation {
                    id: chromo.id.clone(),
                    relation: "excludes",
                    other: other.id.clone(),
                });
            }
        }
        for text in &chromo.requires {
            if let Some(other) = named_chromophore(host, text)
                && other.id != chromo.id
                && !active(other)
            {
                out.push(ResolveWarning::Relation {
                    id: chromo.id.clone(),
                    relation: "requires",
                    other: other.id.clone(),
                });
            }
        }
    }
    out
}

/// The recipe amounts keyed by selectable id, each clamped to `[0, conc_max]` of the host
/// (unselectable ids and clamped amounts are reported in `warnings`).
pub(super) fn clamped_amounts(
    catalogue: &ChromophoreCatalogue,
    host: &HostData,
    recipe: &colorRecipe,
    warnings: &mut Vec<ResolveWarning>,
) -> BTreeMap<String, f64> {
    let selectable = catalogue.selectable_elements(&host.id);
    let mut amounts: BTreeMap<String, f64> = BTreeMap::new();
    for e in &recipe.entries {
        if !selectable.contains(&e.id) {
            warnings.push(ResolveWarning::UnknownEntry { id: e.id.clone() });
            continue;
        }
        let max = host.element_conc_max(&e.id);
        let used = e.amount.clamp(0.0, max);
        if used < e.amount {
            warnings.push(ResolveWarning::AmountClamped {
                id: e.id.clone(),
                requested: e.amount,
                used,
            });
        }
        *amounts.entry(e.id.clone()).or_insert(0.0) += used;
    }
    amounts
}

/// Emits the tagged bands of every active chromophore as `(o, e, beta)` lists per host class.
fn collect_bands(
    host: &HostData,
    amounts: &BTreeMap<String, f64>,
    treated: &Treated,
    strength: f64,
) -> (Vec<TaggedBand>, Vec<TaggedBand>, Vec<TaggedBand>) {
    let is_biaxial = host.optical == "biaxial";
    let is_uniaxial = host.optical == "uniaxial";
    let mut raw_o: Vec<TaggedBand> = Vec::new();
    let mut raw_e: Vec<TaggedBand> = Vec::new();
    let mut raw_b: Vec<TaggedBand> = Vec::new();

    for chromo in &host.chromophores {
        if !chromo.is_offered() {
            continue; // suspect or without a usable band
        }
        let c_eff =
            effective_concentration(host, chromo, amounts, &treated.split, &treated.created);
        if !c_eff.is_finite() || c_eff <= 1e-12 {
            continue;
        }
        let chromo_removals = treated.removals.get(&chromo.id);

        for band in chromo.usable_bands() {
            let (Some(_fwhm), Some(peak_coeff)) = (band.fwhm_cm1, band.peak_coeff) else {
                continue;
            };
            let mut factor = 1.0;
            if let Some(rs) = chromo_removals {
                for (bands_nm, f) in rs {
                    if bands_nm.is_empty()
                        || bands_nm.iter().any(|c| (c - band.centre_nm).abs() < 1.0)
                    {
                        factor *= f;
                    }
                }
            }
            // Peak in cm^-1: linear plus the data-driven quadratic (pair-enhanced) term; then
            // cm^-1 to mm^-1 is divide by 10.0.
            let peak_cm =
                peak_coeff.mul_add(c_eff, band.quadratic_coeff.unwrap_or(0.0) * c_eff * c_eff);
            let coeff_mm = (peak_cm * strength * factor / 10.0).min(MAX_PEAK_PER_MM);
            if !coeff_mm.is_finite() || coeff_mm <= 1e-12 {
                continue;
            }

            let weight = |keys: &[&str], default: f64| -> f64 {
                keys.iter()
                    .find_map(|k| band.pol.get(*k).copied())
                    .unwrap_or(default)
                    .max(0.0)
            };
            let make = |w: f64| {
                (
                    AbsorptionBand::energy(
                        band.centre_nm as f32,
                        band.fwhm_cm1.unwrap_or(0.0) as f32,
                        (coeff_mm * w) as f32,
                    ),
                    chromo.id.clone(),
                )
            };

            if is_biaxial {
                let (wa, wb, wg) = (
                    weight(&["a"], 0.0),
                    weight(&["b"], 0.0),
                    weight(&["g"], 0.0),
                );
                if wa > 0.0 {
                    raw_o.push(make(wa));
                }
                if wb > 0.0 {
                    raw_b.push(make(wb));
                }
                if wg > 0.0 {
                    raw_e.push(make(wg));
                }
            } else if is_uniaxial {
                let (wo, we) = (weight(&["o"], 0.0), weight(&["e"], 0.0));
                if wo > 0.0 {
                    raw_o.push(make(wo));
                }
                if we > 0.0 {
                    raw_e.push(make(we));
                }
            } else {
                // Isotropic: the only mode; a missing key is weight 1.
                let wo = weight(&["o"], 1.0);
                if wo > 0.0 {
                    raw_o.push(make(wo));
                }
            }
        }
    }
    (raw_o, raw_e, raw_b)
}

/// Valence split and treatment effects of a recipe, from the host data.
pub(super) struct Treated {
    /// Valence split after `valence_shift` effects.
    pub(super) split: BTreeMap<String, BTreeMap<String, f64>>,
    /// Intensities of centres created by treatments, keyed by chromophore id.
    pub(super) created: BTreeMap<String, f64>,
    /// `remove_centre` multipliers keyed by chromophore id.
    pub(super) removals: BTreeMap<String, Vec<Removal>>,
}

/// Applies the recipe's treatments in catalogue order; skips (with a warning) unknown ones and
/// those whose required elements are absent.
pub(super) fn apply_treatments(
    host: &HostData,
    recipe: &colorRecipe,
    amounts: &BTreeMap<String, f64>,
    warnings: &mut Vec<ResolveWarning>,
) -> Treated {
    let mut split = host.valence_split.clone();
    let mut created: BTreeMap<String, f64> = BTreeMap::new();
    let mut removals: BTreeMap<String, Vec<Removal>> = BTreeMap::new();
    for treatment in &host.treatments {
        if !recipe.treatments.iter().any(|t| t == &treatment.id) {
            continue;
        }
        if let Some(missing) = treatment
            .requires
            .iter()
            .find(|r| amounts.get(r.as_str()).copied().unwrap_or(0.0) <= 0.0)
        {
            warnings.push(ResolveWarning::TreatmentSkipped {
                id: treatment.id.clone(),
                reason: format!("required element {missing} is absent"),
            });
            continue;
        }
        for eff in &treatment.effects {
            match eff.effect_type.as_str() {
                "valence_shift" => {
                    let (Some(el), Some(from), Some(to), Some(frac)) =
                        (&eff.element, &eff.from, &eff.to, eff.fraction)
                    else {
                        continue;
                    };
                    let m = split.entry(el.clone()).or_default();
                    let moved = m.get(from).copied().unwrap_or(0.0) * frac.clamp(0.0, 1.0);
                    *m.entry(from.clone()).or_insert(0.0) -= moved;
                    *m.entry(to.clone()).or_insert(0.0) += moved;
                }
                "create_centre" => {
                    let Some(cid) = &eff.centre_id else { continue };
                    let Some(chromo) = host.chromophores.iter().find(|c| &c.id == cid) else {
                        continue;
                    };
                    let value = created_intensity(host, amounts, eff, chromo);
                    let value = chromo.conc_max.map_or(value, |m| value.min(m));
                    created.insert(cid.clone(), value);
                }
                "remove_centre" => {
                    let (Some(cid), Some(factor)) = (&eff.centre_id, eff.factor) else {
                        continue;
                    };
                    removals
                        .entry(cid.clone())
                        .or_default()
                        .push((eff.bands_nm.clone(), factor.clamp(0.0, 1.0)));
                }
                _ => {}
            }
        }
    }
    for id in &recipe.treatments {
        if !host.treatments.iter().any(|t| &t.id == id) {
            warnings.push(ResolveWarning::TreatmentSkipped {
                id: id.clone(),
                reason: "unknown treatment for this host".to_string(),
            });
        }
    }
    Treated {
        split,
        created,
        removals,
    }
}

/// Intensity of a centre created by `eff`, in the centre's own unit: a fraction of the precursor
/// element's amount (converted between unit bases), or a fraction of `conc_max` without precursor.
fn created_intensity(
    host: &HostData,
    amounts: &BTreeMap<String, f64>,
    eff: &TreatmentEffectData,
    chromo: &ChromophoreData,
) -> f64 {
    let Some(precursor) = &eff.from else {
        return eff.max_intensity.unwrap_or(1.0).clamp(0.0, 1.0) * chromo.conc_max.unwrap_or(1.0);
    };
    let amount = amounts.get(precursor).copied().unwrap_or(0.0);
    let n_in = host
        .element_unit(precursor)
        .map_or(0.0, |u| host.n_site_for_unit(&u));
    let n_centre = host.n_site_for_unit(&chromo.conc_unit);
    if n_centre > 0.0 {
        eff.fraction.unwrap_or(1.0).clamp(0.0, 1.0) * amount * n_in / n_centre
    } else {
        0.0
    }
}

/// Effective concentration of one chromophore in its own `conc_unit`.
pub(super) fn effective_concentration(
    host: &HostData,
    chromo: &ChromophoreData,
    amounts: &BTreeMap<String, f64>,
    split: &BTreeMap<String, BTreeMap<String, f64>>,
    created: &BTreeMap<String, f64>,
) -> f64 {
    let n_own = host.n_site_for_unit(&chromo.conc_unit);
    // Sites per cm^3 of element `el` in species `species` (None: all of it).
    let n_species = |el: &str, species: Option<&str>| -> f64 {
        let amount = amounts.get(el).copied().unwrap_or(0.0);
        let Some(unit) = host.element_unit(el) else {
            return 0.0;
        };
        amount * host.n_site_for_unit(&unit) * HostData::valence_fraction(split, el, species)
    };

    match chromo.kind.as_str() {
        "intrinsic" => 1.0,
        "end_member" => chromo
            .end_member
            .as_ref()
            .and_then(|id| amounts.get(id))
            .copied()
            .unwrap_or(0.0),
        "ivct_pair" => {
            let [pa, pb] = [&chromo.partners[0], &chromo.partners[1]];
            let (ea, eb) = (species_element(pa), species_element(pb));
            // Active only when both elements are present in the recipe.
            if amounts.get(ea).copied().unwrap_or(0.0) <= 0.0
                || amounts.get(eb).copied().unwrap_or(0.0) <= 0.0
            {
                return 0.0;
            }
            let ppm = 1e-6 * host.cation_site_density_cm3;
            if ppm <= 0.0 || n_own <= 0.0 {
                return 0.0;
            }
            let mut c_a = n_species(ea, Some(pa)) / ppm; // ppm of cation sites
            let mut c_b = n_species(eb, Some(pb)) / ppm;
            // Data-driven compensator: the element binds the named partner into an inactive
            // complex, so only `max(0, partner - compensator)` pairs (same basis, via N).
            if let Some(comp) = &chromo.compensator {
                let bound = n_species(&comp.element, None) / ppm;
                if &comp.subtract_from == pa {
                    c_a = (c_a - bound).max(0.0);
                } else if &comp.subtract_from == pb {
                    c_b = (c_b - bound).max(0.0);
                }
            }
            let c_pair_ppm = match chromo.pairing.as_deref() {
                Some("clustered" | "clustered_min") => c_a.min(c_b),
                Some("random" | "product") => chromo.pair_z.unwrap_or(0.0) * c_a * c_b * 1e-6,
                _ => 0.0,
            };
            c_pair_ppm * ppm / n_own
        }
        _ => {
            if let Some(&c) = created.get(&chromo.id) {
                return c;
            }
            if host.is_treatment_created(&chromo.id)
                || chromo.conc_unit == "intensity"
                || n_own <= 0.0
            {
                return 0.0; // only exists once a treatment creates it
            }
            let total: f64 = chromo
                .elements
                .iter()
                .map(|el| n_species(el, chromo.valence.as_deref()))
                .sum();
            total / n_own
        }
    }
}

fn sorted(mut v: Vec<TaggedBand>) -> Vec<TaggedBand> {
    v.sort_by(cmp_tagged);
    v
}

fn strip(v: Vec<TaggedBand>) -> Vec<AbsorptionBand> {
    v.into_iter().map(|(b, _)| b).collect()
}

/// Canonical band order: centre, then width, then chromophore id.
fn cmp_tagged(a: &TaggedBand, b: &TaggedBand) -> std::cmp::Ordering {
    a.0.center_nm
        .total_cmp(&b.0.center_nm)
        .then(a.0.width_nm.total_cmp(&b.0.width_nm))
        .then_with(|| a.1.cmp(&b.1))
}

/// Merges bands with identical centre and width by summing their peaks (the smallest id is kept).
fn merge_bands(bands: Vec<TaggedBand>) -> Vec<TaggedBand> {
    let mut out: Vec<TaggedBand> = Vec::new();
    for (b, id) in bands {
        if let Some((existing, _)) = out.iter_mut().find(|(e, _)| {
            (e.center_nm - b.center_nm).abs() < 0.1
                && (e.width_nm - b.width_nm).abs() < 0.1
                && e.shape == b.shape
        }) {
            existing.peak += b.peak;
        } else {
            out.push((b, id));
        }
    }
    out
}

/// Maximum bands per eigenmode (the GPU material layout).
const MAX_BANDS: usize = 8;

/// `(nu0, sigma, peak)` of an energy-domain band in cm^-1.
pub(super) fn energy_params(b: &AbsorptionBand) -> (f64, f64, f64) {
    (
        1e7 / f64::from(b.center_nm),
        f64::from(b.width_nm),
        f64::from(b.peak),
    )
}

#[expect(
    clippy::suboptimal_flops,
    reason = "moment formulas written as published"
)]
/// The single energy Gaussian that replaces `a` and `b`: area preserving, centred on the
/// area-weighted mean energy, with the variance of the pair (moment matching).
pub(super) fn merged_pair(a: &AbsorptionBand, b: &AbsorptionBand) -> AbsorptionBand {
    let (na, sa, pa) = energy_params(a);
    let (nb, sb, pb) = energy_params(b);
    let (area_a, area_b) = (pa * sa, pb * sb);
    let area = area_a + area_b;
    let nu = (area_a * na + area_b * nb) / area;
    let var =
        (area_a * (sa * sa + (na - nu).powi(2)) + area_b * (sb * sb + (nb - nu).powi(2))) / area;
    let sigma = var.sqrt();
    AbsorptionBand {
        center_nm: (1e7 / nu) as f32,
        width_nm: sigma as f32,
        peak: (area / sigma) as f32,
        shape: a.shape,
    }
}

#[expect(
    clippy::suboptimal_flops,
    clippy::many_single_char_names,
    reason = "short names of the band pair and the wavelength grid"
)]
/// color-weighted squared error (absorbance units over the reference path, weighted by
/// `xbar + ybar + zbar`) of replacing `a` and `b` by `m`, on a 5 nm grid over 380-780 nm.
fn merge_cost(a: &AbsorptionBand, b: &AbsorptionBand, m: &AbsorptionBand, path_mm: f64) -> f64 {
    (0..=80)
        .map(|i| {
            let l = 380.0 + 5.0 * f64::from(i);
            let cmf = cie_1931_cmf(l as f32);
            let w = f64::from(cmf[0] + cmf[1] + cmf[2]);
            let d = f64::from(a.evaluate(l as f32) + b.evaluate(l as f32) - m.evaluate(l as f32))
                * path_mm;
            w * d * d
        })
        .sum()
}

/// Reduces a mode to <= 8 bands. First the two bands whose replacement by one energy Gaussian
/// (moment matching, see [`merged_pair`]) costs the least color error are merged, repeatedly;
/// bands that are not both energy Gaussians are never merged. Whatever still exceeds the budget
/// is dropped (weakest CMF-weighted absorbance first, ties by id). The warnings carry the
/// CIEDE2000 change of the whole step at the reference path under D65.
fn budget_bands(
    mut bands: Vec<TaggedBand>,
    mode_name: &str,
    ref_path_mm: f64,
) -> (Vec<TaggedBand>, Vec<ResolveWarning>) {
    if bands.len() <= MAX_BANDS {
        return (bands, Vec::new());
    }
    let eval = |set: &[TaggedBand], lambda: f64| -> f64 {
        f64::from(
            set.iter()
                .map(|(b, _)| b.evaluate(lambda as f32))
                .sum::<f32>(),
        )
    };
    let original = bands.clone();
    let col_pre = body_color(|l| eval(&original, l), ref_path_mm, Illuminant::D65);

    let mut warnings = Vec::new();
    let mut merged_count = 0usize;
    while bands.len() > MAX_BANDS {
        let mut best: Option<(usize, usize, AbsorptionBand, f64)> = None;
        for i in 0..bands.len() {
            for j in (i + 1)..bands.len() {
                let (a, b) = (&bands[i].0, &bands[j].0);
                if a.shape != b.shape
                    || a.shape != crate::optics::absorption::BandShape::GaussianEnergy
                {
                    continue;
                }
                let m = merged_pair(a, b);
                if !(m.peak.is_finite() && m.width_nm.is_finite() && m.width_nm > 0.0) {
                    continue;
                }
                let cost = merge_cost(a, b, &m, ref_path_mm);
                if best.as_ref().is_none_or(|(_, _, _, c)| cost < *c) {
                    best = Some((i, j, m, cost));
                }
            }
        }
        let Some((i, j, m, _)) = best else { break };
        let id = if bands[i].0.peak >= bands[j].0.peak {
            bands[i].1.clone()
        } else {
            bands[j].1.clone()
        };
        bands.remove(j);
        bands[i] = (m, id);
        merged_count += 1;
    }
    if merged_count > 0 {
        bands.sort_by(cmp_tagged);
        let col_mid = body_color(|l| eval(&bands, l), ref_path_mm, Illuminant::D65);
        warnings.push(ResolveWarning::BandsMerged {
            mode: mode_name.to_string(),
            count: merged_count,
            delta_e: delta_e_2000(col_pre.lab, col_mid.lab),
        });
    }

    if bands.len() > MAX_BANDS {
        let before = bands.clone();
        let col_before = body_color(|l| eval(&before, l), ref_path_mm, Illuminant::D65);
        let score = |b: &AbsorptionBand| {
            let cmf = cie_1931_cmf(b.center_nm);
            b.peak * b.width_nm * (cmf[0] + cmf[1] + cmf[2])
        };
        bands.sort_by(|a, b| {
            score(&b.0)
                .total_cmp(&score(&a.0))
                .then_with(|| a.1.cmp(&b.1))
        });
        let dropped_count = bands.len() - MAX_BANDS;
        bands.truncate(MAX_BANDS);
        bands.sort_by(cmp_tagged);
        let col_post = body_color(|l| eval(&bands, l), ref_path_mm, Illuminant::D65);
        warnings.push(ResolveWarning::BandsDropped {
            mode: mode_name.to_string(),
            count: dropped_count,
            delta_e: delta_e_2000(col_before.lab, col_post.lab),
        });
    }
    (bands, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::body_color::{Illuminant, body_colors};

    fn cat() -> &'static ChromophoreCatalogue {
        ChromophoreCatalogue::global()
    }

    fn peak_near(bands: &[AbsorptionBand], nm: f32) -> Option<f32> {
        bands
            .iter()
            .find(|b| (b.center_nm - nm).abs() < 1.0)
            .map(|b| b.peak)
    }

    /// `(centre nm, peak_coeff)` of the strongest band of `chromophore` in `host` that carries
    /// the pol key `pol` (the catalogue data, not a hard-coded wavelength).
    fn data_band(host: &str, chromophore: &str, pol: &str) -> (f32, f64) {
        let c = cat()
            .host(host)
            .expect("host")
            .chromophores
            .iter()
            .find(|c| c.id == chromophore)
            .expect("chromophore");
        c.usable_bands()
            .filter(|b| b.pol.get(pol).copied().unwrap_or(0.0) > 0.0)
            .max_by(|a, b| a.peak_coeff.partial_cmp(&b.peak_coeff).expect("finite"))
            .map(|b| (b.centre_nm as f32, b.peak_coeff.expect("peak")))
            .expect("band")
    }

    /// Minimal uniaxial host with one random pair, one suspect chromophore and an isotropic twin.
    fn fixture() -> ChromophoreCatalogue {
        let toml = r#"
data_version = 1

[[host]]
id = "toy"
name = "Toy"
formula = "X"
optical = "uniaxial"
materials = ["Toy"]
density_g_cm3 = 4.0
formula_weight_g_mol = 100.0
sites_per_fu = 1.0
cation_site_density_cm3 = 1.0e22
total_atom_density_cm3 = 5.0e22
valence_split = { Fe = { "Fe2+" = 0.5, "Fe3+" = 0.5 } }

  [[host.chromophore]]
  id = "pair"
  kind = "ivct_pair"
  elements = ["Fe", "Ti"]
  conc_unit = "ppm_site"
  partners = ["Fe2+", "Ti4+"]
  pairing = "random"
  pair_z = 6.0
    [[host.chromophore.band]]
    centre_nm = 600.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
    pol = { o = 1.0 }

  [[host.chromophore]]
  id = "hidden"
  kind = "ion"
  elements = ["Cr"]
  conc_unit = "ppm_site"
  suspect = "test"
    [[host.chromophore.band]]
    centre_nm = 500.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
    pol = { o = 1.0, e = 1.0 }

  [[host.chromophore]]
  id = "visible"
  kind = "ion"
  elements = ["Cr"]
  conc_unit = "ppm_site"
  conc_max = 100.0
    [[host.chromophore.band]]
    centre_nm = 450.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
    pol = { o = 1.0, e = 1.0 }
    [[host.chromophore.band]]
    centre_nm = 700.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
    suspect = "band level"
    pol = { o = 1.0, e = 1.0 }
    [[host.chromophore.band]]
    centre_nm = 550.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
    pol = { o = 1.0 }

[[host]]
id = "iso"
name = "Iso"
formula = "Y"
optical = "isotropic"
materials = ["Iso"]
density_g_cm3 = 4.0
formula_weight_g_mol = 100.0
sites_per_fu = 1.0
cation_site_density_cm3 = 1.0e22
total_atom_density_cm3 = 5.0e22

  [[host.chromophore]]
  id = "ion"
  kind = "ion"
  elements = ["Cr"]
  conc_unit = "ppm_site"
    [[host.chromophore.band]]
    centre_nm = 500.0
    fwhm_cm1 = 3000.0
    peak_coeff = 1.0
"#;
        ChromophoreCatalogue::from_toml(toml).expect("fixture loads")
    }

    #[test]
    fn fe_alone_gives_no_fe_ti_band() {
        let mut r = colorRecipe::new("corundum", cat().data_version);
        r.set_amount("Fe", 1000.0);
        let (t, _) = resolve(&r, cat()).expect("resolves");
        assert!(
            peak_near(&t.o_ray, 580.0).is_none(),
            "Fe2+-Ti4+ band without Ti: {:?}",
            t.o_ray
        );
        assert!(peak_near(&t.e_ray, 580.0).is_none());
        // Ti alone is colorless too.
        let mut r = colorRecipe::new("corundum", cat().data_version);
        r.set_amount("Ti", 500.0);
        let (t, _) = resolve(&r, cat()).expect("resolves");
        assert!(t.o_ray.is_empty() && t.e_ray.is_empty(), "{:?}", t.o_ray);
    }

    #[test]
    fn fe_ti_scales_by_the_clustered_pair_law() {
        // Default Fe2+ share of the host data; clustered pair = min(Fe2+, Ti_active) pairs.
        let fe2_share = cat().host("corundum").expect("corundum").valence_split["Fe"]["Fe2+"];
        let (o_nm, o_k) = data_band("corundum", "Fe2+-Ti4+", "o");
        let (e_nm, e_k) = data_band("corundum", "Fe2+-Ti4+", "e");
        let peak = |fe: f64, ti: f64, mg: f64| {
            let mut r = colorRecipe::new("corundum", cat().data_version);
            r.set_amount("Fe", fe);
            r.set_amount("Ti", ti);
            if mg > 0.0 {
                r.set_amount("Mg", mg);
            }
            let (t, _) = resolve(&r, cat()).expect("resolves");
            (
                f64::from(peak_near(&t.o_ray, o_nm).unwrap_or(0.0)),
                f64::from(peak_near(&t.e_ray, e_nm).unwrap_or(0.0)),
            )
        };
        let fe = 1000.0;
        let fe2 = fe * fe2_share;
        let (o50, e50) = peak(fe, 50.0, 0.0);
        assert!((o50 - o_k * 50.0 / 10.0).abs() < 1e-4, "{o50}");
        assert!(
            (e50 / o50 - e_k / o_k).abs() < 1e-4,
            "e/o pair coefficient ratio: {e50}"
        );
        let fe_low = 400.0;
        let (o_cap, _) = peak(fe_low, 450.0, 0.0); // Ti beyond Fe2+: capped by min
        assert!(
            (o_cap - o_k * fe_low * fe2_share / 10.0).abs() < 1e-3,
            "{o_cap}"
        );
        let _ = fe2;
        let (o25, _) = peak(fe, 25.0, 0.0);
        assert!((o25 / o50 - 0.5).abs() < 1e-4);
        // Compensator (data): Mg binds Ti4+, [Ti]_active = max(0, Ti - Mg).
        let (o_mg, _) = peak(fe, 100.0, 30.0);
        assert!(
            (o_mg - o_k * 70.0 / 10.0).abs() < 1e-4,
            "Ti 100 - Mg 30: {o_mg}"
        );
        assert!(
            peak_near(&resolve_o(fe, 100.0, 100.0), o_nm).is_none(),
            "Mg >= Ti: no blue"
        );
        assert!(peak_near(&resolve_o(fe, 100.0, 250.0), o_nm).is_none());
    }

    fn resolve_o(fe: f64, ti: f64, mg: f64) -> Vec<AbsorptionBand> {
        let mut r = colorRecipe::new("corundum", cat().data_version);
        r.set_amount("Fe", fe);
        r.set_amount("Ti", ti);
        r.set_amount("Mg", mg);
        resolve(&r, cat()).expect("resolves").0.o_ray
    }

    #[test]
    fn random_pair_law_is_z_ca_cb_in_ppm_site() {
        let cat = fixture();
        let mut r = colorRecipe::new("toy", cat.data_version);
        r.set_amount("Fe", 100.0); // Fe2+ 50 ppm
        r.set_amount("Ti", 40.0);
        let (t, _) = resolve(&r, &cat).expect("resolves");
        let expect = 6.0 * 50.0 * 40.0 * 1e-6 * 1.0 / 10.0;
        let got = f64::from(peak_near(&t.o_ray, 600.0).expect("pair band"));
        assert!((got - expect).abs() / expect < 1e-4, "{got} vs {expect}");
        // One element alone activates nothing.
        let mut r = colorRecipe::new("toy", cat.data_version);
        r.set_amount("Fe", 100.0);
        let (t, _) = resolve(&r, &cat).expect("resolves");
        assert!(peak_near(&t.o_ray, 600.0).is_none());
    }

    #[test]
    fn suspect_chromophores_and_bands_are_skipped_and_not_offered() {
        let cat = fixture();
        assert_eq!(cat.selectable_elements("toy"), vec!["Cr", "Fe", "Ti"]);
        let mut r = colorRecipe::new("toy", cat.data_version);
        r.set_amount("Cr", 10.0);
        let (t, _) = resolve(&r, &cat).expect("resolves");
        assert!(
            peak_near(&t.o_ray, 500.0).is_none(),
            "chromophore-level suspect used"
        );
        assert!(
            peak_near(&t.o_ray, 700.0).is_none(),
            "band-level suspect used"
        );
        assert!(peak_near(&t.o_ray, 450.0).is_some());
    }

    #[test]
    fn missing_pol_key_is_weight_zero_except_isotropic() {
        let cat = fixture();
        let mut r = colorRecipe::new("toy", cat.data_version);
        r.set_amount("Cr", 10.0);
        let (t, _) = resolve(&r, &cat).expect("resolves");
        assert!(peak_near(&t.o_ray, 550.0).is_some());
        assert!(
            peak_near(&t.e_ray, 550.0).is_none(),
            "pol = {{o}} must give weight 0 in e"
        );
        assert!(peak_near(&t.e_ray, 450.0).is_some());

        let mut r = colorRecipe::new("iso", cat.data_version);
        r.set_amount("Cr", 10.0);
        let (t, _) = resolve(&r, &cat).expect("resolves");
        assert!(
            peak_near(&t.o_ray, 500.0).is_some(),
            "isotropic host without pol keeps weight 1"
        );
    }

    #[test]
    fn non_finite_amounts_are_rejected_and_amounts_clamped() {
        let mut r = colorRecipe::new("corundum", cat().data_version);
        assert!(!r.set_amount("Cr", f64::NAN));
        assert!(!r.set_amount("Cr", f64::INFINITY));
        assert!(r.entries.is_empty());
        r.entries.push(super::super::recipe::RecipeEntry {
            id: "Cr".into(),
            amount: f64::NAN,
        });
        assert!(matches!(
            resolve(&r, cat()),
            Err(ResolveError::NonFinite(_))
        ));
        r.entries.clear();
        r.strength = f64::INFINITY;
        assert!(matches!(
            resolve(&r, cat()),
            Err(ResolveError::NonFinite(_))
        ));

        // 1e300 is clamped to conc_max (2.0 wt% Cr2O3): U-band peak = 2.0 * k / 10 mm^-1.
        let mut r = colorRecipe::new("corundum", cat().data_version);
        r.set_amount("Cr", 1e300);
        let (t, w) = resolve(&r, cat()).expect("resolves");
        let (u_nm, u_k) = data_band("corundum", "Cr3+", "o");
        let p = f64::from(peak_near(&t.o_ray, u_nm).expect("band"));
        assert!((p - 2.0 * u_k / 10.0).abs() < 1e-3, "{p}");
        assert!(
            w.iter()
                .any(|w| matches!(w, ResolveWarning::AmountClamped { .. }))
        );
        assert!(t.o_ray.iter().chain(&t.e_ray).all(|b| b.peak.is_finite()));
        r.set_amount("Cr", -5.0);
        assert_eq!(r.amount("Cr"), 0.0);
    }

    #[test]
    fn deserialising_non_finite_amounts_fails() {
        use super::super::recipe::RecipeEntry;
        assert!(toml::from_str::<RecipeEntry>("id = \"Cr\"\namount = nan").is_err());
        assert!(toml::from_str::<RecipeEntry>("id = \"Cr\"\namount = inf").is_err());
        assert!(toml::from_str::<RecipeEntry>("id = \"Cr\"\namount = 0.5").is_ok());
    }

    #[test]
    fn bands_are_emitted_in_centre_width_id_order() {
        let mut r = colorRecipe::new("corundum", cat().data_version);
        for (el, a) in [("Cr", 0.3), ("Fe", 800.0), ("Ti", 100.0), ("V", 100.0)] {
            r.set_amount(el, a);
        }
        let (t, _) = resolve(&r, cat()).expect("resolves");
        for bands in [&t.o_ray, &t.e_ray] {
            assert!(bands.len() <= 8);
            assert!(
                bands.windows(2).all(|w| w[0].center_nm <= w[1].center_nm),
                "{bands:?}"
            );
        }
    }

    #[test]
    fn treatments_come_from_data_and_reach_every_selectable_element() {
        // Quartz: Fe and Al only color through the gamma-irradiation centres.
        let c = cat();
        assert!(c.selectable_elements("quartz").contains(&"Al".to_string()));
        let mut r = colorRecipe::new("quartz", c.data_version);
        r.set_amount("Al", 50.0);
        let (t, _) = resolve(&r, c).expect("resolves");
        assert!(t.o_ray.is_empty(), "Al hole centre needs irradiation");
        r.treatments.push("gamma_irradiation".to_string());
        let (t, _) = resolve(&r, c).expect("resolves");
        assert!(
            peak_near(&t.o_ray, 470.0).is_some() || !t.o_ray.is_empty(),
            "smoky Al centre: {:?}",
            t.o_ray
        );
        let smoky_peak = t.o_ray.iter().map(|b| b.peak).fold(0.0, f32::max);
        // Fe adds the amethyst centre on top.
        r.set_amount("Fe", 100.0);
        let (t2, _) = resolve(&r, c).expect("resolves");
        assert!(
            t2.o_ray.len() > t.o_ray.len()
                || t2.o_ray.iter().map(|b| b.peak).sum::<f32>() > smoky_peak
        );
        // A treatment whose required element is absent is skipped with a warning.
        let mut r = colorRecipe::new("quartz", c.data_version);
        r.set_amount("Al", 50.0);
        r.treatments.push("thermal_anneal_450".to_string());
        let (_, w) = resolve(&r, c).expect("resolves");
        assert!(
            w.iter()
                .any(|w| matches!(w, ResolveWarning::TreatmentSkipped { .. }))
        );

        // Diamond: nitrogen reaches the C-centre (the other N species are unpopulated).
        let mut r = colorRecipe::new("diamond", c.data_version);
        r.set_amount("N", 100.0);
        let (t, _) = resolve(&r, c).expect("resolves");
        assert!(!t.o_ray.is_empty(), "diamond N must color");

        // Corundum reducing anneal shifts Fe3+ -> Fe2+: the Fe2+-Ti4+ blue gets stronger.
        let host = c.host("corundum").expect("corundum");
        let fe2_default = host.valence_split["Fe"]["Fe2+"];
        let fe2_reduced = (1.0 - fe2_default).mul_add(0.8, fe2_default);
        let (o_nm, _) = data_band("corundum", "Fe2+-Ti4+", "o");
        let blue = |treat: bool| {
            let mut r = colorRecipe::new("corundum", c.data_version);
            r.set_amount("Fe", 200.0);
            r.set_amount("Ti", 300.0);
            if treat {
                r.treatments.push("reducing_anneal".to_string());
            }
            let (t, _) = resolve(&r, c).expect("resolves");
            peak_near(&t.o_ray, o_nm).expect("pair band")
        };
        // Ti 300 exceeds Fe2+ either way, so the pairs follow Fe2+ = 200 x the Fe2+ share.
        assert!((f64::from(blue(true) / blue(false)) - fe2_reduced / fe2_default).abs() < 1e-3);
    }

    #[test]
    fn heated_tanzanite_reduces_coefficient_and_keeps_weights() {
        let c = cat();
        let mk = |heated: bool| {
            let mut r = colorRecipe::new("tanzanite", c.data_version);
            r.set_amount("V", 500.0);
            if heated {
                r.treatments.push("air_anneal_550".to_string());
            }
            resolve(&r, c).expect("resolves").0
        };
        let (raw, heated) = (mk(false), mk(true));
        // The treatment bleaches the brown IVCT bands named in the data (gamma ray = e_ray): the
        // coefficient factor is applied to those bands only, the polarization weights are not.
        let eff = &c.host("tanzanite").expect("tanzanite").treatments[0].effects[0];
        let factor = eff.factor.expect("factor");
        assert!(!eff.bands_nm.is_empty());
        let brown = eff.bands_nm[0] as f32;
        let raw_brown = peak_near(&raw.e_ray, brown).expect("brown band in the unheated gamma ray");
        match peak_near(&heated.e_ray, brown) {
            Some(h) => assert!((h / raw_brown - factor as f32).abs() < 1e-4),
            None => assert!(factor == 0.0, "a zero factor removes the band"),
        }
        // The V3+ bands are untouched.
        let (v_nm, _) = data_band("tanzanite", "V3+ (natural unheated, trichroic)", "b");
        let beta = |t: &AbsorptionTensor| peak_near(t.beta_ray.as_ref().expect("beta"), v_nm);
        assert!(beta(&raw).is_some());
        assert_eq!(beta(&heated), beta(&raw));
        // Biaxial pol keys a/b/g are honoured: the beta ray exists.
        assert!(raw.beta_ray.is_some());
    }

    #[test]
    fn garnet_end_members_color_with_a_colorless_remainder() {
        let c = cat();
        let sel = c.selectable_elements("garnet_pyralspite");
        assert!(sel.contains(&"almandine".to_string()) && sel.contains(&"spessartine".to_string()));
        assert!(
            !sel.contains(&"pyrope".to_string()),
            "the remainder is not selectable"
        );

        let mut r = colorRecipe::new("garnet_pyralspite", c.data_version);
        r.set_amount("almandine", 0.3);
        let (t, _) = resolve(&r, c).expect("resolves");
        assert!(!t.o_ray.is_empty());
        let lab = body_colors(&t, 5.0, Illuminant::D65).unpolarised.lab;
        assert!(lab[0] < 90.0, "almandine must absorb: {lab:?}");

        let fr = c.end_member_fractions(&r).expect("fractions");
        let sum: f64 = fr.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-12);
        assert!(
            fr.iter()
                .any(|(id, x)| id == "pyrope" && (x - 0.7).abs() < 1e-12)
        );

        r.set_amount("spessartine", 0.8);
        assert!(matches!(
            resolve(&r, c),
            Err(ResolveError::EndMemberSumExceeded(_))
        ));

        // RI/SG interpolate linearly: pure pyrope, 50/50 pyrope-almandine.
        let (ri, sg) = super::super::garnet_optics(&[]);
        assert!((ri - 1.714).abs() < 1e-12 && (sg - 3.58).abs() < 1e-12);
        let (ri, sg) = super::super::garnet_optics(&[("almandine", 0.5)]);
        assert!((ri - 1.772).abs() < 1e-12);
        assert!((sg - 3.885).abs() < 1e-12);
        let (ri, _) = c
            .recipe_optics(&{
                let mut r = colorRecipe::new("garnet_pyralspite", c.data_version);
                r.set_amount("almandine", 0.5);
                r
            })
            .expect("garnet optics");
        assert!((ri - 1.772).abs() < 1e-9);
    }

    #[test]
    fn corundum_ppma_basis_is_restored() {
        let c = cat();
        let host = c.host("corundum").expect("corundum");
        for id in ["Fe3+", "V3+", "Ti3+"] {
            let ch = host
                .chromophores
                .iter()
                .find(|ch| ch.id == id)
                .expect("chromophore");
            assert_eq!(ch.conc_unit, "ppma_all", "{id}");
            assert!(
                ch.bands.iter().all(|b| b.suspect.is_none()),
                "{id} must pass the 2x sigma check"
            );
        }
        // 1000 ppm_site of V = 400 ppma_all: the V3+ visible band is k * 400 / 10 mm^-1.
        let (v_nm, v_k) = data_band("corundum", "V3+", "o");
        let mut r = colorRecipe::new("corundum", c.data_version);
        r.set_amount("V", 1000.0);
        let (t, _) = resolve(&r, c).expect("resolves");
        let p = f64::from(peak_near(&t.o_ray, v_nm).expect("band"));
        assert!((p - v_k * 400.0 / 10.0).abs() / p < 1e-3, "{p}");
    }

    #[test]
    fn violated_requires_and_excludes_are_reported_without_changing_the_bands() {
        let c = cat();
        let mut both = colorRecipe::new("diamond", c.data_version);
        both.set_amount("N", 200.0);
        both.set_amount("B", 0.5);
        let (tensor, warnings) = resolve(&both, c).expect("resolves");
        assert!(
            warnings.iter().any(|w| matches!(
                w,
                ResolveWarning::Relation { relation: "excludes", other, .. }
                    if other.starts_with("C centre")
            )),
            "{warnings:?}"
        );
        assert!(!tensor.o_ray.is_empty());
        // Nitrogen alone violates nothing.
        let mut n_only = colorRecipe::new("diamond", c.data_version);
        n_only.set_amount("N", 200.0);
        let (_, warnings) = resolve(&n_only, c).expect("resolves");
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w, ResolveWarning::Relation { .. })),
            "{warnings:?}"
        );
    }
}

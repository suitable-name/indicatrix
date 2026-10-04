//! Chromophore catalogue data structures, validation, and consistency checks.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};

/// Global chromophore catalogue instance.
static CATALOGUE: OnceLock<ChromophoreCatalogue> = OnceLock::new();

/// The root chromophore catalogue holding all gemstone hosts and their chromophore data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromophoreCatalogue {
    /// Data schema version.
    pub data_version: u32,
    /// List of host crystal / glass definitions.
    #[serde(rename = "host")]
    pub hosts: Vec<HostData>,
}

impl ChromophoreCatalogue {
    /// Returns the global singleton instance parsed from the embedded `chromophores.toml`.
    ///
    /// # Panics
    ///
    /// Panics if the embedded catalogue fails to parse or validate (a build-time data error
    /// caught by the unit tests).
    #[must_use]
    pub fn global() -> &'static Self {
        CATALOGUE.get_or_init(|| {
            let toml_src = include_str!("../../../data/chromophores.toml");
            Self::from_toml(toml_src).expect("canonical chromophores.toml must parse and validate")
        })
    }

    /// Parses and validates a catalogue from TOML source text.
    ///
    /// Validation also performs the load-time band normalisation: `lorentzian` and `edge`
    /// bands are converted into the renderer's Gaussian-in-energy shape (see
    /// [`BandData::convert_shape`]).
    ///
    /// # Errors
    ///
    /// Returns an error string if TOML parsing fails or schema validation fails.
    pub fn from_toml(src: &str) -> Result<Self, String> {
        let mut cat: Self = toml::from_str(src).map_err(|e| format!("TOML parse error: {e}"))?;
        cat.validate_and_normalize()?;
        Ok(cat)
    }

    /// Finds a host by its stable lowercase identifier (e.g. `"corundum"`).
    #[must_use]
    pub fn host(&self, id: &str) -> Option<&HostData> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Finds a host by its display name (e.g. `"Corundum"`).
    #[must_use]
    pub fn host_by_name(&self, name: &str) -> Option<&HostData> {
        self.hosts.iter().find(|h| h.name == name)
    }

    /// Finds the host corresponding to a built-in material name (e.g. `"Ruby"` -> Corundum).
    #[must_use]
    pub fn host_for_material(&self, material_name: &str) -> Option<&HostData> {
        self.hosts.iter().find(|h| {
            h.materials
                .iter()
                .any(|m| m.eq_ignore_ascii_case(material_name))
        })
    }

    /// Finds an end member by its id in any host (garnet end members appear in several hosts
    /// with identical data; the first in canonical host order wins).
    #[must_use]
    pub fn end_member(&self, id: &str) -> Option<&EndMemberData> {
        self.hosts
            .iter()
            .flat_map(|h| h.end_members.iter())
            .find(|m| m.id == id)
    }

    /// Returns the unique selectable ids offered for `host_id`, sorted.
    ///
    /// An id is an element symbol (`"Cr"`, `"Fe"`, ...) for ion, pair, colloid and color-centre
    /// chromophores, or an end-member id (`"almandine"`) for `end_member` chromophores. Suspect
    /// chromophores/bands are not offered. `intensity`-unit centres have no element basis and
    /// contribute no element. The recipe keys its entries by exactly these ids.
    #[must_use]
    pub fn selectable_elements(&self, host_id: &str) -> Vec<String> {
        let Some(host) = self.host(host_id) else {
            return Vec::new();
        };

        let mut ids: BTreeSet<String> = BTreeSet::new();
        for chromo in &host.chromophores {
            if !chromo.is_offered() {
                continue;
            }
            if chromo.kind == "end_member" {
                if let Some(em) = &chromo.end_member {
                    ids.insert(em.clone());
                }
                continue;
            }
            if chromo.kind == "ivct_pair" {
                for p in &chromo.partners {
                    ids.insert(species_element(p).to_string());
                }
                if let Some(comp) = &chromo.compensator {
                    ids.insert(comp.element.clone());
                }
                continue;
            }
            if chromo.conc_unit == "intensity" {
                continue;
            }
            for el in &chromo.elements {
                ids.insert(el.clone());
            }
        }
        ids.into_iter().collect()
    }

    /// Treatments of `host_id` whose required elements are all in `present`.
    #[must_use]
    pub fn selectable_treatments(&self, host_id: &str, present: &[&str]) -> Vec<&TreatmentData> {
        let Some(host) = self.host(host_id) else {
            return Vec::new();
        };
        host.treatments
            .iter()
            .filter(|t| t.requires.iter().all(|r| present.contains(&r.as_str())))
            .collect()
    }

    /// End-member fractions of a recipe for hosts with end members: the recipe's end-member
    /// entries plus the colorless remainder, so the fractions sum to exactly 1.
    ///
    /// Returns `None` for hosts without end members, or when the entries sum to more than 1.
    #[must_use]
    pub fn end_member_fractions(
        &self,
        recipe: &super::recipe::colorRecipe,
    ) -> Option<Vec<(String, f64)>> {
        let host = self.host(&recipe.host)?;
        host.end_member_fractions(|id| recipe.amount(id))
    }

    /// Linearly interpolated `(ri, sg)` for a recipe on a host with end members.
    #[must_use]
    pub fn recipe_optics(&self, recipe: &super::recipe::colorRecipe) -> Option<(f64, f64)> {
        let fractions = self.end_member_fractions(recipe)?;
        let refs: Vec<(&str, f64)> = fractions.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        Some(garnet_optics(&refs))
    }

    /// Validates all fields, sorts entries canonically, and performs consistency checks.
    fn validate_and_normalize(&mut self) -> Result<(), String> {
        let mut host_ids = BTreeSet::new();
        for host in &self.hosts {
            if !host_ids.insert(host.id.clone()) {
                return Err(format!("Duplicate host id: {}", host.id));
            }
        }

        // Sort hosts by ID for canonical order
        self.hosts.sort_by(|a, b| a.id.cmp(&b.id));

        for host in &mut self.hosts {
            host.validate_and_normalize()?;
        }

        Ok(())
    }
}

/// Leading element symbol of a species label (`"Fe2+"` -> `"Fe"`, `"Ti4+"` -> `"Ti"`).
#[must_use]
pub fn species_element(species: &str) -> &str {
    let end = species
        .char_indices()
        .find(|(_, c)| !c.is_ascii_alphabetic())
        .map_or(species.len(), |(i, _)| i);
    &species[..end]
}

/// Linearly interpolated refractive index and specific gravity over garnet (or any) end-member
/// fractions, using the end-member data of the global catalogue.
///
/// `fractions` are `(end_member_id, mole_fraction)` pairs. Unknown ids are ignored; negative or
/// non-finite fractions count as 0. If the sum is below 1 the remainder is pyrope (the
/// colorless garnet remainder); if it exceeds 1 the fractions are renormalised. An empty input
/// is pure pyrope.
#[must_use]
pub fn garnet_optics(fractions: &[(&str, f64)]) -> (f64, f64) {
    let cat = ChromophoreCatalogue::global();
    let mut used: Vec<(&EndMemberData, f64)> = Vec::new();
    let mut sum = 0.0;
    for (id, x) in fractions {
        let x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        if x <= 0.0 {
            continue;
        }
        if let Some(em) = cat.end_member(id) {
            used.push((em, x));
            sum += x;
        }
    }
    if sum < 1.0
        && let Some(pyrope) = cat.end_member("pyrope")
    {
        used.push((pyrope, 1.0 - sum));
        sum = 1.0;
    }
    if used.is_empty() || sum <= 0.0 {
        return (1.714, 3.58);
    }
    let (ri, sg) = used
        .iter()
        .fold((0.0, 0.0), |(r, s), (em, x)| (r + em.ri * x, s + em.sg * x));
    (ri / sum, sg / sum)
}

/// Fixed list of concentration unit prefixes/names (spec section 2.2).
const FIXED_UNITS: [&str; 4] = ["ppm_site", "ppma_all", "mol_fraction", "intensity"];

/// Oxide table: name, molar mass in g/mol, cations per formula unit.
const OXIDES: [(&str, f64, f64); 15] = [
    ("Cr2O3", 151.99, 2.0),
    ("Fe2O3", 159.69, 2.0),
    ("FeO", 71.84, 1.0),
    ("V2O3", 149.88, 2.0),
    ("MnO", 70.94, 1.0),
    ("Mn2O3", 157.90, 2.0),
    ("CoO", 74.93, 1.0),
    ("CuO", 79.55, 1.0),
    ("NiO", 74.69, 1.0),
    ("Nd2O3", 336.48, 2.0),
    ("Er2O3", 382.56, 2.0),
    ("CeO2", 172.11, 1.0),
    ("Pr6O11", 1021.44, 6.0),
    ("TiO2", 79.87, 1.0),
    ("UO3", 286.03, 1.0),
];

/// Returns `(molar mass, cations per formula unit)` for a known oxide name.
fn oxide_data(name: &str) -> Option<(f64, f64)> {
    OXIDES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|&(_, m, k)| (m, k))
}

/// Whether `unit` is in the spec's fixed unit list (`wt_pct_oxide:<known oxide>` included).
#[must_use]
pub fn unit_is_valid(unit: &str) -> bool {
    if let Some(ox) = unit.strip_prefix("wt_pct_oxide:") {
        return oxide_data(ox).is_some();
    }
    FIXED_UNITS.contains(&unit)
}

/// Default FWHM in cm^-1 given to an `edge` band that carries none (a soft absorption edge of
/// about 0.37 eV).
pub const EDGE_DEFAULT_FWHM_CM1: f64 = 3000.0;

/// Chromophore kinds the validator accepts.
const KINDS: [&str; 6] = [
    "ion",
    "ivct_pair",
    "end_member",
    "color_centre",
    "colloid",
    "intrinsic",
];

/// Data for a host crystal or glass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostData {
    /// Stable lowercase identifier (e.g. `"corundum"`).
    pub id: String,
    /// Display name (e.g. `"Corundum"`).
    pub name: String,
    /// Chemical formula (e.g. `"Al2O3"`).
    pub formula: String,
    /// Optical class: `"isotropic"`, `"uniaxial"`, or `"biaxial"`.
    pub optical: String,
    /// Axis mapping confidence for biaxial hosts (`"verified"`, `"secondary"`, or `"unknown"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_mapping_confidence: Option<String>,
    /// Biaxial hosts: the crystallographic axis length in angstrom along which each ray vibrates,
    /// keyed by the band `pol` key (`a` = alpha, `b` = beta, `g` = gamma, i.e. smallest to
    /// largest refractive index). The mapping is stored by axis length, not by the a/b/c letter,
    /// because the letters differ with the cell setting (Pnma, Pbnm, Pmnb) while the physics does
    /// not: chrysoberyl alpha 4.43, beta 9.40, gamma 5.48 angstrom; zoisite alpha 5.55,
    /// beta 10.0, gamma 16.2 angstrom; forsterite alpha 10.2, beta 6.0, gamma 4.76 angstrom.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ray_axis_angstrom: BTreeMap<String, f64>,
    /// Intrinsic UV cutoff edge wavelength in nm, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv_edge_nm: Option<f64>,
    /// Built-in gemstone material names that map to this host.
    pub materials: Vec<String>,
    /// Density in g/cm^3.
    pub density_g_cm3: f64,
    /// Formula unit molecular weight in g/mol.
    pub formula_weight_g_mol: f64,
    /// Number of cation sites per formula unit.
    pub sites_per_fu: f64,
    /// Cation site density in cm^-3.
    pub cation_site_density_cm3: f64,
    /// Total atom density in cm^-3.
    pub total_atom_density_cm3: f64,
    /// Host-level compositional or spectroscopic rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    /// Per-host valence split: element -> species label (e.g. `"Fe2+"`) -> fraction of that
    /// element in the species. An element with an entry only populates the listed species; an
    /// element without one is not split (every chromophore sees the full amount).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub valence_split: BTreeMap<String, BTreeMap<String, f64>>,
    /// Optional explicit input unit per element (otherwise derived from the host's chromophores).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub element_units: BTreeMap<String, String>,
    /// Solid-solution end members (garnet, olivine): sum of fractions is exactly 1 with a
    /// colorless remainder.
    #[serde(default, rename = "end_member", skip_serializing_if = "Vec::is_empty")]
    pub end_members: Vec<EndMemberData>,
    /// Treatments that can act on this host's recipe.
    #[serde(default, rename = "treatment", skip_serializing_if = "Vec::is_empty")]
    pub treatments: Vec<TreatmentData>,
    /// Chromophore definitions for this host.
    #[serde(default, rename = "chromophore", skip_serializing_if = "Vec::is_empty")]
    pub chromophores: Vec<ChromophoreData>,
}

impl HostData {
    /// Calculates the number density of absorbing sites in cm^-3 for one concentration unit.
    ///
    /// Returns 0.0 for an unknown unit (the validator rejects those at load time) and 1.0 for the
    /// dimensionless `intensity` unit.
    #[must_use]
    pub fn n_site_for_unit(&self, unit: &str) -> f64 {
        const N_AVOGADRO: f64 = 6.022_140_76e23;

        if let Some(oxide) = unit.strip_prefix("wt_pct_oxide:") {
            let Some((m_ox, num_cations)) = oxide_data(oxide) else {
                return 0.0;
            };
            let mass_ox_per_cm3 = 0.01 * self.density_g_cm3;
            (num_cations * mass_ox_per_cm3 / m_ox) * N_AVOGADRO
        } else {
            match unit {
                "ppma_all" => 1e-6 * self.total_atom_density_cm3,
                "mol_fraction" => self.cation_site_density_cm3,
                "ppm_site" => 1e-6 * self.cation_site_density_cm3,
                "intensity" => 1.0,
                _ => 0.0,
            }
        }
    }

    /// Whether a treatment of this host creates (or removes) the chromophore `chromo_id`.
    #[must_use]
    pub fn is_treatment_created(&self, chromo_id: &str) -> bool {
        self.treatments.iter().any(|t| {
            t.effects.iter().any(|e| {
                e.effect_type == "create_centre" && e.centre_id.as_deref() == Some(chromo_id)
            })
        })
    }

    /// Input unit of an element amount in a recipe for this host.
    ///
    /// Explicit `element_units` entry first; otherwise the unit of the first (canonical order)
    /// offered chromophore that is driven by the element, preferring plain ions over pairs and
    /// centres, and never `intensity`.
    #[must_use]
    pub fn element_unit(&self, element: &str) -> Option<String> {
        if let Some(u) = self.element_units.get(element) {
            return Some(u.clone());
        }
        for rank in ["ion", "colloid", "color_centre", "ivct_pair"] {
            for c in &self.chromophores {
                if c.kind == rank
                    && c.conc_unit != "intensity"
                    && c.is_offered()
                    && c.elements.iter().any(|e| e == element)
                {
                    return Some(c.conc_unit.clone());
                }
            }
        }
        None
    }

    /// Fraction of `element` in the species `species` (`None` species or an unsplit element
    /// give 1.0; a split element not listing the species gives 0.0).
    #[must_use]
    pub fn valence_fraction(
        split: &BTreeMap<String, BTreeMap<String, f64>>,
        element: &str,
        species: Option<&str>,
    ) -> f64 {
        let Some(species) = species else {
            return 1.0;
        };
        split
            .get(element)
            .map_or(1.0, |m| m.get(species).copied().unwrap_or(0.0))
    }

    /// Upper bound of a selectable id's amount in its input unit.
    ///
    /// End members: 1.0. Elements: the largest `conc_max` of the offered chromophores driven by the
    /// element, converted into the element's input unit; hosts without any `conc_max` fall back to
    /// full occupation of the cation sites.
    #[must_use]
    pub fn element_conc_max(&self, id: &str) -> f64 {
        if self.end_members.iter().any(|m| m.id == id) {
            return 1.0;
        }
        let Some(unit) = self.element_unit(id) else {
            return 0.0;
        };
        let n_in = self.n_site_for_unit(&unit);
        if n_in <= 0.0 {
            return 0.0;
        }
        let mut best: f64 = 0.0;
        for c in &self.chromophores {
            if c.is_offered()
                && c.compensator.as_ref().is_some_and(|k| k.element == id)
                && let Some(m) = c.conc_max
            {
                // A compensator can cancel at most the whole partner it subtracts from.
                best = best.max(m * self.n_site_for_unit(&c.conc_unit) / n_in);
            }
            if !c.is_offered() || c.conc_unit == "intensity" || !c.elements.iter().any(|e| e == id)
            {
                continue;
            }
            if let Some(m) = c.conc_max {
                best = best.max(m * self.n_site_for_unit(&c.conc_unit) / n_in);
            }
        }
        let site_bound = self.cation_site_density_cm3 / n_in;
        if best > 0.0 {
            best.min(site_bound)
        } else {
            site_bound
        }
    }

    /// End-member fractions with the colorless remainder, `None` if the host has no end members
    /// or the entries sum to more than 1 (tolerance 1e-9).
    #[must_use]
    pub fn end_member_fractions(&self, amount: impl Fn(&str) -> f64) -> Option<Vec<(String, f64)>> {
        if self.end_members.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let mut sum = 0.0;
        let mut remainder_id = None;
        for m in &self.end_members {
            if m.colorless {
                remainder_id = Some(m.id.clone());
                continue;
            }
            let x = amount(&m.id);
            let x = if x.is_finite() {
                x.clamp(0.0, 1.0)
            } else {
                0.0
            };
            sum += x;
            out.push((m.id.clone(), x));
        }
        if sum > 1.0 + 1e-9 {
            return None;
        }
        if let Some(r) = remainder_id {
            out.push((r, (1.0 - sum).max(0.0)));
        }
        Some(out)
    }

    fn validate_and_normalize(&mut self) -> Result<(), String> {
        let host_id = self.id.clone();
        if !matches!(self.optical.as_str(), "isotropic" | "uniaxial" | "biaxial") {
            return Err(format!(
                "Host {host_id}: unknown optical class {}",
                self.optical
            ));
        }

        let mut chromo_ids = BTreeSet::new();
        for chromo in &self.chromophores {
            if !chromo_ids.insert(chromo.id.clone()) {
                return Err(format!(
                    "Duplicate chromophore id {} in host {host_id}",
                    chromo.id
                ));
            }
        }

        // Ray axis lengths: biaxial hosts only, keys a/b/g, positive finite lengths.
        if !self.ray_axis_angstrom.is_empty() {
            if self.optical != "biaxial" {
                return Err(format!(
                    "Host {host_id}: ray_axis_angstrom is for biaxial hosts only"
                ));
            }
            if self
                .ray_axis_angstrom
                .iter()
                .any(|(k, v)| !matches!(k.as_str(), "a" | "b" | "g") || !v.is_finite() || *v <= 0.0)
            {
                return Err(format!("Host {host_id}: invalid ray_axis_angstrom"));
            }
        }

        // Valence splits: fractions in [0, 1], total at most 1.
        for (el, split) in &self.valence_split {
            let total: f64 = split.values().sum();
            if split
                .values()
                .any(|f| !f.is_finite() || !(0.0..=1.0).contains(f))
                || total > 1.0 + 1e-9
            {
                return Err(format!("Host {host_id}: invalid valence split for {el}"));
            }
        }
        for (el, unit) in &self.element_units {
            if !unit_is_valid(unit) || unit == "intensity" {
                return Err(format!(
                    "Host {host_id}: invalid element unit {unit} for {el}"
                ));
            }
        }

        // End members: unique ids, exactly one colorless remainder.
        if !self.end_members.is_empty() {
            let mut ids = BTreeSet::new();
            for m in &self.end_members {
                if !ids.insert(m.id.clone()) {
                    return Err(format!("Host {host_id}: duplicate end member {}", m.id));
                }
                if !m.ri.is_finite() || !m.sg.is_finite() || m.ri <= 1.0 || m.sg <= 0.0 {
                    return Err(format!(
                        "Host {host_id}: end member {} has invalid ri/sg",
                        m.id
                    ));
                }
            }
            if self.end_members.iter().filter(|m| m.colorless).count() != 1 {
                return Err(format!(
                    "Host {host_id}: exactly one colorless end member required"
                ));
            }
            self.end_members.sort_by(|a, b| a.id.cmp(&b.id));
        }

        // Sort chromophores by ID
        let mut chromophores = std::mem::take(&mut self.chromophores);
        chromophores.sort_by(|a, b| a.id.cmp(&b.id));

        let optical = self.optical.clone();
        for chromo in &mut chromophores {
            chromo.validate_and_normalize(self, &optical)?;
        }
        self.chromophores = chromophores;

        self.validate_treatments()
    }

    /// Treatments reference real chromophores and carry complete effects.
    fn validate_treatments(&self) -> Result<(), String> {
        let host_id = &self.id;
        for t in &self.treatments {
            for eff in &t.effects {
                let known_chromo = |id: &Option<String>| {
                    id.as_ref()
                        .is_some_and(|id| self.chromophores.iter().any(|c| &c.id == id))
                };
                match eff.effect_type.as_str() {
                    "valence_shift" => {
                        if eff.element.is_none()
                            || eff.from.is_none()
                            || eff.to.is_none()
                            || eff.fraction.is_none()
                        {
                            return Err(format!(
                                "Host {host_id}: treatment {} valence_shift needs element, from, to, fraction",
                                t.id
                            ));
                        }
                    }
                    "create_centre" | "remove_centre" => {
                        if !known_chromo(&eff.centre_id) {
                            return Err(format!(
                                "Host {host_id}: treatment {} targets unknown chromophore {:?}",
                                t.id, eff.centre_id
                            ));
                        }
                        if eff.effect_type == "remove_centre" && eff.factor.is_none() {
                            return Err(format!(
                                "Host {host_id}: treatment {} remove_centre needs factor",
                                t.id
                            ));
                        }
                    }
                    other => {
                        return Err(format!(
                            "Host {host_id}: treatment {} unknown effect {other}",
                            t.id
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A solid-solution end member (garnet pyrope, almandine, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndMemberData {
    /// Stable id used as the recipe entry id (`"almandine"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Refractive index (sodium D) of the pure end member.
    pub ri: f64,
    /// Specific gravity of the pure end member.
    pub sg: f64,
    /// Whether this is the colorless remainder end member (exactly one per host).
    #[serde(default)]
    pub colorless: bool,
}

/// A treatment that modifies the recipe or valence state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreatmentData {
    /// Stable identifier (e.g. `"reducing_anneal"`).
    pub id: String,
    /// Human-readable title.
    pub name: String,
    /// Physical conditions (e.g. `"1400°C, H2"`).
    pub conditions: String,
    /// Required precursor elements (e.g. `["Fe", "Ti"]`); the treatment does nothing, with a
    /// warning, unless all are present in the recipe.
    #[serde(default)]
    pub requires: Vec<String>,
    /// List of effects produced by this treatment.
    #[serde(default, rename = "effect")]
    pub effects: Vec<TreatmentEffectData>,
}

/// A specific effect produced by a treatment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreatmentEffectData {
    /// Effect type: `"valence_shift"`, `"create_centre"`, or `"remove_centre"`.
    #[serde(rename = "type")]
    pub effect_type: String,
    /// Element whose valence split is shifted (`valence_shift` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// `valence_shift`: source species label (`"Fe3+"`). `create_centre`: precursor element
    /// whose amount bounds the centre (absent: no precursor, intensity is `max_intensity` of the
    /// centre's `conc_max`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Target species label for `valence_shift`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// `valence_shift`: fraction of the source species converted. `create_centre` with a
    /// precursor: fraction of the precursor converted into the centre (default 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// Id of the target chromophore (`create_centre`, `remove_centre`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centre_id: Option<String>,
    /// `create_centre` without precursor: centre intensity as a fraction of its `conc_max`
    /// (1.0 if the chromophore has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_intensity: Option<f64>,
    /// Multiplier on the target's coefficient for `remove_centre`; polarization weights are
    /// untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factor: Option<f64>,
    /// `remove_centre`: restrict the factor to bands with these centres in nm (1 nm tolerance);
    /// empty means every band of the target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bands_nm: Vec<f64>,
}

/// A compensating element of an `ivct_pair`: the partner species it is subtracted from.
///
/// The active partner concentration is `max(0, c_partner - c_compensator)`, both converted to
/// the same basis (sites per cm^3) through the host's number densities. The subtract rule is
/// the only one defined (`rule = "subtract"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompensatorData {
    /// Compensating element symbol (`"Mg"`).
    pub element: String,
    /// Partner species label it is subtracted from (`"Ti4+"`).
    pub subtract_from: String,
    /// Rule name; only `"subtract"` exists.
    #[serde(default = "default_compensator_rule")]
    pub rule: String,
}

fn default_compensator_rule() -> String {
    "subtract".to_string()
}

/// Definition of an absorbing chromophore in a host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromophoreData {
    /// Stable identifier (e.g. `"Cr3+"`, `"Fe2+-Ti4+"`).
    pub id: String,
    /// Classification: `"ion"`, `"ivct_pair"`, `"end_member"`, `"color_centre"`, `"colloid"` or
    /// `"intrinsic"` (host matrix, always on).
    pub kind: String,
    /// Elements that constitute or drive this chromophore.
    pub elements: Vec<String>,
    /// Coordination / site description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Explicit concentration unit (e.g. `"wt_pct_oxide:Cr2O3"`, `"ppm_site"`).
    pub conc_unit: String,
    /// Typical concentration range `[min, max]` in `conc_unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conc_typical: Option<[f64; 2]>,
    /// Maximum plausible concentration in `conc_unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conc_max: Option<f64>,
    /// Species label (`"Fe3+"`) of the single element this chromophore represents; selects the
    /// fraction from the host's `valence_split`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valence: Option<String>,
    /// `ivct_pair`: the two partner species (`["Fe2+", "Ti4+"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partners: Vec<String>,
    /// `ivct_pair` with `random` pairing: number of qualifying neighbour sites z.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_z: Option<f64>,
    /// `end_member`: id of the host end member that supplies the mole fraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_member: Option<String>,
    /// Pairing law for IVCT pairs, selectable per pair in the data: `"clustered_min"` (alias
    /// `"clustered"`): `c_pair = min(c_A, c_B)`, linear in the scarcer partner; `"product"`
    /// (alias `"random"`): `c_pair = pair_z * c_A * c_B * 1e-6` in `ppm_site`. With a
    /// `compensator` the compensated partner enters as `max(0, partner - compensator)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<String>,
    /// Confidence of pairing law.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_confidence: Option<String>,
    /// Empirical quadratic coefficient for reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_coeff_empirical_per_ppm2: Option<f64>,
    /// `ivct_pair`: an element that binds one partner into an inactive complex (corundum Mg
    /// binds Ti4+ into inactive Mg-Ti). Data-driven, never hard-coded in the resolver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensator: Option<CompensatorData>,
    /// Other chromophores required for this one to absorb.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Other chromophores that mutually exclude this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excludes: Vec<String>,
    /// Chromophores that quench this chromophore's luminescence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quenched_by: Vec<String>,
    /// Confidence level: `"measured"`, `"verified"`, `"secondary"`, `"estimate"`, or `"unknown"`.
    #[serde(default = "default_confidence")]
    pub confidence: String,
    /// Reason string if data is suspect; the whole chromophore is then hidden and skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspect: Option<String>,
    /// Absorption bands.
    #[serde(default, rename = "band")]
    pub bands: Vec<BandData>,
    /// Luminescence / fluorescence entries.
    #[serde(
        default,
        rename = "fluorescence",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub fluorescence: Vec<FluorescenceData>,
}

fn default_confidence() -> String {
    "secondary".to_string()
}

impl ChromophoreData {
    /// Bands the forward model may use: not suspect, with centre, width and peak coefficient.
    pub fn usable_bands(&self) -> impl Iterator<Item = &BandData> {
        let hidden = self.suspect.is_some();
        self.bands.iter().filter(move |b| {
            !hidden && b.suspect.is_none() && b.peak_coeff.is_some() && b.fwhm_cm1.is_some()
        })
    }

    /// Whether this chromophore is offered: not suspect and at least one usable band.
    #[must_use]
    pub fn is_offered(&self) -> bool {
        self.usable_bands().next().is_some()
    }

    fn validate_and_normalize(&mut self, host: &HostData, optical: &str) -> Result<(), String> {
        let hid = &host.id;
        let cid = self.id.clone();
        if !KINDS.contains(&self.kind.as_str()) {
            return Err(format!("{hid}/{cid}: unknown kind {}", self.kind));
        }
        if !unit_is_valid(&self.conc_unit) {
            return Err(format!(
                "{hid}/{cid}: unit {:?} is not in the fixed unit list",
                self.conc_unit
            ));
        }
        if let (Some(t), Some(m)) = (self.conc_typical, self.conc_max)
            && !(t[0] >= 0.0 && t[0] <= t[1] && t[1] <= m)
        {
            return Err(format!(
                "{hid}/{cid}: conc_typical must lie in [0, conc_max]"
            ));
        }
        if let Some(m) = self.conc_max
            && (!m.is_finite() || m < 0.0)
        {
            return Err(format!("{hid}/{cid}: conc_max must be finite and >= 0"));
        }
        match self.kind.as_str() {
            "ivct_pair" => {
                if self.partners.len() != 2 {
                    return Err(format!("{hid}/{cid}: ivct_pair needs exactly two partners"));
                }
                match self.pairing.as_deref() {
                    Some("clustered" | "clustered_min") => {}
                    Some("random" | "product") => {
                        if !self.pair_z.is_some_and(|z| z.is_finite() && z > 0.0) {
                            return Err(format!(
                                "{hid}/{cid}: random/product pairing needs pair_z > 0"
                            ));
                        }
                    }
                    other => {
                        return Err(format!(
                            "{hid}/{cid}: ivct_pair needs pairing, got {other:?}"
                        ));
                    }
                }
                if let Some(comp) = &self.compensator
                    && !self.partners.iter().any(|p| p == &comp.subtract_from)
                {
                    return Err(format!(
                        "{hid}/{cid}: compensator subtract_from {:?} is not a partner",
                        comp.subtract_from
                    ));
                }
            }
            "end_member" => {
                let ok = self
                    .end_member
                    .as_ref()
                    .is_some_and(|id| host.end_members.iter().any(|m| &m.id == id && !m.colorless));
                if !ok {
                    return Err(format!(
                        "{hid}/{cid}: end_member must name a colored host end member"
                    ));
                }
                if self.conc_unit != "mol_fraction" {
                    return Err(format!("{hid}/{cid}: end_member unit must be mol_fraction"));
                }
            }
            _ => {}
        }

        let allowed_pol: &[&str] = match optical {
            "biaxial" => &["a", "b", "g"],
            _ => &["o", "e"], // uniaxial; isotropic uses only `o` but tolerates `e` in legacy rows
        };
        for band in &mut self.bands {
            band.validate_and_normalize(hid, &cid, allowed_pol, host, &self.conc_unit)?;
        }
        self.bands
            .sort_by(|a, b| a.centre_nm.total_cmp(&b.centre_nm));
        for f in &self.fluorescence {
            f.validate(hid, &cid)?;
        }
        Ok(())
    }
}

/// A single absorption band; after loading always a Gaussian in energy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandData {
    /// Band centre in nm.
    pub centre_nm: f64,
    /// Full width at half maximum in cm^-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fwhm_cm1: Option<f64>,
    /// Band shape as written in the data: `"gaussian_energy"` (default), `"lorentzian"` or `"edge"`.
    /// Loading converts the last two (see [`BandData::convert_shape`]); afterwards this is
    /// `"gaussian_energy"` and [`BandData::shape_original`] records the source shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// Source shape when the load-time conversion changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape_original: Option<String>,
    /// Peak absorption coefficient in cm^-1 per concentration unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_coeff: Option<f64>,
    /// Quadratic term: the peak is `peak_coeff * c + quadratic_coeff * c^2` (cm^-1, `c` the
    /// effective concentration in the chromophore's unit), e.g. pair-enhanced corundum Fe3+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quadratic_coeff: Option<f64>,
    /// Relative polarization weights (e.g. `{ o = 1.0, e = 0.75 }`); a missing key means weight 0
    /// for that mode (isotropic hosts: weight 1).
    #[serde(default)]
    pub pol: BTreeMap<String, f64>,
    /// Peak absorption cross section in cm^2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_cm2: Option<f64>,
    /// Molar decadic absorption coefficient in L/(mol*cm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epsilon_l_mol_cm: Option<f64>,
    /// Spectroscopic transition class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_class: Option<String>,
    /// Band-specific confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Reason string if band is suspect (also set by the loader's consistency checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspect: Option<String>,
    /// Bibliographic source citation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// CIEDE2000 residual of the fit that produced this band's parameters, against the color
    /// of the fitted primary spectrum (`data_version` 4: the GIA cross-sections or the Caltech
    /// raw spectra through our color code, largest over the fitted path lengths and the
    /// D65 / A illuminants). Earlier versions fitted against the research's stated CIELAB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_residual_de00: Option<f64>,
}

impl BandData {
    /// Converts `lorentzian` and `edge` bands into the renderer's Gaussian-in-energy shape.
    ///
    /// - `lorentzian`: same centre, FWHM and peak, Gaussian wings. The integrated area is 32 %
    ///   lower than the Lorentzian's and the far wings are lost; the peak (the tabulated
    ///   quantity) is kept.
    /// - `edge`: `centre_nm` is the onset wavelength and `peak_coeff` the absorption at the onset.
    ///   The edge becomes a Gaussian whose half-maximum point on the long-wavelength side sits
    ///   at the onset: centred half a FWHM above the onset energy (a shorter wavelength) with
    ///   twice the tabulated peak, so the tail reproduces the onset value and rises towards the
    ///   UV. The width is `fwhm_cm1`, or [`EDGE_DEFAULT_FWHM_CM1`] when the band carries none.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown shape name.
    pub fn convert_shape(&mut self) -> Result<(), String> {
        match self.shape.as_deref() {
            None | Some("gaussian_energy") => {}
            Some("lorentzian") => {
                self.shape_original = Some("lorentzian".to_string());
                self.shape = Some("gaussian_energy".to_string());
            }
            Some("edge") => {
                let fwhm = self.fwhm_cm1.unwrap_or(EDGE_DEFAULT_FWHM_CM1);
                let onset_cm1 = 1e7 / self.centre_nm;
                self.centre_nm = 1e7 / 0.5f64.mul_add(fwhm, onset_cm1);
                self.fwhm_cm1 = Some(fwhm);
                self.peak_coeff = self.peak_coeff.map(|p| 2.0 * p);
                self.shape_original = Some("edge".to_string());
                self.shape = Some("gaussian_energy".to_string());
            }
            Some(other) => return Err(format!("unknown band shape {other:?}")),
        }
        Ok(())
    }

    fn validate_and_normalize(
        &mut self,
        hid: &str,
        cid: &str,
        allowed_pol: &[&str],
        host: &HostData,
        conc_unit: &str,
    ) -> Result<(), String> {
        if !self.centre_nm.is_finite() || !(250.0..=1200.0).contains(&self.centre_nm) {
            return Err(format!(
                "Band centre {} nm in {hid}/{cid} outside allowed 250-1200 nm range",
                self.centre_nm
            ));
        }
        if let Some(fwhm) = self.fwhm_cm1
            && (!fwhm.is_finite() || fwhm <= 0.0)
        {
            return Err(format!("Band FWHM {fwhm} in {hid}/{cid} must be > 0"));
        }
        if let Some(peak) = self.peak_coeff
            && (!peak.is_finite() || peak < 0.0)
        {
            return Err(format!(
                "Band peak_coeff {peak} in {hid}/{cid} must be >= 0"
            ));
        }
        if let Some(q) = self.quadratic_coeff
            && (!q.is_finite() || q < 0.0)
        {
            return Err(format!(
                "Band quadratic_coeff {q} in {hid}/{cid} must be finite and >= 0"
            ));
        }
        for (k, w) in &self.pol {
            if !allowed_pol.contains(&k.as_str()) || !w.is_finite() || *w < 0.0 {
                return Err(format!(
                    "Band pol key {k:?}={w} in {hid}/{cid} invalid for the {} host",
                    host.optical
                ));
            }
        }
        if host.optical != "isotropic" && self.pol.is_empty() {
            return Err(format!(
                "Band at {} nm in {hid}/{cid} has no pol weights",
                self.centre_nm
            ));
        }

        if let (Some(sigma), Some(peak)) = (self.sigma_cm2, self.peak_coeff) {
            let expected_alpha = sigma * host.n_site_for_unit(conc_unit);
            if expected_alpha > 0.0 && peak > 0.0 {
                let ratio = if expected_alpha > peak {
                    expected_alpha / peak
                } else {
                    peak / expected_alpha
                };
                if ratio >= 1.95 {
                    self.suspect = Some(format!(
                        "consistency discrepancy: peak_coeff ({peak}) does not match sigma*N_site ({expected_alpha:.4}) within 2x"
                    ));
                }
            }
        }

        // Transition class vs epsilon (spec section 2.4, ranges in L/(mol cm)).
        if let (Some(eps), Some(t_class)) =
            (self.epsilon_l_mol_cm, self.transition_class.as_deref())
        {
            let in_range = match t_class {
                "spin_allowed" => (1.0..=50.0).contains(&eps),
                "spin_forbidden" => (0.05..=2.0).contains(&eps),
                "ivct" => (100.0..=2000.0).contains(&eps),
                _ => true,
            };
            if !in_range {
                self.suspect = Some(format!(
                    "consistency discrepancy: epsilon {eps} outside {t_class} bounds"
                ));
            }
        }

        self.convert_shape()
            .map_err(|e| format!("{hid}/{cid}: {e}"))
    }
}

/// Luminescence and emission parameters of one chromophore (see `docs/fluorescence-plan.md`
/// section 3 and `resolve_fluorescence`).
///
/// A chromophore emits (gets a `FluorescentEmitter`) only when `quantum_yield` is above 0 and it
/// has at least one emission line; an entry with `uv = inert`, no lines or `quantum_yield = 0`
/// is qualitative documentation only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FluorescenceData {
    /// Peak emission wavelength(s) in nm (zero-phonon lines or band centres).
    #[serde(default, deserialize_with = "deserialize_numbers")]
    pub emission_nm: Vec<f64>,
    /// FWHM in nm of each `emission_nm` entry: one value (applies to all lines) or one per line.
    /// Required for an emitting entry. Lines narrower than the renderer's sampling limit
    /// (`MIN_EMISSION_FWHM_NM`, 1 nm) are widened to 1 nm at resolve time, keeping their area.
    #[serde(
        default,
        deserialize_with = "deserialize_numbers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub emission_fwhm_nm: Vec<f64>,
    /// Relative weight (area share) of each `emission_nm` line: empty means equal weights,
    /// else one value per line. Normalised at resolve time.
    #[serde(
        default,
        deserialize_with = "deserialize_numbers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub emission_weight: Vec<f64>,
    /// Optional vibronic sideband (a broad Gaussian next to the zero-phonon lines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideband: Option<SidebandData>,
    /// Radiative quantum yield of the isolated chromophore, `Phi_0` (0.0 to 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantum_yield: Option<f64>,
    /// Concentration quenching: `Phi_eff = Phi_0 * prod 1 / (1 + (c_q / c_half)^n)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quench: Vec<QuenchData>,
    /// Qualitative UV response (e.g. `{ lw = "strong", sw = "weak" }`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub uv: BTreeMap<String, String>,
    /// Confidence of the emission data (`"estimate"` when the yield, widths or the quench law are
    /// the author's estimate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Bibliographic source of the emission data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A vibronic sideband of an emission spectrum: a Gaussian of `fwhm_nm` at `centre_nm` carrying
/// `weight` of all emitted photons (the lines share the remaining `1 - weight`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SidebandData {
    /// Sideband centre in nm.
    pub centre_nm: f64,
    /// Sideband FWHM in nm.
    pub fwhm_nm: f64,
    /// Share of the total emission in the sideband, in `[0, 1)`.
    pub weight: f64,
}

/// One quencher of a luminescent chromophore.
///
/// `c_q` is the recipe amount of the element `by`, converted from its input unit into `unit`
/// through the host's number densities, so `c_half` is stated in a convenient unit
/// (`wt_pct_oxide:FeO`) whatever unit the recipe uses for the element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuenchData {
    /// Quenching element symbol (`"Fe"`), a selectable recipe id of the host.
    pub by: String,
    /// Concentration unit of `c_half` (any valid unit except `intensity`).
    pub unit: String,
    /// Concentration in `unit` at which the yield is halved.
    pub c_half: f64,
    /// Steepness `n` of the quench law.
    pub n: f64,
}

impl FluorescenceData {
    /// Whether this entry describes an emitter: a positive yield and at least one line.
    #[must_use]
    pub fn is_emitting(&self) -> bool {
        self.quantum_yield.is_some_and(|q| q > 0.0) && !self.emission_nm.is_empty()
    }

    fn validate(&self, hid: &str, cid: &str) -> Result<(), String> {
        let ctx = format!("{hid}/{cid} fluorescence");
        if let Some(q) = self.quantum_yield
            && !(q.is_finite() && (0.0..=1.0).contains(&q))
        {
            return Err(format!("{ctx}: quantum_yield {q} outside [0, 1]"));
        }
        let lines = self.emission_nm.len();
        if self
            .emission_nm
            .iter()
            .any(|w| !w.is_finite() || !(200.0..=2000.0).contains(w))
        {
            return Err(format!("{ctx}: emission_nm outside 200-2000 nm"));
        }
        for (name, v) in [
            ("emission_fwhm_nm", &self.emission_fwhm_nm),
            ("emission_weight", &self.emission_weight),
        ] {
            if !(v.is_empty() || v.len() == lines || (name == "emission_fwhm_nm" && v.len() == 1))
                || v.iter().any(|x| !x.is_finite() || *x < 0.0)
            {
                return Err(format!("{ctx}: {name} must be empty or one per line"));
            }
        }
        if self.emission_fwhm_nm.iter().any(|w| *w <= 0.0) {
            return Err(format!("{ctx}: emission_fwhm_nm must be > 0"));
        }
        if let Some(sb) = &self.sideband
            && !(sb.centre_nm.is_finite()
                && sb.fwhm_nm.is_finite()
                && sb.fwhm_nm > 0.0
                && (0.0..1.0).contains(&sb.weight))
        {
            return Err(format!("{ctx}: invalid sideband"));
        }
        if lines + usize::from(self.sideband.is_some()) > crate::optics::fluorescence::MAX_BANDS {
            return Err(format!("{ctx}: more than 8 emission bands"));
        }
        if self.is_emitting() && self.emission_fwhm_nm.is_empty() {
            return Err(format!("{ctx}: an emitting entry needs emission_fwhm_nm"));
        }
        for q in &self.quench {
            if q.by.is_empty()
                || !unit_is_valid(&q.unit)
                || q.unit == "intensity"
                || !(q.c_half.is_finite() && q.c_half > 0.0)
                || !(q.n.is_finite() && q.n > 0.0)
            {
                return Err(format!("{ctx}: invalid quench entry for {:?}", q.by));
            }
        }
        Ok(())
    }
}

fn deserialize_numbers<'de, D>(deserializer: D) -> Result<Vec<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        One(f64),
        Many(Vec<f64>),
    }

    match Option::<Helper>::deserialize(deserializer)? {
        None => Ok(Vec::new()),
        Some(Helper::One(v)) => Ok(vec![v]),
        Some(Helper::Many(v)) => Ok(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = r#"
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
"#;

    fn with_chromophore(body: &str) -> String {
        format!(
            "{HOST}\n[[host.chromophore]]\nid = \"c\"\nkind = \"ion\"\nelements = [\"Cr\"]\nconc_unit = \"ppm_site\"\n{body}"
        )
    }

    fn band(shape: &str, extra: &str) -> String {
        format!(
            "[[host.chromophore.band]]\ncentre_nm = 400.0\nshape = \"{shape}\"\npeak_coeff = 8.0\npol = {{ o = 1.0, e = 1.0 }}\n{extra}\n"
        )
    }

    #[test]
    fn lorentzian_and_edge_bands_become_gaussians_at_load() {
        let cat = ChromophoreCatalogue::from_toml(&with_chromophore(&format!(
            "{}{}",
            band("lorentzian", "fwhm_cm1 = 1200.0"),
            band("edge", "fwhm_cm1 = 3000.0")
        )))
        .expect("loads");
        let bands = &cat.host("toy").expect("toy").chromophores[0].bands;
        let lor = bands
            .iter()
            .find(|b| b.shape_original.as_deref() == Some("lorentzian"))
            .expect("lorentzian");
        assert_eq!(lor.shape.as_deref(), Some("gaussian_energy"));
        assert_eq!(
            (lor.centre_nm, lor.fwhm_cm1, lor.peak_coeff),
            (400.0, Some(1200.0), Some(8.0))
        );
        // Edge: onset 400 nm = 25000 cm-1; Gaussian centred 1500 cm-1 higher (shorter wavelength)
        // with twice the peak, so its half maximum sits exactly at the onset.
        let edge = bands
            .iter()
            .find(|b| b.shape_original.as_deref() == Some("edge"))
            .expect("edge");
        assert_eq!(edge.shape.as_deref(), Some("gaussian_energy"));
        assert!((edge.centre_nm - 1e7 / 26500.0).abs() < 1e-9);
        assert!(edge.centre_nm < 400.0);
        assert_eq!(edge.peak_coeff, Some(16.0));
        let sigma = edge.fwhm_cm1.expect("fwhm") / (2.0 * (2.0f64 * std::f64::consts::LN_2).sqrt());
        let at_onset = 16.0 * (-0.5 * (1500.0 / sigma).powi(2)).exp();
        assert!(
            (at_onset - 8.0).abs() < 1e-9,
            "tail must reproduce the onset value: {at_onset}"
        );
    }

    #[test]
    fn edge_without_width_gets_the_default_and_becomes_usable() {
        let cat =
            ChromophoreCatalogue::from_toml(&with_chromophore(&band("edge", ""))).expect("loads");
        let c = &cat.host("toy").expect("toy").chromophores[0];
        assert_eq!(c.bands[0].fwhm_cm1, Some(EDGE_DEFAULT_FWHM_CM1));
        assert!(c.is_offered());
        assert!(unknown_shape_is_rejected());
    }

    fn unknown_shape_is_rejected() -> bool {
        ChromophoreCatalogue::from_toml(&with_chromophore(&band("voigt", "fwhm_cm1 = 100.0")))
            .is_err()
    }

    #[test]
    fn loaded_catalogue_has_only_gaussian_bands() {
        for h in &ChromophoreCatalogue::global().hosts {
            for c in &h.chromophores {
                for b in &c.bands {
                    assert!(
                        matches!(b.shape.as_deref(), None | Some("gaussian_energy")),
                        "{}/{}: {:?}",
                        h.id,
                        c.id,
                        b.shape
                    );
                }
            }
        }
    }

    #[test]
    fn units_are_validated_against_the_fixed_list() {
        for good in [
            "ppm_site",
            "ppma_all",
            "mol_fraction",
            "intensity",
            "wt_pct_oxide:Cr2O3",
            "wt_pct_oxide:UO3",
        ] {
            assert!(unit_is_valid(good), "{good}");
        }
        for bad in [
            "matrix (not a dopant)",
            "relative centre density",
            "wt_pct_oxide:generic",
            "ppm",
            "wt_pct_oxide:",
        ] {
            assert!(!unit_is_valid(bad), "{bad}");
        }
        let src = with_chromophore(&band("gaussian_energy", "fwhm_cm1 = 100.0"))
            .replace("ppm_site", "relative centre density");
        assert!(ChromophoreCatalogue::from_toml(&src).is_err());
        for h in &ChromophoreCatalogue::global().hosts {
            for c in &h.chromophores {
                assert!(
                    unit_is_valid(&c.conc_unit),
                    "{}/{}: {}",
                    h.id,
                    c.id,
                    c.conc_unit
                );
            }
        }
    }

    #[test]
    fn pol_keys_must_match_the_optical_class() {
        let src = with_chromophore(
            "[[host.chromophore.band]]\ncentre_nm = 500.0\nfwhm_cm1 = 100.0\npeak_coeff = 1.0\npol = { a = 1.0 }\n",
        );
        assert!(ChromophoreCatalogue::from_toml(&src).is_err());
    }

    /// Axis mappings are stored by axis length: biaxial hosts only, keys a/b/g, positive lengths.
    #[test]
    fn ray_axis_lengths_are_validated_and_shipped_for_the_biaxial_hosts() {
        let host_with = |optical: &str, axes: &str| {
            HOST.replace(
                "optical = \"uniaxial\"",
                &format!("optical = \"{optical}\"\nray_axis_angstrom = {axes}"),
            )
        };
        let good = ChromophoreCatalogue::from_toml(&host_with(
            "biaxial",
            "{ a = 4.43, b = 9.40, g = 5.48 }",
        ))
        .expect("valid axes load");
        assert_eq!(good.hosts[0].ray_axis_angstrom["b"], 9.40);
        for (optical, axes) in [
            ("uniaxial", "{ a = 4.43 }"),
            ("biaxial", "{ x = 4.43 }"),
            ("biaxial", "{ a = -1.0 }"),
            ("biaxial", "{ a = 0.0 }"),
        ] {
            assert!(
                ChromophoreCatalogue::from_toml(&host_with(optical, axes)).is_err(),
                "{optical} {axes}"
            );
        }
        let cat = ChromophoreCatalogue::global();
        for host in &cat.hosts {
            assert_eq!(
                host.ray_axis_angstrom.len(),
                if matches!(host.id.as_str(), "chrysoberyl" | "tanzanite" | "peridot") {
                    3
                } else {
                    0
                },
                "{}",
                host.id
            );
        }
    }

    #[test]
    fn epsilon_ranges_follow_the_spec() {
        let check = |class: &str, eps: f64| {
            let src = with_chromophore(&format!(
                "[[host.chromophore.band]]\ncentre_nm = 500.0\nfwhm_cm1 = 100.0\npeak_coeff = 1.0\npol = {{ o = 1.0, e = 1.0 }}\nepsilon_l_mol_cm = {eps}\ntransition_class = \"{class}\"\n"
            ));
            let cat = ChromophoreCatalogue::from_toml(&src).expect("loads");
            cat.host("toy").expect("toy").chromophores[0].bands[0]
                .suspect
                .is_some()
        };
        assert!(!check("spin_allowed", 1.0) && !check("spin_allowed", 50.0));
        assert!(check("spin_allowed", 0.9) && check("spin_allowed", 60.0));
        assert!(!check("spin_forbidden", 0.05) && check("spin_forbidden", 3.0));
        assert!(
            !check("ivct", 100.0)
                && !check("ivct", 2000.0)
                && check("ivct", 2500.0)
                && check("ivct", 80.0)
        );
        // The corundum Fe2+-Ti4+ band carries epsilon ~500 and passes.
        let cor = ChromophoreCatalogue::global()
            .host("corundum")
            .expect("corundum");
        let pair = cor
            .chromophores
            .iter()
            .find(|c| c.kind == "ivct_pair" && c.partners[1] == "Ti4+")
            .expect("pair");
        assert!(pair.bands.iter().any(|b| b.epsilon_l_mol_cm == Some(500.0)));
        assert!(pair.is_offered());
    }

    #[test]
    fn full_flagged_set_is_pinned() {
        let mut flagged: Vec<String> = Vec::new();
        for h in &ChromophoreCatalogue::global().hosts {
            for c in &h.chromophores {
                if let Some(why) = &c.suspect {
                    flagged.push(format!(
                        "{}/{} [{}]",
                        h.id,
                        c.id,
                        why.split(';').next().unwrap_or("")
                    ));
                    assert!(!c.is_offered());
                }
                for b in &c.bands {
                    if b.suspect.is_some() {
                        flagged.push(format!("{}/{} @{} nm", h.id, c.id, b.centre_nm));
                    }
                }
            }
        }
        flagged.sort();
        let got: Vec<&str> = flagged.iter().map(String::as_str).collect();
        assert_eq!(
            got,
            vec![
                "corundum/[h•–Fe3+] @450 nm",
                "corundum/[h•–Fe3+] [10x too strong in Report]",
                "cubic_zirconia/Cu, V, Ti, Mn, Ni dopants (commercial, unverified) [no band data: commercial dopants, unverified (Report p.12)]",
                "diamond/GR1 (neutral vacancy V0) @700 nm",
                "diamond/GR1 (neutral vacancy V0) [sigma vs alpha inconsistent by 10x]",
                "garnet_pyralspite/Fe2+-Fe3+ IVCT [IVCT 20-30x below its own epsilon (plan 2.4)]",
                "tanzanite/V3+ / Ti4+ (heated 550 C, dichroic blue) [heated row reduces both coefficients and weights (double counting, plan 2.4)]",
            ]
        );
        // The corundum unit-basis flags of review finding 4 are gone.
        assert!(!got.iter().any(|g| g.starts_with("corundum/Fe3+")
            || g.starts_with("corundum/V3+")
            || g.starts_with("corundum/Ti3+")));
    }

    #[test]
    fn selectable_ids_by_kind() {
        let cat = ChromophoreCatalogue::global();
        assert_eq!(
            cat.selectable_elements("corundum"),
            vec!["Cr", "Fe", "Mg", "Ti", "V"] // Mg: compensator of the Fe2+-Ti4+ pair
        );
        assert_eq!(
            cat.selectable_elements("chrysoberyl"),
            vec!["Cr", "Fe", "V"]
        );
        assert!(
            cat.selectable_elements("diamond")
                .contains(&"N".to_string())
        );
        assert!(
            cat.selectable_elements("quartz")
                .contains(&"Al".to_string())
        );
        assert!(cat.selectable_elements("nonexistent").is_empty());
        assert_eq!(
            cat.selectable_treatments("corundum", &["Fe"])
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["oxidizing_anneal"]
        );
    }

    #[test]
    fn oxide_bases_convert_through_site_density() {
        let cat = ChromophoreCatalogue::global();
        let beryl = cat.host("beryl").expect("beryl");
        let ratio =
            beryl.n_site_for_unit("wt_pct_oxide:FeO") / beryl.n_site_for_unit("wt_pct_oxide:Fe2O3");
        assert!((ratio - (1.0 / 71.84) / (2.0 / 159.69)).abs() < 1e-12);
        let cor = cat.host("corundum").expect("corundum");
        assert!(
            (cor.n_site_for_unit("ppma_all") / cor.n_site_for_unit("ppm_site") - 2.5).abs() < 1e-3
        );
        assert_eq!(
            cor.element_unit("Cr").as_deref(),
            Some("wt_pct_oxide:Cr2O3")
        );
        assert_eq!(cor.element_unit("Fe").as_deref(), Some("ppm_site"));
    }

    #[test]
    fn end_member_data_is_complete() {
        let cat = ChromophoreCatalogue::global();
        for h in cat.hosts.iter().filter(|h| !h.end_members.is_empty()) {
            assert_eq!(
                h.end_members.iter().filter(|m| m.colorless).count(),
                1,
                "{}",
                h.id
            );
            for c in h.chromophores.iter().filter(|c| c.kind == "end_member") {
                assert!(
                    h.end_members
                        .iter()
                        .any(|m| Some(&m.id) == c.end_member.as_ref())
                );
            }
        }
        assert!(cat.end_member("almandine").is_some());
    }
}

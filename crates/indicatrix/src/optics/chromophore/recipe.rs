//! color recipe and resolved band budget definitions.

use serde::{Deserialize, Deserializer, Serialize};

use crate::optics::absorption::{AbsorptionBand, AbsorptionTensor};

/// A physical chromophore recipe specifying host, concentration entries, treatments and scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct colorRecipe {
    /// Host crystal identifier (e.g. `"corundum"`).
    pub host: String,
    /// Catalogue data version this recipe was built with.
    pub data_version: u32,
    /// Element and chromophore amounts, sorted by ID.
    pub entries: Vec<RecipeEntry>,
    /// Active treatments applied in catalogue order.
    pub treatments: Vec<String>,
    /// Global absorption multiplier on every item's resulting alpha (default 1.0); must be finite.
    #[serde(deserialize_with = "finite_f64")]
    pub strength: f64,
    /// Reference stone path thickness in mm for preview swatches and solver (default 5.0); must be finite.
    #[serde(deserialize_with = "finite_f32")]
    pub reference_path_mm: f32,
    /// The budgeted, rendered absorption tensor bands (single source of truth for rendering).
    pub resolved_bands: ResolvedBands,
}

impl colorRecipe {
    /// Builds a new recipe with defaults (strength = 1.0, `reference_path_mm` = 5.0).
    #[must_use]
    pub fn new(host: impl Into<String>, data_version: u32) -> Self {
        Self {
            host: host.into(),
            data_version,
            entries: Vec::new(),
            treatments: Vec::new(),
            strength: 1.0,
            reference_path_mm: 5.0,
            resolved_bands: ResolvedBands::default(),
        }
    }

    /// Sets an element or end-member amount, maintaining sorted order.
    ///
    /// Non-finite amounts are rejected (the recipe is left unchanged and `false` is returned);
    /// negative amounts are stored as 0. The upper bound `conc_max` is applied by
    /// [`Self::clamp_amounts`] and by `resolve`, which know the host.
    pub fn set_amount(&mut self, id: &str, amount: f64) -> bool {
        if !amount.is_finite() {
            return false;
        }
        let amount = amount.max(0.0);
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.amount = amount;
        } else {
            self.entries.push(RecipeEntry {
                id: id.to_string(),
                amount,
            });
            self.entries.sort_by(|a, b| a.id.cmp(&b.id));
        }
        true
    }

    /// Sets the global strength; non-finite values are rejected and negative ones stored as 0.
    pub const fn set_strength(&mut self, strength: f64) -> bool {
        if !strength.is_finite() {
            return false;
        }
        self.strength = strength.max(0.0);
        true
    }

    /// Clamps every entry to `[0, conc_max]` of its host (`HostData::element_conc_max`) and
    /// drops entries whose id is not selectable in the host. Returns the number of changed or
    /// dropped entries. A recipe for an unknown host is left untouched.
    pub fn clamp_amounts(&mut self, catalogue: &super::catalogue::ChromophoreCatalogue) -> usize {
        let Some(host) = catalogue.host(&self.host) else {
            return 0;
        };
        let selectable = catalogue.selectable_elements(&self.host);
        let before = self.entries.clone();
        self.entries.retain(|e| selectable.contains(&e.id));
        for e in &mut self.entries {
            let max = host.element_conc_max(&e.id);
            e.amount = if e.amount.is_finite() {
                e.amount.clamp(0.0, max)
            } else {
                0.0
            };
        }
        before.iter().filter(|b| !self.entries.contains(b)).count()
    }

    /// Gets an entry amount, or 0.0 if absent.
    #[must_use]
    pub fn amount(&self, id: &str) -> f64 {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map_or(0.0, |e| e.amount)
    }

    /// Removes an entry by ID.
    pub fn remove_entry(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
    }
}

/// A single entry in a recipe: element/chromophore identifier and its numerical amount.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeEntry {
    /// Selectable identifier (e.g. `"Cr"`, `"Fe"`, or chromophore ID).
    pub id: String,
    /// Amount in the item's input unit; must be finite (deserialisation rejects NaN/inf).
    #[serde(deserialize_with = "finite_f64")]
    pub amount: f64,
}

fn finite_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(
            "non-finite number in color recipe",
        ))
    }
}

fn finite_f32<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = f32::deserialize(d)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(serde::de::Error::custom(
            "non-finite number in color recipe",
        ))
    }
}

/// A serialized snapshot of the budgeted [`AbsorptionTensor`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResolvedBands {
    /// Ordinary ray / alpha bands.
    pub o_ray: Vec<AbsorptionBand>,
    /// Extraordinary ray / gamma bands.
    pub e_ray: Vec<AbsorptionBand>,
    /// Optional beta ray bands for trichroic biaxial materials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta_ray: Option<Vec<AbsorptionBand>>,
    /// Whether the material has pleochroic absorption.
    pub is_pleochroic: bool,
}

impl ResolvedBands {
    /// Converts these budgeted bands into a live [`AbsorptionTensor`].
    #[must_use]
    pub fn to_tensor(&self) -> AbsorptionTensor {
        self.beta_ray.as_ref().map_or_else(
            || {
                if self.is_pleochroic {
                    AbsorptionTensor::uniaxial(self.o_ray.clone(), self.e_ray.clone())
                } else {
                    AbsorptionTensor::isotropic(self.o_ray.clone())
                }
            },
            |beta| AbsorptionTensor::biaxial(self.o_ray.clone(), beta.clone(), self.e_ray.clone()),
        )
    }

    /// Captures bands from an [`AbsorptionTensor`].
    #[must_use]
    pub fn from_tensor(tensor: &AbsorptionTensor) -> Self {
        Self {
            o_ray: tensor.o_ray.clone(),
            e_ray: tensor.e_ray.clone(),
            beta_ray: tensor.beta_ray.clone(),
            is_pleochroic: tensor.is_pleochroic,
        }
    }
}

/// Warnings produced during forward model band budgeting.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveWarning {
    /// Bands exceeded the GPU budget of 8 per eigenmode and the weakest were dropped.
    BandsDropped {
        /// Mode name (`"o_ray"`, `"e_ray"`, or `"beta_ray"`).
        mode: String,
        /// Number of dropped bands.
        count: usize,
        /// color difference Delta E 2000 caused by dropping them.
        delta_e: f64,
    },
    /// Bands exceeded the GPU budget of 8 per eigenmode and the cheapest pairs were merged into
    /// single energy Gaussians (moment matching, least color error first).
    BandsMerged {
        /// Mode name (`"o_ray"`, `"e_ray"`, or `"beta_ray"`).
        mode: String,
        /// Number of merges performed (each removes one band).
        count: usize,
        /// color difference Delta E 2000 caused by the merges.
        delta_e: f64,
    },
    /// A treatment was ignored because a required element is absent or the id is unknown.
    TreatmentSkipped {
        /// Treatment id.
        id: String,
        /// Why it was skipped.
        reason: String,
    },
    /// A recipe entry that is not selectable in the host was ignored.
    UnknownEntry {
        /// Entry id.
        id: String,
    },
    /// A catalogue `requires` / `excludes` relation is violated: an active chromophore `id` whose
    /// `relation` partner `other` is active (`"excludes"`) or absent (`"requires"`). Advisory.
    Relation {
        /// The active chromophore carrying the relation.
        id: String,
        /// `"requires"` or `"excludes"`.
        relation: &'static str,
        /// The chromophore the relation names.
        other: String,
    },
    /// An amount above the host's `conc_max` was clamped.
    AmountClamped {
        /// Entry id.
        id: String,
        /// Requested amount.
        requested: f64,
        /// Amount used.
        used: f64,
    },
}

/// Errors during forward model resolution.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveError {
    /// The specified host crystal was not found in the catalogue.
    HostNotFound(String),
    /// An amount, the strength or the reference path is NaN or infinite.
    NonFinite(String),
    /// Garnet-style end-member fractions sum to more than 1.
    EndMemberSumExceeded(f64),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostNotFound(h) => write!(f, "Host crystal '{h}' not found in catalogue"),
            Self::NonFinite(what) => write!(f, "Non-finite value in color recipe: {what}"),
            Self::EndMemberSumExceeded(sum) => write!(f, "End-member fractions sum to {sum} > 1"),
        }
    }
}

impl std::error::Error for ResolveError {}

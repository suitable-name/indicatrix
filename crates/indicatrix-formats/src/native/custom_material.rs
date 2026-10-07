//! The `[material.custom]` snapshot table of the native document: everything a custom
//! material needs to be rebuilt on load (see [`CustomMaterialSnapshot`]). Split out of
//! `schema.rs` by size only; `schema::CustomMaterialSnapshot` re-exports it.

/// The `[material.custom]` snapshot for a design's custom catalogue material.
///
/// Everything needed to reconstruct that material if its name does not resolve on
/// the machine that opens the file -- without it, a native file referencing a
/// custom material by name only falls back to Diamond elsewhere.
///
/// Sharing a `.indicatrix.toml` + `.asc` pair naming a custom material (or simply
/// reinstalling and losing the local database row) would otherwise silently
/// resolve that name to Diamond, with no warning. Every field here is a plain
/// primitive, not
/// `indicatrix`'s own `GemMaterial` type: this crate has no dependency on
/// `indicatrix` at all (see the module doc comment's "only pulls in what it
/// needs" rule), so building this snapshot from a real `GemMaterial` -- and
/// registering it back as a session-local custom material when the name fails to
/// resolve -- is `indicatrix-cut-core`'s job, same split as every other table
/// here. `crystal_system`/`optical_character` are carried as their `Debug`-style
/// names (e.g. `"Trigonal"`, `"UniaxialNegative"`) purely for a human reading the
/// raw TOML; `GemMaterial::new_custom` re-derives both from the sign of
/// `birefringence_delta` on load; and `specific_gravity` is `None` when the
/// material carried no SG at save time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomMaterialSnapshot {
    /// Mean refractive index.
    pub mean_ri: f64,
    /// Dispersion offset from the reference value.
    pub dispersion_delta: f64,
    /// Birefringence offset from the reference value.
    pub birefringence_delta: f64,
    /// Specific gravity, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_gravity: Option<f64>,
    /// Crystal system of the material.
    pub crystal_system: String,
    /// Optical character of the material.
    pub optical_character: String,
    /// The material's body color, `absorption_rgb = [r, g, b]`: the isotropic
    /// absorption triple `GemMaterial::new_custom` takes (`None` = colorless).
    ///
    /// Added after this table first shipped: `#[serde(default)]` loads a file written
    /// before it existed with `None` -- exactly the colorless material such a file
    /// always described -- so the schema version ([`FORMAT_VERSION`]) does not move.
    /// An older build reading a newer file keeps the key in [`Self::unknown`] and
    /// writes it back unchanged. Stored as `f64` (TOML's only float) through each
    /// `f32` component's shortest round-trip decimal -- see [`Self::with_body_color`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_rgb: Option<[f64; 3]>,
    /// The N-band body color (path-aware L*C*h editor): `absorption_bands_per_mm =
    /// [[centre_nm, width_nm, amplitude_per_mm], ...]`, written NEXT TO the fitted
    /// [`Self::absorption_rgb`] triple (the nearest three-band colour, so a build that does
    /// not know this key still shows a close colour and writes the key back unchanged).
    /// When present and non-empty the bands win on load. `#[serde(default)]`: older files
    /// load with `None`; omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_bands_per_mm: Option<Vec<[f64; 3]>>,
    /// Optional physically based color recipe and fallback tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_recipe: Option<ColorRecipeDto>,
    /// The material's dispersion curve as coefficients (`[material.custom.dispersion_model]`),
    /// when the author typed Sellmeier or Cauchy coefficients instead of a refractive index
    /// and an `n_F - n_C` figure.
    ///
    /// `None` is the plain path: the material is the Cauchy fit
    /// `GemMaterial::new_custom` builds from [`Self::mean_ri`] and
    /// [`Self::dispersion_delta`], which is what every file written before this field
    /// existed describes (`#[serde(default)]` loads them with `None`, and nothing is written
    /// while it is `None`, so those files keep their bytes). With a model present, `mean_ri`
    /// and `dispersion_delta` still hold the model's `n_d` and `n_F - n_C`, so a build that
    /// does not know the table (it keeps it in [`Self::unknown`] and writes it back
    /// unchanged) restores a close Cauchy stand-in rather than nothing. A table this build
    /// cannot read (a model kind from a newer build, or a malformed one) loads as `None` and
    /// the material takes the plain path.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_dispersion_model"
    )]
    pub dispersion_model: Option<DispersionModelDto>,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// A custom material's dispersion curve in the native file, as plain numbers.
///
/// The same three closed forms as `indicatrix::optics::dispersion::DispersionModel` (this
/// crate has no dependency on `indicatrix`; `indicatrix-cut-core` converts both ways).
///
/// Wavelengths in the formulas are in micrometers: Sellmeier `n^2 = 1 + sum(B_i * l^2 /
/// (l^2 - C_i))` with `C` in square micrometers, Cauchy `n = a + b / l^2 + c / l^4`. Written
/// as a table with a `kind` key, for example:
///
/// ```toml
/// [material.custom.dispersion_model]
/// kind = "cauchy"
/// a = 1.7
/// b = 0.006
/// c = 0.0
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DispersionModelDto {
    /// One Sellmeier term.
    Sellmeier1 {
        /// The term's weight (dimensionless).
        b1: f64,
        /// The term's resonance, in square micrometers.
        c1: f64,
    },
    /// Three Sellmeier terms.
    Sellmeier3 {
        /// The three weights (dimensionless).
        b: [f64; 3],
        /// The three resonances, in square micrometers.
        c: [f64; 3],
    },
    /// A Cauchy fit.
    Cauchy {
        /// The constant term.
        a: f64,
        /// The `1 / l^2` coefficient, in square micrometers.
        b: f64,
        /// The `1 / l^4` coefficient, in micrometers to the fourth.
        c: f64,
    },
}

/// Reads [`CustomMaterialSnapshot::dispersion_model`], turning a table this build cannot
/// parse into `None` instead of failing the whole file.
#[expect(
    clippy::unnecessary_wraps,
    reason = "serde's `deserialize_with` requires this exact `Result` signature"
)]
fn lenient_dispersion_model<'de, D>(deserializer: D) -> Result<Option<DispersionModelDto>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    Ok(Option::<DispersionModelDto>::deserialize(deserializer)
        .ok()
        .flatten())
}

/// Serialized DTO for a physically based color recipe in the native format.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ColorRecipeDto {
    /// Raw recipe JSON string containing recipe data, `resolved_bands`, and `data_version`.
    pub recipe_json: String,
    /// The fallback absorption RGB triple written alongside the recipe.
    #[serde(default)]
    pub fallback_rgb: [f64; 3],
}

impl CustomMaterialSnapshot {
    /// Builds a fresh snapshot with no unknown/future fields carried over -- see
    /// [`PreformTable::new`]'s own doc comment for why this lives here rather than
    /// in `indicatrix-cut-core`.
    #[must_use]
    pub fn new(
        mean_ri: f64,
        dispersion_delta: f64,
        birefringence_delta: f64,
        specific_gravity: Option<f64>,
        crystal_system: impl Into<String>,
        optical_character: impl Into<String>,
    ) -> Self {
        Self {
            mean_ri,
            dispersion_delta,
            birefringence_delta,
            specific_gravity,
            crystal_system: crystal_system.into(),
            optical_character: optical_character.into(),
            absorption_rgb: None,
            absorption_bands_per_mm: None,
            color_recipe: None,
            dispersion_model: None,
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::dispersion_model`].
    #[must_use]
    pub const fn with_dispersion_model(mut self, model: Option<DispersionModelDto>) -> Self {
        self.dispersion_model = model;
        self
    }

    /// Attaches [`Self::color_recipe`].
    #[must_use]
    pub fn with_color_recipe(mut self, recipe: Option<ColorRecipeDto>) -> Self {
        self.color_recipe = recipe;
        self
    }

    /// Attaches [`Self::absorption_rgb`] as the `f64` triple stored on disk.
    #[must_use]
    pub const fn with_absorption_rgb(mut self, rgb: Option<[f64; 3]>) -> Self {
        self.absorption_rgb = rgb;
        self
    }

    /// Attaches [`Self::absorption_bands_per_mm`] from `f32` rows
    /// (`[centre_nm, width_nm, amplitude_per_mm]`), each component through its shortest
    /// round-trip decimal (see [`Self::with_body_color`]). An empty list stores `None`.
    #[must_use]
    pub fn with_absorption_bands(mut self, rows: &[[f32; 3]]) -> Self {
        self.absorption_bands_per_mm = (!rows.is_empty()).then(|| {
            rows.iter()
                .map(|r| {
                    r.map(|v| {
                        v.to_string()
                            .parse::<f64>()
                            .unwrap_or_else(|_| f64::from(v))
                    })
                })
                .collect()
        });
        self
    }

    /// The N-band body color as `f32` rows (`None` when absent or empty).
    #[must_use]
    pub fn absorption_bands(&self) -> Option<Vec<[f32; 3]>> {
        self.absorption_bands_per_mm
            .as_ref()
            .filter(|b| !b.is_empty())
            .map(|rows| rows.iter().map(|r| r.map(|v| v as f32)).collect())
    }

    /// Attaches the body color from an editor's `f32` absorption triple.
    ///
    /// Each component is stored as its shortest round-trip decimal rather than a plain
    /// widening cast: `f64::from(0.2_f32)` would be written as `0.20000000298023224`,
    /// which is what a cutter reading the raw file would then see for a color picked
    /// as "0.2". The decimal text identifies the `f32` uniquely, so
    /// [`Self::body_color`] returns the identical bits.
    #[must_use]
    pub fn with_body_color(self, rgb: Option<[f32; 3]>) -> Self {
        let stored = rgb.map(|rgb| {
            rgb.map(|v| {
                v.to_string()
                    .parse::<f64>()
                    .unwrap_or_else(|_| f64::from(v))
            })
        });
        self.with_absorption_rgb(stored)
    }

    /// The body color as the editor's `f32` absorption triple (`None` = colorless).
    #[must_use]
    pub fn body_color(&self) -> Option<[f32; 3]> {
        self.absorption_rgb.map(|rgb| rgb.map(|v| v as f32))
    }
}

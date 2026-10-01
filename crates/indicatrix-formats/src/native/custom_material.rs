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
    /// The material's body colour, `absorption_rgb = [r, g, b]`: the isotropic
    /// absorption triple `GemMaterial::new_custom` takes (`None` = colourless).
    ///
    /// Added after this table first shipped: `#[serde(default)]` loads a file written
    /// before it existed with `None` -- exactly the colourless material such a file
    /// always described -- so the schema version ([`FORMAT_VERSION`]) does not move.
    /// An older build reading a newer file keeps the key in [`Self::unknown`] and
    /// writes it back unchanged. Stored as `f64` (TOML's only float) through each
    /// `f32` component's shortest round-trip decimal -- see [`Self::with_body_colour`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absorption_rgb: Option<[f64; 3]>,
    /// See [`PreformTable::unknown`]'s doc comment.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
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
            unknown: toml::Table::new(),
        }
    }

    /// Attaches [`Self::absorption_rgb`] as the `f64` triple stored on disk.
    #[must_use]
    pub const fn with_absorption_rgb(mut self, rgb: Option<[f64; 3]>) -> Self {
        self.absorption_rgb = rgb;
        self
    }

    /// Attaches the body colour from an editor's `f32` absorption triple.
    ///
    /// Each component is stored as its shortest round-trip decimal rather than a plain
    /// widening cast: `f64::from(0.2_f32)` would be written as `0.20000000298023224`,
    /// which is what a cutter reading the raw file would then see for a colour picked
    /// as "0.2". The decimal text identifies the `f32` uniquely, so
    /// [`Self::body_colour`] returns the identical bits.
    #[must_use]
    pub fn with_body_colour(self, rgb: Option<[f32; 3]>) -> Self {
        let stored = rgb.map(|rgb| {
            rgb.map(|v| {
                v.to_string()
                    .parse::<f64>()
                    .unwrap_or_else(|_| f64::from(v))
            })
        });
        self.with_absorption_rgb(stored)
    }

    /// The body colour as the editor's `f32` absorption triple (`None` = colourless).
    #[must_use]
    pub fn body_colour(&self) -> Option<[f32; 3]> {
        self.absorption_rgb.map(|rgb| rgb.map(|v| v as f32))
    }
}

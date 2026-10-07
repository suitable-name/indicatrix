//! A design's material choice ([`MaterialSelection`]), how it resolves to a real
//! [`GemMaterial`] plus refractive index ([`MaterialSelection::resolve`]), and the
//! [`MaterialLookup`] seam that resolution goes through -- see [`crate::material`]'s
//! module doc comment for the full picture.

use super::specific_gravity::built_in_specific_gravity;
use crate::optics_hints::critical_angle_deg;
use indicatrix::optics::{
    dispersion::DispersionModel,
    materials::{GemMaterial, body_color::preset_label_for_rgb},
};

/// A design's material choice for the carat-weight estimate.
///
/// Which built-in preset (if any) to default from, plus an optional per-design
/// override -- SG varies by variety and specimen even within one named species, so a
/// design must always be able to state its own number regardless of what (if
/// anything) [`super::built_in_specific_gravity`] knows about the selected name.
///
/// Stored on [`crate::design::Design`] like [`crate::preform::PreformSpec`] -- a
/// design input, mutated only through [`crate::edit::History`].
#[derive(Clone, PartialEq, Default)]
pub struct MaterialSelection {
    /// The selected preset name, exactly a `GemMaterial::name` string, or `None` for
    /// "no preset selected" (a brand-new design, or one relying entirely on
    /// `specific_gravity_override` for a species with no preset, e.g. garnet).
    pub name: Option<String>,
    /// A user-authored SG that overrides whatever `name` looks up, or supplies one
    /// outright when `name` is `None` or looks up to `None`. `None` means "use the
    /// selected preset's own representative figure".
    pub specific_gravity_override: Option<f64>,
    /// A user-authored refractive index (sodium D line) that overrides whatever
    /// `name` resolves to, same precedence as `specific_gravity_override`. `None`
    /// means "use the resolved material's own `n_D`".
    pub refractive_index_override: Option<f64>,
    /// A per-design body color: the `[R, G, B]` absorption triple
    /// `GemMaterial::with_body_color` applies on top of whatever `name` resolves to
    /// (see `indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS` for the
    /// fixed presets). `None` means "the material's own color". Lets a cutter try a
    /// design as, say, a yellow instead of a blue sapphire without authoring a new
    /// custom material; the variant is isotropic (the base material's pleochroism
    /// is not modelled while this is set).
    pub body_color_override: Option<[f32; 3]>,
    /// The N-band form of the per-design body color (path-aware L*C*h editor): one
    /// `[centre_nm, width_nm, amplitude_per_mm]` row per non-zero band
    /// (`indicatrix::optics::absorption::body_color_bands`). Written NEXT TO the nearest
    /// `body_color_override` triple so an older build still shows a close colour; when
    /// present (and non-empty) the bands win over the triple in [`Self::apply_overrides`].
    pub body_color_bands_override: Option<Vec<[f32; 3]>>,
    /// Millimetres per model unit the bands' amplitudes were solved for
    /// (`GemMaterial::absorption_path_scale`); only meaningful with
    /// [`Self::body_color_bands_override`]. `None` keeps the material's own scale.
    pub absorption_path_scale_override: Option<f32>,
}

/// Hand-written rather than derived so a selection WITHOUT a body-color override
/// prints exactly as it did before that field existed (the desktop's identity pins
/// hash these `Debug` dumps); the field is printed only when it is set.
impl std::fmt::Debug for MaterialSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("MaterialSelection");
        out.field("name", &self.name)
            .field("specific_gravity_override", &self.specific_gravity_override)
            .field("refractive_index_override", &self.refractive_index_override);
        if let Some(rgb) = &self.body_color_override {
            out.field("body_color_override", rgb);
        }
        if let Some(bands) = &self.body_color_bands_override {
            out.field("body_color_bands_override", bands);
        }
        if let Some(scale) = &self.absorption_path_scale_override {
            out.field("absorption_path_scale_override", scale);
        }
        out.finish()
    }
}

impl MaterialSelection {
    /// No preset selected and no override -- a brand-new design's starting point.
    /// `const` (the derived `Default` impl is not) for a cheap, allocation-free
    /// default; [`crate::design::Design::new`] itself stopped being `const fn`
    /// once it started allocating a fresh `Vec<TierId>` per call, but every other
    /// caller of this constructor still benefits.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        }
    }

    /// Returns this selection with `body_color_override` replaced by `rgb` -- every
    /// other field carries through unchanged, EXCEPT the N-band form (bands and path
    /// scale), which is dropped: it would win over the new triple. `None` restores the
    /// material's own color.
    #[must_use]
    pub fn with_body_color(mut self, rgb: Option<[f32; 3]>) -> Self {
        self.body_color_override = rgb;
        self.body_color_bands_override = None;
        self.absorption_path_scale_override = None;
        self
    }

    /// Returns this selection with the N-band colour set: the nearest triple in
    /// `body_color_override` (for older builds), the band rows and the path scale they
    /// were solved for. `bands = None` clears both band fields (the triple is left to the
    /// caller, see [`Self::with_body_color`]).
    #[must_use]
    pub fn with_body_color_bands(
        mut self,
        triple: Option<[f32; 3]>,
        bands: Option<Vec<[f32; 3]>>,
        path_scale: Option<f32>,
    ) -> Self {
        self.body_color_override = triple;
        self.absorption_path_scale_override = bands.as_ref().and(path_scale);
        self.body_color_bands_override = bands;
        self
    }

    /// Whether this selection recolors its material (see
    /// [`Self::body_color_override`]).
    #[must_use]
    pub const fn has_body_color_override(&self) -> bool {
        self.body_color_override.is_some()
    }

    /// A short label for [`Self::body_color_override`]: the matching preset's own
    /// label (`"Yellow"`), `"custom color"` for a triple that matches no preset, or
    /// `None` when no override is set. Used to name a recolored material
    /// (`Sapphire (Yellow)`) in the undo history and the render readout.
    #[must_use]
    pub fn body_color_label(&self) -> Option<&'static str> {
        self.body_color_override
            .map(|rgb| preset_label_for_rgb(rgb).unwrap_or("custom color"))
    }

    /// `gem` with this selection's per-design overrides applied: the
    /// `refractive_index_override` as a flat (non-dispersive) Cauchy fit at exactly
    /// the typed value, then the `body_color_override` via
    /// `GemMaterial::with_body_color`. With neither set, `gem` comes back
    /// unchanged.
    ///
    /// The ONE place a design's selection turns a looked-up material into the stone
    /// the optimizer, tilt curve, live render and export all see -- every
    /// "selection -> `GemMaterial`" helper routes through this, so an override can
    /// never reach one consumer and miss another. An RI override is, by
    /// construction, a single typed index with no accompanying dispersion data, so
    /// "flat at the override" is the only honest curve to draw.
    #[must_use]
    pub fn apply_overrides(&self, mut gem: GemMaterial) -> GemMaterial {
        if let Some(n_d) = self.refractive_index_override {
            gem.dispersion = DispersionModel::Cauchy {
                a: n_d as f32,
                b: 0.0,
                c: 0.0,
            };
        }
        if let Some(bands) = self
            .body_color_bands_override
            .as_deref()
            .filter(|b| !b.is_empty())
        {
            // The N-band form wins over the triple stored next to it (the triple is only
            // there so an older build still shows a close colour).
            let scale = self
                .absorption_path_scale_override
                .unwrap_or(gem.absorption_path_scale);
            gem = gem.with_body_color_bands(bands, scale);
        } else if let Some(rgb) = self.body_color_override {
            // Note: with_body_color replaces the entire absorption tensor with 3 isotropic
            // legacy bands, which replaces any physics chromophore absorption. The desktop
            // therefore greys the override control out for a physics material
            // ("replaces the physical color", `EditorModel.design_material_is_physics`) and
            // warns when one is already set; a physics override is a follow-up.
            gem = gem.with_body_color(rgb);
        }
        gem
    }

    /// Returns a copy of this selection with only `specific_gravity_override`
    /// replaced -- `name` and `refractive_index_override` carry through unchanged.
    /// Exists so a caller that owns just the SG field (e.g. the Yield form parser,
    /// which has no RI-override field of its own) never has to reconstruct this
    /// whole struct by hand and risk silently dropping `refractive_index_override`
    /// to `None` in the process, exactly the bug this method closes.
    #[must_use]
    pub fn with_specific_gravity_override(&self, specific_gravity_override: Option<f64>) -> Self {
        Self {
            specific_gravity_override,
            ..self.clone()
        }
    }

    /// The specific gravity actually in effect: the override when present, else the
    /// selected preset's own representative figure, else `None` (nothing to estimate
    /// a carat weight from -- see [`crate::yield_metrics::YieldReport`]).
    ///
    /// Built-ins only -- see [`Self::effective_specific_gravity_with`] for a
    /// catalogue-aware version that also resolves a CUSTOM material's own SG.
    #[must_use]
    pub fn effective_specific_gravity(&self) -> Option<f64> {
        self.specific_gravity_override.or_else(|| {
            self.name
                .as_deref()
                .and_then(built_in_specific_gravity)
                .map(|sg| sg.representative)
        })
    }

    /// Like [`Self::effective_specific_gravity`], but resolves `name` through
    /// `catalogue` (see [`MaterialLookup::specific_gravity`]) instead of only this
    /// crate's own built-in table, so a CUSTOM catalogue material's authored SG
    /// reaches the carat-weight estimate too. The per-design override still wins over
    /// everything, exactly as in the built-ins-only path.
    #[must_use]
    pub fn effective_specific_gravity_with(&self, catalogue: &dyn MaterialLookup) -> Option<f64> {
        self.specific_gravity_override.or_else(|| {
            self.name
                .as_deref()
                .and_then(|name| catalogue.specific_gravity(name))
        })
    }

    /// Resolves this selection to a real [`GemMaterial`] plus its refractive index
    /// and critical angle, via `catalogue` (see [`MaterialLookup`]).
    ///
    /// Always returns something usable: an absent or unrecognized `name` falls back
    /// to [`GemMaterial::diamond`] rather than failing a caller that needs SOME
    /// concrete material to run against. `refractive_index_override` always wins
    /// over the resolved material's own `n_D` when present.
    #[must_use]
    pub fn resolve(&self, catalogue: &dyn MaterialLookup) -> ResolvedMaterial {
        let gem = self
            .name
            .as_deref()
            .and_then(|name| catalogue.lookup(name))
            .unwrap_or_else(GemMaterial::diamond);
        let n_d = self
            .refractive_index_override
            .unwrap_or_else(|| n_d_of(&gem));
        ResolvedMaterial {
            critical_angle_deg: critical_angle_deg(n_d),
            n_d,
            gem,
        }
    }
}

/// A material's refractive index at the sodium D line (589.3nm), read from its own
/// dispersion curve -- the one place this module evaluates `GemMaterial::dispersion`,
/// shared by [`MaterialSelection::resolve`] and [`built_in_refractive_index`].
///
/// Visible across [`crate::material`]'s submodules only: [`super::catalogue`] needs it
/// too, for [`super::catalogue::MaterialEntry::ri_d`].
pub(in crate::material) fn n_d_of(gem: &GemMaterial) -> f64 {
    f64::from(gem.dispersion.evaluate(589.3))
}

/// Looks up a built-in material by name, matched EXACTLY (case-insensitively
/// only) against `indicatrix::optics::materials::GemMaterial::all_materials`.
///
/// Deliberately NOT [`GemMaterial::by_name`], whose own substring fallback would
/// resolve a query like "My Blue Sapphire" to Sapphire just because it CONTAINS
/// that name -- wrong for anything treating a match here as "this design's
/// material really is this built-in species" (a diagram title mentioning a gem in
/// passing is not the same claim). See `by_name`'s own doc comment for why that
/// fallback exists there (a looser, diagram-title-tolerant lookup a different
/// caller wants); this crate's own material resolution needs the stricter one.
#[must_use]
pub fn built_in_material_by_exact_name(name: &str) -> Option<GemMaterial> {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
}

/// Looks up a built-in material's own `n_D` by exactly the name
/// `indicatrix::optics::materials::GemMaterial::name` uses for that preset.
///
/// The refractive-index counterpart to [`super::built_in_specific_gravity`], built-ins
/// only. Used by [`crate::design::Design::effective_refractive_index`], which
/// (unlike [`MaterialSelection::resolve`]) has no catalogue to consult and must fall
/// back to a design's legacy schedule RI rather than to diamond when `name` is
/// absent or unrecognized.
#[must_use]
pub fn built_in_refractive_index(material_name: &str) -> Option<f64> {
    built_in_material_by_exact_name(material_name).map(|gem| n_d_of(&gem))
}

/// Resolves a preset NAME to a real [`GemMaterial`].
///
/// The seam that keeps this crate free of a dependency on `indicatrix-vault` while
/// still letting a caller with a richer catalogue (built-ins plus custom,
/// user-authored materials) plug its own lookup into [`MaterialSelection::resolve`].
pub trait MaterialLookup {
    /// Looks up `name` (exactly a `GemMaterial::name`-style string), or `None` if
    /// this catalogue has nothing by that name.
    fn lookup(&self, name: &str) -> Option<GemMaterial>;

    /// Looks up `name`'s specific gravity, or `None` if this catalogue has no SG
    /// on file for it. Defaults to `None` so an existing implementor keeps compiling
    /// unchanged; a catalogue that actually stores custom-material SG (e.g. the
    /// app's `EditorMaterialLookup`) overrides this to read it back, the same way
    /// [`BuiltinMaterials`] overrides it for the built-in table via
    /// [`super::built_in_specific_gravity`].
    fn specific_gravity(&self, name: &str) -> Option<f64> {
        let _ = name;
        None
    }
}

/// A [`MaterialLookup`] over exactly `indicatrix::optics::materials::GemMaterial`'s
/// own built-in preset table, no custom/catalogue materials at all -- via
/// [`GemMaterial::by_name`].
///
/// Lets a caller with no real catalogue on hand resolve a [`MaterialSelection`]
/// immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuiltinMaterials;

impl MaterialLookup for BuiltinMaterials {
    fn lookup(&self, name: &str) -> Option<GemMaterial> {
        GemMaterial::by_name(name)
    }

    fn specific_gravity(&self, name: &str) -> Option<f64> {
        built_in_specific_gravity(name).map(|sg| sg.representative)
    }
}

/// What [`MaterialSelection::resolve`] produces.
///
/// A real [`GemMaterial`] (for the optimizer/renderer/tilt curves) alongside the
/// scalar figures [`crate::optics_hints`] needs, computed once so a caller never
/// has to re-derive `n_d`/the critical angle from the material by hand.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMaterial {
    /// Underlying gem material definition.
    pub gem: GemMaterial,
    /// Refractive index at the sodium D line: `refractive_index_override` when
    /// present, else `gem`'s own dispersion curve evaluated at 589.3nm.
    pub n_d: f64,
    /// [`crate::optics_hints::critical_angle_deg`] at `n_d`.
    pub critical_angle_deg: f64,
}

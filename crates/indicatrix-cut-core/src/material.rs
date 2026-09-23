//! Specific gravity (density) data for the carat-weight estimate.
//!
//! See [`crate::yield_metrics`]'s module docs for how this feeds that estimate, and
//! for why it is kept strictly separate from volumetric yield (which needs no
//! material data at all).
//!
//! # This is new data
//!
//! Neither `indicatrix::optics::materials` (optical properties only: dispersion,
//! absorption, birefringence) nor the `.asc`/catalogue schema carries specific
//! gravity, density, or carat figures anywhere -- the table below is genuinely new
//! data, not a rewiring of something that already existed.
//!
//! # Provenance
//!
//! Unlike `indicatrix::optics::materials`'s single-paper-traceable dispersion fits,
//! specific gravity has no equivalent primary literature for most gem species --
//! these are textbook constants from standard gemological reference tables (Mineral
//! Data Publishing's "Handbook of Mineralogy" and the International Gem Society
//! property tables, both already cited elsewhere in `indicatrix::optics::materials`).
//!
//! Keyed by exactly the strings `indicatrix::optics::materials::GemMaterial::name`
//! uses, so a caller already holding one of those names can look its SG up directly.
//!
//! # Resolving a selection to a real material and refractive index
//!
//! [`MaterialSelection::resolve`] turns this module's preset-name-plus-overrides model
//! into a real `indicatrix::optics::materials::GemMaterial` plus its refractive index
//! at the sodium D line (`n_d`) and the critical angle that follows from it. It takes
//! the lookup as a [`MaterialLookup`] trait object rather than depending on
//! `indicatrix-vault` directly, with [`BuiltinMaterials`] as the built-ins-only
//! implementation available with no full catalogue wired up.
//!
//! # Ranges
//!
//! Unlike refractive index, SG is often substantially sensitive to trace/major-element
//! substitution across a compositional solid-solution series. Two entries carry a
//! real cited range for that reason: **Zircon** (ordinary crystalline "high" zircon
//! ~4.6-4.7; radiation-damaged "low"/metamict as low as ~3.90) and **Tourmaline**
//! (the elbaite entry `indicatrix::optics::materials` models optically is one
//! composition in a larger dravite/schorl solid-solution family; SG climbs with iron
//! content across it). [`SpecificGravity::representative`] is always the midpoint of
//! the cited spread, not a blind average of the family's theoretical extremes.
//!
//! There is no "Garnet" preset at all: `GemMaterial::all_materials` covers thirteen
//! named species only, and adding an SG row with no corresponding optical entry would
//! let a material picker offer a species it can never actually render.
//! [`MaterialSelection::specific_gravity_override`] exists precisely so a user cutting
//! a species this table has no entry for (garnet included) can type their own
//! specimen's known or estimated SG directly.

use crate::optics_hints::critical_angle_deg;
use indicatrix::optics::materials::{GemMaterial, OpticalCharacter};

/// One material's specific gravity.
///
/// A representative point figure for [`crate::yield_metrics::carat_weight`], plus the
/// real low/high bounds it simplifies away for a species with a genuine compositional
/// range (see this module's doc comment). `range` equals `(representative,
/// representative)` when published sources agree to within rounding, not because no
/// natural variation exists at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecificGravity {
    /// The single number [`crate::yield_metrics::carat_weight`] multiplies by,
    /// absent a per-design override.
    pub representative: f64,
    /// `(low, high)` -- the cited real-world spread this representative figure
    /// simplifies. Always `low <= representative <= high`.
    pub range: (f64, f64),
}

/// Looks up a built-in specific gravity by exactly the name
/// `indicatrix::optics::materials::GemMaterial::name` uses for that preset.
///
/// See this module's doc comment for the full covered list and provenance. `None`
/// for any name outside that list, notably "Garnet", which has no built-in preset.
#[must_use]
pub fn built_in_specific_gravity(material_name: &str) -> Option<SpecificGravity> {
    // Sources: Handbook of Mineralogy / IGS (see module doc comment), except where
    // noted. Zircon/Tourmaline carry a real compositional range -- see "Ranges"
    // above; representative there is a deliberate midpoint choice, not an average.
    let (representative, range) = match material_name {
        "Diamond" => (3.52, (3.50, 3.53)),
        // Sapphire & Ruby: same corundum host lattice, same SG.
        "Sapphire" | "Ruby" => (4.00, (3.99, 4.10)),
        // Emerald-specific beryl figure (beryl overall runs wider, 2.63-2.92).
        "Emerald" => (2.76, (2.67, 2.78)),
        // Representative leans toward high (crystalline) zircon (4.65) to match
        // this crate's Zircon optical entry (gem-trade "starlite" pairing).
        "Zircon" => (4.65, (3.90, 4.73)),
        "Alexandrite" => (3.73, (3.68, 3.78)),
        // OH-rich/F-rich ends of the solid solution shift SG slightly.
        "Topaz" => (3.53, (3.49, 3.57)),
        "Spinel" => (3.60, (3.58, 3.61)),
        "Quartz" => (2.65, (2.65, 2.66)),
        // Representative (3.06) is elbaite's own midpoint (the species modeled
        // optically); the cited range spans the whole dravite/schorl family since a
        // cutter may not know which tourmaline species their rough is.
        "Tourmaline" => (3.06, (2.82, 3.32)),
        "Tanzanite" => (3.35, (3.15, 3.36)),
        "Synthetic Moissanite" => (3.22, (3.21, 3.22)),
        // Synthetic; SG varies with stabilizer content.
        "Cubic Zirconia" => (5.80, (5.60, 6.00)),
        _ => return None,
    };
    Some(SpecificGravity {
        representative,
        range,
    })
}

/// A design's material choice for the carat-weight estimate.
///
/// Which built-in preset (if any) to default from, plus an optional per-design
/// override -- SG varies by variety and specimen even within one named species, so a
/// design must always be able to state its own number regardless of what (if
/// anything) [`built_in_specific_gravity`] knows about the selected name.
///
/// Stored on [`crate::design::Design`] like [`crate::preform::PreformSpec`] -- a
/// design input, mutated only through [`crate::edit::History`].
#[derive(Debug, Clone, PartialEq, Default)]
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
        }
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
fn n_d_of(gem: &GemMaterial) -> f64 {
    f64::from(gem.dispersion.evaluate(589.3))
}

/// Looks up a built-in material's own `n_D` by exactly the name
/// `indicatrix::optics::materials::GemMaterial::name` uses for that preset.
///
/// The refractive-index counterpart to [`built_in_specific_gravity`], built-ins
/// only. Used by [`crate::design::Design::effective_refractive_index`], which
/// (unlike [`MaterialSelection::resolve`]) has no catalogue to consult and must fall
/// back to a design's legacy schedule RI rather than to diamond when `name` is
/// absent or unrecognized.
#[must_use]
pub fn built_in_refractive_index(material_name: &str) -> Option<f64> {
    GemMaterial::by_name(material_name).map(|gem| n_d_of(&gem))
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
    /// [`built_in_specific_gravity`].
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
    pub gem: GemMaterial,
    /// Refractive index at the sodium D line: `refractive_index_override` when
    /// present, else `gem`'s own dispersion curve evaluated at 589.3nm.
    pub n_d: f64,
    /// [`crate::optics_hints::critical_angle_deg`] at `n_d`.
    pub critical_angle_deg: f64,
}

/// Where a [`MaterialEntry`] came from: one of [`GemMaterial::all_materials`]'s
/// built-in presets, or a user-authored material from the vault's custom-material
/// catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialKind {
    /// One of `GemMaterial::all_materials()`'s presets.
    BuiltIn,
    /// A user-authored material saved through the material editor dialog.
    Custom,
}

/// One material a picker may offer.
///
/// Covers the CAD editor's design-settings combo, the New Design dialog, and the
/// live-render viewport's Render Material combo -- enough of its optics is
/// summarized here to describe it without re-deriving them from the resolved
/// [`GemMaterial`] at every call site.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialEntry {
    /// Exactly the [`GemMaterial::name`] this entry resolves through
    /// [`MaterialLookup::lookup`].
    pub name: String,
    /// Where this entry came from -- see [`MaterialKind`].
    pub kind: MaterialKind,
    /// Refractive index at the sodium D line (589.3nm) -- see [`ResolvedMaterial::n_d`].
    pub ri_d: f64,
    /// This material's specific gravity, when one is on file: the built-in table
    /// ([`built_in_specific_gravity`]) for a [`MaterialKind::BuiltIn`] entry, or the
    /// vault's saved figure (threaded through by [`MaterialCatalogue::build_with_sg`])
    /// for a [`MaterialKind::Custom`] one. `None` for a built-in this crate has no SG
    /// row for (a species added to `GemMaterial::all_materials()` after this table was
    /// last extended) or a custom material saved with no SG recorded.
    pub sg: Option<f64>,
    /// Whether this material has any birefringence at all -- `false` only for an
    /// isotropic material (cubic crystal system, or a custom material saved with zero
    /// birefringence). Drives the same "is the crystal-axis control meaningful"
    /// question `gui::startup_settings::is_c_axis_override_available` answers from a
    /// resolved `GemMaterial` directly.
    pub birefringent: bool,
    /// Whether this material carries any absorption bands on any ray at all -- `false`
    /// for the handful of built-ins with zero absorption (Diamond, Synthetic
    /// Moissanite, Cubic Zirconia) and for any custom material saved with a fully
    /// transparent (black) absorption color.
    pub has_absorption: bool,
}

/// The single source of truth for "every material a picker may offer".
///
/// Every built-in [`GemMaterial::all_materials`] preset, in that function's own
/// order, followed by every custom catalogue material, sorted by name
/// (case-insensitive).
///
/// This type is the one list a caller on either side can build from. Both the
/// live-render viewport's Render Material combo and the CAD editor's design-settings
/// combo now read from this catalogue, so a species present in one picker and not the
/// other cannot happen by construction.
///
/// [bpn]: ../../indicatrix_cut/gui/editor/state/fn.builtin_preset_names.html
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialCatalogue {
    entries: Vec<MaterialEntry>,
}

impl MaterialCatalogue {
    /// Builds the catalogue from `custom` (already-resolved custom materials, e.g.
    /// `RenderContext::custom_materials`/`EditorMaterialLookup`'s own source) --
    /// built-ins first in [`GemMaterial::all_materials`]'s own order, then `custom`
    /// sorted by name (case-insensitive; ties keep `custom`'s own relative order,
    /// since [`Vec::sort_by`] is stable).
    ///
    /// A custom material's [`MaterialEntry::sg`] is always `None` here -- `custom`'s
    /// own `GemMaterial` carries no specific-gravity field at all (see
    /// `crate::material`'s module doc comment on why SG is this crate's own,
    /// separate table). Use [`Self::build_with_sg`] when a caller also has the
    /// vault's per-custom-material SG side channel on hand (e.g.
    /// `RenderContext::custom_material_specific_gravity`) and wants it reflected.
    #[must_use]
    pub fn build(custom: &[GemMaterial]) -> Self {
        Self::build_with_sg(custom, &[])
    }

    /// Like [`Self::build`], but resolves each custom entry's [`MaterialEntry::sg`]
    /// from `custom_sg` -- a `(name, sg)` list matching `RenderContext::
    /// custom_material_specific_gravity`'s own shape (case-insensitive name match,
    /// first match wins).
    #[must_use]
    pub fn build_with_sg(custom: &[GemMaterial], custom_sg: &[(String, f64)]) -> Self {
        let mut entries: Vec<MaterialEntry> = GemMaterial::all_materials()
            .iter()
            .map(|gem| {
                let sg = built_in_specific_gravity(&gem.name).map(|sg| sg.representative);
                material_entry(gem, MaterialKind::BuiltIn, sg)
            })
            .collect();
        let mut customs: Vec<MaterialEntry> = custom
            .iter()
            .map(|gem| {
                let sg = custom_sg
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(&gem.name))
                    .map(|(_, sg)| *sg);
                material_entry(gem, MaterialKind::Custom, sg)
            })
            .collect();
        customs.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        entries.extend(customs);
        Self { entries }
    }

    /// Every entry, in this catalogue's own stable order (built-ins, then customs
    /// sorted by name).
    #[must_use]
    pub fn entries(&self) -> &[MaterialEntry] {
        &self.entries
    }

    /// Just the names, in the same order as [`Self::entries`] -- what a `ComboBox`
    /// model needs.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    /// Finds an entry by name, case-insensitively -- the same matching convention
    /// [`BuiltinMaterials::lookup`]/`EditorMaterialLookup::lookup` already use.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&MaterialEntry> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// How many entries this catalogue holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether this catalogue has no entries at all -- only possible for a
    /// hypothetical empty `GemMaterial::all_materials()`, since [`Self::build`]
    /// always includes every built-in.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Builds one [`MaterialEntry`] from a resolved `gem`, shared by [`MaterialCatalogue::
/// build_with_sg`]'s built-in and custom passes.
fn material_entry(gem: &GemMaterial, kind: MaterialKind, sg: Option<f64>) -> MaterialEntry {
    let has_absorption = !gem.absorption.o_ray.is_empty()
        || !gem.absorption.e_ray.is_empty()
        || gem
            .absorption
            .beta_ray
            .as_ref()
            .is_some_and(|bands| !bands.is_empty());
    MaterialEntry {
        name: gem.name.clone(),
        kind,
        ri_d: n_d_of(gem),
        sg,
        birefringent: gem.optical_character != OpticalCharacter::Isotropic,
        has_absorption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_preset_names_resolve() {
        for name in [
            "Diamond",
            "Sapphire",
            "Ruby",
            "Emerald",
            "Zircon",
            "Alexandrite",
            "Topaz",
            "Spinel",
            "Quartz",
            "Tourmaline",
            "Tanzanite",
            "Synthetic Moissanite",
            "Cubic Zirconia",
        ] {
            let sg = built_in_specific_gravity(name)
                .unwrap_or_else(|| panic!("{name} must have a built-in SG"));
            assert!(sg.representative > 0.0, "{name}: {sg:?}");
            assert!(
                sg.range.0 <= sg.representative && sg.representative <= sg.range.1,
                "{name}: representative {} not inside its own range {:?}",
                sg.representative,
                sg.range
            );
        }
    }

    /// There is no "Garnet" preset (or any other name outside the thirteen above).
    #[test]
    fn unknown_names_including_garnet_resolve_to_none() {
        assert_eq!(built_in_specific_gravity("Garnet"), None);
        assert_eq!(built_in_specific_gravity("Not A Real Material"), None);
        assert_eq!(built_in_specific_gravity(""), None);
    }

    #[test]
    fn none_selection_has_no_effective_sg() {
        assert_eq!(MaterialSelection::none().effective_specific_gravity(), None);
        assert_eq!(MaterialSelection::default(), MaterialSelection::none());
    }

    #[test]
    fn preset_alone_uses_its_representative_figure() {
        let m = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        assert_eq!(m.effective_specific_gravity(), Some(3.52));
    }

    /// An override wins even when the preset name is also known -- the user's own
    /// number is always authoritative over the table.
    #[test]
    fn override_wins_over_a_known_preset() {
        let m = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: Some(3.515),
            refractive_index_override: None,
        };
        assert_eq!(m.effective_specific_gravity(), Some(3.515));
    }

    /// An override is exactly what makes a species with no preset (garnet) usable.
    #[test]
    fn override_alone_works_for_a_species_with_no_preset() {
        let m = MaterialSelection {
            name: Some("Garnet".to_string()),
            specific_gravity_override: Some(3.90),
            refractive_index_override: None,
        };
        assert_eq!(m.effective_specific_gravity(), Some(3.90));
    }

    // --- MaterialSelection::with_specific_gravity_override ---

    /// The one field named in the call must change; `name` and
    /// `refractive_index_override` must carry through untouched -- the exact
    /// property that makes this safe for a caller (the Yield form parser) that
    /// owns only the SG field to use without reconstructing the whole struct.
    #[test]
    fn with_specific_gravity_override_replaces_only_that_field() {
        let original = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: Some(3.50),
            refractive_index_override: Some(2.42),
        };
        let updated = original.with_specific_gravity_override(Some(3.55));
        assert_eq!(updated.specific_gravity_override, Some(3.55));
        assert_eq!(updated.name, original.name);
        assert_eq!(
            updated.refractive_index_override,
            original.refractive_index_override
        );
    }

    /// Passing `None` clears the override (back to "use the preset's own figure")
    /// while still leaving `name`/`refractive_index_override` alone.
    #[test]
    fn with_specific_gravity_override_can_clear_the_override() {
        let original = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: Some(2.70),
            refractive_index_override: Some(1.55),
        };
        let updated = original.with_specific_gravity_override(None);
        assert_eq!(updated.specific_gravity_override, None);
        assert_eq!(updated.name, Some("Quartz".to_string()));
        assert_eq!(updated.refractive_index_override, Some(1.55));
    }

    // --- MaterialSelection::resolve / built_in_refractive_index ---

    #[test]
    fn resolve_a_known_preset_returns_that_material_and_its_own_n_d() {
        let m = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let resolved = m.resolve(&BuiltinMaterials);
        assert_eq!(
            resolved.gem.name,
            GemMaterial::by_name("Quartz").unwrap().name
        );
        let expected_n_d = f64::from(
            GemMaterial::by_name("Quartz")
                .unwrap()
                .dispersion
                .evaluate(589.3),
        );
        assert!((resolved.n_d - expected_n_d).abs() < 1e-9);
        assert!((resolved.critical_angle_deg - critical_angle_deg(expected_n_d)).abs() < 1e-9);
    }

    /// No selection at all (a brand-new design) resolves to diamond.
    #[test]
    fn resolve_with_no_name_falls_back_to_diamond() {
        let resolved = MaterialSelection::none().resolve(&BuiltinMaterials);
        assert_eq!(resolved.gem.name, GemMaterial::diamond().name);
    }

    /// A name `BuiltinMaterials` does not recognize falls back to diamond too,
    /// rather than panicking or silently doing nothing.
    #[test]
    fn resolve_with_an_unrecognized_name_falls_back_to_diamond() {
        let m = MaterialSelection {
            name: Some("Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let resolved = m.resolve(&BuiltinMaterials);
        assert_eq!(resolved.gem.name, GemMaterial::diamond().name);
    }

    /// `refractive_index_override` must win over the resolved material's own `n_D`.
    #[test]
    fn resolve_refractive_index_override_wins_over_the_resolved_material() {
        let m = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.70),
        };
        let resolved = m.resolve(&BuiltinMaterials);
        assert!((resolved.n_d - 1.70).abs() < 1e-12);
        assert!((resolved.critical_angle_deg - critical_angle_deg(1.70)).abs() < 1e-9);
    }

    #[test]
    fn built_in_refractive_index_matches_the_material_own_dispersion_curve() {
        let expected = f64::from(GemMaterial::diamond().dispersion.evaluate(589.3));
        assert!((built_in_refractive_index("Diamond").unwrap() - expected).abs() < 1e-9);
        assert_eq!(built_in_refractive_index("Garnet"), None);
        assert_eq!(built_in_refractive_index("Not A Real Material"), None);
    }

    // --- MaterialLookup::specific_gravity / effective_specific_gravity_with ---

    /// [`BuiltinMaterials::specific_gravity`] must agree with
    /// [`built_in_specific_gravity`] for every known preset.
    #[test]
    fn builtin_materials_specific_gravity_matches_the_table() {
        assert_eq!(BuiltinMaterials.specific_gravity("Diamond"), Some(3.52));
        assert_eq!(BuiltinMaterials.specific_gravity("Garnet"), None);
    }

    /// A [`MaterialLookup`] that never overrides [`MaterialLookup::specific_gravity`]
    /// keeps compiling and simply reports `None` -- the default-method contract
    /// [`MaterialLookup::specific_gravity`]'s own doc comment promises.
    #[test]
    fn a_lookup_with_no_sg_override_reports_none() {
        struct LookupWithNoSg;
        impl MaterialLookup for LookupWithNoSg {
            fn lookup(&self, name: &str) -> Option<GemMaterial> {
                GemMaterial::by_name(name)
            }
        }
        assert_eq!(LookupWithNoSg.specific_gravity("Diamond"), None);
    }

    /// [`MaterialSelection::effective_specific_gravity_with`] must match the
    /// built-ins-only [`MaterialSelection::effective_specific_gravity`] when the
    /// catalogue is [`BuiltinMaterials`].
    #[test]
    fn effective_specific_gravity_with_matches_built_ins_only_path() {
        let m = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        assert_eq!(
            m.effective_specific_gravity_with(&BuiltinMaterials),
            m.effective_specific_gravity()
        );
    }

    /// The per-design override still wins over whatever the catalogue reports.
    #[test]
    fn effective_specific_gravity_with_override_wins_over_the_catalogue() {
        let m = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: Some(3.515),
            refractive_index_override: None,
        };
        assert_eq!(
            m.effective_specific_gravity_with(&BuiltinMaterials),
            Some(3.515)
        );
    }

    /// A catalogue that DOES know a custom material's SG (unlike the built-ins-only
    /// path, which has no such entry) must have it reach the effective figure.
    #[test]
    fn effective_specific_gravity_with_resolves_a_custom_material_the_built_in_table_cannot() {
        struct CustomOnlyLookup;
        impl MaterialLookup for CustomOnlyLookup {
            fn lookup(&self, name: &str) -> Option<GemMaterial> {
                GemMaterial::by_name(name)
            }
            fn specific_gravity(&self, name: &str) -> Option<f64> {
                (name == "My Garnet").then_some(3.90)
            }
        }
        let m = MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        assert_eq!(m.effective_specific_gravity(), None);
        assert_eq!(
            m.effective_specific_gravity_with(&CustomOnlyLookup),
            Some(3.90)
        );
    }

    // --- MaterialCatalogue ---

    /// Every one of `GemMaterial::all_materials()`'s built-ins must appear, in that
    /// function's own order -- the whole point of this type: a species available on
    /// the render side can no longer be missing from a picker built off this
    /// catalogue.
    #[test]
    fn catalogue_lists_every_builtin_in_all_materials_order() {
        let catalogue = MaterialCatalogue::build(&[]);
        let expected: Vec<String> = GemMaterial::all_materials()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(catalogue.names(), expected);
        assert!(
            catalogue
                .entries()
                .iter()
                .all(|e| e.kind == MaterialKind::BuiltIn)
        );
    }

    /// Customs are appended after every built-in, sorted by name case-insensitively
    /// -- not in `custom`'s own (arbitrary, load-order) sequence.
    #[test]
    fn catalogue_appends_customs_sorted_by_name_after_every_builtin() {
        let mut a = GemMaterial::diamond();
        a.name = "Zircon-ish Garnet".to_string();
        let mut b = GemMaterial::diamond();
        b.name = "amber quartz".to_string();
        let catalogue = MaterialCatalogue::build(&[a, b]);
        let names = catalogue.names();
        let builtin_count = GemMaterial::all_materials().len();
        assert_eq!(names.len(), builtin_count + 2);
        // Case-insensitive sort: "amber quartz" sorts before "Zircon-ish Garnet"
        // despite its lowercase leading letter.
        assert_eq!(names[builtin_count], "amber quartz");
        assert_eq!(names[builtin_count + 1], "Zircon-ish Garnet");
        assert_eq!(
            catalogue.entries()[builtin_count].kind,
            MaterialKind::Custom
        );
    }

    #[test]
    fn catalogue_find_is_case_insensitive() {
        let catalogue = MaterialCatalogue::build(&[]);
        let found = catalogue.find("diamond").expect("Diamond must resolve");
        assert_eq!(found.name, "Diamond");
        assert!(catalogue.find("Not A Real Material").is_none());
    }

    /// Every built-in entry's `ri_d`/`birefringent` must agree with the resolved
    /// `GemMaterial` itself -- the catalogue must describe the real material, not an
    /// approximation of it.
    #[test]
    fn catalogue_entries_carry_the_real_material_ri_and_birefringence() {
        let catalogue = MaterialCatalogue::build(&[]);
        let diamond = catalogue.find("Diamond").unwrap();
        assert!((diamond.ri_d - 2.417).abs() < 0.01);
        assert!(!diamond.birefringent, "Diamond is isotropic");
        assert!(!diamond.has_absorption, "Diamond has no absorption bands");

        let sapphire = catalogue.find("Sapphire").unwrap();
        assert!(sapphire.birefringent, "Sapphire is uniaxial");
        assert!(
            sapphire.has_absorption,
            "Sapphire carries real absorption bands"
        );
    }

    /// A built-in the SG table covers reports it; a custom material with no SG side
    /// channel reports `None` rather than a fabricated figure.
    #[test]
    fn catalogue_sg_is_populated_for_known_builtins_and_none_for_an_unrecorded_custom() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let catalogue = MaterialCatalogue::build(&[custom]);
        assert_eq!(catalogue.find("Diamond").unwrap().sg, Some(3.52));
        assert_eq!(catalogue.find("My Garnet").unwrap().sg, None);
    }

    /// [`MaterialCatalogue::build_with_sg`] resolves a custom entry's SG from the
    /// caller-supplied side channel, case-insensitively.
    #[test]
    fn catalogue_build_with_sg_resolves_a_custom_materials_specific_gravity() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let sg_entries = [("my garnet".to_string(), 3.90)];
        let catalogue = MaterialCatalogue::build_with_sg(&[custom], &sg_entries);
        assert_eq!(catalogue.find("My Garnet").unwrap().sg, Some(3.90));
    }
}

//! [`MaterialCatalogue`]: the single source of truth for "every material a picker may
//! offer", built-ins plus custom materials -- see [`crate::material`]'s module doc
//! comment for the wider picture.

use super::{selection::n_d_of, specific_gravity::built_in_specific_gravity};
use indicatrix::optics::materials::{GemMaterial, OpticalCharacter};

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
    /// [`super::MaterialLookup::lookup`].
    pub name: String,
    /// Where this entry came from -- see [`MaterialKind`].
    pub kind: MaterialKind,
    /// Refractive index at the sodium D line (589.3nm) -- see [`super::ResolvedMaterial::n_d`].
    pub ri_d: f64,
    /// This material's specific gravity, when one is on file: the built-in table
    /// ([`super::built_in_specific_gravity`]) for a [`MaterialKind::BuiltIn`] entry, or the
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
/// [bpn]: ../../../indicatrix_cut/gui/editor/state/fn.builtin_preset_names.html
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
    /// [`super::BuiltinMaterials::lookup`]/`EditorMaterialLookup::lookup` already use.
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

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
//! The SG table covers fourteen species (Rutile, the synthetic reference material, has
//! its own row), a subset of `GemMaterial::all_materials`: the optical list also holds
//! species (the individual garnets among them) that have no SG row yet. There is no
//! generic "Garnet" preset, and an SG row with no corresponding optical entry would let
//! a material picker offer a species it can never actually render.
//! [`MaterialSelection::specific_gravity_override`] exists precisely so a user cutting
//! a species this table has no entry for can type their own specimen's known or
//! estimated SG directly.

mod catalogue;
pub mod color;
mod library_row;
mod selection;
mod specific_gravity;

#[cfg(test)]
mod tests;

pub use catalogue::{MaterialCatalogue, MaterialEntry, MaterialKind};
pub use color::{ActiveColor, ColorMode, RecipeHistory};
pub use library_row::{
    LibraryMaterial, absorption_bands_from_json, absorption_bands_to_json,
    crystal_system_from_name, crystal_system_name, optical_character_from_name,
    optical_character_name, with_library_bands,
};
pub use selection::{
    BuiltinMaterials, MaterialLookup, MaterialSelection, ResolvedMaterial,
    built_in_material_by_exact_name, built_in_refractive_index,
};
pub use specific_gravity::{SpecificGravity, built_in_specific_gravity};

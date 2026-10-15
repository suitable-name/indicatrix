//! The conversions behind the `.isdd` design file (Indicatrix Suite Design Document;
//! files saved as `.indicatrix` before are read the same way), plus the catalogue `.asc`
//! text a save stores next to it.
//!
//! The on-disk document shape itself -- the schema, TOML encode/decode and path
//! rules -- lives in [`indicatrix_formats::native`], a dependency-free-of-editor-types
//! format crate this module re-exports from (see that module's own doc comment for the
//! format and "the gap this closes"). What stays HERE is everything that needs this
//! crate's own editor types in scope: [`convert`]'s free functions between the on-disk
//! mirror types and [`crate::design::Design`]/[`crate::preform::PreformSpec`]/
//! [`crate::material::MaterialSelection`]/`indicatrix`'s own
//! [`indicatrix::geometry::meet_solver::MeetConstraint`] (see [`convert`]'s own doc
//! comment for why these are free functions, not `From`/`Into` impls), and the two
//! directions built on top of them: [`design_to_file`]/[`design_from_file`] and
//! [`save`]'s [`catalogue_asc`].
//!
//! # The self-contained design file
//!
//! [`design_to_file`]/[`design_from_file`] convert a design to and from the single-file
//! `.isdd` format ([`indicatrix_formats::native::design`]), which carries the whole
//! design and needs no paired `.asc`. There is no other native file: the older
//! `.indicatrix.toml` overlay sidecar, which needed its `.asc` next to it, is not read
//! or written any more.
//!
//! # Preserving the original `.asc` text
//!
//! [`indicatrix_formats::asc::to_asc_string`] is round-trip-stable but not byte-identical to
//! hand-authored `GemCAD` output. Saving a design that came from a catalogue `.asc`
//! and was never actually edited should not reformat that text just because a design
//! file sits beside it: [`catalogue_asc`] takes the caller's already-loaded original
//! `.asc` text and preserves it byte for byte whenever the design's current
//! [`crate::design::Design::to_asc_schedule`] output is semantically equal to what
//! that text itself parses to, WITH ONE FIELD MASKED: `refractive_index` is compared
//! against the original text's own value, not the design's current
//! [`crate::design::Design::effective_refractive_index`] -- picking a different
//! built-in material changes that derived figure on its own, with no tier/preform
//! data touched at all, and the design file's `[material]` table already records the
//! real selection, so the exported `I` line staying at its original value costs
//! nothing. So: nothing authored actually changed, even if
//! `girdle_diameter_mm`/`material`/`preform` were touched (none of which round-trip
//! into `.asc` beyond that one masked field). Once a real tier/preform edit breaks that
//! (masked) equality, this regenerates a fresh `.asc` instead -- see the
//! `catalogue_asc_preserves_untouched_text`/`catalogue_asc_preserves_untouched_text_after_a_material_change_alone`/`catalogue_asc_regenerates_after_a_real_edit`
//! tests, and that module's own (private) `schedules_equal_ignoring_refractive_index`
//! for the actual comparison.

mod convert;
mod design_file;
mod dispersion_dto;
mod load;
mod save;
#[cfg(test)]
mod tests;
mod unknown_keys;

pub use dispersion_dto::{
    dispersion_dto_from_json, dispersion_model_dto, dispersion_model_from_dto,
    dispersion_model_from_json, dispersion_model_to_json,
};
pub use unknown_keys::UnknownFileKeys;

pub use convert::{
    SaveExtras, SnapshotColor, color_recipe_dto, gem_material_from_custom_snapshot,
    gem_material_from_custom_snapshot_keeping_recipe, snapshot_color,
};
pub use design_file::{
    DESIGN_HISTORY_LIMIT, DesignExtras, DesignLoadError, LoadedDesign, design_from_file,
    design_from_str, design_to_file, design_to_string,
};
pub use load::MaterialResolution;
pub use save::{
    CatalogueAsc, DraftReason, SaveError, catalogue_asc, catalogue_asc_extended,
    catalogue_asc_extended_from_solved,
};

pub use indicatrix_formats::native::{
    CustomMaterialSnapshot, DispersionModelDto, HistoryTable, MaterialTable, NativeMeetConstraint,
    NativePreformShape, PreformTable, SourceTable, TierTable,
    design::{
        AttachmentBlob, AttachmentRole, DesignFile, DesignFileError, DesignMetadata, FileKind,
    },
    sha256_hex,
};

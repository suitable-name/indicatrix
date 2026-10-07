//! Saved variants of a design: named copies the cutter can come back to.
//!
//! Kept in the local library keyed by the design's UUID (see [`super::design_key`]).
//!
//! See `Database::save_variant`, `list_variants`, `load_variant`, `rename_variant`,
//! `set_variant_note`, `delete_variant` and `variant_thumbnail` for the storage side.

/// What a new variant is made of. Borrowed, so a caller hands over the serialized design
/// without copying it first.
#[derive(Debug, Clone, Copy)]
pub struct NewVariant<'a> {
    /// The cutter's name for the variant. Surrounding spaces are removed; it must not
    /// be empty afterwards.
    pub name: &'a str,
    /// The variant this one was made from, if any. It must exist and belong to the same
    /// design.
    pub parent_variant_id: Option<i64>,
    /// A free-text note. Blank means no note.
    pub note: Option<&'a str>,
    /// The design as its `.indicatrix` file text. Keeping it free of attached files
    /// (PDFs, images) keeps the library database small; the design file keeps those.
    pub design_text: &'a str,
    /// A small PNG preview of the design, if one was rendered. An empty slice means none.
    pub thumbnail_png: Option<&'a [u8]>,
    /// When the variant was made, in Unix seconds.
    pub created_at: i64,
}

/// A variant as a list shows it: everything except the design text and the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantSummary {
    /// The variant's id in the library database.
    pub variant_id: i64,
    /// The UUID of the design the variant belongs to, in lowercase.
    pub design_uuid: String,
    /// The cutter's name for the variant.
    pub name: String,
    /// The variant this one was made from. `None` for a first variant, and for one whose
    /// parent was deleted.
    pub parent_variant_id: Option<i64>,
    /// When the variant was made, in Unix seconds.
    pub created_at: i64,
    /// The cutter's note, if any.
    pub note: Option<String>,
}

/// A variant with the design it keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignVariant {
    /// The list entry for this variant.
    pub summary: VariantSummary,
    /// The design as its `.indicatrix` file text, exactly as it was saved.
    pub design_text: String,
}

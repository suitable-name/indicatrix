//! The cached front/top preview renders this crate stores per design.
//!
//! See `crate::db::sqlite::Database::save_preview_images`/`get_preview_images`/
//! `ensure_preview_material` for the storage side, and `crate::model::material_match`
//! for the pure RI-to-preset matching logic that picks [`PreviewImages::material`]'s
//! value.

/// One design's cached preview state, exactly as stored -- the result of
/// `crate::db::sqlite::Database::get_preview_images`.
///
/// Every field is independently `Option`. "Attempted" and "this particular image
/// exists" are deliberately not collapsed into one flag: a design whose generation was
/// attempted but where one view's render failed can have `front` populated and `top`
/// `None` while `generated_at` is still `Some`, so a caller can tell "never tried" from
/// "tried, one view came back empty" (this crate makes no retry-policy decision itself).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreviewImages {
    /// PNG bytes of the front-view render, or `None` if never generated or that view's
    /// render failed.
    pub front: Option<Vec<u8>>,
    /// PNG bytes of the top-view render, or `None` if never generated or that view's
    /// render failed.
    pub top: Option<Vec<u8>>,
    /// The `indicatrix::optics::materials::GemMaterial` preset name these previews were
    /// (or will be) rendered with -- see
    /// `crate::db::sqlite::Database::ensure_preview_material` for the "rolled once,
    /// reused forever" contract that keeps a design's rendered colour stable. Can be
    /// `Some` even while `front`/`top` are both `None`: the material is chosen and
    /// persisted before rendering happens.
    pub material: Option<String>,
    /// Unix seconds of the last time preview generation was *attempted*, or `None` if
    /// never. Answers "never generated" vs "generated and genuinely came back empty" --
    /// `front`/`top` being `None` is ambiguous alone, `generated_at` is not.
    pub generated_at: Option<i64>,
}

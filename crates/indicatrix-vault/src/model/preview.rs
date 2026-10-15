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
    /// reused forever" contract that keeps a design's rendered color stable. Can be
    /// `Some` even while `front`/`top` are both `None`: the material is chosen and
    /// persisted before rendering happens.
    pub material: Option<String>,
    /// Unix seconds of the last time preview generation was *attempted*, or `None` if
    /// never. Answers "never generated" vs "generated and genuinely came back empty" --
    /// `front`/`top` being `None` is ambiguous alone, `generated_at` is not.
    pub generated_at: Option<i64>,
}

/// How a design's persisted preview material was decided
/// (`diagram_previews.preview_material_source`).
///
/// The source decides how long the stored name is trusted: a refractive-index pick is
/// "rolled once, reused forever" (`crate::db::sqlite::Database::ensure_preview_material`),
/// while a name taken from the design's own file is overwritten every time the design is
/// resolved again (`crate::db::sqlite::Database::set_preview_material`), because the file
/// is the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewMaterialSource {
    /// Picked from the design's refractive index. Also how a row written before the
    /// column existed (a `NULL` source) reads.
    #[default]
    RefractiveIndex,
    /// Named by the design's own file (its material, with the file's overrides).
    DesignFile,
}

impl PreviewMaterialSource {
    /// The text stored in the column: `"design"` for [`Self::DesignFile`], `"ri"` for
    /// [`Self::RefractiveIndex`].
    #[must_use]
    pub const fn as_column_text(self) -> &'static str {
        match self {
            Self::RefractiveIndex => "ri",
            Self::DesignFile => "design",
        }
    }

    /// Reads the column back: `"design"` is [`Self::DesignFile`]; anything else,
    /// including a `NULL`, is [`Self::RefractiveIndex`].
    #[must_use]
    pub fn from_column_text(text: Option<&str>) -> Self {
        if text.is_some_and(|text| text == "design") {
            Self::DesignFile
        } else {
            Self::RefractiveIndex
        }
    }
}

/// What `diagram_previews` persists about the material a design's cached renders are made in.
///
/// Its name, the stone size it was sized with and how the name was decided. Read by
/// the staleness scans (`crate::db::sqlite::Database::entry_ids_missing_previews`,
/// `entry_ids_missing_tilt_curves`) so the fingerprint they compare is the one a render
/// made now would store, without resolving any design file.
///
/// `Default` is the state of a row that has none of it: no name, no size, a
/// refractive-index source.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreviewMaterialState {
    /// The material's name, or `None` before one has been picked.
    pub name: Option<String>,
    /// The girdle diameter in millimetres the material was sized with, or `None` for
    /// "not set" (the 7 mm calibration).
    pub girdle_mm: Option<f64>,
    /// How [`Self::name`] was decided.
    pub source: PreviewMaterialSource,
}

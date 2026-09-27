//! Small value types the search/display queries in [`super`] are parameterized over:
//! the sort order a caller can choose, and the argument-bundling structs that keep the
//! functions taking them under clippy's `too_many_arguments` lint.

/// Sort order for [`crate::db::sqlite::Database::search_diagrams_display`] -- the
/// library panel's sort selector (the tag/collection half is deliberately out of scope
/// here).
///
/// `#[default]` is [`Self::CatalogueOrder`], the same `de.id ASC` every other search in
/// this module has always used, so a caller that never sets a sort preference sees no
/// change in behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// `de.id ASC` -- insertion order, this module's long-standing default.
    #[default]
    CatalogueOrder,
    /// Case-insensitive title, A-Z.
    Title,
    /// Most recently created first (`diagram_entries.created_at`).
    /// A row that predates that column (`NULL`) sorts last -- SQLite already orders
    /// `NULL` after every non-null value in `DESC`, so no extra `CASE` is needed.
    Newest,
    /// Most recently edited first (`diagram_entries.updated_at`).
    /// Same `NULL`-sorts-last behaviour as [`Self::Newest`].
    RecentlyEdited,
}

impl SortOrder {
    /// The `ORDER BY` clause text for this order, appended directly after
    /// [`super::predicate::build_search_predicate`]'s `WHERE` clause -- every variant is
    /// a fixed literal, never built from caller input, so this is safe to interpolate.
    pub(super) const fn order_by_sql(self) -> &'static str {
        match self {
            Self::CatalogueOrder => " ORDER BY de.id ASC ",
            Self::Title => " ORDER BY de.title COLLATE NOCASE ASC, de.id ASC ",
            Self::Newest => " ORDER BY de.created_at DESC, de.id DESC ",
            Self::RecentlyEdited => " ORDER BY de.updated_at DESC, de.id DESC ",
        }
    }
}

/// [`crate::db::sqlite::Database::search_diagrams_display`]/
/// [`crate::db::sqlite::Database::count_matching_diagrams`]'s sort/restriction options.
///
/// Bundled into one value purely to keep those two public functions under clippy's
/// `too_many_arguments` lint -- same reasoning as [`DisplayPage`], which this
/// expands into once `offset`/`limit` are known for a given page.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayFilters<'a> {
    pub order: SortOrder,
    /// "My designs" restriction.
    pub local_only: bool,
    /// Tag chip restriction, `None` for no
    /// restriction.
    pub tag_filter: Option<i64>,
    /// "Show these N" restriction to an explicit id set (the batch
    /// case), `None`/empty for no restriction.
    pub id_filter: Option<&'a [i64]>,
}

/// Bundles [`crate::db::sqlite::Database::search_diagrams_page_raw_ordered`]'s
/// order/restriction/paging parameters into one value, purely to keep that function
/// under clippy's `too_many_arguments` lint -- each field is independent, not a
/// cohesive value in its own right, so this has no behaviour of its own beyond grouping
/// them.
#[derive(Debug, Clone, Copy)]
pub(super) struct DisplayPage<'a> {
    pub(super) order: SortOrder,
    pub(super) local_only: bool,
    /// Restricts the page to designs carrying this tag id, `None` for no tag
    /// restriction. See [`super::predicate::build_search_predicate`]'s own doc
    /// comment for the predicate this adds.
    pub(super) tag_filter: Option<i64>,
    /// Restricts the page to exactly these entry ids (the "show
    /// these N" batch-import case), `None`/empty for no restriction. Borrowed, not
    /// owned: every caller already holds the id list (`imported_ids`, or a UI
    /// property read once per query) for at least as long as the query runs, so
    /// cloning it into every `DisplayPage` a multi-page performance-filter walk
    /// builds would be pure waste.
    pub(super) id_filter: Option<&'a [i64]>,
    pub(super) offset: i64,
    pub(super) limit: i64,
}

/// Maximum rows [`crate::db::sqlite::Database::search_diagrams`] and friends will
/// return.
///
/// Also the default page size a mirror walking
/// [`crate::db::sqlite::Database::search_diagrams_page`] is sized around
/// (`apps/indicatrix-worker`'s `SearchPage` handler uses the same value).
pub const SEARCH_RESULT_CAP: i64 = 1000;

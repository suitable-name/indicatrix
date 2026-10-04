//! Builds the shared `WHERE`-clause predicate every search/count/display query in
//! [`super`] filters on, so free-text, shape/gear, range, tag, and tilt-performance
//! filtering can never drift apart between them.

use super::types::DisplayFilters;
use crate::model::filter::RangeFilter;
use rusqlite::functions::FunctionFlags;
use std::fmt::Write as _;

/// Registers the `fold(text)` SQL scalar function [`build_search_predicate`]'s title/
/// designer `LIKE` clauses wrap both sides in (see [`fold`]'s own doc comment for what
/// it does and why). Must be called exactly once per [`rusqlite::Connection`], right
/// after it's opened -- both [`crate::db::sqlite::Database::new`] and
/// [`crate::db::sqlite::Database::open_read_only`] call this, since search runs over
/// read-only connections too (e.g. `indicatrix-worker`'s per-request connection). A
/// `SELECT` naming `fold(...)` on a connection this was never called on fails outright
/// ("no such function: fold"), never silently falling back to an unfolded comparison.
///
/// # Errors
///
/// Returns an error if `rusqlite::Connection::create_scalar_function` fails.
pub(in crate::db::sqlite) fn register_fold_function(
    conn: &rusqlite::Connection,
) -> anyhow::Result<()> {
    conn.create_scalar_function(
        "fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            // `dd.designer_info`/`dd.designer` are nullable columns (and the `LEFT
            // JOIN` to `dd` itself can produce a NULL row), so `fold(...)` must accept
            // SQL NULL as an ordinary input, not just a `String` -- `ctx.get::<String>`
            // errors out on a NULL argument ("Invalid function parameter type Null"),
            // which used to abort the ENTIRE search query (not just that one row) the
            // moment it reached a design with no `designer_info`/`designer` recorded.
            // `NULL` propagates to `NULL` here, exactly like SQLite's own `LIKE`
            // already treats a NULL operand -- "no match", never an error.
            let text = ctx.get::<Option<String>>(0)?;
            Ok(text.map(|t| fold(&t)))
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to register the fold() SQL function: {e}"))
}

/// Folds `text` for a typography-/case-insensitive `LIKE` comparison:
///
/// - Lowercases, Unicode-aware (not just ASCII) -- SQLite's own `LIKE` only case-folds
///   ASCII `A-Z`/`a-z`, so e.g. `"TORBJÖRN"` and `"torbjörn"` compare UNEQUAL through a
///   plain `LIKE`, even though a human reading the catalogue considers them the same
///   name.
/// - Maps the curly-quote/dash Unicode punctuation a scraped web page (or a word
///   processor) commonly substitutes for the plain ASCII character a user types into a
///   search box: U+2018/U+2019 (`'`/`'`) -> `'`, U+201C/U+201D (`"`/`"`) -> `"`,
///   U+2013/U+2014 (`-`/`—`) -> `-`. So a plain `"Cam's"` typed in the search box finds
///   a designer name scraped with a real typographic apostrophe (`"Cam’s"`).
///
/// Deliberately NOT full Unicode normalization (no NFKC): this is a small, fixed,
/// auditable substitution list sized to the punctuation this catalogue actually has
/// trouble with, not a general text-folding library. None of the six substituted
/// characters, nor lowercasing, touches `%`/`_`/`\` -- the `LIKE` wildcards and
/// `ESCAPE '\'` escape character [`escape_like_pattern`] relies on all survive folding
/// untouched, since [`register_fold_function`]'s SQL function is applied to both the
/// column and the bound pattern.
#[must_use]
pub(super) fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let mapped = match ch {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        };
        out.extend(mapped.to_lowercase());
    }
    out
}

/// Builds the `SELECT ... WHERE ...` predicate (everything through the last filter
/// clause, not `ORDER BY`/`LIMIT`) shared by
/// [`crate::db::sqlite::Database::search_diagrams_page_raw`] and
/// [`crate::db::sqlite::Database::search_diagrams_with_performance_exclusions`], plus
/// the bound parameters its `?` placeholders need, in SQL-text order.
///
/// `index_gear`/`lw_ratio`/`refractive_index`/`volume` are stored as REAL/INTEGER (see
/// `migrate_numeric_columns`) and cast back to TEXT so `DiagramListItem`'s
/// display-string fields stay unchanged.
///
/// # Tilt-performance filters: SQL narrows, Rust decides
///
/// A [`crate::model::performance::PerformanceFilter`]'s tilt radius is an arbitrary
/// `0.0..=90.0` value, ruling out a precomputed SQL column. This function adds only a
/// sound, incomplete narrowing predicate per active filter
/// ([`crate::model::performance::PerformanceFilter::sound_sql_narrowing`], built from
/// `diagram_tilt_curves`'s 6 precomputed global-min/max columns): it can exclude a row
/// with certainty, but a row it lets through isn't yet confirmed. Every caller
/// re-checks every surviving row exactly, in Rust, against the decoded curve (see
/// [`crate::db::sqlite::Database::item_satisfies_performance_filters`]) -- this
/// predicate only shrinks the candidate set, it is never the source of truth.
///
/// `include_performance` selects whether `range.performance`'s narrowing predicates
/// are appended at all: `true` for every ordinary search, `false` only for
/// `search_diagrams_with_performance_exclusions`'s second query, which counts designs
/// excluded *for lack of curves* and so must apply every other filter but not these.
///
/// `local_only` restricts the predicate to designs synced under
/// [`crate::local::LOCAL_SOURCE_ID`] -- identified the same way
/// `library/detail.rs::is_local` already does, by `diagram_entries.url` starting with
/// the synthetic `local://` scheme every local import writes (see
/// `crate::local::import_asc`) -- as opposed to a real page URL from the wider scraped
/// catalogue. `false` (the default for every pre-existing caller) applies no such
/// restriction.
///
/// The returned `SELECT` always `LEFT JOIN`s `diagram_tilt_curves AS tc` regardless of
/// `include_performance`: both are 1:1 joins on `diagram_entries.id`, so the
/// unconditional join costs nothing measurable.
pub(super) fn build_search_predicate(
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilter,
    include_performance: bool,
    filters: DisplayFilters<'_>,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let DisplayFilters {
        local_only,
        tag_filter,
        id_filter,
        order: _order,
    } = filters;
    let q_pattern = format!("%{}%", escape_like_pattern(query.trim()));
    let mut sql = String::from(
        "SELECT de.id, de.title, de.url, de.design_id,
                dd.shape, CAST(dd.index_gear AS TEXT), dd.facets_count, dd.designer_info,
                CAST(dd.lw_ratio AS TEXT), CAST(dd.refractive_index AS TEXT),
                CAST(dd.volume AS TEXT), dd.competition_diagram,
                de.ignored
         FROM diagram_entries de
         LEFT JOIN diagram_details dd ON de.id = dd.entry_id
         LEFT JOIN diagram_tilt_curves tc ON de.id = tc.entry_id
         WHERE 1=1 ",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if query.trim().is_empty() {
        sql.push_str(" AND (1=1 OR ?1 IS NULL) ");
    } else {
        // The search tooltip/placeholder (header.slint) promises "title, designer
        // or notes", and this predicate matches a stored note accordingly.
        // `angle_settings.notes` is per-TIER (one design has many rows),
        // so matching it needs an `EXISTS` subquery rather than a plain joined
        // column, which would otherwise duplicate a design once per matching tier.
        //
        // `dd.designer` (the machine-split designer name -- see `FacetingDiagramDetail::designer`'s
        // doc comment) is matched here too, not just `dd.designer_info`: before this,
        // a query that only appeared in the split `designer` column (never in the
        // free-text `designer_info` blob it was split from) silently matched nothing
        // -- the "Cam's finds nothing" case was exactly a `designer_info` miss with
        // no `designer` fallback.
        //
        // `de.title`/`dd.designer_info`/`dd.designer` are wrapped in `fold(...)` on
        // BOTH sides (column and `?1` pattern) -- see `fold`'s own doc comment: SQLite's
        // `LIKE` only case-folds ASCII, so an accented name typed in a different case
        // (`TORBJÖRN` vs `torbjörn`) would otherwise never match, and a plain ASCII
        // apostrophe typed by the user would never match a scraped typographic one.
        // `de.design_id`/the notes `EXISTS` subquery are deliberately NOT folded: an id
        // is an exact scraped token, not prose, and notes text has no reported
        // apostrophe/case complaint to fix -- folding it would only add cost with no
        // known benefit.
        //
        // `ESCAPE '\'` on every LIKE here, paired with `escape_like_pattern` above:
        // without it, a literal `%`/`_` a user typed (both appear in real titles and
        // designer names) is read as a SQL wildcard instead of the character it looks
        // like -- an unescaped `_` alone matches every row's title. `fold` never
        // touches `%`/`_`/`\`, so the escape still works after folding.
        sql.push_str(
            " AND (fold(de.title) LIKE fold(?1) ESCAPE '\\'
                   OR fold(dd.designer_info) LIKE fold(?1) ESCAPE '\\'
                   OR fold(dd.designer) LIKE fold(?1) ESCAPE '\\'
                   OR de.design_id LIKE ?1 ESCAPE '\\'
                   OR EXISTS (
                       SELECT 1 FROM angle_settings a
                       WHERE a.detail_id = dd.id AND a.notes LIKE ?1 ESCAPE '\\'
                   )) ",
        );
    }
    params.push(Box::new(q_pattern));

    // Bound parameters, not string-interpolated literals -- `?1` above is reserved for
    // the LIKE pattern, so every parameter from here on is a plain positional `?`;
    // rusqlite/SQLite number those sequentially starting just after the highest
    // explicit index used (`?1`), so the first plain `?` here binds to position 2 and
    // each subsequent one to the next position, matching `params`' push order below.
    if !shape_filter.is_empty() && shape_filter != "All" {
        sql.push_str(" AND dd.shape = ? ");
        params.push(Box::new(shape_filter.to_owned()));
    }

    if !gear_filter.is_empty() && gear_filter != "All" {
        sql.push_str(" AND dd.index_gear = ? ");
        params.push(Box::new(gear_filter.to_owned()));
    }

    // Bound parameters, same as shape/gear above: these are slider numbers.
    if let Some(min) = range.ri_min {
        sql.push_str(" AND dd.refractive_index >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.ri_max {
        sql.push_str(" AND dd.refractive_index <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.lw_min {
        sql.push_str(" AND dd.lw_ratio >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.lw_max {
        sql.push_str(" AND dd.lw_ratio <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.volume_min {
        sql.push_str(" AND dd.volume >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.volume_max {
        sql.push_str(" AND dd.volume <= ? ");
        params.push(Box::new(max));
    }
    if let Some(min) = range.facets_min {
        sql.push_str(" AND dd.facets >= ? ");
        params.push(Box::new(min));
    }
    if let Some(max) = range.facets_max {
        sql.push_str(" AND dd.facets <= ? ");
        params.push(Box::new(max));
    }

    // RI-tolerance band ANDs alongside ri_min/ri_max above, not replacing them.
    if let Some((center, tolerance)) = range.ri_tolerance {
        sql.push_str(" AND dd.refractive_index >= ? AND dd.refractive_index <= ? ");
        params.push(Box::new(center - tolerance));
        params.push(Box::new(center + tolerance));
    }

    append_concave_filter(&mut sql, range.has_concave);

    // Ignored designs excluded unless opted back in. No bound parameter: 0/1 is a
    // fixed literal, not caller-supplied.
    if !range.include_ignored {
        sql.push_str(" AND de.ignored = 0 ");
    }

    // "My designs" restriction -- no bound parameter: the
    // `local://` prefix is a fixed literal this crate itself writes (see
    // `crate::local::import_asc`), never caller-supplied text.
    if local_only {
        sql.push_str(" AND de.url LIKE 'local://%' ");
    }

    append_tag_and_id_filters(&mut sql, &mut params, tag_filter, id_filter);

    // Tilt-performance narrowing (see "SQL narrows, Rust decides" above).
    // `sound_sql_narrowing`'s column name only ever comes from
    // `global_extreme_column_name`, never user-supplied text.
    if include_performance && !range.performance.is_empty() {
        // No stored curves can never satisfy any active filter, including a `Mean`
        // filter whose `sound_sql_narrowing()` is `None`.
        sql.push_str(" AND tc.generated_at IS NOT NULL ");
        for filter in &range.performance {
            if let Some((column, comparator, threshold)) = filter.sound_sql_narrowing() {
                let _ = write!(sql, " AND tc.{column} {comparator} ? ");
                params.push(Box::new(f64::from(threshold)));
            }
        }
    }

    (sql, params)
}

/// Appends the `has_concave` clause: fixed literals, no bound parameter. A design with
/// no detail row (NULL after the LEFT JOIN) counts as planar, so it satisfies
/// `Some(false)`.
fn append_concave_filter(sql: &mut String, has_concave: Option<bool>) {
    match has_concave {
        Some(true) => sql.push_str(" AND COALESCE(dd.concave_tiers, 0) > 0 "),
        Some(false) => sql.push_str(" AND COALESCE(dd.concave_tiers, 0) = 0 "),
        None => {}
    }
}

/// Appends [`build_search_predicate`]'s tag-chip and "show
/// these N" restrictions -- split out purely to keep that
/// function under clippy's `too_many_lines` limit, not because these two are a
/// cohesive concept; see [`build_search_predicate`]'s own doc comment for the
/// predicate as a whole.
fn append_tag_and_id_filters(
    sql: &mut String,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    tag_filter: Option<i64>,
    id_filter: Option<&[i64]>,
) {
    // Tag chip restriction -- bound, not interpolated, even though a tag id is
    // already an integer: consistent with every other numeric bound in
    // `build_search_predicate`.
    if let Some(tag_id) = tag_filter {
        sql.push_str(" AND de.id IN (SELECT entry_id FROM diagram_tag_links WHERE tag_id = ?) ");
        params.push(Box::new(tag_id));
    }

    // "Show these N" restriction to an explicit id set -- one bound `?` per id, not
    // a single interpolated literal list, even though an entry id is already
    // caller-internal (never raw user text): consistent with every other bound
    // value in `build_search_predicate`, and it costs nothing here since the id set
    // is always small (one import batch's worth).
    if let Some(ids) = id_filter
        && !ids.is_empty()
    {
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = write!(sql, " AND de.id IN ({placeholders}) ");
        for &id in ids {
            params.push(Box::new(id));
        }
    }
}

/// Escapes `\`, `%` and `_` in `raw` so it matches only as a literal substring once
/// wrapped in `%...%` -- paired with `ESCAPE '\'` on every `LIKE ?1` clause
/// [`build_search_predicate`] builds.
///
/// Without this, a character a user typed as ordinary text is silently read as a SQL
/// wildcard instead: real titles and designer names in the catalogue contain both `%`
/// and `_`, so an unescaped `_` alone matches every row's title (any single character),
/// and `50%` degrades to "contains `50` followed by anything" rather than a literal
/// percent sign. `\` itself is escaped first (into `\\`) so a literal backslash already
/// present in the query text isn't misread as the start of an escape sequence once
/// `ESCAPE '\'` is in effect.
fn escape_like_pattern(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

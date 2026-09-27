//! Small building blocks the migrations in [`super`] share: a SQL-identifier
//! safety check, the generic TEXT-to-numeric column retype sequence, and the
//! one-off `facets_count` splitter.

use crate::model::facets::parse_facets_count;
use anyhow::{Context, Result};
use rusqlite::{Transaction, params};

/// Validates `name` as safe to interpolate directly into DDL/`PRAGMA` text.
///
/// SQLite's prepared-statement placeholders (`?1`, `params![...]`) can only
/// bind *values*, never identifiers, so every table/column name this module
/// formats into `ALTER TABLE`/`CREATE INDEX`/`PRAGMA table_info` text has to
/// be interpolated as a string -- the same mechanism a real SQL-injection
/// bug would use. Every identifier this module currently formats is a
/// hard-coded literal (or built from one, like
/// [`retype_text_column_to_numeric`]'s `{column}__migrated`), so nothing can
/// actually be injected today; this guard exists so a future refactor that
/// makes any of them dynamic (a caller-supplied column name, say) cannot
/// silently reopen that door.
///
/// Accepts only `^[A-Za-z_][A-Za-z0-9_]*$`: an ASCII letter or underscore,
/// then any run of ASCII letters/digits/underscores. Checked with a plain
/// char loop rather than a `regex` crate dependency -- the pattern is simple
/// enough not to need one.
///
/// # Errors
///
/// Returns an error naming `name` if it is empty or contains any character
/// outside that pattern.
pub(in crate::db::sqlite) fn sql_identifier(name: &str) -> Result<&str> {
    let mut chars = name.chars();
    let starts_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if starts_ok && rest_ok {
        Ok(name)
    } else {
        anyhow::bail!("'{name}' is not a valid SQL identifier")
    }
}

/// Retypes `diagram_details.{column}` from TEXT to `sql_type` (`"REAL"` or
/// `"INTEGER"`) in place, via SQLite's standard "add the replacement, populate it, drop
/// the original, rename the replacement" sequence (SQLite has no `ALTER COLUMN ...
/// TYPE`). Non-numeric-looking values become `NULL` rather than `CAST`'s silent `0.0`,
/// which would otherwise fabricate data for empty rows.
///
/// # Errors
///
/// Returns an error if `column` or `sql_type` is not a valid SQL identifier
/// (see [`sql_identifier`]), or if adding the replacement column, populating
/// it, or dropping/renaming the original fails.
pub(super) fn retype_text_column_to_numeric(
    tx: &Transaction<'_>,
    column: &str,
    sql_type: &str,
) -> Result<()> {
    let column = sql_identifier(column)?;
    let sql_type = sql_identifier(sql_type)?;
    let staging = format!("{column}__migrated");
    let staging = sql_identifier(&staging)?;
    tx.execute_batch(&format!(
        "ALTER TABLE diagram_details ADD COLUMN {staging} {sql_type};"
    ))
    .with_context(|| format!("Failed to add staging column for '{column}'"))?;

    tx.execute(
        &format!(
            "UPDATE diagram_details
             SET {staging} = CASE
                 WHEN {column} IS NULL OR TRIM({column}) = '' THEN NULL
                 ELSE CAST({column} AS {sql_type})
             END"
        ),
        [],
    )
    .with_context(|| format!("Failed to populate staging column for '{column}'"))?;

    tx.execute_batch(&format!(
        "ALTER TABLE diagram_details DROP COLUMN {column};
         ALTER TABLE diagram_details RENAME COLUMN {staging} TO {column};"
    ))
    .with_context(|| format!("Failed to swap staging column into place for '{column}'"))?;

    Ok(())
}

/// Populates the new `facets`/`girdle_facets` INTEGER columns from the existing
/// `facets_count` TEXT column (e.g. `"55+6"` -> `facets = 55, girdle_facets = 6`), via
/// [`parse_facets_count`]; `facets_count` itself is left untouched as the display
/// value. Done row-by-row in Rust, not SQL string functions, since the real data has
/// more shapes than `"N+M"` and `parse_facets_count` already handles all of them.
///
/// # Errors
///
/// Returns an error if reading `(id, facets_count)` rows or writing any
/// `facets`/`girdle_facets` update fails.
pub(super) fn split_facets_count_column(tx: &Transaction<'_>) -> Result<()> {
    let rows: Vec<(i64, Option<String>)> = {
        let mut select_stmt = tx
            .prepare("SELECT id, facets_count FROM diagram_details")
            .context("Failed to prepare facets_count read for splitting")?;
        select_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .context("Failed to run facets_count read for splitting")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to decode a row while reading facets_count for splitting")?
    };

    let mut update_stmt = tx
        .prepare("UPDATE diagram_details SET facets = ?1, girdle_facets = ?2 WHERE id = ?3")
        .context("Failed to prepare facets/girdle_facets update")?;
    for (id, raw) in rows {
        let (facets, girdle_facets) = parse_facets_count(raw.as_deref());
        update_stmt
            .execute(params![facets, girdle_facets, id])
            .with_context(|| format!("Failed to write facets/girdle_facets for id {id}"))?;
    }
    Ok(())
}

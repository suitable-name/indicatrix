//! Small building blocks the migrations in [`super`] share: a SQL-identifier
//! safety check, the generic TEXT-to-numeric column retype sequence, and the
//! one-off `facets_count` splitter, and the `PRAGMA table_info` column probes.

use super::super::Database;
use crate::model::facets::parse_facets_count;
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, params};

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
/// TYPE`).
///
/// Text that does not begin with a numeric literal (empty, `'n/a'`, `'?'`) becomes
/// `NULL`: a bare `CAST` would turn every one of those into a fabricated `0`/`0.0`.
/// Text that does begin with one (`'96 index'`, `'.5'`, `'-1.2'`) keeps the parsed
/// leading number, exactly as `CAST` reads it, and a genuine `'0'` stays `0`.
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
                 WHEN {column} IS NULL THEN NULL
                 WHEN TRIM({column}) GLOB '[0-9]*'
                   OR TRIM({column}) GLOB '[+-][0-9]*'
                   OR TRIM({column}) GLOB '.[0-9]*'
                   OR TRIM({column}) GLOB '[+-].[0-9]*'
                 THEN CAST(TRIM({column}) AS {sql_type})
                 ELSE NULL
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

impl Database {
    /// Adds every `(name, declaration)` of `columns` that `table` does not have yet,
    /// all inside ONE transaction, so a batch of `ALTER TABLE ... ADD COLUMN`
    /// statements either lands completely or not at all -- SQLite's DDL is
    /// transactional, and a crash or `SQLITE_BUSY` between two autocommitted ALTERs
    /// would otherwise leave a half-applied column set.
    ///
    /// Each ALTER is guarded by a [`Self::column_exists`] probe, so re-running over a
    /// table that an older, non-transactional build left with only some of the columns
    /// adds exactly the missing ones. A caller gates the whole migration on the LAST
    /// column of `columns` so that such a partially migrated table is picked up again.
    /// `declaration` is interpolated into DDL text and must be a hard-coded type
    /// literal (`"REAL"`, `"INTEGER"`), never caller input.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` or a column name is not a valid SQL identifier (see
    /// [`sql_identifier`]), or if starting the transaction, probing a column, adding a
    /// column or committing fails -- in which case nothing is committed.
    pub(in crate::db::sqlite) fn add_missing_columns(
        &self,
        table: &str,
        columns: &[(&str, &str)],
    ) -> Result<()> {
        let table = sql_identifier(table)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .with_context(|| format!("Failed to start the ADD COLUMN transaction for '{table}'"))?;
        for &(column, declaration) in columns {
            let column = sql_identifier(column)?;
            if Self::column_exists(&tx, table, column)? {
                continue;
            }
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {declaration};"
            ))
            .with_context(|| format!("Failed to add column '{column}' to '{table}'"))?;
        }
        tx.commit()
            .with_context(|| format!("Failed to commit the ADD COLUMN transaction for '{table}'"))
    }

    /// Whether `table` currently has a column named `column`, via `PRAGMA table_info`.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the `PRAGMA table_info` query fails.
    pub(in crate::db::sqlite) fn column_exists(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<bool> {
        Ok(Self::column_sql_type(conn, table, column)?.is_some())
    }

    /// `table.column`'s declared SQL type (e.g. `"TEXT"`, `"REAL"`, `"INTEGER"`) via
    /// `PRAGMA table_info`, or `None` if `table` has no such column. Lets a migration
    /// gate itself on what a column actually IS rather than merely whether it exists
    /// -- see [`Self::migrate_numeric_columns`] for why that distinction matters: a
    /// fresh database can already have every column a migration would otherwise add or
    /// retype, already in its final shape, and presence alone can't tell those two
    /// cases apart.
    ///
    /// # Errors
    ///
    /// Returns an error if `table` is not a valid SQL identifier (see
    /// [`sql_identifier`]) or the `PRAGMA table_info` query fails.
    pub(in crate::db::sqlite) fn column_sql_type(
        conn: &Connection,
        table: &str,
        column: &str,
    ) -> Result<Option<String>> {
        let table = sql_identifier(table)?;
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get("name")?;
            if name == column {
                return Ok(Some(row.get("type")?));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs [`retype_text_column_to_numeric`] over a one-column `diagram_details` holding
    /// `values` and reads the retyped column back in row order.
    fn retyped(values: &[Option<&str>], sql_type: &str) -> Vec<Option<f64>> {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE diagram_details (id INTEGER PRIMARY KEY, v TEXT);")
            .unwrap();
        for value in values {
            conn.execute(
                "INSERT INTO diagram_details (v) VALUES (?1)",
                params![value],
            )
            .unwrap();
        }
        let tx = conn.transaction().unwrap();
        retype_text_column_to_numeric(&tx, "v", sql_type).unwrap();
        tx.commit().unwrap();
        let mut stmt = conn
            .prepare("SELECT v FROM diagram_details ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn text_without_a_leading_number_becomes_null_not_zero() {
        let got = retyped(
            &[
                Some("n/a"),
                Some("?"),
                Some(""),
                Some("  "),
                None,
                Some("x1.5"),
            ],
            "REAL",
        );
        assert_eq!(got, vec![None; 6]);
    }

    #[test]
    fn a_genuine_zero_and_leading_numbers_survive() {
        let got = retyped(
            &[
                Some("0"),
                Some(" 1.76 "),
                Some(".5"),
                Some("-2.5"),
                Some("96 index"),
            ],
            "REAL",
        );
        assert_eq!(
            got,
            vec![Some(0.0), Some(1.76), Some(0.5), Some(-2.5), Some(96.0)]
        );
    }
}

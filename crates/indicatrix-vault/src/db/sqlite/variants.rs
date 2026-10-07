//! Storage for `design_variants`: named copies of a whole design, kept per design UUID.
//!
//! See `Database::migrate_design_variants_table`'s doc comment (in `super::migrations`)
//! for the schema and why this is keyed by the design's UUID rather than by `entry_id`,
//! and `crate::model::design_variant` for the models.
//!
//! None of this touches `diagram_entries` -- in particular never its `updated_at`, which
//! keys the preview and tilt-curve caches: a variant is a side copy, not an edit of the
//! catalogued design.

use super::Database;
use crate::model::{
    design_key::require_design_uuid,
    design_variant::{DesignVariant, NewVariant, VariantSummary},
};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};

/// The columns of a list or load query, in the order [`summary_of`] reads them.
const SUMMARY_COLUMNS: &str = "variant_id, design_uuid, name, parent_variant_id, created_at, note";

/// Reads [`SUMMARY_COLUMNS`] from `row`.
fn summary_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<VariantSummary> {
    Ok(VariantSummary {
        variant_id: row.get(0)?,
        design_uuid: row.get(1)?,
        name: row.get(2)?,
        parent_variant_id: row.get(3)?,
        created_at: row.get(4)?,
        note: row.get(5)?,
    })
}

/// `name` without surrounding spaces.
///
/// # Errors
///
/// Returns an error when nothing is left.
fn clean_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("A variant needs a name");
    }
    Ok(trimmed.to_string())
}

/// `note` without surrounding spaces, or `None` when it is blank.
fn clean_note(note: Option<&str>) -> Option<String> {
    note.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

impl Database {
    /// Saves a new variant of the design with UUID `design_uuid`, returning its id.
    ///
    /// The name is trimmed and must not be empty, the design text must not be blank, and
    /// a blank note or an empty preview is stored as none. `variant.parent_variant_id`,
    /// when given, must name an existing variant of the SAME design.
    ///
    /// The design does not have to be in the catalogue. Nothing in `diagram_entries`
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID, the name or design text is blank,
    /// the parent does not exist or belongs to another design, or the `INSERT` fails.
    pub fn save_variant(&self, design_uuid: &str, variant: &NewVariant<'_>) -> Result<i64> {
        let key = require_design_uuid(design_uuid)?;
        let name = clean_name(variant.name)?;
        if variant.design_text.trim().is_empty() {
            bail!("A variant needs the design it keeps");
        }
        if let Some(parent) = variant.parent_variant_id {
            let owner: Option<String> = self
                .conn
                .query_row(
                    "SELECT design_uuid FROM design_variants WHERE variant_id = ?1",
                    params![parent],
                    |row| row.get(0),
                )
                .optional()
                .with_context(|| format!("Failed to look up the parent variant {parent}"))?;
            match owner {
                None => bail!("The parent variant {parent} does not exist"),
                Some(owner) if owner != key => {
                    bail!("The parent variant {parent} belongs to another design")
                }
                Some(_) => {}
            }
        }
        let thumbnail = variant.thumbnail_png.filter(|bytes| !bytes.is_empty());
        self.conn
            .execute(
                "INSERT INTO design_variants (
                     design_uuid, name, parent_variant_id, created_at, note, design_text,
                     thumbnail_png
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    key,
                    name,
                    variant.parent_variant_id,
                    variant.created_at,
                    clean_note(variant.note),
                    variant.design_text,
                    thumbnail,
                ],
            )
            .with_context(|| format!("Failed to save variant '{name}' of design {key}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The variants of the design with UUID `design_uuid`, oldest first
    /// (`created_at`, then id), without their design text or preview.
    ///
    /// # Errors
    ///
    /// Returns an error if `design_uuid` is not a UUID or the query fails.
    pub fn list_variants(&self, design_uuid: &str) -> Result<Vec<VariantSummary>> {
        let key = require_design_uuid(design_uuid)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {SUMMARY_COLUMNS}
                 FROM design_variants
                 WHERE design_uuid = ?1
                 ORDER BY created_at, variant_id"
            ))
            .context("Failed to prepare the variant list query")?;
        stmt.query_map(params![key], summary_of)
            .context("Failed to run the variant list query")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .with_context(|| format!("Failed to read the variants of design {key}"))
    }

    /// The variant `variant_id` with the design it keeps, or `None` when there is no such
    /// variant (for example it was deleted in another window).
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn load_variant(&self, variant_id: i64) -> Result<Option<DesignVariant>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {SUMMARY_COLUMNS}, design_text
                     FROM design_variants
                     WHERE variant_id = ?1"
                ),
                params![variant_id],
                |row| {
                    Ok(DesignVariant {
                        summary: summary_of(row)?,
                        design_text: row.get(6)?,
                    })
                },
            )
            .optional()
            .with_context(|| format!("Failed to load variant {variant_id}"))
    }

    /// Renames variant `variant_id`. Returns how many rows changed: 0 when no such
    /// variant exists, which is not an error here -- the caller decides what to tell the
    /// cutter.
    ///
    /// # Errors
    ///
    /// Returns an error if the new name is blank or the `UPDATE` fails.
    pub fn rename_variant(&self, variant_id: i64, name: &str) -> Result<usize> {
        let name = clean_name(name)?;
        self.conn
            .execute(
                "UPDATE design_variants SET name = ?1 WHERE variant_id = ?2",
                params![name, variant_id],
            )
            .with_context(|| format!("Failed to rename variant {variant_id}"))
    }

    /// Sets (or, with `None` or blank text, clears) the note of variant `variant_id`.
    /// Returns how many rows changed: 0 when no such variant exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the `UPDATE` fails.
    pub fn set_variant_note(&self, variant_id: i64, note: Option<&str>) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE design_variants SET note = ?1 WHERE variant_id = ?2",
                params![clean_note(note), variant_id],
            )
            .with_context(|| format!("Failed to set the note of variant {variant_id}"))
    }

    /// Deletes variant `variant_id`. Returns how many rows were deleted: 0 when no such
    /// variant exists.
    ///
    /// Deleting a variant never deletes other variants: the ones made from it stay, with
    /// no parent from then on.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DELETE` fails.
    pub fn delete_variant(&self, variant_id: i64) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM design_variants WHERE variant_id = ?1",
                params![variant_id],
            )
            .with_context(|| format!("Failed to delete variant {variant_id}"))
    }

    /// The PNG preview of variant `variant_id`, or `None` when the variant has none or
    /// does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub fn variant_thumbnail(&self, variant_id: i64) -> Result<Option<Vec<u8>>> {
        let found: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                "SELECT thumbnail_png FROM design_variants WHERE variant_id = ?1",
                params![variant_id],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to read the preview of variant {variant_id}"))?;
        Ok(found.flatten())
    }
}

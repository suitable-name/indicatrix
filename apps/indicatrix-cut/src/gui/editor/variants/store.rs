//! Variants in the library database: the calls the view makes, with plain-English errors.
//!
//! These take a `&Database` and no window, so the tests run them on an in-memory database.
//! The glue in [`super`] calls them on a worker thread, with the database's lock held only for
//! the call.

use super::capture::{Pixels, decode_png, design_from_text};
use indicatrix_cut_core::Design;
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        design_key::normalize_design_uuid,
        design_variant::{NewVariant, VariantSummary},
    },
};
use std::fmt;

/// What a new variant is made of.
pub(super) struct SaveRequest<'a> {
    /// The design's UUID.
    pub(super) uuid: &'a str,
    /// The name, already cleaned (see `rows::clean_name`).
    pub(super) name: &'a str,
    /// The note, already cleaned; `None` for none.
    pub(super) note: Option<&'a str>,
    /// The `.indicatrix` text of the design.
    pub(super) design_text: &'a str,
    /// The PNG picture, if one could be drawn.
    pub(super) picture_png: Option<&'a [u8]>,
    /// The variant the design came from, if the cutter knows one. Dropped when it is gone.
    pub(super) parent: Option<i64>,
    /// The time in Unix seconds.
    pub(super) now: i64,
}

/// A library error as one sentence.
fn plain(error: &anyhow::Error) -> String {
    format!("The library could not be used. {error:#}")
}

/// The variants of the design `uuid`, oldest first.
///
/// # Errors
///
/// A plain sentence when the library cannot be read.
pub(super) fn list(db: &Database, uuid: &str) -> Result<Vec<VariantSummary>, String> {
    db.list_variants(uuid).map_err(|error| plain(&error))
}

/// A variant with its stored picture as PNG bytes (`None` for a variant with no picture), not
/// decoded yet.
pub(super) type VariantWithPng = (VariantSummary, Option<Vec<u8>>);

/// The variants of the design `uuid` with their stored picture bytes, still PNG.
///
/// Only reads: the caller holds the library's lock for exactly this call, so the pictures are
/// decoded afterwards ([`decode_pictures`]) and never while the lock is held.
///
/// # Errors
///
/// A plain sentence when the library cannot be read.
pub(super) fn list_with_png(db: &Database, uuid: &str) -> Result<Vec<VariantWithPng>, String> {
    list(db, uuid)?
        .into_iter()
        .map(|summary| {
            let png = db
                .variant_thumbnail(summary.variant_id)
                .map_err(|error| plain(&error))?;
            Ok((summary, png))
        })
        .collect()
}

/// `variants` with their pictures decoded (`None` for a variant with no picture or a damaged
/// one). Takes no library lock: it is the slow half of [`list_with_png`]'s answer.
pub(super) fn decode_pictures(
    variants: Vec<VariantWithPng>,
) -> Vec<(VariantSummary, Option<Pixels>)> {
    variants
        .into_iter()
        .map(|(summary, png)| {
            let picture = png.as_deref().and_then(decode_png);
            (summary, picture)
        })
        .collect()
}

/// Saves a new variant and returns its id.
///
/// # Errors
///
/// A plain sentence when the library refuses it.
pub(super) fn save(db: &Database, request: &SaveRequest<'_>) -> Result<i64, String> {
    // A parent that was deleted in the meantime would be refused, so it is dropped here.
    let parent = match request.parent {
        Some(id) => list(db, request.uuid)?
            .iter()
            .any(|variant| variant.variant_id == id)
            .then_some(id),
        None => None,
    };
    db.save_variant(
        request.uuid,
        &NewVariant {
            name: request.name,
            parent_variant_id: parent,
            note: request.note,
            design_text: request.design_text,
            thumbnail_png: request.picture_png,
            created_at: request.now,
        },
    )
    .map_err(|error| plain(&error))
}

/// Why a variant could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoadProblem {
    /// There is no such variant any more (it was deleted in another window).
    Missing,
    /// The variant belongs to another design.
    OtherDesign,
    /// The variant could not be read.
    Unreadable(String),
}

impl fmt::Display for LoadProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("That variant no longer exists."),
            Self::OtherDesign => f.write_str("That variant belongs to another design."),
            Self::Unreadable(reason) => f.write_str(reason),
        }
    }
}

/// The variant `id` of the design `uuid` with the design it keeps.
///
/// # Errors
///
/// [`LoadProblem`] when it does not exist, belongs to another design or cannot be read.
pub(super) fn load_design(
    db: &Database,
    uuid: &str,
    id: i64,
) -> Result<(VariantSummary, Design), LoadProblem> {
    let loaded = db
        .load_variant(id)
        .map_err(|error| LoadProblem::Unreadable(plain(&error)))?
        .ok_or(LoadProblem::Missing)?;
    if normalize_design_uuid(uuid).as_deref() != Some(loaded.summary.design_uuid.as_str()) {
        return Err(LoadProblem::OtherDesign);
    }
    let design = design_from_text(&loaded.design_text).map_err(LoadProblem::Unreadable)?;
    Ok((loaded.summary, design))
}

/// Renames variant `id`.
///
/// # Errors
///
/// A plain sentence when the variant is gone or the library refuses.
pub(super) fn rename(db: &Database, id: i64, name: &str) -> Result<(), String> {
    match db.rename_variant(id, name) {
        Ok(0) => Err(LoadProblem::Missing.to_string()),
        Ok(_) => Ok(()),
        Err(error) => Err(plain(&error)),
    }
}

/// Sets (or, with `None`, clears) the note of variant `id`.
///
/// # Errors
///
/// A plain sentence when the variant is gone or the library refuses.
pub(super) fn set_note(db: &Database, id: i64, note: Option<&str>) -> Result<(), String> {
    match db.set_variant_note(id, note) {
        Ok(0) => Err(LoadProblem::Missing.to_string()),
        Ok(_) => Ok(()),
        Err(error) => Err(plain(&error)),
    }
}

/// Deletes variant `id`. A variant that is already gone is not an error.
///
/// # Errors
///
/// A plain sentence when the library refuses.
pub(super) fn delete(db: &Database, id: i64) -> Result<(), String> {
    db.delete_variant(id)
        .map(|_| ())
        .map_err(|error| plain(&error))
}

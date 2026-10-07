//! The `[meta]` table and `[[attachments]]` a Save writes into the `.indicatrix` file.
//!
//! The file stores what an importer cannot recompute (see
//! `indicatrix_formats::native::design`'s module documentation for the full list) and
//! nothing else. A Save starts from what the open design arrived with
//! ([`DesignFileExtras`]: its `[meta]` table with unknown keys, and its attachments),
//! then overlays what the design's library row says: the row is where the owner edits
//! title, designer, source, shape, tags and the marks, so it wins for those. Fields
//! the library has no column for (`id`, `notes`, `license`, `copyright`, the creation
//! time, unknown keys) come from the session unchanged.
//!
//! Everything here is plain data in, plain data out: [`assemble`] and
//! [`merge_attachments`] touch neither the database nor the clock, so they are tested
//! directly; [`prepare_design_extras`] is the thin database-and-clock wrapper.

use crate::{
    bridge::export_thread::filename_template::civil_from_unix_seconds,
    gui::editor::state::{DesignFileExtras, fresh_design_uuid},
};
use indicatrix_cut_core::native::{
    AttachmentBlob, AttachmentRole, DesignMetadata, LEGACY_NATIVE_EXTENSION_SUFFIX,
    NATIVE_EXTENSION_SUFFIX,
};
use indicatrix_formats::native::design::MAX_ATTACHMENT_BYTES;
use indicatrix_vault::{
    db::sqlite::Database,
    local::is_native_design_name,
    model::{entry::FullDiagramRecord, file::AttachedFile},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// What the design's library row holds, read once per Save.
pub(super) struct RowState {
    record: FullDiagramRecord,
    tags: Vec<String>,
    ignored: bool,
    planner_excluded: bool,
}

/// What a Save writes: the `[meta]` table and the attachments, in file order.
pub(super) struct PreparedExtras {
    /// The `[meta]` table, with `id`, `created_at` and `modified_at` set.
    pub(super) metadata: DesignMetadata,
    /// The attachments, sorted by name.
    pub(super) attachments: Vec<AttachmentBlob>,
}

/// `value` trimmed, or `""` for an absent one.
fn plain(value: Option<&str>) -> String {
    value.map(str::trim).unwrap_or_default().to_string()
}

/// Formats Unix seconds as ISO-8601 UTC (`2026-10-02T09:30:00Z`).
pub(super) fn iso8601_utc(unix_seconds: i64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix_seconds(unix_seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// A random version-4 UUID as 8-4-4-4-12 lowercase hexadecimal -- the id a Save gives a
/// design that somehow reaches it without one.
///
/// Every design opened or created in the editor already has its UUID by then (see
/// `state::design_identity`), so this only fires for a state built without going through
/// those paths.
pub(super) fn new_design_id() -> String {
    fresh_design_uuid()
}

/// Overlays what the row says onto `meta`: the fields the library edits (title,
/// designer, source, shape, competition, PDF/`.gem` names, tags, the two marks).
fn overlay_row(meta: &mut DesignMetadata, row: &RowState) {
    let record = &row.record;
    meta.title = record.title.trim().to_string();
    meta.designer = plain(record.designer.as_deref());
    meta.designer_info = plain(record.designer_info.as_deref());
    meta.source_citation = plain(record.source_citation.as_deref());
    meta.source_url = plain(Some(&record.page_url));
    meta.source_design_id = plain(record.design_id.as_deref());
    meta.shape = plain(record.shape.as_deref());
    meta.shape_category = plain(record.shape_category.as_deref());
    meta.competition = plain(record.competition_diagram.as_deref());
    meta.pdf_file = plain(record.pdf_file.as_deref());
    meta.gem_file = plain(record.gem_file.as_deref());
    meta.tags.clone_from(&row.tags);
    meta.ignored = row.ignored;
    meta.planner_excluded = row.planner_excluded;
}

/// Sorts and de-duplicates the tags (deterministic output) and drops blank ones.
fn normalise_tags(tags: &mut Vec<String>) {
    for tag in tags.iter_mut() {
        *tag = tag.trim().to_string();
    }
    tags.retain(|tag| !tag.is_empty());
    tags.sort();
    tags.dedup();
}

/// The `[meta]` table to write: `base` (what the design arrived with) overlaid with
/// the library row when there is one, then `id` (a fresh one from `new_id` when
/// absent), `created_at` (`now` when absent) and `modified_at` (`now`) set.
pub(super) fn assemble_metadata(
    base: &DesignMetadata,
    row: Option<&RowState>,
    now: &str,
    new_id: impl FnOnce() -> String,
) -> DesignMetadata {
    let mut meta = base.clone();
    if let Some(row) = row {
        overlay_row(&mut meta, row);
    }
    normalise_tags(&mut meta.tags);
    if meta.id.is_empty() {
        meta.id = new_id();
    }
    if meta.created_at.is_empty() {
        meta.created_at = now.to_string();
    }
    meta.modified_at = now.to_string();
    meta
}

/// `name` as a valid attachment name: path separators and control characters become
/// `_`, over-long names are cut on a character boundary, an empty one becomes
/// `attachment`.
fn safe_name(name: &str) -> String {
    let mut cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    while cleaned.len() > 255 {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        "attachment".to_string()
    } else {
        cleaned
    }
}

/// What an attached file is for, from its extension.
fn role_for_name(name: &str) -> AttachmentRole {
    let has = |ext: &str| {
        std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
    };
    if has("pdf") {
        AttachmentRole::Pdf
    } else if has("gem") {
        AttachmentRole::Gem
    } else if has("asc") {
        AttachmentRole::Asc
    } else {
        AttachmentRole::Other
    }
}

/// `true` for a design file or an older paired sidecar: the file being written replaces
/// them, so they are never nested inside it.
fn is_design_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_native_design_name(name)
        || lower.ends_with(NATIVE_EXTENSION_SUFFIX)
        || lower.ends_with(LEGACY_NATIVE_EXTENSION_SUFFIX)
}

/// The row's files as attachments: every attached file except design files and
/// sidecars, with at most one `.asc`, plus the diagram image under its own role.
pub(super) fn row_attachments(
    files: &[AttachedFile],
    image: Option<(&str, &[u8])>,
) -> Vec<AttachmentBlob> {
    let mut out: Vec<AttachmentBlob> = Vec::new();
    for file in files.iter().filter(|f| !is_design_file_name(&f.name)) {
        let name = safe_name(&file.name);
        let role = role_for_name(&name);
        let duplicate = out.iter().any(|b| b.name == name);
        let second_asc =
            role == AttachmentRole::Asc && out.iter().any(|b| b.role == AttachmentRole::Asc);
        if duplicate || second_asc {
            continue;
        }
        let mut blob = AttachmentBlob::new(name, role, file.content.clone());
        blob.source_url.clone_from(&file.url);
        out.push(blob);
    }
    if let Some((name, data)) = image
        && !data.is_empty()
    {
        let name = safe_name(name);
        out.retain(|b| b.name != name);
        out.push(AttachmentBlob::new(
            name,
            AttachmentRole::DiagramImage,
            data.to_vec(),
        ));
    }
    out
}

/// The attachments to write: `base` (what the design arrived with) merged with the
/// row's, the row winning a name clash. The `.asc` and diagram-image roles hold one
/// file each, so a row's replaces the session's even under another name. Sorted by
/// name, so the same inputs always write the same bytes.
pub(super) fn merge_attachments(
    base: &[AttachmentBlob],
    row: Vec<AttachmentBlob>,
) -> Vec<AttachmentBlob> {
    let row_has = |role| row.iter().any(|b| b.role == role);
    let drop_asc = row_has(AttachmentRole::Asc);
    let drop_image = row_has(AttachmentRole::DiagramImage);
    let mut by_name: BTreeMap<String, AttachmentBlob> = base
        .iter()
        .filter(|b| {
            !(drop_asc && b.role == AttachmentRole::Asc
                || drop_image && b.role == AttachmentRole::DiagramImage)
        })
        .map(|b| (b.name.clone(), b.clone()))
        .collect();
    for blob in row {
        by_name.insert(blob.name.clone(), blob);
    }
    by_name.into_values().collect()
}

/// Refuses a set of attachments the file format cannot hold, before any of it is
/// encoded: a ready-to-toast message naming the total and the limit.
///
/// # Errors
///
/// The message, when the summed size exceeds [`MAX_ATTACHMENT_BYTES`].
pub(super) fn check_attachment_budget(blobs: &[AttachmentBlob]) -> Result<(), String> {
    let total: u64 = blobs.iter().map(|b| b.data.len() as u64).sum();
    if total > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "the attached files total {} MiB, more than the {} MiB a design file holds -- \
             remove some of the design's attachments in the library, then save again",
            total.div_ceil(1024 * 1024),
            MAX_ATTACHMENT_BYTES / (1024 * 1024),
        ));
    }
    Ok(())
}

/// Builds the metadata and attachments for one Save from `base`, the library row (when
/// there is one) and the clock value `now`.
///
/// # Errors
///
/// A ready-to-toast message when the attachments exceed the file's size limit.
pub(super) fn assemble(
    base: &DesignFileExtras,
    row: Option<&RowState>,
    now: &str,
    new_id: impl FnOnce() -> String,
) -> Result<PreparedExtras, String> {
    let row_blobs = row.map_or_else(Vec::new, |row| {
        let record = &row.record;
        let image = record
            .diagram_image_name
            .as_deref()
            .zip(record.diagram_image_data.as_deref());
        row_attachments(&record.attached_files, image)
    });
    let attachments = merge_attachments(&base.attachments, row_blobs);
    check_attachment_budget(&attachments)?;
    Ok(PreparedExtras {
        metadata: assemble_metadata(&base.metadata, row, now, new_id),
        attachments,
    })
}

/// Reads `entry_id`'s row for a Save: `Ok(None)` when no row has that id (deleted while
/// the design was open).
fn read_row(db: &Database, entry_id: i64) -> Result<Option<RowState>, String> {
    let read =
        |what: &str, e: anyhow::Error| format!("could not read the library row ({what}): {e}");
    let Some(record) = db
        .get_diagram_full(entry_id)
        .map_err(|e| read("details", e))?
    else {
        return Ok(None);
    };
    let tags = db
        .tags_for_entry(entry_id)
        .map_err(|e| read("tags", e))?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let ignored = db
        .is_diagram_ignored(entry_id)
        .map_err(|e| read("ignored mark", e))?;
    let planner_excluded = db
        .planner_excluded_among(&[entry_id])
        .map_err(|e| read("planner exclusion", e))?
        .contains(&entry_id);
    Ok(Some(RowState {
        record,
        tags,
        ignored,
        planner_excluded,
    }))
}

/// The metadata and attachments the Save of the design with library row
/// `source_entry_id` (when it has one) writes, stamped with the current time.
///
/// # Errors
///
/// A ready-to-toast message when the row cannot be read or the attachments exceed the
/// file's size limit.
pub(super) fn prepare_design_extras(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    base: &DesignFileExtras,
) -> Result<PreparedExtras, String> {
    let row = match source_entry_id {
        Some(id) => {
            let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            read_row(&db, id)?
        }
        None => None,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    assemble(base, row.as_ref(), &iso8601_utc(now), new_design_id)
}

#[cfg(test)]
impl RowState {
    /// A row state built from plain parts, for the tests.
    pub(super) const fn for_test(
        record: FullDiagramRecord,
        tags: Vec<String>,
        ignored: bool,
        planner_excluded: bool,
    ) -> Self {
        Self {
            record,
            tags,
            ignored,
            planner_excluded,
        }
    }
}

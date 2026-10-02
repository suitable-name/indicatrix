//! Cataloguing a self-contained `.indicatrix` design file: recognising its attachment
//! by name and turning one into the entry and detail rows a library import stores.
//!
//! The file carries the whole design, so a standalone one needs no `.asc` beside it;
//! the angle-settings table and header metadata are read from its own tiers and
//! schedule table. A `.indicatrix` file next to a `.asc`/`.gem`/`.gcs` is attached to
//! that design instead (see `design_attachment_position` for the cutting-instructions
//! file the other readers use).

use super::{ImportedAsc, ImportedExtras, catalogue_rows};
use crate::{db::sqlite::Database, model::file::AttachedFile};
use indicatrix_formats::{
    asc::{AscLineEnding, AscSchedule, AscTier},
    native::design::{
        self, AttachmentBlob, AttachmentRole, DESIGN_EXTENSION, DesignFile, DesignMetadata,
        attachment_blobs,
    },
};
use std::path::Path;

/// `true` when `file_name` names a self-contained design file: its last extension is
/// `indicatrix`, compared case-insensitively. A `name.indicatrix.toml` overlay sidecar
/// is not one.
#[must_use]
pub fn is_native_design_name(file_name: &str) -> bool {
    design::is_design_path(Path::new(file_name))
}

/// The position (in `names`' order) of the first attachment that is a self-contained
/// design file, if any.
#[must_use]
pub fn native_design_attachment_position<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Option<usize> {
    names.into_iter().position(is_native_design_name)
}

/// The schedule a design file's tiers and schedule table describe, for the catalogue
/// columns (title, gear, symmetry, facet count, angle table).
///
/// Mast distances are not part of that view and stay `0.0`; nothing here is solved.
fn schedule_of(file: &DesignFile) -> AscSchedule {
    let schedule = &file.schedule;
    let tiers = file
        .tiers
        .iter()
        .map(|tier| AscTier {
            angle_deg: tier.angle_deg.unwrap_or(0.0),
            mast: 0.0,
            name: tier.name.clone(),
            indices: tier.indices.clone().unwrap_or_default(),
            index_names: Vec::new(),
            notes: tier
                .original_notes
                .clone()
                .or_else(|| tier.note.clone())
                .unwrap_or_default(),
        })
        .collect();
    AscSchedule {
        gemcad_version: schedule.gemcad_version.clone(),
        gear_teeth: schedule.gear_teeth,
        gear_reference_angle: schedule.gear_reference_angle,
        symmetry_order: schedule.symmetry_order,
        mirror: schedule.mirror,
        refractive_index: schedule.refractive_index,
        headers: schedule.headers.clone(),
        footnotes: schedule.footnotes.clone(),
        tiers,
        warnings: Vec::new(),
        line_ending: AscLineEnding::default(),
    }
}

/// `value` trimmed, or `None` when nothing is left.
fn text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Sets `slot` from `value` when `value` holds text; an unset field leaves `slot` alone.
fn set_text(slot: &mut Option<String>, value: &str) {
    if let Some(value) = text(value) {
        *slot = Some(value);
    }
}

/// Copies the `[meta]` fields that have a counterpart in the library row into
/// `imported`, and the row-side state (tags, marks) into [`ImportedAsc::extras`].
///
/// Only the stored fields: the derived ones (ratios, volume, facet count, the angle
/// table) stay what [`catalogue_rows`] computed from the tiers. `id`, `notes`,
/// `license`, `copyright` and the timestamps have no column here; they stay in the
/// design file, which the row keeps as an attachment.
fn apply_meta(imported: &mut ImportedAsc, meta: &DesignMetadata) {
    if let Some(title) = text(&meta.title) {
        imported.entry.title = title;
    }
    if let Some(design_id) = text(&meta.source_design_id) {
        imported.entry.design_id = design_id;
    }
    let detail = &mut imported.detail;
    if let Some(url) = text(&meta.source_url) {
        detail.page_url = url;
    }
    set_text(&mut detail.designer, &meta.designer);
    set_text(&mut detail.designer_info, &meta.designer_info);
    set_text(&mut detail.source_citation, &meta.source_citation);
    set_text(&mut detail.shape, &meta.shape);
    set_text(&mut detail.competition_diagram, &meta.competition);
    set_text(&mut detail.pdf_file, &meta.pdf_file);
    set_text(&mut detail.gem_file, &meta.gem_file);
    if let Some(category) = text(&meta.shape_category)
        && category.parse::<i64>().is_ok()
    {
        detail.shape_category = Some(category);
    }
    let mut tags: Vec<String> = meta.tags.iter().filter_map(|t| text(t)).collect();
    tags.sort();
    tags.dedup();
    imported.extras = ImportedExtras {
        tags,
        ignored: meta.ignored,
        planner_excluded: meta.planner_excluded,
    };
}

/// Adds the design file's attachments to `imported`: the diagram image goes to the
/// row's image fields (when it has none yet), every other file becomes an attached
/// file unless one of that name is already there. A design file carries at most the
/// one `.asc` the row already holds from the schedule, so a second `.asc` is not added.
fn apply_attachments(imported: &mut ImportedAsc, blobs: Vec<AttachmentBlob>) {
    let is_asc = |name: &str| name.to_ascii_lowercase().ends_with(".asc");
    let detail = &mut imported.detail;
    for blob in blobs {
        if blob.role == AttachmentRole::DiagramImage {
            if detail.diagram_image_data.is_none() {
                detail.diagram_image_name = Some(blob.name);
                detail.diagram_image_data = Some(blob.data);
            }
            continue;
        }
        let duplicate = detail.attached_files.iter().any(|f| f.name == blob.name);
        let second_asc = blob.role == AttachmentRole::Asc
            && detail.attached_files.iter().any(|f| is_asc(&f.name));
        if !duplicate && !second_asc {
            detail.attached_files.push(AttachedFile {
                name: blob.name,
                url: blob.source_url,
                content: blob.data,
            });
        }
    }
}

/// Fills `imported`'s library row from the design file `file`.
///
/// The stored `[meta]` fields (title, designer, source, shape, competition, PDF/`.gem`
/// names, ...) go to the row, the row-side marks to [`ImportedAsc::extras`], and every
/// `[[attachments]]` file becomes an attachment.
///
/// Everything the file does NOT store is left as the schedule import produced it, so
/// the derived columns are recomputed exactly as for any other import.
///
/// # Errors
///
/// A message when an attachment does not decode or verify.
pub fn apply_design_file_meta(imported: &mut ImportedAsc, file: &DesignFile) -> Result<(), String> {
    let blobs = attachment_blobs(&file.attachments).map_err(|e| e.to_string())?;
    apply_meta(imported, &file.meta);
    apply_attachments(imported, blobs);
    Ok(())
}

/// Writes the row state in `extras` onto the saved row `entry_id`.
///
/// That is the tags, the ignored mark and the Rough Planner exclusion. Only sets: an
/// unset mark never clears one the row already has, and a tag the row already carries
/// is a no-op.
///
/// # Errors
///
/// The underlying database error.
pub fn apply_imported_extras(
    db: &Database,
    entry_id: i64,
    extras: &ImportedExtras,
) -> anyhow::Result<()> {
    for tag in &extras.tags {
        db.add_tag_to_entry(entry_id, tag)?;
    }
    if extras.ignored {
        db.set_diagram_ignored(entry_id, true)?;
    }
    if extras.planner_excluded {
        db.set_planner_excluded(entry_id, true)?;
    }
    Ok(())
}

/// Parses one `.indicatrix` file's `bytes` into an [`ImportedAsc`] ready to save into
/// the local library, with the file itself as its first attachment.
///
/// The `[meta]` table and the `[[attachments]]` fill the row through
/// [`apply_design_file_meta`]; the ratios, volume, facet count and angle table come
/// from the tiers and schedule, never from the file's metadata.
///
/// `file_name` gives the title fallback (its stem) and the synthetic
/// `local://<file_name>` url, which dedupes a re-import by name exactly like a `.asc`
/// import does. The measured proportions are left for the caller to fill in from the
/// angle table, as for a `.asc`.
///
/// # Errors
///
/// A human-readable message when `bytes` is not UTF-8 text, is not a design file, or
/// is one this build cannot read (a newer major version, a malformed table).
pub fn import_native_design(file_name: &str, bytes: &[u8]) -> Result<ImportedAsc, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| format!("the design file is not UTF-8 text: {e}"))?;
    let file = design::parse(text).map_err(|e| e.to_string())?;
    let schedule = schedule_of(&file);
    let suffix = format!(".{DESIGN_EXTENSION}");
    let stem = file_name
        .len()
        .checked_sub(suffix.len())
        .filter(|&at| file_name.is_char_boundary(at))
        .filter(|&at| file_name[at..].eq_ignore_ascii_case(&suffix))
        .map_or(file_name, |at| &file_name[..at]);
    let mut imported = catalogue_rows(
        file_name,
        stem,
        &schedule,
        vec![AttachedFile {
            name: file_name.to_string(),
            url: String::new(),
            content: bytes.to_vec(),
        }],
    );
    apply_design_file_meta(&mut imported, &file)?;
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_formats::native::{
        MaterialTable, NativeMeetConstraint, NativePreformShape, PreformTable, TierTable,
        design::ScheduleTable,
    };
    use std::collections::BTreeSet;

    fn sample_design_file() -> DesignFile {
        let mut tier = TierTable::new(
            "P1",
            NativeMeetConstraint::ScaleReference { mast: 0.5 },
            Vec::new(),
        );
        tier.angle_deg = Some(-41.0);
        tier.indices = Some(vec![0.0, 24.0, 48.0, 72.0]);
        let schedule = ScheduleTable {
            gemcad_version: "5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 4,
            mirror: true,
            refractive_index: 1.54,
            headers: vec!["Stand-alone Brilliant".to_string()],
            footnotes: Vec::new(),
            unknown: std::iter::empty().collect(),
        };
        DesignFile::new(
            PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
            MaterialTable::new(None, None, None),
            schedule,
            None,
            vec![tier],
        )
    }

    fn sample_file_text() -> String {
        design::to_string(&sample_design_file()).expect("serializes")
    }

    fn described_file() -> DesignFile {
        let meta = DesignMetadata {
            title: "Capps Brilliant".to_string(),
            designer: "Capps, Jerry".to_string(),
            designer_info: "Capps, Jerry; Lapidary Journal, May 1994".to_string(),
            source_citation: "Lapidary Journal, May 1994".to_string(),
            source_url: "https://example.org/capps".to_string(),
            source_design_id: "cb-17".to_string(),
            shape: "Round".to_string(),
            shape_category: "5".to_string(),
            competition: "Masters".to_string(),
            pdf_file: "capps.pdf".to_string(),
            gem_file: "capps.gem".to_string(),
            notes: "Cut in quartz first".to_string(),
            tags: vec![
                "b-tag".to_string(),
                "a-tag".to_string(),
                "a-tag".to_string(),
            ],
            planner_excluded: true,
            ignored: true,
            ..DesignMetadata::default()
        };
        sample_design_file().with_meta(meta).with_attachments(&[
            AttachmentBlob::new("capps.pdf", AttachmentRole::Pdf, b"%PDF".to_vec()),
            AttachmentBlob::new("capps.png", AttachmentRole::DiagramImage, vec![1, 2, 3]),
            AttachmentBlob::new("capps.asc", AttachmentRole::Asc, b"GemCad".to_vec()),
        ])
    }

    #[test]
    fn names_are_recognised_by_their_last_extension() {
        assert!(is_native_design_name("a.indicatrix"));
        assert!(is_native_design_name("A.INDICATRIX"));
        assert!(!is_native_design_name("a.indicatrix.toml"));
        assert!(!is_native_design_name("a.asc"));
        assert_eq!(
            native_design_attachment_position(["a.asc", "a.indicatrix.toml", "a.indicatrix"]),
            Some(2)
        );
    }

    #[test]
    fn a_standalone_design_file_becomes_a_catalogue_row_with_itself_attached() {
        let text = sample_file_text();
        let imported =
            import_native_design("brilliant.indicatrix", text.as_bytes()).expect("imports");
        assert_eq!(imported.entry.title, "Stand-alone Brilliant");
        assert_eq!(imported.entry.url, "local://brilliant.indicatrix");
        assert_eq!(imported.detail.angle_settings_table.len(), 1);
        assert_eq!(imported.detail.angle_settings_table[0].facet, "P1");
        assert_eq!(imported.detail.index_gear.as_deref(), Some("96"));
        assert_eq!(imported.detail.attached_files.len(), 1);
        assert_eq!(
            imported.detail.attached_files[0].name,
            "brilliant.indicatrix"
        );
        assert_eq!(imported.detail.attached_files[0].content, text.as_bytes());
    }

    #[test]
    fn the_title_falls_back_to_the_file_stem() {
        let text = sample_file_text().replace("Stand-alone Brilliant", "");
        let imported = import_native_design("Nameless.indicatrix", text.as_bytes()).expect("ok");
        assert_eq!(imported.entry.title, "Nameless");
    }

    #[test]
    fn something_that_is_not_a_design_file_is_refused() {
        assert!(import_native_design("x.indicatrix", b"not a design").is_err());
        assert!(import_native_design("x.indicatrix", &[0xFF, 0xFE]).is_err());
    }
    #[test]
    fn meta_fills_the_row_and_the_derived_columns_still_come_from_the_tiers() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.indicatrix", text.as_bytes()).expect("imports");
        assert_eq!(imported.entry.title, "Capps Brilliant");
        assert_eq!(imported.entry.design_id, "cb-17");
        let detail = &imported.detail;
        assert_eq!(detail.designer.as_deref(), Some("Capps, Jerry"));
        assert_eq!(
            detail.designer_info.as_deref(),
            Some("Capps, Jerry; Lapidary Journal, May 1994")
        );
        assert_eq!(detail.page_url, "https://example.org/capps");
        assert_eq!(detail.shape.as_deref(), Some("Round"));
        assert_eq!(detail.shape_category.as_deref(), Some("5"));
        assert_eq!(detail.competition_diagram.as_deref(), Some("Masters"));
        assert_eq!(detail.pdf_file.as_deref(), Some("capps.pdf"));
        assert_eq!(detail.gem_file.as_deref(), Some("capps.gem"));
        // Derived, not read from `[meta]`.
        assert_eq!(detail.index_gear.as_deref(), Some("96"));
        assert_eq!(detail.symmetry_order.as_deref(), Some("4"));
        assert_eq!(detail.angle_settings_table.len(), 1);
        assert!(detail.lw_ratio.is_none() && detail.volume.is_none());
    }

    #[test]
    fn every_attachment_is_kept_and_the_diagram_image_goes_to_the_image_fields() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.indicatrix", text.as_bytes()).expect("imports");
        let names: Vec<&str> = imported
            .detail
            .attached_files
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["capps.indicatrix", "capps.pdf", "capps.asc"]);
        assert_eq!(imported.detail.attached_files[1].content, b"%PDF");
        assert_eq!(
            imported.detail.diagram_image_name.as_deref(),
            Some("capps.png")
        );
        assert_eq!(
            imported.detail.diagram_image_data.as_deref(),
            Some(&[1u8, 2, 3][..])
        );
    }

    #[test]
    fn tags_are_sorted_and_deduplicated_in_the_extras() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.indicatrix", text.as_bytes()).expect("imports");
        assert_eq!(imported.extras.tags, ["a-tag", "b-tag"]);
        assert!(imported.extras.ignored && imported.extras.planner_excluded);
    }

    #[test]
    fn the_planner_exclusion_tags_and_ignored_mark_are_restored_on_the_saved_row() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.indicatrix", text.as_bytes()).expect("imports");
        let db = Database::new(Some(":memory:")).expect("in-memory db");
        let id = db
            .save_design(
                &imported.entry,
                &imported.detail,
                super::super::LOCAL_SOURCE_ID,
            )
            .expect("saves");
        apply_imported_extras(&db, id, &imported.extras).expect("applies");
        assert_eq!(
            db.planner_excluded_ids().expect("ids"),
            BTreeSet::from([id])
        );
        assert!(db.is_diagram_ignored(id).expect("flag"));
        let tags: Vec<String> = db
            .tags_for_entry(id)
            .expect("tags")
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tags, ["a-tag", "b-tag"]);
    }

    #[test]
    fn a_file_without_meta_imports_exactly_as_before() {
        let text = sample_file_text();
        let imported = import_native_design("plain.indicatrix", text.as_bytes()).expect("imports");
        assert_eq!(imported.extras, ImportedExtras::default());
        assert!(imported.detail.designer.is_none());
        assert!(imported.detail.diagram_image_data.is_none());
        assert_eq!(imported.detail.attached_files.len(), 1);
    }
}

//! Cataloguing a self-contained `.isdd` design file (or one saved as `.indicatrix`
//! before; both are read the same way): recognising its attachment
//! by name and turning one into the entry and detail rows a library import stores.
//!
//! The file carries the whole design, so a standalone one needs no `.asc` beside it;
//! the angle-settings table and header metadata are read from its own tiers and
//! schedule table. A design file next to a `.asc`/`.gem`/`.gcs` is attached to
//! that design instead (see `design_attachment_position` for the cutting-instructions
//! file the other readers use).

use super::{ImportedAsc, ImportedExtras, catalogue_rows};
use crate::{
    db::sqlite::Database,
    model::{
        file::AttachedFile,
        frosting::{StoredTierFrosting, normalised},
    },
};
use indicatrix_formats::{
    asc::{AscLineEnding, AscSchedule, AscTier},
    native::design::{
        self, AttachmentBlob, AttachmentRole, DESIGN_READ_EXTENSIONS, DesignFile, DesignMetadata,
        attachment_blobs,
    },
};
use std::path::Path;

/// `true` when `file_name` names a self-contained design file.
///
/// Its last extension is `isdd` or the earlier `indicatrix`, compared case-insensitively. A
/// `name.indicatrix.toml` name (the removed overlay sidecar) is not one.
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
        // Set from the tiers, not the metadata: see `apply_design_file_meta`.
        frosting: Vec::new(),
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
    imported.extras.frosting = frosting_of(file);
    apply_attachments(imported, blobs);
    Ok(())
}

/// Two index-wheel positions this close are the same facet (the editor's own tolerance for
/// naming a frosted facet).
const SAME_INDEX_EPSILON: f64 = 1e-6;

/// The frosting `file`'s tiers state (`frosted` for a whole tier, `frosted_facets` for single
/// facets), by tier position and by facet slot in the tier's `indices`: the form the library
/// stores. A frosted facet position that is none of the tier's `indices` names nothing and is
/// dropped.
fn frosting_of(file: &DesignFile) -> Vec<StoredTierFrosting> {
    let rows = file
        .tiers
        .iter()
        .enumerate()
        .filter_map(|(position, tier)| {
            let tier_position = u32::try_from(position).ok()?;
            if tier.frosted {
                return Some(StoredTierFrosting::whole(tier_position));
            }
            let indices = tier.indices.as_deref()?;
            let facets: Vec<u32> = indices
                .iter()
                .enumerate()
                .filter(|&(_, &index)| {
                    tier.frosted_facets
                        .iter()
                        .any(|&frosted| (frosted - index).abs() < SAME_INDEX_EPSILON)
                })
                .filter_map(|(slot, _)| u32::try_from(slot).ok())
                .collect();
            (!facets.is_empty()).then_some(StoredTierFrosting {
                tier: tier_position,
                facets,
            })
        });
    normalised(rows.collect())
}

/// Writes the row state in `extras` onto the saved row `entry_id`.
///
/// That is the tags, the ignored mark, the Rough Planner exclusion and the frosting. The
/// marks only set: an unset mark never clears one the row already has, and a tag the row
/// already carries is a no-op. The frosting REPLACES the row's (an import that no longer
/// states frosting clears it), since it describes the geometry the import just wrote.
///
/// # Errors
///
/// The underlying database error.
pub fn apply_imported_extras(
    db: &Database,
    entry_id: i64,
    extras: &ImportedExtras,
) -> anyhow::Result<()> {
    db.set_diagram_frosting(entry_id, &extras.frosting)?;
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

/// `file_name` without its design extension (`.isdd` or `.indicatrix`, compared
/// case-insensitively); the whole name when it ends in neither.
fn design_file_stem(file_name: &str) -> &str {
    DESIGN_READ_EXTENSIONS
        .iter()
        .find_map(|extension| {
            let suffix = format!(".{extension}");
            file_name
                .len()
                .checked_sub(suffix.len())
                .filter(|&at| file_name.is_char_boundary(at))
                .filter(|&at| file_name[at..].eq_ignore_ascii_case(&suffix))
                .map(|at| &file_name[..at])
        })
        .unwrap_or(file_name)
}

/// Parses one design file's (`.isdd` or `.indicatrix`) `bytes` into an [`ImportedAsc`] ready to save into
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
    let stem = design_file_stem(file_name);
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
        design_file_of(vec![sample_tier("P1", -41.0)])
    }

    fn sample_tier(name: &str, angle_deg: f64) -> TierTable {
        let mut tier = TierTable::new(
            name,
            NativeMeetConstraint::ScaleReference { mast: 0.5 },
            Vec::new(),
        );
        tier.angle_deg = Some(angle_deg);
        tier.indices = Some(vec![0.0, 24.0, 48.0, 72.0]);
        tier
    }

    fn design_file_of(tiers: Vec<TierTable>) -> DesignFile {
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
            tiers,
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
        assert!(is_native_design_name("a.isdd"));
        assert!(is_native_design_name("A.ISDD"));
        // The earlier extension is still a design file.
        assert!(is_native_design_name("a.indicatrix"));
        assert!(is_native_design_name("A.INDICATRIX"));
        // The removed overlay sidecar names and other files are not.
        assert!(!is_native_design_name("a.indicatrix.toml"));
        assert!(!is_native_design_name("a.gemcut.toml"));
        assert!(!is_native_design_name("a.asc"));
        assert_eq!(
            native_design_attachment_position(["a.asc", "a.indicatrix.toml", "a.isdd"]),
            Some(2)
        );
        assert_eq!(
            native_design_attachment_position(["a.asc", "a.indicatrix"]),
            Some(1)
        );
        assert_eq!(
            native_design_attachment_position(["a.asc", "a.indicatrix.toml"]),
            None
        );
    }

    #[test]
    fn a_legacy_indicatrix_file_imports_like_an_isdd_one() {
        let text = sample_file_text();
        let legacy = import_native_design("Legacy.indicatrix", text.as_bytes()).expect("imports");
        let current = import_native_design("Legacy.isdd", text.as_bytes()).expect("imports");
        assert_eq!(legacy.entry.title, current.entry.title);
        assert_eq!(legacy.entry.url, "local://Legacy.indicatrix");
        assert_eq!(current.entry.url, "local://Legacy.isdd");
        assert_eq!(
            legacy.detail.attached_files[0].name, "Legacy.indicatrix",
            "the stored name is never rewritten"
        );
        assert_eq!(design_file_stem("Legacy.INDICATRIX"), "Legacy");
        assert_eq!(design_file_stem("Legacy.Isdd"), "Legacy");
        assert_eq!(design_file_stem("Legacy.asc"), "Legacy.asc");
    }

    #[test]
    fn a_standalone_design_file_becomes_a_catalogue_row_with_itself_attached() {
        let text = sample_file_text();
        let imported = import_native_design("brilliant.isdd", text.as_bytes()).expect("imports");
        assert_eq!(imported.entry.title, "Stand-alone Brilliant");
        assert_eq!(imported.entry.url, "local://brilliant.isdd");
        assert_eq!(imported.detail.angle_settings_table.len(), 1);
        assert_eq!(imported.detail.angle_settings_table[0].facet, "P1");
        assert_eq!(imported.detail.index_gear.as_deref(), Some("96"));
        assert_eq!(imported.detail.attached_files.len(), 1);
        assert_eq!(imported.detail.attached_files[0].name, "brilliant.isdd");
        assert_eq!(imported.detail.attached_files[0].content, text.as_bytes());
    }

    #[test]
    fn the_title_falls_back_to_the_file_stem() {
        let text = sample_file_text().replace("Stand-alone Brilliant", "");
        let imported = import_native_design("Nameless.isdd", text.as_bytes()).expect("ok");
        assert_eq!(imported.entry.title, "Nameless");
    }

    #[test]
    fn something_that_is_not_a_design_file_is_refused() {
        assert!(import_native_design("x.isdd", b"not a design").is_err());
        assert!(import_native_design("x.isdd", &[0xFF, 0xFE]).is_err());
    }
    #[test]
    fn meta_fills_the_row_and_the_derived_columns_still_come_from_the_tiers() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.isdd", text.as_bytes()).expect("imports");
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
        let imported = import_native_design("capps.isdd", text.as_bytes()).expect("imports");
        let names: Vec<&str> = imported
            .detail
            .attached_files
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["capps.isdd", "capps.pdf", "capps.asc"]);
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
        let imported = import_native_design("capps.isdd", text.as_bytes()).expect("imports");
        assert_eq!(imported.extras.tags, ["a-tag", "b-tag"]);
        assert!(imported.extras.ignored && imported.extras.planner_excluded);
    }

    #[test]
    fn the_planner_exclusion_tags_and_ignored_mark_are_restored_on_the_saved_row() {
        let text = design::to_string(&described_file()).expect("serializes");
        let imported = import_native_design("capps.isdd", text.as_bytes()).expect("imports");
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

    /// Three tiers: the first frosted as a whole, the second at two of its four facets
    /// (and at a position the tier does not have), the third polished.
    fn frosted_design_file() -> DesignFile {
        design_file_of(vec![
            sample_tier("C1", 35.0).with_frosting(true, Vec::new()),
            sample_tier("C2", 41.0).with_frosting(false, vec![24.0, 72.0, 13.0]),
            sample_tier("C3", 45.0),
        ])
    }

    #[test]
    fn a_design_files_frosted_tiers_and_facets_are_read_by_tier_position_and_slot() {
        let text = design::to_string(&frosted_design_file()).expect("serializes");
        let imported = import_native_design("frosted.isdd", text.as_bytes()).expect("imports");
        assert_eq!(
            imported.extras.frosting,
            vec![
                StoredTierFrosting::whole(0),
                StoredTierFrosting {
                    tier: 1,
                    facets: vec![1, 3]
                },
            ]
        );
    }

    #[test]
    fn a_gcs_import_records_its_frosting_through_with_frosting() {
        use indicatrix_formats::gcs::GcsTierFrosting;
        let text = sample_file_text();
        let imported = import_native_design("plain.isdd", text.as_bytes())
            .expect("imports")
            .with_frosting(&[
                (2, GcsTierFrosting::Facets(vec![0, 2])),
                (0, GcsTierFrosting::Whole),
            ]);
        assert_eq!(
            imported.extras.frosting,
            vec![
                StoredTierFrosting::whole(0),
                StoredTierFrosting {
                    tier: 2,
                    facets: vec![0, 2]
                },
            ]
        );
    }

    #[test]
    fn the_frosting_is_stored_with_the_row_and_a_reimport_without_it_clears_it() {
        let text = design::to_string(&frosted_design_file()).expect("serializes");
        let imported = import_native_design("frosted.isdd", text.as_bytes()).expect("imports");
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
            db.diagram_frosting(id).expect("reads"),
            imported.extras.frosting
        );
        assert_eq!(
            db.get_diagram_full(id)
                .expect("reads")
                .expect("a row")
                .frosting,
            imported.extras.frosting
        );

        // The same design re-imported from a file that states no frosting: the row's
        // frosting is replaced, not kept.
        apply_imported_extras(&db, id, &ImportedExtras::default()).expect("applies");
        assert_eq!(db.diagram_frosting(id).expect("reads").len(), 0);
    }

    #[test]
    fn a_file_without_meta_imports_exactly_as_before() {
        let text = sample_file_text();
        let imported = import_native_design("plain.isdd", text.as_bytes()).expect("imports");
        assert_eq!(imported.extras, ImportedExtras::default());
        assert!(imported.detail.designer.is_none());
        assert!(imported.detail.diagram_image_data.is_none());
        assert_eq!(imported.detail.attached_files.len(), 1);
    }
}

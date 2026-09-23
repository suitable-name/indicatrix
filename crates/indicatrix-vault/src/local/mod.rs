//! Local import/export for the user's own `.asc` files -- independent of any online
//! source.
//!
//! Everything here only ever reads a file the user handed it directly. Nothing in
//! this module touches the network -- see this crate's doc comment on why that
//! boundary is deliberate.

use crate::model::{
    angle::AngleSetting, detail::FacetDiagramDetail, entry::FacetDiagramEntry, file::AttachedFile,
};
use indicatrix_formats::asc::{self, AscSchedule, AscTier};

/// `diagram_entries.source_id` every locally-imported `.asc` design is attributed to.
///
/// Distinguishes "the user's own file" from anything mirrored in from another
/// catalogue (see `db::sqlite`'s `source_id` column). Deliberately a plain string
/// literal: `source_id` is an open-ended column, and this crate must not grow a
/// dependency on whatever produces any other value for it.
pub const LOCAL_SOURCE_ID: &str = "local-import";

/// One `.asc` file, parsed and packaged for `db::sqlite::Database::save_diagram_entry`
/// / `save_diagram_detail`.
pub struct ImportedAsc {
    pub entry: FacetDiagramEntry,
    pub detail: FacetDiagramDetail,
    /// The catalogue row this file's own text says it was derived from -- recovered
    /// from a [`SOURCE_ENTRY_FOOTNOTE_PREFIX`] footnote line, when the `.asc` was
    /// written by `gui::editor::native_io`'s Save Native/Export .asc: an
    /// export-then-reimport matches its source row and becomes a version rather than
    /// a second same-titled duplicate. `None` for a `.asc` with no such
    /// footnote -- an original hand-authored file, one scraped from another source, or
    /// one exported before this stamp existed. The caller is responsible for verifying
    /// the id still names a real row (it may have been deleted since) before recording
    /// it via `db::sqlite::Database::set_derived_from_entry_id` -- this crate's own
    /// hard rule against inventing provenance from anything OTHER than a recorded id
    /// (never a title/filename match) stops at parsing the id out; it never guesses
    /// one.
    pub derived_from_entry_id: Option<i64>,
}

/// The footnote-line prefix a `.asc`'s recorded source-catalogue-row marker starts with.
///
/// `gui::editor::native_io` (the `apps/indicatrix-cut` editor) writes it into an
/// exported/saved `.asc`'s footnotes when the design in the editor has a known
/// catalogue source row -- see [`ImportedAsc::derived_from_entry_id`]'s own doc
/// comment for why this exists and [`format_source_entry_footnote`]/
/// [`parse_source_entry_footnote`] for the two halves of the round trip. A plain `.asc`
/// footnote (`indicatrix_formats::asc`'s `F` record) rather than a native-sidecar field:
/// it survives a bare "Export .asc" with no sidecar at all, which is exactly the case
/// this item's own "export-then-reimport" scenario describes, and needs no schema
/// change to the native TOML format (a different crate this one does not own).
pub const SOURCE_ENTRY_FOOTNOTE_PREFIX: &str = "Indicatrix-Source-Entry-Id: ";

/// Builds the one footnote line [`parse_source_entry_footnote`] reads back.
///
/// `gui::editor::native_io` pushes this into `Design::meta.footnotes` (replacing any
/// previous stamp; see that module's own doc comment) before writing a `.asc` for a
/// design with a known `entry_id`.
#[must_use]
pub fn format_source_entry_footnote(entry_id: i64) -> String {
    format!("{SOURCE_ENTRY_FOOTNOTE_PREFIX}{entry_id}")
}

/// The first [`SOURCE_ENTRY_FOOTNOTE_PREFIX`]-prefixed footnote in `footnotes`, parsed
/// back into the `diagram_entries.id` it names.
///
/// `None` when no such footnote is present, or its remainder doesn't parse as a plain
/// integer (a hand-edited or corrupted file, treated as "no recorded provenance"
/// rather than an error: there is nothing to import over this).
#[must_use]
pub fn parse_source_entry_footnote(footnotes: &[String]) -> Option<i64> {
    footnotes
        .iter()
        .find_map(|f| f.strip_prefix(SOURCE_ENTRY_FOOTNOTE_PREFIX))
        .and_then(|rest| rest.trim().parse().ok())
}

/// Parses one `.asc` file's `content` (already read from disk by the caller -- this
/// function does no file I/O of its own) into an [`ImportedAsc`] ready to save into
/// the local library.
///
/// `file_name` is used only for the title fallback and the synthetic URL/attachment
/// name.
///
/// The synthetic `url` (`local://<file_name>`) is what `diagram_entries.url`'s
/// `UNIQUE` constraint dedupes against, so re-importing a file with the same name
/// updates that design in place -- mirroring how a remote source's real page URL
/// dedupes a re-sync there.
///
/// `native_sidecar`, when the caller found a `<stem>.indicatrix.toml`/`.gemcut.toml`
/// file sitting beside the `.asc` on disk, is attached as a SECOND [`AttachedFile`]
/// alongside the `.asc` itself: without it, a design that goes out through Save
/// Native and back in through Import loses every sidecar-only field (authored meet
/// constraints, preform, detached facets, material/RI override), since only the
/// `.asc` would otherwise be stored. `gui::editor::loading::design_from_full_record`
/// already prefers `indicatrix_cut_core::load_paired` whenever both attachments are
/// present, so attaching it here is the only piece this crate needs to add.
///
/// # Errors
///
/// Returns a human-readable message if `indicatrix_formats::asc::parse_asc` fails to parse
/// `content` (e.g. empty input, or a malformed required header field).
pub fn import_asc(
    file_name: &str,
    content: &str,
    native_sidecar: Option<(&str, &[u8])>,
) -> Result<ImportedAsc, String> {
    let schedule = asc::parse_asc(content).map_err(|e| e.to_string())?;

    let title = schedule
        .headers
        .first()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| strip_asc_extension(file_name).to_string());

    let entry = FacetDiagramEntry {
        title,
        url: format!("local://{file_name}"),
        design_id: String::new(),
    };

    let mut attached_files = vec![AttachedFile {
        name: file_name.to_string(),
        url: String::new(),
        content: content.as_bytes().to_vec(),
    }];
    if let Some((sidecar_name, sidecar_content)) = native_sidecar {
        attached_files.push(AttachedFile {
            name: sidecar_name.to_string(),
            url: String::new(),
            content: sidecar_content.to_vec(),
        });
    }

    let detail = FacetDiagramDetail {
        angle_settings_table: angle_settings_from_tiers(&schedule.tiers),
        attached_files,
        refractive_index: Some(schedule.refractive_index.to_string()),
        index_gear: Some(schedule.gear_teeth_abs().to_string()),
        facets_count: Some(schedule.facet_plane_count().to_string()),
        symmetry_order: Some(schedule.symmetry_order.to_string()),
        mirror_symmetry: Some(schedule.mirror),
        ..FacetDiagramDetail::default()
    };
    let derived_from_entry_id = parse_source_entry_footnote(&schedule.footnotes);

    Ok(ImportedAsc {
        entry,
        detail,
        derived_from_entry_id,
    })
}

fn strip_asc_extension(file_name: &str) -> &str {
    file_name
        .strip_suffix(".asc")
        .or_else(|| file_name.strip_suffix(".ASC"))
        .unwrap_or(file_name)
}

fn angle_settings_from_tiers(tiers: &[AscTier]) -> Vec<AngleSetting> {
    tiers
        .iter()
        .enumerate()
        .map(|(order_index, tier)| AngleSetting {
            order_index: order_index as u32,
            facet: tier.name.clone(),
            angle: format!("{}\u{b0}", tier.angle_deg),
            index: tier
                .indices
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            notes: tier.notes.clone(),
        })
        .collect()
}

/// Rebuilds an [`AscSchedule`] from a saved design's angle-settings table, for
/// exporting a design that has no original `.asc` attachment.
///
/// One example is a design whose schedule came from a scraped HTML table rather than
/// a file.
///
/// Mast distances are not recoverable from a plain angle/index table -- see
/// `indicatrix_formats::asc`'s module doc comment: only a real `.asc` file's `a` records carry
/// them -- so every tier's `mast` is left at `0.0`, and index-wheel/symmetry metadata
/// beyond the gear-tooth count (`index_gear`) is defaulted too. The returned schedule
/// is always marked via [`asc::mark_reconstructed`] so it can never be mistaken for a
/// verified, hand-authored one.
///
/// Returns `None` if `angle_settings` is empty (nothing to export).
#[must_use]
pub fn reconstruct_asc_schedule(
    title: &str,
    refractive_index: Option<&str>,
    index_gear: Option<&str>,
    angle_settings: &[AngleSetting],
) -> Option<AscSchedule> {
    if angle_settings.is_empty() {
        return None;
    }

    let tiers = angle_settings
        .iter()
        .map(|a| AscTier {
            angle_deg: parse_angle_deg(&a.angle).unwrap_or(0.0),
            mast: 0.0,
            name: a.facet.clone(),
            indices: a
                .index
                .split([',', ' ', ';'])
                .filter_map(|s| s.trim().parse::<f64>().ok())
                .collect(),
            notes: a.notes.clone(),
        })
        .collect();

    let mut schedule = AscSchedule {
        gemcad_version: "5.0".to_string(),
        gear_teeth: index_gear.and_then(|g| g.parse().ok()).unwrap_or(96),
        gear_reference_angle: 0.0,
        symmetry_order: 1,
        mirror: false,
        refractive_index: refractive_index.and_then(|r| r.parse().ok()).unwrap_or(0.0),
        headers: vec![title.to_string()],
        footnotes: Vec::new(),
        tiers,
    };
    asc::mark_reconstructed(
        &mut schedule,
        "exported from a stored angle/index table, not an original .asc file -- mast \
         distances and index-wheel/symmetry metadata beyond the gear-tooth count were \
         not part of that table and are placeholders",
    );
    Some(schedule)
}

fn parse_angle_deg(angle: &str) -> Option<f64> {
    angle.trim().trim_end_matches('\u{b0}').trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

    #[test]
    fn import_asc_parses_headers_into_title_and_tiers_into_angle_settings() {
        let imported = import_asc("trichecker.asc", SAMPLE_ASC, None).expect("valid .asc");
        assert_eq!(imported.entry.title, "Round Trichecker-12");
        assert_eq!(imported.entry.url, "local://trichecker.asc");
        assert_eq!(imported.detail.angle_settings_table.len(), 2);
        assert_eq!(imported.detail.angle_settings_table[0].facet, "1");
        assert_eq!(imported.detail.angle_settings_table[0].angle, "-41\u{b0}");
        assert_eq!(imported.detail.refractive_index.as_deref(), Some("1.72"));
        assert_eq!(imported.detail.index_gear.as_deref(), Some("96"));
        assert_eq!(imported.detail.attached_files.len(), 1);
        assert_eq!(imported.detail.attached_files[0].name, "trichecker.asc");
    }

    #[test]
    fn import_asc_rejects_invalid_content() {
        assert!(import_asc("bad.asc", "not an asc file", None).is_err());
    }

    #[test]
    fn import_asc_has_no_provenance_without_a_source_entry_footnote() {
        let imported = import_asc("trichecker.asc", SAMPLE_ASC, None).expect("valid .asc");
        assert_eq!(imported.derived_from_entry_id, None);
    }

    #[test]
    fn import_asc_recovers_provenance_from_a_source_entry_footnote() {
        let text = format!("{SAMPLE_ASC}F {}\n", format_source_entry_footnote(42));
        let imported = import_asc("trichecker.asc", &text, None).expect("valid .asc");
        assert_eq!(imported.derived_from_entry_id, Some(42));
    }

    #[test]
    fn parse_source_entry_footnote_ignores_unrelated_footnotes_and_garbage() {
        assert_eq!(
            parse_source_entry_footnote(&["Cut to TCP".to_string()]),
            None
        );
        assert_eq!(
            parse_source_entry_footnote(&[format!("{SOURCE_ENTRY_FOOTNOTE_PREFIX}not-a-number")]),
            None
        );
        assert_eq!(
            parse_source_entry_footnote(&[format_source_entry_footnote(7)]),
            Some(7)
        );
    }

    #[test]
    fn import_asc_attaches_a_native_sidecar_as_a_second_attached_file() {
        let sidecar_bytes = b"format_version = 1\n";
        let imported = import_asc(
            "trichecker.asc",
            SAMPLE_ASC,
            Some(("trichecker.indicatrix.toml", sidecar_bytes)),
        )
        .expect("valid .asc");
        assert_eq!(imported.detail.attached_files.len(), 2);
        assert_eq!(imported.detail.attached_files[0].name, "trichecker.asc");
        assert_eq!(
            imported.detail.attached_files[1].name,
            "trichecker.indicatrix.toml"
        );
        assert_eq!(imported.detail.attached_files[1].content, sidecar_bytes);
    }

    #[test]
    fn reconstruct_asc_schedule_round_trips_through_to_asc_string() {
        let settings = vec![AngleSetting {
            order_index: 0,
            facet: "T".to_string(),
            angle: "0\u{b0}".to_string(),
            index: "0, 24, 48, 72".to_string(),
            notes: String::new(),
        }];
        let schedule = reconstruct_asc_schedule("Test Design", Some("1.76"), Some("96"), &settings)
            .expect("non-empty angle settings must produce a schedule");
        assert!(schedule.headers[0].starts_with("RECONSTRUCTED"));
        assert_eq!(schedule.tiers.len(), 1);
        assert_eq!(schedule.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);

        let text = asc::to_asc_string(&schedule);
        let reparsed = asc::parse_asc(&text).expect("reconstructed schedule must re-parse");
        assert_eq!(reparsed.tiers.len(), 1);
    }

    #[test]
    fn reconstruct_asc_schedule_returns_none_for_no_angle_settings() {
        assert!(reconstruct_asc_schedule("Empty", None, None, &[]).is_none());
    }
}

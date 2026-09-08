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
/// # Errors
///
/// Returns a human-readable message if `indicatrix_formats::asc::parse_asc` fails to parse
/// `content` (e.g. empty input, or a malformed required header field).
pub fn import_asc(file_name: &str, content: &str) -> Result<ImportedAsc, String> {
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

    let detail = FacetDiagramDetail {
        angle_settings_table: angle_settings_from_tiers(&schedule.tiers),
        attached_files: vec![AttachedFile {
            name: file_name.to_string(),
            url: String::new(),
            content: content.as_bytes().to_vec(),
        }],
        refractive_index: Some(schedule.refractive_index.to_string()),
        index_gear: Some(schedule.gear_teeth_abs().to_string()),
        facets_count: Some(schedule.facet_plane_count().to_string()),
        symmetry_order: Some(schedule.symmetry_order.to_string()),
        mirror_symmetry: Some(schedule.mirror),
        ..FacetDiagramDetail::default()
    };

    Ok(ImportedAsc { entry, detail })
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
        let imported = import_asc("trichecker.asc", SAMPLE_ASC).expect("valid .asc");
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
        assert!(import_asc("bad.asc", "not an asc file").is_err());
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

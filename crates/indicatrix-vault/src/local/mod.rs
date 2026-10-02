//! Local import/export for the user's own `.asc` files -- independent of any online
//! source.
//!
//! Everything here only ever reads a file the user handed it directly. Nothing in
//! this module touches the network -- see this crate's doc comment on why that
//! boundary is deliberate.

use crate::model::{
    angle::AngleSetting, detail::FacetingDiagramDetail, entry::FacetingDiagramEntry,
    file::AttachedFile,
};
use indicatrix_formats::asc::{self, AscSchedule, AscTier};

mod design_file;
mod native_design;

pub use design_file::{
    DesignFileKind, DesignFileText, converted_asc_file_name, design_attachment_position,
    design_file_to_asc_text,
};
pub use native_design::{
    apply_design_file_meta, apply_imported_extras, import_native_design, is_native_design_name,
    native_design_attachment_position,
};

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
    /// Library entry metadata.
    pub entry: FacetingDiagramEntry,
    /// Full design detail.
    pub detail: FacetingDiagramDetail,
    /// The catalogue row this file's own text says it was derived from -- recovered
    /// from a [`SOURCE_ENTRY_FOOTNOTE_PREFIX`] footnote line, when the `.asc` was
    /// written by `gui::editor::native_io`'s Save/Export .asc: an
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
    /// Row state that lives outside the entry and detail rows (tags, the ignored mark,
    /// the Rough Planner exclusion). Empty for everything but a `.indicatrix` design
    /// file whose `[meta]` table names some; written with [`apply_imported_extras`]
    /// once the row exists.
    pub extras: ImportedExtras,
}

/// The row state a design file's `[meta]` carries that is stored beside the entry and
/// detail rows rather than in them -- see [`ImportedAsc::extras`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedExtras {
    /// The design's tags, trimmed, de-duplicated and sorted.
    pub tags: Vec<String>,
    /// The owner marked the design ignored.
    pub ignored: bool,
    /// The design is excluded from the Rough Planner's candidate set.
    pub planner_excluded: bool,
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

/// Parses one `.asc` file's `content` (text the caller already holds -- this function
/// does no file I/O of its own) into an [`ImportedAsc`] ready to save into the local
/// library.
///
/// The stored `.asc` attachment is the UTF-8 encoding of `content`. A caller that read
/// the file from disk should call [`import_asc_bytes`] instead, which attaches the
/// file's original bytes: a text-only caller (for example, the editor saving a design
/// it just wrote itself) has no other bytes to keep.
///
/// `file_name` is used only for the title fallback and the synthetic URL/attachment
/// name.
///
/// The synthetic `url` (`local://<file_name>`) is what `diagram_entries.url`'s
/// `UNIQUE` constraint dedupes against, so re-importing a file with the same name
/// updates that design in place -- mirroring how a remote source's real page URL
/// dedupes a re-sync there.
///
/// `native_sidecar`, when the caller found a `<stem>.indicatrix` design file (or an
/// older `<stem>.indicatrix.toml`/`.gemcut.toml` sidecar) sitting beside the `.asc` on
/// disk, is attached as a SECOND [`AttachedFile`] alongside the `.asc` itself: without
/// it, a design that goes through Import loses every field only the design file
/// carries (authored meet constraints, preform, detached facets, material/RI
/// override), since only the `.asc` would otherwise be stored.
/// `gui::editor::loading::design_from_full_record` already prefers the attached design
/// file (or `indicatrix_cut_core::load_paired` for an older sidecar) whenever both
/// attachments are present, so attaching it here is the only piece this crate needs
/// to add.
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
    build_import(file_name, content, content.as_bytes(), native_sidecar)
}

/// [`import_asc`] for a file's raw `bytes`: parses the text [`asc::decode_asc_bytes`]
/// reads out of them, but attaches the ORIGINAL `bytes` unchanged.
///
/// The decoding strips a UTF-8 byte-order mark, accepts Windows-1252, and reads a lone
/// carriage return as a line break.
///
/// Keeping the file as it came off disk is what makes a later export byte-for-byte: a
/// Windows-1252 file (about one real `.asc` in ten, a legacy `0xB0` degree sign in a
/// header or footnote) would otherwise come back as a different UTF-8 file that
/// `GemCad` renders with mojibake. Every reader of the attachment decodes it with
/// [`asc::decode_asc_bytes`], which accepts both the original bytes and the UTF-8 text an
/// earlier version of this crate stored.
///
/// # Errors
///
/// Exactly [`import_asc`]'s: the decoded text must parse as `.asc`.
pub fn import_asc_bytes(
    file_name: &str,
    bytes: &[u8],
    native_sidecar: Option<(&str, &[u8])>,
) -> Result<ImportedAsc, String> {
    let content = asc::decode_asc_bytes(bytes);
    build_import(file_name, &content, bytes, native_sidecar)
}

/// The shared body of [`import_asc`]/[`import_asc_bytes`]: parses `content` and stores
/// `attachment` (the bytes the `.asc` attachment row keeps) beside the parsed metadata.
fn build_import(
    file_name: &str,
    content: &str,
    attachment: &[u8],
    native_sidecar: Option<(&str, &[u8])>,
) -> Result<ImportedAsc, String> {
    let schedule = asc::parse_asc(content).map_err(|e| e.to_string())?;

    let mut attached_files = vec![AttachedFile {
        name: file_name.to_string(),
        url: String::new(),
        content: attachment.to_vec(),
    }];
    if let Some((sidecar_name, sidecar_content)) = native_sidecar {
        attached_files.push(AttachedFile {
            name: sidecar_name.to_string(),
            url: String::new(),
            content: sidecar_content.to_vec(),
        });
    }
    Ok(catalogue_rows(
        file_name,
        strip_asc_extension(file_name),
        &schedule,
        attached_files,
    ))
}

/// The entry and detail rows for a design whose cutting data is `schedule` and whose
/// stored files are `attached_files`: the title is the schedule's first non-blank
/// header, else `fallback_title`; the url is `local://<file_name>`.
fn catalogue_rows(
    file_name: &str,
    fallback_title: &str,
    schedule: &AscSchedule,
    attached_files: Vec<AttachedFile>,
) -> ImportedAsc {
    let title = schedule
        .headers
        .first()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| fallback_title.to_string());

    let entry = FacetingDiagramEntry {
        title,
        url: format!("local://{file_name}"),
        design_id: String::new(),
    };
    let detail = FacetingDiagramDetail {
        angle_settings_table: angle_settings_from_tiers(&schedule.tiers),
        attached_files,
        refractive_index: Some(schedule.refractive_index.to_string()),
        index_gear: Some(schedule.gear_teeth_abs().to_string()),
        facets_count: Some(schedule.facet_plane_count().to_string()),
        symmetry_order: Some(schedule.symmetry_order.to_string()),
        mirror_symmetry: Some(schedule.mirror),
        ..FacetingDiagramDetail::default()
    };
    ImportedAsc {
        entry,
        detail,
        derived_from_entry_id: parse_source_entry_footnote(&schedule.footnotes),
        extras: ImportedExtras::default(),
    }
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
/// Returns `Ok(None)` if `angle_settings` is empty (nothing to export) -- unchanged
/// from before this function returned a plain `Option`.
///
/// # A tier's angle, and the refractive index, must parse or this errors out
///
/// This used to fall back to a silent `0.0` for an unparsable tier angle or refractive
/// index (`.unwrap_or(0.0)`), and only split a tier's `index` text on `,`/` `/`;` --
/// missing the real catalogue's actual separator for scraped index-wheel data, a
/// hyphen (`"96-08-16-24-32-40-48-56-64-72-80-88"`). Measured on the real catalogue:
/// 48,559 of 50,817 `angle_settings.index_val` rows had no parsable index under the
/// old separator set, and every tier of the 140 designs that reach this
/// reconstruction path (no `.asc` attachment, so this is their ONLY export path) came
/// out with a fabricated `0.0` angle -- a schedule that looks real but silently
/// isn't. Now: `-` is a recognized index separator alongside `,`/` `/`;`, and an
/// unparsable tier angle or refractive index is a hard [`Err`] naming the offending
/// tier/facet (or the raw refractive-index text), never a fabricated `0.0`.
///
/// The gear-tooth count fallback (`unwrap_or(96)`, `GemCad`'s near-universal default
/// index wheel) is intentionally NOT promoted to an error: unlike a tier's angle or the
/// design's refractive index, a wrong gear-tooth count only affects the (already
/// explicitly placeholder, per [`asc::mark_reconstructed`]'s message below)
/// index-wheel metadata this reconstruction can never fully recover anyway -- never
/// the tier geometry itself, which is this function's actual job to get right.
///
/// A missing (`None`) refractive index -- as opposed to text present but unparsable --
/// still falls back to `0.0`: an absent RI is a genuinely unknown value on some
/// designs, the same "a blank/absent field is not an error, only unparsable text is"
/// contract `crate::model::metadata_update::parse_optional_numeric` uses elsewhere in
/// this crate.
///
/// # Errors
///
/// Returns `Err` naming the offending tier (facet name and order index) if that tier's
/// `angle` text fails to parse, or naming the offending text if `refractive_index` is
/// present but fails to parse. Never fails for a missing/unparsable `index_gear`, or
/// for an individual index within a tier's index-wheel text that fails to parse (each
/// such index is simply dropped, same as before).
pub fn reconstruct_asc_schedule(
    title: &str,
    refractive_index: Option<&str>,
    index_gear: Option<&str>,
    angle_settings: &[AngleSetting],
) -> Result<Option<AscSchedule>, String> {
    if angle_settings.is_empty() {
        return Ok(None);
    }

    let mut tiers = Vec::with_capacity(angle_settings.len());
    for a in angle_settings {
        let angle_deg = parse_angle_deg(&a.angle).ok_or_else(|| {
            format!(
                "tier '{}' (order index {}): angle '{}' is not a valid number",
                a.facet, a.order_index, a.angle
            )
        })?;
        // A zero angle is the table unless its sign says culet (see
        // `AscTier::angle_deg`); the scraped table marks a culet by its index or
        // facet text instead, so carry that onto the sign.
        let is_culet_row = a.index.trim().eq_ignore_ascii_case("culet")
            || a.facet.to_ascii_lowercase().contains("culet");
        let angle_deg = if angle_deg == 0.0 && is_culet_row {
            -0.0
        } else {
            angle_deg
        };
        tiers.push(AscTier {
            angle_deg,
            mast: 0.0,
            name: a.facet.clone(),
            indices: a
                .index
                .split([',', ' ', ';', '-'])
                .filter_map(|s| s.trim().parse::<f64>().ok())
                .collect(),
            index_names: Vec::new(),
            notes: a.notes.clone(),
        });
    }

    let refractive_index = match refractive_index {
        None => 0.0,
        Some(r) => r
            .parse::<f64>()
            .map_err(|_| format!("refractive index '{r}' is not a valid number"))?,
    };

    let mut schedule = AscSchedule {
        gemcad_version: "5.0".to_string(),
        // A missing/unparsable gear-tooth count is deliberately NOT an error here --
        // see this function's own doc comment.
        gear_teeth: index_gear.and_then(|g| g.parse().ok()).unwrap_or(96),
        gear_reference_angle: 0.0,
        symmetry_order: 1,
        mirror: false,
        refractive_index,
        headers: vec![title.to_string()],
        footnotes: Vec::new(),
        tiers,
        warnings: Vec::new(),
        line_ending: asc::AscLineEnding::default(),
    };
    asc::mark_reconstructed(
        &mut schedule,
        "exported from a stored angle/index table, not an original .asc file -- mast \
         distances and index-wheel/symmetry metadata beyond the gear-tooth count were \
         not part of that table and are placeholders",
    );
    Ok(Some(schedule))
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

    /// A `.asc` as `GemCad` for Windows writes it: a Windows-1252 `e`-acute and degree
    /// sign (bytes `0xE9`, `0xB0`) in the header, which are not valid UTF-8.
    fn windows_1252_asc_bytes() -> Vec<u8> {
        b"GemCad 5.0\ng 96 0.0\ny 6 y\nI 1.72\nH Caf\xE9 \xB0 Round\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n"
            .to_vec()
    }

    #[test]
    fn import_asc_bytes_attaches_the_original_bytes_and_parses_the_decoded_text() {
        let raw = windows_1252_asc_bytes();
        assert!(
            std::str::from_utf8(&raw).is_err(),
            "the fixture must not be valid UTF-8"
        );
        let imported = import_asc_bytes("cafe.asc", &raw, None).expect("valid .asc");
        assert_eq!(imported.entry.title, "Caf\u{e9} \u{b0} Round");
        assert_eq!(imported.detail.attached_files.len(), 1);
        assert_eq!(
            imported.detail.attached_files[0].content, raw,
            "the attachment must be the file's bytes, not a UTF-8 re-encoding"
        );
        assert_eq!(
            asc::decode_asc_bytes(&imported.detail.attached_files[0].content),
            "GemCad 5.0\ng 96 0.0\ny 6 y\nI 1.72\nH Caf\u{e9} \u{b0} Round\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n"
        );
    }

    #[test]
    fn import_asc_bytes_keeps_a_byte_order_mark_and_classic_mac_line_endings_in_the_attachment() {
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(SAMPLE_ASC.replace('\n', "\r").as_bytes());
        let imported = import_asc_bytes("mac.asc", &raw, None).expect("valid .asc");
        assert_eq!(imported.entry.title, "Round Trichecker-12");
        assert_eq!(imported.detail.attached_files[0].content, raw);
    }

    #[test]
    fn import_asc_attaches_the_utf8_encoding_of_the_text() {
        let imported = import_asc("trichecker.asc", SAMPLE_ASC, None).expect("valid .asc");
        assert_eq!(
            imported.detail.attached_files[0].content,
            SAMPLE_ASC.as_bytes()
        );
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
            .expect("must not error")
            .expect("non-empty angle settings must produce a schedule");
        assert!(schedule.headers[0].starts_with("RECONSTRUCTED"));
        assert_eq!(schedule.tiers.len(), 1);
        assert_eq!(schedule.tiers[0].indices, vec![0.0, 24.0, 48.0, 72.0]);

        let text = asc::to_asc_string(&schedule).expect("facet name \"T\" has no whitespace");
        let reparsed = asc::parse_asc(&text).expect("reconstructed schedule must re-parse");
        assert_eq!(reparsed.tiers.len(), 1);
    }

    #[test]
    fn reconstruct_asc_schedule_returns_none_for_no_angle_settings() {
        assert!(
            reconstruct_asc_schedule("Empty", None, None, &[])
                .expect("must not error")
                .is_none()
        );
    }

    /// The real catalogue's scraped index-wheel text uses `-` as a separator
    /// (`"96-08-16-24-32-40-48-56-64-72-80-88"`), which the old `[',', ' ', ';']`
    /// separator set didn't recognize at all -- 48,559 of 50,817 real rows had no
    /// parsable index under it.
    #[test]
    fn reconstruct_asc_schedule_splits_hyphen_separated_indices() {
        let settings = vec![AngleSetting {
            order_index: 0,
            facet: "C1".to_string(),
            angle: "41\u{b0}".to_string(),
            index: "96-08-16-24-32-40-48-56-64-72-80-88".to_string(),
            notes: String::new(),
        }];
        let schedule = reconstruct_asc_schedule("Hyphen Test", Some("1.76"), Some("96"), &settings)
            .expect("must not error")
            .expect("non-empty angle settings must produce a schedule");
        assert_eq!(
            schedule.tiers[0].indices,
            vec![
                96.0, 8.0, 16.0, 24.0, 32.0, 40.0, 48.0, 56.0, 64.0, 72.0, 80.0, 88.0
            ]
        );
    }

    /// An unparsable tier angle used to silently become `0.0`; it must now be a
    /// hard error naming the offending tier instead.
    #[test]
    fn reconstruct_asc_schedule_errors_on_an_unparsable_angle_naming_the_tier() {
        let settings = vec![AngleSetting {
            order_index: 3,
            facet: "P2".to_string(),
            angle: "not-a-number".to_string(),
            index: "0".to_string(),
            notes: String::new(),
        }];
        let err = reconstruct_asc_schedule("Bad Angle", Some("1.76"), Some("96"), &settings)
            .expect_err("an unparsable angle must be a hard error, never a silent 0.0");
        assert!(
            err.contains("P2"),
            "error must name the offending tier: {err}"
        );
    }

    /// An unparsable refractive index used to silently become `0.0`; it must now
    /// be a hard error naming the offending text.
    #[test]
    fn reconstruct_asc_schedule_errors_on_an_unparsable_refractive_index() {
        let settings = vec![AngleSetting {
            order_index: 0,
            facet: "T".to_string(),
            angle: "0\u{b0}".to_string(),
            index: "0".to_string(),
            notes: String::new(),
        }];
        let err = reconstruct_asc_schedule("Bad RI", Some("garbage"), Some("96"), &settings)
            .expect_err("an unparsable refractive index must be a hard error, never a silent 0.0");
        assert!(
            err.contains("garbage"),
            "error should mention the offending text: {err}"
        );
    }

    /// A missing (not merely unparsable) refractive index is a genuinely unknown
    /// value, not an error -- same "blank/absent is not an error" contract as
    /// `parse_optional_numeric`.
    #[test]
    fn reconstruct_asc_schedule_defaults_a_missing_refractive_index_to_zero() {
        let settings = vec![AngleSetting {
            order_index: 0,
            facet: "T".to_string(),
            angle: "0\u{b0}".to_string(),
            index: "0".to_string(),
            notes: String::new(),
        }];
        let schedule = reconstruct_asc_schedule("No RI", None, Some("96"), &settings)
            .expect("a missing refractive index must not error")
            .expect("non-empty angle settings must produce a schedule");
        assert!((schedule.refractive_index - 0.0).abs() < f64::EPSILON);
    }
}

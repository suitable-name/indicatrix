//! Resolves a [`Design`] from a catalogue entry's full record, or from a bare
//! `.asc` file's own text -- preferring a real attached schedule (and, when present,
//! its paired native sidecar) over the angle-table placeholder reconstruction.

use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_cut_core::{
    Design, PreformSpec, load_paired,
    native::{LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX},
};
use indicatrix_vault::model::file::AttachedFile;
use tracing::warn;

/// A preform sized to bound a design that may already fully specify its own closed
/// shape (a real `.asc` file's tiers, or the placeholder reconstruction in
/// [`design_from_full_record`]): generously large relative to the "mast order 1" scale
/// every facet offset in this codebase uses (see `indicatrix_cut_core::preform`'s module doc
/// comment), so this backstop rough does not itself clip a single facet of an
/// already-complete design -- only a schedule that's missing a closing facet in some
/// direction would ever actually touch this preform's own walls.
///
/// `length_over_width` is read from the catalogue's own recorded `lw_ratio` when it
/// parses as a positive finite number, so an oval design's preform isn't needlessly
/// round; `1.0` (round/square) otherwise, matching `indicatrix_cut_core::PreformSpec`'s own default
/// shape assumption.
fn default_preform_for_schedule(
    schedule: &indicatrix_formats::asc::AscSchedule,
    lw_ratio: Option<&str>,
) -> PreformSpec {
    let length_over_width = lw_ratio
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    PreformSpec::cylinder_for_schedule(schedule, 3.0, length_over_width, 3.0)
}

/// [`design_from_full_record`]'s full result -- see that function's own doc comment.
pub(in crate::gui::editor) struct LoadedDesign {
    pub(in crate::gui::editor) design: Design,
    /// `true` iff no real attached `.asc` was found and `design`'s schedule instead
    /// came from the angle-table placeholder reconstruction (every mast `0.0`) --
    /// see [`design_from_full_record`]'s doc comment.
    pub(in crate::gui::editor) used_placeholder: bool,
    /// The real attached `.asc`'s own bare file name, `None` on the placeholder
    /// path. Reconstructed text was never a real `.asc` this catalogue entry
    /// ships, so there is nothing there worth [`indicatrix_cut_core::save_paired`]
    /// preserving verbatim. Fed to `super::state::EditorState::asc_filename` by
    /// `super::callbacks::setup_load_selected_callback`.
    pub(in crate::gui::editor) asc_filename: Option<String>,
    /// The real attached `.asc`'s own exact original text, `None` on the
    /// placeholder path for the identical reason. Fed to
    /// `super::state::EditorState::original_asc_text`.
    pub(in crate::gui::editor) original_asc_text: Option<String>,
}

/// Builds a [`LoadedDesign`] from a real `.asc` file's own text, always along the
/// "real schedule" outcome (`used_placeholder: false` -- there is no angle-table
/// fallback here, since that reconstruction needs a full catalogue record's columns
/// this function never sees).
///
/// Factored out of [`design_from_full_record`]'s own real-attachment branch so
/// `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s remote
/// branch can build the identical [`Design`] from
/// `gui::library::remote::RemoteDesignSource::asc_text` -- the exact bytes/text a
/// locally-attached `.asc` file would carry (see that struct's own doc comment) --
/// without needing a `FullDiagramRecord` at all (a remote fetch never has one; only
/// the bare file name and text cross the wire).
///
/// # Errors
///
/// Returns `Err` (the parse failure's own message) when `text` does not parse as a
/// `.asc` cutting schedule at all.
pub(in crate::gui::editor) fn design_from_asc_text(
    file_name: &str,
    text: &str,
    lw_ratio: Option<&str>,
) -> Result<LoadedDesign, String> {
    let schedule = indicatrix_formats::asc::parse_asc(text).map_err(|e| e.to_string())?;
    let preform = default_preform_for_schedule(&schedule, lw_ratio);
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: false,
        asc_filename: Some(file_name.to_string()),
        original_asc_text: Some(text.to_string()),
    })
}

/// A sibling native sidecar (current `.indicatrix.toml`, or the legacy
/// `.gemcut.toml`) among `files`, if the catalogue entry has one attached alongside
/// its `.asc` -- see [`design_from_asc_and_native`]'s own doc comment for why that
/// combination is worth preferring over the bare `.asc` alone.
///
/// Nothing in this app's importer attaches a second file today (`gui::library::
/// local::import` collects only `.asc`), so this currently only ever matches when a
/// FUTURE import (or a remote source) starts doing so -- see this crate's own
/// handoff notes for that other half.
fn native_sidecar_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    files.iter().find(|f| {
        let lower = f.name.to_lowercase();
        lower.ends_with(NATIVE_EXTENSION_SUFFIX) || lower.ends_with(LEGACY_NATIVE_EXTENSION_SUFFIX)
    })
}

/// [`design_from_full_record`]'s preferred path when a catalogue entry carries BOTH
/// a real `.asc` attachment and a native sidecar paired with it: [`load_paired`]
/// restores the sidecar's own preform/material/girdle-diameter unconditionally, and
/// (when the fingerprint still matches) every tier's authored meet constraint and
/// detached-facet set too -- all of which a bare `.asc` re-import otherwise
/// reconstructs as fresh `ScaleReference` anchors with no detached facets at all
/// (see `indicatrix_cut_core::native`'s own module doc comment for the full list of
/// what only the sidecar carries).
///
/// Falls back to [`design_from_asc_text`] (logging a warning) when the sidecar text
/// itself fails to parse -- a real `.asc` schedule is still strictly better than
/// refusing to load the design at all.
fn design_from_asc_and_native(
    asc_name: &str,
    asc_text: &str,
    native_text: &str,
    lw_ratio: Option<&str>,
    entry_id: i64,
) -> Result<LoadedDesign, String> {
    match load_paired(asc_text, native_text, false) {
        Ok(result) => Ok(LoadedDesign {
            design: result.design,
            used_placeholder: false,
            asc_filename: Some(asc_name.to_string()),
            original_asc_text: Some(asc_text.to_string()),
        }),
        Err(e) => {
            warn!(
                "Native sidecar paired with '{asc_name}' on diagram #{entry_id} failed to \
                 load ({e}); falling back to the plain .asc schedule."
            );
            design_from_asc_text(asc_name, asc_text, lw_ratio)
        }
    }
}

/// Builds the design that should be loaded for one catalogue entry's full record.
///
/// Prefers a real attached `.asc` file's own schedule -- its mast values are the
/// file's actual recorded depths -- over `indicatrix_vault::local::reconstruct_asc_schedule`'s
/// placeholder reconstruction from the angle/index table alone, which (per that
/// function's own doc comment) has no depth data to work with at all and fills every
/// tier's `mast` with `0.0`. That fallback is still offered rather than refused
/// outright -- it is the SAME reconstruction `gui::library::local::export::setup_export_asc_callback`
/// already exports today with the same caveat -- but [`LoadedDesign::used_placeholder`]
/// tells the caller which path was taken, so it can warn exactly the way that existing
/// export path already does, rather than silently handing the user a schedule whose
/// masts are all zero.
///
/// When a native sidecar is attached alongside the `.asc` (see
/// [`native_sidecar_attachment`]), [`design_from_asc_and_native`] is preferred over
/// the plain `.asc` path so an imported design does not lose the sidecar-only fields
/// a re-import would otherwise silently discard.
///
/// # Errors
///
/// Returns `Err` (a human-readable message) only when there is no attached `.asc` AND
/// no angle-settings table to reconstruct from at all -- nothing in this diagram's
/// record describes a cutting schedule.
pub(in crate::gui::editor) fn design_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Result<LoadedDesign, String> {
    if let Some(attached) = full
        .attached_files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with(".asc"))
    {
        let text = String::from_utf8_lossy(&attached.content);
        let loaded = native_sidecar_attachment(&full.attached_files).map_or_else(
            || design_from_asc_text(&attached.name, &text, full.lw_ratio.as_deref()),
            |sidecar| {
                let native_text = String::from_utf8_lossy(&sidecar.content);
                design_from_asc_and_native(
                    &attached.name,
                    &text,
                    &native_text,
                    full.lw_ratio.as_deref(),
                    full.entry_id,
                )
            },
        );
        match loaded {
            Ok(loaded) => return Ok(loaded),
            Err(e) => warn!(
                "Attached .asc '{}' on diagram #{} failed to parse ({e}); falling back to the \
                 angle-table reconstruction.",
                attached.name, full.entry_id
            ),
        }
    }

    let schedule = indicatrix_vault::local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &full.angle_settings,
    )
    .ok_or_else(|| "This diagram has no cutting-schedule data to load.".to_string())?;
    let preform = default_preform_for_schedule(&schedule, full.lw_ratio.as_deref());
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: true,
        asc_filename: None,
        original_asc_text: None,
    })
}

/// Builds Deep Solve's external verification targets from a catalogue entry's own
/// printed proportion columns -- see `deep_solve`'s module doc comment ("External
/// verification: printed proportions" in `solve_meet_points_verified`'s own doc
/// comment is the underlying reasoning). `volume`/`lw_ratio`/`cw_ratio`/`pw_ratio`/
/// `hw_ratio` map straight onto `ExternalProportions`' `vol_w3`/`lw`/`cw`/`pw`/`hw`
/// -- the exact same column-to-field mapping
/// `crates/indicatrix/examples/meet_solver_validation.rs` uses when it builds the same
/// struct from a `diagram_details` row.
///
/// Returns `None` when none of the five columns hold a usable positive, finite
/// number: a design with nothing printed on it at all gives the search no external
/// signal to score against at all, so Deep Solve must be disabled rather than run
/// against a target that can never accept or reject anything (see
/// `super::view::deep_solve_hint`).
pub(in crate::gui::editor) fn external_proportions_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<ExternalProportions> {
    fn parse(value: Option<&String>) -> Option<f64> {
        value
            .map(String::as_str)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v > 0.0)
    }
    let props = ExternalProportions {
        vol_w3: parse(full.volume.as_ref()),
        lw: parse(full.lw_ratio.as_ref()),
        cw: parse(full.cw_ratio.as_ref()),
        pw: parse(full.pw_ratio.as_ref()),
        hw: parse(full.hw_ratio.as_ref()),
    };
    (props.vol_w3.is_some()
        || props.lw.is_some()
        || props.cw.is_some()
        || props.pw.is_some()
        || props.hw.is_some())
    .then_some(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_vault::model::entry::FullDiagramRecord;

    fn empty_full_record() -> FullDiagramRecord {
        FullDiagramRecord {
            entry_id: 1,
            title: "Test Design".to_string(),
            url: "local://test.asc".to_string(),
            design_id: None,
            page_url: String::new(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: None,
            refractive_index: None,
            index_gear: None,
            volume: None,
            facets_count: None,
            shape: None,
            designer_info: None,
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            source_citation: None,
            pdf_file: None,
            gem_file: None,
            shape_category: None,
            angle_settings: Vec::new(),
            attached_files: Vec::new(),
        }
    }

    #[test]
    fn design_from_asc_text_parses_a_real_schedule() {
        let text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded = design_from_asc_text("design.asc", text, None).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.original_asc_text.as_deref(), Some(text));
        assert_eq!(loaded.design.tiers.len(), 1);
    }

    #[test]
    fn design_from_asc_text_rejects_unparseable_text() {
        assert!(design_from_asc_text("bad.asc", "not a real .asc schedule", None).is_err());
    }

    #[test]
    fn design_from_full_record_prefers_a_real_attached_asc_file() {
        let mut full = empty_full_record();
        full.attached_files
            .push(indicatrix_vault::model::file::AttachedFile {
                name: "design.asc".to_string(),
                url: String::new(),
                content:
                    b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                        .to_vec(),
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert!(loaded.original_asc_text.is_some());
        assert_eq!(loaded.design.tiers.len(), 1);
        // A lone tier with no explicit anchor gets its own real recorded mast
        // borrowed as a `ScaleReference` anchor by `Design::from_asc_schedule`
        // (see that function's doc comment) -- not a placeholder zero.
        match loaded.design.tiers[0].constraint {
            MeetConstraint::ScaleReference(v) => assert!((v - 0.649_912_34).abs() < 1e-9),
            ref other => panic!("expected a borrowed ScaleReference anchor, got {other:?}"),
        }
    }

    // --- native_sidecar_attachment / design_from_asc_and_native ---

    #[test]
    fn native_sidecar_attachment_finds_the_indicatrix_toml_sibling() {
        let files = vec![
            AttachedFile {
                name: "design.asc".to_string(),
                url: String::new(),
                content: Vec::new(),
            },
            AttachedFile {
                name: "design.indicatrix.toml".to_string(),
                url: String::new(),
                content: b"native".to_vec(),
            },
        ];
        let sidecar = native_sidecar_attachment(&files).expect("sidecar must be found");
        assert_eq!(sidecar.name, "design.indicatrix.toml");
    }

    #[test]
    fn native_sidecar_attachment_also_finds_the_legacy_gemcut_toml_sibling() {
        let files = vec![AttachedFile {
            name: "design.gemcut.toml".to_string(),
            url: String::new(),
            content: b"native".to_vec(),
        }];
        assert!(native_sidecar_attachment(&files).is_some());
    }

    #[test]
    fn native_sidecar_attachment_is_none_without_a_sidecar() {
        let files = vec![AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: Vec::new(),
        }];
        assert!(native_sidecar_attachment(&files).is_none());
    }

    #[test]
    fn design_from_asc_and_native_falls_back_to_the_plain_asc_when_the_sidecar_does_not_parse() {
        let asc_text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded =
            design_from_asc_and_native("design.asc", asc_text, "not valid toml [[[", None, 1)
                .expect("must fall back to the plain .asc path rather than erroring");
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.design.tiers.len(), 1);
    }

    #[test]
    fn design_from_full_record_falls_back_to_the_angle_table_when_no_asc_is_attached() {
        let mut full = empty_full_record();
        full.angle_settings
            .push(indicatrix_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(loaded.used_placeholder);
        assert_eq!(loaded.asc_filename, None);
        assert_eq!(loaded.original_asc_text, None);
        assert_eq!(loaded.design.tiers.len(), 1);
        // The reconstruction's own documented placeholder mast (0.0) is what gets
        // borrowed as this lone tier's `ScaleReference` anchor.
        assert_eq!(
            loaded.design.tiers[0].constraint,
            MeetConstraint::ScaleReference(0.0)
        );
    }

    #[test]
    fn design_from_full_record_errors_with_no_schedule_data_at_all() {
        let full = empty_full_record();
        assert!(design_from_full_record(&full).is_err());
    }
}

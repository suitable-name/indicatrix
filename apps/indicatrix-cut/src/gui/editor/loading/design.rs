//! Resolves a [`Design`] from a catalogue entry's full record -- preferring an attached
//! self-contained `.indicatrix` design file, then a real attached schedule (and, when
//! present, its paired older sidecar), over the angle-table placeholder
//! reconstruction. The bare-`.asc` half
//! (`design_from_asc_text`, the default preform, [`LoadedDesign`]) lives in
//! `indicatrix_editor::loading`, shared with the web app.

use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_cut_core::{
    Design, load_paired,
    native::{
        DesignMetadata, LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX, design_from_str,
    },
};
use indicatrix_editor::loading::{
    LoadedDesign, default_preform_for_schedule, design_from_asc_text,
};
use indicatrix_vault::model::file::AttachedFile;
use tracing::warn;

/// A sibling older sidecar (current `.indicatrix.toml`, or the legacy
/// `.gemcut.toml`) among `files`, if the catalogue entry has one attached alongside
/// its `.asc` -- see [`design_from_asc_and_native`]'s own doc comment for why that
/// combination is worth preferring over the bare `.asc` alone.
///
/// The importer attaches the sidecar it finds beside a `.asc` (`gui::library::local::
/// import`), and a remote source may attach one too. A self-contained `.indicatrix`
/// design file is a different attachment: see [`design_from_design_attachment`].
fn native_sidecar_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    files.iter().find(|f| {
        let lower = f.name.to_lowercase();
        lower.ends_with(NATIVE_EXTENSION_SUFFIX) || lower.ends_with(LEGACY_NATIVE_EXTENSION_SUFFIX)
    })
}

/// [`design_from_full_record`]'s preferred path when a catalogue entry carries BOTH
/// a real `.asc` attachment and an older sidecar paired with it: [`load_paired`]
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
            metadata: DesignMetadata::default(),
            attachments: Vec::new(),
        }),
        Err(e) => {
            warn!(
                "Older sidecar paired with '{asc_name}' on diagram #{entry_id} failed to \
                 load ({e}); falling back to the plain .asc schedule."
            );
            design_from_asc_text(asc_name, asc_text, lw_ratio)
        }
    }
}

/// The design built from `full`'s self-contained `.indicatrix` attachment, if it has
/// one that reads. The file carries the whole design, so it wins over any `.asc`,
/// `.gem` or `.gcs` attached beside it (an import attaches a `.indicatrix` file found
/// next to its schedule; a catalogue save attaches the saved design beside the `.asc`
/// it wrote).
///
/// The design is recorded under the attached `.asc`'s name when there is one -- with
/// that text kept, so a later Save can preserve it byte for byte -- else under
/// `<stem>.asc` of the converted `.gem`/`.gcs` or of the design file itself. `None`
/// (logged) when the attachment is not UTF-8 or does not parse, so the schedule beside
/// it still loads.
fn design_from_design_attachment(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<LoadedDesign> {
    let names = || full.attached_files.iter().map(|f| f.name.as_str());
    let attached =
        &full.attached_files[indicatrix_vault::local::native_design_attachment_position(names())?];
    let parsed = std::str::from_utf8(&attached.content)
        .map_err(|e| e.to_string())
        .and_then(|text| design_from_str(text).map_err(|e| e.to_string()));
    let loaded = match parsed {
        Ok(loaded) => loaded,
        Err(e) => {
            warn!(
                "Attached design file '{}' on diagram #{} could not be read ({e}); using the \
                 schedule beside it.",
                attached.name, full.entry_id
            );
            return None;
        }
    };
    let schedule = indicatrix_vault::local::design_attachment_position(names())
        .map(|(position, kind)| (&full.attached_files[position], kind));
    let (asc_filename, original_asc_text) = match schedule {
        Some((file, indicatrix_vault::local::DesignFileKind::Asc)) => (
            file.name.clone(),
            Some(indicatrix_formats::asc::decode_asc_bytes(&file.content).into_owned()),
        ),
        Some((file, _)) => (
            indicatrix_vault::local::converted_asc_file_name(&file.name),
            None,
        ),
        None => (
            indicatrix_vault::local::converted_asc_file_name(&attached.name),
            None,
        ),
    };
    Some(LoadedDesign {
        design: loaded.design,
        used_placeholder: false,
        asc_filename: Some(asc_filename),
        original_asc_text,
        // Kept so a Save from this design writes the file's own `[meta]` and
        // attachments back instead of dropping them.
        metadata: loaded.metadata,
        attachments: loaded.attachments,
    })
}

/// The design built from `full`'s design-file attachment, picked by
/// `indicatrix_vault::local::design_attachment_position` (the first `.asc`, else the
/// first `.gem`, else the first `.gcs`, extension case-insensitive) and read as
/// `.asc` text by `indicatrix_vault::local::design_file_to_asc_text` -- a `.gem`/
/// `.gcs` converted to cutting instructions, so the design loads with its real
/// masts instead of the angle-table placeholder. Reading runs inside
/// `gui::library::local::catch_file_panic`, the importer's own per-file guard.
///
/// A `.asc` paired with an older sidecar attachment goes through
/// [`design_from_asc_and_native`]; a converted `.gem`/`.gcs` never does (a
/// sidecar's fingerprint describes a `.asc`).
///
/// `None` when the record has no design-file attachment; `Some(Err)` (a ready
/// message) when it has one that does not read, convert or parse.
///
/// A self-contained `.indicatrix` attachment comes first ([`design_from_design_attachment`]).
///
/// Also the design-file half of `super::catalogue_planes::resolve_catalogue_planes`,
/// so the editor's Load Selected and every plane consumer read the same file.
pub(super) fn design_from_attachment(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<Result<LoadedDesign, String>> {
    if let Some(loaded) = design_from_design_attachment(full) {
        return Some(Ok(loaded));
    }
    let (position, kind) = indicatrix_vault::local::design_attachment_position(
        full.attached_files.iter().map(|f| f.name.as_str()),
    )?;
    let attached = &full.attached_files[position];
    let read = crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(|| {
        indicatrix_vault::local::design_file_to_asc_text(&attached.name, kind, &attached.content)
    }));
    let file = match read {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => return Some(Err(format!("'{}': {e}", attached.name))),
        Err(panic_msg) => {
            return Some(Err(format!(
                "'{}': internal error: {panic_msg}",
                attached.name
            )));
        }
    };
    if !file.warnings.is_empty() {
        warn!(
            "Attached '{}' on diagram #{} converted with notes: {:?}",
            attached.name, full.entry_id, file.warnings
        );
    }
    let lw_ratio = full.lw_ratio.as_deref();
    let sidecar = if kind == indicatrix_vault::local::DesignFileKind::Asc {
        native_sidecar_attachment(&full.attached_files)
    } else {
        None
    };
    let loaded = sidecar.map_or_else(
        || design_from_asc_text(&file.asc_file_name, &file.asc_text, lw_ratio),
        |sidecar| {
            let native_text = String::from_utf8_lossy(&sidecar.content);
            design_from_asc_and_native(
                &file.asc_file_name,
                &file.asc_text,
                &native_text,
                lw_ratio,
                full.entry_id,
            )
        },
    );
    Some(loaded.map_err(|e| format!("'{}': {e}", attached.name)))
}

/// Builds the design that should be loaded for one catalogue entry's full record.
///
/// Prefers a real attached design file's own schedule ([`design_from_attachment`]:
/// a `.asc`, else a `.gem`, else a `.gcs`) -- its mast values are the
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
/// When an older sidecar is attached alongside the `.asc` (see
/// [`native_sidecar_attachment`]), [`design_from_asc_and_native`] is preferred over
/// the plain `.asc` path so an imported design does not lose the sidecar-only fields
/// a re-import would otherwise silently discard.
///
/// # Errors
///
/// Returns `Err` (a human-readable message) only when there is no readable attached
/// `.asc`/`.gem`/`.gcs` AND no angle-settings table to reconstruct from at all --
/// nothing in this diagram's record describes cutting instructions.
pub(in crate::gui::editor) fn design_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Result<LoadedDesign, String> {
    match design_from_attachment(full) {
        Some(Ok(loaded)) => return Ok(loaded),
        Some(Err(e)) => warn!(
            "Attached design file on diagram #{} could not be loaded ({e}); falling back to \
             the angle-table reconstruction.",
            full.entry_id
        ),
        None => {}
    }

    let schedule = indicatrix_vault::local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &full.angle_settings,
    )
    .map_err(|e| format!("This diagram's cutting-instructions data could not be read: {e}"))?
    .ok_or_else(|| "This diagram has no cutting-instructions data to load.".to_string())?;
    let preform = default_preform_for_schedule(&schedule, full.lw_ratio.as_deref());
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: true,
        asc_filename: None,
        original_asc_text: None,
        metadata: DesignMetadata::default(),
        attachments: Vec::new(),
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

    /// A record whose attachments hold a `.asc` AND a self-contained `.indicatrix` file
    /// opens from the design file (every tier of it), under the `.asc`'s name.
    #[test]
    fn design_from_full_record_prefers_an_attached_indicatrix_file() {
        use indicatrix_cut_core::{
            ConstraintTier, PreformSpec, ScheduleMeta, native::DesignExtras,
        };
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let text =
            indicatrix_cut_core::native::design_to_string(&design, None, &DesignExtras::default())
                .expect("serializes");
        let asc = b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "round.asc".to_string(),
            url: String::new(),
            content: asc.to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: "round.indicatrix".to_string(),
            url: String::new(),
            content: text.into_bytes(),
        });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.design.tiers.len(), design.tiers.len());
        assert_eq!(loaded.asc_filename.as_deref(), Some("round.asc"));
        assert!(loaded.original_asc_text.is_some());

        // Alone, the design file names the design after itself and has no `.asc` text.
        full.attached_files.remove(0);
        let lone = design_from_full_record(&full).unwrap();
        assert_eq!(lone.asc_filename.as_deref(), Some("round.asc"));
        assert_eq!(lone.original_asc_text, None);
        assert_eq!(lone.design.tiers.len(), design.tiers.len());
    }

    /// An attached `.indicatrix` file that does not parse leaves the `.asc` beside it
    /// to load, and is not used on its own.
    #[test]
    fn an_unreadable_indicatrix_attachment_is_not_used() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                .to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: "design.indicatrix".to_string(),
            url: String::new(),
            content: b"not a design".to_vec(),
        });
        let loaded = design_from_full_record(&full).unwrap();
        assert_eq!(loaded.design.tiers.len(), 1, "the .asc still loads");

        full.attached_files.remove(0);
        full.attached_files[0].content = Vec::new();
        assert!(
            design_from_design_attachment(&full).is_none(),
            "an unreadable lone design file is not used"
        );
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

    /// A record whose only design file is a `.gem` (254 real catalogue details,
    /// none with an `.asc` sibling) opens with the `.gem`'s real geometry, not the
    /// zero-mast angle-table placeholder.
    #[test]
    fn design_from_full_record_opens_a_gem_only_record_with_real_masts() {
        use crate::gui::library::local::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem};
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "Synthetic.GEM".to_string(),
            url: String::new(),
            content: encode_gem(SYNTHETIC_GEM_TIERS, 96, "Synthetic Gem"),
        });
        // An angle-table row too, as a scraped catalogue detail has: the `.gem`
        // must win over it.
        full.angle_settings
            .push(indicatrix_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
            });
        let loaded = design_from_full_record(&full).expect("a .gem-only record loads");
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("Synthetic.asc"));
        assert_eq!(loaded.design.tiers.len(), SYNTHETIC_GEM_TIERS.len());
        let schedule = loaded
            .design
            .to_asc_schedule()
            .expect("the .gem's masts anchor every tier");
        assert_eq!(schedule.tiers.len(), SYNTHETIC_GEM_TIERS.len());
        for (tier, spec) in schedule.tiers.iter().zip(SYNTHETIC_GEM_TIERS) {
            assert!(
                (tier.mast.abs() - spec.2).abs() < 1e-6,
                "tier {} must keep the .gem's real mast {}, got {}",
                spec.0,
                spec.2,
                tier.mast
            );
        }
    }

    /// A corrupt `.gem` falls back to the angle table, exactly like an `.asc`
    /// that fails to parse.
    #[test]
    fn design_from_full_record_falls_back_when_the_gem_does_not_parse() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "broken.gem".to_string(),
            url: String::new(),
            content: vec![1, 2, 3],
        });
        full.angle_settings
            .push(indicatrix_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
            });
        let loaded = design_from_full_record(&full).expect("the angle table still loads");
        assert!(loaded.used_placeholder);
    }

    #[test]
    fn design_from_full_record_errors_with_no_schedule_data_at_all() {
        let full = empty_full_record();
        assert!(design_from_full_record(&full).is_err());
    }
}

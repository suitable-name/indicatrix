//! Resolving a [`Design`](indicatrix_cut_core::Design) from external sources -- a catalogue entry's
//! full record (`super::callbacks::setup_load_selected_callback`) -- and parsing the
//! tier/preform edit forms into `indicatrix-cut-core` edit payloads. See this group's own `mod.rs`
//! doc comment.

use super::state::material_name_from_index;
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions};
use indicatrix_cut_core::{
    ConstraintTier, Design, FreshDesignSpec, MaterialSelection, PreformSpec,
};
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
pub(super) struct LoadedDesign {
    pub(super) design: Design,
    /// `true` iff no real attached `.asc` was found and `design`'s schedule instead
    /// came from the angle-table placeholder reconstruction (every mast `0.0`) --
    /// see [`design_from_full_record`]'s doc comment.
    pub(super) used_placeholder: bool,
    /// Phase 7: the real attached `.asc`'s own bare file name, `None` on the
    /// placeholder path -- reconstructed text was never a real `.asc` this catalogue
    /// entry ships, so there is nothing there worth [`indicatrix_cut_core::save_paired`]
    /// preserving verbatim. Fed to `super::state::EditorState::asc_filename` by
    /// `super::callbacks::setup_load_selected_callback`.
    pub(super) asc_filename: Option<String>,
    /// Phase 7: the real attached `.asc`'s own exact original text, `None` on the
    /// placeholder path for the identical reason. Fed to
    /// `super::state::EditorState::original_asc_text`.
    pub(super) original_asc_text: Option<String>,
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
pub(super) fn design_from_asc_text(
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
/// # Errors
///
/// Returns `Err` (a human-readable message) only when there is no attached `.asc` AND
/// no angle-settings table to reconstruct from at all -- nothing in this diagram's
/// record describes a cutting schedule.
pub(super) fn design_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Result<LoadedDesign, String> {
    if let Some(attached) = full
        .attached_files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with(".asc"))
    {
        let text = String::from_utf8_lossy(&attached.content);
        match design_from_asc_text(&attached.name, &text, full.lw_ratio.as_deref()) {
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
pub(super) fn external_proportions_from_full_record(
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

/// Parses the tier-edit form's five text/enum fields into a [`ConstraintTier`]. Pure
/// and unit tested directly (see this module's tests) rather than only exercised
/// through the Slint callback -- every validation an editor's numeric text field
/// needs (not a number, not finite) surfaces as a plain `Err` message shown via a
/// toast, never a panic on bad user input.
///
/// There is deliberately no `mast` field here any more -- see this group's `mod.rs`
/// doc comment and `indicatrix_cut_core::design`'s own module docs ("Constraints, not masts"): a
/// mast is [`Design::solve`]'s output, never authored state, so the form no longer
/// offers a text field for it at all. `constraint_kind` is `EditorTierItem`'s own
/// three-way encoding (`app.slint`'s `editor_save_tier` callback), the inverse of
/// `super::state`'s `constraint_kind_and_text`:
/// - `0` -- [`MeetConstraint::MeetExisting`] (`constraint_text` ignored).
/// - `1` -- [`MeetConstraint::MeetNamed`], `constraint_text` a comma-separated name
///   list.
/// - `2` -- [`MeetConstraint::ScaleReference`], `constraint_text` a single real
///   number (the authored dimension itself).
/// - anything else -- rejected as an `Err`, never silently coerced to a default
///   constraint kind.
///
/// Indices split on comma/space/semicolon, matching
/// `indicatrix_vault::local::reconstruct_asc_schedule`'s own splitting convention for
/// the same kind of field, so a value copied out of that reconstruction's own indices
/// column round-trips straight back in.
///
/// `imported_meet` is threaded straight through to the built [`ConstraintTier`]
/// unchanged -- this function never inspects or clears it. The caller
/// (`super::callbacks::setup_save_tier_callback`) passes the CURRENT tier's own
/// `imported_meet` when editing an existing row (`index >= 0`) so that tweaking,
/// say, just this tier's name does not silently discard the file's stated meet
/// instruction, and `None` when adding a brand-new tier (`index < 0`), which
/// has no import history to preserve at all. The one-click "Adopt" action
/// (`super::callbacks::setup_adopt_meet_callback`) is the only thing that ever
/// actually CONSULTS this field to build a new constraint from it.
/// Parses just the angle text an inline tier-list cell commits (`inline_set_angle`)
/// -- the same validation [`parse_tier_form`]'s own angle field applies (must parse
/// as a finite `f64`), pulled out so the inline cell doesn't need a whole tier form's
/// worth of other fields just to validate one number.
pub(super) fn parse_angle_only(angle: &str) -> Result<f64, String> {
    let angle_deg: f64 = angle
        .trim()
        .parse()
        .map_err(|_| format!("Angle '{}' is not a number.", angle.trim()))?;
    if !angle_deg.is_finite() {
        return Err("Angle must be a finite number.".to_string());
    }
    Ok(angle_deg)
}

pub(super) fn parse_tier_form(
    angle: &str,
    constraint_kind: i32,
    constraint_text: &str,
    name: &str,
    indices: &str,
    imported_meet: Option<MeetConstraint>,
) -> Result<ConstraintTier, String> {
    let angle_deg: f64 = angle
        .trim()
        .parse()
        .map_err(|_| format!("Angle '{}' is not a number.", angle.trim()))?;
    if !angle_deg.is_finite() {
        return Err("Angle must be a finite number.".to_string());
    }

    let mut parsed_indices = Vec::new();
    for token in indices.split([',', ' ', ';']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let value: f64 = token
            .parse()
            .map_err(|_| format!("Index '{token}' is not a number."))?;
        parsed_indices.push(value);
    }

    let constraint = match constraint_kind {
        0 => MeetConstraint::MeetExisting,
        1 => {
            let names: Vec<String> = constraint_text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if names.is_empty() {
                return Err("\"Meet named\" needs at least one facet name.".to_string());
            }
            MeetConstraint::MeetNamed(names)
        }
        2 => {
            let value: f64 = constraint_text.trim().parse().map_err(|_| {
                format!(
                    "Scale reference '{}' is not a number.",
                    constraint_text.trim()
                )
            })?;
            if !value.is_finite() {
                return Err("Scale reference must be a finite number.".to_string());
            }
            MeetConstraint::ScaleReference(value)
        }
        other => return Err(format!("Unknown constraint kind {other}.")),
    };

    Ok(ConstraintTier {
        angle_deg,
        name: name.trim().to_string(),
        indices: parsed_indices,
        constraint,
        imported_meet,
        detached: Vec::new(),
    })
}

/// Parses the preform form's shape choice and three dimension fields into a
/// [`PreformSpec`]. Pure and unit tested directly, same reasoning as
/// [`parse_tier_form`]. `cylinder_sides` is passed in (rather than read from a
/// `Design` inside this function) so it stays decoupled from `indicatrix-cut-core` state and easy
/// to test in isolation -- the caller resolves it from the active design's own
/// `schedule.gear_teeth_abs()`, matching [`indicatrix_cut_core::PreformSpec::cylinder_for_schedule`]'s
/// own "fold count matches the index gear" reasoning.
pub(super) fn parse_preform_form(
    shape_index: i32,
    half_width: &str,
    length_over_width: &str,
    depth: &str,
    cylinder_sides: usize,
) -> Result<PreformSpec, String> {
    let half_width: f64 = half_width
        .trim()
        .parse()
        .map_err(|_| format!("Half-width '{}' is not a number.", half_width.trim()))?;
    let length_over_width: f64 = length_over_width.trim().parse().map_err(|_| {
        format!(
            "Length/width '{}' is not a number.",
            length_over_width.trim()
        )
    })?;
    let depth: f64 = depth
        .trim()
        .parse()
        .map_err(|_| format!("Depth '{}' is not a number.", depth.trim()))?;
    if !(half_width.is_finite() && length_over_width.is_finite() && depth.is_finite())
        || half_width <= 0.0
        || length_over_width <= 0.0
        || depth <= 0.0
    {
        return Err("Preform dimensions must be positive, finite numbers.".to_string());
    }

    Ok(if shape_index == 0 {
        PreformSpec::block(half_width, length_over_width, depth)
    } else {
        PreformSpec::cylinder(cylinder_sides, half_width, length_over_width, depth)
    })
}

/// The New Design dialog's own symmetry-order/mirror/material fields,
/// combined with an already-resolved gear tooth count and [`PreformSpec`]
/// (the caller -- `super::callbacks::setup_new_design_create_callback` --
/// resolves those two first, via [`gear_choice_to_teeth`] then
/// [`parse_preform_form`], since a cylinder preform's fold count needs the
/// gear already resolved; kept as separate calls rather than folded into one
/// wide function here to stay under clippy's `too_many_arguments`, matching
/// `gui::tilt_hover_preview`'s own documented preference for a handful of
/// small calls over one wide parameter list), into a [`FreshDesignSpec`] for
/// `indicatrix_cut_core::Design::fresh_from_spec`. `symmetry_order_text` must be a
/// positive whole number; `material_index` reuses
/// [`material_name_from_index`] (the fixed built-in list -- see that
/// function's own doc comment; a brand-new design's starting material is
/// just that, a starting point the design settings panel can change to a
/// custom catalogue material afterward).
pub(super) fn parse_new_design_form(
    gear_teeth: i32,
    preform: PreformSpec,
    symmetry_order_text: &str,
    mirror: bool,
    material_index: i32,
) -> Result<FreshDesignSpec, String> {
    let symmetry_order: u32 = symmetry_order_text.trim().parse().map_err(|_| {
        format!(
            "Symmetry order '{}' is not a whole number.",
            symmetry_order_text.trim()
        )
    })?;
    if symmetry_order < 1 {
        return Err("Symmetry order must be a positive whole number.".to_string());
    }
    Ok(FreshDesignSpec {
        gear_teeth,
        symmetry_order,
        mirror,
        material: MaterialSelection {
            name: material_name_from_index(material_index),
            specific_gravity_override: None,
            refractive_index_override: None,
        },
        preform,
    })
}

/// What accepting the catalogue-load material suggestion applies -- see
/// `super::callbacks::setup_load_selected_callback`'s own doc comment for when
/// the suggestion (`super::material_lookup::nearest_built_in_material`) is
/// offered in the first place.
///
/// # Why this pins `refractive_index_override`
///
/// `Design::effective_refractive_index` now derives the EXPORTED RI from
/// the selected material's own built-in `n_D` whenever no override is set --
/// see that method's own doc comment. The suggestion is only ever offered
/// within 0.01 of the schedule's own recorded RI (see
/// `nearest_built_in_material`'s own tolerance parameter), but "within 0.01"
/// is not "identical": accepting a suggested name whose OWN `n_D` differs from
/// the schedule's real recorded RI by more than 0.01 would otherwise silently
/// shift what a subsequent "Export .asc"/"Save Native" writes -- exactly the
/// "never silently change an import's schedule RI" rule the plan states this
/// chapter must hold. Pinning `refractive_index_override` to the schedule's
/// own `schedule_ri` whenever that gap is real keeps an untouched import's
/// exported RI byte-for-byte unchanged after accepting the suggestion, while
/// still giving the optimizer/tilt-curve/viewport the suggested material's
/// real dispersion shape (see `super::material_lookup::resolved_gem_material`'s
/// own doc comment for how the override composes with a resolved material's
/// dispersion). `current`'s `specific_gravity_override` is carried through
/// unchanged -- this only ever touches `name`/`refractive_index_override`.
pub(super) fn material_selection_for_accepted_suggestion(
    name: &str,
    schedule_ri: f64,
    current: &MaterialSelection,
) -> MaterialSelection {
    let built_in_ri = indicatrix_cut_core::built_in_refractive_index(name);
    let refractive_index_override = match built_in_ri {
        Some(ri) if (ri - schedule_ri).abs() > 0.01 => Some(schedule_ri),
        _ => None,
    };
    MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::PreformShape;
    use indicatrix_vault::model::entry::FullDiagramRecord;

    #[test]
    fn parse_angle_only_accepts_a_well_formed_number() {
        assert_eq!(parse_angle_only(" -41.0 ").unwrap(), -41.0);
        assert_eq!(parse_angle_only("0").unwrap(), 0.0);
    }

    #[test]
    fn parse_angle_only_rejects_non_numeric_and_non_finite_text() {
        assert!(parse_angle_only("not-a-number").is_err());
        assert!(parse_angle_only("NaN").is_err());
        assert!(parse_angle_only("inf").is_err());
        assert!(parse_angle_only("").is_err());
    }

    #[test]
    fn parse_tier_form_accepts_a_well_formed_scale_reference_row() {
        let tier = parse_tier_form("-41.0", 2, "0.65", " P1 ", "0, 24, 48, 72", None).unwrap();
        assert_eq!(tier.angle_deg, -41.0);
        assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.65));
        assert_eq!(tier.name, "P1");
        assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
    }

    #[test]
    fn parse_tier_form_accepts_an_empty_index_list() {
        let tier = parse_tier_form("0.0", 2, "0.32", "T", "", None).unwrap();
        assert_eq!(tier.indices, [] as [f64; 0]);
    }

    #[test]
    fn parse_tier_form_splits_on_comma_space_and_semicolon() {
        let tier = parse_tier_form("10", 2, "0.5", "G", "1, 2 3;4", None).unwrap();
        assert_eq!(tier.indices, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn parse_tier_form_rejects_a_non_numeric_angle() {
        let err = parse_tier_form("not-a-number", 2, "0.5", "T", "", None).unwrap_err();
        assert!(err.contains("Angle"));
    }

    #[test]
    fn parse_tier_form_rejects_a_non_numeric_index() {
        let err = parse_tier_form("0.0", 2, "0.5", "T", "1, x, 3", None).unwrap_err();
        assert!(err.contains("Index"));
    }

    #[test]
    fn parse_tier_form_rejects_non_finite_values() {
        assert!(parse_tier_form("NaN", 2, "0.5", "T", "", None).is_err());
        assert!(parse_tier_form("0.0", 2, "inf", "T", "", None).is_err());
    }

    #[test]
    fn parse_tier_form_kind_zero_is_meet_existing_and_ignores_the_text_field() {
        let tier = parse_tier_form("30.0", 0, "this text is ignored", "C1", "", None).unwrap();
        assert_eq!(tier.constraint, MeetConstraint::MeetExisting);
    }

    #[test]
    fn parse_tier_form_kind_one_splits_named_facets_on_comma() {
        let tier = parse_tier_form("30.0", 1, "P1, P2 , G1", "C1", "", None).unwrap();
        assert_eq!(
            tier.constraint,
            MeetConstraint::MeetNamed(vec!["P1".to_string(), "P2".to_string(), "G1".to_string()])
        );
    }

    #[test]
    fn parse_tier_form_kind_one_rejects_an_empty_name_list() {
        let err = parse_tier_form("30.0", 1, "  , ", "C1", "", None).unwrap_err();
        assert!(err.contains("Meet named"));
    }

    #[test]
    fn parse_tier_form_rejects_an_unknown_constraint_kind() {
        let err = parse_tier_form("30.0", 3, "", "C1", "", None).unwrap_err();
        assert!(err.contains('3'));
    }

    #[test]
    fn parse_preform_form_builds_a_block_at_index_zero() {
        let preform = parse_preform_form(0, "1.2", "1.5", "0.8", 96).unwrap();
        assert_eq!(preform.shape, PreformShape::Block);
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
    }

    #[test]
    fn parse_preform_form_builds_a_cylinder_with_the_given_side_count() {
        let preform = parse_preform_form(1, "1.0", "1.0", "0.8", 64).unwrap();
        assert_eq!(preform.shape, PreformShape::Cylinder { sides: 64 });
    }

    #[test]
    fn parse_preform_form_rejects_zero_or_negative_dimensions() {
        assert!(parse_preform_form(0, "0.0", "1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "-1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "1.0", "0.0", 96).is_err());
    }

    #[test]
    fn parse_preform_form_rejects_unparseable_text() {
        assert!(parse_preform_form(0, "wide", "1.0", "0.8", 96).is_err());
    }

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

    // --- parse_new_design_form ---

    #[test]
    fn parse_new_design_form_round_trips_gear_symmetry_mirror_and_material() {
        let preform = PreformSpec::cylinder(80, 1.5, 1.0, 1.5);
        let spec = parse_new_design_form(80, preform, "6", false, 9).unwrap();
        assert_eq!(spec.gear_teeth, 80);
        assert_eq!(spec.symmetry_order, 6);
        assert!(!spec.mirror);
        assert_eq!(spec.material.name.as_deref(), Some("Quartz")); // MATERIAL_PRESET_NAMES[9]
        assert_eq!(spec.preform.half_width, 1.5);
    }

    #[test]
    fn parse_new_design_form_none_material_index_means_no_material_selected() {
        let preform = PreformSpec::cylinder(50, 1.0, 1.0, 1.0);
        let spec = parse_new_design_form(50, preform, "8", true, 0).unwrap();
        assert_eq!(spec.gear_teeth, 50);
        assert_eq!(spec.material.name, None);
    }

    #[test]
    fn parse_new_design_form_rejects_a_non_positive_symmetry_order() {
        let preform = PreformSpec::cylinder(96, 1.0, 1.0, 1.0);
        assert!(parse_new_design_form(96, preform, "0", true, 0).is_err());
        assert!(parse_new_design_form(96, preform, "wide", true, 0).is_err());
    }

    // --- material_selection_for_accepted_suggestion ---

    #[test]
    fn accepted_suggestion_sets_no_override_when_the_built_in_ri_is_within_tolerance() {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", quartz_ri, &current);
        assert_eq!(selection.name.as_deref(), Some("Quartz"));
        assert_eq!(selection.refractive_index_override, None);
    }

    #[test]
    fn accepted_suggestion_pins_the_schedule_ri_when_the_built_in_ri_differs_by_more_than_the_tolerance()
     {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        // A schedule RI 0.02 away from Quartz's own real n_D -- still within the
        // suggestion's own 0.01 SEARCH tolerance is not guaranteed here (this
        // test picks the schedule RI directly, not via `nearest_built_in_material`),
        // but exercises exactly the "accepting would otherwise silently move the
        // exported RI" case this function exists to prevent.
        let schedule_ri = quartz_ri + 0.02;
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", schedule_ri, &current);
        assert_eq!(selection.refractive_index_override, Some(schedule_ri));
    }

    #[test]
    fn accepted_suggestion_keeps_the_current_specific_gravity_override() {
        let mut current = MaterialSelection::none();
        current.specific_gravity_override = Some(3.9);
        let selection = material_selection_for_accepted_suggestion("Diamond", 2.417, &current);
        assert_eq!(selection.specific_gravity_override, Some(3.9));
    }
}

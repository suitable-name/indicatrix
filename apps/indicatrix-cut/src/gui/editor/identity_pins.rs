//! Identity pins for the editor logic shared with the browser app through
//! `crates/indicatrix-editor`: the tier-row/format view models, the yield and
//! proportion texts, the validation banner, the tier/preform/material form parsers,
//! a scripted edit sequence (its native TOML and history depth), the cutting-sheet
//! HTML and diagram PNG bytes, the Retarget proposals, and the Optimize result views.
//!
//! Every value below was recorded BEFORE that logic moved out of this crate, through
//! the desktop's own paths (re-exports or thin adapters afterwards), so a passing run
//! proves the move changed nothing the desktop shows or writes. Hashes are 64-bit
//! FNV-1a over a stable byte form (the raw bytes, or a `Debug` dump), so they do not
//! depend on `std`'s hasher.
//!
//! Two of the pins hold Windows bits: `solid_status_is_pinned` dumps the facet planes,
//! whose normals come from the facet angles through `sin` and `cos`, and
//! `retarget_output_is_pinned` dumps proposals whose angles, masts and Optimize scores
//! run through the same calls. Those go through the platform math library, and glibc
//! rounds a few arguments differently from the Windows runtime, so the dumps differ on
//! Linux in their last digits (seen 2026-10-01). Both tests are ignored off Windows;
//! running them there with `--ignored` prints that platform's hashes.

use super::{
    cut_sheet::{write_cutting_sheet_html, write_diagram_png},
    loading::{
        TierFormFields, design_from_asc_text, material_selection_for_accepted_suggestion,
        parse_angle_only, parse_index_list, parse_new_design_form, parse_preform_form,
        parse_tier_form, parse_tier_target, ri_override_to_preserve,
    },
    material_lookup::{
        EditorMaterialLookup, material_for_refractive_index, material_guess_candidates,
        nearest_built_in_material, resolved_gem_material,
    },
    retarget::{self, CrownShift, RetargetMode},
    state::{
        EditorState, angle_nudge_coalesce_key, builtin_preset_names, cutting_instructions_rows,
        design_label_text, design_material_index_from_name, design_material_options,
        design_to_gpu_planes, first_unresolved_meet_name, gear_choice_to_teeth,
        gear_index_from_teeth, gear_remap_preview, girdle_and_ratio_texts, index_chip_items,
        manufacturability_warnings_tagged, material_name_from_index, parse_design_material_form,
        parse_yield_form, preform_mm_texts, preform_y_offset_mm_text, proportion_verdicts,
        proportions_texts, representative_crown_and_pavilion_angles_deg, result_is_stale,
        ri_source_text, status_text_and_is_problem, status_text_and_is_problem_from_solved,
        tier_items, tier_items_from_solved, tier_items_stale_with_last_solved, tier_matches_filter,
        tiers_incomplete_under_proposed_symmetry, yield_report_texts,
        yield_report_texts_from_solved,
    },
    view::{
        build_optimize_preview_design, facet_count_from_solved, girdle_and_ratio_texts_from_solved,
        optimize_change_rows, optimize_result_rows, optimize_status_text, parse_optimize_weights,
        preform_mm_texts_from_solved, proportions_texts_from_solved,
    },
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolvedTier},
    optics::{dispersion::DispersionModel, materials::GemMaterial},
};
use indicatrix_cut_core::{
    AngleChange, ConstraintTier, Design, Edit, MaterialSelection, ObjectiveComponents,
    OptimizeOutcome, PreformSpec, RemapRounding,
    native::{SaveExtras, save_native_only_toml},
    templates::TEMPLATES,
};
use std::{collections::BTreeSet, fmt::Write as _};

fn fnv1a(bytes: &[u8]) -> u64 {
    indicatrix_solid::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn custom_materials() -> Vec<GemMaterial> {
    let mut garnet = GemMaterial::diamond();
    garnet.name = "My Garnet".to_string();
    garnet.dispersion = DispersionModel::Cauchy {
        a: 1.74,
        b: 0.0,
        c: 0.0,
    };
    vec![garnet]
}

fn custom_sg() -> Vec<(String, f64)> {
    vec![("My Garnet".to_string(), 3.9)]
}

fn template_design(index: usize) -> Design {
    let template = &TEMPLATES[index];
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        template.schedule_meta(),
        template.tiers(),
    )
}

fn tier(name: &str, angle_deg: f64, indices: &[f64], constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// Five fixed designs: three templates (one with a girdle diameter and a custom
/// material, one with an RI override), a design with no anchor, and the empty
/// startup design.
fn fixtures() -> Vec<Design> {
    let d0 = template_design(0);
    let mut d1 = template_design(1);
    d1.girdle_diameter_mm = Some(6.5);
    d1.material.name = Some("My Garnet".to_string());
    let mut d2 = template_design(2);
    d2.material.refractive_index_override = Some(1.66);
    let mut d3 = fresh_design();
    d3.tiers = vec![
        tier(
            "P1",
            -40.0,
            &[0.0, 12.0, 24.0],
            MeetConstraint::MeetExisting,
        ),
        tier(
            "C1",
            34.5,
            &[0.0, 12.0],
            MeetConstraint::MeetNamed(vec!["P1".to_string(), "Nope".to_string()]),
        ),
    ];
    vec![d0, d1, d2, d3, fresh_design()]
}

/// The startup design `EditorState::fresh` builds.
fn fresh_design() -> Design {
    Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54)
}

fn solved(design: &Design) -> Option<Vec<SolvedTier>> {
    design.solve().ok()
}

/// Drops the concave-tier fields (`EditorTierItem::kind`/`tool_line`,
/// `AngleItem::second_line`) and the relation field (`EditorTierItem::relation_text`)
/// from a `Debug` dump while they hold their flat default, so the dump of a planar design
/// is byte-identical to the one recorded before the concave and relation work. A flat row
/// that ever carried a concave value or a relation keeps the field in the dump and fails
/// the pin. (The leading space keeps `constraint_kind` out of it.)
fn without_flat_defaults(dump: &str) -> String {
    dump.replace(" kind: 0, ", " ")
        .replace("tool_line: \"\", ", "")
        .replace("relation_text: \"\", ", "")
        .replace("second_line: \"\", ", "")
}

fn row_dump(design: &Design, customs: &[GemMaterial]) -> String {
    let n_d = design.effective_refractive_index_with(customs);
    let mut out = String::new();
    let _ = writeln!(out, "{:?}", tier_items(design, n_d));
    let _ = writeln!(
        out,
        "{:?}",
        tier_items_stale_with_last_solved(design, n_d, None, &BTreeSet::new())
    );
    let _ = writeln!(out, "{:?}", manufacturability_warnings_tagged(design, None));
    if let Some(s) = solved(design) {
        let _ = writeln!(out, "{:?}", tier_items_from_solved(design, &s, n_d));
        let _ = writeln!(
            out,
            "{:?}",
            tier_items_stale_with_last_solved(design, n_d, Some(&s), &BTreeSet::from([0]))
        );
        let _ = writeln!(out, "{:?}", cutting_instructions_rows(design, &s));
        let _ = writeln!(
            out,
            "{:?}",
            manufacturability_warnings_tagged(design, Some(&s))
        );
    }
    let _ = writeln!(
        out,
        "{:?}",
        representative_crown_and_pavilion_angles_deg(design)
    );
    let _ = writeln!(
        out,
        "{} {}",
        tiers_incomplete_under_proposed_symmetry(design, 6, false),
        tiers_incomplete_under_proposed_symmetry(design, 8, true)
    );
    let names = ["T".to_string(), "Nope".to_string(), "crown".to_string()];
    let _ = writeln!(out, "{:?}", first_unresolved_meet_name(design, &names));
    for rounding in [
        RemapRounding::Nearest,
        RemapRounding::Floor,
        RemapRounding::Ceil,
    ] {
        let _ = writeln!(
            out,
            "{:?}",
            gear_remap_preview(design, design.meta.gear_teeth, 80, rounding)
        );
    }
    without_flat_defaults(&out)
}

#[test]
fn tier_rows_and_formats_are_pinned() {
    let customs = custom_materials();
    let got: Vec<u64> = fixtures()
        .iter()
        .map(|d| fnv1a(row_dump(d, &customs).as_bytes()))
        .collect();
    let chips = format!(
        "{:?}",
        index_chip_items(&[0.0, 12.0, 24.5, 95.999_999_999_9], &[12.0])
    );
    // Re-recorded 2026-10-05: the first four hashes moved, the empty design (no tier rows) and the
    // chips did not. The tier table shows a tier's angle as the unsigned magnitude and keeps the
    // side in `block` (`TierRow::angle_deg`/`angle_full` are `tier.angle_deg.abs()`; Save Tier
    // restores the sign, `tier_form::preserve_saved_tier_side_and_detached`), so a pavilion row now
    // dumps `angle_deg: "40.00", angle_full: "40"` where the recorded dump had `"-40.00"`/`"-40"`
    // (and a culet `"0.00"`/`"0"` for `"-0.00"`/`"-0"`). Checked for the third fixture by
    // reproducing its whole dump by hand: the current code's text hashes to the new value, and the
    // same text with only those two angle strings signed hashes to the old one.
    //
    // Re-recorded 2026-10-06 (lane L1, cutting order and codes): the hashes of the first four
    // fixtures moved again; the empty design and the chips did not. Every tier row now carries its
    // code (`EditorTierItem::code`, `P1`, `G1`, `C1`, `T`, `Culet`), which the `Debug` dump
    // shows; the three template fixtures store their tiers in the order they are cut, so their
    // rows (and the Schedule rows after them) list the pavilion first and the table last; and
    // the table's code is `T`, no longer `Table`.
    // Re-pinned 2026-10-08 for the Schedule-tab codes/index format (first three hashes).
    assert_eq!(
        (got, fnv1a(chips.as_bytes())),
        (
            vec![
                9_899_944_636_254_421_460,
                10_090_318_058_485_717_436,
                4_380_649_056_797_232_228,
                5_527_885_909_529_035_141,
                17_206_618_785_430_422_565,
            ],
            8_329_660_661_060_172_670
        )
    );
}

fn yield_dump(design: &Design, customs: &[GemMaterial]) -> String {
    let sg = custom_sg();
    let n_d = design.effective_refractive_index_with(customs);
    let mut out = String::new();
    let _ = writeln!(out, "{:?}", yield_report_texts(design, &sg));
    let _ = writeln!(out, "{:?}", proportions_texts(design));
    let _ = writeln!(out, "{:?}", girdle_and_ratio_texts(design));
    let _ = writeln!(out, "{:?}", preform_mm_texts(design));
    if let Some(s) = solved(design) {
        let _ = writeln!(out, "{:?}", yield_report_texts_from_solved(design, &s, &sg));
        let _ = writeln!(out, "{:?}", proportions_texts_from_solved(design, &s));
        let _ = writeln!(out, "{:?}", girdle_and_ratio_texts_from_solved(design, &s));
        let _ = writeln!(out, "{:?}", preform_mm_texts_from_solved(design, &s));
        if let Some(props) = design.stone_proportions(&s) {
            let v = proportion_verdicts(design, &props, n_d);
            for verdict in [
                &v.table_pct,
                &v.crown_angle,
                &v.pavilion_angle,
                &v.total_depth_pct,
                &v.girdle_pct,
            ] {
                let _ = writeln!(out, "{} {}", verdict.level, verdict.reason);
            }
        }
    }
    out
}

#[test]
fn yield_and_proportion_texts_are_pinned() {
    let customs = custom_materials();
    let got: Vec<u64> = fixtures()
        .iter()
        .map(|d| fnv1a(yield_dump(d, &customs).as_bytes()))
        .collect();
    let misc = format!(
        "{} {} {} {} {}",
        preform_y_offset_mm_text(0.3, Some(4.0)),
        preform_y_offset_mm_text(-0.125, Some(3.3)),
        preform_y_offset_mm_text(0.3, None),
        design_label_text(Some("stone.asc")),
        design_label_text(None)
    );
    assert_eq!(
        (got, misc.as_str()),
        (
            vec![
                4_014_893_606_236_560_153,
                14_597_099_822_765_318_231,
                15_920_986_950_943_351_804,
                6_638_080_532_459_035_504,
                4_860_840_979_635_099_632,
            ],
            "1.20 -0.41  stone.asc Untitled design"
        )
    );
}

fn status_dump(design: &Design) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:?}", status_text_and_is_problem(design));
    if let Some(s) = solved(design) {
        let _ = writeln!(
            out,
            "{:?}",
            status_text_and_is_problem_from_solved(design, &s)
        );
        let _ = writeln!(out, "{}", facet_count_from_solved(design, &s));
    }
    let _ = writeln!(out, "{:?}", design_to_gpu_planes(design));
    out
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding of the facet angles in the plane dump"
)]
fn solid_status_is_pinned() {
    let got: Vec<u64> = fixtures()
        .iter()
        .map(|d| fnv1a(status_dump(d).as_bytes()))
        .collect();
    let filters: Vec<bool> = [
        ("Crown Main", ""),
        ("Crown Main", "  main "),
        ("Crown Main", "MAIN"),
        ("Pavilion", "crown"),
    ]
    .iter()
    .map(|(h, f)| tier_matches_filter(h, f))
    .chain([
        result_is_stale(None, 3),
        result_is_stale(Some(3), 3),
        result_is_stale(Some(2), 3),
    ])
    .collect();
    // Re-recorded 2026-10-06 (Windows, lane L1, cutting order): the first three hashes moved. The
    // three template fixtures now store their tiers in the order they are cut, so the plane dump
    // (`design_to_gpu_planes`, one plane per tier in stored order) lists the pavilion first and
    // the table last; the planes themselves are the same. The fourth and fifth fixtures and the
    // filters are unchanged.
    assert_eq!(
        (got, filters),
        (
            vec![
                2_872_048_525_647_084_630,
                8_339_025_376_295_679_342,
                15_790_257_466_976_397_087,
                1_089_956_317_437_908_903,
                4_120_773_658_827_740_825,
            ],
            vec![true, true, true, false, false, false, true]
        )
    );
}

fn tier_form_dump() -> String {
    let cases: [(&str, i32, &str, &str, &str); 12] = [
        ("-41.0", 2, "0.65", "P1", "0, 12, 24"),
        ("34.5", 1, "G1, P1", "C1", "0:12:96"),
        ("0", 0, "", "T", ""),
        ("90", 2, "1.0", "G1", "3 x8"),
        ("91", 2, "1.0", "G1", "0"),
        ("abc", 2, "1.0", "G1", "0"),
        ("40", 1, " , ", "C2", "0"),
        ("40", 7, "", "C2", "0"),
        ("40", 3, "2.5", "C3", "0 96"),
        ("40", 2, "1", "Girdle", "0, 0"),
        ("-40", 2, "1", "P9", "5 X4"),
        ("40", 2, "1", "c1/New", "1.5"),
    ];
    let mut out = String::new();
    for (angle, kind, text, name, indices) in cases {
        let form = TierFormFields {
            angle,
            constraint_kind: kind,
            constraint_text: text,
            name,
            indices,
            gear_teeth_abs: 96,
            imported_meet: Some(MeetConstraint::MeetExisting),
            original_notes: Some("note".to_string()),
            other_tier_names: vec!["Girdle".to_string(), "C1".to_string()],
        };
        let _ = writeln!(out, "{:?}", parse_tier_form(form));
        let _ = writeln!(out, "{:?}", parse_tier_target(kind, text));
    }
    for (text, gear) in [
        ("0 12 24", 96),
        ("0:8:64", 64),
        ("90:-12:0", 96),
        ("12 x8", 96),
        ("12 x7", 96),
        ("1; 2, 3", 96),
        ("100", 96),
        ("96, 16, 32, 48", 96),
        ("0, 96", 96),
        ("0:0:5", 96),
        ("x8", 96),
    ] {
        let _ = writeln!(out, "{:?}", parse_index_list(text, gear));
    }
    for text in ["-41.0", " 12 ", "NaN", "inf", "90.01", "-90", "x"] {
        let _ = writeln!(out, "{:?}", parse_angle_only(text));
    }
    out
}

fn preform_and_material_dump() -> String {
    let mut out = String::new();
    for (shape, hw, lw, depth) in [
        (0, "1.2", "1.5", "0.8"),
        (1, "1.0", "1.0", "0.8"),
        (1, "0", "1.0", "0.8"),
        (0, "w", "1", "1"),
    ] {
        let _ = writeln!(out, "{:?}", parse_preform_form(shape, hw, lw, depth, 64));
    }
    let preform = PreformSpec::cylinder(80, 1.5, 1.0, 1.5);
    for (symmetry, material) in [("6", 9), ("8", 0), ("0", 1), ("x", 3), ("4", 99)] {
        let _ = writeln!(
            out,
            "{:?}",
            parse_new_design_form(80, preform, symmetry, true, material)
        );
    }
    let current = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: Some(3.1),
        refractive_index_override: Some(1.8),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    for (girdle, sg) in [("6.5", ""), ("", "3.2"), ("-1", ""), ("x", ""), ("", "0")] {
        let _ = writeln!(out, "{:?}", parse_yield_form(girdle, 3, sg, &current));
    }
    let customs = custom_materials();
    let options = design_material_options(&customs);
    let _ = writeln!(out, "{options:?} {:?}", builtin_preset_names());
    for (index, ri) in [(0, ""), (1, "1.9"), (5, "x"), (40, "0.9"), (-1, "")] {
        let _ = writeln!(
            out,
            "{:?}",
            parse_design_material_form(index, ri, &options, &current)
        );
    }
    for name in [Some("diamond"), Some("My Garnet"), Some("Nope"), None] {
        let _ = writeln!(
            out,
            "{} {}",
            design_material_index_from_name(name, &options),
            ri_source_text(
                &MaterialSelection {
                    name: name.map(str::to_string),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                    body_color_override: None,
                    body_color_bands_override: None,
                    absorption_path_scale_override: None,
                },
                &customs
            )
        );
    }
    let _ = writeln!(out, "{}", ri_source_text(&current, &customs));
    for index in -1..40 {
        let _ = write!(out, "{:?},", material_name_from_index(index));
    }
    for (preset, custom) in [(0, ""), (5, ""), (6, "50"), (6, "-3"), (9, "x"), (-2, "72")] {
        let _ = writeln!(out, "{:?}", gear_choice_to_teeth(preset, custom));
    }
    for teeth in [96, 80, 77, 72, 64, 120, 50, -96] {
        let _ = write!(out, "{},", gear_index_from_teeth(teeth));
    }
    out
}

fn material_search_dump() -> String {
    let mut out = String::new();
    for n_d in [1.54, 1.76, 1.72, 2.16, 1.9, 2.417, 1.62] {
        let _ = writeln!(
            out,
            "{:?} {:?} {:?} {:?} {:?}",
            nearest_built_in_material(n_d, 0.02),
            material_guess_candidates(n_d, 0.05),
            material_for_refractive_index(n_d).map(|(name, gem)| (name, gem.name)),
            ri_override_to_preserve("Quartz", n_d),
            material_selection_for_accepted_suggestion("Sapphire", n_d, &MaterialSelection::none())
        );
    }
    let asc = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
    match design_from_asc_text("pin.asc", asc, Some("1.25")) {
        Ok(loaded) => {
            let _ = writeln!(
                out,
                "{:?} {:?} {:?} {} {:?}",
                loaded.design.tiers,
                loaded.design.preform,
                loaded.design.meta,
                loaded.used_placeholder,
                loaded.asc_filename
            );
        }
        Err(e) => {
            let _ = writeln!(out, "{e}");
        }
    }
    out
}

#[test]
fn form_parsing_is_pinned() {
    let got = (
        fnv1a(tier_form_dump().as_bytes()),
        fnv1a(preform_and_material_dump().as_bytes()),
        fnv1a(material_search_dump().as_bytes()),
    );
    assert_eq!(
        got,
        (
            1_545_794_316_997_941_305,
            15_758_716_057_735_816_184,
            4_662_660_901_061_571_820
        )
    );
}

fn nudge(state: &mut EditorState, index: usize, delta: f64, out: &mut String) {
    let current = state.design.tiers[index].angle_deg;
    let outcome = state.apply_coalescing(
        Edit::RetargetAngles {
            changes: vec![(index, current, current + delta)],
        },
        angle_nudge_coalesce_key(&[index]),
    );
    let _ = writeln!(out, "nudge {outcome:?}");
}

/// The scripted sequence: three tiers added, three coalescing nudges, undo, redo,
/// mirror, complete orbit -- through the same `EditorState` entry points the
/// desktop's callbacks use.
fn scripted_sequence() -> (String, String) {
    let mut log = String::new();
    let mut state = EditorState::fresh();
    let eight = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    for (index, t) in [
        tier("G1", 90.0, &eight, MeetConstraint::ScaleReference(1.0)),
        tier(
            "P1",
            -40.0,
            &eight,
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
        tier(
            "C1",
            34.5,
            &[0.0, 12.0, 24.0],
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = state.apply(Edit::AddTier { index, tier: t });
        let _ = writeln!(log, "add {outcome:?}");
    }
    for _ in 0..3 {
        nudge(&mut state, 1, -0.25, &mut log);
    }
    let _ = writeln!(log, "undo {:?}", state.undo());
    let _ = writeln!(log, "redo {:?}", state.redo());
    let mirrored = state
        .design
        .mirror_indices(2)
        .and_then(|edit| state.apply(edit));
    let _ = writeln!(log, "mirror {mirrored:?}");
    let units = state.design.orbit_units(2);
    let anchors: Vec<f64> = units
        .iter()
        .flatten()
        .filter(|unit| !unit.is_complete())
        .filter_map(|unit| unit.members.first().copied())
        .collect();
    for position in anchors {
        let outcome = state
            .design
            .add_orbit_member(2, position)
            .and_then(|edit| state.apply(edit));
        let _ = writeln!(log, "orbit {position} {outcome:?}");
    }
    let toml = save_native_only_toml(&state.design, "pin.asc", None, &SaveExtras::default())
        .unwrap_or_else(|e| e.to_string());
    let generation = state.generation.load(std::sync::atomic::Ordering::Relaxed);
    let _ = writeln!(log, "generation {generation} dirty {}", state.is_dirty());
    let mut depth = 0;
    while matches!(state.undo(), Ok(true)) {
        depth += 1;
    }
    let mut redo_depth = 0;
    while matches!(state.redo(), Ok(true)) {
        redo_depth += 1;
    }
    let replayed = save_native_only_toml(&state.design, "pin.asc", None, &SaveExtras::default())
        .unwrap_or_else(|e| e.to_string());
    let _ = writeln!(
        log,
        "depth {depth} redo {redo_depth} replay_equal {}",
        replayed == toml
    );
    (toml, log)
}

#[test]
fn scripted_edit_sequence_is_pinned() {
    let (toml, log) = scripted_sequence();
    assert_eq!(
        (
            fnv1a(toml.as_bytes()),
            toml.len(),
            fnv1a(log.as_bytes()),
            log.as_str()
        ),
        (
            8_529_034_122_875_928_046,
            1078,
            6_969_999_423_672_735_711,
            "add Ok(())\nadd Ok(())\nadd Ok(())\nnudge Ok(())\nnudge Ok(())\nnudge Ok(())\n\
             undo Ok(true)\nredo Ok(true)\nmirror Ok(())\norbit -0 Ok(())\n\
             generation 10 dirty true\ndepth 6 redo 6 replay_equal true\n"
        )
    );
}

#[test]
fn cutting_sheet_and_diagram_bytes_are_pinned() {
    let customs = custom_materials();
    let dir =
        std::env::temp_dir().join(format!("indicatrix-cut-editor-pins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut got: Vec<(usize, u64)> = Vec::new();
    for (i, design) in fixtures().iter().take(3).enumerate() {
        let s = solved(design).expect("templates solve");
        let html = dir.join(format!("sheet{i}.html"));
        write_cutting_sheet_html(design, &s, &html, &customs).unwrap();
        let bytes = std::fs::read(&html).unwrap();
        got.push((bytes.len(), fnv1a(&bytes)));
        let png = dir.join(format!("diagram{i}.png"));
        write_diagram_png(design, &s, &png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        got.push((bytes.len(), fnv1a(&bytes)));
    }
    let _ = std::fs::remove_dir_all(&dir);
    // Re-recorded 2026-10-05. Since 2026-10-04 (18:19, `cut_sheet::diagram::render_cut_diagram`)
    // the printed diagram stamps every facet's canonical label (`P1 0`, `C2 3`, ...,
    // `indicatrix_cut_core::design::labelling`) through `DiagramStyle::facet_labels`. The ink is
    // fixed-size 5x7 text at scale 1, so the standalone PNG (1800x720) grew by about 23-24 KB and
    // the sheet's embedded 900x360 PNG by about 21 KB, which base64 and the HTML text turn into the
    // ~28 KB the three sheets grew by; the sheet carries one diagram, not two. The facet planes did
    // not move: `solid_status_is_pinned` and the yield pins above still pass unchanged.
    //
    // Fixture 0 (the Standard Round Brilliant) is pinned once per platform. Two Windows runs
    // on 2026-10-05 (12:46, before the round-2 lanes, and 16:27, after them) gave the first
    // pair; the owner's workspace run of the same day gave the second pair for it and the SAME
    // bytes as Windows for fixtures 1 and 2. The two pairs differ by a few diagram pixels (the
    // standalone PNG by 5 bytes), which fits the platform split named in this file's module
    // doc: the facet normals go through `sin` and `cos`, glibc rounds a few arguments
    // differently from the Windows runtime, and a last-digit change moves a pixel or a label
    // only where a facet centre sits on a pixel boundary. By file times no source on the
    // sheet's path changed between the 16:27 run and the owner's, apart from the pure file
    // splits. Not proven: if a Windows run ever shows the second pair, the cause is a source
    // change and not the platform.
    //
    // Re-recorded 2026-10-06 (lanes L1 and L2), on both platforms. L1: the printed sheet
    // follows `Design::cutting_order()` for a planar design too (pavilion and girdle first,
    // table last), shows each tier's code in its label column with the descriptive name at
    // the front of the instruction (`Crown Main: Meet P1, P2, G1`), and prints index lists as
    // `96-08-16`, `03.50`, `-`; the three template fixtures store their tiers in cutting
    // order. L2: `write_cutting_sheet_html` is `cutting_sheet_document` with
    // `SheetDetails::from_design`, laid out like the fantasy-cut template: heading "Cutting
    // instructions", Facet Data, Size Data, Design Data, FOUR single-panel views (each
    // 560x440, instead of the one 900x360 three-panel picture), then a Pavilion and a Crown
    // section in place of the one tier table. The sheets therefore grew from about 109 KB to
    // about 186 KB. The standalone diagram PNG moved by its labels only (numbered per letter
    // in cutting order, and the table's label is `T`, no longer `Table`); the facet planes
    // did not move.
    #[cfg(windows)]
    let fixture_0: [(usize, u64); 2] = [
        (186_214, 4_140_978_437_035_329_266),
        (145_724, 751_681_646_158_210_119),
    ];
    #[cfg(not(windows))]
    let fixture_0: [(usize, u64); 2] = [
        (186_170, 9_067_540_521_652_588_193),
        (145_681, 13_620_949_975_005_965_458),
    ];
    assert_eq!(
        got,
        [
            fixture_0[0],
            fixture_0[1],
            (184_721, 3_493_856_687_612_366_778),
            (144_425, 2_818_412_057_544_290_729),
            (181_528, 11_701_624_034_297_311_098),
            (145_389, 7_078_474_445_128_246_587),
        ]
    );
}

fn resolved(
    selection: &MaterialSelection,
    customs: &[GemMaterial],
) -> indicatrix_cut_core::ResolvedMaterial {
    let lookup = EditorMaterialLookup::new(customs);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(selection, &lookup);
    indicatrix_cut_core::ResolvedMaterial { gem, ..resolved }
}

#[test]
#[cfg_attr(
    all(not(windows), feature = "zoning"),
    ignore = "pinned to the Windows math library's rounding in the proposal angles and the Optimize scoring; zoning re-pin: owner (GemMaterial Debug dump gained `zoning: None`)"
)]
#[cfg_attr(
    all(not(windows), not(feature = "zoning")),
    ignore = "pinned to the Windows math library's rounding in the proposal angles and the Optimize scoring"
)]
#[cfg_attr(
    all(windows, feature = "zoning"),
    ignore = "zoning re-pin: owner (GemMaterial Debug dump gained `zoning: None`)"
)]
fn retarget_output_is_pinned() {
    let customs = custom_materials();
    let targets = [
        MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(2.0),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
    ];
    let crowns = [
        // The default now follows the pavilion; `fixed` keeps the old "crown stays" rule
        // pinned too.
        CrownShift::default(),
        CrownShift::fixed(),
        CrownShift {
            fraction: 0.5,
            ..CrownShift::fixed()
        },
        CrownShift {
            scale_by_ratio: true,
            ..CrownShift::fixed()
        },
    ];
    let mut got = Vec::new();
    for design in fixtures().iter().take(3) {
        let mut dump = String::new();
        for target in &targets {
            let target = resolved(target, &customs);
            for crown in crowns {
                match retarget::build_proposal(
                    design,
                    &target,
                    crown,
                    RetargetMode::Shift,
                    &customs,
                ) {
                    Ok(proposal) => {
                        let _ = writeln!(
                            dump,
                            "{proposal:?} {:?}",
                            retarget::apply(design, &proposal)
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(dump, "{e}");
                    }
                }
            }
            let optimize = retarget::build_proposal(
                design,
                &target,
                CrownShift::default(),
                RetargetMode::Optimize(indicatrix_cut_core::OptimizeConfig::default()),
                &customs,
            );
            let _ = writeln!(dump, "{:?}", optimize.err());
        }
        got.push(fnv1a(dump.as_bytes()));
    }
    // Re-recorded 2026-10-06 (Windows), three causes. Lane W1-RT (2026-10-05, deliberate
    // behaviour change): the Shift proposal no longer carries the table or the culet (a
    // retarget never tilts a flat facet, so the old table row that crossed to the pavilion
    // side is gone), a shifted angle is guarded (never past the horizontal, 1 to 89.5
    // degrees, with a note), crown rows read the crown-window margin and risk instead of the
    // pavilion formula, and the notes gained the flat-facet and guard lines. The dump above
    // therefore differs for every fixture that has a table. The Optimize half of the dump is
    // untouched.
    //
    // Lane L3 (2026-10-06, angles read positive, tiers read by code): the guard note names the
    // held angle as a magnitude (`held at 89.50°` for a pavilion row, never `-89.50°`), and a
    // relation note names its tiers by display name (code for an empty or old-style name).
    //
    // Lane L1 (2026-10-06, cutting order): the three template fixtures now store their tiers
    // in the order they are cut, so the proposal rows and the dump of the applied design list
    // the pavilion first and the table last.
    //
    // Lane RT1 (2026-10-07, deliberate behaviour change, re-recorded 2026-10-07): the default
    // crown policy follows the pavilion's stretch (crown rows move, a note names the stretch),
    // and `CrownShift::fixed()` joined the list, so the dump differs for every fixture.
    //
    // Re-pinned 2026-10-08 (Windows): the proposal's `target: ResolvedMaterial` holds a
    // `GemMaterial`, whose Debug dump gained the `absorption_unit` field; angles and scores are
    // unchanged.
    assert_eq!(got, RETARGET_DUMP_HASHES);
}

/// The default-build hashes of `retarget_output_is_pinned`.
#[cfg(not(feature = "zoning"))]
const RETARGET_DUMP_HASHES: [u64; 3] = [
    9_969_878_645_811_091_819,
    12_262_411_588_369_304_583,
    14_798_593_155_583_401_097,
];

/// The `zoning`-build twin: the proposal's `target: ResolvedMaterial` holds a `GemMaterial`, whose
/// Debug dump gains `zoning: None` with the feature, so the hashes move. These are still the
/// DEFAULT-build values (they cannot be computed without running the test); the test is
/// `#[ignore]`d under `zoning` until the owner pastes the printed values.
#[cfg(feature = "zoning")]
const RETARGET_DUMP_HASHES: [u64; 3] = [
    9_969_878_645_811_091_819,
    12_262_411_588_369_304_583,
    14_798_593_155_583_401_097,
];

fn sample_outcome(cancelled: bool, polish: usize) -> OptimizeOutcome {
    OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 12.5,
            extinction_pct: 8.0,
            tilt_brilliance_pct: 60.0,
        },
        before_score: 20.0,
        before_yield_loss_pct: 30.0,
        after: ObjectiveComponents {
            windowing_pct: 9.25,
            extinction_pct: 11.0,
            tilt_brilliance_pct: 60.0,
        },
        after_score: 15.0,
        after_yield_loss_pct: 31.5,
        evaluations: 42,
        changes: vec![
            AngleChange {
                index: 1,
                from_deg: -40.0,
                to_deg: -41.5,
            },
            AngleChange {
                index: 9,
                from_deg: 30.0,
                to_deg: 31.0,
            },
        ],
        cancelled,
        polish_evaluations: polish,
        polish_improvement: 0.42,
    }
}

#[test]
fn optimize_views_are_pinned() {
    let design = template_design(0);
    let mut dump = String::new();
    for (cancelled, polish) in [(false, 0), (true, 31)] {
        let outcome = sample_outcome(cancelled, polish);
        let _ = writeln!(
            dump,
            "{:?} {} {:?}",
            optimize_result_rows(&outcome),
            optimize_status_text(&outcome),
            optimize_change_rows(&outcome, &design)
        );
        let preview = build_optimize_preview_design(&design, &outcome);
        let _ = writeln!(dump, "{:?}", preview.tiers);
    }
    let mut empty = sample_outcome(false, 0);
    empty.changes.clear();
    let _ = writeln!(dump, "{}", optimize_status_text(&empty));
    for (w, e, t, y) in [
        ("1", "2.5", "0", 0.0),
        ("x", "1", "1", 0.4),
        ("1", "-0.5", "1", 0.4),
        ("NaN", "1", "1", 0.0),
    ] {
        // The pin predates the tone fields of `ObjectiveWeights`: print the four it recorded,
        // in the Debug shape it recorded them in, so the hash below keeps guarding the parser.
        let line = match parse_optimize_weights(w, e, t, y, 0.0) {
            Ok(v) => format!(
                "Ok(ObjectiveWeights {{ windowing: {:?}, extinction: {:?}, tilt_brilliance: {:?}, yield_weight: {:?} }})",
                v.windowing, v.extinction, v.tilt_brilliance, v.yield_weight
            ),
            Err(message) => format!("Err({message:?})"),
        };
        let _ = writeln!(dump, "{line}");
    }
    // Re-recorded 2026-10-06 (lanes L3 and L1): `optimize_change_rows`
    // now shows both angles as magnitudes and the change as the difference of the two
    // magnitudes, so the sample change `-40.0 -> -41.5` that this dump held as
    // `"-40.00°"`, `"-41.50°"`, `"-1.50°"` is now `"40.00°"`, `"41.50°"`, `"+1.50°"`, and the
    // `30.0 -> 31.0` change keeps `"+1.00°"`. The tier names in the dump are unchanged for
    // this template (its tiers are named by code already) unless one is empty or old-style.
    // Lane L1 (2026-10-06, cutting order) adds a second cause: template 0 now stores its tiers
    // in the order they are cut (pavilion first, table last), so the sample change at index 1
    // names the Girdle where it named a different tier, and the preview design's tier list is
    // in the new order. Nothing else in the dump moved.
    assert_eq!(fnv1a(dump.as_bytes()), 7_714_998_885_164_857_003);
}

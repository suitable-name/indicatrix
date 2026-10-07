//! `info`: what a design is, without solving it.

use super::{deliver, key_values, list_block, open};
use crate::{
    args::InfoArgs,
    format::{ANGLE_DECIMALS, Align, INDEX_DECIMALS, fixed, json_number, json_text, table},
    load::Loaded,
    materials::{Catalogue, resolve_scoring},
    outcome::CommandResult,
};
use indicatrix::geometry::meet_solver::{Block, MeetConstraint, classify_blocks};
use indicatrix_cut_core::{
    ConstraintTier, Design, PreformShape, TierLabelInfo, compute_tier_labels,
};
use serde_json::{Value, json};

/// The word for a tier's block.
const fn block_word(block: Block) -> &'static str {
    match block {
        Block::Crown => "crown",
        Block::Pavilion => "pavilion",
        Block::Girdle => "girdle",
    }
}

/// The standard code of the tier at `index` (`P1`, `G1`, `C1`, `Table`, ...), as the tier
/// table and the cutting sheet show it. Empty for an index with no label.
fn tier_code(labels: &[TierLabelInfo], index: usize) -> String {
    labels
        .get(index)
        .map_or_else(String::new, |label| label.code.clone())
}

/// The tier's own name for the column beside its code: what the file calls it (`Pavilion
/// Main`, or an old-style `3`), and nothing when that is just the code again.
fn name_beside_code(name: &str, code: &str) -> String {
    let name = name.trim();
    if name.eq_ignore_ascii_case(code) {
        String::new()
    } else {
        name.to_string()
    }
}

/// An angle as the text report shows it: the magnitude, because the block column names the
/// side. The JSON report keeps the stored signed angle.
fn angle_text(angle_deg: f64) -> String {
    fixed(angle_deg.abs(), ANGLE_DECIMALS)
}

/// One index position: whole numbers without decimals, others with trailing zeros trimmed.
fn index_text(position: f64) -> String {
    if position.fract() == 0.0 {
        format!("{position:.0}")
    } else {
        let text = format!("{position:.4}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The index positions of a tier, space separated.
fn indices_text(tier: &ConstraintTier) -> String {
    tier.indices
        .iter()
        .map(|position| index_text(*position))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a constraint says, in a cutter's words.
fn constraint_text(constraint: &MeetConstraint) -> String {
    match constraint {
        MeetConstraint::MeetExisting => "meets a vertex".to_string(),
        MeetConstraint::MeetNamed(names) => format!("meets {}", names.join(", ")),
        MeetConstraint::ScaleReference(mast) => format!("mast {mast:.4}"),
    }
}

/// What a tier meets or follows: the meet the file stated when it stated one (and the mast the
/// import pinned it at), else its constraint; a tier whose angle follows a relation says so.
fn meet_text(design: &Design, index: usize, tier: &ConstraintTier) -> String {
    if let Some(relation) = design.relation_text(index) {
        return format!("follows {relation}");
    }
    match (&tier.imported_meet, &tier.constraint) {
        (Some(stated), MeetConstraint::ScaleReference(mast)) => {
            format!("{} (pinned, mast {mast:.4})", constraint_text(stated))
        }
        (Some(stated), _) => constraint_text(stated),
        (None, constraint) => constraint_text(constraint),
    }
}

/// The kind of a constraint, for JSON.
const fn constraint_kind(constraint: &MeetConstraint) -> &'static str {
    match constraint {
        MeetConstraint::MeetExisting => "meet_existing",
        MeetConstraint::MeetNamed(_) => "meet_named",
        MeetConstraint::ScaleReference(_) => "scale_reference",
    }
}

/// Decimals of a preform figure in the text report.
const PREFORM_DECIMALS: usize = 4;

/// The preform line: its shape, size and the offset of its vertical span. The half width, depth
/// and offset are in the mast units every plane offset of a design uses, not millimetres; the
/// length/width ratio is a plain ratio and is named after them, outside the unit note.
fn preform_text(design: &Design) -> String {
    let preform = &design.preform;
    let shape = match preform.shape {
        PreformShape::Block => "block".to_string(),
        PreformShape::Cylinder { sides } => format!("cylinder ({sides} sides)"),
    };
    format!(
        "{shape}, half width {}, depth {}, offset {} (mast units), length/width {}",
        fixed(preform.half_width, PREFORM_DECIMALS),
        fixed(preform.depth, PREFORM_DECIMALS),
        fixed(design.preform_y_offset, PREFORM_DECIMALS),
        fixed(preform.length_over_width, PREFORM_DECIMALS),
    )
}

/// The preform, as JSON.
fn preform_json(design: &Design) -> Value {
    let preform = &design.preform;
    let (shape, sides) = match preform.shape {
        PreformShape::Block => ("block", None),
        PreformShape::Cylinder { sides } => ("cylinder", Some(sides)),
    };
    json!({
        "shape": shape,
        "sides": sides,
        "half_width": json_number(preform.half_width, 6),
        "length_over_width": json_number(preform.length_over_width, 6),
        "depth": json_number(preform.depth, 6),
        "y_offset": json_number(design.preform_y_offset, 6),
    })
}

/// The material line: the name with its refractive index, or why it has none.
fn material_text(design: &Design, catalogue: &Catalogue) -> String {
    if let Ok(scoring) = resolve_scoring(design, catalogue, None) {
        let selection = &scoring.selection;
        if scoring.note.is_some() {
            format!(
                "none set (refractive index {} from the schedule)",
                fixed(scoring.n_d(), INDEX_DECIMALS)
            )
        } else if selection.name.is_some() && selection.refractive_index_override.is_none() {
            format!(
                "{} (n {})",
                scoring.label,
                fixed(scoring.n_d(), INDEX_DECIMALS)
            )
        } else {
            scoring.label
        }
    } else {
        let name = design.material.name.as_deref().unwrap_or("?");
        format!("{name} (not found: give the design library with --db FILE)")
    }
}

/// The material, as JSON.
fn material_json(design: &Design, catalogue: &Catalogue) -> Value {
    let found = resolve_scoring(design, catalogue, None).ok();
    json!({
        "name": design.material.name,
        "refractive_index_override": design.material.refractive_index_override,
        "refractive_index": found.as_ref().map(|scoring| json_number(scoring.n_d(), 6)),
        "resolved": found.is_some(),
    })
}

/// The report as text.
fn text_report(loaded: &Loaded, catalogue: &Catalogue) -> String {
    let design = &loaded.design;
    let meta = &design.meta;
    let mirror = if meta.mirror { "mirror" } else { "no mirror" };
    let mut pairs = vec![
        ("Design", loaded.name.clone()),
        ("File", format!("{} ({})", loaded.file_name, loaded.source)),
        (
            "Gear",
            format!(
                "{} teeth, reference angle {}",
                meta.gear_teeth_abs(),
                fixed(meta.gear_reference_angle, ANGLE_DECIMALS)
            ),
        ),
        (
            "Symmetry",
            format!("{}-fold, {mirror}", meta.symmetry_order),
        ),
        ("Preform", preform_text(design)),
        ("Material", material_text(design, catalogue)),
    ];
    if let Some(width) = design.girdle_diameter_mm {
        pairs.push(("Width", format!("{} mm", fixed(width, 2))));
    }
    let concave = design.concave_tiers.len();
    let tiers = if concave == 0 {
        design.tiers.len().to_string()
    } else {
        format!("{} (and {concave} concave)", design.tiers.len())
    };
    pairs.push(("Tiers", tiers));
    let mut text = key_values(&pairs);

    let blocks = classify_blocks(&design.meet_tier_inputs());
    let labels = compute_tier_labels(&design.tiers);
    let rows: Vec<Vec<String>> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let code = tier_code(&labels, index);
            let name = name_beside_code(&tier.name, &code);
            vec![
                (index + 1).to_string(),
                code,
                name,
                blocks
                    .get(index)
                    .map_or("", |block| block_word(*block))
                    .to_string(),
                angle_text(tier.angle_deg),
                meet_text(design, index, tier),
                indices_text(tier),
            ]
        })
        .collect();
    if !rows.is_empty() {
        text.push('\n');
        text.push_str(&table(
            &["#", "code", "name", "block", "angle", "meets", "indices"],
            &[
                Align::Right,
                Align::Left,
                Align::Left,
                Align::Left,
                Align::Right,
                Align::Left,
                Align::Left,
            ],
            &rows,
        ));
    }
    if !loaded.notes.is_empty() {
        text.push('\n');
        text.push_str(&list_block("Notes", &loaded.notes));
    }
    text
}

/// The report as JSON.
fn json_report(loaded: &Loaded, catalogue: &Catalogue) -> String {
    let design = &loaded.design;
    let meta = &design.meta;
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let labels = compute_tier_labels(&design.tiers);
    let tiers: Vec<Value> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let mast = match &tier.constraint {
                MeetConstraint::ScaleReference(mast) => json_number(*mast, 6),
                _ => Value::Null,
            };
            json!({
                "number": index + 1,
                "code": tier_code(&labels, index),
                "name": tier.name,
                "block": blocks.get(index).map(|block| block_word(*block)),
                "angle_deg": json_number(tier.angle_deg, 6),
                "indices": tier.indices.iter().map(|p| json_number(*p, 4)).collect::<Vec<_>>(),
                "constraint": constraint_kind(&tier.constraint),
                "mast": mast,
                "meets": meet_text(design, index, tier),
                "follows": design.relation_text(index),
            })
        })
        .collect();
    json_text(&json!({
        "name": loaded.name,
        "file": loaded.file_name,
        "format": loaded.source,
        "gear_teeth": meta.gear_teeth_abs(),
        "gear_reference_angle_deg": json_number(meta.gear_reference_angle, 6),
        "symmetry_order": meta.symmetry_order,
        "mirror": meta.mirror,
        "width_mm": design.girdle_diameter_mm.map(|mm| json_number(mm, 4)),
        "schedule_refractive_index": json_number(meta.refractive_index, 6),
        "preform": preform_json(design),
        "material": material_json(design, catalogue),
        "tier_count": design.tiers.len(),
        "concave_tier_count": design.concave_tiers.len(),
        "tiers": tiers,
        "notes": loaded.notes,
    }))
}

/// Runs `info`.
///
/// # Errors
///
/// [`crate::outcome::CliError`] when the design or the library cannot be opened.
pub fn run(args: &InfoArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    let report = if args.json {
        json_report(&loaded, &catalogue)
    } else {
        text_report(&loaded, &catalogue)
    };
    Ok(deliver(report, args.out.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use indicatrix_cut_core::PreformSpec;

    fn template_loaded() -> Loaded {
        Loaded::in_memory(testing::template(), "round.indicatrix")
    }

    #[test]
    fn index_positions_are_short_and_exact() {
        assert_eq!(index_text(12.0), "12");
        assert_eq!(index_text(0.0), "0");
        assert_eq!(index_text(6.5), "6.5");
        assert_eq!(index_text(6.125), "6.125");
        assert_eq!(index_text(1.0 / 3.0), "0.3333");
    }

    #[test]
    fn a_meet_reads_in_a_cutters_words() {
        assert_eq!(
            constraint_text(&MeetConstraint::MeetExisting),
            "meets a vertex"
        );
        assert_eq!(
            constraint_text(&MeetConstraint::MeetNamed(vec![
                "P1".to_string(),
                "G1".to_string()
            ])),
            "meets P1, G1"
        );
        assert_eq!(
            constraint_text(&MeetConstraint::ScaleReference(0.5)),
            "mast 0.5000"
        );
    }

    #[test]
    fn the_text_report_lists_the_header_and_every_tier() {
        let loaded = template_loaded();
        let report = text_report(&loaded, &Catalogue::default());
        assert!(
            report.starts_with(&format!("Design:    {}\n", loaded.name)),
            "{report}"
        );
        assert!(report.contains("Gear:"), "{report}");
        assert!(report.contains("96 teeth"), "{report}");
        assert!(report.contains("8-fold, mirror"), "{report}");
        assert!(report.contains("Material:  Diamond (n 2.41"), "{report}");
        let labels = compute_tier_labels(&loaded.design.tiers);
        for (tier, label) in loaded.design.tiers.iter().zip(&labels) {
            assert!(report.contains(&tier.name), "{report}");
            assert!(report.contains(&label.code), "{report}");
        }
        // Header, rule and one line per tier follow the key-value block.
        let table_lines = report
            .lines()
            .skip_while(|line| !line.trim_start().starts_with('#'))
            .count();
        assert_eq!(table_lines, 2 + loaded.design.tiers.len(), "{report}");
        assert!(report.lines().all(|line| line == line.trim_end()));
    }

    #[test]
    fn the_json_report_has_the_same_facts() {
        let loaded = template_loaded();
        let text = json_report(&loaded, &Catalogue::default());
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["name"], loaded.name.as_str());
        assert_eq!(value["gear_teeth"], 96);
        assert_eq!(value["symmetry_order"], 8);
        assert_eq!(value["mirror"], true);
        assert_eq!(value["material"]["name"], "Diamond");
        assert_eq!(value["material"]["resolved"], true);
        assert_eq!(
            value["tiers"].as_array().map(Vec::len),
            Some(loaded.design.tiers.len())
        );
        assert_eq!(value["tiers"][0]["number"], 1);
        assert_eq!(value["tier_count"], loaded.design.tiers.len());
    }

    #[test]
    fn the_preform_is_reported_in_text_and_json() {
        let mut design = testing::template();
        design.preform = PreformSpec::block(2.0, 1.5, 4.0);
        design.preform_y_offset = 0.25;
        assert_eq!(
            preform_text(&design),
            "block, half width 2.0000, depth 4.0000, offset 0.2500 (mast units), \
             length/width 1.5000"
        );
        let loaded = Loaded::in_memory(design.clone(), "block.indicatrix");
        let report = text_report(&loaded, &Catalogue::default());
        assert!(
            report.contains("Preform:   block, half width 2.0000"),
            "{report}"
        );

        let value: Value =
            serde_json::from_str(&json_report(&loaded, &Catalogue::default())).expect("valid JSON");
        assert_eq!(value["preform"]["shape"], "block");
        assert!(value["preform"]["sides"].is_null());
        assert_eq!(value["preform"]["half_width"], 2.0);
        assert_eq!(value["preform"]["length_over_width"], 1.5);
        assert_eq!(value["preform"]["depth"], 4.0);
        assert_eq!(value["preform"]["y_offset"], 0.25);

        design.preform = PreformSpec::cylinder(96, 2.0, 1.0, 4.0);
        assert!(
            preform_text(&design).starts_with("cylinder (96 sides), half width 2.0000"),
            "{}",
            preform_text(&design)
        );
        let loaded = Loaded::in_memory(design, "round.indicatrix");
        let value: Value =
            serde_json::from_str(&json_report(&loaded, &Catalogue::default())).expect("valid JSON");
        assert_eq!(value["preform"]["shape"], "cylinder");
        assert_eq!(value["preform"]["sides"], 96);
    }

    /// The row of the tier table that starts with row number `number`.
    fn row_of(report: &str, number: usize) -> String {
        report
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{number} ")))
            .unwrap_or_else(|| panic!("no row {number} in {report}"))
            .to_string()
    }

    #[test]
    fn a_tier_row_shows_the_standard_code_the_own_name_and_a_positive_angle() {
        let mut design = testing::brilliant_at_172();
        let pavilion = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        design.tiers[pavilion].angle_deg = -41.0;
        let labels = compute_tier_labels(&design.tiers);
        let code = labels[pavilion].code.clone();
        let loaded = Loaded::in_memory(design, "round.indicatrix");
        let report = text_report(&loaded, &Catalogue::default());

        let header = report
            .lines()
            .find(|line| line.trim_start().starts_with('#'))
            .expect("the table header");
        let columns: Vec<&str> = header.split_whitespace().collect();
        assert_eq!(
            columns,
            ["#", "code", "name", "block", "angle", "meets", "indices"]
        );

        let row = row_of(&report, pavilion + 1);
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(cells[1], code, "{row}");
        assert!(row.contains("Pavilion Main"), "{row}");
        assert!(row.contains("pavilion"), "{row}");
        // Stored as -41, shown as 41.
        assert!(row.contains(" 41.0000"), "{row}");
        assert!(!row.contains("-41.0000"), "{row}");
    }

    #[test]
    fn an_old_style_name_is_kept_beside_its_code_and_a_name_equal_to_the_code_is_not_repeated() {
        assert_eq!(name_beside_code("3", "P2"), "3");
        assert_eq!(name_beside_code("  Pavilion Main ", "P1"), "Pavilion Main");
        assert_eq!(name_beside_code("P1", "P1"), "");
        assert_eq!(name_beside_code("p1", "P1"), "");
        assert_eq!(name_beside_code("", "C1"), "");
    }

    #[test]
    fn a_culet_stored_at_minus_zero_reads_zero_not_minus_zero() {
        assert_eq!(angle_text(-0.0), "0.0000");
        assert_eq!(angle_text(-41.0), "41.0000");
        assert_eq!(angle_text(34.5), "34.5000");
    }

    #[test]
    fn the_json_report_keeps_the_stored_signed_angle_and_adds_the_code() {
        let mut design = testing::brilliant_at_172();
        let pavilion = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        design.tiers[pavilion].angle_deg = -41.0;
        let code = compute_tier_labels(&design.tiers)[pavilion].code.clone();
        let loaded = Loaded::in_memory(design, "round.indicatrix");
        let value: Value =
            serde_json::from_str(&json_report(&loaded, &Catalogue::default())).expect("valid JSON");
        let tier = &value["tiers"][pavilion];
        assert_eq!(tier["angle_deg"], -41.0);
        assert_eq!(tier["code"], code.as_str());
        assert_eq!(tier["name"], "Pavilion Main");
        assert_eq!(tier["block"], "pavilion");
    }

    #[test]
    fn the_report_is_the_same_every_time() {
        let loaded = template_loaded();
        let catalogue = Catalogue::default();
        assert_eq!(
            text_report(&loaded, &catalogue),
            text_report(&loaded, &catalogue)
        );
        assert_eq!(
            json_report(&loaded, &catalogue),
            json_report(&loaded, &catalogue)
        );
    }

    #[test]
    fn a_design_with_no_material_says_where_its_index_comes_from() {
        let mut design = testing::template();
        design.material = indicatrix_cut_core::MaterialSelection::none();
        let line = material_text(&design, &Catalogue::default());
        assert!(line.starts_with("none set (refractive index "), "{line}");
    }

    #[test]
    fn a_missing_custom_material_is_reported_not_fatal() {
        let mut design = testing::template();
        design.material.name = Some("Vanished Custom".to_string());
        let line = material_text(&design, &Catalogue::default());
        assert!(line.contains("not found"), "{line}");
        let value = material_json(&design, &Catalogue::default());
        assert_eq!(value["resolved"], false);
    }

    #[test]
    fn end_to_end_info_prints_text_and_json_and_can_write_a_file() {
        let path = testing::write_design("info-e2e.indicatrix", &testing::template());
        let text = testing::run(&["info", &path]);
        assert_eq!(text.exit, 0, "{}", text.stderr);
        assert!(text.stdout.contains("Tiers:"), "{}", text.stdout);

        let json = testing::run(&["info", &path, "--json"]);
        assert_eq!(json.exit, 0, "{}", json.stderr);
        let value: Value = serde_json::from_str(&json.stdout).expect("valid JSON");
        assert_eq!(value["format"], ".indicatrix");

        let target = testing::temp_path("info-e2e-report.txt");
        let target = target.to_string_lossy().into_owned();
        let written = testing::run(&["info", &path, "--out", &target]);
        assert_eq!(written.exit, 0);
        assert_eq!(written.stdout, "");
        assert_eq!(
            testing::file_text(&written, &target),
            Some(text.stdout.as_str())
        );
    }

    #[test]
    fn end_to_end_info_on_a_missing_file_is_an_io_error() {
        let missing = testing::temp_path("info-missing.indicatrix");
        let outcome = testing::run(&["info", &missing.to_string_lossy()]);
        assert_eq!(outcome.exit, crate::outcome::EXIT_IO);
        assert!(
            outcome.stderr.starts_with("error: cannot read"),
            "{}",
            outcome.stderr
        );
    }
}

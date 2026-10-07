//! `validate`: the manufacturability warnings and the overall verdict of a design.
//!
//! The verdict is the one the editor's status strip shows (Good, Check or Problem, with the
//! reasons and the fix each reason offers), computed by `indicatrix_editor::verdict` from the
//! same inputs. The measured optics (windowing, extinction, brilliance with the table up) join
//! it only when a material is known: the design names one, or `--material` or `--ri` does.
//! A design that does not solve or close is not an error here: it is a Problem verdict.
//!
//! The exit code is 4 when the verdict is Problem, 0 otherwise.

use super::{deliver, key_values, list_block, open};
use crate::{
    args::{ValidateArgs, lighting_name},
    format::{INDEX_DECIMALS, PERCENT_DECIMALS, fixed, json_number, json_text},
    load::Loaded,
    materials::{Catalogue, resolve_scoring},
    outcome::{CliError, CommandResult, EXIT_OK, EXIT_PROBLEM},
    stone::{Closure, analyze},
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    retarget::{
        metrics::{MetricColumn, measure_column},
        plan::tier_display_names,
    },
    verdict::{FixAction, Level, Reason, Verdict, evaluate, gather},
};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// The verdict of a design with what it was judged on.
struct Report {
    verdict: Verdict,
    warnings: Vec<String>,
    optics: Option<MetricColumn>,
    material: Option<String>,
    n_d: f64,
    lighting: &'static str,
    notes: Vec<String>,
}

/// Judges `loaded`.
///
/// # Errors
///
/// [`CliError`] when `--material` is unknown or the design's own material cannot be found.
fn judge(loaded: &Loaded, catalogue: &Catalogue, args: &ValidateArgs) -> Result<Report, CliError> {
    let design = &loaded.design;
    let analysis = analyze(design);
    let mut notes = loaded.notes.clone();
    let material = &design.material;
    let named = args.material.is_some()
        || material.name.is_some()
        || material.refractive_index_override.is_some();
    let scoring = if named {
        Some(resolve_scoring(design, catalogue, args.material.as_ref())?)
    } else {
        None
    };
    let n_d = if let Some(scoring) = &scoring {
        notes.extend(scoring.note.clone());
        scoring.n_d()
    } else {
        let n_d = design.effective_refractive_index_with(catalogue.custom());
        notes.push(format!(
            "the design names no material, so the optics are not judged and windowing is read \
             at refractive index {}",
            fixed(n_d, INDEX_DECIMALS)
        ));
        n_d
    };
    // A solve that left a tier unplaced has no stone to read: it is judged as not solved.
    let solved = if analysis.closure == Closure::NotSolved {
        None
    } else {
        analysis.solved.as_deref()
    };
    let inputs = gather(design, solved, n_d, analysis.failure.as_deref());
    let optics = match (&scoring, solved) {
        (Some(scoring), Some(solved)) if analysis.is_closed() => Some(measure_column(
            &design.planes_from_solved(solved),
            &scoring.resolved.gem,
            args.lighting,
        )),
        _ => None,
    };
    Ok(Report {
        warnings: inputs.warnings.iter().map(ToString::to_string).collect(),
        verdict: evaluate(&inputs.with_optics(optics)),
        optics,
        material: scoring.map(|scoring| scoring.label),
        n_d,
        lighting: lighting_name(args.lighting),
        notes,
    })
}

/// Where a reason points: `row 4: Pavilion Main` (the name the tier table shows: the tier's
/// own name, or its standard code when it has none or an old-style one such as `3`), or just
/// `row 4` for a tier the design lacks.
fn tier_text(design: &Design, tier: usize) -> String {
    match tier_display_names(design).get(tier) {
        Some(name) if !name.trim().is_empty() => format!("row {}: {}", tier + 1, name.trim()),
        _ => format!("row {}", tier + 1),
    }
}

/// One reason as a line of the text report.
fn reason_line(design: &Design, reason: &Reason) -> String {
    let mut line = format!("[{}] {}", reason.level.word(), reason.text);
    if let Some(tier) = reason.tier {
        let _ = write!(line, " ({})", tier_text(design, tier));
    }
    if let Some(fix) = &reason.fix {
        let _ = write!(line, " Fix in the editor: {}.", fix.label());
    }
    line
}

/// The optics as one line.
fn optics_text(optics: &MetricColumn) -> String {
    format!(
        "windowing {} %, brilliance {} %, extinction {} % (table up, canonical light)",
        fixed(f64::from(optics.windowing_pct), PERCENT_DECIMALS),
        fixed(f64::from(optics.brilliance_pct), PERCENT_DECIMALS),
        fixed(f64::from(optics.extinction_pct), PERCENT_DECIMALS),
    )
}

/// The text report.
fn text_report(loaded: &Loaded, report: &Report) -> String {
    let design = &loaded.design;
    let mut rows = vec![
        ("Design", format!("{} ({})", loaded.name, loaded.file_name)),
        (
            "Material",
            report
                .material
                .clone()
                .unwrap_or_else(|| "none named".to_string()),
        ),
        ("Verdict", report.verdict.level.word().to_string()),
        ("Summary", report.verdict.headline.clone()),
    ];
    if let Some(optics) = &report.optics {
        rows.push(("Optics", optics_text(optics)));
    }
    let mut text = key_values(&rows);
    let reasons: Vec<String> = report
        .verdict
        .reasons
        .iter()
        .map(|reason| reason_line(design, reason))
        .collect();
    for block in [
        list_block("Reasons", &reasons),
        list_block("Warnings", &report.warnings),
        list_block("Notes", &report.notes),
    ] {
        if !block.is_empty() {
            text.push('\n');
            text.push_str(&block);
        }
    }
    text
}

/// One reason as a JSON object.
fn reason_json(design: &Design, reason: &Reason) -> Value {
    json!({
        "level": reason.level.word(),
        "kind": format!("{:?}", reason.kind),
        "text": reason.text,
        "row": reason.tier.map(|tier| tier + 1),
        "tier": reason.tier.and_then(|tier| design.tiers.get(tier)).map(|tier| tier.name.clone()),
        "fix": reason.fix.as_ref().map(FixAction::label),
    })
}

/// The JSON report.
fn json_report(loaded: &Loaded, report: &Report) -> String {
    let design = &loaded.design;
    let reasons: Vec<Value> = report
        .verdict
        .reasons
        .iter()
        .map(|reason| reason_json(design, reason))
        .collect();
    let optics = report.optics.map(|optics| {
        json!({
            "table_up_windowing_pct": json_number(f64::from(optics.windowing_pct), 2),
            "table_up_brilliance_pct": json_number(f64::from(optics.brilliance_pct), 2),
            "table_up_extinction_pct": json_number(f64::from(optics.extinction_pct), 2),
        })
    });
    json_text(&json!({
        "design": loaded.name,
        "file": loaded.file_name,
        "material": report.material,
        "refractive_index": json_number(report.n_d, 4),
        "lighting": report.lighting,
        "verdict": report.verdict.level.word(),
        "level": report.verdict.level.code(),
        "headline": report.verdict.headline,
        "reasons": reasons,
        "warnings": report.warnings,
        "optics": optics,
        "notes": report.notes,
    }))
}

/// Runs `validate` on an opened design.
///
/// # Errors
///
/// [`CliError`] when the material cannot be resolved.
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &ValidateArgs) -> CommandResult {
    let report = judge(loaded, catalogue, args)?;
    let text = if args.json {
        json_report(loaded, &report)
    } else {
        text_report(loaded, &report)
    };
    let exit = if report.verdict.level == Level::Problem {
        EXIT_PROBLEM
    } else {
        EXIT_OK
    };
    Ok(deliver(text, args.out.as_deref()).with_exit(exit))
}

/// Runs `validate`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened or its material cannot be resolved. A design
/// that does not solve is not an error: it is the verdict Problem, exit code 4.
pub fn run(args: &ValidateArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{args::MaterialArg, testing};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        CANONICAL_LIGHTING_PRESET, ConstraintTier, PreformSpec, ScheduleMeta,
    };
    use indicatrix_editor::verdict::ReasonKind;
    use std::path::PathBuf;

    fn args(material: Option<MaterialArg>, json: bool) -> ValidateArgs {
        ValidateArgs {
            design: PathBuf::new(),
            json,
            material,
            lighting: CANONICAL_LIGHTING_PRESET,
            out: None,
            db: None,
        }
    }

    /// A design with a lone tier that has nothing to anchor it: it does not solve.
    fn unsolvable() -> Design {
        let tier = ConstraintTier {
            angle_deg: -41.0,
            name: "P1".to_string(),
            indices: vec![12.0, 36.0, 60.0, 84.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        };
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            vec![tier],
        )
    }

    fn template_report(material: Option<MaterialArg>) -> Report {
        let loaded = Loaded::in_memory(testing::template(), "round.indicatrix");
        judge(&loaded, &Catalogue::default(), &args(material, false)).expect("judges")
    }

    #[test]
    fn a_stone_that_closes_is_not_judged_as_unsolved() {
        let report = template_report(Some(MaterialArg::Ri(1.76)));
        assert!(
            report.verdict.reasons.iter().all(|reason| !matches!(
                reason.kind,
                ReasonKind::DoesNotSolve | ReasonKind::NotClosed
            )),
            "{:?}",
            report.verdict
        );
        assert!(report.optics.is_some(), "a named index measures the optics");
        assert_eq!(report.material.as_deref(), Some("refractive index 1.7600"));
    }

    #[test]
    fn without_a_material_the_optics_are_left_out_and_the_notes_say_so() {
        let mut design = testing::template();
        design.material = indicatrix_cut_core::MaterialSelection::default();
        let loaded = Loaded::in_memory(design, "round.indicatrix");
        let report = judge(&loaded, &Catalogue::default(), &args(None, false)).expect("judges");
        assert!(report.optics.is_none());
        assert!(report.material.is_none());
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("names no material")),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn a_design_that_does_not_solve_is_a_problem_with_exit_code_4() {
        let loaded = Loaded::in_memory(unsolvable(), "lonely.indicatrix");
        let outcome = execute(
            &loaded,
            &Catalogue::default(),
            &args(Some(MaterialArg::Ri(1.76)), false),
        )
        .expect("a verdict, not an error");
        assert_eq!(outcome.exit, EXIT_PROBLEM, "{}", outcome.stdout);
        assert!(
            outcome.stdout.contains("Verdict:   Problem"),
            "{}",
            outcome.stdout
        );
        assert!(outcome.stdout.contains("Reasons ("), "{}", outcome.stdout);
        assert_eq!(outcome.stderr, "");
    }

    #[test]
    fn the_json_verdict_matches_the_exit_code() {
        let loaded = Loaded::in_memory(unsolvable(), "lonely.indicatrix");
        let outcome = execute(
            &loaded,
            &Catalogue::default(),
            &args(Some(MaterialArg::Ri(1.76)), true),
        )
        .expect("a verdict");
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["verdict"], "Problem");
        assert_eq!(value["level"], 2);
        assert_eq!(outcome.exit, EXIT_PROBLEM);
        let reasons = value["reasons"].as_array().expect("reasons");
        assert_ne!(reasons.len(), 0);
        assert!(reasons[0]["text"].is_string());
        assert!(value["optics"].is_null());
    }

    #[test]
    fn the_exit_code_follows_the_verdict_level() {
        let loaded = Loaded::in_memory(testing::template(), "round.indicatrix");
        let outcome = execute(
            &loaded,
            &Catalogue::default(),
            &args(Some(MaterialArg::Ri(1.76)), true),
        )
        .expect("a verdict");
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        let problem = value["level"] == 2;
        assert_eq!(outcome.exit == EXIT_PROBLEM, problem, "{}", outcome.stdout);
        assert!(value["optics"]["table_up_brilliance_pct"].is_number());
    }

    #[test]
    fn reasons_name_their_row_and_their_fix() {
        let design = testing::template();
        let reason = Reason {
            kind: ReasonKind::OffGear,
            level: Level::Check,
            text: "Some positions are between gear teeth.".to_string(),
            tier: Some(2),
            fix: Some(FixAction::SnapToTeeth { tier: 2 }),
            confirm: None,
        };
        let line = reason_line(&design, &reason);
        assert!(
            line.starts_with("[Check] Some positions are between gear teeth. (row 3"),
            "{line}"
        );
        assert!(
            line.ends_with("Fix in the editor: Snap to teeth."),
            "{line}"
        );
    }

    #[test]
    fn a_reason_names_an_old_style_tier_by_its_standard_code() {
        let mut design = testing::brilliant_at_172();
        let tier = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        let named = tier_text(&design, tier);
        assert_eq!(named, format!("row {}: Pavilion Main", tier + 1));

        design.tiers[tier].name = "3".to_string();
        let code = indicatrix_cut_core::compute_tier_labels(&design.tiers)[tier]
            .code
            .clone();
        let coded = tier_text(&design, tier);
        assert_eq!(coded, format!("row {}: {code}", tier + 1));
        assert!(!coded.ends_with(": 3"), "{coded}");
        assert_eq!(tier_text(&design, 999), "row 1000");
    }

    #[test]
    fn end_to_end_validate_writes_a_report_file_and_keeps_the_verdict_exit() {
        let input = testing::write_design("validate-e2e.indicatrix", &testing::template());
        let target = testing::temp_path("validate-e2e.json")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&[
            "validate", &input, "--ri", "1.76", "--json", "--out", &target,
        ]);
        assert!(
            outcome.exit == EXIT_OK || outcome.exit == EXIT_PROBLEM,
            "{} {}",
            outcome.exit,
            outcome.stderr
        );
        let text = testing::file_text(&outcome, &target).expect("the report is written");
        let value: Value = serde_json::from_str(text).expect("valid JSON");
        assert!(value["headline"].is_string());
        assert_eq!(outcome.stdout, "");
    }
}

//! `sweep`: one tier's angle over a range, every angle solved and scored.
//!
//! This is the editor's Angle Sweep with the same engine (`indicatrix_editor::sweep`): the
//! design is cloned once per angle, the tier is set to it, the clone is solved, checked and
//! scored with Optimize's quick table-up score (and with `--tilt` the tilt averages, about a
//! second and a half a row), and the figures come back as a table. The rows run from the
//! flattest angle to the steepest and do not depend on how many threads ran them. The design's
//! own angle is always a row.
//!
//! Angles are degrees from flat, as positive numbers: `--from 39 --to 43` for a pavilion tier
//! (a sign typed in front is ignored, and the side of the girdle comes from the tier). The text
//! report and the CSV show positive angles. The JSON report keeps the stored signed convention:
//! its `angle_deg` is negative on a pavilion tier. `--csv FILE` writes the same CSV the dialog
//! does (comma separated, four decimals, CRLF line ends, a missing figure left empty).

use super::{key_values, list_block, open};
use crate::{
    args::{SweepArgs, lighting_name},
    format::{ANGLE_DECIMALS, Align, count_noun, fixed, json_optional, json_text, table},
    load::Loaded,
    materials::{Catalogue, resolve_scoring},
    outcome::{CliError, CommandResult, Outcome},
    stone::analyze,
};
use indicatrix_cut_core::{
    Design,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use indicatrix_editor::sweep::{
    SweepError, SweepMetric, SweepOptions, SweepOutcome, SweepRange, SweepRow, SweepScene,
    best_flags, default_worker_count, plan_sweep, sweep_csv, sweep_tier_angle,
};
use serde_json::{Map, Value, json};
use std::sync::atomic::AtomicBool;

/// The decimals of a figure in JSON, as in the CSV.
const FIGURE_DECIMALS: i32 = 4;

/// The position of the tier `text` names: its name (exact first, then without regard to
/// letter case), or `#N` for the Nth row of the tier table, counted from 1.
///
/// # Errors
///
/// [`CliError::usage`] when no tier, or more than one, answers to `text`.
fn resolve_tier(design: &Design, text: &str) -> Result<usize, CliError> {
    let text = text.trim();
    if let Some(number) = text.strip_prefix('#') {
        let count = design.tiers.len();
        return match number.trim().parse::<usize>() {
            Ok(row) if (1..=count).contains(&row) => Ok(row - 1),
            Ok(_) => Err(CliError::usage(format!(
                "--tier {text}: the design has {}, so a row is #1 to #{count}",
                count_noun(count, "tier", "tiers")
            ))),
            Err(_) => Err(CliError::usage(format!(
                "--tier {text:?}: a row is written #N, with N a whole number from 1"
            ))),
        };
    }
    design
        .tier_position_by_name(text)
        .map_err(|error| CliError::usage(format!("--tier {text:?}: {error}")))
}

/// A sweep refusal as an error: a tier that cannot be swept is a problem with the design (exit
/// 2), a range that cannot be used a problem with the command line (exit 1).
fn sweep_error(error: &SweepError) -> CliError {
    match error {
        SweepError::NoSuchTier { .. }
        | SweepError::Driven { .. }
        | SweepError::Flat { .. }
        | SweepError::Vertical { .. } => CliError::design(error.to_string()),
        SweepError::BadNumber(_)
        | SweepError::StepNotPositive
        | SweepError::OutOfBounds { .. }
        | SweepError::TooManySteps { .. } => CliError::usage(error.to_string()),
    }
}

/// The figures a sweep shows: the quick ones, and the tilt averages when it measured them.
fn shown_metrics(tilt: bool) -> Vec<SweepMetric> {
    SweepMetric::ALL
        .into_iter()
        .filter(|metric| tilt || !metric.is_tilt())
        .collect()
}

/// The count of facet warnings of a row, or `-` for a row that is not a stone.
fn warning_cell(row: &SweepRow) -> String {
    row.metrics.map_or_else(
        || "-".to_string(),
        |figures| figures.warning_count.to_string(),
    )
}

/// The rows as a table.
fn rows_table(outcome: &SweepOutcome, metrics: &[SweepMetric]) -> String {
    let mut headers = vec!["Angle", "Now"];
    headers.extend(metrics.iter().copied().map(SweepMetric::column_title));
    headers.extend(["Warn", "Notes"]);
    let mut aligns = vec![Align::Right, Align::Left];
    aligns.extend(metrics.iter().map(|_| Align::Right));
    aligns.extend([Align::Right, Align::Left]);
    let rows: Vec<Vec<String>> = outcome
        .rows
        .iter()
        .map(|row| {
            let current = if row.is_current { "*" } else { "" };
            let mut cells = vec![angle_text(row.angle_deg), current.to_string()];
            cells.extend(metrics.iter().map(|metric| metric.cell_text(row)));
            cells.push(warning_cell(row));
            cells.push(row.notes_text());
            cells
        })
        .collect();
    table(&headers, &aligns, &rows)
}

/// An angle as the text report shows it: the magnitude, because the report is read by a
/// person and the side of the girdle comes from the tier.
fn angle_text(angle_deg: f64) -> String {
    fixed(angle_deg.abs(), ANGLE_DECIMALS)
}

/// For each figure, the angles that are the best of the sweep (see `best_flags`).
fn best_angles(outcome: &SweepOutcome, metrics: &[SweepMetric]) -> Vec<(SweepMetric, Vec<f64>)> {
    let flags = best_flags(&outcome.rows);
    metrics
        .iter()
        .map(|&metric| {
            let angles: Vec<f64> = outcome
                .rows
                .iter()
                .zip(&flags)
                .filter(|(_, flag)| flag.get(metric))
                .map(|(row, _)| row.angle_deg)
                .collect();
            (metric, angles)
        })
        .filter(|(_, angles)| !angles.is_empty())
        .collect()
}

/// What a report says about the run, besides the rows.
struct Header<'a> {
    loaded: &'a Loaded,
    material: &'a str,
    lighting: &'static str,
    notes: &'a [String],
}

/// The text report.
fn text_report(header: &Header<'_>, outcome: &SweepOutcome, metrics: &[SweepMetric]) -> String {
    let swept = format!(
        "{} (row {}), now {} degrees",
        outcome.tier_name,
        outcome.tier + 1,
        angle_text(outcome.current_deg)
    );
    let mut text = key_values(&[
        (
            "Design",
            format!("{} ({})", header.loaded.name, header.loaded.file_name),
        ),
        ("Tier", swept),
        ("Material", header.material.to_string()),
        ("Lighting", header.lighting.to_string()),
        (
            "Angles",
            format!(
                "{} tried, {} give a stone",
                outcome.requested,
                outcome.valid_count()
            ),
        ),
    ]);
    text.push('\n');
    text.push_str(&rows_table(outcome, metrics));
    let best: Vec<String> = best_angles(outcome, metrics)
        .into_iter()
        .map(|(metric, angles)| {
            let angles: Vec<String> = angles.into_iter().map(angle_text).collect();
            format!("{}: {}", metric.label(), angles.join(", "))
        })
        .collect();
    for block in [list_block("Best", &best), list_block("Notes", header.notes)] {
        if !block.is_empty() {
            text.push('\n');
            text.push_str(&block);
        }
    }
    text
}

/// One row as a JSON object.
fn row_json(row: &SweepRow, metrics: &[SweepMetric]) -> Value {
    let mut figures = Map::new();
    for metric in metrics {
        figures.insert(
            metric.csv_name().to_string(),
            json_optional(metric.value(row), FIGURE_DECIMALS),
        );
    }
    figures.insert(
        "facet_warnings".to_string(),
        row.metrics
            .map_or(Value::Null, |found| Value::from(found.warning_count)),
    );
    json!({
        "angle_deg": json_optional(Some(row.angle_deg), FIGURE_DECIMALS),
        "current": row.is_current,
        "valid": row.is_valid(),
        "figures": figures,
        "notes": row.notes,
    })
}

/// The JSON report.
fn json_report(header: &Header<'_>, outcome: &SweepOutcome, metrics: &[SweepMetric]) -> String {
    let rows: Vec<Value> = outcome
        .rows
        .iter()
        .map(|row| row_json(row, metrics))
        .collect();
    let mut best = Map::new();
    for (metric, angles) in best_angles(outcome, metrics) {
        let angles: Vec<Value> = angles
            .into_iter()
            .map(|angle| json_optional(Some(angle), FIGURE_DECIMALS))
            .collect();
        best.insert(metric.csv_name().to_string(), Value::Array(angles));
    }
    json_text(&json!({
        "design": header.loaded.name,
        "file": header.loaded.file_name,
        "tier": outcome.tier_name,
        "row": outcome.tier + 1,
        "current_deg": json_optional(Some(outcome.current_deg), FIGURE_DECIMALS),
        "material": header.material,
        "lighting": header.lighting,
        "tilt_average": outcome.tilt_average,
        "requested": outcome.requested,
        "rows": rows,
        "best": best,
        "notes": header.notes,
    }))
}

/// Runs `sweep` on an opened design.
///
/// # Errors
///
/// [`CliError`] when the material or the tier is unknown, the range cannot be swept, or the
/// design is not a usable stone.
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &SweepArgs) -> CommandResult {
    let design = &loaded.design;
    let scoring = resolve_scoring(design, catalogue, args.material.as_ref())?;
    let tier = resolve_tier(design, &args.tier)?;
    let range = SweepRange {
        from_deg: args.from_deg,
        to_deg: args.to_deg,
        step_deg: args.step_deg,
    };
    let plan = plan_sweep(design, tier, range).map_err(|error| sweep_error(&error))?;
    analyze(design).require_stone(design)?;
    let scene = SweepScene {
        material: &scoring.resolved.gem,
        environment: args
            .lighting
            .studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH),
    };
    let options = SweepOptions {
        tilt_average: args.tilt,
        workers: default_worker_count(),
    };
    let outcome = sweep_tier_angle(
        design,
        &plan,
        &scene,
        &options,
        &AtomicBool::new(false),
        &|_, _| {},
    );
    let mut notes = loaded.notes.clone();
    notes.extend(scoring.note.clone());
    let header = Header {
        loaded,
        material: &scoring.label,
        lighting: lighting_name(args.lighting),
        notes: &notes,
    };
    let metrics = shown_metrics(args.tilt);
    let report = if args.json {
        json_report(&header, &outcome, &metrics)
    } else {
        text_report(&header, &outcome, &metrics)
    };
    let mut result = Outcome::text(report);
    if let Some(path) = &args.csv {
        result = result.with_file(path.clone(), sweep_csv(&outcome));
    }
    Ok(result)
}

/// Runs `sweep`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, the material or the tier is unknown, the
/// range cannot be swept, or the design is not a usable stone.
pub fn run(args: &SweepArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        outcome::{EXIT_DESIGN, EXIT_OK, EXIT_USAGE},
        testing,
    };
    use indicatrix_cut_core::CANONICAL_LIGHTING_PRESET;
    use std::path::PathBuf;

    /// The tier the tests sweep, and its angle in `design`.
    fn pavilion(design: &Design) -> (usize, f64) {
        let tier = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        (tier, design.tiers[tier].angle_deg)
    }

    fn args(design: &Design, half_range: f64, step: f64) -> SweepArgs {
        let (_, angle) = pavilion(design);
        SweepArgs {
            design: PathBuf::new(),
            tier: "Pavilion Main".to_string(),
            from_deg: angle - half_range,
            to_deg: angle + half_range,
            step_deg: step,
            tilt: false,
            csv: None,
            json: false,
            material: None,
            lighting: CANONICAL_LIGHTING_PRESET,
            db: None,
        }
    }

    fn loaded() -> Loaded {
        Loaded::in_memory(testing::brilliant_at_172(), "brilliant.indicatrix")
    }

    #[test]
    fn a_tier_is_found_by_name_by_name_in_any_case_and_by_row() {
        let design = testing::brilliant_at_172();
        let (tier, _) = pavilion(&design);
        assert_eq!(resolve_tier(&design, "Pavilion Main").expect("exact"), tier);
        assert_eq!(
            resolve_tier(&design, "pavilion main").expect("any case"),
            tier
        );
        assert_eq!(
            resolve_tier(&design, &format!("#{}", tier + 1)).expect("row"),
            tier
        );
        assert_eq!(resolve_tier(&design, "#1").expect("first row"), 0);
    }

    #[test]
    fn a_tier_that_is_not_there_is_a_usage_error() {
        let design = testing::brilliant_at_172();
        for text in ["Nonesuch", "#0", "#9999", "#x", ""] {
            let error = resolve_tier(&design, text).expect_err(text);
            assert_eq!(error.code, EXIT_USAGE, "{text}: {}", error.message);
            assert!(error.message.contains("--tier"), "{}", error.message);
        }
    }

    #[test]
    fn a_tier_that_cannot_be_swept_is_a_design_error_and_a_bad_range_a_usage_error() {
        let design = testing::brilliant_at_172();
        let flat = SweepError::Flat {
            tier: "Table".to_string(),
        };
        assert_eq!(sweep_error(&flat).code, EXIT_DESIGN);
        assert_eq!(sweep_error(&SweepError::StepNotPositive).code, EXIT_USAGE);
        assert_eq!(
            sweep_error(&SweepError::TooManySteps {
                steps: 500,
                max: 200
            })
            .code,
            EXIT_USAGE
        );
        // The table is flat, so sweeping it is refused by the engine itself.
        let mut table = args(&design, 1.0, 1.0);
        table.tier = "#1".to_string();
        let error = execute(&loaded(), &Catalogue::default(), &table).expect_err("a flat tier");
        assert_eq!(error.code, EXIT_DESIGN, "{}", error.message);
    }

    #[test]
    fn the_sweep_has_a_row_per_angle_and_marks_the_current_one() {
        let design = testing::brilliant_at_172();
        let (_, angle) = pavilion(&design);
        let result =
            execute(&loaded(), &Catalogue::default(), &args(&design, 1.0, 1.0)).expect("sweeps");
        assert_eq!(result.exit, EXIT_OK);
        let text = result.stdout;
        assert!(text.contains("Tier:      Pavilion Main (row"), "{text}");
        assert!(text.contains("3 tried,"), "{text}");
        // The tier stores a negative angle; the report reads positive numbers.
        assert!(angle < 0.0, "the standard pavilion is stored negative");
        for shown in [angle - 1.0, angle, angle + 1.0] {
            assert!(text.contains(&angle_text(shown)), "{text}");
            assert!(!text.contains(&fixed(shown, ANGLE_DECIMALS)), "{text}");
        }
        assert!(
            text.contains(&format!("now {} degrees", angle_text(angle))),
            "{text}"
        );
        let marked = text.lines().filter(|line| line.contains(" * ")).count();
        assert_eq!(marked, 1, "{text}");
        assert!(text.contains("Brilliance %"), "{text}");
        assert!(!text.contains("Tilt brill."), "{text}");
    }

    #[test]
    fn the_json_sweep_lists_every_row_flattest_first_with_the_stored_signed_angles() {
        let design = testing::brilliant_at_172();
        let mut sweep_args = args(&design, 1.0, 1.0);
        sweep_args.json = true;
        let result = execute(&loaded(), &Catalogue::default(), &sweep_args).expect("sweeps");
        let value: Value = serde_json::from_str(&result.stdout).expect("valid JSON");
        let rows = value["rows"].as_array().expect("rows");
        assert!(rows.len() >= 3, "{}", result.stdout);
        let angles: Vec<f64> = rows
            .iter()
            .map(|row| row["angle_deg"].as_f64().expect("an angle"))
            .collect();
        assert!(
            angles.windows(2).all(|pair| pair[0].abs() < pair[1].abs()),
            "{angles:?}"
        );
        // JSON keeps the stored convention: a pavilion tier's angles are negative.
        assert!(angles.iter().all(|angle| *angle < 0.0), "{angles:?}");
        assert!(value["current_deg"].as_f64().is_some_and(|a| a < 0.0));
        assert_eq!(rows.iter().filter(|row| row["current"] == true).count(), 1);
        assert!(rows[0]["figures"]["brilliance_pct"].is_number() || rows[0]["valid"] == false);
        assert!(rows[0]["figures"].get("tilt_brilliance_pct").is_none());
        assert_eq!(value["tilt_average"], false);
        assert_eq!(value["tier"], "Pavilion Main");
    }

    #[test]
    fn the_report_is_the_same_every_time() {
        let design = testing::brilliant_at_172();
        let first =
            execute(&loaded(), &Catalogue::default(), &args(&design, 1.0, 1.0)).expect("sweeps");
        let second =
            execute(&loaded(), &Catalogue::default(), &args(&design, 1.0, 1.0)).expect("sweeps");
        assert_eq!(first.stdout, second.stdout);
    }

    #[test]
    fn end_to_end_sweep_writes_the_csv_the_dialog_writes() {
        let design = testing::brilliant_at_172();
        let (_, angle) = pavilion(&design);
        let input = testing::write_design("sweep-e2e.indicatrix", &design);
        let target = testing::temp_path("sweep-e2e.csv")
            .to_string_lossy()
            .into_owned();
        let from = fixed(angle - 1.0, 1);
        let to = fixed(angle + 1.0, 1);
        let outcome = testing::run(&[
            "sweep",
            &input,
            "--tier",
            "Pavilion Main",
            "--from",
            &from,
            "--to",
            &to,
            "--step",
            "1",
            "--csv",
            &target,
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let csv = testing::file_text(&outcome, &target).expect("the CSV is written");
        assert!(
            csv.starts_with("tier,angle_deg,current,valid,brilliance_pct,"),
            "{csv}"
        );
        assert!(csv.lines().count() >= 4, "{csv}");
        // The file shows positive angles, flattest first.
        let first_row = csv.lines().nth(1).expect("a first row");
        assert!(
            first_row.starts_with(&format!("Pavilion Main,{:.4},", angle.abs() - 1.0)),
            "{csv}"
        );
        assert!(outcome.stdout.contains("Angle"), "{}", outcome.stdout);
    }

    #[test]
    fn end_to_end_a_signed_and_an_unsigned_range_make_the_same_report() {
        let design = testing::brilliant_at_172();
        let (_, angle) = pavilion(&design);
        let input = testing::write_design("sweep-e2e-sign.indicatrix", &design);
        let low = fixed(angle.abs() - 1.0, 1);
        let high = fixed(angle.abs() + 1.0, 1);
        let run_with = |from: &str, to: &str| {
            let outcome = testing::run(&[
                "sweep",
                &input,
                "--tier",
                "Pavilion Main",
                "--from",
                from,
                "--to",
                to,
                "--step",
                "1",
            ]);
            assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
            outcome.stdout
        };
        // The pavilion angles are typed as positive numbers; a sign in front is ignored.
        let unsigned = run_with(&low, &high);
        assert_eq!(unsigned, run_with(&format!("-{low}"), &format!("-{high}")));
        assert_eq!(unsigned, run_with(&high, &low));
        assert!(unsigned.contains(&angle_text(angle - 1.0)), "{unsigned}");
    }

    #[test]
    fn end_to_end_an_angle_out_of_range_is_refused_before_any_work() {
        let design = testing::brilliant_at_172();
        let input = testing::write_design("sweep-e2e-bounds.indicatrix", &design);
        let outcome = testing::run(&[
            "sweep",
            &input,
            "--tier",
            "Pavilion Main",
            "--from",
            "95",
            "--to",
            "42",
            "--step",
            "1",
        ]);
        assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
        assert!(
            outcome.stderr.contains("outside what a sweep takes"),
            "{}",
            outcome.stderr
        );
    }
}

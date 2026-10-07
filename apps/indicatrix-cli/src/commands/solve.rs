//! `solve`: solve the design, say whether it closes, list the warnings, optionally save it.

use super::{key_values, list_block, open};
use crate::{
    args::SolveArgs,
    format::{count_noun, json_text},
    load::Loaded,
    outcome::{CliError, CommandResult, EXIT_DESIGN, Outcome},
    stone::{Analysis, analyze},
};
use serde_json::json;

/// The sentence describing what the solver produced.
fn solve_line(analysis: &Analysis, tiers: usize) -> String {
    match &analysis.solved {
        Some(_) if analysis.failure.is_none() || analysis.is_closed() => {
            format!("solved ({})", count_noun(tiers, "tier", "tiers"))
        }
        Some(_) => format!(
            "solved ({}), with a problem",
            count_noun(tiers, "tier", "tiers")
        ),
        None => "failed".to_string(),
    }
}

/// The sentence describing whether the facets enclose a stone.
fn closure_line(analysis: &Analysis) -> String {
    analysis.failure.as_ref().map_or_else(
        || analysis.closure.word().to_string(),
        |why| format!("{}: {why}", analysis.closure.word()),
    )
}

/// The report as text.
fn text_report(loaded: &Loaded, analysis: &Analysis) -> String {
    let tiers = loaded.design.tiers.len();
    let mut text = key_values(&[
        ("Design", format!("{} ({})", loaded.name, loaded.file_name)),
        ("Tiers", tiers.to_string()),
        ("Solve", solve_line(analysis, tiers)),
        ("Closure", closure_line(analysis)),
    ]);
    let warnings: Vec<String> = analysis.warnings.iter().map(ToString::to_string).collect();
    text.push_str(&list_block("Warnings", &warnings));
    text.push_str(&list_block("Notes", &loaded.notes));
    text
}

/// The report as JSON.
fn json_report(
    loaded: &Loaded,
    analysis: &Analysis,
    problem: Option<&str>,
    written: Option<&str>,
) -> String {
    let warnings: Vec<String> = analysis.warnings.iter().map(ToString::to_string).collect();
    json_text(&json!({
        "name": loaded.name,
        "file": loaded.file_name,
        "tiers": loaded.design.tiers.len(),
        "solved": analysis.solved.is_some(),
        "closure": analysis.closure.word(),
        "problem": problem,
        "warnings": warnings,
        "notes": loaded.notes,
        "written": written,
    }))
}

/// Runs `solve`.
///
/// The design is solved and its stone checked. A design that does not solve and close is
/// reported (the report is the stdout text) and ends with exit code 2, and nothing is written.
/// Otherwise `--out` saves the design as a `.indicatrix` file, which is also how an `.asc`,
/// `.gem` or `.gcs` file is converted.
///
/// # Errors
///
/// [`CliError`] when the design or the library cannot be opened, or the design cannot be written.
pub fn run(args: &SolveArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    let analysis = analyze(&loaded.design);
    let problem = analysis.problem(&loaded.design);
    if let Some(problem) = problem {
        let report = if args.json {
            json_report(&loaded, &analysis, Some(problem.as_str()), None)
        } else {
            text_report(&loaded, &analysis)
        };
        let error = CliError::design(format!("the design is not a usable stone: {problem}"));
        return Ok(Outcome {
            stdout: report,
            stderr: format!("error: {}\n", error.message),
            exit: EXIT_DESIGN,
            files: Vec::new(),
        });
    }
    let (mut outcome, written) = if let Some(path) = &args.out {
        let text = loaded.indicatrix_text(&loaded.design, &catalogue)?;
        (
            Outcome::default().with_file(path.clone(), text),
            Some(path.to_string_lossy().into_owned()),
        )
    } else {
        (Outcome::default(), None)
    };
    outcome.stdout = if args.json {
        json_report(&loaded, &analysis, None, written.as_deref())
    } else {
        text_report(&loaded, &analysis)
    };
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{outcome::EXIT_OK, testing};
    use indicatrix_cut_core::{Design, PreformSpec, ScheduleMeta};
    use serde_json::Value;

    #[test]
    fn the_text_report_says_solved_and_closed() {
        let loaded = Loaded::in_memory(testing::template(), "round.indicatrix");
        let analysis = analyze(&loaded.design);
        let report = text_report(&loaded, &analysis);
        assert!(report.contains("Solve:    solved ("), "{report}");
        assert!(report.contains("Closure:  closed\n"), "{report}");
        assert!(
            report.contains(&format!("Tiers:    {}\n", loaded.design.tiers.len())),
            "{report}"
        );
    }

    #[test]
    fn the_json_report_carries_the_problem_and_the_file() {
        let loaded = Loaded::in_memory(testing::template(), "round.indicatrix");
        let analysis = analyze(&loaded.design);
        let text = json_report(&loaded, &analysis, None, Some("out.indicatrix"));
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["solved"], true);
        assert_eq!(value["closure"], "closed");
        assert_eq!(value["problem"], Value::Null);
        assert_eq!(value["written"], "out.indicatrix");
        assert!(value["warnings"].is_array());
    }

    #[test]
    fn end_to_end_solve_saves_a_file_that_opens_again() {
        let input = testing::write_design("solve-e2e.indicatrix", &testing::template());
        let target = testing::temp_path("solve-e2e-saved.indicatrix")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["solve", &input, "--out", &target]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(
            outcome.stdout.contains("Closure:  closed"),
            "{}",
            outcome.stdout
        );
        let saved = testing::file_text(&outcome, &target).expect("the design is saved");
        let again = crate::load::from_bytes("saved.indicatrix", saved.as_bytes()).expect("opens");
        assert_eq!(
            testing::shape(&again.design),
            testing::shape(&testing::template())
        );
    }

    #[test]
    fn end_to_end_solve_without_out_writes_nothing() {
        let input = testing::write_design("solve-e2e-plain.indicatrix", &testing::template());
        let outcome = testing::run(&["solve", &input, "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 0);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["closure"], "closed");
        assert_eq!(value["written"], Value::Null);
    }

    #[test]
    fn a_design_without_facets_is_refused_and_not_written() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let input = testing::write_design("solve-e2e-empty.indicatrix", &design);
        let target = testing::temp_path("solve-e2e-empty-out.indicatrix")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["solve", &input, "--out", &target]);
        assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 0);
        assert!(outcome.stderr.starts_with("error:"), "{}", outcome.stderr);
    }

    #[test]
    fn the_core_reports_a_design_without_facets_as_data() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let loaded = Loaded::in_memory(design, "empty.indicatrix");
        let analysis = analyze(&loaded.design);
        let report = text_report(&loaded, &analysis);
        assert!(report.contains("Tiers:"), "{report}");
        assert!(analysis.problem(&loaded.design).is_some());
    }
}

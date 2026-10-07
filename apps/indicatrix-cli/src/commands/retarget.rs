//! `retarget`: new facet angles for a different material, with the editor's engine and gate.
//!
//! This is the Retarget dialog without the window. `--mode shift` (the default) moves the
//! crown and pavilion angles by the critical-angle shift (`build_plan`), re-anchors every
//! pinned facet about its girdle-side edge and refits the table and culet heights
//! (`check_retarget`), and judges the result with the validity gate. `--mode optimize` starts
//! from that result and searches every crown and pavilion angle inside `--range` degrees
//! (`run_search`), each result going through the same gate. The angles, the re-anchored masts
//! and the new material are applied as ONE editor edit, so tiers that follow a relation follow.
//!
//! # The gate is stricter than the dialog's
//!
//! The dialog lets a change through when the verdict is Valid, and also when it is Unchecked (the
//! current design itself could not be analysed, so nothing could be compared). A command line
//! has nobody to look at the stone, so `retarget` accepts Valid only: Invalid and Unchecked are
//! refused with exit code 2, the reasons are printed, and nothing is written.
//!
//! The report is always printed (what was asked, the verdict, and for an accepted change the
//! tiers that moved and the optical figures). `--out FILE` saves the retargeted design as a
//! `.indicatrix` file with the new material; without it nothing is written, which makes the
//! command a dry run.

mod report;

use super::open;
use crate::{
    args::{Mode, RetargetArgs},
    load::Loaded,
    materials::{Catalogue, current_gem, resolve_scoring},
    outcome::{CliError, CommandResult, EXIT_DESIGN, Outcome},
    stone::analyze,
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, Edit, History, MaterialSelection, ObjectivePreset};
use indicatrix_editor::{
    EditorSession,
    retarget::{
        RetargetPlan, apply_with_anchors, build_plan,
        check::{CheckInputs, check_retarget},
        metrics::RetargetMetrics,
        search::{SearchInputs, SearchSettings, run_search},
        validity::{RetargetValidity, ValidityStatus},
    },
};
use report::{Report, json_report, text_report};
use std::{fmt::Write as _, sync::atomic::AtomicBool};

/// What the search of `--mode optimize` did.
struct SearchFacts {
    preset: ObjectivePreset,
    range_deg: f64,
    budget: usize,
    seed: u64,
    keep_look: bool,
    evaluations: usize,
    free_tiers: usize,
    options: usize,
}

/// What the engine decided: the edit to make, or why it will not be made.
struct Decision {
    change: Result<Edit, String>,
    validity: RetargetValidity,
    metrics: RetargetMetrics,
    anchors: usize,
    search: Option<SearchFacts>,
    notes: Vec<String>,
}

/// `Ok` when the gate says the retargeted stone is valid; otherwise the refusal text: the
/// headline and every further reason.
///
/// Stricter than the dialog, which also lets an Unchecked change through (see the module
/// documentation).
fn gate(validity: &RetargetValidity) -> Result<(), String> {
    match validity.status {
        ValidityStatus::Valid => Ok(()),
        ValidityStatus::Invalid | ValidityStatus::Unchecked => {
            let mut message = validity.headline();
            for line in validity.detail_lines() {
                message.push_str("\n  - ");
                message.push_str(&line);
            }
            Err(message)
        }
    }
}

/// The Shift decision: the plan, checked.
fn shift_decision(
    design: &Design,
    plan: &RetargetPlan,
    current: Option<&GemMaterial>,
    args: &RetargetArgs,
) -> Decision {
    let check = check_retarget(&CheckInputs {
        girdle: None,
        design,
        plan,
        current_gem: current,
        lighting: args.lighting,
    });
    let change = gate(&check.validity)
        .map(|()| apply_with_anchors(design, &plan.proposal(), &check.anchors));
    Decision {
        change,
        validity: check.validity,
        metrics: check.metrics,
        anchors: check.anchors.len(),
        search: None,
        notes: plan.notes.clone(),
    }
}

/// The Optimize decision: the search, and its best option.
///
/// # Errors
///
/// [`CliError::design`] when the search cannot run (the design does not solve or close).
fn search_decision(
    design: &Design,
    plan: &RetargetPlan,
    current: Option<&GemMaterial>,
    args: &RetargetArgs,
) -> Result<Decision, CliError> {
    let defaults = SearchSettings::default();
    let settings = SearchSettings {
        preset: args.preset,
        range_deg: args.range_deg.unwrap_or(defaults.range_deg),
        evaluations: args.budget.unwrap_or(defaults.evaluations),
        seed: args.seed,
        keep: defaults.keep,
        keep_look: args.keep_look,
        // The CLI keeps the band as it is until its own flag lands (CLI lane).
        girdle: None,
    };
    let inputs = SearchInputs {
        design,
        plan,
        current_gem: current,
        lighting: args.lighting,
        settings: &settings,
    };
    let report = run_search(&inputs, &AtomicBool::new(false), &|_, _| {})
        .map_err(|error| CliError::design(error.to_string()))?;
    let mut notes = plan.notes.clone();
    notes.extend(report.notes());
    let facts = SearchFacts {
        preset: settings.preset,
        range_deg: settings.range_deg,
        budget: settings.evaluations,
        seed: settings.seed,
        keep_look: settings.keep_look,
        evaluations: report.evaluations,
        free_tiers: report.free_tiers,
        options: report.candidates.len(),
    };
    Ok(match report.best() {
        Some(best) => {
            let proposal = plan.with_angles(design, &best.angles).proposal();
            Decision {
                change: gate(&best.validity)
                    .map(|()| apply_with_anchors(design, &proposal, &best.anchors)),
                validity: best.validity.clone(),
                metrics: best.metrics,
                anchors: best.anchors.len(),
                search: Some(facts),
                notes,
            }
        }
        None => Decision {
            change: Err(gate(&report.shift_validity)
                .err()
                .unwrap_or_else(|| "the search found no valid option".to_string())),
            validity: report.shift_validity.clone(),
            metrics: RetargetMetrics::default(),
            anchors: 0,
            search: Some(facts),
            notes,
        },
    })
}

/// `edit` and the new material applied to a copy of `design` as one editor edit, checked to be
/// a closed stone.
///
/// # Errors
///
/// The refusal text when the session does not take the edit or the result is not a stone.
fn apply(design: &Design, edit: Edit, selection: &MaterialSelection) -> Result<Design, String> {
    let mut edits = match edit {
        Edit::Batch(edits) => edits,
        single => vec![single],
    };
    if *selection != design.material {
        edits.push(Edit::SetMaterial {
            material: selection.clone(),
        });
    }
    let mut session = EditorSession::with_history(design.clone(), History::new());
    session
        .try_apply(Edit::Batch(edits))
        .map_err(|error| format!("the change could not be applied: {error}"))?;
    if let Some(problem) = analyze(&session.design).problem(&session.design) {
        return Err(format!(
            "the retargeted design is not a usable stone: {problem}"
        ));
    }
    Ok(session.design)
}

/// Runs `retarget` on an opened design.
///
/// # Errors
///
/// [`CliError`] when the target material is unknown, the design is not a usable stone, or no
/// angle can move. A change the gate refuses is not an error value: it is an outcome with exit
/// code 2, the report on standard output, the reasons on standard error and no file.
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &RetargetArgs) -> CommandResult {
    let design = &loaded.design;
    let target = resolve_scoring(design, catalogue, Some(&args.target))?;
    analyze(design).require_stone(design)?;
    let plan = build_plan(design, &target.resolved, args.crown, catalogue.custom());
    if plan.is_empty() {
        return Err(CliError::design(
            "none of the facet angles can move: every crown and pavilion tier is flat or follows \
             a relation",
        ));
    }
    let current = current_gem(design, catalogue);
    let decision = match args.mode {
        Mode::Shift => shift_decision(design, &plan, current.as_ref(), args),
        Mode::Optimize => search_decision(design, &plan, current.as_ref(), args)?,
    };
    let result = match &decision.change {
        Ok(edit) => apply(design, edit.clone(), &target.selection),
        Err(refusal) => Err(refusal.clone()),
    };
    let mut notes = loaded.notes.clone();
    notes.extend(target.note.clone());
    notes.extend(decision.notes.clone());
    let saved = match (&result, &args.out) {
        (Ok(after), Some(path)) => Some((path.clone(), loaded.indicatrix_text(after, catalogue)?)),
        _ => None,
    };
    let report = Report {
        loaded,
        args,
        plan: &plan,
        target: &target,
        decision: &decision,
        after: result.as_ref().ok(),
        written: saved
            .as_ref()
            .map(|(path, _)| path.to_string_lossy().into_owned()),
        notes,
    };
    let text = if args.json {
        json_report(&report)
    } else {
        text_report(&report)
    };
    let mut outcome = Outcome::text(text);
    if let Err(refusal) = &result {
        outcome = outcome.with_exit(EXIT_DESIGN);
        let _ = writeln!(outcome.stderr, "error: the retarget was refused: {refusal}");
    }
    if let Some((path, text)) = saved {
        outcome = outcome.with_file(path, text);
    }
    Ok(outcome)
}

/// Runs `retarget`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, the target material is unknown, the design is
/// not a usable stone, or no angle can move. A refused change is an outcome, not an error: see
/// the module documentation.
pub fn run(args: &RetargetArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        args::MaterialArg,
        load::from_bytes,
        outcome::{EXIT_OK, EXIT_USAGE},
        testing,
    };
    use indicatrix_cut_core::CANONICAL_LIGHTING_PRESET;
    use indicatrix_editor::retarget::{
        CrownShift,
        validity::{InvalidReason, ValidityFigures},
    };
    use serde_json::Value;
    use std::path::PathBuf;

    /// The owner's sapphire case: crown a third of the shift.
    const SAPPHIRE_CROWN: CrownShift = CrownShift {
        fraction: 0.33,
        scale_by_ratio: false,
        follow_pavilion: false,
    };

    fn args(target: MaterialArg, crown: CrownShift, out: Option<&str>) -> RetargetArgs {
        RetargetArgs {
            design: PathBuf::new(),
            target,
            mode: Mode::Shift,
            crown,
            preset: ObjectivePreset::Balanced,
            range_deg: None,
            budget: None,
            seed: 0,
            keep_look: true,
            lighting: CANONICAL_LIGHTING_PRESET,
            json: false,
            out: out.map(PathBuf::from),
            db: None,
        }
    }

    fn loaded() -> Loaded {
        Loaded::in_memory(testing::brilliant_at_172(), "brilliant.indicatrix")
    }

    fn validity(status: ValidityStatus, reasons: Vec<InvalidReason>) -> RetargetValidity {
        RetargetValidity {
            status,
            reasons,
            warnings: vec!["a warning".to_string()],
            figures: ValidityFigures::default(),
            strategy: indicatrix_editor::retarget::validity::RetargetStrategy::AnglesOnly,
        }
    }

    #[test]
    fn the_gate_accepts_valid_and_refuses_invalid_and_unchecked() {
        assert!(gate(&validity(ValidityStatus::Valid, Vec::new())).is_ok());
        let invalid = gate(&validity(
            ValidityStatus::Invalid,
            vec![InvalidReason::NotClosed, InvalidReason::NotClosed],
        ))
        .expect_err("invalid is refused");
        assert!(invalid.starts_with("Not valid:"), "{invalid}");
        assert!(
            invalid.contains("\n  - "),
            "the further reasons are listed: {invalid}"
        );
        let unchecked = gate(&RetargetValidity::unchecked(&InvalidReason::NotClosed))
            .expect_err("unchecked is refused, unlike in the dialog");
        assert!(unchecked.starts_with("Could not check"), "{unchecked}");
    }

    #[test]
    fn a_sapphire_retarget_is_valid_and_applied() {
        let outcome = execute(
            &loaded(),
            &Catalogue::default(),
            &args(
                MaterialArg::Ri(1.7681),
                SAPPHIRE_CROWN,
                Some("sapphire.indicatrix"),
            ),
        )
        .expect("retargets");
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stdout);
        assert!(
            outcome.stdout.contains("Verdict:   Valid"),
            "{}",
            outcome.stdout
        );
        assert!(
            outcome.stdout.contains("Changed tiers ("),
            "{}",
            outcome.stdout
        );
        let saved = testing::file_text(&outcome, "sapphire.indicatrix").expect("saved");
        let again = from_bytes("sapphire.indicatrix", saved.as_bytes()).expect("opens");
        assert_eq!(
            again.design.material.refractive_index_override,
            Some(1.7681)
        );
        assert_ne!(
            testing::shape(&again.design),
            testing::shape(&testing::brilliant_at_172()),
            "the angles moved"
        );
        assert!(analyze(&again.design).is_closed());
    }

    #[test]
    fn the_same_retarget_gives_the_same_report() {
        let wanted = args(MaterialArg::Ri(1.7681), SAPPHIRE_CROWN, None);
        let first = execute(&loaded(), &Catalogue::default(), &wanted).expect("retargets");
        let second = execute(&loaded(), &Catalogue::default(), &wanted).expect("retargets");
        assert_eq!(first.stdout, second.stdout);
        assert!(first.files.is_empty(), "no --out, no file");
        assert!(
            first.stdout.contains("applied (not saved"),
            "{}",
            first.stdout
        );
    }

    #[test]
    fn a_refused_retarget_exits_2_writes_nothing_and_says_why() {
        // A refractive index far from the design's can leave the shift with no valid stone; whichever
        // way each one goes, the exit code and the files must agree with the gate.
        for ri in [1.45, 1.55, 1.9, 2.4] {
            let outcome = execute(
                &loaded(),
                &Catalogue::default(),
                &args(
                    MaterialArg::Ri(ri),
                    CrownShift::default(),
                    Some("refused.indicatrix"),
                ),
            )
            .expect("an outcome, not an error");
            let refused = outcome.exit == EXIT_DESIGN;
            assert_eq!(
                outcome.files.is_empty(),
                refused,
                "ri {ri}: {}",
                outcome.stdout
            );
            assert!(outcome.exit == EXIT_OK || refused, "ri {ri}");
            assert!(
                outcome.stdout.contains("Verdict:"),
                "ri {ri}: {}",
                outcome.stdout
            );
            if refused {
                assert!(
                    outcome.stderr.contains("the retarget was refused"),
                    "{}",
                    outcome.stderr
                );
                assert!(
                    outcome.stdout.contains("refused, nothing was written"),
                    "{}",
                    outcome.stdout
                );
            }
        }
    }

    #[test]
    fn a_default_retarget_follows_the_pavilion_and_reports_the_depth() {
        // The default crown policy follows the pavilion's stretch; a valid result quotes the
        // stone's depth in the verdict line. A refusal is tolerated exactly as in the test
        // above: the gate and the exit code must agree.
        let outcome = execute(
            &loaded(),
            &Catalogue::default(),
            &args(MaterialArg::Ri(1.7681), CrownShift::default(), None),
        )
        .expect("an outcome, not an error");
        let refused = outcome.exit == EXIT_DESIGN;
        assert!(outcome.exit == EXIT_OK || refused, "{}", outcome.stdout);
        if !refused {
            let verdict = outcome
                .stdout
                .lines()
                .find(|line| line.starts_with("Verdict:"))
                .unwrap_or_else(|| panic!("no verdict line in {}", outcome.stdout));
            assert!(verdict.contains("depth"), "{verdict}");
            assert!(
                outcome.stdout.contains("follows the pavilion's stretch"),
                "{}",
                outcome.stdout
            );
        }

        let mut wanted = args(MaterialArg::Ri(1.7681), CrownShift::default(), None);
        wanted.json = true;
        let outcome = execute(&loaded(), &Catalogue::default(), &wanted).expect("an outcome");
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["crown"]["follow_pavilion"], true);
    }

    #[test]
    fn the_json_report_carries_the_verdict_and_the_changes() {
        let mut wanted = args(MaterialArg::Ri(1.7681), SAPPHIRE_CROWN, None);
        wanted.json = true;
        let outcome = execute(&loaded(), &Catalogue::default(), &wanted).expect("retargets");
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["verdict"]["status"], "valid");
        assert_eq!(value["applied"], true);
        assert_eq!(value["mode"], "shift");
        assert_ne!(value["changes"].as_array().expect("changes").len(), 0);
        assert!(value["to"]["refractive_index"].is_number());
        assert!(value["search"].is_null());
    }

    #[test]
    fn an_unknown_target_material_is_a_usage_error_and_a_missing_stone_a_design_error() {
        let error = execute(
            &loaded(),
            &Catalogue::default(),
            &args(
                MaterialArg::Named("Unobtainium".to_string()),
                CrownShift::default(),
                None,
            ),
        )
        .expect_err("unknown");
        assert_eq!(error.code, EXIT_USAGE, "{}", error.message);
        let empty = Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 4.0),
            indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let error = execute(
            &Loaded::in_memory(empty, "empty.indicatrix"),
            &Catalogue::default(),
            &args(MaterialArg::Ri(1.7), CrownShift::default(), None),
        )
        .expect_err("no stone");
        assert_eq!(error.code, EXIT_DESIGN, "{}", error.message);
    }

    #[test]
    fn end_to_end_a_named_material_is_retargeted_or_refused_consistently() {
        let input = testing::write_design("retarget-e2e.indicatrix", &testing::brilliant_at_172());
        let target = testing::temp_path("retarget-e2e-out.indicatrix")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&[
            "retarget",
            &input,
            "--material",
            "Sapphire",
            "--crown-fraction",
            "0.33",
            "--out",
            &target,
        ]);
        let refused = outcome.exit == EXIT_DESIGN;
        assert!(
            outcome.exit == EXIT_OK || refused,
            "{} {}",
            outcome.exit,
            outcome.stderr
        );
        assert_eq!(outcome.files.is_empty(), refused);
        if !refused {
            let saved = testing::file_text(&outcome, &target).expect("saved");
            let again = from_bytes("out.indicatrix", saved.as_bytes()).expect("opens");
            assert_eq!(again.design.material.name.as_deref(), Some("Sapphire"));
        }
    }

    #[test]
    fn end_to_end_optimize_mode_follows_the_same_rule() {
        let input =
            testing::write_design("retarget-e2e-opt.indicatrix", &testing::brilliant_at_172());
        let target = testing::temp_path("retarget-e2e-opt-out.indicatrix")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&[
            "retarget",
            &input,
            "--ri",
            "1.7681",
            "--crown-fraction",
            "0.33",
            "--mode",
            "optimize",
            "--budget",
            "24",
            "--json",
            "--out",
            &target,
        ]);
        let refused = outcome.exit == EXIT_DESIGN;
        assert!(
            outcome.exit == EXIT_OK || refused,
            "{} {}",
            outcome.exit,
            outcome.stderr
        );
        assert_eq!(outcome.files.is_empty(), refused, "{}", outcome.stdout);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["mode"], "optimize");
        assert!(value["search"]["evaluations"].is_u64());
        assert_eq!(value["search"]["keep_look"], true);
        assert_eq!(value["applied"], !refused);
    }

    #[test]
    fn no_keep_look_reaches_the_search_and_the_report() {
        let input =
            testing::write_design("retarget-e2e-look.indicatrix", &testing::brilliant_at_172());
        let outcome = testing::run(&[
            "retarget",
            &input,
            "--ri",
            "1.7681",
            "--mode",
            "optimize",
            "--budget",
            "24",
            "--no-keep-look",
            "--json",
        ]);
        assert!(
            outcome.exit == EXIT_OK || outcome.exit == EXIT_DESIGN,
            "{} {}",
            outcome.exit,
            outcome.stderr
        );
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert_eq!(value["search"]["keep_look"], false);
    }
}

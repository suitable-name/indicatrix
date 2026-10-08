//! `optimize`: search the free tier angles of a design for a better stone, as the editor's
//! Optimize tab does.
//!
//! The run is the tab's own: `build_run_plan` turns the objective, the budget, the seed and
//! the number of candidates into the search settings, `optimize_design_with` runs the
//! deterministic coordinate search and the polish stage, and the ranked candidates are the
//! ones the tab lists. Like the tab, anchored (scale-reference) tiers are varied when nothing
//! else is free, so a freshly imported `.asc` can be optimized without `--vary-anchored`; the
//! flag turns it on for any design. "Keep the girdle" and "Polish" are on, as in the tab, and
//! the yield is not part of the score.
//!
//! The figures are measured the way the tab measures them: the search scores table-up under the
//! canonical light pose, and the start and every candidate are re-scored at full quality (the
//! tilt averages over four axes and 181 angles). A design with no material is scored with its
//! own refractive index as a flat material, and the report says so.
//!
//! `--out FILE` saves the BEST candidate as a `.indicatrix` file, applied through the same
//! editor session path as the tab's Apply (tiers that follow a relation follow). When the search
//! finds nothing better, nothing is written and the exit code is still 0: the report says so.

use super::{key_values, list_block, open};
use crate::{
    args::{OptimizeArgs, lighting_name, preset_name},
    format::{
        ANGLE_DECIMALS, Align, count_noun, fixed, json_number, json_number32, json_text, table,
    },
    load::Loaded,
    materials::{Catalogue, Scoring, resolve_scoring},
    outcome::{CliError, CommandResult, Outcome},
    stone::{Analysis, analyze},
};
use indicatrix::color::metrics::FaceUpTone;
use indicatrix_cut_core::{
    AngleChange, Design, History, ObjectiveComponents, ObjectivePreset, OptimizeCandidate,
    OptimizeResult, SearchHooks, ToneGoal, optimize_design_with,
};
use indicatrix_editor::{
    EditorSession,
    optimize_view::{
        CandidateLine, RunForm, RunPlan, apply_candidate, baseline_line, build_run_plan,
        candidate_lines, default_vary_anchored, measure_anchor_hinges, optimize_availability,
        optimize_run_status, tone_lighting_label,
    },
    retarget::plan::tier_display_names,
    solve_policy::design_to_gpu_planes_from_solved,
};
use serde_json::{Value, json};
use std::fmt::Write as _;

/// The decimals of a percentage or a score in JSON.
const FIGURE_DECIMALS: i32 = 2;

/// The decimals of an angle in JSON.
const JSON_ANGLE_DECIMALS: i32 = 4;

/// The run plan for `args` against `design`, with the hinges anchored variation needs.
///
/// # Errors
///
/// [`CliError::design`] when Optimize is not available for the design (nothing can move) or the
/// settings cannot be read.
fn plan_for(
    design: &Design,
    analysis: &Analysis,
    args: &OptimizeArgs,
) -> Result<(RunPlan, bool), CliError> {
    let vary_anchored = args.vary_anchored || default_vary_anchored(design);
    let (available, hint) = optimize_availability(design, args.budget, vary_anchored);
    if !available {
        return Err(CliError::design(format!(
            "there is nothing to optimize in this design: {hint}"
        )));
    }
    let budget = args.budget.to_string();
    let seed = args.seed.to_string();
    let form = RunForm {
        preset_index: ObjectivePreset::ALL
            .iter()
            .position(|preset| *preset == args.preset)
            .unwrap_or(0),
        weight_windowing: "",
        weight_extinction: "",
        weight_tilt_brilliance: "",
        yield_weight: 0.0,
        tone: 0.0,
        vary_anchored,
        keep_girdle: true,
        budget_text: &budget,
        starts: args.starts,
        seed_text: &seed,
        polish: true,
        candidates: args.candidates,
        ranges: &[],
    };
    let mut plan = build_run_plan(design, &form, args.lighting).map_err(CliError::design)?;
    if plan.options.vary_anchored {
        // The stone is solved already; the hinges are measured exactly as the desktop's
        // Optimize tab measures them (`prepare_anchor_hinges`, which solves for itself).
        let solved = analysis.require_stone(design)?;
        measure_anchor_hinges(&mut plan.options, design, solved, None);
    }
    Ok((plan, vary_anchored))
}

/// The best candidate applied to a copy of `loaded`'s design, through the editor session (so
/// the tiers that follow a relation follow), checked to still be a closed stone.
///
/// # Errors
///
/// [`CliError::design`] when the candidate no longer applies or the result is not a stone.
fn applied_design(loaded: &Loaded, best: &OptimizeCandidate) -> Result<Design, CliError> {
    let mut session = EditorSession::with_history(loaded.design.clone(), History::new());
    apply_candidate(&mut session, best).map_err(CliError::design)?;
    if let Some(problem) = analyze(&session.design).problem(&session.design) {
        return Err(CliError::design(format!(
            "the best candidate does not make a usable stone, so nothing was written: {problem}"
        )));
    }
    Ok(session.design)
}

/// `value` with an explicit sign.
fn signed(value: f64, decimals: usize) -> String {
    format!("{value:+.decimals$}")
}

/// The changed tiers of a candidate as table rows.
///
/// The text report is read by a person, so the tier is named the way the tier table names it
/// (an old-style `3` reads as its standard code) and both angles are magnitudes. The change is
/// the difference of those two numbers, with an explicit sign: `+0.5` is half a degree steeper
/// whichever side of the girdle the tier is on. JSON keeps the stored signed angles.
fn change_rows(design: &Design, changes: &[AngleChange]) -> Vec<Vec<String>> {
    let names = tier_display_names(design);
    changes
        .iter()
        .map(|change| {
            vec![
                (change.index + 1).to_string(),
                names.get(change.index).cloned().unwrap_or_default(),
                fixed(change.from_deg.abs(), ANGLE_DECIMALS),
                fixed(change.to_deg.abs(), ANGLE_DECIMALS),
                signed(change.to_deg.abs() - change.from_deg.abs(), ANGLE_DECIMALS),
            ]
        })
        .collect()
}

/// One row of the candidate table.
fn line_cells(line: &CandidateLine) -> Vec<String> {
    vec![
        line.rank.clone(),
        line.score.clone(),
        line.windowing.clone(),
        line.brilliance.clone(),
        line.extinction.clone(),
        line.yield_loss.clone(),
        line.tone.clone(),
        line.changed.clone(),
    ]
}

/// Everything the report prints.
struct Run<'a> {
    loaded: &'a Loaded,
    scoring: &'a Scoring,
    args: &'a OptimizeArgs,
    vary_anchored: bool,
    result: &'a OptimizeResult,
    status: String,
    notes: Vec<String>,
    written: Option<String>,
}

/// The "Search" line of the text report.
fn search_summary(run: &Run<'_>) -> String {
    let outcome = &run.result.outcome;
    format!(
        "budget {}, {} of {} requested, seed {}, {} evaluations ({} polishing), \
         light {}{}",
        run.args.budget,
        count_noun(run.result.starts_run, "start", "starts"),
        run.args.starts,
        run.args.seed,
        outcome.evaluations,
        outcome.polish_evaluations,
        lighting_name(run.args.lighting),
        if run.result.starts_run > 1 {
            format!(", best from start {}", run.result.best_start + 1)
        } else {
            String::new()
        }
    )
}

/// The text report.
fn text_report(run: &Run<'_>) -> String {
    let mut text = key_values(&[
        (
            "Design",
            format!("{} ({})", run.loaded.name, run.loaded.file_name),
        ),
        ("Material", run.scoring.label.clone()),
        ("Objective", preset_name(run.args.preset).to_string()),
        ("Search", search_summary(run)),
        (
            "Varied",
            if run.vary_anchored {
                "free tiers, and anchored tiers turned about their girdle edges"
            } else {
                "free tiers"
            }
            .to_string(),
        ),
        ("Result", run.status.clone()),
    ]);
    let mut lines = vec![baseline_line(run.result)];
    lines.extend(candidate_lines(run.result));
    let rows: Vec<Vec<String>> = lines.iter().map(line_cells).collect();
    text.push('\n');
    text.push_str(&table(
        &[
            "Rank",
            "Score",
            "Windowing",
            "Brilliance",
            "Extinction",
            "Yield loss",
            "Tone",
            "Changed",
        ],
        &[
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ],
        &rows,
    ));
    if run.result.tone_before.is_some() {
        let weighted = run.result.tone_goal.map_or_else(String::new, |goal| {
            format!(", weighted: the search favoured a {} stone", goal.label())
        });
        let _ = write!(
            text,
            "\nTone: the face-up lightness L* of the returned light, table-up, {}{weighted}.\n",
            tone_lighting_label(run.result.lighting),
        );
    }
    if let Some(best) = run.result.candidates.first() {
        let _ = write!(
            text,
            "\nBest candidate changes {}:\n",
            count_noun(best.changes.len(), "tier", "tiers")
        );
        text.push_str(&table(
            &["Row", "Tier", "From", "To", "Change"],
            &[
                Align::Right,
                Align::Left,
                Align::Right,
                Align::Right,
                Align::Right,
            ],
            &change_rows(&run.loaded.design, &best.changes),
        ));
    }
    if let Some(path) = &run.written {
        let _ = write!(text, "\nWrote the best candidate to {path}\n");
    }
    let notes = list_block("Notes", &run.notes);
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes);
    }
    text
}

/// A set of objective figures as a JSON object.
fn components_json(
    components: &ObjectiveComponents,
    score: f32,
    yield_loss_pct: f32,
) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    map.insert("score".to_string(), json_number32(score, FIGURE_DECIMALS));
    map.insert(
        "windowing_pct".to_string(),
        json_number32(components.windowing_pct, FIGURE_DECIMALS),
    );
    map.insert(
        "brilliance_pct".to_string(),
        json_number32(components.tilt_brilliance_pct, FIGURE_DECIMALS),
    );
    map.insert(
        "extinction_pct".to_string(),
        json_number32(components.extinction_pct, FIGURE_DECIMALS),
    );
    map.insert(
        "yield_loss_pct".to_string(),
        json_number32(yield_loss_pct, FIGURE_DECIMALS),
    );
    map
}

/// A face-up tone as a JSON object.
fn tone_json(tone: &FaceUpTone) -> Value {
    json!({
        "l_star": json_number32(tone.l_star, FIGURE_DECIMALS),
        "chroma": json_number32(tone.chroma, FIGURE_DECIMALS),
        "srgb": tone.srgb,
    })
}

/// One candidate as a JSON object.
fn candidate_json(design: &Design, rank: usize, candidate: &OptimizeCandidate) -> Value {
    let mut map = components_json(&candidate.after, candidate.score, candidate.yield_loss_pct);
    if let Some(tone) = &candidate.tone {
        map.insert("tone".to_string(), tone_json(tone));
    }
    map.insert("rank".to_string(), Value::from(rank));
    let changes: Vec<Value> = candidate
        .changes
        .iter()
        .map(|change| {
            json!({
                "row": change.index + 1,
                "tier": design.tiers.get(change.index).map(|tier| tier.name.clone()),
                "from_deg": json_number(change.from_deg, JSON_ANGLE_DECIMALS),
                "to_deg": json_number(change.to_deg, JSON_ANGLE_DECIMALS),
            })
        })
        .collect();
    map.insert("changes".to_string(), Value::Array(changes));
    map.insert(
        "mast_changes".to_string(),
        Value::from(candidate.mast_changes.len()),
    );
    Value::Object(map)
}

/// The JSON report.
fn json_report(run: &Run<'_>) -> String {
    let outcome = &run.result.outcome;
    let candidates: Vec<Value> = run
        .result
        .candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| candidate_json(&run.loaded.design, position + 1, candidate))
        .collect();
    let mut start = components_json(
        &outcome.before,
        outcome.before_score,
        outcome.before_yield_loss_pct,
    );
    if let Some(tone) = &run.result.tone_before {
        start.insert("tone".to_string(), tone_json(tone));
    }
    json_text(&json!({
        "design": run.loaded.name,
        "file": run.loaded.file_name,
        "material": run.scoring.label,
        "objective": preset_name(run.args.preset),
        "lighting": lighting_name(run.args.lighting),
        "budget": run.args.budget,
        "starts": run.args.starts,
        "starts_run": run.result.starts_run,
        "best_start": run.result.best_start,
        "seed": run.args.seed,
        "vary_anchored": run.vary_anchored,
        "evaluations": outcome.evaluations,
        "polish_evaluations": outcome.polish_evaluations,
        "tone_lighting": lighting_name(run.result.lighting),
        "tone_goal": run.result.tone_goal.map(ToneGoal::label),
        "start": start,
        "candidates": candidates,
        "status": run.status,
        "written": run.written,
        "notes": run.notes,
    }))
}

/// Runs `optimize` on an opened design.
///
/// # Errors
///
/// [`CliError`] when the design's material cannot be found, the design is not a usable stone,
/// nothing in it can move, or the search cannot start.
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &OptimizeArgs) -> CommandResult {
    let design = &loaded.design;
    let scoring = resolve_scoring(design, catalogue, None)?;
    let analysis = analyze(design);
    analysis.require_stone(design)?;
    let (plan, vary_anchored) = plan_for(design, &analysis, args)?;
    // The face-up tone is sized by the design's girdle diameter, like the Live Render.
    let planes = design_to_gpu_planes_from_solved(design, analysis.require_stone(design)?);
    let gem = scoring.sized_gem(design, &planes);
    let result = optimize_design_with(
        design,
        &gem,
        &plan.config,
        &plan.options,
        &SearchHooks::default(),
    )
    .map_err(|error| CliError::design(format!("the search could not start: {error}")))?;
    let mut notes = loaded.notes.clone();
    notes.extend(scoring.note.clone());
    let best = result.candidates.first();
    let file = if let (Some(path), Some(best)) = (&args.out, best) {
        let after = applied_design(loaded, best)?;
        Some((path.clone(), loaded.indicatrix_text(&after, catalogue)?))
    } else {
        None
    };
    let status = if best.is_none() && result.outcome.evaluations == 0 {
        "no tier was free to move, so nothing was changed.".to_string()
    } else {
        optimize_run_status(&result, None)
    };
    let run = Run {
        loaded,
        scoring: &scoring,
        args,
        vary_anchored,
        status,
        result: &result,
        notes,
        written: file
            .as_ref()
            .map(|(path, _)| path.to_string_lossy().into_owned()),
    };
    let report = if args.json {
        json_report(&run)
    } else {
        text_report(&run)
    };
    let mut outcome = Outcome::text(report);
    match file {
        Some((path, text)) => outcome = outcome.with_file(path, text),
        None if args.out.is_some() => {
            outcome = outcome.with_note("no better arrangement was found, so nothing was written");
        }
        None => {}
    }
    Ok(outcome)
}

/// Runs `optimize`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, is not a usable stone, has nothing that can
/// move, or the search cannot start.
pub fn run(args: &OptimizeArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load::from_bytes,
        outcome::{EXIT_DESIGN, EXIT_OK},
        testing,
    };
    use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, PreformSpec, ScheduleMeta};
    use std::path::PathBuf;

    fn args(budget: usize) -> OptimizeArgs {
        OptimizeArgs {
            design: PathBuf::new(),
            preset: ObjectivePreset::Balanced,
            budget,
            starts: 1,
            seed: 0,
            vary_anchored: false,
            candidates: 1,
            lighting: CANONICAL_LIGHTING_PRESET,
            json: false,
            out: None,
            db: None,
        }
    }

    fn loaded() -> Loaded {
        Loaded::in_memory(testing::brilliant_at_172(), "brilliant.indicatrix")
    }

    #[test]
    fn a_pinned_design_is_varied_about_its_girdle_edges_by_default() {
        let design = testing::brilliant_at_172();
        let analysis = analyze(&design);
        let (plan, vary) = plan_for(&design, &analysis, &args(30)).expect("a plan");
        assert!(vary, "every tier of the brilliant is pinned");
        assert!(plan.options.vary_anchored);
        assert!(
            !plan.options.anchor_hinges.is_empty(),
            "the hinges are measured"
        );
        assert_eq!(plan.config.max_evaluations, 30);
        assert_eq!(plan.config.seed, 0);
        assert_eq!(plan.options.keep_candidates, 1);
        assert!(
            plan.options.min_girdle_fraction.is_some(),
            "the girdle is kept"
        );
    }

    #[test]
    fn the_objective_and_seed_reach_the_search() {
        let design = testing::brilliant_at_172();
        let analysis = analyze(&design);
        let mut wanted = args(10);
        wanted.preset = ObjectivePreset::LowWindowing;
        wanted.seed = 7;
        wanted.candidates = 3;
        let (plan, _) = plan_for(&design, &analysis, &wanted).expect("a plan");
        assert_eq!(plan.config.seed, 7);
        assert_eq!(plan.config.weights, ObjectivePreset::LowWindowing.weights());
        assert_eq!(plan.options.keep_candidates, 3);
    }

    #[test]
    fn a_design_that_does_not_close_is_refused() {
        let empty = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let loaded = Loaded::in_memory(empty, "empty.indicatrix");
        let error = execute(&loaded, &Catalogue::default(), &args(10)).expect_err("no stone");
        assert_eq!(error.code, EXIT_DESIGN, "{}", error.message);
    }

    #[test]
    fn a_short_search_reports_a_start_row_and_a_status() {
        let outcome = execute(&loaded(), &Catalogue::default(), &args(12)).expect("searches");
        assert_eq!(outcome.exit, EXIT_OK);
        assert_eq!(outcome.files.len(), 0);
        let text = outcome.stdout;
        assert!(text.contains("Objective:"), "{text}");
        // The ranking table: a header line and the unchanged design as its first row.
        assert!(text.lines().any(|line| line.starts_with("Rank")), "{text}");
        assert!(text.lines().any(|line| line.starts_with("Start")), "{text}");
        // The header labels are padded to the longest one ("Objective:"), so the number of
        // spaces after "Material:" is not part of what the line says.
        let material = text
            .lines()
            .find_map(|line| line.strip_prefix("Material:"))
            .expect("a Material line");
        assert_eq!(material.trim(), "refractive index 1.7200", "{text}");
    }

    #[test]
    fn the_tone_presets_reach_the_search_and_the_report_names_the_goal() {
        let design = testing::brilliant_at_172();
        let analysis = analyze(&design);
        let mut wanted = args(10);
        wanted.preset = ObjectivePreset::LightenDark;
        let (plan, _) = plan_for(&design, &analysis, &wanted).expect("a plan");
        assert_eq!(plan.config.weights, ObjectivePreset::LightenDark.weights());
        let outcome = execute(&loaded(), &Catalogue::default(), &wanted).expect("searches");
        assert!(outcome.stdout.contains("Tone"), "{}", outcome.stdout);
        assert!(
            outcome
                .stdout
                .contains("the search favoured a lighter stone"),
            "{}",
            outcome.stdout
        );
    }

    #[test]
    fn the_same_search_gives_the_same_report() {
        let first = execute(&loaded(), &Catalogue::default(), &args(10)).expect("searches");
        let second = execute(&loaded(), &Catalogue::default(), &args(10)).expect("searches");
        assert_eq!(first.stdout, second.stdout);
    }

    #[test]
    fn end_to_end_a_file_is_written_exactly_when_a_candidate_was_found() {
        let design = testing::brilliant_at_172();
        let input = testing::write_design("optimize-e2e.indicatrix", &design);
        let target = testing::temp_path("optimize-e2e-out.indicatrix")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&[
            "optimize",
            &input,
            "--budget",
            "20",
            "--candidates",
            "1",
            "--json",
            "--out",
            &target,
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        // The face-up tone is always reported, weighted or not, under the run's own light.
        assert_eq!(
            value["tone_lighting"],
            lighting_name(CANONICAL_LIGHTING_PRESET),
            "{}",
            outcome.stdout
        );
        assert!(value["tone_goal"].is_null(), "{}", outcome.stdout);
        assert!(
            value["start"]["tone"]["l_star"].is_number(),
            "{}",
            outcome.stdout
        );
        assert_eq!(
            value["start"]["tone"]["srgb"].as_array().map(Vec::len),
            Some(3)
        );
        let found = !value["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty();
        assert_eq!(
            outcome.files.len(),
            usize::from(found),
            "{}",
            outcome.stdout
        );
        if found {
            let saved = testing::file_text(&outcome, &target).expect("the design is saved");
            let again = from_bytes("saved.indicatrix", saved.as_bytes()).expect("opens");
            assert_ne!(testing::shape(&again.design), testing::shape(&design));
            assert_eq!(value["written"], target.as_str());
        } else {
            assert!(
                outcome.stderr.contains("nothing was written"),
                "{}",
                outcome.stderr
            );
            assert!(value["written"].is_null());
        }
    }

    #[test]
    fn the_change_table_reads_magnitudes_and_a_difference_with_its_sign() {
        let mut design = testing::brilliant_at_172();
        let tier = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        design.tiers[tier].angle_deg = -41.0;
        let rows = change_rows(
            &design,
            &[AngleChange {
                index: tier,
                from_deg: -41.0,
                to_deg: -41.5,
            }],
        );
        // The stored angles are negative; the report shows 41 and 41.5, and half a degree
        // steeper is +0.5 whichever side of the girdle the tier is on.
        assert_eq!(
            rows,
            vec![vec![
                (tier + 1).to_string(),
                "Pavilion Main".to_string(),
                "41.0000".to_string(),
                "41.5000".to_string(),
                "+0.5000".to_string(),
            ]]
        );
    }

    #[test]
    fn the_change_table_names_an_old_style_tier_by_its_standard_code() {
        let mut design = testing::brilliant_at_172();
        let tier = design
            .tier_position_by_name("Pavilion Main")
            .expect("the standard brilliant has a Pavilion Main");
        design.tiers[tier].name = "3".to_string();
        let code = indicatrix_cut_core::compute_tier_labels(&design.tiers)[tier]
            .code
            .clone();
        let rows = change_rows(
            &design,
            &[AngleChange {
                index: tier,
                from_deg: -41.0,
                to_deg: -40.0,
            }],
        );
        assert_eq!(rows[0][1], code);
        assert_ne!(rows[0][1], "3", "the old-style name is not shown");
        assert_eq!(rows[0][4], "-1.0000", "one degree flatter");
    }
}

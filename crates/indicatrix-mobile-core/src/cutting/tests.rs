//! Tests for the instructions, the done marks, the sheet and the diagram.
//!
//! They run on the standard round brilliant template, whose eight tiers are, in tier
//! order: 0 Pavilion Main, 1 Girdle, 2 Lower Girdle, 3 Culet, 4 Crown Main, 5 Star,
//! 6 Upper Girdle, 7 Table.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use indicatrix_cut_core::{design::TierRef, templates::TEMPLATES};
use indicatrix_editor::EditorSession;

use super::*;
use crate::{
    design_state::{Installed, begin_solve, install, run_solve, set_angle_from_text},
    platform::Platform,
    platform_mock::MockPlatform,
};

/// The Crown Main tier.
const CROWN: usize = 4;

/// A state over the round brilliant template, titled "Test Stone", not solved yet.
fn unsolved_template() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.session.design.meta.headers = vec!["Test Stone".to_owned()];
    state
}

fn solve_now(state: &mut AppState) {
    let job = begin_solve(state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(state, job.generation, response, Duration::from_millis(5));
    assert_eq!(installed, Installed::Applied);
}

fn solved_template() -> AppState {
    let mut state = unsolved_template();
    solve_now(&mut state);
    state
}

// ---- rows -------------------------------------------------------------------------------

#[test]
fn nothing_is_offered_before_the_design_is_solved() {
    let state = unsolved_template();
    assert_eq!(instruction_rows(&state), Err(NotSolved));
    assert_eq!(summary(&state), Err(NotSolved));
    assert!(cutting_sheet_html(&state, "").is_err());
    assert_eq!(diagram_png(&state), Err(CutError::NotSolved));
    assert!(diagram_job(&state, None, 100, 40).is_err());
}

#[test]
fn rows_of_a_solved_template_follow_the_cutting_order() {
    let state = solved_template();
    let rows = instruction_rows(&state).expect("rows");
    assert_eq!(rows.len(), 8);
    for (position, row) in rows.iter().enumerate() {
        assert_eq!(row.step, position);
        assert_ne!(row.code, "");
        assert!(row.angle_text.ends_with('\u{b0}'), "{}", row.angle_text);
        assert!(!row.angle_text.starts_with('-'), "{}", row.angle_text);
        assert!(!row.done && !row.changed);
    }
    // The first row opens the first section; a header appears only where it changes.
    assert!(rows[0].section_start);
    for pair in rows.windows(2) {
        assert_eq!(pair[1].section_start, pair[1].section != pair[0].section);
    }
    // Every flat tier appears exactly once.
    let mut tiers: Vec<usize> = rows.iter().filter_map(|row| row.tier_index).collect();
    tiers.sort_unstable();
    assert_eq!(tiers, (0..8).collect::<Vec<_>>());
    // Index positions are dash-joined and zero-padded (`96-08-16` style).
    let plan = plan(&state).expect("plan");
    let many = plan
        .steps
        .iter()
        .position(|step| step.indices.len() > 1)
        .expect("a tier with several indices");
    assert!(
        rows[many].index_text.contains('-'),
        "{}",
        rows[many].index_text
    );
    assert!(
        rows[many]
            .index_text
            .split('-')
            .all(|part| part.len() >= 2 && part.chars().all(|c| c.is_ascii_digit() || c == '.')),
        "{}",
        rows[many].index_text
    );
}

#[test]
fn a_stale_solve_gives_no_rows() {
    let mut state = solved_template();
    set_angle_from_text(&mut state, CROWN, "33").expect("edit");
    assert_eq!(instruction_rows(&state), Err(NotSolved));
}

// ---- progress ---------------------------------------------------------------------------

#[test]
fn marks_survive_a_restart() {
    let mut state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    assert!(state.cutting.progress.is_bound());
    state.cutting.progress.set_done(&steps[0], true);
    assert_eq!(state.cutting.progress.done_count(&steps), 1);

    // A fresh store (a new process) reads the file back.
    state.cutting = CuttingState::default();
    let rows = instruction_rows(&state).expect("rows");
    assert!(rows[0].done);
    assert!(rows[1..].iter().all(|row| !row.done));
    let summary = summary(&state).expect("summary");
    assert_eq!((summary.done, summary.total), (1, 8));
    assert_eq!(summary.text, "1 of 8 Facets Done");
    assert_eq!(summary.next_step, 1);
    assert!(!summary.all_done);
}

#[test]
fn a_mark_whose_values_changed_is_ignored() {
    let mut state = unsolved_template();
    // An id keeps the marks under one file across edits (without one the text hash
    // changes with the edit and the marks are simply gone).
    state.document.metadata.id = "00000000-0000-4000-8000-000000000001".to_owned();
    solve_now(&mut state);
    let before = plan(&state).expect("plan");
    let position = before
        .steps
        .iter()
        .position(|step| step.tier == TierRef::Flat(CROWN))
        .expect("the crown step");
    sync_progress(&state);
    state
        .cutting
        .progress
        .set_done(&before.steps[position], true);
    assert!(state.cutting.progress.is_done(&before.steps[position]));

    set_angle_from_text(&mut state, CROWN, "33").expect("edit");
    solve_now(&mut state);
    let rows = instruction_rows(&state).expect("rows");
    let row = rows
        .iter()
        .find(|row| row.tier_index == Some(CROWN))
        .expect("the crown row");
    assert!(!row.done);
    assert!(row.changed);
    let summary = summary(&state).expect("summary");
    assert_eq!(summary.done, 0);
    assert_eq!(summary.changed, 1);
    assert!(
        summary.text.contains("1 Changed Since Marked"),
        "{}",
        summary.text
    );

    // Marking it again replaces the old mark.
    let after = plan(&state).expect("plan");
    let step = after
        .steps
        .iter()
        .find(|step| step.tier == TierRef::Flat(CROWN))
        .expect("step");
    state.cutting.progress.set_done(step, true);
    assert!(state.cutting.progress.is_done(step));
}

#[test]
fn toggle_and_reset() {
    let state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    let progress = &state.cutting.progress;
    assert!(progress.toggle_done(&steps[2]));
    assert!(progress.is_done(&steps[2]));
    assert!(!progress.toggle_done(&steps[2]));
    assert!(!progress.is_done(&steps[2]));

    for step in &steps {
        progress.set_done(step, true);
    }
    assert_eq!(progress.done_count(&steps), steps.len());
    assert!((progress.progress_fraction(&steps) - 1.0).abs() < f32::EPSILON);
    assert_eq!(progress.resume_position(&steps), steps.len() - 1);
    let all = summary(&state).expect("summary");
    assert!(all.all_done);

    progress.reset();
    assert_eq!(progress.done_count(&steps), 0);
    assert_eq!(progress.resume_position(&steps), 0);
    let mut state = state;
    state.cutting = CuttingState::default();
    assert_eq!(summary(&state).expect("summary").done, 0);
}

#[test]
fn a_corrupt_progress_file_reads_as_no_marks() {
    let state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    state.cutting.progress.set_done(&steps[0], true);
    let dir = state.platform().app_dirs().files.join("cut_progress");
    let file = std::fs::read_dir(&dir)
        .expect("progress dir")
        .next()
        .expect("one file")
        .expect("entry")
        .path();
    std::fs::write(&file, "{ not json").expect("corrupt it");

    let mut state = state;
    state.cutting = CuttingState::default();
    assert_eq!(summary(&state).expect("summary").done, 0);
}

#[test]
fn marks_made_before_the_design_got_an_id_are_adopted() {
    let mut state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    state.cutting.progress.set_done(&steps[3], true);

    // The first save stamps an id; the text (and so the legacy key) is unchanged.
    state.document.metadata.id = "00000000-0000-4000-8000-0000000000aa".to_owned();
    state.cutting = CuttingState::default();
    let rows = instruction_rows(&state).expect("rows");
    assert!(rows[3].done);
}

// ---- removing marks ---------------------------------------------------------------------

/// The names of the files in the progress folder of `state`, sorted.
fn progress_file_names(state: &AppState) -> Vec<String> {
    let dir = progress_dir(&state.platform().app_dirs().files);
    let mut names = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn reset_clears_marks_that_are_only_changed_ones() {
    let mut state = unsolved_template();
    state.document.metadata.id = "00000000-0000-4000-8000-0000000000bb".to_owned();
    solve_now(&mut state);
    let before = plan(&state).expect("plan");
    let step = before
        .steps
        .iter()
        .find(|step| step.tier == TierRef::Flat(CROWN))
        .expect("the crown step");
    sync_progress(&state);
    state.cutting.progress.set_done(step, true);
    set_angle_from_text(&mut state, CROWN, "33").expect("edit");
    solve_now(&mut state);
    let standing = summary(&state).expect("summary");
    assert_eq!((standing.done, standing.changed), (0, 1));
    assert!(
        state.cutting.progress.has_marks(),
        "a mark whose step changed is still stored"
    );
    assert_eq!(progress_file_names(&state).len(), 1);

    state.cutting.progress.reset();

    assert!(!state.cutting.progress.has_marks());
    assert_eq!(progress_file_names(&state), [] as [String; 0]);
    assert_eq!(summary(&state).expect("summary").changed, 0);
}

#[test]
fn reset_also_removes_the_file_the_marks_were_adopted_from() {
    let mut state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    state.cutting.progress.set_done(&steps[3], true);
    assert_eq!(progress_file_names(&state).len(), 1, "the text-keyed file");

    // The first save stamps an id; the text (and so the legacy key) is unchanged.
    state.document.metadata.id = "00000000-0000-4000-8000-0000000000cc".to_owned();
    state.cutting = CuttingState::default();
    sync_progress(&state);
    assert!(
        state.cutting.progress.has_marks(),
        "the marks of the text key are adopted"
    );
    state.cutting.progress.set_done(&steps[4], true);
    assert_eq!(
        progress_file_names(&state).len(),
        2,
        "the id file joins the text file"
    );

    state.cutting.progress.reset();

    let left = progress_file_names(&state);
    assert!(left.is_empty(), "left behind: {left:?}");
    // A restart does not read the old marks back.
    state.cutting = CuttingState::default();
    assert_eq!(summary(&state).expect("summary").done, 0);
    assert!(!state.cutting.progress.has_marks());
}

#[test]
fn delete_all_removes_every_designs_marks_and_the_folder() {
    let state = solved_template();
    let steps = plan(&state).expect("plan").steps.clone();
    sync_progress(&state);
    state.cutting.progress.set_done(&steps[0], true);
    let dir = progress_dir(&state.platform().app_dirs().files);
    // Another design's marks, and a temp file a crash left beside them.
    std::fs::write(
        dir.join("id-another.json"),
        r#"{"version":1,"marks":{"k":"s"}}"#,
    )
    .expect("writes");
    std::fs::write(dir.join("id-another.json.tmp"), "{").expect("writes");
    assert_eq!(stored_progress_count(&state), 2);

    assert_eq!(delete_all_progress(&state), 2);

    assert_eq!(stored_progress_count(&state), 0);
    assert!(!dir.exists(), "the folder goes with its last file");
    assert!(!state.cutting.progress.has_marks());
    assert_eq!(state.cutting.progress.done_count(&steps), 0);

    // Marking a step again starts the folder anew.
    state.cutting.progress.set_done(&steps[1], true);
    assert_eq!(stored_progress_count(&state), 1);
    assert!(state.cutting.progress.is_done(&steps[1]));
}

#[test]
fn deleting_all_progress_with_none_stored_is_fine() {
    let state = unsolved_template();
    assert_eq!(stored_progress_count(&state), 0);
    assert_eq!(delete_all_progress(&state), 0);
}

#[test]
fn the_file_names_of_a_design_are_its_id_key_and_its_text_key() {
    let mut state = unsolved_template();
    let design = &state.session.design;
    let plain = progress_file_stems("", design);
    assert_eq!(plain.len(), 1);
    assert!(
        plain[0].starts_with("sha-") && plain[0].len() == "sha-".len() + 64,
        "{plain:?}"
    );
    let with_id = progress_file_stems(" 8f2c-AA01 ", design);
    assert_eq!(with_id, ["id-8f2c-AA01", plain[0].as_str()]);

    // They are the names the open design's own marks are filed under.
    assert_eq!(super::key::progress_keys(&state).into_stems(), plain);
    state.document.metadata.id = "8f2c-AA01".to_owned();
    assert_eq!(super::key::progress_keys(&state).into_stems(), with_id);
}

#[test]
fn removing_progress_files_names_them_and_ignores_missing_ones() {
    let state = unsolved_template();
    let files = state.platform().app_dirs().files.clone();
    let dir = progress_dir(&files);
    std::fs::create_dir_all(&dir).expect("creates");
    std::fs::write(dir.join("id-a.json"), "{}").expect("writes");
    std::fs::write(dir.join("id-b.json"), "{}").expect("writes");

    remove_progress_files(&files, &["id-a".to_owned(), "id-missing".to_owned()]);

    assert_eq!(progress_file_names(&state), ["id-b.json"]);
}

// ---- the sheet and the picture ----------------------------------------------------------

#[test]
fn the_sheet_names_the_design_and_every_tier_code() {
    let state = solved_template();
    let html = cutting_sheet_html(&state, "October 2026").expect("sheet");
    assert!(html.contains("Test Stone"));
    for step in &plan(&state).expect("plan").steps {
        assert!(html.contains(&step.code), "the sheet lacks {}", step.code);
    }
}

#[test]
fn the_diagram_is_a_png() {
    let state = solved_template();
    let png = diagram_png(&state).expect("diagram");
    assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
}

#[test]
fn the_highlighted_diagram_differs_from_the_plain_one() {
    let state = solved_template();
    let plain = diagram_job(&state, None, 300, 120)
        .expect("job")
        .render()
        .expect("frame");
    let marked = diagram_job(&state, Some(CROWN), 300, 120)
        .expect("job")
        .render()
        .expect("frame");
    assert_eq!((plain.width, plain.height), (300, 120));
    assert_eq!(plain.color.len(), 300 * 120 * 4);
    assert_ne!(plain.color, marked.color);
}

#[test]
fn month_and_year_come_from_the_clock_seconds() {
    assert_eq!(date_text_for(0), "January 1970");
    assert_eq!(date_text_for(951_782_400), "February 2000");
    assert_eq!(date_text_for(951_868_800), "March 2000");
    assert_eq!(date_text_for(1_760_000_000), "October 2025");
    assert_eq!(date_text_for(-1), "December 1969");
    assert_ne!(date_text_now(), "");
}

#[test]
fn file_names_lose_the_characters_a_file_system_refuses() {
    assert_eq!(
        file_name("Round Brilliant", "_cutting_sheet.html"),
        "Round Brilliant_cutting_sheet.html"
    );
    assert_eq!(
        file_name("A/B: \"C\"?", "_diagram.png"),
        "A_B_ _C___diagram.png"
    );
    assert_eq!(file_name("  ", "_diagram.png"), "Design_diagram.png");
    assert_eq!(file_name("..", "_diagram.png"), "Design_diagram.png");
}

#[test]
fn not_solved_reads_as_a_sentence() {
    assert_eq!(
        NotSolved.to_string(),
        "Solve the design to see cutting instructions"
    );
    assert_eq!(CutError::from(NotSolved), CutError::NotSolved);
    assert_eq!(CutError::Failed("x".to_owned()).to_string(), "x");
}

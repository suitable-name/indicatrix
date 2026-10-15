//! Tests for Cutting Mode's model and its diagram. They run on the standard round
//! brilliant template, solved the way the app solves it.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use indicatrix_cut_core::templates::TEMPLATES;
use indicatrix_editor::EditorSession;

use super::*;
use crate::{
    design_state::{Installed, begin_solve, install, run_solve},
    platform::Platform,
    platform_mock::MockPlatform,
};

/// A state over the standard round brilliant template, not solved yet.
fn template_state() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state
}

/// Solves the current design and installs the answer.
fn solve_now(state: &mut AppState) {
    let job = begin_solve(state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(state, job.generation, response, Duration::from_millis(5));
    assert_eq!(installed, Installed::Applied);
}

/// A solved template with Cutting Mode open at its first step.
fn open_state() -> AppState {
    let mut state = template_state();
    solve_now(&mut state);
    open(&mut state, None).expect("a solved template opens");
    state
}

/// Pretends the design changed: the generation moves, the solve is not current any more.
fn change_design(state: &AppState) {
    state.session.generation.fetch_add(1, Ordering::Relaxed);
}

fn current_step(state: &AppState) -> usize {
    view(state).expect("the mode is open").step
}

#[test]
fn an_unsolved_design_does_not_open() {
    let mut state = template_state();
    assert!(open(&mut state, None).is_err());
    assert!(!is_open(&state));
    assert!(view(&state).is_none());
}

#[test]
fn opening_starts_at_the_first_step_and_walks_the_whole_sequence() {
    let mut state = open_state();
    let first = view(&state).expect("open");
    assert_eq!(first.step, 0);
    assert!(first.count >= 8, "the template has at least eight tiers");
    assert!(!first.has_previous);
    assert!(first.has_next);
    assert!(!previous(&mut state), "no step before the first");
    for expected in 1..first.count {
        assert!(next(&mut state));
        assert_eq!(current_step(&state), expected);
    }
    let last = view(&state).expect("open");
    assert!(!last.has_next);
    assert!(!next(&mut state), "no step after the last");
    assert!(previous(&mut state));
    assert_eq!(current_step(&state), first.count - 2);
}

#[test]
fn opening_at_a_step_honors_it_and_ignores_a_step_that_does_not_exist() {
    let mut state = template_state();
    solve_now(&mut state);
    open(&mut state, Some(3)).expect("opens");
    assert_eq!(current_step(&state), 3);
    open(&mut state, Some(10_000)).expect("opens");
    assert_eq!(
        current_step(&state),
        0,
        "nothing is done yet: the first step"
    );
}

#[test]
fn jump_goes_to_a_step_and_refuses_an_unknown_one() {
    let mut state = open_state();
    let count = view(&state).expect("open").count;
    assert!(jump(&mut state, 4));
    assert_eq!(current_step(&state), 4);
    assert!(!jump(&mut state, 4), "already there");
    assert!(!jump(&mut state, count));
    assert_eq!(current_step(&state), 4);
}

#[test]
fn the_view_carries_the_texts_of_a_step() {
    let mut state = open_state();
    let count = view(&state).expect("open").count;
    let mut sections = std::collections::BTreeSet::new();
    for step in 0..count {
        assert!(step == 0 || next(&mut state));
        let page = view(&state).expect("open");
        assert!(!page.code.is_empty(), "step {step} has a code");
        assert!(page.angle_text.ends_with('°'), "{}", page.angle_text);
        assert!(
            page.index_text
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == '.'),
            "step {step}: {}",
            page.index_text
        );
        assert_ne!(page.index_text, "");
        sections.insert(page.section);
    }
    assert!(sections.contains("Pavilion"));
    assert!(sections.contains("Crown"));
}

#[test]
fn the_angle_and_index_text_follow_the_printed_sheet() {
    let mut state = open_state();
    // The first step of the sheet is a pavilion tier with a listed set of indices.
    let page = view(&state).expect("open");
    let mode = state.cutting.mode.as_ref().expect("open");
    let step = &mode.steps[0];
    assert_eq!(page.angle_text, format!("{:.2}°", step.angle_deg));
    let tier = match step.tier {
        TierRef::Flat(index) => &state.session.design.tiers[index],
        TierRef::Concave(_) => panic!("the template has no concave tier"),
    };
    assert_eq!(page.index_text, format_sheet_indices(&tier.indices));
    assert!(next(&mut state));
}

#[test]
fn done_marks_persist_and_decide_where_the_mode_reopens() {
    let mut state = open_state();
    assert_eq!(toggle_done(&mut state), Some(true));
    assert!(view(&state).expect("open").done);
    assert!(next(&mut state));
    assert!(!view(&state).expect("open").done);
    close(&mut state);
    assert_eq!(toggle_done(&mut state), None, "closed: nothing to mark");
    open(&mut state, None).expect("reopens");
    assert_eq!(
        current_step(&state),
        1,
        "step 0 is done: resume at the next one"
    );
    assert!(jump(&mut state, 0));
    assert_eq!(
        toggle_done(&mut state),
        Some(false),
        "the mark is taken back"
    );
    assert!(!view(&state).expect("open").done);
}

#[test]
fn an_unchanged_design_is_fresh() {
    let mut state = open_state();
    assert!(!is_stale(&state));
    assert_eq!(restart_if_stale(&mut state), Staleness::Fresh);
    assert!(is_open(&state));
    close(&mut state);
    assert_eq!(restart_if_stale(&mut state), Staleness::Fresh);
}

#[test]
fn a_changed_design_closes_the_mode_until_it_is_solved_again() {
    let mut state = open_state();
    assert!(next(&mut state));
    change_design(&state);
    assert!(is_stale(&state));
    assert_eq!(restart_if_stale(&mut state), Staleness::Closed);
    assert!(!is_open(&state));
}

#[test]
fn a_changed_and_solved_design_restarts_the_mode() {
    let mut state = open_state();
    assert!(next(&mut state));
    assert!(next(&mut state));
    change_design(&state);
    solve_now(&mut state);
    assert_eq!(restart_if_stale(&mut state), Staleness::Restarted);
    assert!(is_open(&state));
    assert!(!is_stale(&state));
    assert_eq!(current_step(&state), 0);
}

#[test]
fn the_diagram_highlights_the_tier_being_cut() {
    let state = open_state();
    let scene = diagram_scene(&state).expect("a solved template closes");
    // Tier 0 is the Pavilion Main, tier 2 the Lower Girdle: both show on the pavilion panel.
    let pavilion = diagram::panel_for(StepSide::Pavilion);
    let plain = scene.render(None, pavilion, 400, 400);
    assert_eq!((plain.width, plain.height), (400, 400));
    assert_eq!(plain.rgba.len(), 400 * 400 * 4);
    let first = scene.render(Some(0), pavilion, 400, 400);
    let second = scene.render(Some(2), pavilion, 400, 400);
    assert_ne!(plain.rgba, first.rgba, "the highlight changes the picture");
    assert_ne!(
        first.rgba, second.rgba,
        "another tier highlights other facets"
    );
    // The same request draws the same picture.
    assert_eq!(first, scene.render(Some(0), pavilion, 400, 400));
}

#[test]
fn each_side_picks_its_own_panel() {
    use indicatrix_solid::diagram2d::PanelKind;
    assert_eq!(diagram::panel_for(StepSide::Crown), PanelKind::Crown);
    assert_eq!(diagram::panel_for(StepSide::Pavilion), PanelKind::Pavilion);
    assert_eq!(diagram::panel_for(StepSide::Girdle), PanelKind::Profile);
}

#[test]
fn there_is_no_diagram_without_a_current_solve() {
    let state = open_state();
    change_design(&state);
    assert!(diagram_scene(&state).is_none());
}

#[test]
fn the_details_line_lists_depth_and_the_cutters_note() {
    let mut state = open_state();
    let mode = state.cutting.mode.as_mut().expect("open");
    mode.steps[0].notes = "Check the polish".to_owned();
    mode.steps[0].cheater_offset_deg = Some(1.5);
    mode.steps[0].mast = Some(0.5);
    mode.steps[0].depth_mm = Some(2.25);
    let page = view(&state).expect("open");
    let lines: Vec<&str> = page.notes.lines().collect();
    assert_eq!(lines[0], "Check the polish");
    assert_eq!(lines[1], "Cheater offset +1.50°");
    assert_eq!(lines[2], "Depth 2.25 mm  ·  mast 0.5000");
}

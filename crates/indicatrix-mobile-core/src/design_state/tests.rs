//! Tests for the solve state, the tier rows, tier editing, undo and the keypad.
//!
//! They run on the standard round brilliant template, whose tiers (all anchored by a
//! scale value) are, in order: 0 Pavilion Main (-41), 1 Girdle (90), 2 Lower Girdle
//! (-42.5), 3 Culet (-0, no indices), 4 Crown Main (34.5), 5 Star (15), 6 Upper Girdle
//! (41), 7 Table (0, no indices).

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{Edit, templates::TEMPLATES};
use indicatrix_editor::EditorSession;

use super::*;
use crate::{app_state::AppState, platform::Platform, platform_mock::MockPlatform};

/// The Crown Main tier: a crown tier with eight indices that most tests edit.
const CROWN: usize = 4;
/// The Lower Girdle tier: a pavilion tier with sixteen indices.
const LOWER_GIRDLE: usize = 2;

/// A state over the standard round brilliant template.
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

/// A state over the start-up design: a preform and no tiers.
fn fresh_state() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    AppState::new(platform)
}

/// Runs a full solve of the current design and installs it.
fn solve_now(state: &mut AppState) {
    let job = begin_solve(state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(state, job.generation, response, Duration::from_millis(5));
    assert_eq!(installed, Installed::Applied);
}

fn generation(state: &AppState) -> u64 {
    state.session.current_generation()
}

fn at(seconds: f64) -> Duration {
    Duration::from_secs_f64(seconds)
}

fn angle(state: &AppState, tier: usize) -> f64 {
    state.session.design.tiers[tier].angle_deg
}

// ---- solving --------------------------------------------------------------------------

#[test]
fn a_solved_template_has_current_tiers_planes_and_a_status() {
    let mut state = template_state();
    let tiers = state.session.design.tiers.len();
    assert_eq!(tiers, 8);
    assert!(state.solve.current_tiers(generation(&state)).is_none());

    solve_now(&mut state);
    let generation = generation(&state);
    assert_eq!(
        state.solve.current_tiers(generation).map(<[_]>::len),
        Some(tiers)
    );
    assert!(
        state
            .solve
            .current_planes(generation)
            .is_some_and(|planes| !planes.is_empty())
    );
    assert_eq!(state.solve.last_tiers().map(<[_]>::len), Some(tiers));
    assert!(state.solve.last_took().is_some());
    let (text, _) = state.solve.status();
    assert_ne!(text, "");
    assert!(!state.solve.is_solving());
    let line = status_line(&state);
    assert!(matches!(
        line.kind,
        StatusKind::Solved | StatusKind::Problem
    ));
    assert!(line.text.contains("solved in"));
}

#[test]
fn install_ignores_an_answer_for_a_generation_the_design_left() {
    let mut state = template_state();
    let job = begin_solve(&mut state).expect("job");
    assert!(state.solve.is_solving());
    // The design moves on while the job "runs".
    set_angle_from_text(&mut state, CROWN, "33").expect("edit");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(5),
    );
    assert_eq!(installed, Installed::Stale);
    assert!(!state.solve.is_solving());
    assert!(state.solve.current_tiers(generation(&state)).is_none());
    assert!(state.solve.last_tiers().is_none());
}

#[test]
fn a_cancelled_job_changes_nothing_but_stops_running() {
    let mut state = template_state();
    let job = begin_solve(&mut state).expect("job");
    let cancel = AtomicBool::new(true);
    let response = run_solve(&job, &cancel);
    let installed = install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(1),
    );
    assert_eq!(installed, Installed::Cancelled);
    assert!(!state.solve.is_solving());
    assert!(state.solve.last_tiers().is_none());
}

#[test]
fn begin_solve_refuses_a_design_that_has_an_answer_or_a_running_job() {
    let mut state = template_state();
    let job = begin_solve(&mut state).expect("first job");
    assert!(
        begin_solve(&mut state).is_none(),
        "the same generation is running"
    );
    let response = run_solve(&job, &AtomicBool::new(false));
    install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(2),
    );
    assert!(
        begin_solve(&mut state).is_none(),
        "already solved at this generation"
    );
    set_angle_from_text(&mut state, CROWN, "34").expect("edit");
    assert!(
        begin_solve(&mut state).is_some(),
        "an edit makes a new solve possible"
    );
}

#[test]
fn an_empty_design_still_solves_to_planes_for_the_viewport() {
    let mut state = fresh_state();
    solve_now(&mut state);
    let generation = generation(&state);
    assert_eq!(
        state.solve.current_tiers(generation).map(<[_]>::len),
        Some(0)
    );
    assert!(state.solve.current_planes(generation).is_some());
}

#[test]
fn invalidate_forgets_everything() {
    let mut state = template_state();
    solve_now(&mut state);
    let revision = state.solve.revision();
    state.solve.invalidate();
    assert!(state.solve.last_tiers().is_none());
    assert_ne!(state.solve.revision(), revision);
    assert_eq!(state.solve.status().0, "Not solved yet.");
}

#[test]
fn the_auto_solve_policy_follows_the_budget_and_the_answer() {
    let mut state = template_state();
    assert_eq!(
        should_auto_solve(&state),
        Some(indicatrix_editor::solve_policy::AUTO_SOLVE_DEBOUNCE)
    );
    solve_now(&mut state);
    assert_eq!(should_auto_solve(&state), None, "already solved");
    set_angle_from_text(&mut state, CROWN, "35").expect("edit");
    assert!(should_auto_solve(&state).is_some());
    state.update_settings(|settings| settings.auto_solve_budget_ms = 0);
    assert_eq!(should_auto_solve(&state), None, "auto-solve is off");
}

#[test]
fn a_slow_last_solve_switches_auto_solve_off_for_a_big_design() {
    let mut state = template_state();
    let job = begin_solve(&mut state).expect("job");
    let response = run_solve(&job, &AtomicBool::new(false));
    install(&mut state, job.generation, response, Duration::from_secs(3));
    set_angle_from_text(&mut state, CROWN, "36").expect("edit");
    assert_eq!(should_auto_solve(&state), None);
    assert!(!should_solve_synchronously(&state));
    assert!(status_line(&state).text.starts_with("Auto-solve off"));
}

#[test]
fn small_designs_solve_on_the_spot() {
    assert!(should_solve_synchronously(&fresh_state()));
}

// ---- rows and status ------------------------------------------------------------------

#[test]
fn solved_rows_carry_masts_and_no_stale_mark() {
    let mut state = template_state();
    solve_now(&mut state);
    let rows = tier_rows(&state);
    assert_eq!(rows.len(), state.session.design.tiers.len());
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.index, index);
        assert!(!row.is_stale);
        assert!(!row.is_concave);
        assert_ne!(row.angle_text, "");
        assert_ne!(row.code, "");
    }
    assert!(rows.iter().any(|row| !row.mast_text.is_empty()));
    assert_eq!(
        rows[0].angle_text, "41.00",
        "a pavilion angle shows no sign"
    );
    assert_eq!(rows[0].block, "Pavilion");
    assert_eq!(rows[CROWN].block, "Crown");
}

#[test]
fn rows_go_stale_after_an_edit_and_keep_untouched_masts() {
    let mut state = template_state();
    solve_now(&mut state);
    let before = tier_rows(&state);
    set_angle_from_text(&mut state, CROWN, "37").expect("edit");
    let rows = tier_rows(&state);
    assert!(rows.iter().all(|row| row.is_stale));
    assert_eq!(
        rows[CROWN].mast_text, "",
        "the edited tier's mast is blanked"
    );
    assert_eq!(rows[CROWN].angle_text, "37.00");
    assert!(
        !rows[0].mast_text.is_empty() && rows[0].mast_text == before[0].mast_text,
        "an untouched tier keeps its previous mast"
    );
    solve_now(&mut state);
    assert!(tier_rows(&state).iter().all(|row| !row.is_stale));
}

#[test]
fn an_edit_nobody_reported_makes_every_row_dirty() {
    let mut state = template_state();
    solve_now(&mut state);
    let mut changed = state.session.design.tiers[LOWER_GIRDLE].clone();
    changed.angle_deg += 1.0;
    state
        .session
        .apply(Edit::ModifyTier {
            index: LOWER_GIRDLE,
            tier: changed,
        })
        .expect("direct edit");
    let rows = tier_rows(&state);
    assert!(
        rows.iter()
            .all(|row| row.is_stale && row.mast_text.is_empty())
    );
}

#[test]
fn status_lines_cover_empty_solving_stale_and_solved() {
    let mut fresh = fresh_state();
    let line = status_line(&fresh);
    assert_eq!(line.kind, StatusKind::None);
    assert!(line.text.starts_with("Preform only"));

    let mut state = template_state();
    assert_eq!(status_line(&state).kind, StatusKind::Stale);
    assert_eq!(status_line(&state).text, "Not solved since the last edit.");
    let job = begin_solve(&mut state).expect("job");
    assert_eq!(status_line(&state).kind, StatusKind::Solving);
    let response = run_solve(&job, &AtomicBool::new(false));
    install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(3),
    );
    assert!(matches!(
        status_line(&state).kind,
        StatusKind::Solved | StatusKind::Problem
    ));

    state.update_settings(|settings| settings.auto_solve_budget_ms = 0);
    set_angle_from_text(&mut state, CROWN, "38").expect("edit");
    assert!(status_line(&state).text.contains("Auto-solve is off"));

    solve_now(&mut fresh);
    assert_eq!(status_line(&fresh).kind, StatusKind::None);
}

#[test]
fn tier_detail_lists_the_indices_and_move_limits() {
    let mut state = template_state();
    solve_now(&mut state);
    let rows = tier_rows(&state);
    let first = tier_detail(&state, &rows[0]).expect("detail");
    assert!(!first.can_move_up);
    assert!(first.can_move_down);
    assert_eq!(
        first.indices.len(),
        state.session.design.tiers[0].indices.len()
    );
    let last = rows.len() - 1;
    let end = tier_detail(&state, &rows[last]).expect("detail");
    assert!(end.can_move_up);
    assert!(!end.can_move_down);
    assert_ne!(first.title, "");
    assert_ne!(first.meet_summary, "");
}

// ---- angle editing --------------------------------------------------------------------

#[test]
fn typed_angles_apply_once_and_an_unchanged_value_spends_no_step() {
    let mut state = template_state();
    let result = set_angle_from_text(&mut state, CROWN, "33.5").expect("edit");
    assert!(result.changed);
    assert!((angle(&state, CROWN) - 33.5).abs() < 1e-9);
    let again = set_angle_from_text(&mut state, CROWN, "33.5").expect("no change");
    assert!(!again.changed);
    assert_eq!(again.notice.as_deref(), Some("No change."));
    let error = set_angle_from_text(&mut state, CROWN, "abc").expect_err("not a number");
    assert!(matches!(error, EditRefusal::Failed(_)));
    let steep = set_angle_from_text(&mut state, CROWN, "95").expect_err("over 90");
    assert!(steep.to_string().contains("exceeds 90"));
}

#[test]
fn a_typed_magnitude_keeps_a_pavilion_tier_on_its_side() {
    let mut state = template_state();
    set_angle_from_text(&mut state, LOWER_GIRDLE, "43").expect("edit");
    assert!((angle(&state, LOWER_GIRDLE) + 43.0).abs() < 1e-9);
}

#[test]
fn nudges_within_the_window_are_one_undo_step() {
    let mut state = template_state();
    let original = angle(&state, CROWN);
    nudge_angle(&mut state, CROWN, 0.1, at(10.0)).expect("nudge");
    nudge_angle(&mut state, CROWN, 0.1, at(10.4)).expect("nudge");
    nudge_angle(&mut state, CROWN, 0.01, at(11.0)).expect("nudge");
    let nudged = angle(&state, CROWN);
    assert!(
        (nudged - (original + 0.21)).abs() < 1e-9,
        "{nudged} vs {original}"
    );
    undo(&mut state).expect("undo");
    assert_eq!(
        angle(&state, CROWN).to_bits(),
        original.to_bits(),
        "one undo takes back the whole burst"
    );
    redo(&mut state).expect("redo");
    assert!((angle(&state, CROWN) - nudged).abs() < 1e-12);
}

#[test]
fn nudges_far_apart_are_separate_steps() {
    let mut state = template_state();
    let original = angle(&state, CROWN);
    nudge_angle(&mut state, CROWN, 1.0, at(10.0)).expect("nudge");
    nudge_angle(&mut state, CROWN, 1.0, at(20.0)).expect("nudge");
    undo(&mut state).expect("undo");
    assert!((angle(&state, CROWN) - (original + 1.0)).abs() < 1e-9);
}

#[test]
fn a_nudge_moves_the_shown_number_of_a_pavilion_tier() {
    let mut state = template_state();
    let original = angle(&state, LOWER_GIRDLE);
    nudge_angle(&mut state, LOWER_GIRDLE, 0.5, at(5.0)).expect("nudge");
    assert!((angle(&state, LOWER_GIRDLE) - (original - 0.5)).abs() < 1e-9);
}

#[test]
fn the_slider_keeps_a_pavilion_tier_on_its_side_and_coalesces() {
    let mut state = template_state();
    let pavilion = 0;
    let original = angle(&state, pavilion);
    assert!(original < 0.0);
    set_angle_value(&mut state, pavilion, 40.0, at(5.0)).expect("slider");
    set_angle_value(&mut state, pavilion, 40.5, at(5.1)).expect("slider");
    assert!((angle(&state, pavilion) + 40.5).abs() < 1e-9);
    undo(&mut state).expect("undo");
    assert_eq!(angle(&state, pavilion).to_bits(), original.to_bits());
}

#[test]
fn a_nudge_that_would_cross_zero_stops_there_and_says_so() {
    let mut state = template_state();
    set_angle_from_text(&mut state, CROWN, "0.05").expect("edit");
    let result = nudge_angle(&mut state, CROWN, -1.0, at(30.0)).expect("nudge");
    assert!(result.changed);
    assert!(result.notice.expect("a notice").contains("stopped at 0"));
    assert!(angle(&state, CROWN).abs() < 1e-12);
    assert!(!angle(&state, CROWN).is_sign_negative());
}

#[test]
fn offset_multi_moves_every_selected_tier() {
    let mut state = template_state();
    let (a, b) = (CROWN, 5);
    let (angle_a, angle_b) = (angle(&state, a), angle(&state, b));
    toggle_multi(&mut state, a);
    toggle_multi(&mut state, b);
    offset_multi(&mut state, 0.5, at(40.0)).expect("offset");
    assert!((angle(&state, a) - (angle_a + 0.5)).abs() < 1e-9);
    assert!((angle(&state, b) - (angle_b + 0.5)).abs() < 1e-9);
}

// ---- selection ------------------------------------------------------------------------

#[test]
fn a_plain_selection_empties_the_group_unless_select_mode_is_on() {
    let mut state = template_state();
    assert!(toggle_multi(&mut state, 1));
    assert!(toggle_multi(&mut state, 2));
    select_tier(&mut state, Some(3), true);
    assert_eq!(state.selected_tier, Some(3));
    assert_eq!(state.session.multi_selected.len(), 2);
    select_tier(&mut state, Some(4), false);
    assert!(state.session.multi_selected.is_empty());
    select_tier(&mut state, Some(99), false);
    assert_eq!(state.selected_tier, None);
    assert!(!toggle_multi(&mut state, 99));
    select_all(&mut state);
    assert_eq!(
        state.session.multi_selected.len(),
        state.session.design.tiers.len()
    );
    clear_multi(&mut state);
    assert!(state.session.multi_selected.is_empty());
}

#[test]
fn the_selection_follows_its_tier_through_remove_undo_and_redo() {
    let mut state = template_state();
    let count = state.session.design.tiers.len();
    select_tier(&mut state, Some(5), false);
    remove(&mut state, 2, false).expect("remove");
    assert_eq!(state.session.design.tiers.len(), count - 1);
    assert_eq!(state.selected_tier, Some(4), "a later tier follows down");
    undo(&mut state).expect("undo");
    assert_eq!(state.session.design.tiers.len(), count);
    assert_eq!(state.selected_tier, Some(5), "and back up");
    redo(&mut state).expect("redo");
    assert_eq!(state.selected_tier, Some(4));
    select_tier(&mut state, Some(2), false);
    remove(&mut state, 2, false).expect("remove the selected tier");
    assert_eq!(state.selected_tier, None);
}

#[test]
fn undo_and_redo_report_their_hints_and_availability() {
    let mut state = template_state();
    assert!(!can_undo(&state) && !can_redo(&state));
    assert_eq!(undo_hint(&state), "");
    set_angle_from_text(&mut state, CROWN, "39").expect("edit");
    assert!(can_undo(&state) && !can_redo(&state));
    assert_ne!(undo_hint(&state), "");
    let undone = undo(&mut state).expect("undo");
    assert!(undone.changed);
    assert!(undone.notice.expect("notice").starts_with("Undid"));
    assert!(can_redo(&state));
    assert_ne!(redo_hint(&state), "");
    redo(&mut state).expect("redo");
    let nothing = redo(&mut state).expect("nothing left");
    assert!(!nothing.changed);
}

// ---- structure ------------------------------------------------------------------------

#[test]
fn add_and_quick_add_work_on_a_fresh_design() {
    let mut state = fresh_state();
    let added = add_tier(&mut state).expect("add");
    assert!(added.changed);
    assert_eq!(state.session.design.tiers.len(), 1);
    assert_eq!(state.selected_tier, Some(0));
    assert!((angle(&state, 0) - 40.0).abs() < 1e-9);
    assert_eq!(
        state.session.design.tiers[0].indices.len(),
        8,
        "a full orbit"
    );

    quick_add(&mut state, QuickAdd::Table).expect("table");
    assert_eq!(state.session.design.tiers.len(), 2);
    assert_eq!(state.session.design.tiers[1].name, "Table");
    assert_eq!(state.selected_tier, Some(1));
    let again = quick_add(&mut state, QuickAdd::Table).expect_err("names are unique");
    assert!(again.to_string().contains("already named"));
    quick_add(&mut state, QuickAdd::Culet).expect("culet");
    assert!(angle(&state, 2).is_sign_negative());
    assert_eq!(QuickAdd::from_index(1), Some(QuickAdd::Girdle));
    assert_eq!(QuickAdd::from_index(7), None);
}

#[test]
fn duplicate_selects_the_copy_and_move_follows_the_tier() {
    let mut state = template_state();
    let count = state.session.design.tiers.len();
    duplicate(&mut state, 2).expect("duplicate");
    assert_eq!(state.session.design.tiers.len(), count + 1);
    assert_eq!(state.selected_tier, Some(3));
    move_tier(&mut state, 3, -1).expect("move up");
    assert_eq!(state.selected_tier, Some(2));
    let at_top = move_tier(&mut state, 0, -1).expect("already first");
    assert!(!at_top.changed);
}

#[test]
fn removing_a_tier_that_others_meet_asks_first() {
    let mut state = template_state();
    set_name(&mut state, 2, "Zed").expect("rename");
    state
        .session
        .apply(Edit::SetConstraint {
            index: 3,
            constraint: MeetConstraint::MeetNamed(vec!["Zed".to_owned()]),
        })
        .expect("a meet by name");
    let count = state.session.design.tiers.len();
    let refused = remove(&mut state, 2, false).expect_err("dependants");
    assert!(matches!(refused, EditRefusal::NeedsConfirmation(_)));
    assert!(refused.to_string().contains("still meet it by name"));
    assert_eq!(state.session.design.tiers.len(), count, "nothing changed");
    remove(&mut state, 2, true).expect("confirmed");
    assert_eq!(state.session.design.tiers.len(), count - 1);
    undo(&mut state).expect("one undo restores both the tier and the references");
    assert_eq!(state.session.design.tiers.len(), count);
}

#[test]
fn batch_delete_removes_the_group_and_clears_the_selection() {
    let mut state = template_state();
    let count = state.session.design.tiers.len();
    toggle_multi(&mut state, 0);
    toggle_multi(&mut state, 1);
    let removed = remove_multi(&mut state, false).expect("batch");
    assert!(removed.notice.expect("notice").contains("Removed 2 tiers"));
    assert_eq!(state.session.design.tiers.len(), count - 2);
    assert_eq!(state.selected_tier, None);
    assert!(state.session.multi_selected.is_empty());
    let nothing = remove_multi(&mut state, false).expect("empty group");
    assert!(!nothing.changed);
}

#[test]
fn names_must_be_unique() {
    let mut state = template_state();
    let other = state.session.design.tiers[3].name.clone();
    assert_ne!(other, "");
    let collision = set_name(&mut state, 2, &other).expect_err("taken");
    assert!(collision.to_string().contains("already has that name"));
    set_name(&mut state, 2, "  Fresh  ").expect("trimmed");
    assert_eq!(state.session.design.tiers[2].name, "Fresh");
    let same = set_name(&mut state, 2, "Fresh").expect("same");
    assert!(!same.changed);
}

#[test]
fn indices_are_parsed_validated_and_kept_non_empty() {
    let mut state = template_state();
    let indices = |state: &AppState| state.session.design.tiers[CROWN].indices.clone();
    set_indices_from_text(&mut state, CROWN, "0 12 24").expect("edit");
    assert_eq!(indices(&state), vec![0.0, 12.0, 24.0]);
    let bad = set_indices_from_text(&mut state, CROWN, "0 x").expect_err("not a number");
    assert!(bad.to_string().contains("not a number"));
    let off = set_indices_from_text(&mut state, CROWN, "200").expect_err("off the gear");
    assert!(off.to_string().contains("off the gear"));
    let empty = set_indices_from_text(&mut state, CROWN, "").expect_err("empty");
    assert!(empty.to_string().contains("at least one index"));

    add_index(&mut state, CROWN, 36.0).expect("add");
    assert_eq!(indices(&state), vec![0.0, 12.0, 24.0, 36.0]);
    let repeat = add_index(&mut state, CROWN, 12.0).expect_err("repeat");
    assert!(repeat.to_string().contains("more than once"));
    remove_index(&mut state, CROWN, 1).expect("remove");
    assert_eq!(indices(&state), vec![0.0, 24.0, 36.0]);
    let outside = remove_index(&mut state, CROWN, 9).expect("no such chip");
    assert!(!outside.changed);

    rotate_indices(&mut state, CROWN, 1, at(50.0)).expect("rotate");
    assert_eq!(indices(&state), vec![1.0, 25.0, 37.0]);
    rotate_indices(&mut state, CROWN, 1, at(50.2)).expect("rotate");
    undo(&mut state).expect("undo");
    assert_eq!(indices(&state), vec![0.0, 24.0, 36.0]);
}

#[test]
fn the_orbit_tools_edit_the_indices() {
    let mut state = template_state();
    set_indices_from_text(&mut state, CROWN, "0 24").expect("a partial orbit");
    complete_orbit(&mut state, CROWN).expect("complete");
    assert!(state.session.design.tiers[CROWN].indices.len() > 2);
    let whole = complete_orbit(&mut state, CROWN).expect("already whole");
    assert!(!whole.changed);
    toggle_detach(&mut state, CROWN).expect("detach");
    assert_ne!(
        state.session.design.tiers[CROWN].detached,
        Vec::<f64>::new()
    );
    toggle_detach(&mut state, CROWN).expect("reattach");
    assert_eq!(
        state.session.design.tiers[CROWN].detached,
        Vec::<f64>::new()
    );
}

#[test]
fn adopting_without_imported_meets_says_so() {
    let mut state = template_state();
    let nothing = adopt_all(&mut state).expect("nothing to adopt");
    assert!(!nothing.changed);
    assert!(
        nothing
            .notice
            .expect("notice")
            .starts_with("Nothing to adopt")
    );
    assert!(!adopt(&mut state, 2).expect("none").changed);
    toggle_multi(&mut state, 2);
    assert!(!adopt_selected(&mut state).expect("none").changed);
}

// ---- keypad ---------------------------------------------------------------------------

fn typed(keys: &[&str]) -> KeypadState {
    let mut keypad = KeypadState::new();
    for key in keys {
        keypad.press(Key::from_id(key).expect("a known key"));
    }
    keypad
}

#[test]
fn the_keypad_builds_numbers_and_parses_them() {
    let keypad = typed(&["1", "2", ".", "5"]);
    assert_eq!(keypad.display(), "12.5");
    assert_eq!(keypad.value(), Some(12.5));
    assert!(keypad.has_point());
    assert_eq!(keypad.entry_text().as_deref(), Some("12.5"));

    let negative = typed(&["4", "1", "sign"]);
    assert_eq!(negative.display(), "-41");
    assert_eq!(negative.value(), Some(-41.0));
    assert!(negative.is_negative());
    assert_eq!(typed(&["4", "1", "sign", "sign"]).display(), "41");
}

#[test]
fn the_keypad_edits_and_clears() {
    let mut keypad = typed(&["7", "8", "9", "back"]);
    assert_eq!(keypad.display(), "78");
    keypad.press(Key::Backspace);
    keypad.press(Key::Backspace);
    assert!(keypad.is_empty());
    assert_eq!(keypad.display(), "0");
    assert_eq!(keypad.value(), None);
    assert_eq!(keypad.entry_text(), None);

    let mut signed = typed(&["sign"]);
    assert_eq!(signed.display(), "-");
    assert_eq!(signed.value(), None);
    signed.press(Key::Backspace);
    assert!(
        !signed.is_negative(),
        "backspace on an empty entry drops the sign"
    );

    let mut full = typed(&["3", ".", "1", "4"]);
    full.press(Key::Clear);
    assert_eq!(full, KeypadState::default());
}

#[test]
fn the_keypad_handles_points_zeros_and_length() {
    assert_eq!(typed(&["."]).display(), "0.");
    assert_eq!(typed(&[".", "5"]).value(), Some(0.5));
    assert_eq!(
        typed(&["1", ".", ".", "2"]).display(),
        "1.2",
        "a second point is ignored"
    );
    assert_eq!(typed(&["0", "0", "0"]).display(), "0");
    assert_eq!(typed(&["0", "7"]).display(), "7", "no leading zeros");
    assert_eq!(typed(&["0", ".", "0", "5"]).value(), Some(0.05));
    assert_eq!(
        typed(&["9", "."]).value(),
        Some(9.0),
        "a trailing point still parses"
    );
    let long = typed(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
    assert_eq!(long.display(), "1234567", "the entry stops growing");
}

#[test]
fn minus_zero_is_kept_because_its_sign_names_the_block() {
    let keypad = typed(&["0", "sign"]);
    assert_eq!(keypad.entry_text().as_deref(), Some("-0"));
    let value = keypad.value().expect("a value");
    assert!(value == 0.0 && value.is_sign_negative());
}

#[test]
fn key_ids_map_to_keys() {
    assert_eq!(Key::from_id("7"), Some(Key::Digit(7)));
    assert_eq!(Key::from_id("."), Some(Key::Point));
    assert_eq!(Key::from_id("sign"), Some(Key::Sign));
    assert_eq!(Key::from_id("back"), Some(Key::Backspace));
    assert_eq!(Key::from_id("clear"), Some(Key::Clear));
    assert_eq!(Key::from_id("77"), None);
    assert_eq!(Key::from_id(""), None);
    assert_eq!(Key::from_id("x"), None);
}

#[test]
fn a_typed_keypad_value_commits_through_the_angle_text() {
    let mut state = template_state();
    let keypad = typed(&["3", "6", ".", "2"]);
    let text = keypad.entry_text().expect("typed");
    set_angle_from_text(&mut state, CROWN, &text).expect("commit");
    assert!((angle(&state, CROWN) - 36.2).abs() < 1e-9);
}

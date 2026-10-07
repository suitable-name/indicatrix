//! The History tab's glue: when the rows are out of date, how they are rebuilt, and how a
//! row's picture is asked for. (The rows' wording, the cache keys, the drawing and the
//! thread have their own tests next to them; the Slint side cannot be run here.)

use super::*;
use indicatrix_cut_core::Edit;
use std::time::Instant;

/// A state whose history holds `steps` preform-offset changes.
fn state_with_steps(steps: usize) -> EditorState {
    let mut state = EditorState::fresh();
    for step in 0..steps {
        state
            .apply(Edit::SetPreformYOffset {
                y_offset: 0.05f64.mul_add(step as f64, 0.1),
            })
            .expect("edit must apply");
    }
    state
}

fn new_panel() -> (Panel, Rc<VecModel<HistoryRowData>>) {
    let model = Rc::new(VecModel::<HistoryRowData>::default());
    (Panel::new(Rc::clone(&model)), model)
}

fn titles(model: &VecModel<HistoryRowData>) -> Vec<String> {
    model.iter().map(|row| row.title.to_string()).collect()
}

fn spec(position: usize, title: &str) -> RowSpec {
    RowSpec {
        position,
        title: title.to_string(),
        detail: format!("Step {position}"),
        current: false,
        undone: false,
        start: position == 0,
    }
}

#[test]
fn counts_saturate_into_slints_int() {
    assert_eq!(to_i32(0), 0);
    assert_eq!(to_i32(7), 7);
    assert_eq!(to_i32(usize::MAX), i32::MAX);
}

#[test]
fn a_row_carries_every_field_of_its_spec() {
    let mut source = spec(3, "Set P1 angle");
    source.current = true;
    source.undone = true;
    let row = to_row_data(&source);
    assert_eq!(row.position, 3);
    assert_eq!(row.title.as_str(), "Set P1 angle");
    assert_eq!(row.detail.as_str(), "Step 3");
    assert!(row.current && row.undone && !row.start);
    assert!(to_row_data(&spec(0, "Start")).start);
}

#[test]
fn the_waiting_picture_is_marked_as_still_drawing() {
    assert_eq!(waiting_thumb().state, 0);
}

#[test]
fn the_signature_is_stable_while_nothing_changes() {
    let state = state_with_steps(2);
    assert_eq!(Signature::of(&state, 72), Signature::of(&state, 72));
}

#[test]
fn every_kind_of_history_change_moves_the_signature() {
    let mut state = state_with_steps(2);
    let mut seen = vec![Signature::of(&state, 72)];
    let mut expect_new = |state: &EditorState, what: &str| {
        let now = Signature::of(state, 72);
        assert!(!seen.contains(&now), "{what} left the signature as it was");
        seen.push(now);
    };

    state.undo().expect("undo must work");
    expect_new(&state, "an undo");
    state.redo().expect("redo must work");
    expect_new(&state, "a redo");
    state
        .apply(Edit::SetPreformYOffset { y_offset: 0.9 })
        .expect("edit must apply");
    expect_new(&state, "a new edit");
    state.jump_to(0).expect("jump must work");
    expect_new(&state, "a jump");
    state.design_epoch.fetch_add(1, Ordering::Relaxed);
    expect_new(&state, "a new design");
}

#[test]
fn the_picture_size_moves_the_signature() {
    let state = state_with_steps(1);
    assert_ne!(Signature::of(&state, 72), Signature::of(&state, 144));
}

#[test]
fn rows_grow_change_and_shrink_in_place() {
    let (mut panel, model) = new_panel();

    panel.push_rows(vec![spec(2, "Two"), spec(1, "One"), spec(0, "Start")]);
    assert_eq!(titles(&model), ["Two", "One", "Start"]);

    // A new step on top shifts every row down by one.
    panel.push_rows(vec![
        spec(3, "Three"),
        spec(2, "Two"),
        spec(1, "One"),
        spec(0, "Start"),
    ]);
    assert_eq!(titles(&model), ["Three", "Two", "One", "Start"]);

    // One row's wording changes (a nudge merged into the newest step).
    panel.push_rows(vec![
        spec(3, "Three, further"),
        spec(2, "Two"),
        spec(1, "One"),
        spec(0, "Start"),
    ]);
    assert_eq!(titles(&model), ["Three, further", "Two", "One", "Start"]);

    // A new design: fewer rows than before.
    panel.push_rows(vec![spec(0, "Start")]);
    assert_eq!(titles(&model), ["Start"]);

    panel.push_rows(Vec::new());
    assert_eq!(model.row_count(), 0);
}

#[test]
fn a_sync_lists_the_steps_newest_first_and_ends_with_start() {
    let state = state_with_steps(2);
    let (mut panel, model) = new_panel();

    assert!(panel.sync(&state, 72), "the first look must build the rows");
    let rows: Vec<HistoryRowData> = model.iter().collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().map(|row| row.position).collect::<Vec<_>>(),
        [2, 1, 0]
    );
    assert!(rows[0].current && !rows[1].current && !rows[2].current);
    assert!(rows[2].start);
    assert_eq!(panel.current_position, 2);
    assert_eq!(panel.step_count, 2);
    assert_eq!(panel.keys.len(), 3);
    assert_eq!(panel.keys[0].position, 0);
    assert_eq!(panel.keys[2].position, 2);
    assert!(panel.keys.iter().all(|key| key.edge_px == 72));
}

#[test]
fn a_second_look_at_an_unchanged_history_does_nothing() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();
    assert!(panel.sync(&state, 72));
    let version = panel.version;
    assert!(!panel.sync(&state, 72));
    assert_eq!(
        panel.version, version,
        "pictures must not be asked for again"
    );
}

#[test]
fn undone_steps_stay_listed_and_are_marked() {
    let mut state = state_with_steps(3);
    let (mut panel, model) = new_panel();
    assert!(panel.sync(&state, 72));

    state.jump_to(1).expect("jump must work");
    assert!(panel.sync(&state, 72), "a jump must show");
    let rows: Vec<HistoryRowData> = model.iter().collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter().map(|row| row.undone).collect::<Vec<_>>(),
        [true, true, false, false]
    );
    assert!(rows[2].current, "step 1 is where the design stands");
    assert_eq!(panel.current_position, 1);
    assert_eq!(panel.step_count, 3, "undone steps still count");
}

#[test]
fn a_new_edit_after_a_jump_back_drops_the_undone_rows() {
    let mut state = state_with_steps(3);
    let (mut panel, model) = new_panel();
    state.jump_to(1).expect("jump must work");
    state
        .apply(Edit::SetPreformYOffset { y_offset: 0.9 })
        .expect("edit must apply");

    assert!(panel.sync(&state, 72));
    assert_eq!(model.row_count(), 3);
    assert!(model.iter().all(|row| !row.undone));
}

#[test]
fn a_new_picture_size_rebuilds_the_keys() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();
    assert!(panel.sync(&state, 72));
    assert!(panel.sync(&state, 144));
    assert!(panel.keys.iter().all(|key| key.edge_px == 144));
}

#[test]
fn a_row_without_a_known_step_shows_the_waiting_picture() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();

    // Nothing built yet, so no step has a key.
    assert_eq!(panel.thumbnail(1).state, 0);
    assert!(panel.sync(&state, 72));
    assert_eq!(panel.thumbnail(-1).state, 0);
    assert_eq!(panel.thumbnail(99).state, 0);
}

#[test]
fn asking_twice_for_the_same_picture_queues_it_once() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();
    assert!(panel.sync(&state, 72));
    let key = panel.keys[1].clone();

    assert_eq!(panel.thumbnail(1).state, 0);
    assert!(matches!(panel.cache.lookup(&key), Lookup::Waiting));
    assert_eq!(panel.thumbnail(1).state, 0);
}

#[test]
fn a_requested_picture_comes_back_from_the_thread() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();
    assert!(panel.sync(&state, 72));
    assert_eq!(panel.thumbnail(1).state, 0);

    // The fresh design has no tiers, so there is no solid to draw: the answer is "does not
    // solve" (2), or a picture (1) if the starter design ever grows one -- never "waiting".
    let started = Instant::now();
    let mut answer = 0;
    while answer == 0 && started.elapsed() < Duration::from_secs(20) {
        if panel.collect_finished() {
            answer = panel.thumbnail(1).state;
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert_ne!(answer, 0, "the thread never answered");
}

#[test]
fn finished_pictures_bump_the_version_the_rows_watch() {
    let state = state_with_steps(1);
    let (mut panel, _model) = new_panel();
    assert!(panel.sync(&state, 72));
    let version = panel.version;
    assert!(!panel.collect_finished(), "nothing was asked for yet");
    assert_eq!(panel.version, version);

    assert_eq!(panel.thumbnail(0).state, 0);
    let started = Instant::now();
    while !panel.collect_finished() && started.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_ne!(panel.version, version, "the rows would never ask again");
}

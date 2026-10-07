//! The walks of the History, Variants, Snapshot and Compare and Edit as Text lessons, and the
//! premises they state about the design.

use super::{RICH, Sim, act, events, read, walk};
use crate::{
    guide::StartingState,
    raw_text::{apply_plan, diff_lines, generate_text, plan_apply},
    snapshot::diff_row_view,
};
use indicatrix_cut_core::{Design, Edit, ScheduleState, diff_tiers};

/// The text of Edit as Text for `design`, as the dialog first shows it.
fn text_of(design: &Design) -> String {
    let solved = design.solve().expect("the design solves");
    generate_text(design, &solved, &[]).expect("the design writes")
}

/// The lesson's text edit: the `Crown_Main` line's angle changed from 34.5 to 36, planned and
/// applied the way Apply does it.
fn edit_crown_as_text(sim: &mut Sim) {
    let design = sim.session.design.clone();
    let base = text_of(&design);
    let edited = base.replace("a 34.5 ", "a 36 ");
    assert_ne!(base, edited, "the text has a line whose angle is 34.5");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("the text can be used");
    assert!(!plan.is_noop(), "the edit changes the design");
    apply_plan(&mut sim.session, plan).expect("the text applies");
    sim.solved = false;
    // The dialog's Compare with... > Snapshot, as a line-by-line list.
    sim.raise(events::RAW_TEXT_DIFF_SHOWN);
}

#[test]
fn the_history_lesson_can_be_played() {
    walk(
        "solving-history",
        vec![
            act(|sim| sim.open_tab(4)),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(|sim| sim.set_angle("Pavilion Main", "-41")),
            // The row under the top one: the step after the crown change.
            act(|sim| sim.jump(1)),
            // The Start row.
            act(|sim| sim.jump(0)),
            // The top row, the newest step.
            act(|sim| sim.jump(2)),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn two_changes_are_two_rows_and_a_jump_dims_the_ones_it_goes_past() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    assert!(
        sim.session.history_entries().is_empty(),
        "a new design has only Start"
    );
    sim.set_angle("Crown Main", "36");
    sim.set_angle("Pavilion Main", "-41");
    let entries = sim.session.history_entries();
    assert_eq!(entries.len(), 2, "one row for each change");
    assert!(entries.iter().all(|entry| !entry.undone));
    sim.jump(1);
    let entries = sim.session.history_entries();
    assert_eq!(entries.len(), 2, "a jump loses nothing");
    assert!(
        !entries[0].undone && entries[1].undone,
        "the top row reads undone"
    );
    assert_eq!(sim.session.history_position(), 1);
}

#[test]
fn a_new_change_after_a_jump_drops_the_steps_that_were_gone_past() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    sim.set_angle("Crown Main", "36");
    sim.set_angle("Pavilion Main", "-41");
    sim.jump(1);
    sim.set_angle("Crown Main", "37");
    assert_eq!(
        sim.session.history_entries().len(),
        2,
        "the undone pavilion step is dropped from the list"
    );
    assert!(
        sim.session
            .history_entries()
            .iter()
            .all(|entry| !entry.undone)
    );
}

#[test]
fn the_variants_lesson_can_be_played() {
    walk(
        "solving-variants",
        vec![
            act(|sim| {
                sim.open_tab(4);
                sim.raise(events::VARIANTS_OPENED);
            }),
            act(|sim| {
                let design = sim.session.design.clone();
                sim.variants.push(design);
                sim.raise(events::VARIANT_SAVED);
            }),
            act(|sim| sim.set_angle("Crown Main", "38")),
            act(|sim| {
                let design = sim.session.design.clone();
                sim.variants.push(design);
                sim.raise(events::VARIANT_SAVED);
            }),
            // Open on the older row: the variant's whole schedule replaces the design's.
            act(|sim| {
                let state = ScheduleState::of(&sim.variants[0]);
                sim.apply(Edit::ReplaceSchedule(Box::new(state)));
            }),
            act(Sim::undo),
            act(|sim| sim.raise(events::VARIANTS_COMPARED)),
            read(),
        ],
    );
}

#[test]
fn opening_a_variant_is_one_undo_step_and_brings_the_crown_back() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let first = ScheduleState::of(&sim.session.design);
    sim.set_angle("Crown Main", "38");
    sim.apply(Edit::ReplaceSchedule(Box::new(first)));
    let crown = sim.row("Crown Main");
    assert!((sim.session.design.tiers[crown].angle_deg - 34.5).abs() < 1e-9);
    sim.undo();
    assert!((sim.session.design.tiers[crown].angle_deg - 38.0).abs() < 1e-9);
}

#[test]
fn the_snapshot_lesson_can_be_played() {
    walk(
        "solving-snapshot-compare",
        vec![
            act(|sim| sim.raise(events::SNAPSHOT_TAKEN)),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(|sim| sim.raise(events::COMPARE_OPENED)),
            act(|sim| sim.raise(events::COMPARE_WINDOW_OPENED)),
            read(),
        ],
    );
}

#[test]
fn the_compare_table_reads_changed_for_the_crown_and_same_for_the_rest() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let before = sim.session.design.clone();
    let before_solved = before.solve().expect("the snapshot solves");
    sim.set_angle("Crown Main", "36");
    let after = sim.session.design.clone();
    let after_solved = after.solve().expect("the design solves");
    let rows: Vec<_> = diff_tiers(
        &before.tiers,
        Some(before_solved.as_slice()),
        &after.tiers,
        Some(after_solved.as_slice()),
    )
    .iter()
    .map(diff_row_view)
    .collect();
    assert_eq!(rows.len(), after.tiers.len());
    for row in &rows {
        let wanted = if row.name.trim().eq_ignore_ascii_case("Crown Main") {
            "Changed"
        } else {
            "Same"
        };
        assert_eq!(row.status_label, wanted, "{}", row.name);
    }
}

#[test]
fn the_raw_text_lesson_can_be_played() {
    walk(
        "solving-raw-text",
        vec![
            act(|sim| sim.raise(events::SNAPSHOT_TAKEN)),
            act(edit_crown_as_text),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn the_text_has_the_line_the_lesson_tells_the_learner_to_edit() {
    let sim = Sim::start(&StartingState::Template(RICH));
    let text = text_of(&sim.session.design);
    let crown_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("Crown_Main"))
        .collect();
    assert_eq!(crown_lines.len(), 1, "one line holds Crown_Main:\n{text}");
    assert!(
        crown_lines[0].starts_with("a 34.5 "),
        "the angle is the first number after the a: {}",
        crown_lines[0]
    );
    assert_eq!(
        text.matches("a 34.5 ").count(),
        1,
        "no other tier starts with that angle"
    );
}

#[test]
fn the_text_comparison_shows_the_changed_line_as_added_and_removed() {
    let sim = Sim::start(&StartingState::Template(RICH));
    let base = text_of(&sim.session.design);
    let edited = base.replace("a 34.5 ", "a 36 ");
    let diff = diff_lines(&base, &edited);
    assert_eq!(diff.added, 1, "the new line is marked added");
    assert_eq!(diff.removed, 1, "the old line is marked removed");
    assert!(diff.same > 0, "the rest is unchanged and folded");
}

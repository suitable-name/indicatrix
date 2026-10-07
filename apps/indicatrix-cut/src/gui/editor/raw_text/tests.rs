//! Tests of the pure half of "Edit as Text" ([`super::logic`]): what the live check says
//! about an edited text, and the rows of the comparison. The merge itself and the diff are
//! tested in `indicatrix_editor::raw_text`.

use super::logic::{
    KIND_NEUTRAL, KIND_PROBLEM, KIND_READY, Outcome, PanelView, RowView, UNCHANGED_SENTENCE,
    compare_failure_view, compare_title, compare_view, design_moved_problem, evaluate, panel_view,
    same_lines,
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    loading::design_from_asc_text,
    raw_text::{diff_lines, generate_text},
};

/// Five tiers on a 96-tooth gear, all pinned to their file masts; line 7 of the text it
/// generates is the first crown tier.
const FIXTURE: &str = "GemCad 5.0\n\
    g 96 0.0\n\
    y 4 y\n\
    I 1.54\n\
    H Test stone\n\
    a 0 0.32 n Table\n\
    a 34.5 0.59 0 24 48 72 n C1\n\
    a 41 0.67 12 36 60 84 n C2 G Cut to TCP\n\
    a -41 0.67 0 24 48 72 n P1\n\
    a -0 -0.88 n Culet\n\
    F Hand cut only\n";

fn fixture() -> Design {
    design_from_asc_text("test.asc", FIXTURE, None)
        .expect("the fixture parses")
        .design
}

/// The text the dialog opens with.
fn text_of(design: &Design) -> String {
    let solved = design.solve().expect("every tier is pinned");
    generate_text(design, &solved, &[]).expect("the design writes")
}

#[test]
fn line_endings_and_the_last_line_break_do_not_make_texts_differ() {
    assert!(same_lines("a\nb", "a\r\nb\r\n"));
    assert!(same_lines("a\nb\n", "a\nb"));
    assert!(!same_lines("a\nb", "a\nb\nc"));
    assert!(!same_lines("a\nb", "a\nB"));
}

#[test]
fn a_text_nobody_edited_is_unchanged_and_cannot_be_applied() {
    let design = fixture();
    let base = text_of(&design);
    assert_eq!(evaluate(&design, &base, &base, &[]), Outcome::Unchanged);
    let crlf = base.replace('\n', "\r\n");
    assert_eq!(evaluate(&design, &base, &crlf, &[]), Outcome::Unchanged);

    let view = panel_view(&Outcome::Unchanged);
    assert_eq!(view.status, UNCHANGED_SENTENCE);
    assert_eq!(view.kind, KIND_NEUTRAL);
    assert!(!view.can_apply);
    assert!(view.changed.is_empty() && view.lost.is_empty());
}

#[test]
fn a_valid_edit_is_ready_to_apply_and_says_what_changes() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a 34.5 ", "a 35 ");
    assert_ne!(edited, base, "the generated text has the crown line");

    let outcome = evaluate(&design, &base, &edited, &[]);
    let Outcome::Plan(plan) = &outcome else {
        panic!("a plain angle edit is a plan, got {outcome:?}");
    };
    assert!(!plan.is_noop());

    let view = panel_view(&outcome);
    assert_eq!(view.kind, KIND_READY);
    assert!(view.can_apply);
    assert_ne!(view.changed, Vec::<String>::new(), "{view:?}");
    assert!(view.status.contains("Undo"), "{}", view.status);
}

#[test]
fn a_bad_number_is_a_red_sentence_with_its_line_and_no_apply() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a 34.5 ", "a x ");

    let outcome = evaluate(&design, &base, &edited, &[]);
    assert!(matches!(outcome, Outcome::Problem(_)), "{outcome:?}");

    let view = panel_view(&outcome);
    assert_eq!(view.kind, KIND_PROBLEM);
    assert!(!view.can_apply);
    assert!(view.status.starts_with("Line 7: "), "{}", view.status);
    assert!(view.changed.is_empty() && view.lost.is_empty());
}

#[test]
fn an_angle_outside_the_range_is_refused_with_its_line() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a 34.5 ", "a 95 ");

    let view = panel_view(&evaluate(&design, &base, &edited, &[]));
    assert_eq!(view.kind, KIND_PROBLEM, "{}", view.status);
    assert!(view.status.starts_with("Line 7: "), "{}", view.status);
    assert!(!view.can_apply);
}

#[test]
fn blank_lines_change_the_text_but_not_the_design() {
    let design = fixture();
    let base = text_of(&design);
    let spaced = base.replace('\n', "\n\n");
    assert!(!same_lines(&base, &spaced));

    let outcome = evaluate(&design, &base, &spaced, &[]);
    let Outcome::Plan(plan) = &outcome else {
        panic!("blank lines still make a valid text, got {outcome:?}");
    };
    assert!(plan.is_noop());

    let view = panel_view(&outcome);
    assert_eq!(view.kind, KIND_NEUTRAL);
    assert!(!view.can_apply, "there is nothing to apply");
    assert_eq!(view.changed, Vec::<String>::new());
}

#[test]
fn a_text_written_for_another_design_is_refused() {
    let design = fixture();
    let base = text_of(&design);
    let other = design_from_asc_text(
        "other.asc",
        "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na 0 0.32 n Table\n",
        None,
    )
    .expect("the other design parses")
    .design;
    let edited = base.replace("a 34.5 ", "a 35 ");

    let view = panel_view(&evaluate(&other, &base, &edited, &[]));
    assert_eq!(view.kind, KIND_PROBLEM);
    assert!(view.status.contains("no longer matches"), "{}", view.status);
    assert!(!view.can_apply);
}

#[test]
fn a_design_that_moved_on_asks_for_a_revert() {
    let view = panel_view(&Outcome::Problem(design_moved_problem()));
    assert_eq!(view.kind, KIND_PROBLEM);
    assert!(view.status.contains("Revert"), "{}", view.status);
    assert!(!view.can_apply);
    assert_eq!(PanelView::problem("x").status, "x");
    assert_eq!(PanelView::empty().status, "");
}

#[test]
fn the_comparison_folds_unchanged_runs_and_numbers_each_side() {
    let lines: Vec<String> = (1..=10).map(|n| format!("line {n}")).collect();
    let old = lines.join("\n") + "\n";
    let new = old.replace("line 5", "changed 5");
    let diff = diff_lines(&old, &new);
    let view = compare_view("the saved file", &diff);

    assert_eq!(
        view.title,
        "Comparing the saved file with the text in the box"
    );
    assert_eq!(
        view.summary,
        "1 line added, 1 line removed, 9 lines unchanged. Added lines are only in the text \
         box, removed lines only in the saved file."
    );
    let kinds: Vec<i32> = view.rows.iter().map(|row| row.kind).collect();
    assert_eq!(kinds, vec![3, 0, 0, 0, 2, 1, 0, 0, 0, 3]);
    assert_eq!(view.rows[0].text, "1 unchanged line");
    assert_eq!(view.rows[9].text, "2 unchanged lines");

    let removed = &view.rows[4];
    assert_eq!(
        (removed.old_line.as_str(), removed.new_line.as_str()),
        ("5", "")
    );
    assert_eq!(removed.text, "line 5");
    let added = &view.rows[5];
    assert_eq!(
        (added.old_line.as_str(), added.new_line.as_str()),
        ("", "5")
    );
    assert_eq!(added.text, "changed 5");
    let after = &view.rows[6];
    assert_eq!(
        (after.old_line.as_str(), after.new_line.as_str()),
        ("6", "6")
    );
}

#[test]
fn identical_texts_say_so_and_keep_every_line() {
    let diff = diff_lines("a\nb\n", "a\nb\n");
    let view = compare_view("the snapshot", &diff);
    assert_eq!(view.summary, "The two texts are identical.");
    let kinds: Vec<i32> = view.rows.iter().map(|row| row.kind).collect();
    assert_eq!(kinds, vec![0, 0]);
}

#[test]
fn a_comparison_that_cannot_be_made_says_why() {
    let view = compare_failure_view("the snapshot", "It does not solve.");
    assert_eq!(view.title, "Cannot compare with the snapshot");
    assert_eq!(view.summary, "It does not solve.");
    assert_eq!(view.rows, Vec::<RowView>::new());
    assert_eq!(
        compare_title("the snapshot"),
        "Comparing the snapshot with the text in the box"
    );
}

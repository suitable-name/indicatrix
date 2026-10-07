//! Tests for the Variants view, none of which needs a window.
//!
//! They cover the rows and compare boxes, the forms, the text comparison, the branch book,
//! opening a variant as one undo step, and the library calls on an in-memory database.

use super::{
    Identity,
    actions::{capture_from, refusal_text},
    apply::{open_label, replacement_edit},
    branch::Branch,
    capture::{self, PICTURE_EDGE_PX},
    diff::{self, compare_designs, diff_view, failure_view},
    form::{self, Form},
    rows::{self, Choice},
    store::{self, LoadProblem, SaveRequest},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, History, MaterialSelection, PreformSpec, ScheduleMeta,
    design::{ConcaveTier, ConcaveTool, RelationError, ToolMotion},
};
use indicatrix_editor::{EditorSession, raw_text::diff_lines, session::SessionEditError};
use indicatrix_vault::{db::sqlite::Database, model::design_variant::VariantSummary};

const UUID_A: &str = "6f1c2b7e-0a1d-4c8e-9b21-3d5e7f9a1c4b";
const UUID_B: &str = "0f9e8d7c-6b5a-4938-8271-605f4e3d2c1b";

// --- fixtures ------------------------------------------------------------------------------

/// The standard round brilliant, named Diamond.
fn round_brilliant() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        ..MaterialSelection::default()
    };
    design
}

/// [`round_brilliant`] with the crown mains steepened from 34.5 to 37 degrees.
fn steeper_crown() -> Design {
    let mut design = round_brilliant();
    let main = design
        .tiers
        .iter_mut()
        .find(|tier| tier.name == "Crown Main")
        .expect("the standard round brilliant has a Crown Main tier");
    main.angle_deg = 37.0;
    design
}

/// [`steeper_crown`] that also differs in material, rough, girdle size and rough offset.
fn changed_everywhere() -> Design {
    let mut design = steeper_crown();
    design.material = MaterialSelection {
        name: Some("Ruby".to_string()),
        ..MaterialSelection::default()
    };
    design.preform = PreformSpec::block(2.2, 1.0, 2.0);
    design.girdle_diameter_mm = Some(7.0);
    design.preform_y_offset = 0.05;
    design
}

/// [`round_brilliant`] with the Crown Main at `crown_main` degrees and the Upper Girdle
/// following it (`Upper Girdle = Crown Main + 6.5`).
fn linked_brilliant(crown_main: f64) -> Design {
    let mut design = round_brilliant();
    let relation = design
        .parse_relation("[Crown Main] + 6.5")
        .expect("the relation reads");
    let upper_girdle = design.tier_ids[3];
    design.tier_relations.insert(upper_girdle, relation);
    design.tiers[2].angle_deg = crown_main;
    design.tiers[3].angle_deg = crown_main + 6.5;
    design
}

/// A design with no scale-reference anchor at all: it cannot solve.
fn unsolvable_design() -> Design {
    let mut design = Design::fresh(PreformSpec::block(2.0, 1.0, 2.0), 96, 4, 1.62);
    design.tiers.push(ConstraintTier {
        angle_deg: 30.0,
        name: "A".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetExisting,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    });
    design
}

fn memory_db() -> Database {
    Database::new(Some(":memory:")).expect("in-memory database")
}

fn summary(
    id: i64,
    name: &str,
    created_at: i64,
    parent: Option<i64>,
    note: Option<&str>,
) -> VariantSummary {
    VariantSummary {
        variant_id: id,
        design_uuid: UUID_A.to_owned(),
        name: name.to_owned(),
        parent_variant_id: parent,
        created_at,
        note: note.map(str::to_owned),
    }
}

/// Saves `design` as a variant of `uuid` and returns its id.
fn save(
    db: &Database,
    uuid: &str,
    name: &str,
    design: &Design,
    parent: Option<i64>,
    now: i64,
) -> i64 {
    let text = capture::design_text(design).expect("the design can be written");
    store::save(
        db,
        &SaveRequest {
            uuid,
            name,
            note: None,
            design_text: &text,
            picture_png: None,
            parent,
            now,
        },
    )
    .expect("the variant is saved")
}

fn session_of(design: Design) -> EditorSession {
    EditorSession::with_history(design, History::new())
}

// --- the list ------------------------------------------------------------------------------

#[test]
fn variants_are_listed_newest_first_and_say_where_they_came_from() {
    let list = vec![
        summary(1, "First", 100, None, None),
        summary(2, "Second", 200, Some(1), Some("Steeper crown")),
        summary(3, "Third", 300, Some(9), None),
    ];
    let built = rows::build_rows(&list, Some(2), 300);
    let ids: Vec<i64> = built.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![3, 2, 1]);
    // A parent that is not in the list (deleted) says nothing.
    assert_eq!(built[0].parent, "");
    assert_eq!(built[1].parent, "from: First");
    assert_eq!(built[2].parent, "");
    assert_eq!(built[1].note, "Steeper crown");
    assert_eq!(built[0].note, "");
    // Only the variant the design came from is marked.
    let marked: Vec<bool> = built.iter().map(|row| row.opened_from).collect();
    assert_eq!(marked, vec![false, true, false]);
}

#[test]
fn equal_times_are_ordered_by_id_newest_first() {
    let list = vec![
        summary(4, "A", 500, None, None),
        summary(5, "B", 500, None, None),
    ];
    let ids: Vec<i64> = rows::build_rows(&list, None, 500)
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, vec![5, 4]);
}

#[test]
fn ages_read_as_plain_words() {
    let now = 10_000_000;
    assert_eq!(rows::age_text(now, now), "Just now");
    assert_eq!(rows::age_text(now, now - 59), "Just now");
    assert_eq!(rows::age_text(now, now - 60), "1 minute ago");
    assert_eq!(rows::age_text(now, now - 120), "2 minutes ago");
    assert_eq!(rows::age_text(now, now - 3_600), "1 hour ago");
    assert_eq!(rows::age_text(now, now - 7_300), "2 hours ago");
    assert_eq!(rows::age_text(now, now - 86_400), "1 day ago");
    assert_eq!(rows::age_text(now, now - 3 * 86_400), "3 days ago");
    // A clock set back reads as just now.
    assert_eq!(rows::age_text(now, now + 500), "Just now");
}

#[test]
fn an_old_variant_shows_its_date_and_the_hover_text_is_exact() {
    // 2024-03-05 06:07:08 UTC.
    let created = 1_709_618_828;
    let much_later = created + 60 * 86_400;
    assert_eq!(rows::age_text(much_later, created), "5 Mar 2024");
    assert_eq!(rows::exact_text(created), "2024-03-05 06:07:08 UTC");
    assert_eq!(rows::exact_text(0), "1970-01-01 00:00:00 UTC");
}

#[test]
fn the_offered_name_is_never_one_already_used() {
    assert_eq!(rows::default_name(&[]), "Variant 1");
    let two = vec![
        summary(1, "Variant 1", 1, None, None),
        summary(2, "Variant 2", 2, None, None),
    ];
    assert_eq!(rows::default_name(&two), "Variant 3");
    // A higher number wins over the count.
    let seven = vec![summary(1, "Variant 7", 1, None, None)];
    assert_eq!(rows::default_name(&seven), "Variant 8");
    // Other names count as variants too.
    let named = vec![summary(1, "Final", 1, None, None)];
    assert_eq!(rows::default_name(&named), "Variant 2");
    // Case does not make a name new.
    let lower = vec![summary(1, "variant 2", 1, None, None)];
    assert_eq!(rows::default_name(&lower), "Variant 3");
}

#[test]
fn names_and_notes_are_cleaned_and_limited() {
    assert_eq!(
        rows::clean_name("  Steeper crown "),
        Ok("Steeper crown".to_owned())
    );
    assert!(rows::clean_name("   ").is_err());
    assert!(rows::clean_name(&"x".repeat(rows::NAME_LIMIT + 1)).is_err());
    assert!(rows::clean_name(&"x".repeat(rows::NAME_LIMIT)).is_ok());
    assert_eq!(rows::clean_note("  "), Ok(None));
    assert_eq!(
        rows::clean_note(" low light "),
        Ok(Some("low light".to_owned()))
    );
    assert!(rows::clean_note(&"x".repeat(rows::NOTE_LIMIT + 1)).is_err());
}

// --- the compare boxes ---------------------------------------------------------------------

#[test]
fn the_compare_boxes_offer_the_current_design_then_the_variants_newest_first() {
    let list = vec![
        summary(1, "Old", 100, None, None),
        summary(2, "New", 200, None, None),
    ];
    let (choices, labels) = rows::build_choices(&list);
    assert_eq!(
        choices,
        vec![Choice::Current, Choice::Variant(2), Choice::Variant(1)]
    );
    assert_eq!(labels, vec!["Current design", "New", "Old"]);
}

#[test]
fn equal_variant_names_are_told_apart_in_the_boxes() {
    let list = vec![
        summary(1, "Same", 100, None, None),
        summary(2, "Same", 200, None, None),
        summary(3, "Current design", 300, None, None),
    ];
    let (_, labels) = rows::build_choices(&list);
    let distinct: std::collections::BTreeSet<&String> = labels.iter().collect();
    assert_eq!(distinct.len(), labels.len(), "{labels:?}");
    assert!(
        labels[2].contains("saved 1970-01-01 00:03:20 UTC"),
        "{labels:?}"
    );
    assert!(labels[1].contains("Current design"), "{labels:?}");
}

#[test]
fn two_variants_with_one_name_saved_in_the_same_second_get_their_numbers() {
    let list = vec![
        summary(7, "Same", 100, None, None),
        summary(8, "Same", 100, None, None),
    ];
    let (_, labels) = rows::build_choices(&list);
    let distinct: std::collections::BTreeSet<&String> = labels.iter().collect();
    assert_eq!(distinct.len(), labels.len(), "{labels:?}");
    assert!(
        labels.iter().any(|label| label.contains("number 7")),
        "{labels:?}"
    );
}

#[test]
fn the_boxes_keep_their_designs_when_the_entries_change() {
    let (before, _) = rows::build_choices(&[
        summary(1, "A", 100, None, None),
        summary(2, "B", 200, None, None),
    ]);
    // Boxes: current design (0) and A (2).
    let (after, _) = rows::build_choices(&[
        summary(1, "A", 100, None, None),
        summary(2, "B", 200, None, None),
        summary(3, "C", 300, None, None),
    ]);
    // Entries are now: current, C, B, A. A moved from 2 to 3.
    assert_eq!(rows::reselect(&before, 0, 2, &after), (0, 3));
}

#[test]
fn a_box_whose_design_was_deleted_falls_back_and_the_two_never_agree() {
    let (before, _) = rows::build_choices(&[
        summary(1, "A", 100, None, None),
        summary(2, "B", 200, None, None),
    ]);
    // The second box was on B (index 1); B is gone.
    let (after, _) = rows::build_choices(&[summary(1, "A", 100, None, None)]);
    assert_eq!(rows::reselect(&before, 0, 1, &after), (0, 1));
    // Both boxes on the same entry cannot happen: the second moves away.
    let (only, _) = rows::build_choices(&[]);
    let (first, second) = rows::reselect(&only, 0, 0, &only);
    assert_eq!((first, second), (0, 0), "a lone entry can only name itself");
    let (two, _) = rows::build_choices(&[summary(1, "A", 100, None, None)]);
    let (first, second) = rows::reselect(&two, 1, 1, &two);
    assert_ne!(first, second);
}

#[test]
fn a_pair_to_compare_needs_two_different_entries() {
    let (choices, _) = rows::build_choices(&[summary(1, "A", 100, None, None)]);
    assert_eq!(
        rows::resolve_pair(&choices, 0, 1),
        Ok((Choice::Current, Choice::Variant(1)))
    );
    assert_eq!(
        rows::resolve_pair(&choices, 1, 1),
        Err("Choose two different designs to compare.")
    );
    assert_eq!(
        rows::resolve_pair(&choices, 0, 5),
        Err("Choose the two designs to compare.")
    );
    assert_eq!(
        rows::resolve_pair(&choices, -1, 0),
        Err("Choose the two designs to compare.")
    );
}

// --- the forms -----------------------------------------------------------------------------

#[test]
fn each_form_has_its_own_code_and_the_closed_form_is_the_default() {
    assert_eq!(Form::default(), Form::Closed);
    assert_eq!(Form::Closed.code(), 0);
    assert_eq!(
        Form::Save {
            position: None,
            revision: None,
            design: (UUID_A.to_owned(), 0),
        }
        .code(),
        1
    );
    assert_eq!(Form::Rename(3).code(), 2);
    assert_eq!(Form::Note(3).code(), 3);
    assert_eq!(Form::Delete(3).code(), 4);
}

#[test]
fn the_save_form_says_which_design_it_keeps() {
    let now = form::save_view(None, "Variant 1".to_owned());
    assert_eq!(now.name, "Variant 1");
    assert_eq!(now.note, "");
    assert_eq!(now.hint, "Keeps the design as it is now.");
    let start = form::save_view(Some((0, "")), "Variant 2".to_owned());
    assert!(start.hint.contains("at the start"), "{}", start.hint);
    let step = form::save_view(Some((3, "Set P1 angle to 41.0 degrees")), "V".to_owned());
    assert!(step.hint.contains("step 3"), "{}", step.hint);
    assert!(
        step.hint.contains("Set P1 angle to 41.0 degrees"),
        "{}",
        step.hint
    );
}

#[test]
fn the_other_forms_start_from_the_variant() {
    let variant = summary(5, "Steeper", 100, None, Some("Crown 37"));
    let rename = form::rename_view(&variant);
    assert_eq!(rename.name, "Steeper");
    assert_eq!(rename.ok, "Rename");
    let note = form::note_view(&variant);
    assert_eq!(note.note, "Crown 37");
    assert!(note.title.contains("Steeper"), "{}", note.title);
    let delete = form::delete_view(&variant);
    assert_eq!(delete.ok, "Delete");
    assert!(delete.hint.contains("Steeper"), "{}", delete.hint);
    assert!(delete.hint.contains("cannot be undone"), "{}", delete.hint);
    assert!(
        delete.hint.contains("current design is not changed"),
        "{}",
        delete.hint
    );
}

// --- the branch book -----------------------------------------------------------------------

#[test]
fn a_branch_belongs_to_one_design_and_one_opening() {
    let mut branch = Branch::default();
    assert_eq!(branch.get(UUID_A, 0), None);
    branch.set(UUID_A, 0, 7);
    assert_eq!(branch.get(UUID_A, 0), Some(7));
    // Another design, or the same file opened again, has none.
    assert_eq!(branch.get(UUID_B, 0), None);
    assert_eq!(branch.get(UUID_A, 1), None);
    // The newest wins.
    branch.set(UUID_A, 0, 9);
    assert_eq!(branch.get(UUID_A, 0), Some(9));
}

#[test]
fn deleting_the_branch_variant_forgets_it_and_other_deletes_do_not() {
    let mut branch = Branch::default();
    branch.set(UUID_A, 2, 7);
    branch.forget(8);
    assert_eq!(branch.get(UUID_A, 2), Some(7));
    branch.forget(7);
    assert_eq!(branch.get(UUID_A, 2), None);
}

// --- the text comparison -------------------------------------------------------------------

#[test]
fn the_same_text_is_reported_as_the_same() {
    let text = "a\nb\nc\n";
    let view = diff_view("One", "Two", &diff_lines(text, text));
    assert!(
        view.summary
            .starts_with("The cutting instructions are the same."),
        "{}",
        view.summary
    );
    assert!(view.rows.iter().all(|row| row.kind == 0));
    assert_eq!(view.title, diff::title("One", "Two"));
}

#[test]
fn changed_lines_are_marked_and_long_unchanged_runs_are_folded() {
    let old = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let new = old.replace("line 10\n", "line ten\n");
    let view = diff_view("Before", "After", &diff_lines(&old, &new));
    let kinds: Vec<i32> = view.rows.iter().map(|row| row.kind).collect();
    assert!(kinds.contains(&1), "an added line: {kinds:?}");
    assert!(kinds.contains(&2), "a removed line: {kinds:?}");
    assert!(kinds.contains(&3), "a folded run: {kinds:?}");
    let added = view.rows.iter().find(|row| row.kind == 1).unwrap();
    assert_eq!(added.text, "line ten");
    assert_eq!(added.old_line, "");
    assert_eq!(added.new_line, "10");
    let removed = view.rows.iter().find(|row| row.kind == 2).unwrap();
    assert_eq!(removed.text, "line 10");
    assert_eq!(removed.old_line, "10");
    assert_eq!(removed.new_line, "");
    assert!(view.summary.contains("\"After\""), "{}", view.summary);
    assert!(view.summary.contains("\"Before\""), "{}", view.summary);
}

#[test]
fn the_cutting_instructions_of_two_designs_are_compared() {
    let view = compare_designs(
        (&round_brilliant(), "Current design"),
        (&steeper_crown(), "Steeper"),
        &[],
    );
    assert!(
        view.rows.iter().any(|row| row.kind == 1),
        "{:?}",
        view.summary
    );
    assert!(
        view.rows.iter().any(|row| row.kind == 2),
        "{:?}",
        view.summary
    );
    assert!(view.summary.contains("added"), "{}", view.summary);
    // The same design twice is the same.
    let same = compare_designs(
        (&round_brilliant(), "Current design"),
        (&round_brilliant(), "Copy"),
        &[],
    );
    assert!(
        same.summary
            .starts_with("The cutting instructions are the same."),
        "{}",
        same.summary
    );
}

#[test]
fn a_design_that_does_not_solve_gives_a_view_that_says_so() {
    let view = compare_designs(
        (&round_brilliant(), "Current design"),
        (&unsolvable_design(), "Broken"),
        &[],
    );
    assert_eq!(view.rows.len(), 0);
    assert!(view.summary.contains("\"Broken\""), "{}", view.summary);
    assert!(view.summary.contains("does not solve"), "{}", view.summary);
    let plain = failure_view("A", "B", "Nothing to show.");
    assert_eq!(plain.summary, "Nothing to show.");
    assert_eq!(plain.rows.len(), 0);
}

// --- opening a variant ---------------------------------------------------------------------

#[test]
fn the_same_design_needs_no_edit() {
    assert_eq!(
        replacement_edit(&round_brilliant(), &round_brilliant()),
        Ok(None)
    );
}

#[test]
fn opening_a_variant_is_one_undo_step_and_undo_puts_everything_back() {
    let current = round_brilliant();
    let variant = steeper_crown();
    let edit = replacement_edit(&current, &variant)
        .expect("the variant can be put in place")
        .expect("the designs differ");
    let mut session = session_of(current.clone());
    session.try_apply(edit).expect("the session takes the edit");
    assert_eq!(session.design, variant);
    assert_eq!(session.history_entries().len(), 1, "one step");
    assert_eq!(session.history_position(), 1);

    session
        .undo()
        .expect("undo works")
        .expect("a step was undone");
    assert_eq!(session.design, current, "undo is exact");
    assert_eq!(session.history_position(), 0);

    session
        .redo()
        .expect("redo works")
        .expect("a step was redone");
    assert_eq!(session.design, variant, "redo is exact");
}

#[test]
fn a_variant_that_differs_in_rough_material_and_girdle_is_still_one_step() {
    let current = round_brilliant();
    let variant = changed_everywhere();
    let edit = replacement_edit(&current, &variant)
        .expect("the variant can be put in place")
        .expect("the designs differ");
    let mut session = session_of(current.clone());
    session.try_apply(edit).expect("the session takes the edit");
    assert_eq!(session.design, variant);
    assert_eq!(session.history_entries().len(), 1, "one step");
    session
        .undo()
        .expect("undo works")
        .expect("a step was undone");
    assert_eq!(session.design, current);
}

/// The history says what the cutter did, not what the edit is made of: a plain variant is a
/// `ReplaceSchedule` ("Edit instructions as text"), one that also changes the material or
/// the rough is a batch ("N combined edits").
#[test]
fn opening_a_variant_is_worded_as_that_in_the_history() {
    assert_eq!(
        open_label("Steeper crown"),
        "Open variant \"Steeper crown\""
    );
    assert_eq!(open_label("  Padded "), "Open variant \"Padded\"");

    for (variant, name) in [
        (steeper_crown(), "Steeper crown"),
        (changed_everywhere(), "Everything"),
    ] {
        let current = round_brilliant();
        let edit = replacement_edit(&current, &variant)
            .expect("the variant can be put in place")
            .expect("the designs differ");
        let mut session = session_of(current.clone());
        session
            .try_apply_mapped(edit, Some(&open_label(name)))
            .expect("the session takes the edit");
        let words = open_label(name);
        let entries = session.history_entries();
        assert_eq!(entries.len(), 1, "one step");
        assert_eq!(entries[0].label, words);
        assert_eq!(
            session.history.description_log(),
            std::slice::from_ref(&words)
        );
        assert_eq!(session.undo_hint(), words, "the Undo hint says so too");

        // The words survive undo and redo.
        session.undo().expect("undo works").expect("undone");
        assert_eq!(session.design, current);
        assert_eq!(session.history_entries()[0].label, words);
        assert_eq!(session.redo_hint(), words);
        session.redo().expect("redo works").expect("redone");
        assert_eq!(session.design, variant);
        assert_eq!(session.history_entries()[0].label, words);
    }
}

#[test]
fn opening_a_variant_after_other_edits_keeps_the_earlier_history() {
    let base = round_brilliant();
    let mut session = session_of(base.clone());
    let first = replacement_edit(&base, &steeper_crown())
        .expect("accepted")
        .expect("differs");
    session.try_apply(first).expect("applied");
    let second = replacement_edit(&session.design, &changed_everywhere())
        .expect("accepted")
        .expect("differs");
    session.try_apply(second).expect("applied");
    assert_eq!(session.history_entries().len(), 2);
    session.undo().expect("undo").expect("undone");
    assert_eq!(session.design, steeper_crown());
    session.undo().expect("undo").expect("undone");
    assert_eq!(session.design, base);
}

fn identity(uuid: Option<&str>, epoch: u64) -> Identity {
    Identity {
        has_design: uuid.is_some(),
        uuid: uuid.map(str::to_owned),
        epoch,
    }
}

#[test]
fn an_identity_is_the_design_it_was_opened_as_and_nothing_else() {
    let open = identity(Some(UUID_A), 3);
    assert!(open.is_design(UUID_A, 3));
    assert!(!open.is_design(UUID_B, 3), "another design");
    assert!(!open.is_design(UUID_A, 4), "the same file opened again");
    assert!(!identity(None, 3).is_design(UUID_A, 3), "no design open");
}

/// Revisions count from zero in every new history, so step 1 of design A and step 1 of design
/// B can be the very same `(position, revision)`. The save form must not take one for the
/// other.
#[test]
fn a_step_form_opened_for_one_design_does_not_save_a_step_of_another() {
    let base = round_brilliant();
    let edit = replacement_edit(&base, &steeper_crown())
        .expect("accepted")
        .expect("differs");
    let mut session = session_of(base);
    session.try_apply(edit).expect("applied");
    let revision = session.history_entries()[0].revision;

    // The form was opened on design A (epoch 1) at its step 1.
    let opened_for = (UUID_A, 1);
    let saved = capture_from(
        &session,
        &identity(Some(UUID_A), 1),
        Some(1),
        Some(revision),
        opened_for,
    )
    .expect("the same design and step are saved");
    assert_eq!(saved.uuid, UUID_A);
    assert_eq!(saved.epoch, 1);
    assert_eq!(saved.design, steeper_crown());

    // Another design opened since: its step 1 has the same revision number, and is refused.
    for (now, why) in [
        (identity(Some(UUID_B), 2), "another design"),
        (identity(Some(UUID_A), 2), "the same file opened again"),
    ] {
        let error = capture_from(&session, &now, Some(1), Some(revision), opened_for)
            .map(|saved| saved.uuid)
            .unwrap_err();
        assert!(error.contains("The design changed"), "{why}: {error}");
    }
    // A whole-design save is refused for the same reason: the form said "as it is now".
    assert!(capture_from(&session, &identity(Some(UUID_B), 2), None, None, opened_for).is_err());
    // With no design open there is nothing to save.
    assert_eq!(
        capture_from(&session, &identity(None, 0), None, None, opened_for)
            .map(|saved| saved.uuid)
            .unwrap_err(),
        "Open or create a design first."
    );
    // The same design with a step that has since been replaced still says so.
    let error = capture_from(
        &session,
        &identity(Some(UUID_A), 1),
        Some(1),
        Some(revision + 100),
        opened_for,
    )
    .map(|saved| saved.uuid)
    .unwrap_err();
    assert!(error.contains("history changed"), "{error}");
}

#[test]
fn history_steps_and_the_current_design_can_both_be_kept() {
    let base = round_brilliant();
    let mut session = session_of(base.clone());
    let edit = replacement_edit(&base, &steeper_crown())
        .expect("accepted")
        .expect("differs");
    session.try_apply(edit).expect("applied");
    assert_eq!(
        capture::design_at_step(&session, None).expect("now"),
        steeper_crown()
    );
    assert_eq!(
        capture::design_at_step(&session, Some(1)).expect("the last step"),
        steeper_crown()
    );
    assert_eq!(
        capture::design_at_step(&session, Some(0)).expect("the start"),
        base
    );
    assert!(capture::design_at_step(&session, Some(9)).is_err());
    // The session itself did not move.
    assert_eq!(session.design, steeper_crown());
    assert_eq!(session.history_position(), 1);
}

#[test]
fn a_refusal_is_explained_in_plain_words() {
    let error =
        SessionEditError::Relation(RelationError::Cycle(vec!["C1".to_owned(), "C2".to_owned()]));
    assert_eq!(
        refusal_text(&error),
        "The design cannot take this variant. C1 and C2 refer to each other in a loop."
    );
}

/// A variant is opened as one undo step, and the open design equals it afterwards, undo and
/// redo included.
fn assert_opens_exactly(current: &Design, variant: &Design) {
    let edit = replacement_edit(current, variant)
        .expect("the variant can be put in place")
        .expect("the designs differ");
    let mut session = session_of(current.clone());
    session
        .try_apply(edit)
        .expect("a variant brings its own relations, so none of the open design's blocks it");
    assert_eq!(&session.design, variant);
    assert_eq!(session.history_entries().len(), 1, "one step");
    session
        .undo()
        .expect("undo works")
        .expect("a step was undone");
    assert_eq!(&session.design, current, "undo is exact");
    session
        .redo()
        .expect("redo works")
        .expect("a step was redone");
    assert_eq!(&session.design, variant, "redo is exact");
}

#[test]
fn a_variant_that_moves_a_tier_following_a_relation_opens_with_its_relations() {
    // The open design and the variant both have Upper Girdle = Crown Main + 6.5, at other
    // angles: the variant moves a driven tier.
    let open = linked_brilliant(34.5);
    let variant = linked_brilliant(37.0);
    assert_eq!(variant.tiers[3].angle_deg, 43.5);
    assert_opens_exactly(&open, &variant);
}

#[test]
fn a_variant_may_drop_or_bring_a_relation() {
    let open = linked_brilliant(34.5);
    // The same tier is free in the variant, at an angle of its own.
    let mut free = steeper_crown();
    free.tiers[3].angle_deg = 45.0;
    assert_opens_exactly(&open, &free);
    // And the other way round: the open design has none, the variant has one.
    assert_opens_exactly(&free, &open);
    assert_opens_exactly(&round_brilliant(), &linked_brilliant(37.0));
}

#[test]
fn a_variant_with_a_different_relation_on_the_same_tier_opens_with_that_one() {
    let open = linked_brilliant(34.5);
    let mut variant = linked_brilliant(34.5);
    let relation = variant
        .parse_relation("[Crown Main] + 8")
        .expect("the relation reads");
    let id = variant.tier_ids[3];
    variant.tier_relations.insert(id, relation);
    variant.tiers[3].angle_deg = 42.5;
    assert_opens_exactly(&open, &variant);
}

#[test]
fn a_variant_whose_relations_cannot_hold_is_refused_and_changes_nothing() {
    let open = linked_brilliant(34.5);
    let mut variant = linked_brilliant(34.5);
    // Crown Main + 60 would be 94.5 degrees.
    let relation = variant
        .parse_relation("[Crown Main] + 60")
        .expect("the relation reads");
    let id = variant.tier_ids[3];
    variant.tier_relations.insert(id, relation);
    variant.tiers[3].angle_deg = 41.0;

    let edit = replacement_edit(&open, &variant)
        .expect("the edit itself is fine")
        .expect("the designs differ");
    let mut session = session_of(open.clone());
    let error = session.try_apply(edit).unwrap_err();
    let text = refusal_text(&error);
    assert!(
        text.starts_with("The design cannot take this variant. "),
        "{text}"
    );
    assert_eq!(session.design, open, "nothing changed");
    assert_eq!(session.history_entries().len(), 0);
}

/// [`round_brilliant`] with one concave tier, `Groove`, holding `indices`.
fn grooved_brilliant(indices: &[f64]) -> Design {
    let mut design = round_brilliant();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: ConcaveTier {
                name: "Groove".to_owned(),
                angle_deg: -40.0,
                indices: indices.to_vec(),
                instructions: String::new(),
                tool: ConcaveTool::Cylinder,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.0, 0.1],
                diameter_ratio: 0.5,
                tool_angle_deg: None,
                motion: ToolMotion::Reciprocating,
            },
        })
        .expect("the groove adds");
    design
}

/// A variant on a smaller gear is one `ReplaceSchedule` when its concave tiers are the open
/// design's own, and that edit obeys the gear rule: it opens when the concave indices fit the
/// smaller wheel. A variant file never holds a concave index off its own wheel (it does not
/// load), but if one did, the edit refuses it instead of writing a design that will not reopen.
#[test]
fn a_variant_on_a_smaller_gear_needs_its_concave_indices_to_fit_the_wheel() {
    let open = grooved_brilliant(&[0.0, 50.0]);
    let mut smaller = open.clone();
    smaller.meta.gear_teeth = 64;
    assert_opens_exactly(&open, &smaller);

    let open = grooved_brilliant(&[0.0, 80.0]);
    let mut smaller = open.clone();
    smaller.meta.gear_teeth = 64;
    let text = replacement_edit(&open, &smaller).expect_err("80 is off a 64-tooth gear");
    assert!(
        text.starts_with("The design cannot take this variant. "),
        "{text}"
    );
}

// --- the design text and the picture -------------------------------------------------------

#[test]
fn a_design_written_as_text_reads_back_the_same() {
    let design = steeper_crown();
    let text = capture::design_text(&design).expect("written");
    let back = capture::design_from_text(&text).expect("read");
    assert_eq!(back.tiers.len(), design.tiers.len());
    for (read, written) in back.tiers.iter().zip(&design.tiers) {
        assert_eq!(read.name, written.name);
        assert_eq!(read.angle_deg.to_bits(), written.angle_deg.to_bits());
        assert_eq!(read.indices, written.indices);
    }
    assert_eq!(back.preform, design.preform);
    assert_eq!(back.material, design.material);
    // Writing what was read changes nothing, so a variant stays what it was saved as.
    assert_eq!(capture::design_text(&back).expect("written again"), text);
}

#[test]
fn text_that_is_not_a_design_is_refused_in_words() {
    let error = capture::design_from_text("this is not a design").unwrap_err();
    assert!(
        error.starts_with("The saved variant could not be read."),
        "{error}"
    );
}

#[test]
fn a_solving_design_gets_a_picture_with_a_clear_background() {
    let png = capture::picture_png(&round_brilliant()).expect("the design draws");
    let pixels = capture::decode_png(&png).expect("the picture reads back");
    assert_eq!(pixels.width(), PICTURE_EDGE_PX);
    assert_eq!(pixels.height(), PICTURE_EDGE_PX);
    let bytes = pixels.as_bytes();
    assert_eq!(bytes[3], 0, "the corner is transparent");
    assert!(
        bytes.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0),
        "the stone is drawn"
    );
}

#[test]
fn a_design_that_does_not_solve_has_no_picture_and_a_damaged_picture_reads_as_none() {
    assert!(capture::picture_png(&unsolvable_design()).is_none());
    assert!(capture::decode_png(b"not a png").is_none());
}

// --- the library ---------------------------------------------------------------------------

#[test]
fn a_saved_variant_comes_back_as_the_design_that_was_saved() {
    let db = memory_db();
    let design = steeper_crown();
    let text = capture::design_text(&design).expect("written");
    let id = store::save(
        &db,
        &SaveRequest {
            uuid: UUID_A,
            name: "Steeper",
            note: Some("Crown at 37"),
            design_text: &text,
            picture_png: None,
            parent: None,
            now: 1_000,
        },
    )
    .expect("saved");

    let (found, loaded) = store::load_design(&db, UUID_A, id).expect("loaded");
    assert_eq!(found.variant_id, id);
    assert_eq!(found.name, "Steeper");
    assert_eq!(found.note.as_deref(), Some("Crown at 37"));
    assert_eq!(found.created_at, 1_000);
    assert_eq!(found.parent_variant_id, None);
    assert_eq!(loaded.tiers.len(), design.tiers.len());
    assert_eq!(capture::design_text(&loaded).expect("written"), text);
}

#[test]
fn the_list_holds_only_the_variants_of_the_open_design() {
    let db = memory_db();
    let one = save(&db, UUID_A, "A one", &round_brilliant(), None, 10);
    let two = save(&db, UUID_A, "A two", &steeper_crown(), None, 20);
    let other = save(&db, UUID_B, "B one", &round_brilliant(), None, 30);

    let a: Vec<i64> = store::list(&db, UUID_A)
        .expect("listed")
        .iter()
        .map(|variant| variant.variant_id)
        .collect();
    assert_eq!(a, vec![one, two]);
    let b: Vec<i64> = store::list(&db, UUID_B)
        .expect("listed")
        .iter()
        .map(|variant| variant.variant_id)
        .collect();
    assert_eq!(b, vec![other]);
    // The key is the UUID however it is written.
    let upper = store::list(&db, &UUID_A.to_uppercase()).expect("listed");
    assert_eq!(upper.len(), 2);
    // A design with no variants has an empty list, and a name that is no UUID is an error.
    assert_eq!(
        store::list(&db, "0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a")
            .expect("listed")
            .len(),
        0
    );
    assert!(store::list(&db, "not a uuid").is_err());
}

#[test]
fn a_new_variant_remembers_the_one_it_was_made_from() {
    let db = memory_db();
    let first = save(&db, UUID_A, "First", &round_brilliant(), None, 10);
    let second = save(&db, UUID_A, "Second", &steeper_crown(), Some(first), 20);
    // A second branch from the first.
    let third = save(&db, UUID_A, "Third", &changed_everywhere(), Some(first), 30);

    let list = store::list(&db, UUID_A).expect("listed");
    let parent_of = |id: i64| {
        list.iter()
            .find(|variant| variant.variant_id == id)
            .and_then(|variant| variant.parent_variant_id)
    };
    assert_eq!(parent_of(first), None);
    assert_eq!(parent_of(second), Some(first));
    assert_eq!(parent_of(third), Some(first));

    let built = rows::build_rows(&list, Some(third), 40);
    let line = |id: i64| {
        built
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.parent.clone())
            .unwrap_or_default()
    };
    assert_eq!(line(second), "from: First");
    assert_eq!(line(third), "from: First");
    assert_eq!(line(first), "");
}

#[test]
fn a_parent_that_is_gone_is_dropped_instead_of_refusing_the_save() {
    let db = memory_db();
    let first = save(&db, UUID_A, "First", &round_brilliant(), None, 10);
    store::delete(&db, first).expect("deleted");
    let second = save(&db, UUID_A, "Second", &steeper_crown(), Some(first), 20);
    let list = store::list(&db, UUID_A).expect("listed");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].variant_id, second);
    assert_eq!(list[0].parent_variant_id, None);
}

#[test]
fn a_parent_of_another_design_is_dropped_too() {
    let db = memory_db();
    let foreign = save(&db, UUID_B, "Foreign", &round_brilliant(), None, 10);
    let mine = save(&db, UUID_A, "Mine", &steeper_crown(), Some(foreign), 20);
    let list = store::list(&db, UUID_A).expect("listed");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].variant_id, mine);
    assert_eq!(list[0].parent_variant_id, None);
}

#[test]
fn a_variant_of_another_design_is_not_opened() {
    let db = memory_db();
    let theirs = save(&db, UUID_B, "Theirs", &round_brilliant(), None, 10);
    assert_eq!(
        store::load_design(&db, UUID_A, theirs).unwrap_err(),
        LoadProblem::OtherDesign
    );
    assert_eq!(
        store::load_design(&db, UUID_A, theirs + 100).unwrap_err(),
        LoadProblem::Missing
    );
    assert_eq!(
        LoadProblem::Missing.to_string(),
        "That variant no longer exists."
    );
    assert_eq!(
        LoadProblem::OtherDesign.to_string(),
        "That variant belongs to another design."
    );
}

#[test]
fn renaming_changing_a_note_and_deleting_work_and_say_when_the_variant_is_gone() {
    let db = memory_db();
    let id = save(&db, UUID_A, "Before", &round_brilliant(), None, 10);

    store::rename(&db, id, "  After  ").expect("renamed");
    store::set_note(&db, id, Some("A note")).expect("note set");
    let list = store::list(&db, UUID_A).expect("listed");
    assert_eq!(list[0].name, "After");
    assert_eq!(list[0].note.as_deref(), Some("A note"));

    store::set_note(&db, id, None).expect("note cleared");
    assert_eq!(store::list(&db, UUID_A).expect("listed")[0].note, None);

    store::delete(&db, id).expect("deleted");
    assert_eq!(store::list(&db, UUID_A).expect("listed").len(), 0);
    // Deleting twice is fine; the others say the variant is gone.
    store::delete(&db, id).expect("a variant that is gone is not an error");
    assert_eq!(
        store::rename(&db, id, "Again"),
        Err("That variant no longer exists.".to_owned())
    );
    assert_eq!(
        store::set_note(&db, id, Some("Again")),
        Err("That variant no longer exists.".to_owned())
    );
}

#[test]
fn an_empty_name_is_refused_by_the_library_too() {
    let db = memory_db();
    let id = save(&db, UUID_A, "Name", &round_brilliant(), None, 10);
    assert!(store::rename(&db, id, "   ").is_err());
}

#[test]
fn the_picture_is_kept_with_its_variant_and_listed_with_it() {
    let db = memory_db();
    let design = round_brilliant();
    let text = capture::design_text(&design).expect("written");
    let png = capture::picture_png(&design).expect("drawn");
    let with = store::save(
        &db,
        &SaveRequest {
            uuid: UUID_A,
            name: "With picture",
            note: None,
            design_text: &text,
            picture_png: Some(&png),
            parent: None,
            now: 10,
        },
    )
    .expect("saved");
    let without = save(&db, UUID_A, "Without picture", &design, None, 20);

    // The read under the library's lock hands back the stored bytes; decoding comes after.
    let raw = store::list_with_png(&db, UUID_A).expect("listed");
    assert_eq!(raw.len(), 2);
    assert!(
        raw.iter()
            .any(|(variant, stored)| variant.variant_id == with
                && stored.as_deref() == Some(png.as_slice())),
        "the picture is stored and read back byte for byte"
    );
    assert!(
        raw.iter()
            .any(|(variant, stored)| variant.variant_id == without && stored.is_none())
    );
    let listed = store::decode_pictures(raw);
    assert_eq!(listed.len(), 2);
    let picture_of = |id: i64| {
        listed
            .iter()
            .find(|(variant, _)| variant.variant_id == id)
            .and_then(|(_, picture)| picture.as_ref())
    };
    let found = picture_of(with).expect("the picture comes back");
    assert_eq!(found.width(), PICTURE_EDGE_PX);
    assert!(picture_of(without).is_none());
}

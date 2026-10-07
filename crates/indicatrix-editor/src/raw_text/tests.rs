//! Tests of the raw text view: generating, parsing, merging and diffing.

use super::*;
use crate::{loading::design_from_asc_text, session::EditorSession};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    Edit, History, ScheduleState, TierId,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};

/// Five tiers on a 96-tooth gear: a table, two crown tiers (the second with a stated
/// instruction), a pavilion main and a culet, all pinned to their file masts. Line 6 of
/// the text it generates is the table, line 10 the culet.
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

fn text_of(design: &Design) -> String {
    let solved = design.solve().expect("every tier is pinned");
    generate_text(design, &solved, &[]).expect("the design writes")
}

fn groove() -> ConcaveTier {
    ConcaveTier {
        name: "Groove".to_owned(),
        angle_deg: -40.0,
        indices: vec![0.0, 24.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.0, 0.1],
        diameter_ratio: 0.5,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

fn names(state: &ScheduleState) -> Vec<&str> {
    state.tiers.iter().map(|tier| tier.name.as_str()).collect()
}

fn any_line(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|line| line.contains(needle))
}

#[test]
fn generated_text_is_what_export_writes() {
    let design = fixture();
    let text = text_of(&design);
    assert!(text.starts_with("GemCad 5.0\n"), "{text}");
    assert!(!text.contains('\r'), "line feeds only");
    let parsed = parse_text(&text).expect("the generated text parses");
    assert_eq!(parsed.schedule.tiers.len(), 5);
    assert_eq!(parsed.tier_lines, vec![6, 7, 8, 9, 10]);
    assert_eq!(parsed.schedule.footnotes, vec!["Hand cut only".to_owned()]);
    assert_eq!(parsed.schedule.headers, vec!["Test stone".to_owned()]);
    assert!(
        text.contains("a 41 0.67 12 n C2 36 60 84 G Cut to TCP\n"),
        "{text}"
    );
}

#[test]
fn generating_with_the_wrong_number_of_masts_says_so() {
    let design = fixture();
    assert!(generate_text(&design, &[], &[]).is_err());
}

#[test]
fn tier_lines_skip_comments_blank_lines_and_wrapped_lines() {
    let text = "GemCad 5.0\ng 96 0\ny 4 y\nI 1.54\n\n; a note\na 34.5 0.59 0 24\n 48 72 n C1\na -41 0.67 0 24 48 72 n P1\n";
    let parsed = parse_text(text).expect("parses");
    assert_eq!(parsed.tier_lines, vec![7, 9]);
    assert_eq!(
        parsed.schedule.tiers[0].indices,
        vec![0.0, 24.0, 48.0, 72.0]
    );
}

#[test]
fn a_bad_number_names_its_line_and_the_parsers_words() {
    let text = text_of(&fixture()).replace("a 34.5 0.59", "a x 0.59");
    let problem = parse_text(&text).expect_err("x is not an angle");
    assert_eq!(problem.line, Some(7));
    assert!(problem.message.contains("not numeric"), "{problem}");
    assert!(problem.to_string().starts_with("Line 7: "), "{problem}");
}

#[test]
fn a_missing_header_line_is_a_problem_without_a_line() {
    let text = text_of(&fixture()).replace("g 96 0\n", "");
    let problem = parse_text(&text).expect_err("no gear line");
    assert_eq!(problem.line, None);
    assert!(problem.message.contains("gear"), "{problem}");
    assert_eq!(problem.to_string(), problem.message);
    let empty = parse_text("  \n").expect_err("empty");
    assert_eq!(empty.line, None);
}

#[test]
fn normalizing_rewrites_line_endings_and_layout() {
    let crlf = "GemCad 5.0\r\ng 96 0.0\r\ny 4 y\r\nI 1.54\r\na 0 0.32 n Table\r\n";
    let text = normalize_text(crlf).expect("normalizes");
    assert!(!text.contains('\r'));
    assert!(text.contains("g 96 0\n"), "{text}");
    assert!(normalize_text("not an asc file").is_err());
}

#[test]
fn unchanged_text_plans_nothing() {
    let design = fixture();
    let base = text_of(&design);
    let plan = plan_apply(&design, &base, &base, &[]).expect("plans");
    assert!(plan.is_noop());
    assert_eq!(plan.state, ScheduleState::of(&design));
    assert!(plan.report.changed.is_empty() && plan.report.lost.is_empty());
}

#[test]
fn an_angle_edit_changes_only_that_tier() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a 34.5 0.59", "a 35 0.59");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert!(!plan.is_noop());
    assert_eq!(plan.state.tiers[1].angle_deg, 35.0);
    let mut expected = design.tiers.clone();
    expected[1].angle_deg = 35.0;
    assert_eq!(plan.state.tiers, expected);
    assert_eq!(plan.state.tier_ids, design.tier_ids);
    assert!(any_line(&plan.report.changed, "C1"), "{:?}", plan.report);
    assert!(plan.report.lost.is_empty(), "{:?}", plan.report);
}

#[test]
fn a_depth_edit_pins_the_tier_and_keeps_its_instruction_text() {
    let mut design = fixture();
    let base = text_of(&design);
    // The design followed a meet rule when the text was written.
    design.tiers[2].constraint = MeetConstraint::MeetNamed(vec!["C1".to_owned()]);
    let edited = base.replace("a 41 0.67", "a 41 0.7");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    let tier = &plan.state.tiers[2];
    assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.7));
    assert_eq!(tier.original_notes.as_deref(), Some("Cut to TCP"));
    assert!(
        any_line(&plan.report.changed, "Cut depth"),
        "{:?}",
        plan.report
    );
    assert!(
        any_line(&plan.report.lost, "meet rule"),
        "{:?}",
        plan.report
    );
    assert_eq!(plan.state.tiers[1], design.tiers[1]);
}

#[test]
fn typing_a_meet_instruction_records_it_for_adoption() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("n C2 36 60 84 G Cut to TCP", "n C2 36 60 84 G Meet C1, P1");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    let tier = &plan.state.tiers[2];
    assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.67));
    assert_eq!(
        tier.imported_meet,
        Some(MeetConstraint::MeetNamed(vec![
            "C1".to_owned(),
            "P1".to_owned()
        ]))
    );
    assert_eq!(tier.original_notes.as_deref(), Some("Meet C1, P1"));
    assert!(
        any_line(&plan.report.changed, "Instruction text"),
        "{:?}",
        plan.report
    );
}

#[test]
fn a_rename_keeps_the_tier_and_its_id() {
    let mut design = fixture();
    design.tier_notes.insert(3, "grind slowly".to_owned());
    let base = text_of(&design);
    let edited = base.replace("n P1", "n Q1");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(names(&plan.state), ["Table", "C1", "C2", "Q1", "Culet"]);
    assert_eq!(plan.state.tier_ids, design.tier_ids);
    assert_eq!(
        plan.state.tier_notes.get(&3).map(String::as_str),
        Some("grind slowly")
    );
    assert!(
        any_line(&plan.report.changed, "P1 to Q1"),
        "{:?}",
        plan.report
    );
}

#[test]
fn a_rename_and_an_angle_change_on_one_line_still_continue_the_tier() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a -41 0.67 0 n P1", "a -42 0.67 0 n Q1");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(
        plan.state.tier_ids, design.tier_ids,
        "same position, same tier"
    );
    assert_eq!(plan.state.tiers[3].angle_deg, -42.0);
    assert_eq!(plan.state.tiers[3].name, "Q1");
}

#[test]
fn removing_a_line_removes_the_tier_and_moves_the_notes_of_the_rest() {
    let mut design = fixture();
    design.tier_notes.insert(2, "check the meet".to_owned());
    design.tier_notes.insert(3, "grind slowly".to_owned());
    design.cheater_offsets_deg.insert(3, 1.0);
    let base = text_of(&design);
    let edited = base.replace("a 41 0.67 12 n C2 36 60 84 G Cut to TCP\n", "");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(names(&plan.state), ["Table", "C1", "P1", "Culet"]);
    assert_eq!(
        plan.state.tier_ids,
        vec![
            design.tier_ids[0],
            design.tier_ids[1],
            design.tier_ids[3],
            design.tier_ids[4]
        ]
    );
    assert_eq!(plan.state.tier_notes.len(), 1);
    assert_eq!(
        plan.state.tier_notes.get(&2).map(String::as_str),
        Some("grind slowly")
    );
    assert_eq!(plan.state.cheater_offsets_deg.get(&2), Some(&1.0));
    assert!(any_line(&plan.report.lost, "C2"), "{:?}", plan.report);
    assert!(any_line(&plan.report.lost, "1 note"), "{:?}", plan.report);
}

#[test]
fn a_new_line_adds_a_tier_with_a_fresh_id_pinned_to_its_depth() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("F Hand cut only", "a 45 0.5 6 n X1\nF Hand cut only");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(plan.state.tiers.len(), 6);
    assert_eq!(plan.state.tier_ids[5], TierId(design.next_tier_id));
    assert_eq!(plan.state.next_tier_id, design.next_tier_id + 1);
    let added = &plan.state.tiers[5];
    assert_eq!(added.name, "X1");
    assert_eq!(added.angle_deg, 45.0);
    assert_eq!(added.indices, vec![6.0]);
    assert_eq!(added.constraint, MeetConstraint::ScaleReference(0.5));
    assert_eq!(added.original_notes.as_deref(), Some(""));
    assert!(any_line(&plan.report.changed, "X1"), "{:?}", plan.report);
}

#[test]
fn moving_a_line_moves_the_tier_with_its_data() {
    let mut design = fixture();
    design.tier_notes.insert(1, "first crown".to_owned());
    let base = text_of(&design);
    let mut lines: Vec<&str> = base.lines().collect();
    lines.swap(6, 7);
    let edited = lines.join("\n") + "\n";
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(names(&plan.state), ["Table", "C2", "C1", "P1", "Culet"]);
    assert_eq!(plan.state.tier_ids[1], design.tier_ids[2]);
    assert_eq!(plan.state.tier_ids[2], design.tier_ids[1]);
    assert_eq!(
        plan.state.tier_notes.get(&2).map(String::as_str),
        Some("first crown")
    );
    assert!(any_line(&plan.report.changed, "order"), "{:?}", plan.report);
}

#[test]
fn an_edited_depth_drops_the_depth_target_of_that_tier() {
    use indicatrix_cut_core::TierTarget;
    let mut design = fixture();
    // The text is written before the target exists: resolving a depth target needs a
    // finished stone, which this fixture is not.
    let base = text_of(&design);
    let id = design.tier_ids[1];
    design.tier_targets.insert(id, TierTarget::DepthMm(3.0));
    let kept =
        plan_apply(&design, &base, &base.replace("a 41 0.67", "a 42 0.67"), &[]).expect("plans");
    assert_eq!(kept.state.tier_targets.len(), 1, "another tier changed");
    let dropped = plan_apply(
        &design,
        &base,
        &base.replace("a 34.5 0.59", "a 34.5 0.6"),
        &[],
    )
    .expect("plans");
    assert!(dropped.state.tier_targets.is_empty());
    assert!(
        any_line(&dropped.report.lost, "depth target"),
        "{:?}",
        dropped.report
    );
}

#[test]
fn a_driven_tiers_angle_in_the_text_is_ignored_and_said_so() {
    let mut design = fixture();
    let base = text_of(&design);
    let relation = design
        .parse_relation("C1 + 6.5")
        .expect("the relation reads");
    let id = design.tier_ids[2];
    design.tier_relations.insert(id, relation);
    let edited = base.replace("a 41 0.67", "a 42 0.67");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(plan.state.tiers[2].angle_deg, 41.0);
    assert!(plan.state.tier_relations.contains_key(&id));
    assert!(any_line(&plan.report.lost, "relation"), "{:?}", plan.report);
}

#[test]
fn applying_a_text_keeps_a_relation_and_ignores_the_angle_typed_for_its_tier() {
    let mut design = fixture();
    let base = text_of(&design);
    let relation = design
        .parse_relation("C1 + 6.5")
        .expect("the relation reads");
    let id = design.tier_ids[2];
    design.tier_relations.insert(id, relation);
    let before = design.clone();
    // The text moves the driver C1 to 35 and also types 50 for C2, which follows C1.
    let edited = base
        .replace("a 34.5 0.59", "a 35 0.59")
        .replace("a 41 0.67", "a 50 0.67");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    let mut session = EditorSession::with_history(design, History::new());

    let applied = apply_plan(&mut session, plan).expect("the session takes the plan");
    assert!(applied.is_some());
    assert_eq!(session.design.tiers[1].angle_deg, 35.0);
    // C2 followed its driver; the angle in the text did not count.
    assert_eq!(session.design.tiers[2].angle_deg, 41.5);
    assert!(session.is_driven(2));
    assert_eq!(session.history_entries().len(), 1, "one undo step");
    assert!(session.undo().expect("undo works").is_some());
    assert_eq!(session.design, before);
}

#[test]
fn removing_a_tier_a_relation_reads_drops_the_relation() {
    let mut design = fixture();
    let base = text_of(&design);
    let relation = design
        .parse_relation("C1 + 6.5")
        .expect("the relation reads");
    let id = design.tier_ids[2];
    design.tier_relations.insert(id, relation);
    let edited = base.replace("a 34.5 0.59 0 n C1 24 48 72\n", "");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert!(plan.state.tier_relations.is_empty());
    assert!(any_line(&plan.report.lost, "relation"), "{:?}", plan.report);
}

#[test]
fn changing_the_index_list_drops_detached_marks_that_are_gone() {
    let mut design = fixture();
    let base = text_of(&design);
    design.tiers[1].detached = vec![0.0, 72.0];
    let edited = base.replace("0 n C1 24 48 72", "0 n C1 24 48");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(plan.state.tiers[1].indices, vec![0.0, 24.0, 48.0]);
    assert_eq!(plan.state.tiers[1].detached, vec![0.0]);
    assert!(any_line(&plan.report.lost, "detached"), "{:?}", plan.report);
}

#[test]
fn a_changed_gear_is_taken_as_written_and_said_so() {
    let design = fixture();
    let base = text_of(&design);
    let plan = plan_apply(&design, &base, &base.replace("g 96 0", "g 120 0"), &[]).expect("plans");
    assert_eq!(plan.state.meta.gear_teeth, 120);
    assert!(any_line(&plan.report.changed, "120"), "{:?}", plan.report);
    assert!(any_line(&plan.report.lost, "rescaled"), "{:?}", plan.report);
}

#[test]
fn a_smaller_gear_that_leaves_an_index_off_the_wheel_is_refused_at_its_line() {
    let design = fixture();
    let base = text_of(&design);
    let problem = plan_apply(&design, &base, &base.replace("g 96 0", "g 80 0"), &[])
        .expect_err("84 is off an 80-tooth gear");
    assert_eq!(problem.line, Some(8));
    assert!(problem.message.contains("80-tooth"), "{problem}");
}

/// A design on the 96 gear with a concave tier `Groove` at index 95. The flat tiers stop at 84,
/// so a gear of 90 leaves only the concave tier off the wheel.
fn design_with_a_groove_at_95() -> Design {
    let mut design = fixture();
    let mut tier = groove();
    tier.indices = vec![0.0, 95.0];
    design
        .apply_edit(Edit::AddConcaveTier { index: 0, tier })
        .expect("the groove adds");
    design
}

#[test]
fn a_smaller_gear_that_leaves_a_concave_index_off_the_wheel_is_refused_at_the_gear_line() {
    let design = design_with_a_groove_at_95();
    let base = text_of(&design);
    let problem = plan_apply(&design, &base, &base.replace("g 96 0", "g 90 0"), &[])
        .expect_err("95 is off a 90-tooth gear");
    assert_eq!(problem.line, Some(2), "the line of the g record");
    assert!(problem.message.contains("concave tier Groove"), "{problem}");
    assert!(problem.message.contains("90-tooth"), "{problem}");
    assert!(problem.message.contains("96 teeth"), "{problem}");
    assert!(problem.message.contains("design settings"), "{problem}");
    assert!(
        problem.to_string().starts_with("Line 2: "),
        "shown at the g line: {problem}"
    );
}

#[test]
fn any_other_gear_is_refused_too_while_the_design_has_concave_tiers() {
    let design = design_with_a_groove_at_95();
    let base = text_of(&design);
    // Smaller gears (95 is off for the flat culet as well as the groove: the concave refusal
    // comes first, so the sentence names the groove), bigger gears and a double one. The
    // concave tiers are not in the text, so none of these can move them, and a bigger gear
    // must not pass in silence either (index 24 is 90 degrees on 96 teeth and 45 on 192).
    for gear in ["g 95 0", "g 90 0", "g 97 0", "g 120 0", "g 192 0"] {
        let edited = base.replace("g 96 0", gear);
        let Err(problem) = plan_apply(&design, &base, &edited, &[]) else {
            panic!("{gear} changes the gear of a design with a groove and must be refused");
        };
        assert_eq!(problem.line, Some(2), "{gear}: at the g line");
        assert!(
            problem
                .message
                .starts_with("The gear cannot change in the text"),
            "{gear}: {problem}"
        );
        assert!(
            problem.message.contains("the concave tier Groove"),
            "{gear}: {problem}"
        );
        assert!(
            problem.message.contains("design settings"),
            "{gear}: {problem}"
        );
        assert!(
            problem.message.contains("keep 96 teeth"),
            "{gear}: {problem}"
        );
    }
}

#[test]
fn the_refusal_names_the_gear_the_text_asks_for_and_every_concave_tier() {
    let mut design = design_with_a_groove_at_95();
    let mut second = groove();
    second.name = "Dimple".to_owned();
    second.indices = vec![0.0, 48.0];
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 1,
            tier: second,
        })
        .expect("the second tier adds");
    design.concave_tiers[0].name = String::new();
    let base = text_of(&design);
    let problem = plan_apply(&design, &base, &base.replace("g 96 0", "g 192 0"), &[])
        .expect_err("a grown gear is refused");
    assert!(problem.message.contains("192-tooth"), "{problem}");
    assert!(
        problem
            .message
            .contains("the concave tiers number 1, Dimple"),
        "{problem}"
    );
}

#[test]
fn a_gear_that_stays_is_not_refused_whatever_the_concave_tiers_say() {
    let mut design = design_with_a_groove_at_95();
    let base = text_of(&design);
    // The same gear, with another line changed: applies.
    let edited = base.replace("H Test stone", "H Test stone, second cut");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("the gear is unchanged");
    let mut session = EditorSession::with_history(design.clone(), History::new());
    assert!(
        apply_plan(&mut session, plan)
            .expect("the session takes it")
            .is_some()
    );
    // A design that is already off its wheel (the settings dialog refuses to make one, a
    // catalogue file may be one) is not made to fail by an unrelated edit.
    design.concave_tiers[0].indices = vec![0.0, 120.0];
    let base = text_of(&design);
    let edited = base.replace("H Test stone", "H Test stone, second cut");
    plan_apply(&design, &base, &edited, &[]).expect("an unrelated edit is not refused");
}

#[test]
fn a_design_without_concave_tiers_still_takes_a_bigger_or_smaller_gear() {
    let design = fixture();
    let base = text_of(&design);
    // 96 is the least gear the fixture's flat tiers (the culet is at index 96) fit on.
    for gear in ["g 96 0", "g 120 0", "g 192 0"] {
        let plan = plan_apply(&design, &base, &base.replace("g 96 0", gear), &[])
            .unwrap_or_else(|problem| panic!("{gear} on a flat design: {problem}"));
        let mut session = EditorSession::with_history(design.clone(), History::new());
        apply_plan(&mut session, plan).expect("the session takes it");
    }
    plan_apply(&design, &base, &base.replace("g 96 0", "g 95 0"), &[])
        .expect_err("the culet at index 96 is off a 95-tooth gear");
}

#[test]
fn a_concave_tier_without_a_name_is_named_by_its_number() {
    let mut design = design_with_a_groove_at_95();
    design.concave_tiers[0].name = String::new();
    let base = text_of(&design);
    let problem = plan_apply(&design, &base, &base.replace("g 96 0", "g 90 0"), &[])
        .expect_err("95 is off a 90-tooth gear");
    assert!(
        problem.message.contains("concave tier number 1"),
        "{problem}"
    );
}

#[test]
fn the_gear_line_is_found_like_the_reader_finds_it() {
    let parsed = parse_text(FIXTURE).expect("parses");
    assert_eq!(parsed.gear_line, Some(2));
    // A comment is not a gear line, a repeated gear line overrides the earlier one, and a
    // keyword glued to its number still counts.
    let text = "; g 12 0\nGemCad 5.0\ng96 0.0\ny 4 y\nI 1.54\ng 80 0\na 0 0.32 n Table\n";
    let parsed = parse_text(text).expect("parses");
    assert_eq!(parsed.schedule.gear_teeth, 80);
    assert_eq!(parsed.gear_line, Some(6));
    let glued =
        parse_text("GemCad 5.0\ng96 0.0\ny 4 y\nI 1.54\na 0 0.32 n Table\n").expect("parses");
    assert_eq!(glued.gear_line, Some(2));
}

#[test]
fn a_line_that_only_starts_like_a_gear_line_is_not_the_gear_line() {
    // The reader ignores `g2a 24 48` (free text after a tier, because `g2a` is no number); a
    // scan that took any `g` plus a digit would have cited line 8 instead of line 2.
    let text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\nH Test\na 0 0.32 n Table\n\
                a 34.5 0.59 0 24 48 72 n C1\n\
                g2a 24 48\n";
    let parsed = parse_text(text).expect("parses");
    assert_eq!(parsed.schedule.gear_teeth, 96);
    assert_eq!(parsed.gear_line, Some(2));
    // The bare `96 0.0` the reader tolerates in place of a `g` line is the gear line too.
    let bare = parse_text("GemCad 5.0\n96 0.0\ny 4 y\nI 1.54\na 0 0.32 n Table\n").expect("parses");
    assert_eq!(bare.schedule.gear_teeth, 96);
    assert_eq!(bare.gear_line, Some(2));
}

#[test]
fn a_refused_gear_change_cites_the_gear_line_even_with_a_look_alike_line_below() {
    let design = design_with_a_groove_at_95();
    let base = text_of(&design);
    let with_look_alike = format!("{}g2a 24 48\n", base.replace("g 96 0", "g 90 0"));
    let problem = plan_apply(&design, &base, &with_look_alike, &[]).expect_err("refused");
    assert_eq!(problem.line, Some(2), "{problem}");
}

#[test]
fn header_and_footnote_lines_are_taken_when_they_change() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base
        .replace("H Test stone", "H Test stone, second cut")
        .replace("F Hand cut only", "F Hand cut only\nF Polish last");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(
        plan.state.meta.headers,
        vec!["Test stone, second cut".to_owned()]
    );
    assert_eq!(
        plan.state.meta.footnotes,
        vec!["Hand cut only".to_owned(), "Polish last".to_owned()]
    );
}

#[test]
fn the_refractive_index_line_changes_the_stored_value() {
    let design = fixture();
    let base = text_of(&design);
    let plan = plan_apply(&design, &base, &base.replace("I 1.54", "I 1.62"), &[]).expect("plans");
    assert_eq!(plan.state.meta.refractive_index, 1.62);
    assert!(plan.report.lost.is_empty(), "no material decides it");
}

#[test]
fn an_angle_outside_the_range_is_refused_at_its_line() {
    let design = fixture();
    let base = text_of(&design);
    let problem = plan_apply(&design, &base, &base.replace("a 41 0.67", "a 95 0.67"), &[])
        .expect_err("95 degrees is out of range");
    assert_eq!(problem.line, Some(8));
    assert!(problem.message.contains("-90 to 90"), "{problem}");
}

#[test]
fn a_parse_error_in_the_edited_text_comes_back_with_its_line() {
    let design = fixture();
    let base = text_of(&design);
    let problem = plan_apply(
        &design,
        &base,
        &base.replace("a 34.5 0.59", "a x 0.59"),
        &[],
    )
    .expect_err("x is not an angle");
    assert_eq!(problem.line, Some(7));
}

#[test]
fn a_base_text_from_another_design_is_refused() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("a 41 0.67 12 n C2 36 60 84 G Cut to TCP\n", "");
    let problem = plan_apply(&design, &edited, &edited, &[]).expect_err("base is stale");
    assert_eq!(problem.line, None);
    assert!(problem.message.contains("Revert"), "{problem}");
}

#[test]
fn lines_the_reader_ignores_are_listed() {
    let design = fixture();
    let base = text_of(&design);
    let edited = base.replace("F Hand cut only", "b 1 2 3\nF Hand cut only");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert!(plan.is_noop(), "an ignored line changes nothing");
    assert!(any_line(&plan.report.lost, "gnored"), "{:?}", plan.report);
}

#[test]
fn a_concave_tier_keeps_its_footnotes_out_of_the_users_and_its_name_to_itself() {
    let mut design = fixture();
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(),
        })
        .expect("the groove adds");
    let base = text_of(&design);
    assert!(base.contains("Groove"), "{base}");

    let unchanged = plan_apply(&design, &base, &base, &[]).expect("plans");
    assert!(
        unchanged.is_noop(),
        "the generated footnotes are not the user's"
    );

    let edited = base.replace("F Hand cut only", "F Hand cut only!");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    assert_eq!(plan.state.meta.footnotes, vec!["Hand cut only!".to_owned()]);

    let clash = plan_apply(&design, &base, &base.replace("n C1", "n Groove"), &[])
        .expect_err("Groove is a concave tier");
    assert_eq!(clash.line, Some(7));
    assert!(clash.message.contains("concave"), "{clash}");
}

#[test]
fn the_omitted_line_counts_what_the_text_cannot_carry() {
    let mut design = fixture();
    assert!(omitted_line(&design).contains("preform"));
    assert!(!omitted_line(&design).contains("note"));
    design.tier_notes.insert(1, "a note".to_owned());
    design.cheater_offsets_deg.insert(2, 1.5);
    design
        .apply_edit(Edit::AddConcaveTier {
            index: 0,
            tier: groove(),
        })
        .expect("the groove adds");
    let line = omitted_line(&design);
    assert!(line.contains("1 tier note"), "{line}");
    assert!(line.contains("1 cheater offset"), "{line}");
    assert!(line.contains("1 concave tier"), "{line}");
}

#[test]
fn applying_a_plan_is_one_undo_step() {
    let design = fixture();
    let before = design.clone();
    let base = text_of(&design);
    let edited = base
        .replace("a 34.5 0.59", "a 35 0.59")
        .replace("n P1", "n Q1");
    let plan = plan_apply(&design, &base, &edited, &[]).expect("plans");
    let mut session = EditorSession::with_history(design, History::new());
    let change = apply_plan(&mut session, plan)
        .expect("the session accepts it")
        .expect("it changed something");
    assert!(!change.tier_count_changed());
    assert_eq!(session.design.tiers[1].angle_deg, 35.0);
    assert_eq!(session.design.tiers[3].name, "Q1");
    assert!(session.undo().expect("undoes").is_some());
    assert_eq!(session.design, before);
    assert!(
        session.undo().expect("nothing more").is_none(),
        "one step only"
    );
}

#[test]
fn applying_a_noop_plan_does_nothing() {
    let design = fixture();
    let base = text_of(&design);
    let plan = plan_apply(&design, &base, &base, &[]).expect("plans");
    let mut session = EditorSession::with_history(design, History::new());
    assert_eq!(apply_plan(&mut session, plan).expect("fine"), None);
    assert!(!session.is_dirty());
}

#[test]
fn a_line_diff_pairs_a_changed_line_as_removed_then_added() {
    let diff = diff_lines("a\nb\nc\n", "a\nx\nc\n");
    assert_eq!((diff.added, diff.removed, diff.same), (1, 1, 2));
    let kinds: Vec<DiffKind> = diff.rows.iter().map(|row| row.kind).collect();
    assert_eq!(
        kinds,
        [
            DiffKind::Same,
            DiffKind::Removed,
            DiffKind::Added,
            DiffKind::Same
        ]
    );
    assert_eq!(diff.rows[1].old_line, Some(2));
    assert_eq!(diff.rows[1].new_line, None);
    assert_eq!(diff.rows[2].old_line, None);
    assert_eq!(diff.rows[2].new_line, Some(2));
    assert_eq!(
        diff.summary(),
        "1 line added, 1 line removed, 2 lines unchanged."
    );
}

#[test]
fn a_line_diff_finds_an_insertion_and_numbers_both_sides() {
    let diff = diff_lines("a\nc\n", "a\nb\nc\n");
    assert_eq!((diff.added, diff.removed, diff.same), (1, 0, 2));
    let added = &diff.rows[1];
    assert_eq!(added.kind, DiffKind::Added);
    assert_eq!(added.new_line, Some(2));
    let last = &diff.rows[2];
    assert_eq!((last.old_line, last.new_line), (Some(2), Some(3)));
}

#[test]
fn identical_texts_say_so() {
    let diff = diff_lines("a\nb\n", "a\nb\n");
    assert!(diff.is_identical());
    assert_eq!(diff.summary(), "The two texts are identical.");
    let empty = diff_lines("", "");
    assert!(empty.is_identical() && empty.rows.is_empty());
    assert_eq!(empty.collapsed(2), Vec::<DiffRow>::new());
}

#[test]
fn a_line_diff_matches_a_shuffled_block_by_longest_common_subsequence() {
    let diff = diff_lines("a\nb\nc\nd\n", "b\nc\nd\na\n");
    assert_eq!((diff.added, diff.removed, diff.same), (1, 1, 3));
}

#[test]
fn collapsing_folds_unchanged_runs_far_from_a_change() {
    let lines: Vec<String> = (1..=20).map(|n| format!("l{n}")).collect();
    let old = lines.join("\n") + "\n";
    let new = old.replace("l10\n", "X\n");
    let diff = diff_lines(&old, &new);
    assert_eq!((diff.added, diff.removed, diff.same), (1, 1, 19));
    let rows = diff.collapsed(2);
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[0].kind, DiffKind::Skipped);
    assert_eq!(rows[0].text, "7 unchanged lines");
    assert_eq!(rows[7].kind, DiffKind::Skipped);
    assert_eq!(rows[7].text, "8 unchanged lines");
    assert_eq!(rows[3].text, "l10");
    assert_eq!(rows[4].text, "X");
}

#[test]
fn a_text_problem_prints_its_line() {
    let problem = TextProblem::at(Some(3), "Something is wrong.");
    assert_eq!(problem.to_string(), "Line 3: Something is wrong.");
    assert_eq!(
        TextProblem::general("Nothing here.").to_string(),
        "Nothing here."
    );
}

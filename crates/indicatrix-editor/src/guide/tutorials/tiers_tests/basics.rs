//! Walks of the lessons in `tiers/basics.rs`.

use super::{Form, Sim, act, guide_named, read, walk};
use crate::view_model::solid_status::status_text_and_is_problem;
use indicatrix_cut_core::Design;

/// What the Indices field says for the eight main facets.
const MAIN_TEXT: &str = "0, 12, 24, 36, 48, 60, 72, 84";

#[test]
fn add_a_tier_builds_the_girdle_pavilion_and_crown() {
    walk(
        "tiers-add-a-tier",
        vec![
            read(),
            act(|sim| sim.add(&Form::new("90", 2, "1", "G1", MAIN_TEXT))),
            act(|sim| sim.add(&Form::new("-41", 1, "G1", "P1", MAIN_TEXT))),
            act(|sim| sim.add(&Form::new("34.5", 1, "G1", "C1", MAIN_TEXT))),
            read(),
        ],
    );
}

#[test]
fn edit_a_tier_changes_the_angle_the_name_and_the_meet_then_undoes() {
    let sim = walk(
        "tiers-edit-a-tier",
        vec![
            act(|sim| sim.edit_tier("Pavilion Main", |form| form.angle = "-41".to_owned())),
            act(|sim| sim.edit_tier("Pavilion Main", |form| form.name = "P1".to_owned())),
            act(|sim| {
                sim.edit_tier("P1", |form| {
                    form.kind = 1;
                    form.text = "Girdle".to_owned();
                });
            }),
            act(Sim::undo),
            read(),
        ],
    );
    assert_eq!(sim.session.design.tiers[sim.row("P1")].angle_deg, -41.0);
}

/// F5-19: the lesson's Meets step makes the Rich Teaching Design's only pavilion tier meet the
/// Girdle, which leaves the pavilion without an exact scale value. That is a solve error the
/// status strip shows, so the step has to say so (and that the next step undoes it); every other
/// state of the lesson solves and closes.
#[test]
fn the_edit_lesson_solves_everywhere_but_the_meets_step_which_says_why() {
    fn assert_solves(sim: &Sim, when: &str) {
        let design = &sim.session.design;
        design
            .solve()
            .unwrap_or_else(|error| panic!("{when}: the design does not solve: {error}"));
        let (strip, is_problem) = status_text_and_is_problem(design);
        assert!(!is_problem, "{when}: the status strip says: {strip}");
    }

    let guide = guide_named("tiers-edit-a-tier");
    assert_eq!(guide.steps[2].title, "Change what it meets");
    let mut sim = Sim::start(&guide.starting_state);
    assert_solves(&sim, "at the start");
    sim.edit_tier("Pavilion Main", |form| form.angle = "-41".to_owned());
    assert_solves(&sim, "after the angle change");
    sim.edit_tier("Pavilion Main", |form| form.name = "P1".to_owned());
    assert_solves(&sim, "after the rename");

    sim.edit_tier("P1", |form| {
        form.kind = 1;
        form.text = "Girdle".to_owned();
    });
    let design = &sim.session.design;
    design
        .solve()
        .expect_err("the pavilion has no exact scale value left");
    let (strip, is_problem) = status_text_and_is_problem(design);
    assert!(is_problem, "the strip reports the failed solve: {strip}");
    assert!(strip.starts_with("Pavilion has no anchor"), "{strip}");
    let step = &guide.steps[2];
    assert!(
        step.check.contains(&strip),
        "the step quotes the strip's sentence {strip:?}, not {:?}",
        step.check
    );
    assert!(
        step.why.contains("expected") && step.why.contains("undoes"),
        "the step says the message is expected and that the next step undoes it: {:?}",
        step.why
    );

    sim.undo();
    assert_solves(&sim, "after Undo");
    assert!(
        guide.steps[3].check.contains("no anchor"),
        "the Undo step says the message is gone: {:?}",
        guide.steps[3].check
    );
}

#[test]
fn index_shorthands_write_the_ring_for_you() {
    walk(
        "tiers-index-shorthands",
        vec![
            read(),
            act(|sim| sim.add(&Form::new("90", 2, "1", "G1", "0:12:96"))),
            act(|sim| sim.add(&Form::new("-41", 1, "G1", "P1", "12 x8"))),
            act(|sim| sim.add(&Form::new("34.5", 1, "G1", "C1", "6 x8"))),
            read(),
        ],
    );
}

#[test]
fn facet_chips_add_remove_detach_rotate_and_mirror() {
    let sim = walk(
        "tiers-facet-chips",
        vec![
            act(|sim| {
                sim.orbit("Crown Main", |design, row| {
                    design.add_orbit_member(row, 6.0)
                });
            }),
            act(|sim| {
                sim.orbit("Crown Main", |design, row| {
                    design.remove_orbit_member(row, 6.0)
                });
            }),
            act(|sim| {
                sim.orbit("Crown Main", |design, row| {
                    design.detach_orbit_member(row, 84.0)
                });
            }),
            act(|sim| {
                sim.orbit("Crown Main", |design, row| {
                    design.remove_orbit_member(row, 84.0)
                });
            }),
            act(|sim| sim.orbit("Crown Main", |design, row| design.rotate_indices(row, 6.0))),
            act(|sim| sim.orbit("Crown Main", Design::mirror_indices)),
        ],
    );
    let crown = &sim.session.design.tiers[sim.row("Crown Main")];
    assert_eq!(crown.indices.len(), 7, "the ring lost one facet");
    assert!(
        crown.detached.is_empty(),
        "the detached facet went with its chip"
    );
}

#[test]
fn quick_add_writes_a_girdle_a_table_and_a_culet() {
    let sim = walk(
        "tiers-quick-add",
        vec![
            act(|sim| sim.add(&Form::new("90", 2, "1", "Girdle", ""))),
            act(|sim| sim.add(&Form::new("0", 2, "0.32", "Table", ""))),
            act(|sim| sim.add(&Form::new("-0", 2, "0.88", "Culet", ""))),
            read(),
        ],
    );
    let culet = &sim.session.design.tiers[sim.row("Culet")];
    assert!(
        culet.angle_deg.is_sign_negative(),
        "the culet keeps its minus sign"
    );
    assert!(culet.indices.is_empty(), "a quick add tier has no indices");
}

#[test]
fn the_inline_angle_cell_edits_and_nudges() {
    walk(
        "tiers-inline-angle",
        vec![
            act(|sim| sim.set_angle("Pavilion Main", "-41")),
            act(|sim| {
                for _ in 0..3 {
                    sim.nudge("Crown Main", 0.1);
                }
            }),
            act(|sim| {
                for _ in 0..3 {
                    sim.nudge("Crown Main", -0.1);
                }
            }),
            read(),
        ],
    );
}

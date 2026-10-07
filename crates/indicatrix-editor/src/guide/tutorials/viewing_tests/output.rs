//! Walks of the lessons in `viewing/output.rs`.

use super::{Sim, act, event, guide_named, read, viewing_events as events, walk};

#[test]
fn cutting_mode_waits_for_a_step_to_be_marked() {
    walk(
        "output-cutting-mode",
        vec![read(), event(events::CUTTING_STEP_MARKED), read()],
    );
}

#[test]
fn each_export_waits_for_its_own_file() {
    walk(
        "output-export-asc",
        vec![read(), event(events::ASC_EXPORTED), read()],
    );
    walk(
        "output-export-gcs",
        vec![read(), event(events::GCS_EXPORTED), read()],
    );
    walk(
        "output-cutting-sheet",
        vec![read(), event(events::SHEET_EXPORTED), read()],
    );
    walk(
        "output-diagram-png",
        vec![read(), event(events::DIAGRAM_EXPORTED), read()],
    );
}

#[test]
fn an_export_is_not_met_by_another_kind_of_export() {
    let lessons = [
        ("output-export-asc", events::ASC_EXPORTED),
        ("output-export-gcs", events::GCS_EXPORTED),
        ("output-cutting-sheet", events::SHEET_EXPORTED),
        ("output-diagram-png", events::DIAGRAM_EXPORTED),
    ];
    for (id, own) in lessons {
        let guide = guide_named(id);
        for (_, other) in lessons.iter().filter(|(_, other)| *other != own) {
            let mut sim = Sim::start(&guide.starting_state);
            sim.fire(other);
            assert!(
                !sim.met(&guide.steps[1].goal),
                "{id} must not be finished by {other}"
            );
        }
    }
}

#[test]
fn saving_first_changes_a_tier_then_saves() {
    let sim = walk(
        "output-save",
        vec![
            act(|sim| sim.drag_angle("Crown Main", 35.0)),
            event(events::DESIGN_SAVED),
            read(),
            read(),
        ],
    );
    let crown = &sim.session.design.tiers[sim.row("Crown Main")];
    assert!(
        (crown.angle_deg - 35.0).abs() < 1e-9,
        "the lesson leaves the change in the design"
    );
}

#[test]
fn the_save_step_is_not_met_by_changing_the_tier() {
    let guide = guide_named("output-save");
    let mut sim = Sim::start(&guide.starting_state);
    sim.drag_angle("Crown Main", 35.0);
    assert!(
        !sim.met(&guide.steps[1].goal),
        "a changed tier is not a saved design"
    );
}

#[test]
fn open_saves_then_replaces_the_design() {
    walk(
        "output-open",
        vec![
            event(events::DESIGN_SAVED),
            event(events::DESIGN_REPLACED),
            read(),
        ],
    );
}

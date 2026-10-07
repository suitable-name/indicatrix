//! Walks of the lessons in `viewing/solid.rs`.

use super::{Sim, act, event, read, viewing_events as events, walk};

#[test]
fn picking_a_facet_selects_its_tier_in_every_view() {
    walk(
        "viewing-solid-picking",
        vec![
            read(),
            read(),
            event(events::FACET_PICKED),
            act(|sim| sim.show(2)),
            event(events::FACET_PICKED),
            event(events::TIER_SELECTED),
            read(),
        ],
    );
}

#[test]
fn the_three_handles_move_a_tier_and_one_drag_is_one_undo_step() {
    let sim = walk(
        "viewing-drag-handles",
        vec![
            event(events::TIER_SELECTED),
            act(|sim| sim.drag_angle("Crown Main", 38.0)),
            act(Sim::undo),
            act(|sim| sim.drag_depth("Crown Main", 0.65)),
            act(|sim| sim.turn("Crown Main", 2)),
            event(events::SNAP_TOGGLED),
            event(events::SNAP_TOGGLED),
            read(),
        ],
    );
    let crown = &sim.session.design.tiers[sim.row("Crown Main")];
    assert!(
        (crown.angle_deg - 34.5).abs() < 1e-9,
        "the angle drag was undone"
    );
    assert!(
        crown
            .indices
            .iter()
            .all(|index| (index % 12.0).abs() > 1e-9),
        "the index drag turned the ring off the main positions: {:?}",
        crown.indices
    );
}

#[test]
fn slice_draws_flips_cuts_in_keeps_and_undoes() {
    let sim = walk(
        "viewing-slice",
        vec![
            event(events::SLICE_STARTED),
            event(events::SLICE_DRAWN),
            event(events::SLICE_FLIPPED),
            event(events::SLICE_CUTS_STONE),
            event(events::SLICE_SYMMETRIC_TOGGLED),
            act(|sim| sim.keep_slice("C2", 20.0)),
            act(Sim::undo),
            read(),
        ],
    );
    assert_eq!(sim.session.design.tiers.len(), 4, "the slice was undone");
}

#[test]
fn the_diagram_view_picks_drags_and_enlarges() {
    walk(
        "viewing-diagram",
        vec![
            act(|sim| sim.show(3)),
            read(),
            event(events::FACET_PICKED),
            act(|sim| sim.drag_angle("Crown Main", 36.0)),
            event(events::DIAGRAM_PANEL_ENLARGED),
            read(),
        ],
    );
}

#[test]
fn the_cut_slider_goes_to_the_rough_through_a_step_to_finished() {
    walk(
        "viewing-cut-slider",
        vec![
            read(),
            event(events::CUT_SLIDER_ROUGH),
            event(events::CUT_SLIDER_STEP),
            event(events::CUT_SLIDER_FINISHED),
            read(),
        ],
    );
}

#[test]
fn a_step_that_waits_for_a_drag_is_not_met_by_a_click_alone() {
    // Selecting a tier is not changing it: the angle, depth and index steps (2, 4 and 5) ask for
    // a changed design, and the undo step (3) asks for an unchanged one only after a change.
    let guide = super::guide_named("viewing-drag-handles");
    let mut sim = Sim::start(&guide.starting_state);
    sim.fire(events::TIER_SELECTED);
    for at in [1, 3, 4] {
        let step = &guide.steps[at];
        assert!(
            !sim.met(&step.goal),
            "{:?} must wait for a drag, not for a click",
            step.title
        );
    }
}

#[test]
fn the_undo_step_is_only_reached_after_a_drag() {
    // The step before it is done by a drag, so the design differs when the undo step starts.
    let guide = super::guide_named("viewing-drag-handles");
    let mut sim = Sim::start(&guide.starting_state);
    assert!(
        !sim.met(&guide.steps[1].goal),
        "the angle step waits for a drag"
    );
    sim.drag_angle("Crown Main", 38.0);
    assert!(sim.met(&guide.steps[1].goal));
    assert!(
        !sim.met(&guide.steps[2].goal),
        "with the drag still in place the undo step has not been done"
    );
}

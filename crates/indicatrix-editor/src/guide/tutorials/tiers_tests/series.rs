//! Walks of the lessons in `tiers/series.rs`.

use super::{Sim, act, read, walk};
use crate::loading::eval_number;
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The Generate steps form of both ladder lessons: name Step, start 30, step 4, N 3, the
/// eight main indices, no anchor.
fn ladder(sim: &mut Sim, linked: bool) {
    let made = if linked {
        sim.session
            .generate_step_series_linked("Step", "30", "4", 3, "0:12:96", "")
    } else {
        sim.session
            .generate_step_series("Step", "30", "4", 3, "0:12:96", "")
    };
    let made = made.expect("the ladder generates");
    assert_eq!(made.added, 3);
}

#[test]
fn a_plain_ladder_is_three_ordinary_tiers() {
    let sim = walk(
        "tiers-step-series",
        vec![
            read(),
            act(|sim| ladder(sim, false)),
            act(|sim| sim.set_angle("Step2", "35")),
            read(),
        ],
    );
    assert!(
        !sim.session.is_driven(sim.row("Step2")),
        "a plain ladder links nothing"
    );
}

#[test]
fn a_linked_ladder_follows_its_first_rung() {
    let sim = walk(
        "tiers-linked-series",
        vec![
            act(|sim| ladder(sim, true)),
            act(|sim| sim.set_angle("Step1", "32")),
            act(|sim| {
                let row = sim.row("Step3");
                sim.session
                    .clear_tier_relation(row)
                    .expect("the relation clears");
            }),
            read(),
        ],
    );
    let step3 = &sim.session.design.tiers[sim.row("Step3")];
    assert_eq!(step3.angle_deg, 40.0, "the freed rung keeps its angle");
}

#[test]
fn mirroring_copies_a_tier_to_the_other_block() {
    let sim = walk(
        "tiers-mirror-to-other-block",
        vec![
            act(|sim| {
                let row = sim.row("Pavilion Main");
                sim.session
                    .mirror_tier_to_other_block(row, "'")
                    .expect("the tier mirrors")
                    .expect("the tier exists");
            }),
            act(|sim| {
                let row = sim.row("Crown Main");
                sim.session
                    .mirror_tier_to_other_block(row, "b")
                    .expect("the tier mirrors")
                    .expect("the tier exists");
            }),
            read(),
        ],
    );
    assert_eq!(
        sim.session.design.tiers.len(),
        6,
        "two copies were appended"
    );
}

#[test]
fn a_relation_makes_one_tier_follow_another() {
    let sim = walk(
        "tiers-relations",
        vec![
            read(),
            act(|sim| {
                let row = sim.row("Pavilion Main");
                sim.session
                    .set_tier_relation(row, "=[Crown Main]+6.5")
                    .expect("the relation is accepted");
            }),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(|sim| {
                let row = sim.row("Pavilion Main");
                sim.session
                    .clear_tier_relation(row)
                    .expect("the relation clears");
            }),
            read(),
        ],
    );
    let pavilion = &sim.session.design.tiers[sim.row("Pavilion Main")];
    assert!(
        pavilion.angle_deg < 0.0,
        "the pavilion tier kept its own side of the stone"
    );
}

#[test]
fn number_fields_take_arithmetic() {
    let sim = walk(
        "tiers-arithmetic",
        vec![
            act(|sim| sim.edit_tier("Crown Main", |form| form.angle = "34.5+0.3".to_owned())),
            act(|sim| {
                sim.edit_tier("Table", |form| {
                    form.kind = 2;
                    form.text = "0.5-0.15".to_owned();
                });
            }),
            act(|sim| {
                let position = eval_number("96/16", None).expect("96/16 is a number");
                sim.orbit("Crown Main", |design, row| {
                    design.add_orbit_member(row, position)
                });
            }),
            read(),
        ],
    );
    let table = &sim.session.design.tiers[sim.row("Table")];
    assert!(
        matches!(&table.constraint, MeetConstraint::ScaleReference(value) if (value - 0.35).abs() < 1e-9),
        "the sum was worked out: {:?}",
        table.constraint
    );
}

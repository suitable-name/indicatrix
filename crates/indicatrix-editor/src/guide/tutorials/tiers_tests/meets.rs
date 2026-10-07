//! Walks of the lessons in `tiers/meets.rs`.

use super::{Sim, act, guide_named, read, walk};
use crate::{guide::StartingState, view_model::solid_status::status_text_and_is_problem};
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use indicatrix_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Edit, ManufacturabilityWarning, TierTarget,
    check_manufacturability, exceeds_preform,
};

/// Makes `name` meet the facets called `target`, as the tier form's Named facet(s) does.
fn meet_by_name(sim: &mut Sim, name: &str, target: &str) {
    sim.edit_tier(name, |form| {
        form.kind = 1;
        form.text = target.to_owned();
    });
}

/// The solve of the editor as it is now, which has to be a closed stone with every facet on its
/// surface and inside the blank. `when` says which moment of a lesson it is.
fn good_stone(sim: &Sim, when: &str) -> Vec<SolvedTier> {
    let design = &sim.session.design;
    let solved = design
        .solve()
        .unwrap_or_else(|error| panic!("{when}: the design does not solve: {error}"));
    let (status, is_problem) = status_text_and_is_problem(design);
    assert!(
        !is_problem && status.starts_with("Closed solid"),
        "{when}: {status}"
    );
    let vanishing: Vec<String> =
        check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter_map(|warning| match warning {
                ManufacturabilityWarning::VanishingFacet { tier_name, .. } => Some(tier_name),
                _ => None,
            })
            .collect();
    assert!(
        vanishing.is_empty(),
        "{when}: facets that never appear: {vanishing:?}"
    );
    assert!(
        exceeds_preform(design, &solved).is_none(),
        "{when}: the stone is bigger than the blank"
    );
    solved
}

#[test]
fn named_facets_unspecified_vertices_and_renames() {
    let sim = walk(
        "tiers-meets-named-facets",
        vec![
            read(),
            act(|sim| meet_by_name(sim, "Upper Girdle", "Lower Girdle")),
            act(|sim| {
                sim.edit_tier("Table", |form| {
                    form.kind = 0;
                    form.text.clear();
                });
            }),
            act(|sim| sim.edit_tier("Lower Girdle", |form| form.name = "LG1".to_owned())),
            read(),
        ],
    );
    let upper = &sim.session.design.tiers[sim.row("Upper Girdle")];
    assert_eq!(
        upper.constraint,
        MeetConstraint::MeetNamed(vec!["LG1".to_owned()]),
        "the rename followed the tier that meets the lower girdle"
    );
}

/// The lesson leaves a closed stone after every step, and the table does what its step says: the
/// solver puts it on a vertex close to where it was, not on the girdle.
#[test]
fn the_named_facets_lesson_keeps_a_good_stone_at_every_step() {
    let mut sim = Sim::start(&guide_named("tiers-meets-named-facets").starting_state);
    let start = good_stone(&sim, "at the start");
    let table_before = start[sim.row("Table")].mast;
    let upper_before = start[sim.row("Upper Girdle")].mast;

    // Step 2: the upper girdle meets the lower girdle.
    meet_by_name(&mut sim, "Upper Girdle", "Lower Girdle");
    let named = good_stone(&sim, "after Upper Girdle meets Lower Girdle");
    let upper = named[sim.row("Upper Girdle")].mast;
    assert!(
        (upper - upper_before).abs() < 0.05,
        "the solver put Upper Girdle at {upper}, far from its {upper_before}"
    );

    // Step 3: the table is left to the solver.
    sim.edit_tier("Table", |form| {
        form.kind = 0;
        form.text.clear();
    });
    let unspecified = good_stone(&sim, "after the table is left to the solver");
    let table = unspecified[sim.row("Table")].mast;
    assert!(
        (table - table_before).abs() < 0.05,
        "the table landed at {table}, far from its {table_before}"
    );

    // Step 4: the rename changes no depth.
    sim.edit_tier("Lower Girdle", |form| form.name = "LG1".to_owned());
    let renamed = good_stone(&sim, "after the rename");
    for (before, after) in unspecified.iter().zip(&renamed) {
        assert!(
            (before.mast - after.mast).abs() < 1e-9,
            "a rename moved a depth"
        );
    }
}

/// Why the lessons start from the Standard Round Brilliant: on the Rich Teaching Design, naming
/// a facet for its only pavilion tier leaves the pavilion without an exact scale value, and an
/// unspecified table sinks to the girdle, far below its 0.30.
#[test]
fn the_rich_teaching_design_does_not_suit_the_solver_lessons() {
    let mut sim = Sim::start(&StartingState::Template(5));
    meet_by_name(&mut sim, "Pavilion Main", "Girdle");
    let error = sim
        .session
        .design
        .solve()
        .expect_err("the pavilion has no exact scale value left");
    assert!(
        error.to_string().contains("Pavilion has no anchor"),
        "{error}"
    );

    let mut sim = Sim::start(&StartingState::Template(5));
    sim.edit_tier("Table", |form| {
        form.kind = 0;
        form.text.clear();
    });
    let solved = sim
        .session
        .design
        .solve()
        .expect("the crown keeps Crown Main");
    let table = solved[sim.row("Table")].mast;
    assert!(table < 0.15, "the table landed at {table}");
}

#[test]
fn millimetre_targets_need_a_girdle_diameter() {
    let sim = walk(
        "tiers-meets-mm-targets",
        vec![
            act(|sim| {
                sim.apply(Edit::SetGirdleDiameterMm {
                    girdle_diameter_mm: Some(6.5),
                });
            }),
            act(|sim| {
                sim.edit_tier("Pavilion Main", |form| {
                    form.kind = 3;
                    form.text = "2.2".to_owned();
                });
            }),
            act(|sim| {
                sim.edit_tier("Table", |form| {
                    form.kind = 5;
                    form.text = "3.2".to_owned();
                });
            }),
            act(|sim| {
                sim.edit_tier("Girdle", |form| {
                    form.kind = 4;
                    form.text = "0.3".to_owned();
                });
            }),
            read(),
        ],
    );
    let design = &sim.session.design;
    assert_eq!(
        design.tier_target(sim.row("Pavilion Main")),
        Some(TierTarget::DepthMm(2.2))
    );
    assert_eq!(
        design.tier_target(sim.row("Table")),
        Some(TierTarget::TableWidthMm(3.2))
    );
    assert_eq!(
        design.tier_target(sim.row("Girdle")),
        Some(TierTarget::GirdleThicknessMm(0.3))
    );
}

#[test]
fn adopting_the_meets_an_imported_file_stated() {
    walk(
        "tiers-adopt-imported-meets",
        vec![
            // Opening a .asc file: every tier is pinned, and the file's own meet is kept aside.
            act(|sim| {
                for name in ["Crown Main", "Pavilion Main"] {
                    let row = sim.row(name);
                    sim.session.design.tiers[row].imported_meet =
                        Some(MeetConstraint::MeetNamed(vec!["Girdle".to_owned()]));
                }
            }),
            act(|sim| {
                let row = sim.row("Crown Main");
                let adopted = sim
                    .session
                    .adopt_imported_meet(row)
                    .expect("the meet adopts");
                assert!(adopted, "Crown Main had a meet to adopt");
            }),
            act(|sim| {
                let adopted = sim
                    .session
                    .adopt_all_imported_meets()
                    .expect("the meets adopt");
                assert!(adopted >= 1, "something was left to adopt");
            }),
            read(),
        ],
    );
}

#[test]
fn pinning_the_depth_the_solver_found() {
    let sim = walk(
        "tiers-pin-to-mast",
        vec![
            act(|sim| meet_by_name(sim, "Upper Girdle", "Lower Girdle")),
            act(|sim| {
                let row = sim.row("Upper Girdle");
                // The status strip says the stone is solved before Pin is offered, and Pin
                // freezes the depth that solve found.
                let before = good_stone(sim, "before the pin");
                let now = sim.tick();
                let pinned = sim
                    .session
                    .pin_tier_mast(row, before[row].mast, now)
                    .expect("the pin applies")
                    .expect("the tier was not pinned yet");
                assert!(
                    matches!(pinned.replaced_meet, Some(MeetConstraint::MeetNamed(_))),
                    "the pin replaced the meet"
                );
                let after = good_stone(sim, "after the pin");
                for (was, is) in before.iter().zip(&after) {
                    assert!((was.mast - is.mast).abs() < 1e-9, "the pin moved a depth");
                }
            }),
            read(),
        ],
    );
    let upper = &sim.session.design.tiers[sim.row("Upper Girdle")];
    assert!(
        matches!(upper.constraint, MeetConstraint::ScaleReference(_)),
        "the tier is pinned: {:?}",
        upper.constraint
    );
}

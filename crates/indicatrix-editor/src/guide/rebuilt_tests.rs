//! The goals a "Build this design" lesson relies on: a fresh start, concave tiers, and the
//! rebuilt-design comparison of its last step.

use super::{
    EIGHT_FOLD_INDICES, Goal, GoalContext, goal_met, mast_within,
    tests::{design_with, tier},
};
use indicatrix_cut_core::{
    Design,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};

/// The depths of [`original`]'s four tiers, in its tier order.
const MASTS: [f64; 4] = [1.0, 0.7, 0.6, 0.35];

/// A girdle, pavilion mains, crown mains and a table.
fn original() -> Design {
    design_with(&[
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
        tier("P1", -40.0, &EIGHT_FOLD_INDICES),
        tier("C1", 34.5, &EIGHT_FOLD_INDICES),
        tier("T", 0.0, &[]),
    ])
}

fn rebuilt(target: Design, masts: &[f64]) -> Goal {
    Goal::DesignRebuilt {
        target: Box::new(target),
        angle_tol_deg: 0.02,
        mast_rel_tol: 0.01,
        target_masts: masts.to_vec(),
    }
}

/// Whether `goal` is met by `design` solved with `masts` (`None`: not solved yet).
fn met(goal: &Goal, design: &Design, masts: Option<&[f64]>) -> bool {
    let mut ctx = GoalContext::new(design).solved_closed(masts.is_some());
    if let Some(masts) = masts {
        ctx = ctx.solved_masts(masts);
    }
    goal_met(goal, &ctx)
}

fn concave(name: &str) -> ConcaveTier {
    ConcaveTier {
        name: name.to_owned(),
        angle_deg: -42.0,
        indices: vec![0.0, 12.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.15, 0.03],
        diameter_ratio: 0.25,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

#[test]
fn a_fresh_design_goal_wants_no_tiers_and_this_gear() {
    let fresh = design_with(&[]);
    let goal = |gear_teeth, symmetry_order, mirror| Goal::FreshDesign {
        gear_teeth,
        symmetry_order,
        mirror,
    };
    assert!(met(&goal(96, 8, true), &fresh, None));
    assert!(!met(&goal(80, 8, true), &fresh, None));
    assert!(!met(&goal(96, 4, true), &fresh, None));
    assert!(!met(&goal(96, 8, false), &fresh, None));

    // A tier, flat or concave, means it is no longer a fresh start.
    assert!(!met(&goal(96, 8, true), &original(), None));
    let mut with_tool = design_with(&[]);
    with_tool.concave_tiers.push(concave("Groove"));
    assert!(!met(&goal(96, 8, true), &with_tool, None));

    // The sign of the gear (a mirrored index convention) does not matter.
    let mut negative = design_with(&[]);
    negative.meta.gear_teeth = -96;
    assert!(met(&goal(96, 8, true), &negative, None));
}

#[test]
fn a_concave_tier_goal_counts_the_concave_tiers() {
    let mut design = design_with(&[]);
    let goal = Goal::ConcaveTiersAtLeast(2);
    assert!(!met(&goal, &design, None));
    design.concave_tiers.push(concave("Groove"));
    assert!(!met(&goal, &design, None));
    design.concave_tiers.push(concave("Dimple"));
    assert!(met(&goal, &design, None));
}

#[test]
fn the_rebuilt_goal_needs_every_tier_cut_as_in_the_original() {
    let goal = rebuilt(original(), &MASTS);
    assert!(met(&goal, &original(), Some(&MASTS)));

    // Tiers pair by name, not by position: the table first, the girdle last.
    let shuffled = design_with(&[
        tier("T", 0.0, &[]),
        tier("C1", 34.5, &EIGHT_FOLD_INDICES),
        tier("P1", -40.0, &EIGHT_FOLD_INDICES),
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
    ]);
    let shuffled_masts = [0.35, 0.6, 0.7, 1.0];
    assert!(met(&goal, &shuffled, Some(&shuffled_masts)));
    // ... and a mast that belongs to another tier does not count.
    assert!(!met(&goal, &shuffled, Some(&MASTS)));

    // Case and spaces in a name do not matter.
    let spaced = design_with(&[
        tier(" g1 ", 90.0, &EIGHT_FOLD_INDICES),
        tier("p1", -40.0, &EIGHT_FOLD_INDICES),
        tier("C1", 34.5, &EIGHT_FOLD_INDICES),
        tier("T", 0.0, &[]),
    ]);
    assert!(met(&goal, &spaced, Some(&MASTS)));
}

#[test]
fn the_rebuilt_goal_is_exact_about_angles_names_and_indices() {
    let goal = rebuilt(original(), &MASTS);
    let with = |g1: f64, p1: f64, name: &str, indices: &[f64]| {
        design_with(&[
            tier("G1", g1, &EIGHT_FOLD_INDICES),
            tier(name, p1, indices),
            tier("C1", 34.5, &EIGHT_FOLD_INDICES),
            tier("T", 0.0, &[]),
        ])
    };
    assert!(met(
        &goal,
        &with(90.0, -40.01, "P1", &EIGHT_FOLD_INDICES),
        Some(&MASTS)
    ));
    assert!(!met(
        &goal,
        &with(90.0, -40.03, "P1", &EIGHT_FOLD_INDICES),
        Some(&MASTS)
    ));
    assert!(!met(
        &goal,
        &with(90.0, -40.0, "P2", &EIGHT_FOLD_INDICES),
        Some(&MASTS)
    ));
    assert!(!met(
        &goal,
        &with(90.0, -40.0, "P1", &EIGHT_FOLD_INDICES[..7]),
        Some(&MASTS)
    ));

    // A tier too few, or one too many, is a different design.
    let missing = design_with(&[
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
        tier("P1", -40.0, &EIGHT_FOLD_INDICES),
        tier("C1", 34.5, &EIGHT_FOLD_INDICES),
    ]);
    assert!(!met(&goal, &missing, Some(&MASTS[..3])));
    let extra = design_with(&[
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
        tier("P1", -40.0, &EIGHT_FOLD_INDICES),
        tier("C1", 34.5, &EIGHT_FOLD_INDICES),
        tier("T", 0.0, &[]),
        tier("X", 20.0, &EIGHT_FOLD_INDICES),
    ]);
    assert!(!met(&goal, &extra, Some(&[1.0, 0.7, 0.6, 0.35, 0.5])));
}

#[test]
fn the_rebuilt_goal_tells_a_culet_from_a_table() {
    let culet = design_with(&[tier("Cu", -0.0, &[])]);
    let table = design_with(&[tier("Cu", 0.0, &[])]);
    let goal = rebuilt(culet.clone(), &[0.9]);
    assert!(met(&goal, &culet, Some(&[0.9])));
    assert!(!met(&goal, &table, Some(&[0.9])));
}

#[test]
fn a_blank_indices_field_is_a_lone_zero_only_where_the_azimuth_does_not_matter() {
    let design = |angle: f64, indices: &[f64]| design_with(&[tier("X", angle, indices)]);
    let same =
        |target: &Design, user: &Design| met(&rebuilt(target.clone(), &[1.0]), user, Some(&[1.0]));
    // A table, a culet and a girdle read the same either way.
    for angle in [0.0, -0.0, 90.0] {
        assert!(same(&design(angle, &[0.0]), &design(angle, &[])), "{angle}");
        assert!(same(&design(angle, &[]), &design(angle, &[0.0])), "{angle}");
        assert!(
            !same(&design(angle, &[]), &design(angle, &[12.0])),
            "{angle}"
        );
    }
    // A pavilion facet with no azimuth is not one at index 0.
    assert!(!same(&design(-40.0, &[]), &design(-40.0, &[0.0])));
    assert!(!same(&design(-40.0, &[0.0]), &design(-40.0, &[])));
    assert!(same(&design(-40.0, &[0.0]), &design(-40.0, &[0.0])));
}

#[test]
fn the_rebuilt_goal_compares_solved_depths_by_fraction() {
    let goal = rebuilt(original(), &MASTS);
    let user = original();
    let with_mast = |at: usize, mast: f64| {
        let mut masts = MASTS;
        masts[at] = mast;
        masts
    };
    // Half a percent off is the same stone; two percent off is not.
    assert!(met(&goal, &user, Some(&with_mast(1, 0.7 * 1.005))));
    assert!(!met(&goal, &user, Some(&with_mast(1, 0.7 * 1.02))));
    // The tolerance is a fraction of each depth: the table's is tighter than the girdle's.
    assert!(met(&goal, &user, Some(&with_mast(3, 0.35 + 0.003))));
    assert!(!met(&goal, &user, Some(&with_mast(3, 0.35 + 0.005))));
    assert!(met(&goal, &user, Some(&with_mast(0, 1.009))));
    assert!(!met(&goal, &user, Some(&with_mast(0, 1.011))));
}

#[test]
fn a_very_small_depth_never_asks_for_more_than_the_floor_allows() {
    assert!(mast_within(0.0104, 0.01, 0.01));
    assert!(!mast_within(0.0106, 0.01, 0.01));
    assert!(mast_within(0.7035, 0.7, 0.01));
    assert!(!mast_within(0.7075, 0.7, 0.01));
}

#[test]
fn the_rebuilt_goal_never_solves_and_waits_for_a_solve() {
    let goal = rebuilt(original(), &MASTS);
    let user = original();
    // No masts supplied: not solved yet, so not a match yet.
    assert!(!met(&goal, &user, None));
    // Masts supplied but the editor's verdict is not "closed".
    let not_closed = GoalContext::new(&user).solved_masts(&MASTS);
    assert!(!goal_met(&goal, &not_closed));
    // The wrong number of masts is not this design's solve.
    assert!(!met(&goal, &user, Some(&MASTS[..3])));
    // Without target masts only the cut itself is compared, so nothing needs solving.
    let cut_only = rebuilt(original(), &[]);
    assert!(met(&cut_only, &user, None));
    assert!(!cut_only.wants_solved_masts());
}

#[test]
fn the_rebuilt_goal_needs_the_same_gear_symmetry_and_mirror() {
    let goal = rebuilt(original(), &MASTS);
    let mut user = original();
    assert!(met(&goal, &user, Some(&MASTS)));
    user.meta.symmetry_order = 4;
    assert!(!met(&goal, &user, Some(&MASTS)));
    user.meta.symmetry_order = 8;
    user.meta.mirror = false;
    assert!(!met(&goal, &user, Some(&MASTS)));
    user.meta.mirror = true;
    user.meta.gear_teeth = 80;
    assert!(!met(&goal, &user, Some(&MASTS)));
}

#[test]
fn a_goal_that_asks_for_solved_masts_says_so_even_inside_all_and_any() {
    let with_masts = rebuilt(original(), &MASTS);
    assert!(with_masts.wants_solved_masts());
    assert!(Goal::All(vec![Goal::SolvedClosed, with_masts.clone()]).wants_solved_masts());
    assert!(Goal::Any(vec![with_masts]).wants_solved_masts());
    assert!(!rebuilt(original(), &[]).wants_solved_masts());
    assert!(!Goal::SolvedClosed.wants_solved_masts());
    assert!(!Goal::ConcaveTiersAtLeast(1).wants_solved_masts());
}

#[test]
fn the_new_goals_say_what_is_wrong_with_them() {
    assert_eq!(rebuilt(original(), &MASTS).problem(), None);
    assert_eq!(rebuilt(original(), &[]).problem(), None);
    assert!(rebuilt(original(), &MASTS[..3]).problem().is_some());
    assert!(rebuilt(design_with(&[]), &[]).problem().is_some());
    assert!(
        rebuilt(original(), &[1.0, f64::NAN, 0.6, 0.35])
            .problem()
            .is_some()
    );
    let bad_tolerance = Goal::DesignRebuilt {
        target: Box::new(original()),
        angle_tol_deg: f64::NAN,
        mast_rel_tol: 0.01,
        target_masts: Vec::new(),
    };
    assert!(bad_tolerance.problem().is_some());
    let negative = Goal::DesignRebuilt {
        target: Box::new(original()),
        angle_tol_deg: 0.02,
        mast_rel_tol: -0.01,
        target_masts: Vec::new(),
    };
    assert!(negative.problem().is_some());

    assert!(Goal::ConcaveTiersAtLeast(0).problem().is_some());
    assert_eq!(Goal::ConcaveTiersAtLeast(1).problem(), None);
    let no_gear = Goal::FreshDesign {
        gear_teeth: 0,
        symmetry_order: 8,
        mirror: true,
    };
    assert!(no_gear.problem().is_some());
    let fine = Goal::FreshDesign {
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
    };
    assert_eq!(fine.problem(), None);
    // Inside All the new goals are checked like any other.
    assert!(
        Goal::All(vec![Goal::ConcaveTiersAtLeast(0)])
            .problem()
            .is_some()
    );
}

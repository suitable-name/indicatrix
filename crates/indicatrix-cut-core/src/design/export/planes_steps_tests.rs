//! Tests for the Cut slider's step arrangement in [`super::planes`]:
//! [`Design::preview_steps`], [`Design::try_planes_after_steps`],
//! [`Design::try_planes_for_visible_tiers`] and [`Design::concave_tools_after_steps`].

use crate::{
    design::{ConstraintTier, Design, ScheduleMeta, TierRef},
    preform::PreformSpec,
};

fn round_brilliant_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// The concave fixture with its flat tiers reversed, so the stored order (crown
/// first) differs from the cutting order and the tiers shown after a step are not a
/// stored prefix.
fn crown_first_concave_design() -> Design {
    let mut design = Design::concave_fixture();
    design.tiers.reverse();
    design
}

/// The fixture is stored top-down; the slider walks the cutting order, the pavilion section
/// first and the table last, the same order as the printed sheet.
#[test]
fn a_planar_design_walks_its_cutting_order() {
    let design = round_brilliant_design();
    let expected: Vec<TierRef> = [4, 5, 6, 7, 1, 2, 3, 0]
        .into_iter()
        .map(TierRef::Flat)
        .collect();
    assert_eq!(design.preview_steps(), expected);
    assert_eq!(design.preview_steps(), design.cutting_order());
    assert_eq!(design.preview_step_count(), design.tiers.len());
}

#[test]
fn a_concave_design_walks_its_cutting_order_and_counts_both_tier_kinds() {
    let design = Design::concave_fixture();
    assert_eq!(design.preview_steps(), design.cutting_order());
    assert_eq!(
        design.preview_step_count(),
        design.tiers.len() + design.concave_tiers.len()
    );
    assert_eq!(design.preview_steps().len(), design.preview_step_count());
}

#[test]
fn zero_steps_is_the_preform_alone_and_needs_no_masts() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let rough = design.preform.planes_offset(design.preform_y_offset);
    assert_eq!(design.try_planes_after_steps(&solved, 0).unwrap(), rough);
    assert_eq!(
        design.try_planes_after_steps(&[], 0).unwrap(),
        rough,
        "the rough does not depend on the masts, so a stale list cannot break it"
    );
}

#[test]
fn every_step_at_or_past_the_end_is_the_finished_stone() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let full = design.planes_from_solved(&solved);
    let count = design.preview_step_count();
    for steps in [count, count + 1, usize::MAX] {
        assert_eq!(design.try_planes_after_steps(&solved, steps).unwrap(), full);
    }
}

/// After `k` steps a planar design shows exactly the planes of the first `k` tiers of its
/// cutting order (the finished stone's planes with the other tiers' slices removed), and at
/// the last step it is the finished stone.
#[test]
fn a_planar_design_shows_the_first_steps_of_its_cutting_order() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let full = design.planes_from_solved(&solved);
    let preform_len = design.preform.planes().len();
    let order = design.preview_steps();
    for steps in 1..=order.len() {
        let cut: Vec<usize> = order[..steps]
            .iter()
            .filter_map(|tier| match tier {
                TierRef::Flat(index) => Some(*index),
                TierRef::Concave(_) => None,
            })
            .collect();
        let expected: Vec<_> = full
            .iter()
            .enumerate()
            .filter(|&(index, _)| {
                index < preform_len
                    || design
                        .tier_for_plane_index(&solved, index)
                        .is_some_and(|tier| cut.contains(&tier))
            })
            .map(|(_, plane)| *plane)
            .collect();
        assert_eq!(
            design.try_planes_after_steps(&solved, steps).unwrap(),
            expected,
            "after {steps} step(s) a planar design shows its first {steps} tier(s) in cutting order"
        );
    }
    assert_eq!(
        design.try_planes_after_steps(&solved, order.len()).unwrap(),
        full
    );
}

#[test]
fn each_planar_step_adds_planes_and_never_removes_any() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let rough = design.try_planes_after_steps(&solved, 0).unwrap().len();
    assert_eq!(rough, design.preform.planes().len());
    let mut previous = rough;
    for steps in 1..=design.preview_step_count() {
        let now = design.try_planes_after_steps(&solved, steps).unwrap().len();
        assert!(now >= previous, "step {steps} must not remove planes");
        previous = now;
    }
    assert!(previous > rough, "the finished stone has facet planes");
}

#[test]
fn a_misaligned_mast_list_is_an_error_not_a_panic() {
    let design = round_brilliant_design();
    let err = design
        .try_planes_after_steps(&[], 1)
        .expect_err("no masts cannot describe the first tier");
    assert_eq!(err.expected_tiers, design.tiers.len());
    assert_eq!(err.got_tiers, 0);
    assert!(design.try_planes_after_steps(&[], usize::MAX).is_err());
    assert!(design.try_planes_for_visible_tiers(&[], &[]).is_err());
}

#[test]
fn the_visible_mask_agrees_with_the_prefix_truncation() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    let count = design.tiers.len();
    for shown in 1..=count {
        let mask: Vec<bool> = (0..count).map(|i| i < shown).collect();
        assert_eq!(
            design.try_planes_for_visible_tiers(&solved, &mask).unwrap(),
            design.planes_through_tier(&solved, shown - 1),
            "a prefix mask of {shown} tier(s) is the same truncation"
        );
    }
    let all = vec![true; count];
    assert_eq!(
        design.try_planes_for_visible_tiers(&solved, &all).unwrap(),
        design.planes_from_solved(&solved)
    );
}

#[test]
fn a_cutting_order_that_is_not_a_stored_prefix_shows_exactly_the_cut_tiers() {
    let design = crown_first_concave_design();
    let solved = design.solve().expect("the fixture solves");
    // Stored: crown 40 (0), crown 32 (1), pavilion -38 (2), pavilion -42 (3),
    // girdle (4). Cut: pavilion and girdle, the groove, the crowns, the dimple.
    assert_eq!(
        design.preview_steps(),
        vec![
            TierRef::Flat(2),
            TierRef::Flat(3),
            TierRef::Flat(4),
            TierRef::Concave(0),
            TierRef::Flat(0),
            TierRef::Flat(1),
            TierRef::Concave(1),
        ]
    );
    let full = design.planes_from_solved(&solved);
    let preform_len = design.preform.planes().len();
    let after_first = design.try_planes_after_steps(&solved, 1).unwrap();
    let expected: Vec<_> = full
        .iter()
        .enumerate()
        .filter(|&(index, _)| {
            index < preform_len || design.tier_for_plane_index(&solved, index) == Some(2)
        })
        .map(|(_, plane)| *plane)
        .collect();
    assert_eq!(after_first, expected, "only the first pavilion tier is cut");
    assert!(after_first.len() > preform_len && after_first.len() < full.len());

    let mut previous = preform_len;
    for steps in 0..=design.preview_step_count() {
        let now = design.try_planes_after_steps(&solved, steps).unwrap().len();
        assert!(now >= previous, "step {steps} must not remove planes");
        previous = now;
    }
    assert_eq!(
        design
            .try_planes_after_steps(&solved, design.preview_step_count())
            .unwrap(),
        full
    );
}

#[test]
fn a_concave_step_adds_its_tool_and_no_flat_plane() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture solves");
    // Cutting order: three flat pavilion/girdle tiers, the groove, two crown tiers,
    // the dimple.
    let before_groove = design.try_planes_after_steps(&solved, 3).unwrap();
    let after_groove = design.try_planes_after_steps(&solved, 4).unwrap();
    assert_eq!(
        before_groove, after_groove,
        "the groove is a tool, not a plane"
    );
    let tools_at = |steps: usize| {
        design
            .concave_tools_after_steps(&solved, steps)
            .expect("the fixture's concave tiers resolve")
    };
    assert!(tools_at(0).0.is_empty(), "the rough has no tools");
    assert!(tools_at(3).0.is_empty(), "before the groove is cut");
    let (groove, placements) = tools_at(4);
    assert_eq!(groove.len(), 8, "eight groove placements");
    assert!(placements.iter().all(|&(tier, _)| tier == 0));
    assert_eq!(tools_at(6).0.len(), 8, "the dimple is still to come");
    assert_eq!(tools_at(7).0.len(), 12, "eight grooves and four dimples");
    assert_eq!(tools_at(99).0.len(), 12);
}

#[test]
fn a_planar_design_never_has_tools() {
    let design = round_brilliant_design();
    let solved = design.solve().expect("every tier is pinned");
    for steps in [0, 1, design.preview_step_count(), usize::MAX] {
        let (tools, placements) = design
            .concave_tools_after_steps(&solved, steps)
            .expect("nothing to resolve");
        assert!(tools.is_empty() && placements.is_empty());
    }
}

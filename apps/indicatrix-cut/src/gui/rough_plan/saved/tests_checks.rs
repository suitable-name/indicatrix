//! Tests of the loader's cross-checks: every stored figure that follows from others is
//! recomputed, and a file that disagrees is refused with the path of the field.
//!
//! The layout is the fixture of [`super::fixtures`] (its figures are derived there from
//! first principles); each case changes one figure and names the path the loader must
//! report.

use super::{
    checks::{RoughFrame, check_layout, table_axis_of},
    fixtures::{
        FACING_X, SPECIFIC_GRAVITY, exact_fit_layout_in, expect_error, plain_block, sample_layout,
        volume_of, write,
    },
    format::parse_and_validate_plan,
};
use indicatrix_cut_core::rough_plan::{Axis, RoughLayout};

/// One refusal case: what is changed, and the text the message must carry.
type Mutation = fn(&mut RoughLayout);

#[test]
fn every_figure_that_follows_from_others_is_checked_and_named() {
    let cases: [(&str, Mutation, &str); 12] = [
        (
            "a carat that does not follow from the volume",
            |layout| layout.stones[0].carat = 0.5,
            "layouts[0].stones[0].carat",
        ),
        (
            "a carat off by a hundred-thousandth",
            |layout| layout.stones[2].carat *= 1.000_01,
            "layouts[0].stones[2].carat",
        ),
        (
            "a total carat that is not the sum",
            |layout| layout.total_carat = 0.5,
            "layouts[0].total_carat",
        ),
        (
            "a total volume that is not the sum",
            |layout| layout.total_volume_mm3 = 25.0,
            "layouts[0].total_volume_mm3",
        ),
        (
            "a yield against another rough",
            |layout| layout.yield_fraction = 0.5,
            "layouts[0].yield_fraction",
        ),
        (
            "a table axis the pose does not point along",
            |layout| layout.stones[1].table_axis = Axis::X,
            "layouts[0].stones[1].table_axis",
        ),
        (
            "a stone larger than its piece",
            |layout| layout.stones[2].stone_size_mm[1] = 3.5,
            "layouts[0].stones[2].stone_size_mm[1]",
        ),
        (
            "a piece that starts before the rough",
            |layout| layout.stones[0].piece_origin_mm[0] = -1.0,
            "layouts[0].stones[0].piece_origin_mm[0]",
        ),
        (
            "a piece that reaches past the rough",
            |layout| layout.stones[1].piece_origin_mm[2] = 9.0,
            "layouts[0].stones[1].piece_size_mm[2]",
        ),
        (
            "a piece length that is not the saw plan's",
            |layout| layout.cut_plan.slabs[0].bars[0].pieces_mm[1] = 3.3,
            "layouts[0].stones[1].piece_size_mm[2]",
        ),
        (
            "a slab thickness that is not the pieces'",
            |layout| layout.cut_plan.slabs[1].thickness_mm = 2.9,
            "layouts[0].stones[2].piece_size_mm[1]",
        ),
        (
            "an exact fit with three stones",
            |layout| layout.exact_fit = true,
            "layouts[0].exact_fit",
        ),
    ];
    for (what, mutate, path) in cases {
        let mut layout = sample_layout();
        mutate(&mut layout);
        let error = expect_error(&write(&plain_block(), &[layout]));
        assert!(error.contains(path), "{what}: expected {path} in: {error}");
    }
}

#[test]
fn an_exact_fit_is_one_stone_and_at_most_its_own_box_as_a_piece() {
    let model = plain_block();
    let exact = exact_fit_layout_in(&model);
    parse_and_validate_plan(&write(&model, std::slice::from_ref(&exact)))
        .expect("a planner's exact fit loads");

    // A writer that leaves the saw plan out (an exact fit is not sawn) is accepted too.
    let mut unsawn = exact.clone();
    unsawn.cut_plan.slabs.clear();
    let loaded = parse_and_validate_plan(&write(&model, &[unsawn])).expect("no saw stages");
    assert!(loaded.layouts[0].exact_fit);

    // Two pieces of saw plan cannot be an exact fit.
    let mut sawn = exact.clone();
    sawn.cut_plan.slabs[0].bars[0].pieces_mm.push(1.0);
    let error = expect_error(&write(&model, &[sawn]));
    assert!(error.contains("layouts[0].exact_fit"), "{error}");
    assert!(error.contains("no saw stages"), "{error}");

    // Two stones cannot.
    let mut pair = exact;
    pair.stones.push(pair.stones[0]);
    let error = expect_error(&write(&model, &[pair]));
    assert!(error.contains("layouts[0].exact_fit"), "{error}");
    assert!(error.contains("exactly one"), "{error}");
}

#[test]
fn a_stone_that_faces_another_axis_is_accepted_when_the_pose_says_so() {
    let mut layout = sample_layout();
    layout.stones[1].pose.axes = FACING_X;
    layout.stones[1].table_axis = Axis::X;
    parse_and_validate_plan(&write(&plain_block(), &[layout.clone()]))
        .expect("the pose faces X and says so");
    layout.stones[1].table_axis = Axis::Y;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("layouts[0].stones[1].table_axis"), "{error}");
    assert!(error.contains("points along X"), "{error}");
}

#[test]
fn the_table_axis_of_a_normal_follows_the_planners_tie_rule() {
    assert_eq!(table_axis_of([0.0, 1.0, 0.0]), Axis::Y);
    assert_eq!(table_axis_of([0.0, 0.0, -1.0]), Axis::Z);
    assert_eq!(table_axis_of([-1.0, 0.0, 0.0]), Axis::X);
    // Equal components: x wins over y and z, y over z.
    assert_eq!(table_axis_of([0.5, 0.5, 0.5]), Axis::X);
    assert_eq!(table_axis_of([0.0, 0.5, 0.5]), Axis::Y);
    assert_eq!(table_axis_of([0.5, 0.4, 0.5]), Axis::X);
}

#[test]
fn a_yield_is_checked_against_the_model_the_file_describes() {
    // The fixture's yield is 23 mm³ over the block's 2400 mm³.
    let layout = sample_layout();
    assert!((layout.yield_fraction - 23.0 / 2400.0).abs() < 1e-12);
    let frame = RoughFrame {
        specific_gravity: SPECIFIC_GRAVITY,
        volume_mm3: volume_of(&plain_block()),
        bbox_mm: [20.0, 12.0, 10.0],
    };
    check_layout(&layout, &frame, "layouts[0]").expect("the fixture is consistent");
    // The same layout against a rough of half the volume: its yield would be twice as big.
    let half = RoughFrame {
        volume_mm3: 1200.0,
        ..frame
    };
    let error = check_layout(&layout, &half, "layouts[3]").expect_err("another rough");
    assert!(error.contains("layouts[3].yield_fraction"), "{error}");
    // Against another specific gravity the carats no longer follow.
    let denser = RoughFrame {
        specific_gravity: 4.0,
        ..frame
    };
    let error = check_layout(&layout, &denser, "layouts[0]").expect_err("another gravity");
    assert!(error.contains("stones[0].carat"), "{error}");
}

#[test]
fn a_layout_inside_the_bounding_box_passes_at_its_edge() {
    let frame = RoughFrame {
        specific_gravity: SPECIFIC_GRAVITY,
        volume_mm3: volume_of(&plain_block()),
        bbox_mm: [20.0, 12.0, 10.0],
    };
    // Stone 1 ends at z = 3.8 + 3.2 = 7.0 mm: a rough of exactly that depth holds it.
    let mut layout = sample_layout();
    let tight = RoughFrame {
        bbox_mm: [20.0, 12.0, 7.0],
        ..frame
    };
    check_layout(&layout, &tight, "layouts[0]").expect("the piece ends exactly at the edge");
    // A hair beyond the tolerance does not.
    layout.stones[1].piece_origin_mm[2] = 3.8 + 1e-3;
    let error = check_layout(&layout, &tight, "layouts[0]").expect_err("past the edge");
    assert!(error.contains("piece_size_mm[2]"), "{error}");
}

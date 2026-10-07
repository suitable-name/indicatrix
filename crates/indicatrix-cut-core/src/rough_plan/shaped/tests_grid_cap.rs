//! The coarser unit grid of a mesh rough, and the partial ranking of a stopped plan.

use super::{
    ctx::ShapedCtx,
    grid::{
        SHAPED_MESH_PIECE_CAP, SHAPED_PIECE_CAP, choose_shaped_grid_at, choose_shaped_grid_capped,
        piece_count,
    },
    tests::corner_cut_model,
};
use crate::rough_plan::{
    FINAL_LAYOUTS, LayoutGroup, PlanInput, PlanSettings, RoughModel, merge_and_rank,
    partial_ranking, plan,
    shape::mesh_fixture::{C_SHAPE_OBJ, box_hull, import_obj},
    tests::{box_design, settings_with},
};

fn settings(count: u8) -> PlanSettings {
    PlanSettings {
        count,
        ..PlanSettings::default()
    }
}

#[test]
fn the_plain_chooser_is_the_capped_one_at_the_plain_cap() {
    for extents in [
        [20.0; 3],
        [40.0, 25.0, 8.0],
        [100.0, 100.0, 100.0],
        [6.0, 5.0, 4.0],
    ] {
        for count in [1, 4, 10, 40, 99] {
            let settings = settings(count);
            for corner in [[0.0; 3], [3.0, -2.0, 7.5]] {
                assert_eq!(
                    choose_shaped_grid_at(corner, extents, &settings),
                    choose_shaped_grid_capped(corner, extents, &settings, SHAPED_PIECE_CAP),
                    "{extents:?} K={count}"
                );
            }
        }
    }
}

#[test]
fn a_convex_cube_at_k10_keeps_its_16_cell_grid() {
    // 18 cells an axis shrink by 5 % steps (18, 17, 16) until the 2.6 M entry cap holds
    // (136^3 = 2,515,456 entries).
    let grid = choose_shaped_grid_at([0.0; 3], [20.0; 3], &settings(10));
    assert_eq!(grid.cells, [16; 3]);
}

#[test]
fn the_mesh_cap_gives_at_most_the_mesh_cap_and_never_more_cells_than_the_convex_choice() {
    assert_eq!(SHAPED_MESH_PIECE_CAP, SHAPED_PIECE_CAP / 8);
    for extents in [
        [20.0; 3],
        [40.0, 25.0, 8.0],
        [100.0, 100.0, 100.0],
        [60.0, 60.0, 12.0],
    ] {
        for count in [1, 4, 10, 40, 99] {
            let settings = settings(count);
            let convex = choose_shaped_grid_at([0.0; 3], extents, &settings);
            let mesh =
                choose_shaped_grid_capped([0.0; 3], extents, &settings, SHAPED_MESH_PIECE_CAP);
            for axis in 0..3 {
                assert!(
                    mesh.cells[axis] <= convex.cells[axis],
                    "{extents:?} K={count}: {:?} vs {:?}",
                    mesh.cells,
                    convex.cells
                );
            }
            // The shrink stops at 2 cells an axis; above that the cap must hold.
            if mesh.cells.iter().all(|&g| g > 2) {
                assert!(
                    piece_count(mesh.cells) <= SHAPED_MESH_PIECE_CAP,
                    "{:?}",
                    mesh.cells
                );
            }
        }
    }
    let mesh = choose_shaped_grid_capped([0.0; 3], [20.0; 3], &settings(10), SHAPED_MESH_PIECE_CAP);
    assert_eq!(mesh.cells, [11; 3], "{} entries", piece_count(mesh.cells));
}

#[test]
fn a_rough_with_a_mesh_is_planned_on_the_mesh_grid_and_a_convex_one_on_the_plain_grid() {
    let settings = settings(10);
    let c_shape = RoughModel::new(import_obj(C_SHAPE_OBJ), Vec::new());
    let ctx = ShapedCtx::new(&c_shape, &settings).expect("ctx");
    assert!(ctx.mesh.is_some());
    assert_eq!(
        ctx.choose_grid(&settings),
        choose_shaped_grid_capped(
            ctx.bbox_min,
            ctx.bbox_extents,
            &settings,
            SHAPED_MESH_PIECE_CAP
        )
    );

    let convex = ShapedCtx::new(&corner_cut_model(), &settings).expect("ctx");
    assert!(convex.mesh.is_none());
    assert_eq!(
        convex.choose_grid(&settings),
        choose_shaped_grid_at(convex.bbox_min, convex.bbox_extents, &settings)
    );
}

#[test]
fn the_partial_ranking_is_the_ranking_of_everything_found() {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let designs = vec![box_design(1)];
    let hulls = vec![box_hull(1, 1.0, 1.0, 1.0)];
    let model = corner_cut_model();
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    let layouts = plan(&input, &mut |_| true).expect("not cancelled");
    assert!(layouts.len() >= 2, "the fixture plans several layouts");

    let (first, second) = layouts.split_at(layouts.len() / 2);
    let groups = [LayoutGroup {
        pool: Vec::new(),
        layouts: first.to_vec(),
    }];
    let partial = partial_ranking(&groups, second.to_vec());
    assert_eq!(partial, merge_and_rank(layouts.clone(), FINAL_LAYOUTS));
    assert!(partial.len() <= FINAL_LAYOUTS);
    assert_eq!(partial_ranking(&[], Vec::new()), Vec::new());
}

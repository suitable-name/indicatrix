//! The shaped twin of `a_stone_at_the_minimum_width_is_feasible_on_every_path`: a stone
//! exactly at the minimum width must be feasible for the layout builder as it is for every
//! valuation (the table, the uniform pass, the refinement), all of which accept it down to
//! [`min_width_floor`](crate::rough_plan::piece::min_width_floor).

use super::{
    ctx::ShapedCtx,
    tree::{StoneFitter, layout_from_tree_shaped},
};
use crate::rough_plan::{
    CutOrder, RoughBase, RoughModel,
    tests::{box_design, settings_with},
    tree::{Bar, Leaf, Slab, Tree},
};

/// The float one step below `x`.
const fn down(x: f64) -> f64 {
    f64::from_bits(x.to_bits() - 1)
}

/// The float one step above `x`.
const fn up(x: f64) -> f64 {
    f64::from_bits(x.to_bits() + 1)
}

#[test]
fn the_shaped_fitter_accepts_a_stone_at_the_minimum_width() {
    let mw = 1.0_f64;
    let settings = settings_with(2, 0.0, 0.0, mw);
    let pool = [box_design(1)];
    let leaf = Leaf {
        len: 9.0,
        design: 0,
        orient: 0,
    };
    let mut fitter = StoneFitter::new(&settings, &pool, &[]);
    // With no allowance the piece is the stone's box. A cube design is as wide as the
    // narrowest side of its piece: one step under the minimum, on it, one step over it and a
    // hair under (1e-13 relative, inside the slack) are all feasible.
    for width in [down(mw), mw, up(mw), mw * (1.0 - 1e-13)] {
        let stone = fitter.fit([0.0; 3], [width, 9.0, 9.0], &leaf);
        assert!(stone.is_some(), "a piece {width:e} wide");
    }
    // One part in 1e9 short is a real shortfall.
    assert!(
        fitter
            .fit([0.0; 3], [mw * (1.0 - 1e-9), 9.0, 9.0], &leaf)
            .is_none()
    );
}

#[test]
fn a_layout_keeps_the_stone_of_a_piece_at_the_minimum_width() {
    // What the refinement does: it moves a cut to the kink where a stone just becomes wide
    // enough, and values the piece with the shared slack. The layout built from that tree
    // must hold the stone, or the planner silently loses what the refinement moved a cut for.
    let mw = 1.0_f64;
    let width = mw * (1.0 - 1e-13);
    let settings = settings_with(1, 0.3, 0.0, mw);
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: width,
            y_mm: 6.0,
            z_mm: 6.0,
        },
        Vec::new(),
    );
    let ctx = ShapedCtx::new(&model, &settings).expect("ctx");
    let tree = Tree {
        slabs: vec![Slab {
            thickness: width,
            bars: vec![Bar {
                width: 6.0,
                leaves: vec![Leaf {
                    len: 6.0,
                    design: 0,
                    orient: 0,
                }],
            }],
        }],
    };
    let layout = layout_from_tree_shaped(&ctx, CutOrder::Xyz, &tree, &[box_design(1)], &settings);
    assert_eq!(layout.stone_count(), 1);
    assert!((layout.stones[0].stone_size_mm[0] - width).abs() < 1e-12);
}

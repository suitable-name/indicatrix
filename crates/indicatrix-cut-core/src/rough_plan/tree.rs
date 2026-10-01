//! The internal guillotine tree shared by the DP reconstruction, the uniform
//! pass and the refinement, and its conversion to a public [`RoughLayout`].

use super::{
    piece::{ASSIGNMENTS, Norm, stone_scale},
    types::{
        Axis, BarCut, CandidateDesign, CutOrder, CutPlan, PlacedStone, PlanSettings, RoughBlock,
        RoughLayout, SlabCut, StonePose,
    },
};
use crate::yield_metrics::carat_weight;

/// One stone's cell: the piece length along the stage-3 axis, the design (an
/// index into the pool the tree is interpreted against) and the assignment.
#[derive(Debug, Clone)]
pub struct Leaf {
    /// Piece extent along the stage-3 axis, in mm.
    pub len: f64,
    /// Index of the design in the pool.
    pub design: usize,
    /// Index into [`ASSIGNMENTS`].
    pub orient: usize,
}

/// A bar: extent along the stage-2 axis and its pieces.
#[derive(Debug, Clone)]
pub struct Bar {
    /// Extent along the stage-2 axis, in mm.
    pub width: f64,
    /// The pieces, in position order.
    pub leaves: Vec<Leaf>,
}

/// A slab: extent along the stage-1 axis and its bars.
#[derive(Debug, Clone)]
pub struct Slab {
    /// Extent along the stage-1 axis, in mm.
    pub thickness: f64,
    /// The bars, in position order.
    pub bars: Vec<Bar>,
}

/// A whole staged layout, in mm (piece sizes, kerf excluded).
#[derive(Debug, Clone)]
pub struct Tree {
    /// The slabs, in position order.
    pub slabs: Vec<Slab>,
}

/// Place `(stage 1, stage 2, stage 3)` values onto canonical `[x, y, z]`.
pub const fn to_canonical(order: [usize; 3], v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    out[order[0]] = v[0];
    out[order[1]] = v[1];
    out[order[2]] = v[2];
    out
}

/// Walk every piece of `tree` in position order, calling `f(origin_mm, size_mm, leaf)`.
///
/// Coordinates are in canonical `[x, y, z]` millimeters. `origin_mm` is the
/// corner of the first piece (the rough's minimum corner plus the skin); the
/// pieces follow one kerf apart.
pub fn for_each_piece(
    order: CutOrder,
    tree: &Tree,
    origin_mm: [f64; 3],
    kerf_mm: f64,
    mut f: impl FnMut([f64; 3], [f64; 3], &Leaf),
) {
    let ord = order.axes();
    let mut t_off = origin_mm[ord[0]];
    for slab in &tree.slabs {
        let mut w_off = origin_mm[ord[1]];
        for bar in &slab.bars {
            let mut l_off = origin_mm[ord[2]];
            for leaf in &bar.leaves {
                let origin = to_canonical(ord, [t_off, w_off, l_off]);
                let size = to_canonical(ord, [slab.thickness, bar.width, leaf.len]);
                f(origin, size, leaf);
                l_off += leaf.len + kerf_mm;
            }
            w_off += bar.width + kerf_mm;
        }
        t_off += slab.thickness + kerf_mm;
    }
}

/// The saw plan of `tree`: its slab, bar and piece sizes.
pub fn cut_plan_of(tree: &Tree) -> CutPlan {
    let slabs = tree
        .slabs
        .iter()
        .map(|slab| SlabCut {
            thickness_mm: slab.thickness,
            bars: slab
                .bars
                .iter()
                .map(|bar| BarCut {
                    width_mm: bar.width,
                    pieces_mm: bar.leaves.iter().map(|l| l.len).collect(),
                })
                .collect(),
        })
        .collect();
    CutPlan { slabs }
}

/// The layout of `tree` holding `stones` (one per piece, in traversal order),
/// with the yield taken against `reference_volume_mm3`.
pub fn assemble_layout(
    order: CutOrder,
    tree: &Tree,
    stones: Vec<PlacedStone>,
    reference_volume_mm3: f64,
) -> RoughLayout {
    let total_volume_mm3: f64 = stones.iter().map(|s| s.volume_mm3).sum();
    let total_carat: f64 = stones.iter().map(|s| s.carat).sum();
    RoughLayout {
        cut_order: order,
        stones,
        cut_plan: cut_plan_of(tree),
        total_carat,
        total_volume_mm3,
        yield_fraction: if reference_volume_mm3 > 0.0 {
            total_volume_mm3 / reference_volume_mm3
        } else {
            0.0
        },
        exact_fit: false,
    }
}

/// Build the public layout of `tree`, whose leaves index into `pool`.
///
/// Pieces are laid out from the skin inward, separated by one kerf. Stones
/// come out in traversal order. Leaves that are infeasible (which the callers
/// never produce) contribute a zero-size stone.
pub fn layout_from_tree(
    rough: &RoughBlock,
    settings: &PlanSettings,
    order: CutOrder,
    tree: &Tree,
    pool: &[CandidateDesign],
) -> RoughLayout {
    let a2 = 2.0 * settings.allowance_mm;
    let mut stones = Vec::new();
    let origin = [settings.skin_mm; 3];
    for_each_piece(
        order,
        tree,
        origin,
        settings.kerf_mm,
        |origin, size, leaf| {
            let usable = size.map(|s| s - a2);
            stones.push(place_stone(settings, pool, leaf, origin, size, usable));
        },
    );
    assemble_layout(order, tree, stones, rough.volume_mm3())
}

/// One placed stone of `leaf` in the piece at `origin` with `size` and usable
/// box `usable`.
fn place_stone(
    settings: &PlanSettings,
    pool: &[CandidateDesign],
    leaf: &Leaf,
    origin: [f64; 3],
    size: [f64; 3],
    usable: [f64; 3],
) -> PlacedStone {
    let design = &pool[leaf.design];
    let norm = Norm::of(design);
    let assignment = &ASSIGNMENTS[leaf.orient];
    let s = stone_scale(&norm, leaf.orient, usable).max(0.0);
    let mut stone_size = [0.0; 3];
    for (dim, axis) in norm.dims().iter().zip(assignment) {
        stone_size[*axis] = s * dim;
    }
    let volume_mm3 = norm.f * (s * s * s);
    let axes = assignment_axes(assignment);
    let center_mm = [
        0.5f64.mul_add(size[0], origin[0]),
        0.5f64.mul_add(size[1], origin[1]),
        0.5f64.mul_add(size[2], origin[2]),
    ];
    let mm_per_unit = if design.width > 0.0 {
        s / design.width
    } else {
        0.0
    };
    let pose = StonePose {
        center_mm,
        axes,
        mm_per_unit,
    };
    PlacedStone {
        entry_id: design.entry_id,
        piece_origin_mm: origin,
        piece_size_mm: size,
        stone_size_mm: stone_size,
        table_axis: Axis::from_index(assignment[2]),
        carat: carat_weight(volume_mm3, settings.specific_gravity),
        volume_mm3,
        pose,
    }
}

/// The index into [`ASSIGNMENTS`] whose [`assignment_axes`] produced `pose`, or
/// `None` for a pose that is not one of them (an exact single-stone fit).
pub fn orient_of_pose(pose: &StonePose) -> Option<usize> {
    let unit_axis = |v: [f64; 3]| {
        (0..3).find(|&i| v[i] > 0.5 && v.iter().enumerate().all(|(j, c)| j == i || c.abs() < 1e-6))
    };
    let width_axis = unit_axis(pose.axes[0])?;
    let table_axis = unit_axis(pose.axes[1])?;
    ASSIGNMENTS
        .iter()
        .position(|a| a[0] == width_axis && a[2] == table_axis)
}

/// Computes orthonormal right-handed caliper axes for `assignment`:
/// `axes[0] = +e_{assignment[0]}`, `axes[1] = +e_{assignment[2]}`,
/// `axes[2] = axes[0] x axes[1]`.
pub fn assignment_axes(assignment: &[usize; 3]) -> [[f64; 3]; 3] {
    let mut axes = [[0.0; 3]; 3];
    axes[0][assignment[0]] = 1.0;
    axes[1][assignment[2]] = 1.0;
    axes[2] = [
        f64::mul_add(axes[0][2], -axes[1][1], axes[0][1] * axes[1][2]),
        f64::mul_add(axes[0][0], -axes[1][2], axes[0][2] * axes[1][0]),
        f64::mul_add(axes[0][1], -axes[1][0], axes[0][0] * axes[1][1]),
    ];
    axes
}

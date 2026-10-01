//! Guillotine tree reconstruction and layout placement for shaped roughs.
//!
//! Evaluates continuous stone placements by re-solving cut plane intersections at actual
//! refined piece bounds and computes yield relative to the true cut rough model volume.

use glam::DVec3;

use super::{
    clip::{CLASS_EXTERIOR, CLASS_INTERIOR, classify_box_into},
    compact::compact_tree,
    ctx::ShapedCtx,
    rows::{ClipRegion, PartialSolver, caliper_extents},
};
use crate::rough_plan::{
    Axis, CandidateDesign, CutOrder, PlacedStone, PlanSettings, RoughLayout, StonePose,
    piece::{ASSIGNMENTS, Norm, stone_scale},
    tree::{Leaf, Tree, assemble_layout, assignment_axes},
};

/// Fits stones into pieces of a shaped rough, reusing its buffers.
pub struct StoneFitter<'a> {
    settings: &'a PlanSettings,
    pool: &'a [CandidateDesign],
    non_box: &'a [(DVec3, f64)],
    solver: PartialSolver,
    violated: Vec<usize>,
}

impl<'a> StoneFitter<'a> {
    /// A fitter for designs of `pool` against the `non_box` planes.
    #[must_use]
    pub fn new(
        settings: &'a PlanSettings,
        pool: &'a [CandidateDesign],
        non_box: &'a [(DVec3, f64)],
    ) -> Self {
        Self {
            settings,
            pool,
            non_box,
            solver: PartialSolver::new(),
            violated: Vec::new(),
        }
    }

    /// Fits one stone of `leaf` into the piece at `origin` with `size`.
    ///
    /// The stone is the largest scaling of the leaf's design and assignment whose
    /// box lies inside the piece minus the allowance and inside every cut plane.
    /// `None` when the leaf's design is not in the pool, the piece lies outside
    /// the rough, or the stone would be narrower than the minimum width.
    pub fn fit(&mut self, origin: [f64; 3], size: [f64; 3], leaf: &Leaf) -> Option<PlacedStone> {
        let design = self.pool.get(leaf.design)?;
        let norm = Norm::of(design);
        let allowance = self.settings.allowance_mm;
        let usable = size.map(|s| 2.0f64.mul_add(-allowance, s).max(0.0));
        let b_min = origin.map(|o| o + allowance);
        let b_max = [0, 1, 2].map(|i| origin[i] + size[i] - allowance);

        let class = classify_box_into(b_min, b_max, self.non_box, &mut self.violated);
        if class == CLASS_EXTERIOR {
            return None;
        }
        let (scale, center_mm) = if class == CLASS_INTERIOR {
            let s = stone_scale(&norm, leaf.orient, usable).max(0.0);
            (s, [0, 1, 2].map(|i| 0.5f64.mul_add(size[i], origin[i])))
        } else {
            let region = ClipRegion {
                min: b_min,
                max: b_max,
                planes: self.non_box,
                violated: &self.violated,
            };
            self.solver
                .solve(&region, caliper_extents(&norm, leaf.orient))?
        };
        if scale <= 0.0 || scale < self.settings.min_width_mm {
            return None;
        }
        Some(self.placed(design, &norm, leaf.orient, origin, size, (scale, center_mm)))
    }

    /// The finished stone of `design` at scale `fit.0` centred at `fit.1`.
    fn placed(
        &self,
        design: &CandidateDesign,
        norm: &Norm,
        orient: usize,
        origin: [f64; 3],
        size: [f64; 3],
        fit: (f64, [f64; 3]),
    ) -> PlacedStone {
        let (s, center_mm) = fit;
        let assign = &ASSIGNMENTS[orient];
        let mut stone_size = [0.0; 3];
        for (dim, axis) in norm.dims().iter().zip(assign) {
            stone_size[*axis] = s * dim;
        }
        let volume_mm3 = norm.f * (s * s * s);
        let mm_per_unit = if design.width > 0.0 {
            s / design.width
        } else {
            0.0
        };
        PlacedStone {
            entry_id: design.entry_id,
            piece_origin_mm: origin,
            piece_size_mm: size,
            stone_size_mm: stone_size,
            table_axis: Axis::from_index(assign[2]),
            carat: volume_mm3 * self.settings.specific_gravity / 200.0,
            volume_mm3,
            pose: StonePose {
                center_mm,
                axes: assignment_axes(assign),
                mm_per_unit,
            },
        }
    }
}

/// Builds the public [`RoughLayout`] for a shaped rough from a guillotine `tree`.
///
/// The first piece sits at `ctx.origin_mm` and the others follow one kerf
/// apart. Each piece is evaluated at its continuous coordinates, resolving the
/// LP boundary clipping for partial pieces. A piece that cannot hold a stone is
/// merged into its neighbour (see [`compact_tree`]), so `stones` always matches
/// the pieces of the resulting `cut_plan` one to one; a tree without any stone
/// gives an empty layout. Yield is relative to the model's solid volume.
#[must_use]
pub fn layout_from_tree_shaped(
    ctx: &ShapedCtx,
    order: CutOrder,
    tree: &Tree,
    pool: &[CandidateDesign],
    settings: &PlanSettings,
) -> RoughLayout {
    let mut tree = tree.clone();
    let mut fitter = StoneFitter::new(settings, pool, &ctx.non_box);
    let stones = compact_tree(
        order,
        &mut tree,
        ctx.origin_mm,
        settings.kerf_mm,
        &mut |origin, size, leaf| fitter.fit(origin, size, leaf),
    );
    assemble_layout(order, &tree, stones, ctx.model_volume)
}

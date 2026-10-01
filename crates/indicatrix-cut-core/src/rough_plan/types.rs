//! The planner's public data types: the rough, the settings, the candidate
//! designs, and the finished [`RoughLayout`] with its cut plan.

use std::{collections::BTreeMap, fmt};

pub use super::fit::StonePose;

/// Why [`PlanSettings::validate`] or [`RoughBlock::validate`] rejected an input.
///
/// The planners never act on an input that fails validation: [`plan_rough`](super::plan_rough)
/// and [`plan`](super::plan::plan) answer it with an empty result (nothing can be cut), and a
/// caller that wants the reason calls `validate` itself first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanInputError {
    /// The stone count is outside `1..=99`.
    CountOutOfRange(u8),
    /// A setting is not finite, is negative where it may not be, or is not positive where it
    /// must be.
    Setting {
        /// The offending field of [`PlanSettings`].
        field: &'static str,
        /// Its value.
        value: f64,
    },
    /// A rough extent is not finite and positive.
    Extent {
        /// The offending axis.
        axis: Axis,
        /// Its extent in mm.
        value: f64,
    },
}

impl fmt::Display for PlanInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CountOutOfRange(count) => {
                write!(f, "the stone count {count} is outside 1 to 99")
            }
            Self::Setting { field, value } => {
                write!(f, "the setting {field} = {value} is not usable")
            }
            Self::Extent { axis, value } => {
                write!(
                    f,
                    "the rough extent along {axis} = {value} must be finite and positive"
                )
            }
        }
    }
}

impl std::error::Error for PlanInputError {}

/// The rough: an axis-aligned block, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoughBlock {
    /// Extent along the x axis, in mm.
    pub x_mm: f64,
    /// Extent along the y axis, in mm.
    pub y_mm: f64,
    /// Extent along the z axis, in mm.
    pub z_mm: f64,
}

impl RoughBlock {
    /// The three extents as `[x, y, z]`.
    #[must_use]
    pub const fn sizes(&self) -> [f64; 3] {
        [self.x_mm, self.y_mm, self.z_mm]
    }

    /// The block's volume in mm^3.
    #[must_use]
    pub fn volume_mm3(&self) -> f64 {
        self.x_mm * self.y_mm * self.z_mm
    }

    /// Checks that every extent is finite and positive.
    ///
    /// # Errors
    ///
    /// [`PlanInputError::Extent`] naming the first axis (x, y, z) that is not.
    pub fn validate(&self) -> Result<(), PlanInputError> {
        for (index, value) in self.sizes().into_iter().enumerate() {
            if !value.is_finite() || value <= 0.0 {
                return Err(PlanInputError::Extent {
                    axis: Axis::from_index(index),
                    value,
                });
            }
        }
        Ok(())
    }
}

/// The planner's inputs other than the rough and the candidate designs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanSettings {
    /// The MAXIMUM number of stones to cut (`1..=99`); a layout may hold any
    /// count from 1 up to this.
    pub count: u8,
    /// Saw kerf lost at every cut, in mm.
    pub kerf_mm: f64,
    /// Preforming and polishing allowance per side of every piece, in mm.
    pub allowance_mm: f64,
    /// Rough skin trimmed off every outer face, in mm.
    pub skin_mm: f64,
    /// Smallest acceptable finished stone width, in mm.
    pub min_width_mm: f64,
    /// Specific gravity of the material (carats = mm^3 * SG / 200).
    pub specific_gravity: f64,
}

/// A stone-count cap as a `usize`, clamped to `1..=99`.
pub fn clamp_count(count: u8) -> usize {
    usize::from(count.clamp(1, 99))
}

impl PlanSettings {
    /// The stone-count cap as a `usize`, clamped to `1..=99`.
    #[must_use]
    pub fn count_usize(&self) -> usize {
        clamp_count(self.count)
    }

    /// Checks that the settings describe a cut the planner can reason about: a count in
    /// `1..=99`; finite, non-negative kerf, allowance, skin and specific gravity; a finite,
    /// positive minimum width.
    ///
    /// A skin or allowance that leaves nothing of a small rough is NOT an error here: that is
    /// a legitimate input with an empty answer.
    ///
    /// # Errors
    ///
    /// The first offending field, in declaration order.
    pub fn validate(&self) -> Result<(), PlanInputError> {
        if !(1..=99).contains(&self.count) {
            return Err(PlanInputError::CountOutOfRange(self.count));
        }
        let non_negative = [
            ("kerf_mm", self.kerf_mm),
            ("allowance_mm", self.allowance_mm),
            ("skin_mm", self.skin_mm),
        ];
        for (field, value) in non_negative {
            if !value.is_finite() || value < 0.0 {
                return Err(PlanInputError::Setting { field, value });
            }
        }
        if !self.min_width_mm.is_finite() || self.min_width_mm <= 0.0 {
            return Err(PlanInputError::Setting {
                field: "min_width_mm",
                value: self.min_width_mm,
            });
        }
        if !self.specific_gravity.is_finite() || self.specific_gravity < 0.0 {
            return Err(PlanInputError::Setting {
                field: "specific_gravity",
                value: self.specific_gravity,
            });
        }
        Ok(())
    }
}

impl Default for PlanSettings {
    /// One stone, kerf 0.30 mm, allowance 0.20 mm per side, no skin, minimum
    /// width 1.00 mm, quartz-like specific gravity 2.65.
    fn default() -> Self {
        Self {
            count: 1,
            kerf_mm: 0.30,
            allowance_mm: 0.20,
            skin_mm: 0.0,
            min_width_mm: 1.0,
            specific_gravity: 2.65,
        }
    }
}

/// One design the planner may cut, measured on the finished solid in model
/// units (only ratios matter; the planner scales the stone to fit).
///
/// `width <= length` (the smaller and larger horizontal extents; the
/// rotation-invariant caliper pair), `height` is the y extent, and `volume`
/// the solid's volume. All four must be finite and positive; the planner
/// drops any candidate that is not, and swaps a reversed width/length pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateDesign {
    /// The library entry id (also the deterministic tie-break).
    pub entry_id: i64,
    /// Smaller horizontal extent.
    pub width: f64,
    /// Larger horizontal extent.
    pub length: f64,
    /// Height (crown to culet).
    pub height: f64,
    /// Solid volume.
    pub volume: f64,
}

/// A rough axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    /// The x axis.
    X,
    /// The y axis.
    Y,
    /// The z axis.
    Z,
}

impl Axis {
    /// The axis with index `0` (x), `1` (y) or `2` (z); anything else is z.
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::X,
            1 => Self::Y,
            _ => Self::Z,
        }
    }

    /// `0` for x, `1` for y, `2` for z.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

impl fmt::Display for Axis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        })
    }
}

/// The order of the three saw stages (slabs, then bars, then pieces).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CutOrder {
    /// Slabs across X, bars across Y, pieces across Z.
    Xyz,
    /// Slabs across X, bars across Z, pieces across Y.
    Xzy,
    /// Slabs across Y, bars across X, pieces across Z.
    Yxz,
    /// Slabs across Y, bars across Z, pieces across X.
    Yzx,
    /// Slabs across Z, bars across X, pieces across Y.
    Zxy,
    /// Slabs across Z, bars across Y, pieces across X.
    Zyx,
}

impl CutOrder {
    /// All six orders, in the fixed order the planner enumerates them.
    pub const ALL: [Self; 6] = [
        Self::Xyz,
        Self::Xzy,
        Self::Yxz,
        Self::Yzx,
        Self::Zxy,
        Self::Zyx,
    ];

    /// The rough axes `[stage 1, stage 2, stage 3]` as indices (`0` = x).
    #[must_use]
    pub const fn axes(self) -> [usize; 3] {
        match self {
            Self::Xyz => [0, 1, 2],
            Self::Xzy => [0, 2, 1],
            Self::Yxz => [1, 0, 2],
            Self::Yzx => [1, 2, 0],
            Self::Zxy => [2, 0, 1],
            Self::Zyx => [2, 1, 0],
        }
    }

    /// The position of this order in [`CutOrder::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Xyz => 0,
            Self::Xzy => 1,
            Self::Yxz => 2,
            Self::Yzx => 3,
            Self::Zxy => 4,
            Self::Zyx => 5,
        }
    }

    /// The order whose stages cut `axes` (a permutation of `0..3`); any other
    /// input maps to [`CutOrder::Xyz`].
    #[must_use]
    pub fn from_axes(axes: [usize; 3]) -> Self {
        Self::ALL
            .into_iter()
            .find(|order| order.axes() == axes)
            .unwrap_or(Self::Xyz)
    }
}

impl fmt::Display for CutOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c] = self.axes().map(Axis::from_index);
        write!(f, "{a}, then {b}, then {c}")
    }
}

/// One finished stone and the piece of rough it comes from. All triples are
/// `[x, y, z]` in rough coordinates, in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedStone {
    /// The design's library entry id.
    pub entry_id: i64,
    /// Corner of the sawn piece nearest the rough's origin.
    pub piece_origin_mm: [f64; 3],
    /// Size of the sawn piece (before the preform/polish allowance).
    pub piece_size_mm: [f64; 3],
    /// Size of the finished stone's bounding box in the rough's axes.
    pub stone_size_mm: [f64; 3],
    /// The rough axis the stone's table faces (its height runs along it).
    pub table_axis: Axis,
    /// Finished weight in carats.
    pub carat: f64,
    /// Finished volume in mm^3.
    pub volume_mm3: f64,
    /// Spatial placement of the stone in the rough.
    ///
    /// For a box-model stone (everything the piece DP, the uniform pass and the
    /// refinement place) the pose is defined only up to the box's symmetries:
    /// the stone is its axis-aligned bounding box, so every rotation that maps
    /// the box onto itself (at least the turns by 180 degrees about its three
    /// axes) describes the same stone. The planner picks
    /// `axes[0] = +e_a` (the width), `axes[1] = +e_b` (the table normal) and
    /// `axes[2] = axes[0] x axes[1]`, which keeps the frame right-handed. Only
    /// an exact single-stone fit carries a pose that is unique.
    pub pose: super::fit::StonePose,
}

/// One bar of a slab: its width (stage 2) and the lengths of its pieces
/// (stage 3), in mm.
#[derive(Debug, Clone, PartialEq)]
pub struct BarCut {
    /// Extent along the stage-2 axis, in mm.
    pub width_mm: f64,
    /// Extent of each piece along the stage-3 axis, in mm.
    pub pieces_mm: Vec<f64>,
}

/// One slab: its thickness (stage 1) and its bars.
#[derive(Debug, Clone, PartialEq)]
pub struct SlabCut {
    /// Extent along the stage-1 axis, in mm.
    pub thickness_mm: f64,
    /// The bars the slab is cut into.
    pub bars: Vec<BarCut>,
}

/// The full staged guillotine plan, in cutting order.
#[derive(Debug, Clone, PartialEq)]
pub struct CutPlan {
    /// The slabs, in position order along the stage-1 axis.
    pub slabs: Vec<SlabCut>,
}

/// One complete layout: what is cut, where, and the totals.
///
/// `stones` runs in traversal order: slab by slab, bar by bar, piece by piece,
/// matching `cut_plan` one to one: `stones.len()` equals the number of pieces
/// in `cut_plan`. (An exact single-stone fit is a single piece holding its
/// stone's bounding box.)
#[derive(Debug, Clone, PartialEq)]
pub struct RoughLayout {
    /// The saw stages' axis order.
    pub cut_order: CutOrder,
    /// The stones, in cut-plan traversal order.
    pub stones: Vec<PlacedStone>,
    /// The sizes of the slabs, bars and pieces.
    pub cut_plan: CutPlan,
    /// Total finished weight in carats.
    pub total_carat: f64,
    /// Total finished volume in mm^3.
    pub total_volume_mm3: f64,
    /// `total_volume_mm3` over the reference volume: the rough block's volume for block
    /// plans, the rough model's volume (the convex polytope left after its cuts) for shaped
    /// plans and exact single-stone fits. `0.0` when that volume is not positive.
    pub yield_fraction: f64,
    /// Whether this layout is an exact single-stone fit (the stone's real outline placed
    /// freely inside the rough) rather than a sawn layout. An exact fit has one stone whose
    /// piece is its own bounding box and no saw stages to follow; readers must not present
    /// its `cut_plan` as sawing instructions.
    pub exact_fit: bool,
}

impl RoughLayout {
    /// How many stones this layout cuts.
    #[must_use]
    pub const fn stone_count(&self) -> usize {
        self.stones.len()
    }

    /// The composition: `(entry_id, count)` pairs sorted by `entry_id`.
    #[must_use]
    pub fn composition(&self) -> Vec<(i64, usize)> {
        let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
        for stone in &self.stones {
            *counts.entry(stone.entry_id).or_insert(0) += 1;
        }
        counts.into_iter().collect()
    }
}

/// Candidate layouts that share one re-pick pool for the refinement.
///
/// The refinement may swap a leaf's design for any design in `pool`. An empty
/// `pool` restricts every layout to the designs it already uses (the
/// single-design layouts of [`uniform_layouts`](super::uniform_layouts)).
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutGroup {
    /// The designs the refinement may re-pick from, sorted by `entry_id`.
    pub pool: Vec<CandidateDesign>,
    /// The layouts to refine.
    pub layouts: Vec<RoughLayout>,
}

/// Where the planner is, for the caller's progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanProgress {
    /// Pruning dominated designs.
    Pareto,
    /// Filling the piece tables. The size table reports one event per plane of the
    /// first axis (`total` = its plane count). The clipped table reports one event
    /// before every 1024th table entry of its slice, `done` counting from 1 and `total`
    /// = ceil(entries of the slice / 1024); a sliced or threaded table reports the sum
    /// over its slices, which a caller can precompute with `shaped::grid_poll_events`
    /// and `shaped::slice_entry_range`. Alternative rounds report their own Grid and
    /// Dp events between the `Alternatives` events.
    Grid {
        /// Planes finished.
        done: usize,
        /// Planes in all.
        total: usize,
    },
    /// Running the DP of one cut order (`order` counts from 0).
    Dp {
        /// Which order, from 0.
        order: usize,
        /// How many orders in all.
        of: usize,
    },
    /// The single-design pass: every design in every fully filled grid of equal cells.
    /// Reported about every 256 designs, and once more when the pass is complete.
    Uniform {
        /// Designs scanned so far.
        done: usize,
        /// Designs to scan in all.
        total: usize,
    },
    /// The leave-one-out mixed alternatives.
    Alternatives {
        /// Alternatives finished.
        done: usize,
        /// Alternatives in all.
        total: usize,
    },
    /// The continuous refinement of the best candidates.
    Refine,
    /// Fitting single stones inside the rough.
    Fit {
        /// The active sub-stage of fitting.
        stage: super::fit::FitStage,
        /// Units finished in this stage.
        done: usize,
        /// Total units in this stage.
        total: usize,
    },
}

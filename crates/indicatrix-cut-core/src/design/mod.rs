//! A design: a preform plus an editable cutting schedule of *authored
//! constraints*, and the derivation from that state down to a renderable solid.
//!
//! # Constraints, not masts
//!
//! A faceter does not think "cut this facet to depth 0.6499" -- they think "this
//! facet meets the junction of those three." The mast number a `.asc` file records
//! is the *output* of that intent, not the intent itself. [`ConstraintTier`] runs
//! that forwards instead: the constraint
//! ([`indicatrix::geometry::meet_solver::MeetConstraint`], the exact three-case enum
//! the solver classifies real schedules into) is the field a tier's own mutation
//! methods touch; the mast is never stored on a tier at all, only produced by
//! [`Design::solve`] on demand. This is what makes the editor CAD instead of a text
//! form over `.asc` fields.
//!
//! # Scale anchoring
//!
//! Meet-vertex incidence has a real, corpus-measured blind spot: it cannot recover a
//! block's (crown/pavilion/girdle) own translation along its normal, because a
//! coherent shift of every plane in a block preserves every vertex the block's own
//! facets pass through (see `indicatrix::geometry::meet_solver`'s module docs, "What
//! meets alone cannot determine"). A block therefore needs exactly one
//! [`MeetConstraint::ScaleReference`] tier -- a real authored dimension, matching a
//! real schedule's own "Set stone size"/"Level girdle" instructions -- or
//! [`Design::solve`] cannot produce a mast for anything in that block at all.
//!
//! This crate deliberately does not use
//! [`indicatrix::geometry::meet_solver::apply_ratio_anchors`] (the corpus tool's
//! estimate-from-printed-ratios fallback, 10-40% error): an in-progress editor design
//! has no printed ratios to estimate from either, so the only input it could be fed
//! here would itself be another guess. [`Design::from_asc_schedule`] instead
//! synthesizes a missing block anchor from its own first tier's real recorded mast
//! (not a fitted ratio) while that mast is still available on import -- see that
//! function's doc comment. A block with no tiers at all gets no anchor manufactured
//! either: [`Design::solve`] reports exactly which block(s) are missing one via
//! [`MissingAnchor`], and the editor must ask the user for a real
//! `MeetConstraint::ScaleReference` tier rather than silently picking a number.

mod construct;
mod export;
mod missing_anchor;
mod solve;
#[cfg(test)]
mod tests;
mod tier;

pub use construct::FreshDesignSpec;
pub use missing_anchor::MissingAnchor;
pub use tier::{ConstraintTier, ScheduleMeta};

use crate::{material::MaterialSelection, preform::PreformSpec};

/// A preform plus the cutting schedule being edited against it.
///
/// `tiers` is authored constraint state (see the module docs); `meta` is every
/// other `.asc` schedule field. There is no single `AscSchedule` field --
/// [`Design::to_asc_schedule`] builds one on demand (solving every tier's mast
/// first), for [`Design::planes`] and for a caller that wants to write a real
/// `.asc` file via `indicatrix_formats::asc::to_asc_string`. [`Design::from_asc_schedule`] is
/// the reverse direction, for loading one.
#[derive(Debug, Clone, PartialEq)]
pub struct Design {
    pub preform: PreformSpec,
    pub meta: ScheduleMeta,
    pub tiers: Vec<ConstraintTier>,
    /// The design's real-world girdle diameter, in millimetres -- the one dimension
    /// a lapidary actually measures with calipers on a finished stone, and this
    /// crate's sole anchor from the otherwise-dimensionless "mast unit" scale every
    /// plane offset uses (see `crate::preform`'s own module doc comment) to a real
    /// physical size. `None` means "not anchored yet": every figure
    /// `crate::yield_metrics` derives from it (mm-scale volume, carat weight) is
    /// then unavailable, while [`crate::yield_metrics::volumetric_yield`] stays
    /// available regardless -- it needs no mm scale at all.
    ///
    /// `crate::yield_metrics::mm_per_unit` is `girdle_diameter_mm` divided by the
    /// SOLVED design's own measured girdle width
    /// (`indicatrix::geometry::stone_metrics::SolidMetrics::width_axis`), not the
    /// preform's.
    ///
    /// # This does NOT round-trip through `.asc`
    ///
    /// `indicatrix_formats::asc::AscSchedule` has no field for a real-world dimension anywhere
    /// -- every number in a `.asc` file is a dimensionless mast. Exporting a design
    /// to `.asc` drops `girdle_diameter_mm` silently, and importing never populates
    /// it either. It lives only in this in-memory `Design` for now; the native
    /// format (`crate::native`) is what carries it to disk.
    pub girdle_diameter_mm: Option<f64>,
    /// Which material's specific gravity to estimate this design's carat weight
    /// from, plus any per-design override -- see
    /// [`crate::material::MaterialSelection`]'s own doc comment. A design input,
    /// exactly like `preform` above: mutated only through `crate::edit::History`,
    /// and (like `girdle_diameter_mm`) not preserved across an `.asc` round trip for
    /// the same reason.
    pub material: MaterialSelection,
}

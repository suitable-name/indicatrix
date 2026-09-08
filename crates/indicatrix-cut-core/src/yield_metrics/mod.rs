//! binding a dimensionless design to real millimetres via the girdle
//! diameter, and the yield/weight figures that follow from that binding.
//!
//! # Why `mast` needed binding at all
//!
//! `Design::solve`'s masts, and therefore [`indicatrix::geometry::stone_metrics::measure_solid`]'s
//! `volume`/`width_axis`/etc., are all in an arbitrary "mast unit" -- `crate::preform`'s
//! own module doc comment already says so for the preform, and it is equally true of
//! every solved tier. Carat yield -- the commercial metric a cutter actually cares
//! about -- needs a real unit. [`mm_per_unit`] is the one anchor this crate adds:
//! the girdle diameter is the dimension a lapidary actually measures with calipers on
//! a finished stone, so `Design::girdle_diameter_mm` (a real, user-supplied
//! millimetre figure) divided by the solved design's own measured
//! [`width_axis`](indicatrix::geometry::stone_metrics::SolidMetrics::width_axis) gives
//! millimetres per model unit. Every other real-world
//! figure in this module (volume in mm^3, carat weight) is downstream of that one
//! ratio.
//!
//! # Two numbers -- never conflate them
//!
//! - [`volumetric_yield`]: finished solid volume over preform volume. **Exact**, and
//!   needs no mm scale or material data at all -- see that function's own doc comment
//!   for why the `mm_per_unit^3` factor cancels out of the ratio entirely. This number
//!   is a fact about the geometry alone.
//! - [`carat_weight`]: `volume_mm3 * specific_gravity / 200`. **An estimate** --
//!   it inherits whatever error `specific_gravity` itself carries (see
//!   `crate::material`'s module doc comment for how wide that can be for some
//!   species), on top of whatever error `mm_per_unit`'s own anchor carries. Never
//!   shown with more implied precision than that SG supports.
//!
//! [`crate::design::Design::yield_report`] is the one entry point that ties both
//! together against a design's current, already-solved state -- see that method's own
//! doc comment for why it (like
//! [`crate::manufacturability::check_manufacturability`]) takes an already-
//! [`crate::design::Design::solve`]'d mast list rather than forcing a second solve.
//!
//! # Module layout
//!
//! [`scale`] is the mm-per-unit anchor and the figures downstream of it/of geometry
//! alone; [`fit`] is [`exceeds_preform`]/[`PreformFit`]; [`report`] is
//! [`YieldReport`] and [`crate::design::Design::yield_report`] itself, built on
//! both.

mod fit;
mod report;
mod scale;
#[cfg(test)]
mod tests;

pub use fit::{PreformFit, exceeds_preform};
pub use report::YieldReport;
pub use scale::{carat_weight, mm_per_unit, volume_mm3, volumetric_yield};

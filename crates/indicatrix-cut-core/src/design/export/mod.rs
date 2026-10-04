//! [`crate::design::Design`]'s already-solved-to-output side: building an
//! [`indicatrix_formats::asc::AscSchedule`], the full plane arrangement, and the meshed
//! solid/its measurements -- see [`crate::design::Design::to_asc_schedule_from_solved`]'s doc
//! comment for why each has an "already solved" variant alongside the
//! solve-it-yourself convenience wrapper.

mod concave;
pub mod concave_frame;
mod planes;
mod refractive;
mod schedule;

#[cfg(test)]
mod concave_tests;
#[cfg(test)]
mod tests;

pub use concave::{ConcaveResolveError, FlatAndTools, ToolPlacements};
pub use schedule::{meet_name_is_asc_safe, strip_generated_concave_footnotes};

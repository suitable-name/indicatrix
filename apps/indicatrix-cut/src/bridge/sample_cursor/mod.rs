//! [`SampleCursor`]: the shared claim point every backend contributing to one image
//! draws disjoint absolute sample sub-ranges from -- the export's local and remote
//! lanes, and (through [`live::LiveEpoch`]) the live viewport's local tracer and its
//! remote chunk lane.
//!
//! `SampleCursor` itself now lives in the GUI-free `indicatrix-dispatch` crate (so the
//! render coordinator shares the exact same claim semantics) and is re-exported here
//! unchanged; see its own docs there for disjointness and the retry piles. Every
//! existing `bridge::sample_cursor::SampleCursor` path keeps compiling.
//!
//! [`LiveEpoch`] stays here: it is the live viewport's single-remote-lane bookkeeping
//! (one in-flight `Accumulator`, the app's scene-generation stamp), which the
//! coordinator replaces with `indicatrix_dispatch::LanePool` rather than reuses.

pub mod live;
/// Unit tests for [`LiveEpoch`], plus the synthetic-`FRAME` and partition-check helpers
/// the live-lane and CPU-split tests elsewhere in `bridge` reuse. `SampleCursor`'s own
/// tests moved with it to `indicatrix-dispatch`.
#[cfg(test)]
pub(in crate::bridge) mod tests;

pub use indicatrix_dispatch::SampleCursor;
pub use live::{ChunkEnd, LiveEpoch};

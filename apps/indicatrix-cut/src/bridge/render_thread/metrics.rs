//! The gemological-metrics cache: recomputing `evaluate_gem_optical_metrics`/
//! `evaluate_angular_profile` is expensive (single-threaded analytical raytracing), and
//! its result depends only on a handful of inputs that don't change between
//! progressive-accumulation samples -- see [`compute_or_reuse_metrics`].
//!
//! The cache itself lives in `indicatrix::color::metrics` (moved there so the browser
//! app's solve Worker recomputes on exactly the same changes); this module only names
//! it for the render loop.

/// Cheap identity for a facet-plane set -- moved to `indicatrix::render_setup::
/// plane_hash` (see that module's doc comment) so every per-design cache in this
/// crate and the browser app key on the exact same identity. Re-exported at this path
/// so nothing else in this crate needs to change: `bridge::frame_cache::girdle_finish`/
/// `stone_width` and `bridge::frame_cache::guide_pass::GuideCache` all reuse this exact
/// hash as part of their own cache-invalidation key rather than a second, parallel
/// implementation.
pub use indicatrix::render_setup::hash_planes;

pub(super) use indicatrix::color::metrics::{MetricsCache, compute_or_reuse_metrics};

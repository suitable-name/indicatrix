//! Zone geometry for the colour fit (plan 2026-10-09, section 7.3; lane G1). Only built with the
//! `zoning` feature. No user interface: the window code calls these functions.
//!
//! The pieces, all pure and deterministic (fixed iteration caps, no randomness, no hashed
//! collections in results):
//!
//! * [`fit`]: primitive fits from 3D boundary points. [`fit_half_space`], [`fit_slab`],
//!   [`fit_cylinder`], [`fit_prism`] and [`fit_sector`] turn the points of
//!   `locate::locate_polyline` ([`BoundaryPoints`]) into a [`FittedZoneShape`] with the residual
//!   and a leave-one-view-out check. The cylinder and prism take an optional fixed axis (the
//!   crystal's c axis).
//! * [`edit`]: [`ZoneEdit`] operations and the pure [`apply`] / [`apply_with_locks`], each result
//!   validated by `ZonedAbsorption::validate`, plus [`ZoneLocks`].
//! * [`overlay`]: [`project_overlay`], the per-view polylines where the zone boundaries meet the
//!   mesh surface, for drawing over the photos (surface trace, refraction ignored).
//! * [`suggest`]: [`suggest_zones`], candidate [`ZoneSuggestion`]s from the structure of the
//!   residual or of the transmittance images (k-means in Lab, smoothing, back-projection through
//!   the `locate` trace, primitive fits). Never applied automatically.
//! * [`refine`]: [`refine_zone_geometry`], a Gauss-Newton refinement of the boundary offsets and
//!   radii against the photos through the forward tracer and the solver, at most 3 iterations.
//!
//! # Frames
//!
//! Every shape produced here is in the MESH frame in millimetres (the frame of
//! `locate::Located::point`), meant for a `ZonedAbsorption` with an identity frame. The forward
//! tracer takes the zone geometry in the mesh frame too.

#![allow(
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    clippy::type_complexity,
    reason = "small dense numerics and index-parallel arrays read better as written"
)]

mod edit;
mod fit;
mod geometry;
mod overlay;
mod refine;
mod suggest;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_pipeline;

pub use edit::{
    ZoneEdit, ZoneEditError, ZoneLocks, ZoneParameter, apply, apply_with_locks, parameter_value,
};
pub use fit::{
    BoundaryPoints, FittedZoneShape, LeftOutView, RadialRole, ZoneFitError, fit_cylinder,
    fit_half_space, fit_prism, fit_sector, fit_slab,
};
pub use overlay::{
    OverlayOptions, OverlayPolyline, SurfaceTrace, ViewOverlay, project_overlay, surface_traces,
};
pub use refine::{
    DEFAULT_REFINE_ITERATIONS, Evaluation, MAX_REFINE_ITERATIONS, ParameterMove, RefineError,
    RefineOptions, RefineProgress, RefineResult, free_parameters, refine_with,
    refine_zone_geometry,
};
pub use suggest::{
    BackProjection, NO_LABEL, Segmentation, SuggestError, SuggestKind, SuggestOptions,
    SuggestReport, SuggestView, ViewLabels, ZoneSuggestion, segment_views, suggest_zones,
};

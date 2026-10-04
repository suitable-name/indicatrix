//! GIA/AGSL-style optical gemological metrics (brilliance, fire,
//! scintillation, windowing, extinction).
//!
//! Evaluated by firing an analytical ray fan through a stone's facet geometry from the
//! observer's point of view, under the lighting the renderer itself uses.
//!
//! # What is measured
//!
//! Each evaluation traces an 18x18 grid of rays (the cells inside a disc, see below), each
//! with a five-ray sub-aperture bundle, at the d-line. Every ray is classified once:
//!
//! - **Windowing**: the ray leaks out through the pavilion instead of returning.
//! - **Brilliance**: the ray leaves upward through the crown and is *visibly returned* --
//!   outside the observer's 16 degree head-shadow cone and looking at a light source (see
//!   the lighting model below).
//! - **Extinction**: every other ray -- trapped, absorbed, head-shadowed or looking at the
//!   dim surround. The three percentages are shares of all the rays that hit the stone;
//!   a ray blocked at entry counts in the total but in none of the three.
//! - **Fire**: for each returned ray, the angle between its F-line and C-line exit
//!   directions, weighted by the product of the three wavelengths' Fresnel transmittances
//!   and the two exit cosines, summed over the rays and divided by the total rays. Only
//!   pairs that exit through the same facet after the same number of bounces count.
//! - **Scintillation**: the spatial contrast (coefficient of variation) of the per-cell
//!   return fraction, squashed into 0-100, blended 60/40 with the temporal flicker of the
//!   cells' return status as the camera yaws through +/-3 degrees.
//!
//! # Lighting model
//!
//! The metrics take the same [`EnvironmentSource`](crate::optics::raytracer::EnvironmentSource)
//! the tracer renders with, so a preset or light change moves the numbers the way it moves
//! the picture. A ray counts as looking at
//! a light source when the environment's radiance along its exit direction exceeds a
//! multiple of the environment's ambient level (the median radiance over the upper
//! hemisphere): for the four studio presets that is the very key softbox, fill softbox and
//! ring pinpoint lobes the tracer evaluates, for the light tent its overhead softbox and
//! spark light, for the daylight dome its sun, for an HDR panorama its bright regions.
//! The ISO hemisphere is uniformly radiant, so every direction it radiates into counts.
//! The observer's head-shadow cone is applied on top, separately. The details and their
//! edge cases are on the private `lighting` module.
//!
//! # Fan scale
//!
//! The grid is scaled to the stone: the sampled disc reaches 95% of the measured girdle
//! half-width ([`render_setup::measure_model_width`] halved; 1.0 model unit when the
//! design does not measure), and the rays start two and a half half-widths from the
//! stone's centre. An elongated outline is covered along its short axis only. See the
//! private `fan` module.
//!
//! [`render_setup::measure_model_width`]: crate::render_setup::measure_model_width
//!
//! # Layout
//!
//! Split by responsibility: [`camera`] (the shared observer-PoV basis, read off the render
//! camera), [`fan`] (where the rays start), [`lighting`] (the illumination model),
//! [`types`] (the [`GemOpticalMetrics`] result and the 19-point elevation grid),
//! [`ray_trace`] (the single-wavelength refract-then-bounce physics core), [`visibility`]
//! (the head-shadow/illumination "is this ray visibly returned" test), [`scintillation`]
//! (the spatial+temporal sparkle terms), [`classify`] (per-aperture-sample ray
//! classification and the Fire bifurcation gate), [`evaluate`] (the main grid loop,
//! [`evaluate_gem_optical_metrics`]), [`profile`] (the angular-profile sweeps built on top
//! of it), [`sweep`] (the four-axis tilt sweep) and [`cache`] (reuse of the results while
//! their inputs, lighting included, do not change).

mod cache;
mod camera;
mod classify;
mod evaluate;
mod fan;
mod lighting;
mod profile;
mod ray_trace;
mod scintillation;
mod sweep;
mod types;
mod visibility;

pub use cache::{
    MetricsCache, MetricsCacheKey, compute_or_reuse_metrics, compute_or_reuse_metrics_geom,
    compute_or_reuse_pose_metrics, compute_or_reuse_pose_metrics_geom,
};
pub use camera::camera_view_basis;
pub use evaluate::{evaluate_gem_optical_metrics, evaluate_gem_optical_metrics_geom};
pub use profile::{
    EVALUATIONS_PER_AXIS, PROFILE_AZIMUTHS_DEG, TILT_ANGLES_DEG, evaluate_angular_profile,
    evaluate_angular_profile_at_azimuth, evaluate_angular_profile_at_azimuth_geom,
    evaluate_angular_profile_geom, evaluate_full_axis_profile_at_azimuth,
    evaluate_full_axis_profile_at_azimuth_geom, evaluate_full_axis_profile_at_azimuth_stepped,
    evaluate_full_axis_profile_at_azimuth_stepped_geom,
};
pub use sweep::{
    AxisProfile, SweepProgress, evaluate_all_axes_profiles, evaluate_all_axes_profiles_geom,
    evaluate_all_axes_profiles_stepped, evaluate_all_axes_profiles_stepped_geom, total_evaluations,
};
pub use types::{GemOpticalMetrics, PROFILE_ANGLES_DEG};

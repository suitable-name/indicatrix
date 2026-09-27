//! GIA/AGSL-style optical gemological metrics (brilliance, fire,
//! scintillation, windowing, extinction).
//!
//! Evaluated by firing an analytical
//! ray fan through a stone's facet geometry from the observer's point of
//! view.
//!
//! Split by responsibility: [`camera`] (the shared observer-PoV basis),
//! [`types`] (the [`GemOpticalMetrics`] result and the 19-point elevation
//! grid), [`ray_trace`] (the single-wavelength refract-then-bounce physics
//! core), [`visibility`] (the head-shadow/illumination "is this ray visibly
//! returned" test), [`scintillation`] (the spatial+temporal sparkle terms),
//! [`classify`] (per-aperture-sample ray classification and the Fire
//! bifurcation gate), [`evaluate`] (the main grid loop,
//! [`evaluate_gem_optical_metrics`]), and [`profile`] (the angular-profile
//! sweeps built on top of it).

mod camera;
mod classify;
mod evaluate;
mod profile;
mod ray_trace;
mod scintillation;
mod types;
mod visibility;

pub use camera::camera_view_basis;
pub use evaluate::evaluate_gem_optical_metrics;
pub use profile::{
    PROFILE_AZIMUTHS_DEG, TILT_ANGLES_DEG, evaluate_angular_profile,
    evaluate_angular_profile_at_azimuth, evaluate_full_axis_profile_at_azimuth,
};
pub use types::{GemOpticalMetrics, PROFILE_ANGLES_DEG};

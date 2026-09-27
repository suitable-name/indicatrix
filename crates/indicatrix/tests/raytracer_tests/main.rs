//! Integration tests for `indicatrix`'s spectral raytracer, colour pipeline, and
//! optical-metrics machinery. Split by topic across the sibling modules below;
//! `fixtures` holds helpers shared across more than one topic module.

mod absorption_bands;
mod colorimetry;
mod dispersion_fire;
mod fixtures;
mod geometry_optics_basics;
mod golden_regression;
mod illuminant_color_shift;
mod material_defaults_birefringence;
mod material_equivalence_finish;
mod optical_metrics;
mod pleochroic_biaxial_wiring;
mod pleochroic_orientation;
mod studio_rendering_basics;
mod white_furnace_energy_conservation;

//! Converts a fixed-length tilt-angle curve into an SVG path-data string for
//! `Path::commands` in `performance_graph_dialog.slint`.
//!
//! The functions (and their tests) live in `indicatrix_editor::view_model::curve_path`,
//! moved there so the browser app draws its tilt chart from the same code; this module
//! keeps the desktop's paths working.

pub use indicatrix_editor::view_model::curve_path::{full_axis_curve_path, tilt_curve_path};

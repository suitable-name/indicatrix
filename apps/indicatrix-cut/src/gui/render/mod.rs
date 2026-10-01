//! The live-viewport rendering/lighting/material stack: camera and lighting callback
//! wiring ([`camera_lighting`]), saved lighting presets ([`lighting_presets`]),
//! material selection and render-quality controls ([`material_quality`]), the
//! tab/view-mode logic deciding whether the tracer runs ([`render_visibility`]), the
//! target-samples slider's exponent<->count mapping ([`sample_scale`]), and the
//! high-resolution export flow ([`render_export`]).
//!
//! Groups the render/lighting/material domain into one module tree, with
//! `render_export.rs` (the largest, well over this codebase's ~700-line split
//! threshold) further split into its own [`render_export`] submodule folder.

pub mod camera_lighting;
pub mod lighting_presets;
pub mod material_quality;
pub mod render_export;
pub mod render_visibility;
pub mod sample_scale;

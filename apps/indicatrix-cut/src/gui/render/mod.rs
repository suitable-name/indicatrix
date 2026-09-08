//! The live-viewport rendering/lighting/material stack: camera and lighting callback
//! wiring ([`camera_lighting`]), saved lighting presets ([`lighting_presets`]),
//! material selection and render-quality controls ([`material_quality`]), the
//! detachable Live Render window ([`detached_render`]), the target-samples slider's
//! exponent<->count mapping ([`sample_scale`]), and the high-resolution export flow
//! ([`render_export`]).
//!
//! Was 6 flat top-level `gui` files; grouped here as the render/lighting/material
//! domain, with `render_export.rs` (the largest, well over this codebase's ~700-line
//! split threshold) further split into its own [`render_export`] submodule folder.

pub mod camera_lighting;
pub mod detached_render;
pub mod lighting_presets;
pub mod material_quality;
pub mod render_export;
pub mod sample_scale;

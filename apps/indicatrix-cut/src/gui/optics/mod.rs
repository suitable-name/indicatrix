//! Crystal-optics helpers: crystal-axis tilt/azimuth <-> `Vec3` conversion
//! ([`c_axis`]), crystal-system/optical-character string and index conversions plus
//! custom-`GemMaterial` construction ([`crystal_optics`]), tilt-curve SVG path
//! generation for the performance-graph dialog ([`curve_path`]), and the custom
//! material library's own create/edit/delete callbacks ([`custom_materials`]).
//!
//! Grouped together as the material/crystal-optics domain logic that sits underneath
//! the render/tilt/library UI modules, rather than left as flat top-level `gui` files.

pub mod c_axis;
pub mod crystal_optics;
pub mod curve_path;
pub mod custom_materials;

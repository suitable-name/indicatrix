//! Pure render-setup helpers shared by every renderer built on this crate: the
//! desktop viewer (`apps/indicatrix-cut`, through thin re-exports/adapters at its old
//! paths) and the browser app (`apps/indicatrix-web`), so both build the exact same
//! scene/material for the same settings and produce byte-identical output.
//!
//! Everything here is pure: no threads, no `Instant::now`, no filesystem access, so
//! the whole module compiles for `wasm32-unknown-unknown`. State that genuinely needs
//! to persist across frames (a plane-hash-keyed measurement cache, for instance) stays
//! on the caller's side -- see [`stone_width::measure_model_width`]'s own doc comment.
//!
//! Submodules: [`materials`] (by-name material resolution and the render-time
//! override stack), [`backdrop`] (the `Backdrop` level/index mapping), [`plane_hash`]
//! (the cheap plane-set identity every per-design cache keys on), [`stone_width`] (the
//! pure girdle-width measurement `apply_material_overrides`'s stone-size override
//! needs), [`icc_profile`] (the embedded-profile byte builder for wide-gamut PNG
//! export) and, behind the `hdr` feature (which already pulls in `image`),
//! [`png_encode`] (in-memory PNG+ICC encoding), and [`sample_scale`] (the target
//! samples slider's exponent<->count mapping) and [`inclusion_scale`] (the Inclusion Haze
//! slider's position<->coefficient mapping).

pub mod backdrop;
pub mod icc_profile;
pub mod inclusion_scale;
pub mod materials;
pub mod plane_hash;
#[cfg(feature = "hdr")]
pub mod png_encode;
pub mod sample_scale;
pub mod stone_width;

pub use backdrop::Backdrop;
pub use materials::{
    MODEL_UNIT_FACE_UP_PATH, MaterialOverrides, PHYSICS_DEFAULT_STONE_WIDTH_MM,
    absorption_path_scale_for, apply_material_overrides, effective_stone_width_mm,
    material_for_stone, needs_model_width, resolve_material, resolve_material_with_override,
};
pub use plane_hash::{hash_geometry, hash_planes};
#[cfg(feature = "hdr")]
pub use png_encode::encode_png_with_icc;
pub use stone_width::measure_model_width;

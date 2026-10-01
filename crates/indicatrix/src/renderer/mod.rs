pub mod buffers;
/// Wasm-safe, thread-free per-pixel CPU tracer.
///
/// See the module's own doc comment.
pub mod cpu_frame;
pub mod denoise;
pub mod frame_denoise;
/// The backend-independent frame-scene bundle.
///
/// See the module's own doc comment.
pub mod frame_scene;
pub mod guide_pass;
pub mod tonemap;

// Parses and validates every WGSL shader with `naga` (a dev-dependency available
// regardless of the `gpu` feature) so a shader syntax/type error is a `cargo test`
// failure on any machine, GPU or not -- see that module's own doc comment. Declared
// here, OUTSIDE the `#[cfg(feature = "gpu")]` gate below, so plain `cargo test` (no
// `--features gpu`) actually runs it; the file itself still lives under `gpu/` (its
// `include_str!("../shaders/...")` paths are relative to the file, not this
// declaration, so they are unaffected by which module tree reaches it).
#[cfg(test)]
#[path = "gpu/shader_validation_tests.rs"]
mod shader_validation_tests;

/// CPU-side HDR environment-map loading and importance sampling.
///
/// Always available (no `gpu` feature required); see module docs for the `hdr`
/// feature gating actual file decoding.
pub mod env_map;

#[cfg(feature = "gpu")]
pub mod env_map_gpu;

// GPU compute infrastructure and its mandatory self-tests; see that module's doc comment.
#[cfg(feature = "gpu")]
pub mod gpu;

// Not gated behind `feature = "gpu"`: this decline-and-fall-back wrapper exists in both
// configurations so callers' render loops need no `#[cfg]` of their own.
pub mod gpu_backend;

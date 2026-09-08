//! `indicatrix` is a physically-based spectral gemstone renderer.
//!
//! Turns GemCAD-style cutting schedules (facet angles and index positions)
//! into rendered output: spectral analytical / Monte-Carlo raytracing through
//! faceted gemstone geometry, plus brilliance / fire / scintillation metrics.
//!
//! No UI toolkit or data-source dependency: callers supply plain
//! [`FacetSpec`] rows (or hand-built [`geometry::GpuFacetPlane`] sets) and a
//! [`optics::materials::GemMaterial`], and get back pixels and/or metrics.

// `wgpu`'s type nesting overflows the default recursion limit when proving
// `Send` for the closure `renderer::gpu::hybrid` spawns onto a scoped thread.
// 256 clears current nesting with headroom; raise only if a future `wgpu`
// upgrade nests deeper.
#![recursion_limit = "256"]

pub mod color;
pub mod geometry;
pub mod optics;
pub mod renderer;
pub mod simd;

pub use geometry::cuts::FacetSpec;

/// Deterministic content hash of this crate's source, computed at build time.
///
/// Covers `src/**/*.rs` plus `Cargo.toml`; see `build.rs` for the hashing
/// procedure. 16 lowercase hex chars (64-bit FNV-1a), stable across
/// OS/toolchain/`.git` state for byte-identical source.
///
/// Used by `indicatrix-net`'s handshake: a viewer and remote render worker
/// must refuse to combine samples unless builds match, since differing
/// physics summed together silently produces a plausible-looking wrong image.
pub const BUILD_ID: &str = env!("INDICATRIX_BUILD_ID");

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

/// Build identity used by `indicatrix-net`'s handshake: 16 lowercase hex chars, the
/// 64-bit FNV-1a hash of this crate's version string.
///
/// Keyed on the version so a worker and a viewer built from the same release pair
/// across platforms and checkouts. The physics-parity guarantee this handshake
/// exists for (two implementations summed together render a plausible wrong image)
/// therefore depends on the release rule: bump the version whenever anything under
/// `src/` that affects a traced sample changes. Compare [`SOURCE_HASH`] when in doubt.
pub const BUILD_ID: &str = env!("INDICATRIX_BUILD_ID");

/// This crate's version, as the handshake identity is derived from it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Deterministic content hash of this crate's source tree.
///
/// Covers `src/**/*.rs`, `src/**/*.wgsl` and `Cargo.toml`; 16 lowercase hex chars,
/// stable across OS, toolchain, line endings and `.git` state for byte-identical
/// source. Catches a physics-affecting edit that didn't bump [`VERSION`] (which
/// [`BUILD_ID`] is keyed on, and [`BUILD_ID`] disagreeing is always a hard handshake
/// refusal): `indicatrix_net::handshake::verify_compatible` sends this value on both
/// sides and, when BOTH sides could establish their own hash, a disagreement is
/// ALSO a hard refusal (`Incompatible::SourceHash`) -- not a diagnostic-only field.
/// When either side's hash is unknown (e.g. a build that skipped this crate's
/// `build.rs` hashing step), that side's check is skipped and logged at `warn!`
/// instead, since an unknown hash is not itself evidence of a mismatch; see that
/// module's own doc comment for the full two-tier rationale.
pub const SOURCE_HASH: &str = env!("INDICATRIX_SOURCE_HASH");

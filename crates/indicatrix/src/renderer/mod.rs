pub mod buffers;
pub mod denoise;
pub mod tonemap;

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

#[cfg(feature = "gpu")]
pub mod pipeline;

//! Event wavefront GPU compute renderer.
//!
//! Provides [`EventWavefrontRenderer`], a discrete, per-event compute wavefront renderer
//! designed for WebGPU budget constraints and low-memory architectures.

pub mod accumulate;
pub mod chunk;
pub mod clock;
pub mod compare;
pub mod kernels;
pub mod pipelines;
pub mod readback;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_tests;

pub use accumulate::{
    BYTES_PER_RAY, CHUNK_BUDGET_BYTES, EventWavefrontRenderer, TARGET_CHUNK_MS, chunk_pixels_for,
    next_chunk_pixels,
};
pub use kernels::{EwKernel, KERNEL_SET_HASH, needed_arms, needed_kernels};
pub use pipelines::EwPipelines;

// Re-export buffer twins and constants from `renderer::buffers`
pub use crate::renderer::buffers::{
    ARM_FROSTED, ARM_GENERIC, ARM_TIR, ARM_UNI_ENTRY, ARM_UNI_INTERNAL, EwHit, EwParams, EwRayChan,
    EwRayCore, FLAG_HAVE_PREV_PLANE_NORMAL, FLAG_INSIDE_GEM, FLAG_IS_EXTRAORDINARY,
    FLAG_PATH_ESCAPED, PHASE_END_STEP, PHASE_NEXT, PHASE_TERMINATE, TAG_ARM_BASE, TAG_DONE,
    TAG_EXTEND, TAG_FINALIZE, TAG_NEE_HG, TAG_SCATTER, TAG_SPLIT, TAG_SURFACE, TAG_TAIL,
};

/// Why an event-wavefront GPU frame could not be produced.
#[derive(Debug)]
pub enum EwError {
    /// The scene's material is not GPU-supported.
    UnsupportedMaterial(String),
    /// The scene's environment cannot be dispatched on the GPU.
    UnsupportedEnvironment,
    /// A buffer mapping or readback operation failed.
    BufferMapping(String),
    /// The GPU device was lost or unresponsive.
    DeviceLost(String),
    /// A chunk execution timed out.
    ChunkTimeout,
    /// Invalid accumulator buffer size.
    InvalidBufferSize {
        /// Expected size (pixels).
        expected: usize,
        /// Actual slice length.
        actual: usize,
    },
    /// The stone has more concave tools than the kernels trace
    /// ([`GPU_MAX_TOOL_PRIMITIVES`](crate::geometry::tool::GPU_MAX_TOOL_PRIMITIVES)); the CPU
    /// tracer renders it.
    TooManyTools {
        /// Tools in the stone.
        tools: usize,
        /// The most the kernels trace.
        limit: usize,
    },
}

impl std::fmt::Display for EwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMaterial(name) => {
                write!(f, "{name} is not GPU-supported for event wavefront")
            }
            Self::UnsupportedEnvironment => {
                write!(f, "unsupported environment for event wavefront")
            }
            Self::BufferMapping(msg) => write!(f, "buffer mapping failed: {msg}"),
            Self::DeviceLost(msg) => write!(f, "GPU device lost or unresponsive: {msg}"),
            Self::ChunkTimeout => write!(f, "event wavefront chunk timed out"),
            Self::InvalidBufferSize { expected, actual } => {
                write!(
                    f,
                    "invalid buffer size: expected {expected}, actual {actual}"
                )
            }
            Self::TooManyTools { tools, limit } => write!(
                f,
                "the stone has {tools} concave tools; the event wavefront kernels trace at most \
                 {limit}"
            ),
        }
    }
}

impl std::error::Error for EwError {}

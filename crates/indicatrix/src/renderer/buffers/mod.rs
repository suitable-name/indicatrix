//! GPU-side buffer layouts for the `gpu`-feature compute infrastructure (see
//! `renderer::gpu`) and its mandatory struct-layout self-test
//! (`renderer::gpu::layout_check`).
//!
//! # Why every struct here is designed around WGSL's alignment rules first
//!
//! WGSL's host-shareable layout rules (<https://www.w3.org/TR/WGSL/#alignment-and-size>)
//! are NOT Rust's `#[repr(C)]` rules: a `vec3<f32>`/`vec4<f32>` member must start at a
//! 16-byte-aligned offset in WGSL, while the equivalent Rust `[f32; 3]`/`[f32; 4]` field
//! is only 4-byte aligned. A struct that "looks like" a direct translation can silently
//! diverge in per-field offset and total size the instant a smaller scalar sits in front
//! of a vec3/vec4 field.
//!
//! Every struct below is laid out so each field lands on a WGSL-legal offset, either
//! because its Rust-natural offset already happens to be a multiple of its WGSL
//! alignment (documented per-struct), or via explicit `_pad*` fields reproducing WGSL's
//! implicit padding byte-for-byte. Hand-derived offset comments are not trusted alone:
//! `renderer::gpu::layout_check` is this file's actual authority -- it uploads a
//! populated instance, has a compute shader echo every field back, and compares raw
//! bytes. A comment can be wrong; the echo test cannot lie about what the GPU did.
//!
//! # Module layout
//!
//! [`camera`] holds the camera/geometry self-test structs (`CameraUniform`,
//! `GpuCameraParams`, `GpuRay`, `GpuHitRecord`); [`material`] the material encoding
//! (`GpuGemMaterial`/`DispersionParams`/`GpuAbsorptionBand` plus their `encode`
//! constructors); [`transport`] the megakernel's per-dispatch uniform
//! (`GpuTransportParams`); [`girdle_finish`] the parallel per-facet finish buffer; and
//! [`wavefront`] the wavefront pipeline's per-round uniform. Every type below was
//! previously declared directly in this file and is re-exported unchanged so
//! `renderer::buffers::X` paths elsewhere in the workspace keep compiling.

mod camera;
mod girdle_finish;
mod material;
#[cfg(test)]
mod tests;
mod transport;
mod wavefront;

pub use camera::{CameraUniform, GpuCameraParams, GpuHitRecord, GpuRay};
pub use girdle_finish::{encode_facet_finishes, facet_finish};
pub use material::{
    DispersionParams, GpuAbsorptionBand, GpuGemMaterial, MAX_ABSORPTION_BANDS, band_shape,
    crystal_system, dispersion_model_type, optical_character,
};
pub use transport::{GpuTransportParams, studio_model, transport_env_mode};
pub use wavefront::GpuWavefrontParams;

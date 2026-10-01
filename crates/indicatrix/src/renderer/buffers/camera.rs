//! Camera and geometry self-test GPU structs: [`CameraUniform`] (the per-frame
//! camera/render-target uniform), [`GpuCameraParams`] (`optics::raytracer::Camera`'s
//! ray-generation basis), [`GpuRay`] (a traced ray), and [`GpuHitRecord`]
//! (`intersect_polyhedron`'s `Option<HitRecord>` result). Each carries state across the
//! CPU/GPU boundary for a self-test (`renderer::gpu::{camera_check, polyhedron_check}`)
//! and gets its own `layout_check` echo test. Every vec3 field is deliberately followed by
//! a plain f32 scalar so the next vec3's 16-byte alignment is met with no separate
//! `_pad*` field.

use core::mem::offset_of;

/// Per-frame camera/render-target uniform.
///
/// # Layout
///
/// Every field's Rust-natural offset already lands on a WGSL-legal boundary for this
/// exact field order and type set (`mat4x4<f32>`/`vec3<f32>` need 16-byte alignment;
/// every other field is a plain 4-byte scalar). `camera_pos`/`c_axis` both happen to
/// start at offsets (64, 96) already multiples of 16, so WGSL inserts no extra padding --
/// a property of this field order, not a general guarantee, so reordering could silently
/// break it; the `offset_of!` assertions below pin it down explicitly.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraUniform {
    /// View proj inv.
    pub view_proj_inv: [f32; 16],
    /// Camera pos.
    pub camera_pos: [f32; 3],
    /// Index of the progressive frame.
    pub frame_index: u32,
    /// Screen width.
    pub screen_width: u32,
    /// Screen height.
    pub screen_height: u32,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Gem material id.
    pub gem_material_id: u32,
    /// C axis.
    pub c_axis: [f32; 3],
    /// Env intensity.
    pub env_intensity: f32,
}

const _: () = {
    assert!(offset_of!(CameraUniform, view_proj_inv) == 0);
    assert!(offset_of!(CameraUniform, camera_pos) == 64);
    assert!(offset_of!(CameraUniform, frame_index) == 76);
    assert!(offset_of!(CameraUniform, screen_width) == 80);
    assert!(offset_of!(CameraUniform, screen_height) == 84);
    assert!(offset_of!(CameraUniform, max_bounces) == 88);
    assert!(offset_of!(CameraUniform, gem_material_id) == 92);
    assert!(offset_of!(CameraUniform, c_axis) == 96);
    assert!(offset_of!(CameraUniform, env_intensity) == 108);
    assert!(size_of::<CameraUniform>() == 112);
};

/// A camera pose's screen-space ray-generation basis (`optics::raytracer::Camera`),
/// GPU-encoded.
///
/// Produced by porting `Camera::new` (from `(yaw, pitch, distance, fov_deg)`) and
/// consumed by porting `Camera::generate_ray`.
///
/// # Layout
///
/// `origin`/`forward`/`right` each pack with the scalar immediately following into a
/// 16-byte block (see the module doc comment); `up` is the last vec3 and needs no
/// trailing pad since `num_samples` already lands on a legal offset (60) right after it,
/// with the whole 64-byte struct already a multiple of its own alignment.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuCameraParams {
    /// Ray origin.
    pub origin: [f32; 3],
    /// Fov tan.
    pub fov_tan: f32,
    /// Forward.
    pub forward: [f32; 3],
    /// Width.
    pub width: f32,
    /// Right.
    pub right: [f32; 3],
    /// Height.
    pub height: f32,
    /// Up.
    pub up: [f32; 3],
    /// Number of samples per pixel.
    pub num_samples: u32,
}

const _: () = {
    assert!(offset_of!(GpuCameraParams, origin) == 0);
    assert!(offset_of!(GpuCameraParams, fov_tan) == 12);
    assert!(offset_of!(GpuCameraParams, forward) == 16);
    assert!(offset_of!(GpuCameraParams, width) == 28);
    assert!(offset_of!(GpuCameraParams, right) == 32);
    assert!(offset_of!(GpuCameraParams, height) == 44);
    assert!(offset_of!(GpuCameraParams, up) == 48);
    assert!(offset_of!(GpuCameraParams, num_samples) == 60);
    assert!(size_of::<GpuCameraParams>() == 64);
};

/// A traced ray (`optics::raytracer::Ray`), GPU-encoded. Both an intersection kernel's
/// input and a camera-ray-generation kernel's output.
///
/// # Layout
///
/// Neither vec3 here has a natural scalar to pack with (a `Ray` is just two vec3s), so
/// each needs an explicit `_pad*` field reproducing WGSL's implicit trailing padding.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuRay {
    /// Ray origin.
    pub origin: [f32; 3],
    _pad0: f32,
    /// Ray direction.
    pub dir: [f32; 3],
    _pad1: f32,
}

impl GpuRay {
    /// Creates a new value from its components.
    #[must_use]
    pub const fn new(origin: [f32; 3], dir: [f32; 3]) -> Self {
        Self {
            origin,
            _pad0: 0.0,
            dir,
            _pad1: 0.0,
        }
    }
}

const _: () = {
    assert!(offset_of!(GpuRay, origin) == 0);
    assert!(offset_of!(GpuRay, dir) == 16);
    assert!(size_of::<GpuRay>() == 32);
};

/// `intersect_polyhedron`'s `Option<HitRecord>` result, GPU-encoded.
///
/// `hit == 0` encodes `None`; `hit != 0` encodes `Some(HitRecord { t, normal,
/// facet_idx })` (`facet_idx` stored as `i32` with `-1` reserved as an additional "no
/// hit" sentinel for diagnostics).
///
/// # Layout
///
/// `t`/`facet_idx`/`hit`/`_pad0` are four 4-byte-aligned scalars packing tightly into a
/// 16-byte block, so `normal` (a vec3, needing 16-byte alignment) lands at offset 16
/// with no additional padding; its own trailing 4 bytes are reproduced by `_pad1`.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuHitRecord {
    /// Ray parameter of the hit.
    pub t: f32,
    /// Index of the facet that was hit.
    pub facet_idx: i32,
    /// Non-zero when the ray hit the stone.
    pub hit: u32,
    _pad0: u32,
    /// Surface normal at the hit.
    pub normal: [f32; 3],
    _pad1: f32,
}

impl GpuHitRecord {
    /// The record for a ray that hit nothing.
    #[must_use]
    pub const fn miss() -> Self {
        Self {
            t: 0.0,
            facet_idx: -1,
            hit: 0,
            _pad0: 0,
            normal: [0.0, 0.0, 0.0],
            _pad1: 0.0,
        }
    }

    /// The record for a ray that hit a facet.
    #[must_use]
    pub const fn hit(t: f32, facet_idx: i32, normal: [f32; 3]) -> Self {
        Self {
            t,
            facet_idx,
            hit: 1,
            _pad0: 0,
            normal,
            _pad1: 0.0,
        }
    }
}

const _: () = {
    assert!(offset_of!(GpuHitRecord, t) == 0);
    assert!(offset_of!(GpuHitRecord, facet_idx) == 4);
    assert!(offset_of!(GpuHitRecord, hit) == 8);
    assert!(offset_of!(GpuHitRecord, normal) == 16);
    assert!(size_of::<GpuHitRecord>() == 32);
};

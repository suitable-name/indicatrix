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

use crate::optics::{
    absorption::{AbsorptionBand, BandShape},
    raytracer::FacetFinish,
};
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
    pub view_proj_inv: [f32; 16],
    pub camera_pos: [f32; 3],
    pub frame_index: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub max_bounces: u32,
    pub gem_material_id: u32,
    pub c_axis: [f32; 3],
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

/// One dispersion curve (`optics::dispersion::DispersionModel`), GPU-encoded.
///
/// `model_type` selects the interpretation of `param_a`/`param_b`:
/// - `0` (Sellmeier1 `{b1, c1}`): `param_a[0] = b1`, `param_b[0] = c1`.
/// - `1` (Sellmeier3 `{b: [f32;3], c: [f32;3]}`): `param_a[0..3] = b`, `param_b[0..3] = c`.
/// - `2` (Cauchy `{a, b, c}`): `param_a[0..3] = [a, b, c]`.
///
/// `c_axis_and_birefringence.xyz` is `GemMaterial::c_axis`; `.w` is
/// `GemMaterial::birefringence_delta`. `biaxial_delta_beta_alpha` /
/// `has_biaxial_delta` mirror `GemMaterial::biaxial_delta_beta_alpha: Option<f32>`
/// (`has_biaxial_delta != 0` <=> `Some`).
///
/// # Layout
///
/// The field after `model_type` needs 16-byte alignment (every `param_*`/
/// `c_axis_and_birefringence` field is a `vec4<f32>`), so `_pad_after_model_type`
/// reproduces WGSL's implicit 12-byte padding. `param_a` through
/// `c_axis_and_birefringence` pack back-to-back at 16 bytes each; the trailing three
/// scalars pack tightly, and `_pad_tail` reproduces WGSL's final 4-byte struct-size
/// rounding.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DispersionParams {
    pub model_type: u32,
    _pad_after_model_type: [u32; 3],
    pub param_a: [f32; 4],
    pub param_b: [f32; 4],
    pub param_c: [f32; 4],
    pub c_axis_and_birefringence: [f32; 4],
    pub is_anisotropic: u32,
    pub biaxial_delta_beta_alpha: f32,
    pub has_biaxial_delta: u32,
    _pad_tail: f32,
}

const _: () = {
    assert!(offset_of!(DispersionParams, model_type) == 0);
    assert!(offset_of!(DispersionParams, param_a) == 16);
    assert!(offset_of!(DispersionParams, param_b) == 32);
    assert!(offset_of!(DispersionParams, param_c) == 48);
    assert!(offset_of!(DispersionParams, c_axis_and_birefringence) == 64);
    assert!(offset_of!(DispersionParams, is_anisotropic) == 80);
    assert!(offset_of!(DispersionParams, biaxial_delta_beta_alpha) == 84);
    assert!(offset_of!(DispersionParams, has_biaxial_delta) == 88);
    assert!(offset_of!(DispersionParams, _pad_tail) == 92);
    assert!(size_of::<DispersionParams>() == 96);
};

/// `model_type` discriminants for [`DispersionParams`] -- must match
/// `renderer/shaders/layout_echo.wgsl` and (eventually) any real dispersion-evaluating
/// kernel.
pub mod dispersion_model_type {
    pub const SELLMEIER1: u32 = 0;
    pub const SELLMEIER3: u32 = 1;
    pub const CAUCHY: u32 = 2;
}

/// Hard cap on how many [`GpuAbsorptionBand`]s either eigenmode of a [`GpuGemMaterial`]
/// can carry.
///
/// `GemMaterial::absorption`'s `Vec<AbsorptionBand>` is unbounded on the CPU side, but a
/// GPU encoding needs a fixed-capacity array. Enforced on scene ingest by
/// `apps/indicatrix-worker/src/validate.rs`'s `validate_scene`, so a scene that would
/// silently truncate on the GPU is rejected before it ever gets there.
///
/// 8 is comfortably above every built-in material's real band count (the widest is 3,
/// `legacy_rgb_bands`), while staying small enough that the fixed array costs nothing
/// worth measuring for materials that use far fewer.
pub const MAX_ABSORPTION_BANDS: usize = 8;

/// One Gaussian absorption band (`optics::absorption::AbsorptionBand`), GPU-encoded.
///
/// # Layout
///
/// All four fields are 4-byte-aligned scalars (`shape` a `u32`), so this struct's WGSL
/// alignment is 4 (no vec3/vec4 field to trigger the usual pitfall) and its size is
/// exactly 16 bytes with no padding, matching Rust's natural layout. This is why
/// [`GpuGemMaterial`]'s band arrays are safe as plain `[GpuAbsorptionBand;
/// MAX_ABSORPTION_BANDS]` with no per-element padding: a storage buffer's array-stride
/// rule only requires a multiple of the element's own alignment (4), unlike a *uniform*
/// buffer's array, which would require a multiple of 16 -- [`GpuGemMaterial`] is bound
/// as storage specifically because of this.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuAbsorptionBand {
    pub center_nm: f32,
    pub width_nm: f32,
    pub peak: f32,
    /// Which domain this band is Gaussian in -- see [`band_shape`] for the
    /// discriminants, mirroring `optics::absorption::BandShape`.
    pub shape: u32,
}

const _: () = {
    assert!(offset_of!(GpuAbsorptionBand, center_nm) == 0);
    assert!(offset_of!(GpuAbsorptionBand, width_nm) == 4);
    assert!(offset_of!(GpuAbsorptionBand, peak) == 8);
    assert!(offset_of!(GpuAbsorptionBand, shape) == 12);
    assert!(size_of::<GpuAbsorptionBand>() == 16);
};

/// `optics::absorption::BandShape` discriminants for [`GpuAbsorptionBand::shape`]. Must
/// match `renderer/shaders/transport_physics.wgsl`'s `spectral_absorption`'s own
/// `band.shape == 1u` branch.
pub mod band_shape {
    pub const GAUSSIAN_WAVELENGTH: u32 = 0;
    pub const GAUSSIAN_ENERGY: u32 = 1;
}

/// `crystal_system` discriminants for [`GpuGemMaterial`].
///
/// Must match `renderer/shaders/layout_echo.wgsl`'s and (eventually) any real
/// material-evaluating kernel's own numbering. Order matches
/// `optics::materials::CrystalSystem`'s own declaration order.
pub mod crystal_system {
    pub const CUBIC: u32 = 0;
    pub const TETRAGONAL: u32 = 1;
    pub const HEXAGONAL: u32 = 2;
    pub const TRIGONAL: u32 = 3;
    pub const ORTHORHOMBIC: u32 = 4;
    pub const MONOCLINIC: u32 = 5;
    pub const TRICLINIC: u32 = 6;
}
/// `optical_character` discriminants for [`GpuGemMaterial`].
///
/// Must match `renderer/shaders/layout_echo.wgsl`'s and (eventually) any real
/// material-evaluating kernel's own numbering. Order matches
/// `optics::materials::OpticalCharacter`'s own declaration order.
pub mod optical_character {
    pub const ISOTROPIC: u32 = 0;
    pub const UNIAXIAL_POSITIVE: u32 = 1;
    pub const UNIAXIAL_NEGATIVE: u32 = 2;
    pub const BIAXIAL_POSITIVE: u32 = 3;
    pub const BIAXIAL_NEGATIVE: u32 = 4;
}

/// A full `optics::materials::GemMaterial`, GPU-encoded.
///
/// [`DispersionParams`] plus crystal/optical-character discriminants and both
/// eigenmodes' absorption band sets (flattened to [`MAX_ABSORPTION_BANDS`]-capacity
/// arrays with an explicit count, per this crate's Phase-0 plan).
///
/// # Layout
///
/// `dispersion` is 96 bytes (a multiple of its own 16-byte WGSL alignment), occupying
/// 0..96 with no leading padding. `crystal_system` through `e_ray_band_count` are five
/// 4-byte-aligned `u32`s packing at 96..116. `o_ray_bands`/`e_ray_bands` (alignment 4)
/// need only a 4-byte offset, so 116 already qualifies; each is now
/// `MAX_ABSORPTION_BANDS` (8) * 16 bytes = 128 bytes (see [`GpuAbsorptionBand`]'s own
/// Layout doc comment for why its size grew from 12 to 16), so `o_ray_bands` occupies
/// 116..244 and `e_ray_bands` 244..372. `scattering_sigma_s` through
/// `edge_rounding_radius` pack at 372..384.
///
/// Every field from `has_beta_ray` onward is APPENDED at the end rather than inserted
/// alongside its logical sibling, so every earlier field keeps its offset:
/// `has_beta_ray`/`beta_ray_band_count` (384..392), `beta_ray_bands` (392..520, the same
/// 128-byte band-array size as `o_ray_bands`/`e_ray_bands`), `absorption_path_scale`
/// (520..524), `has_extraordinary_dispersion`/`extraordinary_model_type` (524..532,
/// mirroring `GemMaterial::uniaxial_extraordinary_dispersion: Option<DispersionModel>`).
/// `extraordinary_param_a`/`extraordinary_param_b` encode that model like
/// [`DispersionParams::encode`]'s own `param_a`/`param_b`, and being `vec4<f32>` need
/// 16-byte alignment: `_pad_before_extraordinary_params` (532..544) reproduces WGSL's
/// implicit padding to get there; they then occupy 544..576, already 16-byte aligned,
/// with no trailing pad.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuGemMaterial {
    pub dispersion: DispersionParams,
    pub crystal_system: u32,
    pub optical_character: u32,
    pub is_pleochroic: u32,
    pub o_ray_band_count: u32,
    pub e_ray_band_count: u32,
    pub o_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    pub e_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// Inclusion/subsurface scattering: mirrors
    /// `optics::materials::GemMaterial::scattering_sigma_s`/`scattering_g` exactly.
    pub scattering_sigma_s: f32,
    pub scattering_g: f32,
    /// Facet edge rounding: mirrors
    /// `optics::materials::GemMaterial::edge_rounding_radius` exactly.
    pub edge_rounding_radius: f32,
    /// Mirrors `AbsorptionTensor::beta_ray`'s presence (`beta_ray.is_some()`).
    /// `beta_ray_band_count`/`beta_ray_bands` are meaningless when this is 0;
    /// distinguishes "no third band set" from "a third band set with zero bands", same
    /// as `Option<Vec<AbsorptionBand>>` on the CPU side.
    pub has_beta_ray: u32,
    /// The third, `n_beta`, principal direction's absorption bands' count -- see
    /// `has_beta_ray`.
    pub beta_ray_band_count: u32,
    /// The third, `n_beta`, principal direction's absorption bands -- see
    /// `has_beta_ray`. Mirrors `o_ray_bands`/`e_ray_bands`'s fixed-capacity encoding.
    pub beta_ray_bands: [GpuAbsorptionBand; MAX_ABSORPTION_BANDS],
    /// Mirrors `GemMaterial::absorption_path_scale` -- every model-unit length entering
    /// Beer-Lambert absorption or the scattering estimator is multiplied by this before
    /// use, both in the CPU tracer and in `spectral_transport.wgsl`'s mirrored blocks.
    pub absorption_path_scale: f32,
    /// Whether `extraordinary_param_a`/`extraordinary_param_b` carry a genuine
    /// independent extraordinary-ray dispersion curve -- mirrors
    /// `uniaxial_extraordinary_dispersion.is_some()`. Zero means the shader falls back to
    /// the constant-offset approximation `n_o + birefringence_delta` -- see
    /// `spectral_transport.wgsl`'s `extraordinary_index_at`.
    pub has_extraordinary_dispersion: u32,
    /// The extraordinary-ray curve's [`DispersionModel`](crate::optics::dispersion::DispersionModel)
    /// variant, using the SAME [`dispersion_model_type`] discriminants as
    /// `dispersion.model_type`. Meaningless when `has_extraordinary_dispersion == 0`.
    pub extraordinary_model_type: u32,
    /// Explicit padding reproducing the implicit bytes WGSL inserts before
    /// `extraordinary_param_a` (a `vec4<f32>`) -- see this struct's Layout doc comment.
    _pad_before_extraordinary_params: [u32; 3],
    /// The extraordinary-ray curve's `param_a`, encoded like [`DispersionParams::param_a`].
    /// Meaningless when `has_extraordinary_dispersion == 0`.
    pub extraordinary_param_a: [f32; 4],
    /// The extraordinary-ray curve's `param_b`, encoded like [`DispersionParams::param_b`].
    /// Meaningless when `has_extraordinary_dispersion == 0`.
    pub extraordinary_param_b: [f32; 4],
}

const _: () = {
    assert!(offset_of!(GpuGemMaterial, dispersion) == 0);
    assert!(offset_of!(GpuGemMaterial, crystal_system) == 96);
    assert!(offset_of!(GpuGemMaterial, optical_character) == 100);
    assert!(offset_of!(GpuGemMaterial, is_pleochroic) == 104);
    assert!(offset_of!(GpuGemMaterial, o_ray_band_count) == 108);
    assert!(offset_of!(GpuGemMaterial, e_ray_band_count) == 112);
    assert!(offset_of!(GpuGemMaterial, o_ray_bands) == 116);
    assert!(offset_of!(GpuGemMaterial, e_ray_bands) == 244);
    assert!(offset_of!(GpuGemMaterial, scattering_sigma_s) == 372);
    assert!(offset_of!(GpuGemMaterial, scattering_g) == 376);
    assert!(offset_of!(GpuGemMaterial, edge_rounding_radius) == 380);
    assert!(offset_of!(GpuGemMaterial, has_beta_ray) == 384);
    assert!(offset_of!(GpuGemMaterial, beta_ray_band_count) == 388);
    assert!(offset_of!(GpuGemMaterial, beta_ray_bands) == 392);
    assert!(offset_of!(GpuGemMaterial, absorption_path_scale) == 520);
    assert!(offset_of!(GpuGemMaterial, has_extraordinary_dispersion) == 524);
    assert!(offset_of!(GpuGemMaterial, extraordinary_model_type) == 528);
    assert!(offset_of!(GpuGemMaterial, extraordinary_param_a) == 544);
    assert!(offset_of!(GpuGemMaterial, extraordinary_param_b) == 560);
    assert!(size_of::<GpuGemMaterial>() == 576);
};

// Geometry/environment GPU-check structs: each carries state across the CPU/GPU
// boundary for a self-test (renderer::gpu::{camera_check, polyhedron_check,
// environment_check, furnace_check}) and gets its own layout_check echo test. Every
// vec3 field is deliberately followed by a plain f32 scalar so the next vec3's 16-byte
// alignment is met with no separate _pad* field, as in CameraUniform above.

/// A camera pose's screen-space ray-generation basis (`optics::raytracer::Camera`),
/// GPU-encoded.
///
/// Produced by porting `Camera::new` (from `(yaw, pitch, distance, fov_deg)`) and
/// consumed by porting `Camera::generate_ray`.
///
/// # Layout
///
/// `origin`/`forward`/`right` each pack with the scalar immediately following into a
/// 16-byte block (see the section doc comment); `up` is the last vec3 and needs no
/// trailing pad since `num_samples` already lands on a legal offset (60) right after it,
/// with the whole 64-byte struct already a multiple of its own alignment.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuCameraParams {
    pub origin: [f32; 3],
    pub fov_tan: f32,
    pub forward: [f32; 3],
    pub width: f32,
    pub right: [f32; 3],
    pub height: f32,
    pub up: [f32; 3],
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
    pub origin: [f32; 3],
    _pad0: f32,
    pub dir: [f32; 3],
    _pad1: f32,
}

impl GpuRay {
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
    pub t: f32,
    pub facet_idx: i32,
    pub hit: u32,
    _pad0: u32,
    pub normal: [f32; 3],
    _pad1: f32,
}

impl GpuHitRecord {
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

// The isotropic spectral estimator (`shaders/spectral_transport.wgsl`, driven by
// `renderer::gpu::{transport_check, estimator_check}`).

/// Per-dispatch kernel parameters for `shaders/spectral_transport.wgsl`'s
/// `transport_main` entry point.
///
/// `env_mode` selects which environment model to sample (`0` = the direction-independent
/// "uniform furnace" grey environment at `l0`, `1` = the analytic studio rig at the
/// given colour temperature/exposure/rig-pose). `sample_offset` + `camera.num_samples`
/// select the sample range this dispatch traces, mirroring
/// `apps/indicatrix-worker/src/render_core.rs`'s `first_sample`/`samples` convention --
/// this lets `estimator_check`'s statistical comparison give the CPU and GPU DISJOINT
/// sample ranges, as production would. `white_balance` is the precomputed von-Kries
/// white balance (`compute_illuminant_white_balance`, already ULP-verified by
/// `environment_check`). This is a **Bradford LMS-space** diagonal scale, not an
/// XYZ-space one -- the megakernel's `apply_von_kries_white_balance` transforms to
/// Bradford LMS, applies this scale, and transforms back, mirroring the CPU side
/// exactly.
///
/// # Layout
///
/// The ten leading scalars pack into 40 bytes; `pixel_offset`/`write_debug_buffers` bring
/// that to 48 (a multiple of 16), so `white_balance` (`vec3<f32>`) needs no padding
/// before it. The struct's own alignment is 16, and 60 is not a multiple of it, so WGSL
/// rounds the size up to 64; `studio_use_d65` fills that trailing slot as a real field
/// rather than padding.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTransportParams {
    pub num_pixels: u32,
    pub max_bounces: u32,
    pub sample_offset: u32,
    pub env_mode: u32,
    pub l0: f32,
    pub studio_temp_k: f32,
    pub studio_spot_mult: f32,
    pub studio_exposure: f32,
    pub studio_light_yaw: f32,
    pub studio_light_pitch: f32,
    /// Index of the first pixel this dispatch covers, added to the shader's own
    /// `idx / num_samples` to recover a GLOBAL pixel index for camera-ray generation
    /// while output slots stay dispatch-local. Zero for a dispatch covering a whole
    /// frame (every self-test); exists for `renderer::gpu::frame`, which splits a frame
    /// too large for its memory budget into chunks.
    pub pixel_offset: u32,
    /// Whether `transport_main` should write its three per-channel debug output buffers
    /// (`out_radiance`/`out_lambdas`/`out_path_pdf`) this dispatch. Nonzero (the default)
    /// reproduces every existing dispatch's behaviour. Zero (see
    /// [`Self::with_debug_buffers_disabled`]) skips those writes -- only `out_xyz` is
    /// written -- so a production dispatch does 9x less write traffic and its chunk
    /// budget holds 9x more samples per dispatch.
    pub write_debug_buffers: u32,
    pub white_balance: [f32; 3],
    /// Whether `sample_studio_environment_with_rig` should sample the tabulated CIE D65
    /// measured spectrum (nonzero) instead of `blackbody_spectrum` at `studio_temp_k`
    /// (zero, default) -- mirrors that function's own
    /// `matches!(lighting_preset, LightingPreset::Daylight)` branch. Meaningless when
    /// `env_mode == transport_env_mode::UNIFORM_FURNACE`.
    pub studio_use_d65: u32,
}

/// `env_mode` discriminants for [`GpuTransportParams`]. Must match
/// `shaders/spectral_transport.wgsl`'s own `params.env_mode` branch.
pub mod transport_env_mode {
    pub const UNIFORM_FURNACE: u32 = 0;
    pub const STUDIO_RIG: u32 = 1;
    /// Finding G6: `EnvironmentSource::HdrMap`.
    ///
    /// Sampled via `hdr_env_radiance_at` against the `hdr_texels`/`hdr_env_dims`
    /// storage/uniform buffers (bindings 10/11) instead of the analytic studio rig. See
    /// `renderer::env_map_gpu`'s module doc comment.
    pub const HDR_MAP: u32 = 2;
}

impl GpuTransportParams {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per GpuTransportParams field, in field order, because \
                  that field order is itself the #[repr(C)] layout spectral_transport.wgsl's \
                  uniform buffer binds against; bundling them into a context struct here \
                  would just re-introduce this exact struct one level removed"
    )]
    pub const fn new(
        num_pixels: u32,
        max_bounces: u32,
        sample_offset: u32,
        env_mode: u32,
        l0: f32,
        studio_temp_k: f32,
        studio_spot_mult: f32,
        studio_exposure: f32,
        studio_light_yaw: f32,
        studio_light_pitch: f32,
        white_balance: [f32; 3],
    ) -> Self {
        Self {
            num_pixels,
            max_bounces,
            sample_offset,
            env_mode,
            l0,
            studio_temp_k,
            studio_spot_mult,
            studio_exposure,
            studio_light_yaw,
            studio_light_pitch,
            pixel_offset: 0,
            write_debug_buffers: 1,
            white_balance,
            studio_use_d65: 0,
        }
    }

    /// Returns a copy covering pixels `[pixel_offset, pixel_offset + num_pixels)` of a
    /// larger frame -- see [`Self::pixel_offset`]. A separate builder rather than an
    /// eleventh `new` parameter, so every existing caller keeps its exact constructor
    /// call.
    #[must_use]
    pub const fn with_pixel_offset(mut self, pixel_offset: u32) -> Self {
        self.pixel_offset = pixel_offset;
        self
    }

    /// Returns a copy that skips `transport_main`'s three per-channel debug output
    /// writes -- see [`Self::write_debug_buffers`]'s doc comment. Only
    /// `GpuFrameRenderer::accumulate`'s production dispatch calls this.
    #[must_use]
    pub const fn with_debug_buffers_disabled(mut self) -> Self {
        self.write_debug_buffers = 0;
        self
    }

    /// Returns a copy that samples the tabulated CIE D65 measured spectrum instead of
    /// `blackbody_spectrum` at `studio_temp_k` -- see [`Self::studio_use_d65`]'s doc
    /// comment. Only callers whose `EnvironmentSource::Studio` preset is
    /// `LightingPreset::Daylight` should pass `true` here.
    #[must_use]
    pub const fn with_studio_use_d65(mut self, use_d65: bool) -> Self {
        self.studio_use_d65 = if use_d65 { 1 } else { 0 };
        self
    }
}

const _: () = {
    assert!(offset_of!(GpuTransportParams, num_pixels) == 0);
    assert!(offset_of!(GpuTransportParams, max_bounces) == 4);
    assert!(offset_of!(GpuTransportParams, sample_offset) == 8);
    assert!(offset_of!(GpuTransportParams, env_mode) == 12);
    assert!(offset_of!(GpuTransportParams, l0) == 16);
    assert!(offset_of!(GpuTransportParams, studio_temp_k) == 20);
    assert!(offset_of!(GpuTransportParams, studio_spot_mult) == 24);
    assert!(offset_of!(GpuTransportParams, studio_exposure) == 28);
    assert!(offset_of!(GpuTransportParams, studio_light_yaw) == 32);
    assert!(offset_of!(GpuTransportParams, studio_light_pitch) == 36);
    assert!(offset_of!(GpuTransportParams, white_balance) == 48);
    assert!(offset_of!(GpuTransportParams, studio_use_d65) == 60);
    assert!(size_of::<GpuTransportParams>() == 64);
};

/// Encodes a CPU `optics::materials::GemMaterial` into a [`GpuGemMaterial`] for upload.
///
/// Never a hand-copied duplicate of the material data: every field read here is the SAME
/// field `optics::raytracer::trace_spectral_ray` itself reads. `material.absorption.beta_ray`
/// (the optional third, trichroic band set) is encoded into
/// `has_beta_ray`/`beta_ray_band_count`/`beta_ray_bands`, consumed by the shader's
/// genuinely biaxial absorption path whenever `dispersion.has_biaxial_delta != 0`.
impl GpuGemMaterial {
    /// # Panics
    ///
    /// Panics if `material` has more than [`MAX_ABSORPTION_BANDS`] bands in either
    /// eigenmode's `Vec<AbsorptionBand>` -- every built-in material has at most 3, so
    /// this is only reachable for a hand-constructed test material; acceptable to
    /// panic in this self-test-only encoder rather than silently truncate a band set.
    #[must_use]
    pub fn encode(material: &crate::optics::materials::GemMaterial) -> Self {
        use crate::optics::materials::{CrystalSystem, OpticalCharacter};

        let crystal_system_val = match material.crystal_system {
            CrystalSystem::Cubic => crystal_system::CUBIC,
            CrystalSystem::Tetragonal => crystal_system::TETRAGONAL,
            CrystalSystem::Hexagonal => crystal_system::HEXAGONAL,
            CrystalSystem::Trigonal => crystal_system::TRIGONAL,
            CrystalSystem::Orthorhombic => crystal_system::ORTHORHOMBIC,
            CrystalSystem::Monoclinic => crystal_system::MONOCLINIC,
            CrystalSystem::Triclinic => crystal_system::TRICLINIC,
        };
        let optical_character_val = match material.optical_character {
            OpticalCharacter::Isotropic => optical_character::ISOTROPIC,
            OpticalCharacter::UniaxialPositive => optical_character::UNIAXIAL_POSITIVE,
            OpticalCharacter::UniaxialNegative => optical_character::UNIAXIAL_NEGATIVE,
            OpticalCharacter::BiaxialPositive => optical_character::BIAXIAL_POSITIVE,
            OpticalCharacter::BiaxialNegative => optical_character::BIAXIAL_NEGATIVE,
        };

        let (o_ray_bands, o_ray_band_count) = encode_bands(&material.absorption.o_ray);
        let (e_ray_bands, e_ray_band_count) = encode_bands(&material.absorption.e_ray);
        let (beta_ray_bands, beta_ray_band_count) = material
            .absorption
            .beta_ray
            .as_deref()
            .map_or_else(empty_bands, encode_bands);

        // None (every built-in except Quartz/Amethyst/Citrine) encodes to
        // has_extraordinary_dispersion == 0 and an all-zero curve the shader never reads.
        let (
            has_extraordinary_dispersion,
            extraordinary_model_type,
            extraordinary_param_a,
            extraordinary_param_b,
        ) = material.uniaxial_extraordinary_dispersion.map_or(
            (0u32, 0u32, [0.0f32; 4], [0.0f32; 4]),
            |e_dispersion| {
                let (model_type, param_a, param_b) = encode_dispersion_model(&e_dispersion);
                (1u32, model_type, param_a, param_b)
            },
        );

        Self {
            dispersion: DispersionParams::encode(material),
            crystal_system: crystal_system_val,
            optical_character: optical_character_val,
            is_pleochroic: u32::from(material.absorption.is_pleochroic),
            o_ray_band_count,
            e_ray_band_count,
            o_ray_bands,
            e_ray_bands,
            scattering_sigma_s: material.scattering_sigma_s,
            scattering_g: material.scattering_g,
            edge_rounding_radius: material.edge_rounding_radius,
            has_beta_ray: u32::from(material.absorption.beta_ray.is_some()),
            beta_ray_band_count,
            beta_ray_bands,
            absorption_path_scale: material.absorption_path_scale,
            has_extraordinary_dispersion,
            extraordinary_model_type,
            _pad_before_extraordinary_params: [0; 3],
            extraordinary_param_a,
            extraordinary_param_b,
        }
    }
}

/// Encodes one `optics::dispersion::DispersionModel` into its `(model_type, param_a,
/// param_b)` GPU representation -- shared by [`DispersionParams::encode`] (the
/// material's primary curve) and [`GpuGemMaterial::encode`] (the optional extraordinary
/// curve), so the two never independently drift on the mapping.
#[must_use]
const fn encode_dispersion_model(
    model: &crate::optics::dispersion::DispersionModel,
) -> (u32, [f32; 4], [f32; 4]) {
    use crate::optics::dispersion::DispersionModel;

    match *model {
        DispersionModel::Sellmeier1 { b1, c1 } => (
            dispersion_model_type::SELLMEIER1,
            [b1, 0.0, 0.0, 0.0],
            [c1, 0.0, 0.0, 0.0],
        ),
        DispersionModel::Sellmeier3 { b, c } => (
            dispersion_model_type::SELLMEIER3,
            [b[0], b[1], b[2], 0.0],
            [c[0], c[1], c[2], 0.0],
        ),
        DispersionModel::Cauchy { a, b, c } => {
            (dispersion_model_type::CAUCHY, [a, b, c, 0.0], [0.0; 4])
        }
    }
}

impl DispersionParams {
    #[must_use]
    fn encode(material: &crate::optics::materials::GemMaterial) -> Self {
        let (model_type, param_a, param_b) = encode_dispersion_model(&material.dispersion);

        let c_axis = material.c_axis;
        Self {
            model_type,
            _pad_after_model_type: [0; 3],
            param_a,
            param_b,
            param_c: [0.0; 4],
            c_axis_and_birefringence: [c_axis.x, c_axis.y, c_axis.z, material.birefringence_delta],
            is_anisotropic: u32::from(
                material.crystal_system != crate::optics::materials::CrystalSystem::Cubic
                    && material.birefringence_delta.abs() > 1e-4,
            ),
            biaxial_delta_beta_alpha: material.biaxial_delta_beta_alpha.unwrap_or(0.0),
            has_biaxial_delta: u32::from(material.biaxial_delta_beta_alpha.is_some()),
            _pad_tail: 0.0,
        }
    }
}

/// The all-zero-bands, zero-count encoding used for `beta_ray_bands` when
/// `material.absorption.beta_ray` is `None` -- mirrors [`encode_bands`]'s default slot
/// values (`width_nm: 1.0`, else `0.0`) so an unused slot is never a degenerate
/// zero-width Gaussian, even though `peak` is always `0.0` regardless.
const fn empty_bands() -> ([GpuAbsorptionBand; MAX_ABSORPTION_BANDS], u32) {
    (
        [GpuAbsorptionBand {
            center_nm: 0.0,
            width_nm: 1.0,
            peak: 0.0,
            shape: band_shape::GAUSSIAN_WAVELENGTH,
        }; MAX_ABSORPTION_BANDS],
        0,
    )
}

/// Encodes a `Vec<AbsorptionBand>` into a fixed [`MAX_ABSORPTION_BANDS`]-capacity array
/// plus its real length, panicking (see [`GpuGemMaterial::encode`]'s doc comment) if the
/// source has more bands than the GPU encoding can hold.
fn encode_bands(bands: &[AbsorptionBand]) -> ([GpuAbsorptionBand; MAX_ABSORPTION_BANDS], u32) {
    assert!(
        bands.len() <= MAX_ABSORPTION_BANDS,
        "material has {} absorption bands, exceeding MAX_ABSORPTION_BANDS ({MAX_ABSORPTION_BANDS})",
        bands.len()
    );
    let mut out = [GpuAbsorptionBand {
        center_nm: 0.0,
        width_nm: 1.0,
        peak: 0.0,
        shape: band_shape::GAUSSIAN_WAVELENGTH,
    }; MAX_ABSORPTION_BANDS];
    for (slot, band) in out.iter_mut().zip(bands.iter()) {
        *slot = GpuAbsorptionBand {
            center_nm: band.center_nm,
            width_nm: band.width_nm,
            peak: band.peak,
            shape: match band.shape {
                BandShape::GaussianWavelength => band_shape::GAUSSIAN_WAVELENGTH,
                BandShape::GaussianEnergy => band_shape::GAUSSIAN_ENERGY,
            },
        };
    }
    (out, bands.len() as u32)
}

// Girdle finish (bruted/frosted facets): FacetFinish is looked up per-hit via
// HitRecord::facet_idx into a &[FacetFinish] slice PARALLEL to &[GpuFacetPlane] on the
// CPU -- extending GpuFacetPlane itself would disturb that struct's layout, echo test,
// and the intersect_polyhedron kernel, none of which need to know about finish. The GPU
// encoding mirrors that with a SEPARATE storage buffer (one u32 per facet). Still gets
// its own struct-echo test (layout_check::run_facet_finish).
pub mod facet_finish {
    pub const POLISHED: u32 = 0;
    pub const FROSTED: u32 = 1;
}

/// Encodes a `&[FacetFinish]` slice into a `facet_finish::{POLISHED,FROSTED}`-valued
/// `Vec<u32>` of exactly `num_planes` entries.
///
/// Mirrors `trace_spectral_ray_with_finish`'s per-facet lookup semantics
/// (`facet_finishes.get(i).copied().unwrap_or_default()`): an index past the end of a
/// shorter slice, or an empty slice, defaults to `FacetFinish::Polished`, matching the
/// CPU's "no explicit finish means polished" default.
#[must_use]
pub fn encode_facet_finishes(finishes: &[FacetFinish], num_planes: usize) -> Vec<u32> {
    (0..num_planes)
        .map(|i| match finishes.get(i).copied().unwrap_or_default() {
            FacetFinish::Polished => facet_finish::POLISHED,
            FacetFinish::Frosted => facet_finish::FROSTED,
        })
        .collect()
}

// Finding G5 Part B: the wavefront transport pipeline (`shaders/wavefront_transport.wgsl`,
// `renderer::gpu::frame`'s `GpuPipelineKind::Wavefront` path) -- an alternative to the
// megakernel above, not a replacement.

/// Mirrors `wavefront_transport.wgsl`'s `WavefrontParams` uniform struct exactly.
///
/// # Layout
///
/// Four 4-byte-aligned `u32`s pack tightly into 16 bytes with no padding -- the same
/// "every field already lands on a WGSL-legal offset" property [`GpuReduceParams`]-style
/// small uniform structs share throughout this module.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuWavefrontParams {
    /// Total rays this chunk dispatched (`pixels_this_chunk * spp`) -- `wavefront_generate`'s
    /// own dispatch bound, and the fixed size every per-ray struct-of-arrays buffer
    /// (`renderer::gpu::frame::WavefrontRayBuffers`) is sized to.
    pub chunk_rays: u32,
    /// How many entries of `active_ray_indices` are live for the CURRENT bounce round --
    /// `wavefront_bounce`/`wavefront_compact_scan`/`wavefront_compact_scatter`/
    /// `wavefront_finalize_survivors`'s own dispatch bound.
    pub active_count: u32,
    /// The current bounce round index -- fed to `transport_bounce_step` exactly as
    /// `transport_main`'s own `for (var bounce...)` loop counter is.
    pub bounce: u32,
    /// `active_count.div_ceil(64)` -- how many `compact_block_alive_count`/
    /// `compact_block_offset` entries this round's compaction actually uses. Not read by
    /// any WGSL kernel today (each compaction kernel derives its own workgroup index from
    /// `@builtin(workgroup_id)` instead); carried here purely so the host-side CPU prefix
    /// sum and the GPU dispatch's own workgroup count agree on ONE source of truth rather
    /// than each recomputing `div_ceil` independently.
    pub workgroup_count: u32,
}

const _: () = {
    assert!(offset_of!(GpuWavefrontParams, chunk_rays) == 0);
    assert!(offset_of!(GpuWavefrontParams, active_count) == 4);
    assert!(offset_of!(GpuWavefrontParams, bounce) == 8);
    assert!(offset_of!(GpuWavefrontParams, workgroup_count) == 12);
    assert!(size_of::<GpuWavefrontParams>() == 16);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytemuck::Pod` guards against uninitialized padding bytes but says nothing about
    /// WGSL offset rules -- that's what the `offset_of!` assertions above are for. This
    /// pins the documented sizes as an ordinary `#[test]` too, so a failure prints a
    /// named result instead of a compile error buried in this module.
    #[test]
    fn struct_sizes_match_documented_wgsl_layout() {
        assert_eq!(size_of::<CameraUniform>(), 112);
        assert_eq!(size_of::<DispersionParams>(), 96);
        assert_eq!(size_of::<GpuAbsorptionBand>(), 16);
        assert_eq!(size_of::<GpuGemMaterial>(), 576);
        assert_eq!(size_of::<GpuTransportParams>(), 64);
        assert_eq!(size_of::<GpuWavefrontParams>(), 16);
    }

    /// [`offset_of!`] pinned as ordinary `#[test]`s too (see the comment above), for the
    /// two structs P6 touched: a wrong offset here is exactly the "looks right, isn't"
    /// bug class `renderer::gpu::layout_check` exists to catch on the GPU side, but a
    /// plain assertion catches the CPU-side half of it for free on every `cargo test`.
    #[test]
    fn gpu_absorption_band_offsets_match_documented_wgsl_layout() {
        assert_eq!(offset_of!(GpuAbsorptionBand, center_nm), 0);
        assert_eq!(offset_of!(GpuAbsorptionBand, width_nm), 4);
        assert_eq!(offset_of!(GpuAbsorptionBand, peak), 8);
        assert_eq!(offset_of!(GpuAbsorptionBand, shape), 12);
    }

    #[test]
    fn gpu_gem_material_band_array_offsets_match_documented_wgsl_layout() {
        assert_eq!(offset_of!(GpuGemMaterial, o_ray_bands), 116);
        assert_eq!(offset_of!(GpuGemMaterial, e_ray_bands), 244);
        assert_eq!(offset_of!(GpuGemMaterial, scattering_sigma_s), 372);
        assert_eq!(offset_of!(GpuGemMaterial, beta_ray_bands), 392);
        assert_eq!(offset_of!(GpuGemMaterial, absorption_path_scale), 520);
    }

    /// [`encode_bands`] must map each [`BandShape`] variant to the matching
    /// [`band_shape`] discriminant, and leave every other field untouched -- the bug
    /// this guards against is the shape ending up defaulted (always
    /// `GAUSSIAN_WAVELENGTH`) regardless of what the CPU-side band actually specified,
    /// which would silently make `GaussianEnergy` materials render correctly on the CPU
    /// but not on the GPU (the exact P6 finding this field exists to fix).
    #[test]
    fn encode_bands_maps_shape_correctly() {
        let bands = vec![
            AbsorptionBand::new(550.0, 20.0, 2.0),
            AbsorptionBand::energy(620.0, 200.0, 1.5),
        ];
        let (encoded, count) = encode_bands(&bands);
        assert_eq!(count, 2);
        assert_eq!(encoded[0].shape, band_shape::GAUSSIAN_WAVELENGTH);
        assert_eq!(encoded[1].shape, band_shape::GAUSSIAN_ENERGY);
        // Untouched slots keep the wavelength-domain default.
        assert_eq!(encoded[2].shape, band_shape::GAUSSIAN_WAVELENGTH);

        let (empty, empty_count) = empty_bands();
        assert_eq!(empty_count, 0);
        assert!(
            empty
                .iter()
                .all(|b| b.shape == band_shape::GAUSSIAN_WAVELENGTH)
        );
    }

    /// [`encode_facet_finishes`]'s default-fallback semantics must match
    /// `trace_spectral_ray_with_finish`'s lookup exactly: an empty slice, or one shorter
    /// than `num_planes`, defaults every uncovered index to `facet_finish::POLISHED`.
    #[test]
    fn encode_facet_finishes_defaults_to_polished() {
        assert_eq!(encode_facet_finishes(&[], 4), vec![0, 0, 0, 0]);

        let finishes = vec![FacetFinish::Frosted, FacetFinish::Polished];
        assert_eq!(
            encode_facet_finishes(&finishes, 4),
            vec![
                facet_finish::FROSTED,
                facet_finish::POLISHED,
                facet_finish::POLISHED,
                facet_finish::POLISHED,
            ]
        );
    }
}

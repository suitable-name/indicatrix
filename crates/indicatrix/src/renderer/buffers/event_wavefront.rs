//! Event wavefront buffer layouts and host-shareable struct twins.
//!
//! Mirrors the event wavefront WGSL structs declared in `00_common.wgsl`:
//! - [`EwParams`] (uniform, binding 40): per-dispatch parameters
//! - [`EwRayCore`] (storage rw, binding 41): ray geometric and state carry
//! - [`EwRayChan`] (storage rw, binding 42): 8-channel spectral data
//! - [`EwHit`] (storage rw, binding 43): per-bounce hit and surface hand-off

use core::mem::offset_of;

/// Event tag: ray has finished all bounces or terminated.
pub const TAG_DONE: u32 = 0;
/// Event tag: ray ready for scene intersection (extend phase).
pub const TAG_EXTEND: u32 = 1;
/// Event tag: ray inside gem ready for interior scatter evaluation.
pub const TAG_SCATTER: u32 = 2;
/// Event tag: ray hit stone surface, ready for surface phase.
pub const TAG_SURFACE: u32 = 3;
/// Base offset for surface interaction arm tags (`TAG_ARM_BASE + ARM_*`).
pub const TAG_ARM_BASE: u32 = 3;
/// Event tag: ray has deferred exit splits ready for evaluation.
pub const TAG_SPLIT: u32 = 9;
/// Event tag: ray finished interaction arm, ready for glare and Russian roulette.
pub const TAG_TAIL: u32 = 10;
/// Event tag: ray path terminated this round or survives final round, ready for final integration.
pub const TAG_FINALIZE: u32 = 11;
/// Event tag: scatter event parked for the HG next-event-estimation kernel (`ew_nee_hg`);
/// only produced under HDR environments.
pub const TAG_NEE_HG: u32 = 12;

/// Surface interaction arm: frosted facet finish.
pub const ARM_FROSTED: u32 = 1;
/// Surface interaction arm: total internal reflection.
pub const ARM_TIR: u32 = 2;
/// Surface interaction arm: extraordinary uniaxial crystal entry.
pub const ARM_UNI_ENTRY: u32 = 3;
/// Surface interaction arm: uniaxial internal reflection or exit.
pub const ARM_UNI_INTERNAL: u32 = 4;
/// Surface interaction arm: generic isotropic/biaxial surface interaction.
pub const ARM_GENERIC: u32 = 5;

/// Phase execution result: proceed to next phase in current bounce.
pub const PHASE_NEXT: u32 = 0;
/// Phase execution result: current bounce step ended, ray continues to next bounce.
pub const PHASE_END_STEP: u32 = 1;
/// Phase execution result: ray path terminated, finalize.
pub const PHASE_TERMINATE: u32 = 2;

/// Ray flag bit: ray is currently inside gem.
pub const FLAG_INSIDE_GEM: u32 = 1;
/// Ray flag bit: ray is extraordinary wave mode.
pub const FLAG_IS_EXTRAORDINARY: u32 = 2;
/// Ray flag bit: previous plane normal is valid.
pub const FLAG_HAVE_PREV_PLANE_NORMAL: u32 = 4;
/// Ray flag bit: path escaped to environment.
pub const FLAG_PATH_ESCAPED: u32 = 8;

/// Per-dispatch uniform parameters for the event wavefront kernels (binding 40).
///
/// Size: 32 bytes (aligned to 16 bytes for WGSL uniform address space).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EwParams {
    /// Total rays in this chunk (`pixels_in_chunk * spp`).
    pub chunk_rays: u32,
    /// Bounce round index (`0..max_bounces`).
    pub bounce: u32,
    /// Set to 1 only on the post-loop finalization of surviving rays.
    pub finalize_all: u32,
    /// 1 if stone material has `scattering_sigma_s > 0.0`, 0 otherwise.
    pub has_scatter: u32,
    /// Padding to 32 bytes.
    pub pad0: u32,
    /// Padding to 32 bytes.
    pub pad1: u32,
    /// Padding to 32 bytes.
    pub pad2: u32,
    /// Padding to 32 bytes.
    pub pad3: u32,
}

const _: () = {
    assert!(size_of::<EwParams>() == 32);
    assert!(offset_of!(EwParams, chunk_rays) == 0);
    assert!(offset_of!(EwParams, bounce) == 4);
    assert!(offset_of!(EwParams, finalize_all) == 8);
    assert!(offset_of!(EwParams, has_scatter) == 12);
    assert!(offset_of!(EwParams, pad0) == 16);
    assert!(offset_of!(EwParams, pad1) == 20);
    assert!(offset_of!(EwParams, pad2) == 24);
    assert!(offset_of!(EwParams, pad3) == 28);
};

/// Core ray geometric state and metadata (binding 41).
///
/// Size: 128 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EwRayCore {
    /// Ray origin (xyz, w unused/0).
    pub origin: [f32; 4],
    /// Ray propagation/Poynting direction (xyz, w unused/0).
    pub dir: [f32; 4],
    /// Ray wave normal direction k (xyz, w unused/0).
    pub k: [f32; 4],
    /// Previous plane normal (xyz, w unused/0).
    pub prev_plane_normal: [f32; 4],
    /// Pending MIS light direction (xyz) and pending MIS weight (w).
    pub pending_mis_dir: [f32; 4],
    /// Running Henyey-Greenstein NEE contribution (xyz, w unused/0).
    pub nee_xyz: [f32; 4],
    /// Observer vector (xyz, w unused/0).
    pub observer: [f32; 4],
    /// Metadata: x = flags, y = seed0, z = event tag, w = tuple index.
    pub ray_meta: [u32; 4],
}

const _: () = {
    assert!(size_of::<EwRayCore>() == 128);
    assert!(offset_of!(EwRayCore, origin) == 0);
    assert!(offset_of!(EwRayCore, dir) == 16);
    assert!(offset_of!(EwRayCore, k) == 32);
    assert!(offset_of!(EwRayCore, prev_plane_normal) == 48);
    assert!(offset_of!(EwRayCore, pending_mis_dir) == 64);
    assert!(offset_of!(EwRayCore, nee_xyz) == 80);
    assert!(offset_of!(EwRayCore, observer) == 96);
    assert!(offset_of!(EwRayCore, ray_meta) == 112);
};

/// 8-channel spectral data carried by one ray across bounces (binding 42).
///
/// Size: 288 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EwRayChan {
    /// 4-component Stokes vector per channel.
    pub stokes: [[f32; 4]; 8],
    /// Accumulated spectral radiance per channel.
    pub radiance: [f32; 8],
    /// Cumulative path PDF per channel.
    pub path_pdf: [f32; 8],
    /// Staged exit split radiance per channel.
    pub split_radiance: [f32; 8],
    /// Channel wavelengths in nanometers.
    pub lambdas: [f32; 8],
    /// Technique compatibility mask per channel.
    pub compat: [u32; 8],
}

const _: () = {
    assert!(size_of::<EwRayChan>() == 288);
    assert!(offset_of!(EwRayChan, stokes) == 0);
    assert!(offset_of!(EwRayChan, radiance) == 128);
    assert!(offset_of!(EwRayChan, path_pdf) == 160);
    assert!(offset_of!(EwRayChan, split_radiance) == 192);
    assert!(offset_of!(EwRayChan, lambdas) == 224);
    assert!(offset_of!(EwRayChan, compat) == 256);
};

/// Inter-phase hit record and surface state within a single bounce (binding 43).
///
/// Size: 240 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EwHit {
    /// Surface hit point (xyz) and ray parameter t (w).
    pub hit_point: [f32; 4],
    /// Facet normal (xyz); w = 0 (the facet index lives in `split.z`).
    pub hit_normal: [f32; 4],
    /// Shading normal near edges (xyz, w unused/0).
    pub shading_normal: [f32; 4],
    /// Incident wave direction at bounce (xyz, w unused/0).
    pub wave_dir_at_bounce: [f32; 4],
    /// Phase direction for MIS check (xyz) and phase PDF (w).
    pub phase_dir_this_check: [f32; 4],
    /// Miscellaneous flags: x = hit (0/1), y = `pre_bounce_inside_gem`, z = finish, w = arm.
    pub misc: [u32; 4],
    /// Split state: x = `split_mask`, y = bitcast `split_mis_weight`, z = facet index, w = 0.
    pub split: [u32; 4],
    /// Split directions for each channel (xyz = dir, w = transmitted intensity).
    pub split_dir: [[f32; 4]; 8],
}

const _: () = {
    assert!(size_of::<EwHit>() == 240);
    assert!(offset_of!(EwHit, hit_point) == 0);
    assert!(offset_of!(EwHit, hit_normal) == 16);
    assert!(offset_of!(EwHit, shading_normal) == 32);
    assert!(offset_of!(EwHit, wave_dir_at_bounce) == 48);
    assert!(offset_of!(EwHit, phase_dir_this_check) == 64);
    assert!(offset_of!(EwHit, misc) == 80);
    assert!(offset_of!(EwHit, split) == 96);
    assert!(offset_of!(EwHit, split_dir) == 112);
};

include!(concat!(env!("OUT_DIR"), "/ew_kernel_set_hash.rs"));

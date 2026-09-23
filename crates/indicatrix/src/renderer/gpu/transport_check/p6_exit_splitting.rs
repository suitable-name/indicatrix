//! Exit-event spectral splitting -- kernel-level equivalence checks for the three pure
//! per-channel helpers `shaders/transport_physics.wgsl`'s own exit-event splitting
//! section defines: `compute_channel_transmission`, `compute_uniaxial_exit_transmission`,
//! `narrow_compat`.
//!
//! # Why these CPU reference functions are transcriptions, not direct calls
//!
//! Every other Tier 2 case bank in this module tree (see `p2_uniaxial_fresnel.rs`, and
//! `mod.rs`'s own doc comment) calls the REAL CPU function it was translated from,
//! never a hand-written parallel reimplementation. That is not possible here:
//! `optics::raytracer::refraction::compute_channel_transmission` and
//! `compute_uniaxial_exit_transmission` are both module-private (not even
//! `pub(super)`), and `narrow_compat` is `pub(super)` -- visible only within
//! `optics::raytracer`, not from this crate's `renderer` tree. Widening any of their
//! visibility would mean editing `refraction.rs`, which is on the coordinator-owned/
//! protected list (`crates/indicatrix/src/optics/raytracer/
//! {refraction,transport,color}.rs`) -- not modifiable under any circumstance.
//!
//! `cpu_compute_channel_transmission`/`cpu_compute_uniaxial_exit_transmission`/
//! `cpu_narrow_compat` below are therefore verbatim transcriptions of the real CPU
//! source (same operations, same order, same `f32::mul_add` sites -- cross-referenced
//! by file/line in each function's own doc comment) rather than a call through the
//! module boundary, built as much as possible out of REAL, reachable crate primitives
//! (`optics::polarization::{MuellerMatrix, StokesVector}`,
//! `optics::raytracer::uniaxial_fresnel::Cplx`) so only the small amount of "glue"
//! arithmetic those private functions add on top is actually duplicated. This mirrors
//! the precedent `transport_check/mod.rs`'s own doc comment already sets for a
//! similarly unreachable CPU formula (the standalone scalar Fresnel-amplitude check it
//! deliberately omits) -- the difference here is this module explicitly requires
//! kernel-level coverage of these three functions, so rather than omitting the check
//! entirely this module accepts the transcription as the closest achievable
//! approximation to "the real CPU function" given the protected-file boundary.

use glam::Vec3;

use crate::{
    optics::{
        polarization::{MuellerMatrix, StokesVector},
        raytracer::uniaxial_fresnel::Cplx,
    },
    renderer::gpu::compute,
};

use super::{SHADER_SRC, UlpAccumulator, UlpCheckResult};

// ---------------------------------------------------------------------------------
// compute_channel_transmission
// (crates/indicatrix/src/optics/raytracer/refraction.rs:357-385)
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChannelTransmissionCase {
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    entering_anisotropic: u32,
    azimuth_valid: u32,
    stokes_i: f32,
    stokes_q: f32,
    stokes_u: f32,
    stokes_v: f32,
}

const CHANNEL_TRANSMISSION_ULP_BUDGET: u32 = 64;
const CHANNEL_TRANSMISSION_ABS_FLOOR: f32 = 1e-6;

fn build_channel_transmission_cases() -> Vec<ChannelTransmissionCase> {
    let mut cases = Vec::new();
    let angles_deg = [10.0f32, 30.0, 50.0, 70.0, 80.0];
    let index_pairs = [
        (1.0f32, 1.5f32),
        (1.5, 1.0),
        (1.0, 2.616),
        (2.616, 1.0),
        (1.925, 1.0),
    ];
    let stokes_samples = [
        (1.0f32, 0.0f32, 0.0f32, 0.0f32),
        (1.0, 0.3, -0.4, 0.2),
        (1.0, 1.0, 0.0, 0.0),
    ];
    for &ang in &angles_deg {
        let cos_i = ang.to_radians().cos();
        for &(n1k, n2k) in &index_pairs {
            let eta = n1k / n2k;
            let sin2_t_k = eta * eta * cos_i.mul_add(-cos_i, 1.0);
            if sin2_t_k > 1.0 {
                continue;
            }
            let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
            // `r_unpol` is always the shared HERO's own reflectance at (n1k, n2k,
            // cos_i) -- deriving it here from the identical scalar-Fresnel formula
            // `apply_partial_fresnel_bounce`/`mueller_fresnel_transmission`'s own
            // caller uses (rather than sweeping it independently of the geometry)
            // keeps every case physically self-consistent, exactly like a real
            // bounce dispatch: r_unpol=0.98 at near-normal incidence, or r_unpol=0.02
            // at grazing incidence, never occurs in a real trace and drives
            // `mueller_fresnel_transmission`'s own near-normal-incidence `s`/`p`
            // near-cancellation (see `MuellerMatrix::fresnel_transmission`'s `b =
            // 0.5 * (ts2 - tp2)`) combined with an UNREALISTIC `1 / (1 - r_unpol)`
            // amplification an independent sweep can hit but no physical geometry
            // ever would.
            let r_s =
                f32::mul_add(n1k, -cos_i, n2k * cos_t_k) / f32::mul_add(n1k, cos_i, n2k * cos_t_k);
            let r_p =
                f32::mul_add(n2k, -cos_i, n1k * cos_t_k) / f32::mul_add(n2k, cos_i, n1k * cos_t_k);
            let r_unpol = (0.5 * f32::mul_add(r_s, r_s, r_p * r_p)).clamp(0.02, 0.98);
            for entering_anisotropic in [false, true] {
                for azimuth_valid in [false, true] {
                    for &(si, sq, su, sv) in &stokes_samples {
                        cases.push(ChannelTransmissionCase {
                            n1k,
                            n2k,
                            cos_i,
                            cos_t_k,
                            r_unpol,
                            cos_2psi_x: 0.6,
                            sin_2psi_x: 0.8,
                            entering_anisotropic: u32::from(entering_anisotropic),
                            azimuth_valid: u32::from(azimuth_valid),
                            stokes_i: si,
                            stokes_q: sq,
                            stokes_u: su,
                            stokes_v: sv,
                        });
                    }
                }
            }
        }
    }
    cases
}

/// Verbatim transcription of `refraction::compute_channel_transmission` -- see this
/// module's own doc comment for why. Built from the real
/// `MuellerMatrix::fresnel_transmission`/`StokesVector::apply_matrix`/`scale`, so only
/// the small amount of glue arithmetic that function adds on top is duplicated.
fn cpu_compute_channel_transmission(c: &ChannelTransmissionCase) -> [f32; 5] {
    let n1k = c.n1k;
    let n2k = c.n2k;
    let cos_i = c.cos_i;
    let cos_t_k = c.cos_t_k;
    let r_unpol = c.r_unpol;
    let entering_anisotropic = c.entering_anisotropic != 0;
    let azimuth = (c.azimuth_valid != 0).then_some((c.cos_2psi_x, c.sin_2psi_x));
    let incident_stokes_k = StokesVector::new(c.stokes_i, c.stokes_q, c.stokes_u, c.stokes_v);

    let t_s_k = (2.0 * n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    let incident_k = if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = azimuth {
        let i_k = incident_stokes_k.i;
        StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
    } else {
        incident_stokes_k
    };
    let transmitted = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));
    let r_s_k = f32::mul_add(n2k, -cos_t_k, n1k * cos_i) / f32::mul_add(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = f32::mul_add(n1k, -cos_t_k, n2k * cos_i) / f32::mul_add(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(1e-4, 1.0 - 1e-4);
    [
        transmitted.i,
        transmitted.q,
        transmitted.u,
        transmitted.v,
        r_unpol_k,
    ]
}

const CHANNEL_TRANSMISSION_COMPONENT_NAMES: [&str; 5] = [
    "transmitted.i",
    "transmitted.q",
    "transmitted.u",
    "transmitted.v",
    "r_unpol_k",
];

#[must_use]
pub fn run_channel_transmission(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<ChannelTransmissionCase> {
    let cases = build_channel_transmission_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "channel_transmission in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "channel_transmission out",
        total * 5,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "channel_transmission_main",
        SHADER_SRC,
        "channel_transmission_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "channel_transmission bind group",
        &pipeline,
        &[(50, &in_buf), (51, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 5);

    let mut acc = UlpAccumulator::new(
        "compute_channel_transmission",
        CHANNEL_TRANSMISSION_ULP_BUDGET,
        CHANNEL_TRANSMISSION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_compute_channel_transmission(case);
        for c_idx in 0..5usize {
            acc.record(
                case,
                CHANNEL_TRANSMISSION_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 5 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// compute_uniaxial_exit_transmission
// (crates/indicatrix/src/optics/raytracer/refraction.rs:2243-2264)
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UniaxialExitTransmissionCase {
    t_s_re: f32,
    t_s_im: f32,
    t_p_re: f32,
    t_p_im: f32,
    flux_ts: f32,
    flux_tp: f32,
    flux_inc: f32,
    r_branch: f32,
    incident_i: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

const UNIAXIAL_EXIT_TRANSMISSION_ULP_BUDGET: u32 = 64;
const UNIAXIAL_EXIT_TRANSMISSION_ABS_FLOOR: f32 = 1e-6;

fn build_uniaxial_exit_transmission_cases() -> Vec<UniaxialExitTransmissionCase> {
    let mut cases = Vec::new();
    let amps = [0.0f32, 0.3, 0.7, 1.0, -0.2];
    let fluxes = [0.2f32, 0.6, 1.0, 1.8];
    let r_branch_vals = [0.02f32, 0.3, 0.6, 0.98];
    let incident_i_vals = [0.0f32, 0.5, 1.0, 3.5];
    for &t_s_re in &amps {
        for &t_p_re in &amps {
            for &flux_ts in &fluxes {
                for &flux_inc in &fluxes {
                    for &r_branch in &r_branch_vals {
                        for &incident_i in &incident_i_vals {
                            cases.push(UniaxialExitTransmissionCase {
                                t_s_re,
                                t_s_im: 0.1 * t_p_re,
                                t_p_re,
                                t_p_im: -0.15 * t_s_re,
                                flux_ts,
                                flux_tp: flux_ts.mul_add(0.7, 0.1),
                                flux_inc,
                                r_branch,
                                incident_i,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            });
                        }
                    }
                }
            }
        }
    }
    cases
}

/// Verbatim transcription of `refraction::compute_uniaxial_exit_transmission` -- see
/// this module's own doc comment for why. Built from the real
/// `uniaxial_fresnel::Cplx`'s own `scale`/`norm_sqr`/`mul`/`conj`, so only the small
/// amount of glue arithmetic that function adds on top is duplicated.
fn cpu_compute_uniaxial_exit_transmission(c: &UniaxialExitTransmissionCase) -> [f32; 5] {
    let t_s = Cplx {
        re: c.t_s_re,
        im: c.t_s_im,
    };
    let t_p = Cplx {
        re: c.t_p_re,
        im: c.t_p_im,
    };
    let flux_inc = c.flux_inc.max(1e-12);
    let ts_n = t_s.scale((c.flux_ts / flux_inc).sqrt());
    let tp_n = t_p.scale((c.flux_tp / flux_inc).sqrt());
    let i_unit = ts_n.norm_sqr() + tp_n.norm_sqr();
    let q_unit = ts_n.norm_sqr() - tp_n.norm_sqr();
    let cross = ts_n.mul(tp_n.conj());
    let u_unit = 2.0 * cross.re;
    let v_unit = -2.0 * cross.im;
    let incident_i = c.incident_i;
    let transmitted = StokesVector::new(
        incident_i * i_unit,
        incident_i * q_unit,
        incident_i * u_unit,
        incident_i * v_unit,
    )
    .scale(1.0 / (1.0 - c.r_branch));
    [
        transmitted.i,
        transmitted.q,
        transmitted.u,
        transmitted.v,
        i_unit,
    ]
}

const UNIAXIAL_EXIT_TRANSMISSION_COMPONENT_NAMES: [&str; 5] = [
    "transmitted.i",
    "transmitted.q",
    "transmitted.u",
    "transmitted.v",
    "i_unit",
];

#[must_use]
pub fn run_uniaxial_exit_transmission(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<UniaxialExitTransmissionCase> {
    let cases = build_uniaxial_exit_transmission_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "uniaxial_exit_transmission in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "uniaxial_exit_transmission out",
        total * 5,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "uniaxial_exit_transmission_main",
        SHADER_SRC,
        "uniaxial_exit_transmission_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "uniaxial_exit_transmission bind group",
        &pipeline,
        &[(52, &in_buf), (53, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 5);

    let mut acc = UlpAccumulator::new(
        "compute_uniaxial_exit_transmission",
        UNIAXIAL_EXIT_TRANSMISSION_ULP_BUDGET,
        UNIAXIAL_EXIT_TRANSMISSION_ABS_FLOOR,
    );
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_compute_uniaxial_exit_transmission(case);
        for c_idx in 0..5usize {
            acc.record(
                case,
                UNIAXIAL_EXIT_TRANSMISSION_COMPONENT_NAMES[c_idx],
                cpu[c_idx],
                gpu_out[idx * 5 + c_idx],
            );
        }
    }
    acc.finish()
}

// ---------------------------------------------------------------------------------
// narrow_compat (crates/indicatrix/src/optics/raytracer/refraction.rs:315-339) -- exact
// u32 equality, not a ULP comparison (the output is an integer bitmask, not a rounded
// float), so this uses its own small result type rather than `UlpCheckResult`.
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct NarrowCompatCase {
    dirs: [[f32; 4]; 8],
    hero_match_mask: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

#[derive(Debug, Clone, Copy)]
struct NarrowCompatMismatch {
    case_index: usize,
    channel: usize,
    cpu: u32,
    gpu: u32,
}

#[derive(Debug, Clone)]
pub struct NarrowCompatResult {
    pub total_cases: usize,
    pub mismatches: usize,
    first_mismatch: Option<NarrowCompatMismatch>,
}

impl NarrowCompatResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.mismatches == 0
    }

    #[must_use]
    pub fn describe_first_mismatch(&self) -> String {
        self.first_mismatch.map_or_else(
            || "none".to_string(),
            |m| {
                format!(
                    "case {} channel {}: cpu=0x{:02x} gpu=0x{:02x}",
                    m.case_index, m.channel, m.cpu, m.gpu
                )
            },
        )
    }
}

fn build_narrow_compat_cases() -> Vec<NarrowCompatCase> {
    let mut cases = Vec::new();
    // A grid of direction sets: the hero (channel 0) direction is fixed at +Z; every
    // other channel's direction is either exactly +Z (matches), a small perturbation
    // still within `DIRECTION_MATCH_COS_TOL` (matches), or a large perturbation well
    // outside it (mismatches) -- and `dirs_valid` (the `.w` component) is toggled off
    // for a subset of channels to exercise the CPU `Option::None` ("narrows nothing")
    // path. `hero_match_mask` is derived consistently with each direction set (bit `j`
    // set means `final_dir_k[j].dot(hero_dir) >= tol`), exactly mirroring how a real
    // bounce-dispatch call site derives it -- so this grid, and its expected output,
    // stays physically meaningful, not just syntactically valid.
    let hero_dir = Vec3::new(0.0, 0.0, 1.0);
    let close = Vec3::new(0.01, 0.0, 0.9999).normalize();
    let far = Vec3::new(0.6, 0.4, 0.6).normalize();
    let variants = [hero_dir, close, far];
    for pattern in 0..3usize.pow(7) {
        let mut p = pattern;
        let mut dirs = [[0.0f32; 4]; 8];
        dirs[0] = [hero_dir.x, hero_dir.y, hero_dir.z, 1.0];
        let mut hero_match_mask = 0u32;
        for (ch, slot) in dirs.iter_mut().enumerate().skip(1) {
            let variant = p % 3;
            p /= 3;
            let valid = ch % 4 != 0; // channel 4 always invalid, exercising Option::None
            let d = variants[variant];
            *slot = [d.x, d.y, d.z, f32::from(u8::from(valid))];
            if d.dot(hero_dir) >= DIRECTION_MATCH_COS_TOL {
                hero_match_mask |= 1 << ch;
            }
        }
        cases.push(NarrowCompatCase {
            dirs,
            hero_match_mask,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        });
        // 3^7 = 2187 cases is already a thorough sweep; cap it so this bank stays fast.
        if cases.len() >= 512 {
            break;
        }
    }
    cases
}

const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

/// Verbatim transcription of `refraction::narrow_compat`, specialized to the fixed
/// hero index 0 exactly like the WGSL mirror -- see this module's own doc comment for
/// why this is a transcription rather than a direct call.
fn cpu_narrow_compat(case: &NarrowCompatCase) -> [u32; 8] {
    let dirs: [Option<Vec3>; 8] = std::array::from_fn(|k| {
        let d = case.dirs[k];
        (d[3] > 0.5).then(|| Vec3::new(d[0], d[1], d[2]))
    });
    let hero_match: [bool; 8] = std::array::from_fn(|k| (case.hero_match_mask >> k) & 1 != 0);
    let mut compat = [0xFFu32; 8];
    for a in 0..8 {
        for b in (a + 1)..8 {
            let matches = if a == 0 {
                hero_match[b]
            } else if b == 0 {
                hero_match[a]
            } else if let (Some(da), Some(db)) = (dirs[a], dirs[b]) {
                da.dot(db) >= DIRECTION_MATCH_COS_TOL
            } else {
                true
            };
            if !matches {
                compat[a] &= !(1u32 << b);
                compat[b] &= !(1u32 << a);
            }
        }
    }
    compat
}

#[must_use]
pub fn run_narrow_compat(ctx: &crate::renderer::gpu::GpuContext) -> NarrowCompatResult {
    let cases = build_narrow_compat_cases();
    let total = cases.len();
    let in_buf = compute::upload(
        &ctx.device,
        "narrow_compat in",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<u32>(
        &ctx.device,
        "narrow_compat out",
        total * 8,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(
        &ctx.device,
        "narrow_compat_main",
        SHADER_SRC,
        "narrow_compat_main",
    );
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "narrow_compat bind group",
        &pipeline,
        &[(54, &in_buf), (55, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 8);

    let mut mismatches = 0usize;
    let mut first_mismatch = None;
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_narrow_compat(case);
        for ch in 0..8usize {
            let gpu = gpu_out[idx * 8 + ch];
            if cpu[ch] != gpu {
                mismatches += 1;
                if first_mismatch.is_none() {
                    first_mismatch = Some(NarrowCompatMismatch {
                        case_index: idx,
                        channel: ch,
                        cpu: cpu[ch],
                        gpu,
                    });
                }
            }
        }
    }

    NarrowCompatResult {
        total_cases: total,
        mismatches,
        first_mismatch,
    }
}

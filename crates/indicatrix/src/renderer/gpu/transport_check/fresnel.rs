//! Fresnel reflection/transmission -- both run through the shared
//! [`super::run_stokes_case_bank`] Mueller-matrix-and-`StokesVector::apply_matrix`
//! harness.

use crate::optics::polarization::{MuellerMatrix, StokesVector};

use super::{STOKES_SAMPLES, StokesCaseBankConfig, UlpCheckResult, run_stokes_case_bank};

// ---------------------------------------------------------------------------------
// fresnel_reflection
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FresnelReflectionCase {
    r_s: f32,
    r_p: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
}

const FRESNEL_REFLECTION_ULP_BUDGET: u32 = 16;
const FRESNEL_REFLECTION_ABS_FLOOR: f32 = 1e-6;

fn build_fresnel_reflection_cases() -> Vec<FresnelReflectionCase> {
    let mut cases = Vec::new();
    let steps = 20;
    for i in 0..=steps {
        let r_s = (i as f32 / steps as f32).mul_add(2.0, -1.0);
        for j in 0..=steps {
            let r_p = (j as f32 / steps as f32).mul_add(2.0, -1.0);
            for s in [STOKES_SAMPLES[0], STOKES_SAMPLES[4]] {
                cases.push(FresnelReflectionCase {
                    r_s,
                    r_p,
                    si: s[0],
                    sq: s[1],
                    su: s[2],
                    sv: s[3],
                    _pad0: 0.0,
                    _pad1: 0.0,
                });
            }
        }
    }
    // Adversarial: near +-1 (grazing/TIR-adjacent magnitude) and near 0 (normal incidence).
    for &(r_s, r_p) in &[
        (0.999_9, -0.999_9),
        (-0.999_9, 0.999_9),
        (0.0001, -0.0001),
        (1.0, 1.0),
        (-1.0, -1.0),
    ] {
        for s in STOKES_SAMPLES {
            cases.push(FresnelReflectionCase {
                r_s,
                r_p,
                si: s[0],
                sq: s[1],
                su: s[2],
                sv: s[3],
                _pad0: 0.0,
                _pad1: 0.0,
            });
        }
    }
    cases
}

fn cpu_fresnel_reflection(c: &FresnelReflectionCase) -> [f32; 4] {
    let m = MuellerMatrix::fresnel_reflection(c.r_s, c.r_p);
    let s = StokesVector::new(c.si, c.sq, c.su, c.sv);
    s.apply_matrix(&m).to_vec4().to_array()
}

#[must_use]
pub fn run_fresnel_reflection(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<FresnelReflectionCase> {
    let cases = build_fresnel_reflection_cases();
    let config = StokesCaseBankConfig {
        entry_point: "fresnel_reflection_main",
        in_binding: 2,
        out_binding: 3,
        budget: FRESNEL_REFLECTION_ULP_BUDGET,
        abs_floor: FRESNEL_REFLECTION_ABS_FLOOR,
    };
    run_stokes_case_bank(ctx, &config, &cases, cpu_fresnel_reflection)
}

// ---------------------------------------------------------------------------------
// fresnel_transmission
// ---------------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FresnelTransmissionCase {
    n1: f32,
    n2: f32,
    cos_i: f32,
    cos_t: f32,
    t_s: f32,
    t_p: f32,
    si: f32,
    sq: f32,
    su: f32,
    sv: f32,
    _pad0: f32,
    _pad1: f32,
}

const FRESNEL_TRANSMISSION_ULP_BUDGET: u32 = 16;
const FRESNEL_TRANSMISSION_ABS_FLOOR: f32 = 1e-6;

fn build_fresnel_transmission_cases() -> Vec<FresnelTransmissionCase> {
    let mut cases = Vec::new();
    let n_vals = [1.0f32, 1.3, 1.5, 1.77, 2.42];
    let cos_vals = [0.0f32, 0.05, 0.2, 0.5, 0.8, 0.999];
    for &n1 in &n_vals {
        for &n2 in &n_vals {
            for &cos_i in &cos_vals {
                for &cos_t in &cos_vals {
                    let t_s = 2.0 * n1 * cos_i / f32::mul_add(n1, cos_i, n2 * cos_t).max(1e-6);
                    let t_p = 2.0 * n1 * cos_i / f32::mul_add(n2, cos_i, n1 * cos_t).max(1e-6);
                    cases.push(FresnelTransmissionCase {
                        n1,
                        n2,
                        cos_i,
                        cos_t,
                        t_s,
                        t_p,
                        si: 1.0,
                        sq: 0.3,
                        su: -0.2,
                        sv: 0.1,
                        _pad0: 0.0,
                        _pad1: 0.0,
                    });
                }
            }
        }
    }
    // Adversarial: cos_i near 0 (triggers the max(n1*cos_i, 1e-6) clamp).
    for &cos_i in &[0.0f32, 1e-7, 1e-5] {
        cases.push(FresnelTransmissionCase {
            n1: 1.77,
            n2: 1.0,
            cos_i,
            cos_t: 0.5,
            t_s: 1.0,
            t_p: 1.0,
            si: 1.0,
            sq: 0.0,
            su: 0.0,
            sv: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
        });
    }
    cases
}

fn cpu_fresnel_transmission(c: &FresnelTransmissionCase) -> [f32; 4] {
    let m = MuellerMatrix::fresnel_transmission(c.n1, c.n2, c.cos_i, c.cos_t, c.t_s, c.t_p);
    let s = StokesVector::new(c.si, c.sq, c.su, c.sv);
    s.apply_matrix(&m).to_vec4().to_array()
}

#[must_use]
pub fn run_fresnel_transmission(
    ctx: &crate::renderer::gpu::GpuContext,
) -> UlpCheckResult<FresnelTransmissionCase> {
    let cases = build_fresnel_transmission_cases();
    let config = StokesCaseBankConfig {
        entry_point: "fresnel_transmission_main",
        in_binding: 4,
        out_binding: 5,
        budget: FRESNEL_TRANSMISSION_ULP_BUDGET,
        abs_floor: FRESNEL_TRANSMISSION_ABS_FLOOR,
    };
    run_stokes_case_bank(ctx, &config, &cases, cpu_fresnel_transmission)
}

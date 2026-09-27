//! Core single-wavelength ray physics shared by every metric: Fresnel
//! transmittance and the refract-then-bounce trace that
//! [`super::classify`]/[`super::scintillation`] replay at the d-line, F-line,
//! and C-line indices.

use glam::Vec3;

use crate::optics::raytracer::{Ray, intersect_polyhedron_soa};

/// Outcome of refracting an incident ray into the gemstone at a known entry point/normal
/// (for one specific refractive index) and following it through up to 10 internal
/// bounces. Factored out so the exact same entry-refraction-then-bounce logic can be
/// replayed at the d-line index (windowing/extinction/brilliance classification) and
/// independently at the F-line and C-line indices (Fire measurement), from the
/// identical physical entry point and incident direction.
#[derive(Debug, Clone, Copy)]
pub(super) enum RayFate {
    /// Entry refraction has no real solution at this index (only possible for a
    /// pathological index < 1); not classified into any bucket. Unreachable for the
    /// d-line index (clamped to >= 1.1), kept for defensive symmetry with the F/C traces.
    EntryBlocked,
    /// Leaked out through the pavilion bottom (`n_out.y < -0.05`): windowing.
    Leaked,
    /// Exited back out through the upper hemisphere/crown: direction, the fraction of
    /// incident intensity transmitted along this exact path (Fresnel transmittance at
    /// entry times exit, TIR bounces in between being lossless), and the cosine of the
    /// exit angle from the exit facet's own normal (the projected-area radiometric
    /// factor used to weight Fire, distinct from transmittance). Also carries the exit
    /// facet index and internal bounce count -- see `ExitPath`'s doc.
    ExitedUpward(ExitPath),
    /// Trapped internally: exhausted its bounce budget, exited sideways through the
    /// girdle, or hit no further facet.
    Absorbed,
}

/// The physical exit path of a ray that escaped upward through the crown: exit
/// direction, Fresnel entry*exit transmittance and exit-facet-normal cosine (see
/// `RayFate::ExitedUpward`'s doc), plus which facet it exited through and how many
/// internal TIR bounces it took to get there.
///
/// The facet index and bounce count let the Fire measurement tell whether a ray's
/// F-line and C-line companion traces exited via the SAME physical path. Near a
/// critical angle, F and C can straddle the TIR threshold at different bounces --
/// `acos(dir_f . dir_c)` between such unrelated exit directions measures nothing
/// physically meaningful, yet can dominate the weighted Fire sum -- see the bifurcation
/// gate in `evaluate_gem_optical_metrics`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ExitPath {
    pub(super) dir: Vec3,
    pub(super) transmittance: f32,
    pub(super) exit_cos_theta: f32,
    pub(super) facet_idx: usize,
    pub(super) bounces: u32,
}

/// Unpolarized Fresnel transmittance at a dielectric interface, given the cosines of the
/// incident and transmitted angles on either side (`n1` -> `n2`). Averages the s- and
/// p-polarized reflectances (`Rs`, `Rp`) into a single scalar reflectance `R`, then
/// returns `T = 1 - R`, clamped to [0, 1]. Used to weight each ray's contribution to Fire
/// by the energy it actually delivers, rather than counting every surviving ray equally
/// regardless of how much of its light made it through the entry and exit interfaces.
fn fresnel_transmittance(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> f32 {
    let denom_s = n2.mul_add(cos_t, n1 * cos_i);
    let denom_p = n2.mul_add(cos_i, n1 * cos_t);
    // Denominators vanish only at grazing incidence, where reflectance already tends
    // to 1 (T -> 0); zero transmittance there is safe (no 0/0 NaN) and physically correct.
    if denom_s.abs() < 1e-6 || denom_p.abs() < 1e-6 {
        return 0.0;
    }
    let rs = (n2.mul_add(-cos_t, n1 * cos_i) / denom_s).powi(2);
    let rp = (n2.mul_add(-cos_i, n1 * cos_t) / denom_p).powi(2);
    let r = f32::midpoint(rs, rp);
    (1.0 - r).clamp(0.0, 1.0)
}

/// Refracts `incoming_dir` into the gem at `entry_point`/`n_entry` using Snell's law for
/// `index`, then follows up to 10 internal bounces (TIR vs refract-out) exactly as the
/// original single-wavelength trace did. This is the physical core shared by the d-line,
/// F-line, and C-line traces -- identical control flow, parameterized only by which
/// refractive index the light is carrying.
pub(super) fn trace_wavelength(
    entry_point: Vec3,
    incoming_dir: Vec3,
    n_entry: Vec3,
    cos_i: f32,
    plane_soa: &crate::simd::PlanesSoA32,
    index: f32,
) -> RayFate {
    let sin2_t = (1.0 / (index * index)) * cos_i.mul_add(-cos_i, 1.0);
    if sin2_t > 1.0 {
        return RayFate::EntryBlocked;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    let mut curr_dir =
        ((1.0 / index) * incoming_dir + (1.0 / index).mul_add(cos_i, -cos_t) * n_entry).normalize();
    let mut hit_point = entry_point + curr_dir * 1e-4;
    let sin_crit = 1.0 / index;
    // Entry transmittance (air -> gem): computed once here since cos_i/cos_t at entry
    // don't change across bounces; combined with the exit transmittance below to give
    // this path's total energy weight.
    let entry_transmittance = fresnel_transmittance(1.0, index, cos_i, cos_t);

    let mut leaked = false;
    let mut exited_upwards = false;
    let mut exit_dir = Vec3::ZERO;
    let mut exit_transmittance = 0.0f32;
    let mut exit_cos_theta = 0.0f32;
    let mut exit_facet_idx = usize::MAX;
    let mut exit_bounces = 0u32;

    for bounce in 0..10 {
        let inside_ray = Ray {
            origin: hit_point,
            dir: curr_dir,
        };
        let next_hit = intersect_polyhedron_soa(inside_ray, plane_soa);
        let Some(next_rec) = next_hit else { break };
        let next_point = inside_ray.origin + next_rec.t * inside_ray.dir;
        let n_out = next_rec.normal; // outward-pointing facet normal

        let cos_theta = curr_dir.dot(n_out).clamp(0.0, 1.0);
        let sin_theta = cos_theta.mul_add(-cos_theta, 1.0).max(0.0).sqrt();

        if sin_theta < sin_crit {
            // Refracts out of the stone (TIR failed)
            let sin2_out = (index * index) * cos_theta.mul_add(-cos_theta, 1.0);
            if sin2_out <= 1.0 {
                let cos_out = (1.0 - sin2_out).sqrt();
                let out_dir =
                    (index * curr_dir + index.mul_add(-cos_theta, cos_out) * n_out).normalize();
                if n_out.y < -0.05 {
                    leaked = true; // Leaks out through pavilion bottom -> windowing.
                } else if out_dir.y > 0.05 {
                    // Exits back toward upper hemisphere / crown.
                    exited_upwards = true;
                    exit_dir = out_dir;
                    exit_transmittance = fresnel_transmittance(index, 1.0, cos_theta, cos_out);
                    exit_cos_theta = cos_out;
                    exit_facet_idx = next_rec.facet_idx;
                    exit_bounces = bounce;
                }
                break;
            }
        }

        // Total Internal Reflection (TIR)
        curr_dir = (curr_dir - 2.0 * cos_theta * n_out).normalize();
        hit_point = next_point + curr_dir * 1e-4;
    }

    if leaked {
        RayFate::Leaked
    } else if exited_upwards {
        RayFate::ExitedUpward(ExitPath {
            dir: exit_dir,
            transmittance: entry_transmittance * exit_transmittance,
            exit_cos_theta,
            facet_idx: exit_facet_idx,
            bounces: exit_bounces,
        })
    } else {
        RayFate::Absorbed
    }
}

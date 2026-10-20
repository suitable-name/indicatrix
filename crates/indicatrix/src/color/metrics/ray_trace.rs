//! Core single-wavelength ray physics shared by every metric: Fresnel
//! transmittance and the refract-then-bounce trace that
//! [`super::classify`]/[`super::scintillation`] replay at the d-line, F-line,
//! and C-line indices.

use glam::Vec3;

use super::{
    frosted::{FrostedEvent, frosted_event, path_seed},
    image_types::{ExtinctionReason, RayDetail},
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::{FacetFinish, HitRecord, Ray, intersect_stone::intersect_stone_soa},
    simd::PlanesSoA32,
};

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
    /// Total internal path of the ray in model units: the sum of every internal segment
    /// length (`next_rec.t` of each bounce, the final exit segment included), so a ray that
    /// exits at bounce 0 has one segment. Together with `transmittance` (the Fresnel
    /// weight) it describes the ray's contribution to the face-up colour: the weight says
    /// how much light the ray carries, the path how far that light travelled through the
    /// stone's body colour (Beer-Lambert).
    pub(super) path_len: f32,
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

/// The stone every metrics ray is traced against: the plane arena built once per
/// evaluation, plus the tools subtracted from it (empty for a planar stone).
///
/// Bundled so the contexts and [`trace_entry`] gain one field instead of two, and
/// so every metrics tracer calls the one intersector [`intersect_stone_soa`], which is
/// `intersect_polyhedron_soa` exactly when `tools` is empty.
///
/// `finishes` is the per-facet finish list, indexed like [`HitRecord::facet_idx`] (a missing
/// entry is polished). It is EMPTY unless some facet is frosted -- `PreparedEval` drops an
/// all-polished list -- so a stone without frosting takes exactly the polished code path.
#[derive(Clone, Copy)]
pub(super) struct StoneArena<'a> {
    pub(super) plane_soa: &'a PlanesSoA32,
    pub(super) tools: &'a [ToolPrimitive],
    pub(super) finishes: &'a [FacetFinish],
}

impl StoneArena<'_> {
    /// First surface `ray` meets; see [`intersect_stone_soa`].
    pub(super) fn intersect(&self, ray: Ray) -> Option<HitRecord> {
        intersect_stone_soa(ray, self.plane_soa, self.plane_soa.len(), self.tools)
    }

    /// Whether facet `facet_idx` is frosted. Always `false` for a stone without frosting.
    pub(super) fn is_frosted(&self, facet_idx: usize) -> bool {
        self.finishes.get(facet_idx) == Some(&FacetFinish::Frosted)
    }
}

/// A camera ray's entry into the stone: where it hit, along what, the facet's outward normal
/// and cosine of incidence, and which facet it was.
///
/// The facet matters because a frosted entry facet scatters the ray instead of refracting it.
#[derive(Clone, Copy)]
pub(super) struct EntryRay {
    pub(super) point: Vec3,
    pub(super) dir: Vec3,
    pub(super) normal: Vec3,
    pub(super) cos_i: f32,
    pub(super) facet: usize,
}

/// Records `reason` in the optional diagnostic metadata.
fn note_reason(detail: &mut Option<&mut RayDetail>, reason: ExtinctionReason) {
    if let Some(d) = detail.as_deref_mut() {
        d.reason = reason;
    }
}

/// Where a ray's inward journey starts: the point just inside the stone, its direction, the
/// entry interface's transmittance (`1` when a frosted entry facet scattered it, because the
/// scatter branch carries the energy) and the seed of the frosted draws (`0` and unused when
/// the stone has no frosting).
#[derive(Clone, Copy)]
struct Interior {
    hit_point: Vec3,
    dir: Vec3,
    entry_transmittance: f32,
    seed: u32,
}

/// The facet a ray meets from inside the stone, as the exit bookkeeping needs it.
#[derive(Clone, Copy)]
struct InteriorHit {
    /// The facet's outward normal.
    n_out: Vec3,
    /// Cosine of the angle of incidence, clamped to `[0, 1]`.
    cos_theta: f32,
    facet: usize,
    /// Number of the interior hit (the first is `0`).
    bounce: u32,
}

/// Where a ray left the stone: the bookkeeping of its one exit, if it has one.
#[derive(Clone, Copy, Default)]
struct Escape {
    leaked: bool,
    exited_upwards: bool,
    dir: Vec3,
    /// The exit interface's transmittance only; the entry's is multiplied in at the end.
    transmittance: f32,
    cos_theta: f32,
    facet_idx: usize,
    bounces: u32,
}

impl Escape {
    /// Records that the ray left through `hit` along `out_dir`, with exit cosine `cos_out`:
    /// out through the pavilion bottom (windowing), back up through the crown, or sideways
    /// (nothing recorded, the ray is lost). `transmittance` is only evaluated for an upward
    /// exit.
    fn record(
        &mut self,
        hit: InteriorHit,
        out_dir: Vec3,
        cos_out: f32,
        transmittance: impl FnOnce() -> f32,
        detail: &mut Option<&mut RayDetail>,
    ) {
        if let Some(d) = detail.as_deref_mut() {
            d.exit_facet = Some(hit.facet);
            d.bounces = hit.bounce;
            d.reason = ExtinctionReason::SideExit;
        }
        if hit.n_out.y < -0.05 {
            note_reason(detail, ExtinctionReason::None);
            self.leaked = true; // Leaks out through pavilion bottom -> windowing.
        } else if out_dir.y > 0.05 {
            // Exits back toward upper hemisphere / crown.
            note_reason(detail, ExtinctionReason::None);
            self.exited_upwards = true;
            self.dir = out_dir;
            self.transmittance = transmittance();
            self.cos_theta = cos_out;
            self.facet_idx = hit.facet;
            self.bounces = hit.bounce;
        }
    }
}

/// A frosted facet met from inside: the render tracer's `apply_frosted_bounce` with the
/// facet's normal turned to face the stone (see the `frosted` module). `crossed` means the
/// ray leaves; otherwise it scatters back into the stone.
fn frosted_interior(hit: InteriorHit, index: f32, seed: u32) -> FrostedEvent {
    let sin2_t = (index * index) * hit.cos_theta.mul_add(-hit.cos_theta, 1.0);
    // Bounce 0 is the entry event; interior hit `k` is event `k + 1`.
    frosted_event(
        (index, 1.0),
        hit.cos_theta,
        sin2_t,
        -hit.n_out,
        seed,
        hit.bounce + 1,
    )
}

/// What a ray does at a frosted facet it meets from inside, at interior hit `hit`: `None`
/// when it crosses the facet and leaves the stone (the exit is recorded in `escape`), else
/// `Some(dir)`, the direction it scatters back into the stone along.
fn scatter_at_frosted(
    hit: InteriorHit,
    index: f32,
    seed: u32,
    escape: &mut Escape,
    detail: &mut Option<&mut RayDetail>,
) -> Option<Vec3> {
    let event = frosted_interior(hit, index, seed);
    if event.crossed {
        escape.record(hit, event.dir, event.dir.dot(hit.n_out), || 1.0, detail);
        return None;
    }
    if let Some(d) = detail.as_deref_mut() {
        d.bounces = hit.bounce + 1;
    }
    Some(event.dir)
}

/// Traces a camera ray that entered the stone through `entry`, at refractive index `index`:
/// [`trace_wavelength_detail`] (Snell refraction at the entry), except that a frosted entry
/// facet scatters the ray instead -- see the `frosted` module. For a stone without frosting
/// this is [`trace_wavelength_detail`] and nothing else.
///
/// A ray the frosted entry facet reflects diffusely back out never enters the stone, so it is
/// not returned light: the fan counts only light that went through the stone, as it does for a
/// polished facet (whose specular reflection is dropped too).
pub(super) fn trace_entry(
    entry: EntryRay,
    stone: StoneArena<'_>,
    index: f32,
    mut detail: Option<&mut RayDetail>,
) -> RayFate {
    if !stone.is_frosted(entry.facet) {
        return trace_wavelength_detail(
            entry.point,
            entry.dir,
            entry.normal,
            entry.cos_i,
            stone,
            index,
            detail,
        );
    }
    note_reason(&mut detail, ExtinctionReason::BounceBudget);
    let seed = path_seed(entry.point, entry.dir);
    let sin2_t = (1.0 / (index * index)) * entry.cos_i.mul_add(-entry.cos_i, 1.0);
    let event = frosted_event((1.0, index), entry.cos_i, sin2_t, entry.normal, seed, 0);
    if event.crossed {
        return trace_interior(
            Interior {
                hit_point: entry.point + event.dir * 1e-4,
                dir: event.dir,
                entry_transmittance: 1.0,
                seed,
            },
            stone,
            index,
            detail,
        );
    }
    // Reflected diffusely at the entry: not entered, so nothing is returned.
    if let Some(d) = detail {
        d.exit_facet = Some(entry.facet);
        d.bounces = 0;
        d.reason = ExtinctionReason::SideExit;
    }
    RayFate::Absorbed
}

/// Refracts `incoming_dir` into the gem at `entry_point`/`n_entry` using Snell's law for
/// `index`, then follows up to 10 internal bounces (TIR vs refract-out) exactly as the
/// original single-wavelength trace did. This is the physical core shared by the d-line,
/// F-line, and C-line traces -- identical control flow, parameterized only by which
/// refractive index the light is carrying. Identical arithmetic, with optional diagnostic
/// metadata; no path allocation.
fn trace_wavelength_detail(
    entry_point: Vec3,
    incoming_dir: Vec3,
    n_entry: Vec3,
    cos_i: f32,
    stone: StoneArena<'_>,
    index: f32,
    mut detail: Option<&mut RayDetail>,
) -> RayFate {
    note_reason(&mut detail, ExtinctionReason::BounceBudget);
    let sin2_t = (1.0 / (index * index)) * cos_i.mul_add(-cos_i, 1.0);
    if sin2_t > 1.0 {
        note_reason(&mut detail, ExtinctionReason::EntryBlocked);
        return RayFate::EntryBlocked;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    let curr_dir =
        ((1.0 / index) * incoming_dir + (1.0 / index).mul_add(cos_i, -cos_t) * n_entry).normalize();
    // Entry transmittance (air -> gem): computed once here since cos_i/cos_t at entry
    // don't change across bounces; combined with the exit transmittance to give this
    // path's total energy weight.
    let entry_transmittance = fresnel_transmittance(1.0, index, cos_i, cos_t);
    let seed = if stone.finishes.is_empty() {
        0
    } else {
        path_seed(entry_point, incoming_dir)
    };
    trace_interior(
        Interior {
            hit_point: entry_point + curr_dir * 1e-4,
            dir: curr_dir,
            entry_transmittance,
            seed,
        },
        stone,
        index,
        detail,
    )
}

/// Follows a ray that is inside the stone for up to 10 interface events: total internal
/// reflection (or a diffuse scatter at a frosted facet) keeps it in, a refraction (or a
/// frosted transmit) lets it out. See [`RayFate`] for the outcomes.
fn trace_interior(
    start: Interior,
    stone: StoneArena<'_>,
    index: f32,
    mut detail: Option<&mut RayDetail>,
) -> RayFate {
    let mut curr_dir = start.dir;
    let mut hit_point = start.hit_point;
    let sin_crit = 1.0 / index;
    let mut escape = Escape::default();
    let mut path_len = 0.0f32;

    for bounce in 0..10 {
        if let Some(d) = detail.as_deref_mut() {
            d.bounces = bounce;
        }
        let inside_ray = Ray {
            origin: hit_point,
            dir: curr_dir,
        };
        let next_hit = stone.intersect(inside_ray);
        let Some(next_rec) = next_hit else {
            note_reason(&mut detail, ExtinctionReason::NoIntersection);
            break;
        };
        if detail.is_some()
            && (!next_rec.t.is_finite()
                || !curr_dir.is_finite()
                || !hit_point.is_finite()
                || !next_rec.normal.is_finite())
        {
            note_reason(&mut detail, ExtinctionReason::Invalid);
            return RayFate::Absorbed;
        }
        path_len += next_rec.t;
        let next_point = inside_ray.origin + next_rec.t * inside_ray.dir;
        let n_out = next_rec.normal; // outward-pointing facet normal

        let cos_theta = curr_dir.dot(n_out).clamp(0.0, 1.0);
        let sin_theta = cos_theta.mul_add(-cos_theta, 1.0).max(0.0).sqrt();
        let hit = InteriorHit {
            n_out,
            cos_theta,
            facet: next_rec.facet_idx,
            bounce,
        };

        if stone.is_frosted(hit.facet) {
            let Some(dir) = scatter_at_frosted(hit, index, start.seed, &mut escape, &mut detail)
            else {
                break;
            };
            curr_dir = dir;
            hit_point = next_point + curr_dir * 1e-4;
            continue;
        }

        if sin_theta < sin_crit {
            // Refracts out of the stone (TIR failed)
            let sin2_out = (index * index) * cos_theta.mul_add(-cos_theta, 1.0);
            if sin2_out <= 1.0 {
                let cos_out = (1.0 - sin2_out).sqrt();
                let out_dir =
                    (index * curr_dir + index.mul_add(-cos_theta, cos_out) * n_out).normalize();
                escape.record(
                    hit,
                    out_dir,
                    cos_out,
                    || fresnel_transmittance(index, 1.0, cos_theta, cos_out),
                    &mut detail,
                );
                break;
            }
        }

        // Total Internal Reflection (TIR)
        curr_dir = (curr_dir - 2.0 * cos_theta * n_out).normalize();
        hit_point = next_point + curr_dir * 1e-4;
        if let Some(d) = detail.as_deref_mut() {
            d.bounces = bounce + 1;
        }
    }

    if escape.leaked {
        RayFate::Leaked
    } else if escape.exited_upwards {
        RayFate::ExitedUpward(ExitPath {
            dir: escape.dir,
            transmittance: start.entry_transmittance * escape.transmittance,
            exit_cos_theta: escape.cos_theta,
            facet_idx: escape.facet_idx,
            bounces: escape.bounces,
            path_len,
        })
    } else {
        RayFate::Absorbed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::metrics::image_types::{ExtinctionReason, RayDetail},
        geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, LightingPreset},
        },
    };

    fn kind(fate: &RayFate) -> u8 {
        match fate {
            RayFate::EntryBlocked => 0,
            RayFate::Leaked => 1,
            RayFate::ExitedUpward(_) => 2,
            RayFate::Absorbed => 3,
        }
    }

    /// Over a grid of camera rays through the emerald cut, the diagnostic trace follows the
    /// plain trace exactly and the `ExtinctionReason` it writes agrees with the fate: a
    /// leaked or upward-exiting ray has no reason and an exit facet, an absorbed ray always
    /// has one, and a spent bounce budget is exactly ten bounces.
    #[test]
    fn detail_reasons_match_fates() {
        let planes = StandardGemCuts::emerald_cut();
        let material = GemMaterial::diamond();
        let prepared = super::super::prepared::PreparedEval::new(
            StoneGeometry::planes_only(&planes),
            &material,
            LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
        )
        .unwrap();
        let stone = prepared.stone();
        let camera = Camera::new(0.4, 0.9, 2.4, 42.0);
        let (mut leaked, mut upward, mut absorbed) = (0, 0, 0);
        for gy in 0..48 {
            for gx in 0..48 {
                let ray = camera.generate_ray(gx as f32, gy as f32, 48.0, 48.0, 0.0, 0.0);
                let Some(hit) = stone.intersect(ray) else {
                    continue;
                };
                let entry = ray.origin + hit.t * ray.dir;
                let cos_i = (-ray.dir).dot(hit.normal).clamp(0.0, 1.0);
                let entry_ray = EntryRay {
                    point: entry,
                    dir: ray.dir,
                    normal: hit.normal,
                    cos_i,
                    facet: hit.facet_idx,
                };
                let plain = trace_entry(entry_ray, stone, prepared.nd, None);
                let mut detail = RayDetail::default();
                let detailed = trace_entry(entry_ray, stone, prepared.nd, Some(&mut detail));
                assert_eq!(kind(&plain), kind(&detailed), "pixel {gx},{gy}");
                match detailed {
                    RayFate::EntryBlocked => {
                        assert_eq!(detail.reason, ExtinctionReason::EntryBlocked);
                    }
                    RayFate::Leaked => {
                        leaked += 1;
                        assert_eq!(detail.reason, ExtinctionReason::None, "pixel {gx},{gy}");
                        assert!(detail.exit_facet.is_some());
                    }
                    RayFate::ExitedUpward(path) => {
                        upward += 1;
                        assert_eq!(detail.reason, ExtinctionReason::None, "pixel {gx},{gy}");
                        assert_eq!(detail.exit_facet, Some(path.facet_idx));
                        assert_eq!(detail.bounces, path.bounces);
                    }
                    RayFate::Absorbed => {
                        absorbed += 1;
                        assert_ne!(detail.reason, ExtinctionReason::None, "pixel {gx},{gy}");
                        match detail.reason {
                            ExtinctionReason::BounceBudget => assert_eq!(detail.bounces, 10),
                            ExtinctionReason::SideExit => assert!(detail.exit_facet.is_some()),
                            _ => {}
                        }
                    }
                }
            }
        }
        assert!(
            leaked > 0 && upward > 0,
            "grid saw {leaked} leaked, {upward} upward, {absorbed} absorbed"
        );
    }
}

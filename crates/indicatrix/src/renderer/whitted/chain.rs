//! Fixed-order internal reflection chains, including bounded cavity re-entry.

use glam::Vec3;

use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::{FacetFinish, HitRecord, Ray, intersect_stone::intersect_stone_soa_side},
    renderer::frame_scene::FrameScene,
    simd::{PlanesSoA32, exp_lane},
};

use super::{
    CUTOFF, WORKLIST_CAPACITY, WhittedPrep,
    fresnel::{dot, fresnel_unpolarised_f32, reflect, refract, scale_add},
};

const RESTART: f32 = 1.0e-4;

#[derive(Clone, Copy)]
struct Branch {
    ray: Ray,
    weight: Vec3,
    inside: bool,
    depth: u32,
    primary: bool,
}

struct Worklist {
    slots: [Option<Branch>; WORKLIST_CAPACITY],
    len: usize,
    cutoff: f32,
}

impl Worklist {
    const fn new(cutoff: f32) -> Self {
        Self {
            slots: [None; WORKLIST_CAPACITY],
            len: 0,
            cutoff,
        }
    }

    fn push(&mut self, branch: Branch, max_depth: u32) {
        if branch.depth <= max_depth && branch.weight.max_element() >= self.cutoff {
            debug_assert!(self.len < WORKLIST_CAPACITY);
            self.slots[self.len] = Some(branch);
            self.len += 1;
        }
    }

    const fn pop(&mut self) -> Option<Branch> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        self.slots[self.len].take()
    }
}

fn restarted(point: Vec3, dir: Vec3) -> Ray {
    Ray {
        origin: scale_add(dir, RESTART, point),
        dir,
    }
}

/// Reserve the last stack slot for the internal continuation. Outside leaves
/// resolve directly for convex stones; on cavity overflow only that leaf falls
/// back to the environment. The continuation is never silently overwritten.
#[expect(
    clippy::too_many_arguments,
    reason = "flat scalar chain state mirrors the WGSL twin's parameter list"
)]
fn emit_outside(
    work: &mut Worklist,
    sum: &mut Vec3,
    prep: &WhittedPrep<'_>,
    tools: &[ToolPrimitive],
    ray: Ray,
    weight: Vec3,
    observer: Vec3,
    depth: u32,
) {
    if tools.is_empty() || work.len >= WORKLIST_CAPACITY - 1 {
        *sum += weight * prep.env.leaf(ray.dir, observer);
    } else {
        work.push(
            Branch {
                ray,
                weight,
                inside: false,
                depth,
                primary: false,
            },
            prep.max_depth,
        );
    }
}

/// Fresnel-split one surface interaction into its reflected and transmitted branches.
#[expect(
    clippy::too_many_arguments,
    reason = "flat scalar chain state mirrors the WGSL twin's parameter list"
)]
fn split_at_surface(
    work: &mut Worklist,
    sum: &mut Vec3,
    prep: &WhittedPrep<'_>,
    tools: &[ToolPrimitive],
    branch: Branch,
    point: Vec3,
    normal: Vec3,
    observer: Vec3,
    n: f32,
) {
    let cos_i = (-dot(branch.ray.dir, normal)).clamp(0.0, 1.0);
    let eta = if branch.inside { n } else { 1.0 / n };
    let reflectance = fresnel_unpolarised_f32(cos_i, eta);
    let reflection = restarted(point, reflect(branch.ray.dir, normal));
    let depth = branch.depth.saturating_add(1);

    if branch.inside {
        if let Some(transmitted) = refract(branch.ray.dir, normal, eta) {
            emit_outside(
                work,
                sum,
                prep,
                tools,
                restarted(point, transmitted),
                branch.weight * (1.0 - reflectance),
                observer,
                depth,
            );
        }
        work.push(
            Branch {
                ray: reflection,
                weight: branch.weight * reflectance,
                inside: true,
                depth,
                primary: false,
            },
            prep.max_depth,
        );
    } else {
        let glare = if branch.primary { prep.glare } else { 1.0 };
        emit_outside(
            work,
            sum,
            prep,
            tools,
            reflection,
            branch.weight * (reflectance * glare),
            observer,
            depth,
        );
        if let Some(transmitted) = refract(branch.ray.dir, normal, eta) {
            work.push(
                Branch {
                    ray: restarted(point, transmitted),
                    weight: branch.weight * (1.0 - reflectance),
                    inside: true,
                    depth,
                    primary: false,
                },
                prep.max_depth,
            );
        }
    }
}

/// Traces a single scalar-index RGB chain. The optional hit is always the
/// camera's first intersection, never a later branch's intersection.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame scene, prepared state and one camera ray stay flat to mirror the WGSL twin"
)]
pub(super) fn trace_chain(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    camera_ray: Ray,
    n: f32,
    observer: Vec3,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    trace_chain_with_cutoff(
        scene,
        tools,
        plane_soa,
        prep,
        camera_ray,
        n,
        observer,
        primary_hit_out,
        CUTOFF,
    )
}

/// The production cutoff is fixed; an explicit cutoff also lets analytic tests
/// measure energy conservation independently of intentional pruning.
#[expect(
    clippy::too_many_arguments,
    reason = "trace_chain's parameters plus the explicit cutoff"
)]
pub(super) fn trace_chain_with_cutoff(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    camera_ray: Ray,
    n: f32,
    observer: Vec3,
    mut primary_hit_out: Option<&mut Option<HitRecord>>,
    cutoff: f32,
) -> Vec3 {
    if let Some(out) = primary_hit_out.as_deref_mut() {
        *out = None;
    }
    let mut sum = Vec3::ZERO;
    let mut work = Worklist::new(cutoff);
    work.push(
        Branch {
            ray: camera_ray,
            weight: Vec3::ONE,
            inside: false,
            depth: 0,
            primary: true,
        },
        prep.max_depth,
    );

    while let Some(mut branch) = work.pop() {
        // The shared intersection retains an empty-polyhedron sentinel. An
        // image scene with no planes is instead an unobstructed environment.
        let hit = if scene.planes.is_empty() {
            None
        } else {
            intersect_stone_soa_side(branch.ray, plane_soa, scene.planes.len(), tools)
        };
        let Some((hit, entering)) = hit else {
            // One lookup for every escape: a primary miss sees the same lit tray a
            // window does.
            sum += branch.weight * prep.env.leaf(branch.ray.dir, observer);
            continue;
        };
        if branch.primary
            && let Some(out) = primary_hit_out.as_deref_mut()
        {
            *out = Some(hit);
        }
        // At close CSG boundaries the restart offset may cross another face.
        // The intersection's side is authoritative for this entire segment.
        if let Some(entering) = entering {
            branch.inside = !entering;
        }
        if branch.inside {
            let attenuation = -prep.alpha.at(branch.ray.dir) * hit.t;
            branch.weight *= Vec3::new(
                exp_lane(attenuation.x),
                exp_lane(attenuation.y),
                exp_lane(attenuation.z),
            );
        }
        if matches!(
            scene.facet_finishes.get(hit.facet_idx),
            Some(FacetFinish::Frosted)
        ) {
            sum += branch.weight * prep.frost.get(hit.facet_idx).copied().unwrap_or(Vec3::ZERO);
            continue;
        }
        let point = scale_add(branch.ray.dir, hit.t, branch.ray.origin);
        let normal = if branch.inside {
            -hit.normal
        } else {
            hit.normal
        };
        split_at_surface(
            &mut work, &mut sum, prep, tools, branch, point, normal, observer, n,
        );
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cavity_overflow_preserves_the_continuation_and_resolves_the_leaf() {
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::new(0.3, 0.7, 0.9), 64, 1);
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: Vec3::Z,
        };
        let branch = Branch {
            ray,
            weight: Vec3::ONE,
            inside: true,
            depth: 1,
            primary: false,
        };
        let mut work = Worklist::new(CUTOFF);
        for _ in 0..WORKLIST_CAPACITY - 1 {
            work.push(branch, prep.max_depth);
        }
        let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.2)];
        let mut sum = Vec3::ZERO;
        emit_outside(
            &mut work,
            &mut sum,
            &prep,
            &tools,
            ray,
            Vec3::splat(0.25),
            Vec3::NEG_Z,
            2,
        );
        assert_eq!(work.len, WORKLIST_CAPACITY - 1);
        assert!((sum - Vec3::new(0.3, 0.7, 0.9) * 0.25).abs().max_element() < 1e-7);
        work.push(branch, prep.max_depth);
        assert_eq!(work.len, WORKLIST_CAPACITY);
        assert!(work.pop().expect("continuation").inside);
    }
}

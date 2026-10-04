//! Exact CSG ray intersection against a stone: a convex polyhedron minus a
//! list of convex tool volumes.
//!
//! With no tools both entry points return exactly what
//! [`intersect_polyhedron`] / [`intersect_polyhedron_soa`] return, so a design
//! without concave facets is bit-identical to before. With tools the slab
//! interval `[a, b]` of the polyhedron is computed by the same loop (copied
//! here as `slab_interval`, because `intersect.rs` is deliberately untouched),
//! each tool contributes its analytic interval `[c, d]`, and the hit is the
//! first boundary of `[a, b] \ (union of [c, d])` past the `1e-4` floor.

use super::{
    camera::{HitRecord, Ray},
    intersect::{intersect_polyhedron, intersect_polyhedron_soa},
};
use crate::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{MAX_TOOL_PRIMITIVES, StoneGeometry, ToolPrimitive},
    },
    simd::{PlanesSoA32, SlabScan, slab_scan},
};
use glam::Vec3;

/// Hits at or before this parameter are the ray's own origin surface.
const T_FLOOR: f32 = 1e-4;

/// The polyhedron's slab interval with the facets that bound it, mirroring the
/// running state of `intersect_polyhedron`'s loop.
#[derive(Debug, Clone, Copy)]
struct Slab {
    t_near: f32,
    near: Option<usize>,
    t_far: f32,
    far: Option<usize>,
}

/// The slab loop of `intersect_polyhedron`, returning both ends and both
/// facets instead of choosing between them. `None` when the ray misses.
fn slab_interval(ray: Ray, planes: &[GpuFacetPlane]) -> Option<Slab> {
    let mut slab = Slab {
        t_near: -1e30,
        near: None,
        t_far: 1e30,
        far: None,
    };
    for (i, p) in planes.iter().enumerate() {
        let n = Vec3::from_array(p.normal);
        let denom = n.dot(ray.dir);
        let side = p.d + n.dot(ray.origin);
        let numer = -side;
        if denom.abs() > 1e-7 {
            let t = numer / denom;
            if denom < 0.0 {
                if t > slab.t_near {
                    slab.t_near = t;
                    slab.near = Some(i);
                }
            } else if t < slab.t_far {
                slab.t_far = t;
                slab.far = Some(i);
            }
        } else if side > 0.0 {
            return None;
        }
    }
    (slab.t_near <= slab.t_far).then_some(slab)
}

/// One boundary on the ray. `key` orders ties: plane entry `0`, plane exit
/// `1`, then tool `k` as `2 + 2k` (entry) and `3 + 2k` (exit), i.e. a plane
/// beats a tool and a lower tool index beats a higher one.
#[derive(Clone, Copy)]
struct Boundary {
    t: f32,
    key: u32,
}

/// Walks the sorted boundaries of `[a, b]` and every tool interval and returns
/// the first change of the "inside the stone" state past [`T_FLOOR`].
fn carve(
    ray: Ray,
    slab: Slab,
    plane_count: usize,
    tools: &[ToolPrimitive],
    plane_normal: impl Fn(usize) -> Vec3,
) -> Option<HitRecord> {
    debug_assert!(tools.len() <= MAX_TOOL_PRIMITIVES, "too many tools");
    // Excess tools are ignored rather than overflowing the stack array; the
    // validators reject such scenes before they get here.
    let tools = &tools[..tools.len().min(MAX_TOOL_PRIMITIVES)];

    let mut events = [Boundary { t: 0.0, key: 0 }; 2 * MAX_TOOL_PRIMITIVES + 2];
    events[0] = Boundary {
        t: slab.t_near,
        key: 0,
    };
    events[1] = Boundary {
        t: slab.t_far,
        key: 1,
    };
    let mut n = 2;
    for (k, tool) in tools.iter().enumerate() {
        if let Some((c, d)) = tool.ray_interval(ray) {
            let key = 2 + 2 * k as u32;
            events[n] = Boundary { t: c, key };
            events[n + 1] = Boundary { t: d, key: key + 1 };
            n += 2;
        }
    }
    let events = &mut events[..n];
    events.sort_unstable_by(|x, y| x.t.total_cmp(&y.t).then(x.key.cmp(&y.key)));

    let mut plane_depth = 0i32;
    let mut tool_depth = 0i32;
    let mut inside = false;
    for ev in events.iter() {
        match ev.key {
            0 => plane_depth += 1,
            1 => plane_depth -= 1,
            k if k % 2 == 0 => tool_depth += 1,
            _ => tool_depth -= 1,
        }
        let now_inside = plane_depth > 0 && tool_depth <= 0;
        if now_inside == inside {
            continue;
        }
        inside = now_inside;
        if ev.t <= T_FLOOR {
            continue;
        }
        return Some(match ev.key {
            0 | 1 => {
                let idx = if ev.key == 0 { slab.near } else { slab.far };
                HitRecord {
                    t: ev.t,
                    normal: idx.map_or(Vec3::ZERO, &plane_normal),
                    facet_idx: idx.unwrap_or(0),
                }
            }
            k => {
                let tool_idx = ((k - 2) / 2) as usize;
                let point = ray.origin + ray.dir * ev.t;
                HitRecord {
                    t: ev.t,
                    // The tool's outward normal points into the stone material;
                    // the stone's outward normal points into the cavity.
                    normal: -tools[tool_idx].outward_normal(point),
                    facet_idx: plane_count + tool_idx,
                }
            }
        });
    }
    None
}

/// Intersects a ray with the stone `geom` (its polyhedron minus every tool).
///
/// Returns the first surface the ray meets: the entry from outside, or the
/// exit/next surface for a ray that starts inside the material.
///
/// With no tools the result is `intersect_polyhedron`'s, bit for bit,
/// including the `t = 1e30` sentinel hit for an empty plane list. A tool hit
/// has `facet_idx == planes.len() + k`. A boundary is reported without saying
/// whether it is an entry or an exit, matching [`HitRecord`] today.
#[must_use]
pub fn intersect_stone(ray: Ray, geom: StoneGeometry<'_>) -> Option<HitRecord> {
    if geom.is_convex() {
        return intersect_polyhedron(ray, geom.planes);
    }
    let slab = slab_interval(ray, geom.planes)?;
    carve(ray, slab, geom.planes.len(), geom.tools, |i| {
        Vec3::from_array(geom.planes[i].normal)
    })
}

/// `SoA` twin used by transport: identical result to [`intersect_stone`].
///
/// Step 1 is the (bit-identical) SIMD `slab_scan`, so only the tool pass is
/// scalar; tools are few and a SIMD twin waits for a profile that asks for it.
pub(crate) fn intersect_stone_soa(
    ray: Ray,
    plane_soa: &PlanesSoA32,
    planes_len: usize,
    tools: &[ToolPrimitive],
) -> Option<HitRecord> {
    if tools.is_empty() {
        return intersect_polyhedron_soa(ray, plane_soa);
    }
    let SlabScan::Slab {
        t_near,
        near_idx,
        t_far,
        far_idx,
    } = slab_scan(plane_soa, ray.origin, ray.dir)
    else {
        return None;
    };
    if t_near > t_far {
        return None;
    }
    let slab = Slab {
        t_near,
        near: usize::try_from(near_idx).ok(),
        t_far,
        far: usize::try_from(far_idx).ok(),
    };
    carve(ray, slab, planes_len, tools, |i| plane_soa.normal(i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::StandardGemCuts;

    fn unit_cube_planes() -> Vec<GpuFacetPlane> {
        [
            (Vec3::X, -0.5),
            (Vec3::NEG_X, -0.5),
            (Vec3::Y, -0.5),
            (Vec3::NEG_Y, -0.5),
            (Vec3::Z, -0.5),
            (Vec3::NEG_Z, -0.5),
        ]
        .into_iter()
        .map(|(n, d)| GpuFacetPlane::new(n, d))
        .collect()
    }

    /// xorshift32, as elsewhere in this workspace.
    struct Rng(u32);

    impl Rng {
        fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }

        fn signed(&mut self) -> f32 {
            (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        }

        fn unit_vec(&mut self) -> Vec3 {
            loop {
                let v = Vec3::new(self.signed(), self.signed(), self.signed());
                let l = v.length();
                if (0.1..=1.0).contains(&l) {
                    return v / l;
                }
            }
        }
    }

    fn same_hit(a: Option<HitRecord>, b: Option<HitRecord>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.t.to_bits() == b.t.to_bits()
                    && a.facet_idx == b.facet_idx
                    && a.normal.x.to_bits() == b.normal.x.to_bits()
                    && a.normal.y.to_bits() == b.normal.y.to_bits()
                    && a.normal.z.to_bits() == b.normal.z.to_bits()
            }
            _ => false,
        }
    }

    #[test]
    fn intersect_stone_with_no_tools_is_bit_identical_to_intersect_polyhedron() {
        let cases: [Vec<GpuFacetPlane>; 3] = [
            unit_cube_planes(),
            StandardGemCuts::standard_round_brilliant(),
            Vec::new(),
        ];
        let mut rng = Rng(0xC0FF_EE11);
        for planes in &cases {
            let soa = crate::optics::raytracer::build_plane_soa(planes);
            for i in 0..2_000 {
                // Alternate outside origins and (for the closed cases) inside
                // ones so both the entry and the exit branch are covered.
                let radius = if i % 2 == 0 { 3.0 } else { 0.2 };
                let ray = Ray {
                    origin: rng.unit_vec() * radius,
                    dir: rng.unit_vec(),
                };
                let reference = intersect_polyhedron(ray, planes);
                let got = intersect_stone(ray, StoneGeometry::planes_only(planes));
                assert!(same_hit(reference, got), "{reference:?} vs {got:?}");
                let got_soa = intersect_stone_soa(ray, &soa, planes.len(), &[]);
                assert!(
                    same_hit(intersect_polyhedron_soa(ray, &soa), got_soa),
                    "soa {got_soa:?}"
                );
            }
        }
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: Vec3::NEG_Y,
        };
        let sentinel = intersect_stone(ray, StoneGeometry::planes_only(&[]))
            .expect("empty planes keep the 1e30 sentinel hit");
        assert!((sentinel.t - 1e30).abs() < 1.0);
    }

    #[test]
    fn grazing_restart_from_a_small_cylinder_does_not_self_intersect() {
        // A cylinder of radius 0.05 along Z, cut out of the unit cube. A ray
        // restarted on its surface heading outward must not report the surface
        // it stands on.
        let planes = unit_cube_planes();
        let tools = [ToolPrimitive::cylinder(Vec3::ZERO, Vec3::Z, 0.05, 2.0)];
        let geom = StoneGeometry {
            planes: &planes,
            tools: &tools,
        };
        let mut rng = Rng(0x1234_5678);
        for _ in 0..5_000 {
            let phi = (rng.signed() + 1.0) * std::f32::consts::PI;
            let radial = Vec3::new(phi.cos(), phi.sin(), 0.0);
            let origin = radial * 0.05 + Vec3::Z * (0.3 * rng.signed());
            // Outward, including grazing directions near the tangent plane.
            let tangent = Vec3::new(-radial.y, radial.x, 0.0);
            let lean = rng.signed().abs();
            let dir = (radial * (0.05 + lean) + tangent * rng.signed() + Vec3::Z * rng.signed())
                .normalize();
            if dir.dot(radial) <= 0.0 {
                continue;
            }
            let hit = intersect_stone(Ray { origin, dir }, geom).expect("leaves through the cube");
            assert!(
                hit.facet_idx < planes.len(),
                "restart re-hit the cylinder: {hit:?}"
            );
        }
    }

    #[test]
    fn stone_hit_normal_on_a_tool_points_into_the_cavity() {
        // A groove of radius 0.2 along Y through the cube's top, centred at
        // y-axis x = 0, z = 0.5 (half sunk). A ray coming straight down enters
        // the groove and meets the groove's far wall.
        let planes = unit_cube_planes();
        let tools = [ToolPrimitive::cylinder(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::Z,
            0.2,
            2.0,
        )];
        let geom = StoneGeometry {
            planes: &planes,
            tools: &tools,
        };
        for x in [-0.15f32, -0.05, 0.0, 0.1, 0.18] {
            let ray = Ray {
                origin: Vec3::new(x, 2.0, 0.1),
                dir: Vec3::NEG_Y,
            };
            let hit = intersect_stone(ray, geom).expect("hits the groove floor");
            assert_eq!(hit.facet_idx, planes.len(), "tool facet id");
            assert!(
                hit.normal.dot(ray.dir) < 0.0,
                "normal {:?} does not face the incoming ray",
                hit.normal
            );
            let expected_y = 0.5 - x.mul_add(-x, 0.2f32 * 0.2).sqrt();
            let point = ray.origin + ray.dir * hit.t;
            assert!((point.y - expected_y).abs() < 1e-4, "{point:?}");
        }
        // Outside the groove the ray still lands on the top plane (+Y, id 2).
        let ray = Ray {
            origin: Vec3::new(0.4, 2.0, 0.1),
            dir: Vec3::NEG_Y,
        };
        let hit = intersect_stone(ray, geom).expect("hits the top plane");
        assert_eq!(hit.facet_idx, 2);
    }

    #[test]
    fn soa_twin_matches_the_scalar_kernel_with_tools_on_seeded_rays() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let tools =
            [
                ToolPrimitive::ball(Vec3::new(0.2, 0.3, 0.1), 0.25),
                ToolPrimitive::frustum(Vec3::new(-0.3, 0.0, 0.2), Vec3::Y, 0.1, 0.2, 0.3)
                    .with_sweep(crate::geometry::tool::ToolSweep::AlongAxis, 0.1, Vec3::ZERO),
            ];
        let soa = crate::optics::raytracer::build_plane_soa(&planes);
        let geom = StoneGeometry {
            planes: &planes,
            tools: &tools,
        };
        let mut rng = Rng(0xDEAD_BEEF);
        for i in 0..3_000 {
            let radius = if i % 2 == 0 { 4.0 } else { 0.3 };
            let ray = Ray {
                origin: rng.unit_vec() * radius,
                dir: rng.unit_vec(),
            };
            let a = intersect_stone(ray, geom);
            let b = intersect_stone_soa(ray, &soa, planes.len(), &tools);
            assert!(same_hit(a, b), "{a:?} vs {b:?}");
        }
    }
}

//! Host-runnable check of the algorithm the megakernel's WGSL tool carve uses
//! (`shaders/transport_physics/10_concave_tools.wgsl` plus `intersect_ray_tools` in
//! `transport_bounce/05_intersect_and_edge_rounding.wgsl`) against the CPU reference
//! [`intersect_stone`](crate::optics::raytracer::intersect_stone).
//!
//! The GPU replaces the CPU's sorted-boundary walk with a sort-free scan (`ct_scan_*`,
//! `ct_carve_*`): per pass, find whether the current position is inside a tool and where
//! that stretch of tools ends, or where the next tool starts. This module is a line-for-line
//! Rust port of exactly those functions, fed with the CPU's own closed-form tool intervals
//! ([`ToolPrimitive::ray_interval`]), so what it pins is the scan algorithm and the
//! bounding-sphere reject, on any host. The piece geometry itself (intervals, signed
//! distances, normals) is a direct WGSL transliteration of the same CPU code and is checked
//! against the CPU on a real adapter by `renderer::gpu::tool_check` (`INDICATRIX_REQUIRE_GPU`).
//!
//! The carve also reports whether its hit is an entry into the material or an exit
//! (`CtCarve::entering` in WGSL, `HitInfo::side` out of `intersect_ray`), the twin of the
//! `Option<bool>` that `intersect_stone_soa_side` returns and `run_bounce` adopts as
//! `inside_gem` for a concave stone; the tests below pin it against the CPU too.
//!
//! Only an exact tie between two boundaries (identical `f32` ray parameters) may resolve
//! differently from the CPU's `(t, key)` ordering; random scenes never produce one, so the
//! tests demand exact agreement.

use super::*;
use crate::optics::raytracer::{
    HitRecord, build_plane_soa, intersect_stone, intersect_stone::intersect_stone_soa_side,
};

const T_FLOOR: f32 = 1e-4;
const BIG: f32 = 1e30;

/// xorshift32, as in the other seeded generators in this workspace.
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

    /// Uniform in `[-1, 1)`.
    fn signed(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// Uniform in `[lo, hi)`.
    #[expect(
        clippy::suboptimal_flops,
        reason = "the seeded rays and points are the scenario these twin tests were written \
                  against; a fused multiply-add would move the last bit of every one"
    )]
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * 0.5 * (self.signed() + 1.0)
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

// ---- port of `ct_scan_init` / `ct_scan_add` / `ct_carve_begin` / `ct_carve_step` ----

#[derive(Clone, Copy)]
struct Scan {
    covered: bool,
    cover_end: f32,
    cover_k: usize,
    next_c: f32,
    next_k: usize,
}

fn scan_init() -> Scan {
    Scan {
        covered: false,
        cover_end: -BIG,
        cover_k: 0,
        next_c: BIG,
        next_k: 0,
    }
}

fn scan_add(mut s: Scan, k: usize, iv: Option<(f32, f32)>, pos: f32) -> Scan {
    let Some((lo, hi)) = iv else {
        return s;
    };
    if lo <= pos && pos < hi {
        if !s.covered || hi > s.cover_end {
            s.cover_end = hi;
            s.cover_k = k;
        }
        s.covered = true;
    } else if lo > pos && lo < s.next_c {
        s.next_c = lo;
        s.next_k = k;
    }
    s
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Scan,
    Miss,
    HitNear,
    HitFar,
    HitTool,
}

#[derive(Clone, Copy)]
struct Carve {
    state: State,
    pos: f32,
    start_kind: u32,
    start_k: usize,
    hit_t: f32,
    hit_k: usize,
    /// `CtCarve::entering`: the hit changes the state to "inside the material".
    entering: bool,
}

fn carve_begin(a: f32, b: f32) -> Carve {
    let mut st = Carve {
        state: State::Scan,
        pos: T_FLOOR,
        start_kind: 0,
        start_k: 0,
        hit_t: 0.0,
        hit_k: 0,
        entering: false,
    };
    if st.pos < a {
        st.pos = a;
        st.start_kind = 1;
    } else if st.pos >= b {
        st.state = State::Miss;
    }
    st
}

fn carve_step(mut st: Carve, acc: Scan, b: f32) -> Carve {
    if acc.covered {
        st.pos = acc.cover_end;
        st.start_kind = 2;
        st.start_k = acc.cover_k;
        if st.pos >= b {
            st.state = State::Miss;
        }
        return st;
    }
    if st.start_kind == 1 {
        // Air to material at the near plane.
        st.state = State::HitNear;
        st.hit_t = st.pos;
        st.entering = true;
        return st;
    }
    if st.start_kind == 2 {
        st.state = State::HitTool;
        st.hit_t = st.pos;
        st.hit_k = st.start_k;
        // Cavity to material where the tool union ends.
        st.entering = true;
        return st;
    }
    if acc.next_c < b {
        st.state = State::HitTool;
        st.hit_t = acc.next_c;
        st.hit_k = acc.next_k;
        // Material to cavity at the next tool entry.
        st.entering = false;
        return st;
    }
    st.state = State::HitFar;
    st.hit_t = b;
    st.entering = false;
    st
}

// ---- port of `ct_reach` / `ct_ray_may_hit` / `ct_point_may_be_inside` ----

#[expect(
    clippy::suboptimal_flops,
    reason = "mirrors the WGSL twin (`ct_reach`) bit for bit: the same one fused product, the rest plain"
)]
fn reach(t: &ToolPrimitive) -> f32 {
    let rmax = t.origin[3].max(t.profile[0].max(t.profile[1]));
    let ext_a = t.axis[3] + t.profile[2];
    let ext_r = rmax + t.profile[2];
    ext_a.mul_add(ext_a, ext_r * ext_r).sqrt() * 1.001 + 1e-4
}

fn ray_may_hit(t: &ToolPrimitive, ray: Ray) -> bool {
    let r = reach(t);
    let origin = Vec3::new(t.origin[0], t.origin[1], t.origin[2]);
    let w = (ray.origin - origin).cross(ray.dir);
    w.dot(w) <= r * r * ray.dir.dot(ray.dir)
}

fn point_may_be_inside(t: &ToolPrimitive, p: Vec3) -> bool {
    let r = reach(t);
    let origin = Vec3::new(t.origin[0], t.origin[1], t.origin[2]);
    let q = p - origin;
    q.dot(q) <= r * r
}

/// `intersect_ray` (the slab loop) followed by `intersect_ray_tools`, as the megakernel
/// runs them, with each tool's closed-form interval standing in for `ct_tool_ray_interval`.
fn gpu_style_intersect(
    ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
) -> Option<HitRecord> {
    gpu_style_intersect_side(ray, planes, tools).map(|(hit, _)| hit)
}

/// [`gpu_style_intersect`] with the carve's entry/exit flag: `HIT_SIDE_ENTRY` is `true`,
/// `HIT_SIDE_EXIT` is `false`, a miss has no side (`HitInfo::side` stays `HIT_SIDE_NONE`).
fn gpu_style_intersect_side(
    ray: Ray,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
) -> Option<(HitRecord, bool)> {
    let (mut t_near, mut t_far) = (-1e30_f32, 1e30_f32);
    let (mut near_n, mut far_n) = (Vec3::ZERO, Vec3::ZERO);
    let (mut near_i, mut far_i) = (0_usize, 0_usize);
    for (i, p) in planes.iter().enumerate() {
        let n = Vec3::from_array(p.normal);
        let denom = n.dot(ray.dir);
        let side = p.d + n.dot(ray.origin);
        if denom.abs() > 1e-7 {
            let t = -side / denom;
            if denom < 0.0 {
                if t > t_near {
                    t_near = t;
                    near_n = n;
                    near_i = i;
                }
            } else if t < t_far {
                t_far = t;
                far_n = n;
                far_i = i;
            }
        } else if side > 0.0 {
            return None;
        }
    }
    if t_near > t_far {
        return None;
    }
    let mut st = carve_begin(t_near, t_far);
    for _scan in 0..=tools.len() {
        if st.state != State::Scan {
            break;
        }
        let mut acc = scan_init();
        for (k, tool) in tools.iter().enumerate() {
            if ray_may_hit(tool, ray) {
                acc = scan_add(acc, k, tool.ray_interval(ray), st.pos);
            }
        }
        st = carve_step(st, acc, t_far);
    }
    let hit = match st.state {
        State::HitNear => HitRecord {
            t: st.hit_t,
            normal: near_n,
            facet_idx: near_i,
        },
        State::HitFar => HitRecord {
            t: st.hit_t,
            normal: far_n,
            facet_idx: far_i,
        },
        State::HitTool => {
            let point = ray.origin + ray.dir * st.hit_t;
            HitRecord {
                t: st.hit_t,
                normal: -tools[st.hit_k].outward_normal(point),
                facet_idx: planes.len() + st.hit_k,
            }
        }
        State::Miss | State::Scan => return None,
    };
    Some((hit, st.entering))
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

fn cube_planes() -> Vec<GpuFacetPlane> {
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

/// A random tool of any kind and sweep, placed around the cube so some cut the faces, some
/// sit inside, some miss.
fn random_tool(rng: &mut Rng) -> ToolPrimitive {
    let centre = Vec3::new(
        rng.range(-0.6, 0.6),
        rng.range(-0.6, 0.6),
        rng.range(-0.6, 0.6),
    );
    let axis = rng.unit_vec();
    let base = match rng.next_u32() % 4 {
        0 => ToolPrimitive::ball(centre, rng.range(0.05, 0.4)),
        1 => ToolPrimitive::cylinder(centre, axis, rng.range(0.05, 0.3), rng.range(0.05, 0.5)),
        2 => ToolPrimitive::frustum(
            centre,
            axis,
            rng.range(0.02, 0.35),
            rng.range(0.02, 0.35),
            rng.range(0.05, 0.5),
        ),
        _ => ToolPrimitive::bicone(centre, axis, rng.range(0.05, 0.4), rng.range(0.05, 0.5)),
    };
    match rng.next_u32() % 3 {
        0 => base,
        1 => base.with_sweep(ToolSweep::AlongAxis, rng.range(0.02, 0.3), Vec3::ZERO),
        _ => {
            let dir = base.axis_vec().any_orthonormal_vector();
            base.with_sweep(ToolSweep::AcrossAxis, rng.range(0.02, 0.3), dir)
        }
    }
}

#[test]
fn the_scan_carve_matches_the_cpu_carve_on_random_stones_and_rays() {
    let planes = cube_planes();
    let soa = build_plane_soa(&planes);
    let mut rng = Rng(0x9E37_79B9);
    let mut tool_hits = 0_u32;
    let (mut entries, mut exits) = (0_u32, 0_u32);
    for scene in 0..400_usize {
        let count = 1 + scene % 6;
        let tools: Vec<ToolPrimitive> = (0..count).map(|_| random_tool(&mut rng)).collect();
        let geom = StoneGeometry {
            planes: &planes,
            tools: &tools,
        };
        for i in 0..200 {
            // Outside origins (entry from the camera side) and inside ones (a bounce).
            let radius = if i % 2 == 0 { 2.5 } else { 0.35 };
            let ray = Ray {
                origin: rng.unit_vec() * radius * rng.range(0.2, 1.0),
                dir: rng.unit_vec(),
            };
            let cpu = intersect_stone(ray, geom);
            let gpu_side = gpu_style_intersect_side(ray, &planes, &tools);
            let gpu = gpu_side.map(|(hit, _)| hit);
            assert!(
                same_hit(cpu, gpu),
                "scene {scene} ray {i}: CPU {cpu:?} vs scan {gpu:?}\nray {ray:?}\ntools {tools:?}"
            );
            // The side the bounce adopts: the scan's `entering` is the CPU carve's
            // `now_inside`, as `Some(entering)` (a stone with tools always reports one).
            let cpu_side = intersect_stone_soa_side(ray, &soa, planes.len(), &tools)
                .map(|(_, entering)| entering);
            assert_eq!(
                gpu_side.map(|(_, entering)| Some(entering)),
                cpu_side,
                "scene {scene} ray {i}: entry/exit of {cpu:?}\nray {ray:?}\ntools {tools:?}"
            );
            match cpu_side {
                Some(Some(true)) => entries += 1,
                Some(Some(false)) => exits += 1,
                _ => {}
            }
            if cpu.is_some_and(|h| h.facet_idx >= planes.len()) {
                tool_hits += 1;
            }
        }
    }
    assert!(
        tool_hits > 1_000,
        "the test must actually exercise tool hits ({tool_hits})"
    );
    assert!(
        entries > 1_000 && exits > 1_000,
        "the test must exercise both entries ({entries}) and exits ({exits})"
    );
}

#[test]
fn overlapping_tools_are_walked_to_the_end_of_their_union() {
    // Two ball tools overlapping along the ray, so a ray starting inside the first has to
    // hop to the second's exit; and a third, disjoint, behind them.
    let planes = cube_planes();
    let tools = [
        ToolPrimitive::ball(Vec3::new(-0.15, 0.0, 0.0), 0.2),
        ToolPrimitive::ball(Vec3::new(0.15, 0.0, 0.0), 0.2),
        ToolPrimitive::ball(Vec3::new(0.42, 0.0, 0.0), 0.05),
    ];
    let geom = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    for (origin_x, dir) in [
        (-0.9, Vec3::X),
        (-0.1, Vec3::X),
        (0.2, Vec3::X),
        (0.9, Vec3::NEG_X),
    ] {
        let ray = Ray {
            origin: Vec3::new(origin_x, 0.0, 0.0),
            dir,
        };
        let cpu = intersect_stone(ray, geom);
        let gpu = gpu_style_intersect(ray, &planes, &tools);
        assert!(
            same_hit(cpu, gpu),
            "origin {origin_x}: CPU {cpu:?} vs scan {gpu:?}"
        );
    }
}

#[test]
fn the_bounding_sphere_never_rejects_a_ray_or_point_the_tool_touches() {
    let mut rng = Rng(0x0BAD_F00D);
    let mut touched = 0_u32;
    for _ in 0..3_000 {
        let tool = random_tool(&mut rng);
        for _ in 0..40 {
            let ray = Ray {
                origin: rng.unit_vec() * rng.range(0.1, 2.0),
                dir: rng.unit_vec(),
            };
            if tool.ray_interval(ray).is_some() {
                touched += 1;
                assert!(ray_may_hit(&tool, ray), "ray rejected: {ray:?} {tool:?}");
            }
            let p = Vec3::new(rng.signed(), rng.signed(), rng.signed());
            if tool.contains(p) {
                assert!(
                    point_may_be_inside(&tool, p),
                    "point rejected: {p:?} {tool:?}"
                );
            }
        }
    }
    assert!(
        touched > 1_000,
        "the test must actually touch tools ({touched})"
    );
}

#[test]
fn a_ray_parallel_to_a_plane_and_outside_it_misses_exactly_as_on_the_cpu() {
    let planes = cube_planes();
    let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.2)];
    let ray = Ray {
        origin: Vec3::new(0.0, 0.8, 0.0),
        dir: Vec3::X,
    };
    let geom = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    assert!(same_hit(
        intersect_stone(ray, geom),
        gpu_style_intersect(ray, &planes, &tools)
    ));
}

/// The rim where a tool meets a facet leaves a wedge of material that the bounce loop's
/// `1e-4` restart can step across (the numerical seam behind the `inside_gem` assertion in
/// the CPU tracer). A ball centred on the unit cube's top face: a ray restarted in the
/// wedge, `5e-5` outside the ball and `1e-4` below the face, heading into it, is in the
/// cavity for its whole next stretch. The ball boundary it crossed is within `T_FLOOR`, so
/// the carve skips it; the hit that follows (the ball's far side, back into the material)
/// is an entry. The old toggle would still say "inside the material" there; the scan's
/// `entering` (which `bounce_extend` adopts as `inside_gem = side == HIT_SIDE_EXIT`) names
/// the right medium, exactly as the CPU does.
#[test]
fn a_restart_across_a_tool_rim_wedge_is_reported_as_an_entry_like_on_the_cpu() {
    let planes = cube_planes();
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.3)];
    let soa = build_plane_soa(&planes);

    // (ray, expected entry/exit, what it is)
    let cases = [
        (
            Ray {
                origin: Vec3::new(0.300_05, 0.499_9, 0.0),
                dir: Vec3::NEG_X,
            },
            true,
            "restart across the rim wedge: cavity to material at the ball's far side",
        ),
        (
            Ray {
                origin: Vec3::new(0.4, 0.4, 0.0),
                dir: Vec3::NEG_X,
            },
            false,
            "in the material, well clear of the rim: material to cavity",
        ),
        (
            Ray {
                origin: Vec3::new(0.0, 2.0, 0.0),
                dir: Vec3::NEG_Y,
            },
            true,
            "a camera ray into the dimple: the cavity floor is an entry",
        ),
        (
            Ray {
                origin: Vec3::new(2.0, 0.2, 0.1),
                dir: Vec3::NEG_X,
            },
            true,
            "a camera ray beside the dimple: the +X facet is an entry",
        ),
    ];
    for (ray, entering, what) in cases {
        let (hit, gpu_entering) =
            gpu_style_intersect_side(ray, &planes, &tools).unwrap_or_else(|| panic!("{what}"));
        let (cpu_hit, cpu_side) = intersect_stone_soa_side(ray, &soa, planes.len(), &tools)
            .unwrap_or_else(|| panic!("{what}: the CPU misses"));
        assert!(
            same_hit(Some(cpu_hit), Some(hit)),
            "{what}: {cpu_hit:?} vs {hit:?}"
        );
        assert_eq!(cpu_side, Some(entering), "{what}: CPU {cpu_hit:?}");
        assert_eq!(gpu_entering, entering, "{what}: scan {hit:?}");
    }

    // The wedge case in detail: the hit is the ball's far side, and it is an entry. A bounce
    // that toggled `inside_gem` blindly would still be "inside the material" for the stretch
    // that ends here; the carve says that stretch was a cavity.
    let wedge = cases[0].0;
    let (hit, entering) = gpu_style_intersect_side(wedge, &planes, &tools).expect("a hit");
    assert_eq!(hit.facet_idx, planes.len(), "the ball's far side: {hit:?}");
    assert!((hit.t - 0.600_05).abs() < 1e-4, "{hit:?}");
    assert!(entering, "{hit:?}");
}

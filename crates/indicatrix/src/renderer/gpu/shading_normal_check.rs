//! `shading_normal_near_edge` GPU
//! self-test.
//!
//! Driven by `shaders/shading_normal.wgsl`, a standalone Tier 2 kernel file with its
//! OWN `planes` binding (bound to the SAME real 57-facet Standard Round Brilliant
//! plane set `polyhedron_check` already uses), mirroring that module's own precedent
//! for a function whose only extra input beyond scalars/vectors is the polyhedron's
//! plane list itself.
//!
//! Every case's `(hit_point, hit_facet_idx, hit_normal)` triple comes from actually
//! calling the REAL CPU `optics::raytracer::intersect_polyhedron` against real rays
//! through the real geometry (never hand-picked synthetic points), crossed with a
//! sweep of `rounding_radius` values spanning "smaller than any real gap" through
//! "large enough to blend deep into most facets" -- so the case bank exercises the
//! full flat-interior / partial-blend / at-the-edge regime this function has.

use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::raytracer::{Ray, intersect_polyhedron, shading_normal_near_edge},
    renderer::gpu::{
        compute,
        ulp::{ulp_distance, within_tolerance},
    },
};
use glam::Vec3;

const SHADER_SRC: &str = include_str!("../shaders/shading_normal.wgsl");

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadingNormalCase {
    hit_point: [f32; 3],
    hit_facet_idx: u32,
    hit_normal: [f32; 3],
    rounding_radius: f32,
}

const _: () = assert!(size_of::<ShadingNormalCase>() == 32);

/// ULP budget for `shading_normal_near_edge`.
///
/// A per-plane loop over up to 57 planes (each a dot product and a subtraction), a
/// division, a `clamp`, a `fma`-based smoothstep, and two `normalize`s -- comparable
/// composite shape to `polyhedron_check::HIT_ULP_BUDGET` (512) for a similarly-sized
/// per-plane loop, with extra headroom for the additional normalize/blend arithmetic on
/// top.
pub const SHADING_NORMAL_ULP_BUDGET: u32 = 1024;
/// Absolute-difference floor, per `crate::renderer::gpu::ulp::within_tolerance`'s doc
/// comment.
///
/// A shading normal's individual x/y/z components legitimately cross zero near several
/// facet orientations, where ULP distance alone is a poor metric.
pub const SHADING_NORMAL_ABS_FLOOR: f32 = 1e-5;

/// 89 well-spaced directions on the unit sphere (a simple Fibonacci-sphere lattice).
///
/// Mirrors `polyhedron_check::fibonacci_sphere`'s identical construction (duplicated
/// here rather than shared, matching that module's own `pub(crate)`-free, self-contained
/// style -- this is plain deterministic case-generation data, not physics under test).
fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32) / ((n - 1).max(1) as f32);
            let radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let theta = golden_angle * i as f32;
            Vec3::new(theta.cos() * radius, y, theta.sin() * radius)
        })
        .collect()
}

/// Builds the case bank.
///
/// Every real `(hit_point, hit_facet_idx, hit_normal)` triple from a dense directional
/// ray sweep through the real round-brilliant geometry, crossed with a spread of
/// `rounding_radius` values (including `0.0`, the default-off case, and negative, to
/// pin the early-return path too).
#[must_use]
pub fn build_cases() -> Vec<ShadingNormalCase> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let directions = fibonacci_sphere(256);
    let radii = [0.0f32, -0.5, 0.001, 0.01, 0.05, 0.2, 1.0];

    let mut cases = Vec::new();
    for &d in &directions {
        let ray = Ray {
            origin: d * 5.0,
            dir: -d,
        };
        let Some(hit) = intersect_polyhedron(ray, &planes) else {
            continue;
        };
        let hit_point = ray.origin + hit.t * ray.dir;
        for &radius in &radii {
            cases.push(ShadingNormalCase {
                hit_point: hit_point.to_array(),
                hit_facet_idx: hit.facet_idx as u32,
                hit_normal: hit.normal.to_array(),
                rounding_radius: radius,
            });
        }
    }
    cases
}

fn cpu_shading_normal(planes: &[GpuFacetPlane], c: &ShadingNormalCase) -> [f32; 3] {
    shading_normal_near_edge(
        planes,
        Vec3::from_array(c.hit_point),
        c.hit_facet_idx as usize,
        Vec3::from_array(c.hit_normal),
        c.rounding_radius,
    )
    .to_array()
}

#[derive(Debug, Clone)]
pub struct ShadingNormalResult {
    pub total: usize,
    pub max_genuine_ulp: u32,
    pub max_raw_ulp: u32,
    pub over_budget_count: usize,
    pub exempted_near_zero: usize,
}

impl ShadingNormalResult {
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}

#[must_use]
pub fn run(ctx: &crate::renderer::gpu::GpuContext) -> ShadingNormalResult {
    let planes = StandardGemCuts::standard_round_brilliant();
    let cases = build_cases();
    let total = cases.len();

    let planes_buf = compute::upload(
        &ctx.device,
        "shading normal planes",
        &planes,
        wgpu::BufferUsages::STORAGE,
    );
    let cases_buf = compute::upload(
        &ctx.device,
        "shading normal cases",
        &cases,
        wgpu::BufferUsages::STORAGE,
    );
    let out_buf = compute::zeroed_buffer::<f32>(
        &ctx.device,
        "shading normal out",
        total * 3,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline =
        compute::create_compute_pipeline(&ctx.device, "shading_normal", SHADER_SRC, "main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "shading normal bind group",
        &pipeline,
        &[(0, &planes_buf), (1, &cases_buf), (2, &out_buf)],
    );
    let workgroups = (total as u32).div_ceil(64);
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (workgroups, 1, 1),
    );
    let gpu_out: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, &out_buf, total * 3);

    let mut max_genuine_ulp = 0u32;
    let mut max_raw_ulp = 0u32;
    let mut over_budget = 0usize;
    let mut exempted = 0usize;
    for (idx, case) in cases.iter().enumerate() {
        let cpu = cpu_shading_normal(&planes, case);
        for c_idx in 0..3 {
            let cpu_v = cpu[c_idx];
            let gpu_v = gpu_out[idx * 3 + c_idx];
            let ulp = ulp_distance(cpu_v, gpu_v);
            max_raw_ulp = max_raw_ulp.max(ulp);
            if within_tolerance(
                cpu_v,
                gpu_v,
                SHADING_NORMAL_ULP_BUDGET,
                SHADING_NORMAL_ABS_FLOOR,
            ) {
                if ulp > SHADING_NORMAL_ULP_BUDGET {
                    exempted += 1;
                }
                continue;
            }
            over_budget += 1;
            max_genuine_ulp = max_genuine_ulp.max(ulp);
        }
    }

    ShadingNormalResult {
        total,
        max_genuine_ulp,
        max_raw_ulp,
        over_budget_count: over_budget,
        exempted_near_zero: exempted,
    }
}

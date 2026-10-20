//! Concave-stone (tool) GPU self-tests: the megakernel's tool carve against the CPU
//! reference, per tool primitive.
//!
//! # Tier 2: the real kernel code, one query at a time
//!
//! `shaders/tool_probe.wgsl` is appended to the generated megakernel source, so the probe
//! calls the production `intersect_ray`, `scene_any_tool_contains` and
//! `shading_normal_for_hit` against the production bindings (`planes` holds the planes then
//! the packed tool records, `params.num_tools` their count) -- not a hand-copied twin. Its
//! answers are compared with [`intersect_stone`](crate::optics::raytracer::intersect_stone)
//! and [`ToolPrimitive::contains`] for every `ToolKind` x `ToolSweep` combination: cylinder,
//! cone, ball and disc (bicone), each as a plunge (no sweep) and with the along-axis and
//! across-axis reciprocating sweeps, plus one scene holding several tools at once.
//!
//! GPU and CPU run the same operations but the GPU may contract `a * b + c` into an FMA
//! (WGSL cannot forbid it), so values are compared with a relative tolerance, not bit for
//! bit. Rays that are knife-edge on the CPU itself (a 2e-5 rad nudge changes the facet or
//! `t`) are excluded and counted: for those the two implementations may legitimately
//! pick different sides of a tie.
//!
//! # Tier 3: a frame
//!
//! [`run_image_comparison`] renders a concave stone on the GPU
//! ([`GpuFrameRenderer::accumulate_with_tools`]) and on the CPU and checks that the GPU
//! reproduces the CPU's tool effect (cut minus flat) -- which also exercises the tool
//! shadow probe, the HG-scatter exit walk and the edge-rounding rule end to end.
//!
//! Skips (prints, passes) without an adapter; with `INDICATRIX_REQUIRE_GPU=1` that is a
//! failure instead, like every other GPU test in this crate.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::{
    geometry::{
        GpuFacetPlane,
        cuts::StandardGemCuts,
        tool::{StoneGeometry, ToolKind, ToolPrimitive, ToolSweep, pack_planes_and_tools},
    },
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            Camera, HitRecord, LightingPreset, Ray, build_plane_soa, intersect_stone,
            intersect_stone::intersect_stone_soa_side,
        },
    },
    renderer::{
        buffers::GpuTransportParams,
        cpu_frame::trace_pixels_interleaved_geom,
        gpu::{
            GpuContext, GpuFrameRenderer, compute,
            frame::{GpuFrameScene, SHADER_SRC},
        },
    },
};

/// The probe kernel, appended to [`SHADER_SRC`].
const PROBE_WGSL: &str = include_str!("../shaders/tool_probe.wgsl");

/// Relative tolerance on `t` (GPU FMA contraction and a different `sqrt`/division
/// implementation move it by a few ULP; a wrong carve moves it by geometry).
const T_REL_TOLERANCE: f32 = 1e-4;

/// Absolute tolerance per normal component.
const NORMAL_TOLERANCE: f32 = 2e-3;

/// Angle (radians) a ray is nudged by to decide whether the CPU result is stable.
const NUDGE: f32 = 2e-5;

/// Mismatch details kept per check, for a readable failure.
const KEPT_MISMATCHES: usize = 24;

/// Rounding radius used for the shading-normal queries.
const ROUNDING_RADIUS: f32 = 0.04;

/// One probe query, 48 bytes -- `ToolProbeCase` in `tool_probe.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeCase {
    origin: [f32; 4],
    dir: [f32; 4],
    mode: [u32; 4],
}

/// One probe answer, 32 bytes -- `ToolProbeOut` in `tool_probe.wgsl`. For an intersection,
/// `a = (hit, t, facet_idx, side)` with `side` the `HIT_SIDE_*` code of `HitInfo`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeOut {
    a: [f32; 4],
    b: [f32; 4],
}

impl ProbeCase {
    const fn intersect(ray: Ray) -> Self {
        Self {
            origin: [ray.origin.x, ray.origin.y, ray.origin.z, 0.0],
            dir: [ray.dir.x, ray.dir.y, ray.dir.z, 0.0],
            mode: [0; 4],
        }
    }

    const fn contains(p: Vec3) -> Self {
        Self {
            origin: [p.x, p.y, p.z, 0.0],
            dir: [0.0; 4],
            mode: [1, 0, 0, 0],
        }
    }

    fn shading(point: Vec3, facet: usize, normal: Vec3) -> Self {
        Self {
            origin: [point.x, point.y, point.z, ROUNDING_RADIUS],
            dir: [normal.x, normal.y, normal.z, 0.0],
            mode: [
                2,
                u32::try_from(facet).expect("a facet index fits u32"),
                0,
                0,
            ],
        }
    }
}

/// Outcome of the Tier 2 check over every scenario.
#[derive(Debug, Clone, Default)]
pub struct ToolCheckResult {
    /// Queries compared.
    pub compared: usize,
    /// Rays and points left out because the CPU result is knife-edge.
    pub excluded_knife_edge: usize,
    /// True number of disagreements (`mismatches` holds the first few).
    pub total_mismatches: usize,
    /// The first disagreements, one line each.
    pub mismatches: Vec<String>,
    /// Hits on a tool surface (facet id `>= planes`) among the compared rays: proof the
    /// scenarios exercise the tool path at all.
    pub tool_hits: usize,
}

impl ToolCheckResult {
    /// Whether every compared query agreed.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.total_mismatches == 0
    }

    fn note(&mut self, scenario: &str, what: &str) {
        self.total_mismatches += 1;
        if self.mismatches.len() < KEPT_MISMATCHES {
            self.mismatches.push(format!("[{scenario}] {what}"));
        }
    }
}

/// xorshift32, as in the other seeded generators in this workspace.
struct Rng(u32);

impl Rng {
    const fn next_u32(&mut self) -> u32 {
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
        reason = "test inputs only, but the seeded rays are part of the measured scenario \
                  (knife-edge counts, tool-hit counts); a fused multiply-add would move the \
                  last bit of every generated ray"
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

/// The probe pipeline: the megakernel source plus `tool_probe.wgsl`, compiled once.
struct ToolProbe<'a> {
    ctx: &'a GpuContext,
    pipeline: wgpu::ComputePipeline,
}

impl<'a> ToolProbe<'a> {
    fn new(ctx: &'a GpuContext) -> Self {
        let source = format!("{SHADER_SRC}\n{PROBE_WGSL}");
        let pipeline = compute::create_compute_pipeline(
            &ctx.device,
            "concave tool probe",
            &source,
            "tool_probe_main",
        );
        Self { ctx, pipeline }
    }

    /// Runs `cases` against the stone `planes` minus `tools`, packed exactly as the renderer
    /// packs them.
    fn run(
        &self,
        planes: &[GpuFacetPlane],
        tools: &[ToolPrimitive],
        cases: &[ProbeCase],
    ) -> Vec<ProbeOut> {
        if cases.is_empty() {
            return Vec::new();
        }
        let device = &self.ctx.device;
        let packed = pack_planes_and_tools(planes, tools);
        let params = GpuTransportParams::new(0, 1, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, [1.0; 3])
            .with_tool_count(u32::try_from(tools.len()).expect("a few tools"));
        let params_buf = compute::upload(
            device,
            "tool probe params",
            std::slice::from_ref(&params),
            wgpu::BufferUsages::UNIFORM,
        );
        let planes_buf = compute::upload(
            device,
            "tool probe planes + tools",
            &packed[..],
            wgpu::BufferUsages::STORAGE,
        );
        let cases_buf = compute::upload(
            device,
            "tool probe cases",
            cases,
            wgpu::BufferUsages::STORAGE,
        );
        let out_buf = compute::zeroed_buffer::<ProbeOut>(
            device,
            "tool probe output",
            cases.len(),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let bind_group = compute::bind_buffers(
            device,
            "tool probe bind group",
            &self.pipeline,
            &[
                (1, &params_buf),
                (3, &planes_buf),
                (90, &cases_buf),
                (91, &out_buf),
            ],
        );
        let workgroups = u32::try_from(cases.len())
            .expect("a few thousand cases")
            .div_ceil(64);
        compute::dispatch_and_wait(
            device,
            &self.ctx.queue,
            &self.pipeline,
            &bind_group,
            (workgroups, 1, 1),
        );
        compute::readback(device, &self.ctx.queue, &out_buf, cases.len())
    }
}

// ---- scenarios ----

/// The tool kinds with their stone-cutting names (a disc is a bicone: a wheel with a
/// bevelled rim).
const KINDS: [(&str, ToolKind); 4] = [
    ("cylinder", ToolKind::Cylinder),
    ("cone", ToolKind::Frustum),
    ("ball", ToolKind::Ball),
    ("disc", ToolKind::Bicone),
];

/// The sweeps: a plunge presses the tool in; the others reciprocate it.
const SWEEPS: [(&str, ToolSweep); 3] = [
    ("plunge", ToolSweep::None),
    ("along-axis", ToolSweep::AlongAxis),
    ("across-axis", ToolSweep::AcrossAxis),
];

/// One tool of the given kind and sweep, centred at `centre` near the crown of the round
/// brilliant, so it cuts the facets and leaves part of itself in air.
fn make_tool(kind: ToolKind, sweep: ToolSweep, centre: Vec3, axis: Vec3) -> ToolPrimitive {
    let base = match kind {
        ToolKind::Ball => ToolPrimitive::ball(centre, 0.25),
        ToolKind::Cylinder => ToolPrimitive::cylinder(centre, axis, 0.12, 0.3),
        ToolKind::Frustum => ToolPrimitive::frustum(centre, axis, 0.08, 0.2, 0.3),
        ToolKind::Bicone => ToolPrimitive::bicone(centre, axis, 0.25, 0.1),
    };
    match sweep {
        ToolSweep::None => base,
        ToolSweep::AlongAxis => base.with_sweep(sweep, 0.15, Vec3::ZERO),
        ToolSweep::AcrossAxis => {
            // Perpendicular to the tool's own axis (a ball's is +Z, whatever `axis` is).
            let own = Vec3::new(base.axis[0], base.axis[1], base.axis[2]);
            base.with_sweep(sweep, 0.2, own.any_orthonormal_vector())
        }
    }
}

/// Every scenario: each kind x sweep alone, then several mixed tools at once.
///
/// Also the scene table of the event-wavefront tool tests (`event_wavefront::tool_tests`).
#[expect(
    clippy::suboptimal_flops,
    reason = "scene table shared with the GPU probe and the event-wavefront tests: the tool \
              centres stay the unfused products the scenarios were written and measured with"
)]
pub(crate) fn scenarios() -> Vec<(String, Vec<ToolPrimitive>)> {
    let axis = Vec3::new(0.3, 0.9, 0.4).normalize();
    let mut out = Vec::new();
    for (kind_name, kind) in KINDS {
        for (sweep_name, sweep) in SWEEPS {
            let tool = make_tool(kind, sweep, Vec3::new(0.3, 0.2, 0.15), axis);
            debug_assert!(tool.validate().is_ok(), "{kind_name} {sweep_name}");
            out.push((format!("{kind_name} {sweep_name}"), vec![tool]));
        }
    }
    let mut mixed = Vec::new();
    for (i, (_, kind)) in KINDS.iter().enumerate() {
        for (j, (_, sweep)) in SWEEPS.iter().enumerate() {
            if (i + j) % 2 == 0 {
                let (fi, fj) = (i as f32, j as f32);
                let centre = Vec3::new(-0.45 + 0.28 * fi, 0.1 + 0.08 * fj, -0.3 + 0.3 * fj);
                mixed.push(make_tool(*kind, *sweep, centre, axis));
            }
        }
    }
    out.push(("mixed tools".to_string(), mixed));
    out
}

fn inside_planes(planes: &[GpuFacetPlane], p: Vec3) -> bool {
    planes
        .iter()
        .all(|pl| Vec3::from_array(pl.normal).dot(p) + pl.d <= 0.0)
}

fn any_tool_contains(tools: &[ToolPrimitive], p: Vec3) -> bool {
    tools.iter().any(|t| t.contains(p))
}

const fn tool_centre(tool: &ToolPrimitive) -> Vec3 {
    Vec3::new(tool.origin[0], tool.origin[1], tool.origin[2])
}

/// Rays from outside aimed around the tools, from inside the material, and from inside a
/// tool (within the polyhedron).
fn scenario_rays(rng: &mut Rng, planes: &[GpuFacetPlane], tools: &[ToolPrimitive]) -> Vec<Ray> {
    let mut rays = Vec::new();
    for i in 0..600_usize {
        let from = rng.unit_vec() * 4.0;
        let tool = &tools[i % tools.len()];
        let target = tool_centre(tool) + Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 0.45;
        rays.push(Ray {
            origin: from,
            dir: (target - from).normalize(),
        });
    }
    for _ in 0..1_500 {
        let p = Vec3::new(
            rng.range(-0.9, 0.9),
            rng.range(-0.7, 0.4),
            rng.range(-0.9, 0.9),
        );
        if rays.len() >= 1_100 {
            break;
        }
        if inside_planes(planes, p) && !any_tool_contains(tools, p) {
            rays.push(Ray {
                origin: p,
                dir: rng.unit_vec(),
            });
        }
    }
    for i in 0..800_usize {
        let tool = &tools[i % tools.len()];
        let p = tool_centre(tool) + rng.unit_vec() * rng.range(0.0, 0.3);
        if inside_planes(planes, p) && any_tool_contains(tools, p) {
            rays.push(Ray {
                origin: p,
                dir: rng.unit_vec(),
            });
        }
    }
    rays
}

/// Whether the CPU answer for `ray` survives a small nudge of its direction: same facet,
/// `t` within a relative `1e-3`. A knife-edge ray is one where GPU and CPU may legitimately
/// land on different sides of a tie.
fn cpu_hit_is_stable(ray: Ray, geom: StoneGeometry<'_>, base: Option<HitRecord>) -> bool {
    let dir = ray.dir.normalize();
    let across = dir.any_orthonormal_vector();
    let other = dir.cross(across);
    [across, -across, other, -other].into_iter().all(|side| {
        let nudged = Ray {
            origin: ray.origin,
            dir: (dir + side * NUDGE).normalize(),
        };
        match (base, intersect_stone(nudged, geom)) {
            (None, None) => true,
            (Some(before), Some(after)) => {
                before.facet_idx == after.facet_idx
                    && (before.t - after.t).abs() <= 1e-3 * (1.0 + before.t.abs())
            }
            _ => false,
        }
    })
}

/// Whether `contains` for `p` survives a `1e-4` nudge along each axis.
fn contains_is_stable(tools: &[ToolPrimitive], p: Vec3) -> bool {
    let base = any_tool_contains(tools, p);
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .into_iter()
    .all(|axis| any_tool_contains(tools, p + axis * 1e-4) == base)
}

/// `HitInfo::side` as the probe writes it, for the CPU's `Option<bool>` (`Some(true)` is an
/// entry into the material, `Some(false)` an exit): `HIT_SIDE_ENTRY` 1, `HIT_SIDE_EXIT` 2.
fn side_code(entering: Option<bool>) -> f32 {
    entering.map_or(0.0, |entering| if entering { 1.0 } else { 2.0 })
}

fn compare_hit(
    cpu: Option<HitRecord>,
    cpu_side: Option<bool>,
    gpu: &ProbeOut,
) -> Result<(), String> {
    let gpu_hit = gpu.a[0] > 0.5;
    match (cpu, gpu_hit) {
        (None, false) => Ok(()),
        (Some(c), false) => Err(format!(
            "CPU hit t={} facet {}, GPU missed",
            c.t, c.facet_idx
        )),
        (None, true) => Err(format!(
            "CPU missed, GPU hit t={} facet {}",
            gpu.a[1], gpu.a[2]
        )),
        (Some(c), true) => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the probe writes a small non-negative facet id as an exact float"
            )]
            let facet = gpu.a[2] as usize;
            let t = gpu.a[1];
            let normal = Vec3::new(gpu.b[0], gpu.b[1], gpu.b[2]);
            if facet != c.facet_idx {
                Err(format!(
                    "facet CPU {} vs GPU {facet} (t {} vs {t})",
                    c.facet_idx, c.t
                ))
            } else if (t - c.t).abs() > T_REL_TOLERANCE * (1.0 + c.t.abs()) {
                Err(format!("t CPU {} vs GPU {t} (facet {facet})", c.t))
            } else if (normal - c.normal).abs().max_element() > NORMAL_TOLERANCE {
                Err(format!(
                    "normal CPU {:?} vs GPU {normal:?} (facet {facet}, t {t})",
                    c.normal
                ))
            } else if (gpu.a[3] - side_code(cpu_side)).abs() > 0.5 {
                // The entry/exit `bounce_extend` adopts as `inside_gem`.
                Err(format!(
                    "side CPU {cpu_side:?} vs GPU code {} (facet {facet}, t {t})",
                    gpu.a[3]
                ))
            } else {
                Ok(())
            }
        }
    }
}

/// The tool-proximity test of `shading_normal_for_hit` (the four in-plane probes one radius
/// away), written with the CPU's `ToolPrimitive::contains`.
fn near_tool(tools: &[ToolPrimitive], point: Vec3, normal: Vec3) -> bool {
    let a = if normal.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t = (a - normal * normal.dot(a)).normalize_or_zero();
    let b = normal.cross(t);
    [t, -t, b, -b]
        .into_iter()
        .any(|dir| any_tool_contains(tools, point + dir * ROUNDING_RADIUS))
}

/// Runs one scenario's intersection, containment and shading-normal queries.
fn check_scenario(
    probe: &ToolProbe<'_>,
    name: &str,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    rng: &mut Rng,
    result: &mut ToolCheckResult,
) {
    let geom = StoneGeometry { planes, tools };
    let soa = build_plane_soa(planes);

    // Intersection.
    let mut rays = Vec::new();
    let mut expected = Vec::new();
    for ray in scenario_rays(rng, planes, tools) {
        let cpu = intersect_stone(ray, geom);
        if cpu_hit_is_stable(ray, geom, cpu) {
            rays.push(ray);
            // The side the CPU bounce loop adopts (`intersect_stone_soa_side`).
            let side = intersect_stone_soa_side(ray, &soa, planes.len(), tools)
                .and_then(|(_, entering)| entering);
            expected.push((cpu, side));
        } else {
            result.excluded_knife_edge += 1;
        }
    }
    let cases: Vec<ProbeCase> = rays.iter().copied().map(ProbeCase::intersect).collect();
    let outs = probe.run(planes, tools, &cases);
    for ((ray, (cpu, side)), out) in rays.iter().zip(&expected).zip(&outs) {
        result.compared += 1;
        if cpu.is_some_and(|h| h.facet_idx >= planes.len()) {
            result.tool_hits += 1;
        }
        if let Err(what) = compare_hit(*cpu, *side, out) {
            result.note(name, &format!("ray {ray:?}: {what}"));
        }
    }

    // Containment (`scene_any_tool_contains` vs `ToolPrimitive::contains`).
    let mut points = Vec::new();
    for i in 0..1_500_usize {
        let tool = &tools[i % tools.len()];
        let p = tool_centre(tool) + Vec3::new(rng.signed(), rng.signed(), rng.signed()) * 0.6;
        if contains_is_stable(tools, p) {
            points.push(p);
        } else {
            result.excluded_knife_edge += 1;
        }
    }
    let cases: Vec<ProbeCase> = points.iter().copied().map(ProbeCase::contains).collect();
    let outs = probe.run(planes, tools, &cases);
    for (p, out) in points.iter().zip(&outs) {
        result.compared += 1;
        let cpu = any_tool_contains(tools, *p);
        if (out.a[0] > 0.5) != cpu {
            result.note(
                name,
                &format!("contains {p:?}: CPU {cpu}, GPU {}", out.a[0]),
            );
        }
    }

    // Shading normal at the hits found above: a tool hit and a plane hit near a tool keep
    // the geometric normal; any other plane hit gets the planar edge rounding.
    let hits: Vec<(Vec3, usize, Vec3)> = rays
        .iter()
        .zip(&expected)
        .filter_map(|(ray, (cpu, _))| {
            cpu.map(|h| (ray.origin + ray.dir * h.t, h.facet_idx, h.normal))
        })
        .take(600)
        .collect();
    let cases: Vec<ProbeCase> = hits
        .iter()
        .map(|&(p, facet, n)| ProbeCase::shading(p, facet, n))
        .collect();
    let with_tools = probe.run(planes, tools, &cases);
    let planar = probe.run(planes, &[], &cases);
    for ((&(p, facet, n), got), plain) in hits.iter().zip(&with_tools).zip(&planar) {
        // A probe point within 1e-4 of a tool boundary may land on either side on the GPU.
        if facet < planes.len() && !tools_clear_of_probes(tools, p, n) {
            result.excluded_knife_edge += 1;
            continue;
        }
        result.compared += 1;
        let got = Vec3::new(got.b[0], got.b[1], got.b[2]);
        let plain = Vec3::new(plain.b[0], plain.b[1], plain.b[2]);
        let (expected, why) = if facet >= planes.len() || near_tool(tools, p, n) {
            (n, "the geometric normal")
        } else {
            (plain, "the planar edge rounding")
        };
        if (got - expected).abs().max_element() > 1e-5 {
            result.note(
                name,
                &format!("shading at {p:?} facet {facet}: GPU {got:?} != {why} {expected:?}"),
            );
        }
    }
}

/// Whether every shading probe point is clear (by `1e-3` in any axis nudge) of a tool
/// boundary, i.e. whether the tool-proximity decision is stable.
fn tools_clear_of_probes(tools: &[ToolPrimitive], point: Vec3, normal: Vec3) -> bool {
    let a = if normal.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let t = (a - normal * normal.dot(a)).normalize_or_zero();
    let b = normal.cross(t);
    [t, -t, b, -b]
        .into_iter()
        .all(|dir| contains_is_stable(tools, point + dir * ROUNDING_RADIUS))
}

/// Runs the Tier 2 check over every scenario against a live GPU.
///
/// # Panics
///
/// Panics on `wgpu` API misuse.
#[must_use]
pub fn run(ctx: &GpuContext) -> ToolCheckResult {
    let planes = StandardGemCuts::standard_round_brilliant();
    let probe = ToolProbe::new(ctx);
    let mut rng = Rng(0x7001_C0DE);
    let mut result = ToolCheckResult::default();
    for (name, tools) in scenarios() {
        check_scenario(&probe, &name, &planes, &tools, &mut rng, &mut result);
    }
    result
}

// ---- Tier 3 ----

/// Numbers from [`run_image_comparison`].
#[derive(Debug, Clone, Copy)]
pub struct ImageComparison {
    /// Mean Y of the planar stone, CPU.
    pub cpu_flat_mean: f64,
    /// Mean Y of the concave stone, CPU.
    pub cpu_cut_mean: f64,
    /// Mean Y of the planar stone, GPU.
    pub gpu_flat_mean: f64,
    /// Mean Y of the concave stone, GPU.
    pub gpu_cut_mean: f64,
}

impl ImageComparison {
    /// The tool effect (cut minus flat) on the CPU.
    #[must_use]
    pub fn cpu_effect(&self) -> f64 {
        self.cpu_cut_mean - self.cpu_flat_mean
    }

    /// The tool effect on the GPU.
    #[must_use]
    pub fn gpu_effect(&self) -> f64 {
        self.gpu_cut_mean - self.gpu_flat_mean
    }
}

fn mean_y(sums: &[Vec3], spp: u32) -> f64 {
    let total: f64 = sums.iter().map(|v| f64::from(v.y)).sum();
    total / (sums.len() as f64 * f64::from(spp))
}

/// The concave stone of the image check: a ball dimple in the table and a cylinder groove
/// across the crown.
#[must_use]
pub fn image_check_tools() -> Vec<ToolPrimitive> {
    vec![
        ToolPrimitive::ball(Vec3::new(0.25, 0.32, 0.1), 0.22),
        ToolPrimitive::cylinder(
            Vec3::new(-0.3, 0.2, -0.2),
            Vec3::new(1.0, 0.0, 0.2).normalize(),
            0.08,
            0.5,
        ),
    ]
}

/// Renders the round brilliant with and without [`image_check_tools`] on the GPU and the
/// CPU, same samples.
///
/// # Errors
///
/// Whatever the GPU renderer returns.
///
/// # Panics
///
/// Panics if the built-in Diamond material is missing.
pub fn run_image_comparison(
    renderer: &mut GpuFrameRenderer,
) -> Result<ImageComparison, crate::renderer::gpu::GpuFrameError> {
    const SIZE: u32 = 48;
    const SPP: u32 = 48;
    let planes = StandardGemCuts::standard_round_brilliant();
    let tools = image_check_tools();
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let scene = GpuFrameScene {
        camera: &camera,
        width: SIZE,
        height: SIZE,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::LightTent
            .studio(1.0, 0.4, 0.35)
            // As Lit, explicitly: this scene measures the TOOLS' effect on the light that
            // leaks through the pavilion, which must meet the Light tent's own dark ground.
            // A lit tray (the default backdrop since 2026-10-20) replaces that ground with a
            // grey close to the stone's own mean radiance and washes the effect out.
            .with_backdrop(0.0),
    };
    let soa = build_plane_soa(&planes);
    let cpu = |tools: &[ToolPrimitive]| {
        trace_pixels_interleaved_geom(&scene, tools, Fluorescence::none(), &soa, 0, 1, 0, SPP)
    };
    let pixels = (SIZE * SIZE) as usize;
    let mut gpu_flat = vec![Vec3::ZERO; pixels];
    let mut gpu_cut = vec![Vec3::ZERO; pixels];
    renderer.accumulate_with_tools(&scene, &[], 0, SPP, &mut gpu_flat)?;
    renderer.accumulate_with_tools(&scene, &tools, 0, SPP, &mut gpu_cut)?;
    Ok(ImageComparison {
        cpu_flat_mean: mean_y(&cpu(&[]), SPP),
        cpu_cut_mean: mean_y(&cpu(&tools), SPP),
        gpu_flat_mean: mean_y(&gpu_flat, SPP),
        gpu_cut_mean: mean_y(&gpu_cut, SPP),
    })
}

#[cfg(test)]
mod tests;

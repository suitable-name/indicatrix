//! The routing of a concave stone (tools within the GPU limit go to the GPU, beyond it stay
//! on the CPU) and the CPU fallback that carries them without an adapter.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset},
};

const W: u32 = 24;
const H: u32 = 24;

/// Accumulates two samples of the round brilliant, with or without a ball cut out of the
/// table, through `accumulate_frame_samples` and returns the radiance buffer.
fn accumulate(tools: &[ToolPrimitive], target: LocalComputeTarget) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(
        DEFAULT_POSE.yaw,
        DEFAULT_POSE.pitch,
        DEFAULT_POSE.distance,
        DEFAULT_FOV_DEG,
    );
    let frame = BackendFrame {
        width: W,
        height: H,
        yaw: DEFAULT_POSE.yaw,
        pitch: DEFAULT_POSE.pitch,
        distance: DEFAULT_POSE.distance,
        camera: &camera,
        planes: &planes,
        tools,
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        spp: 2,
        sample_offset: 0,
    };
    let pixels = (W * H) as usize;
    let mut accum = vec![Vec3::ZERO; pixels];
    let mut depth = vec![0.0; pixels];
    let mut normal = vec![Vec3::ZERO; pixels];
    let mut facet_id = vec![0; pixels];
    accumulate_frame_samples(
        &mut ViewportGpu::acquire(),
        &frame,
        &mut FrameOutputs {
            accum: &mut accum,
            depth: &mut depth,
            normal: &mut normal,
            facet_id: &mut facet_id,
        },
        &mut HybridPacing::new(),
        target,
    );
    accum
}

fn frame_with<'a>(
    camera: &'a Camera,
    planes: &'a [indicatrix::geometry::plane::GpuFacetPlane],
    material: &'a GemMaterial,
    tools: &'a [ToolPrimitive],
) -> BackendFrame<'a> {
    BackendFrame {
        width: 1,
        height: 1,
        yaw: 0.0,
        pitch: 0.0,
        distance: 5.0,
        camera,
        planes,
        tools,
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        material,
        max_bounces: 1,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
        spp: 1,
        sample_offset: 0,
    }
}

/// Replaces the old "a frame with tools is not convex and routes off the GPU" pin: the
/// GPU traces tools now, so a concave frame routes to it up to the tool limit and only
/// a stone beyond the limit stays on the CPU.
#[test]
fn a_concave_frame_routes_to_the_gpu_up_to_the_tool_limit() {
    use indicatrix::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let camera = Camera::new(0.0, 0.0, 5.0, DEFAULT_FOV_DEG);
    let ball = ToolPrimitive::ball(Vec3::new(0.0, 0.3, 0.0), 0.2);
    let within = vec![ball; GPU_MAX_TOOL_PRIMITIVES];
    let beyond = vec![ball; GPU_MAX_TOOL_PRIMITIVES + 1];

    assert!(frame_with(&camera, &planes, &material, &[]).routes_to_gpu());
    let concave = frame_with(&camera, &planes, &material, &within);
    assert!(!concave.stone().is_convex());
    assert!(concave.routes_to_gpu());
    assert!(!frame_with(&camera, &planes, &material, &beyond).routes_to_gpu());
}

/// A concave frame is traced WITH its tools on every compute target: the cut stone
/// differs from the flat one whatever backend runs it. The CPU tracer is the reference
/// (and the GPU's fallback), so with no adapter the hybrid and GPU-first targets come
/// out of it bit for bit; with an adapter they come out of the GPU kernels, which the
/// crate's own CPU-vs-GPU tool equivalence tests pin, so only "the tools were
/// applied" is asserted here.
#[test]
fn a_concave_frame_is_traced_with_its_tools_on_every_compute_target() {
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.35, 0.0), 0.3)];
    let flat = accumulate(&[], LocalComputeTarget::Cpu);
    let cut = accumulate(&tools, LocalComputeTarget::Cpu);
    assert_ne!(flat, cut, "the ball must change the picture");
    let has_adapter = ViewportGpu::acquire().gpu.adapter_label().is_some();
    for target in [LocalComputeTarget::CpuGpu, LocalComputeTarget::Gpu] {
        let traced = accumulate(&tools, target);
        assert_ne!(traced, flat, "{target:?} must apply the tools");
        if !has_adapter {
            assert_eq!(traced, cut, "{target:?}");
        }
    }
}

/// The kernels' tool limit is the CPU's own (`GPU_MAX_TOOL_PRIMITIVES == MAX_TOOL_PRIMITIVES`,
/// 128 since 2026-10-20), so no valid stone is beyond it; the routing of a longer list is
/// pinned by `a_concave_frame_routes_to_the_gpu_up_to_the_tool_limit`. A stone with the
/// full list is traced with its tools on every target: with no adapter the GPU targets
/// come out of the CPU tracer bit for bit, with one they come out of the kernels.
#[test]
fn a_frame_at_the_tool_limit_is_traced_with_its_tools_on_every_target() {
    use indicatrix::geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, MAX_TOOL_PRIMITIVES};
    assert_eq!(GPU_MAX_TOOL_PRIMITIVES, MAX_TOOL_PRIMITIVES);
    let tools: Vec<ToolPrimitive> = (0..GPU_MAX_TOOL_PRIMITIVES)
        .map(|i| {
            let t = i as f32 / 100.0;
            ToolPrimitive::ball(Vec3::new(0.0, 0.002_f32.mul_add(t, 0.3), 0.0), 0.05)
        })
        .collect();
    let flat = accumulate(&[], LocalComputeTarget::Cpu);
    let cut = accumulate(&tools, LocalComputeTarget::Cpu);
    assert_ne!(flat, cut, "the tools must change the picture");
    let has_adapter = ViewportGpu::acquire().gpu.adapter_label().is_some();
    for target in [LocalComputeTarget::CpuGpu, LocalComputeTarget::Gpu] {
        let traced = accumulate(&tools, target);
        assert_ne!(traced, flat, "{target:?} must apply the tools");
        if !has_adapter {
            assert_eq!(traced, cut, "{target:?}");
        }
    }
}

//! Hardware tests of the backend's concave-stone entry points ([`GpuBackend::try_accumulate_geom`]
//! and `GpuBatchItem::tools`). The per-primitive CPU-vs-GPU equivalence is in
//! `renderer::gpu::tool_check`; these cover what the backend adds: the tool-count limit, that
//! the tools reach the kernel, and that a batch traces a concave picture exactly as the single
//! path does. They skip without an adapter (`INDICATRIX_REQUIRE_GPU=1` makes that a failure),
//! like the rest of this module tree's hardware tests.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{GpuAccumulate, GpuBatchItem, GpuSceneRef, tests::acquire_or_skip};
use crate::{
    geometry::{
        cuts::StandardGemCuts,
        tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
    },
};

const SIZE: u32 = 16;

const fn scene<'a>(
    camera: &'a Camera,
    planes: &'a [crate::geometry::GpuFacetPlane],
    material: &'a GemMaterial,
) -> GpuSceneRef<'a> {
    GpuSceneRef {
        camera,
        width: SIZE,
        height: SIZE,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    }
}

const fn dimple() -> ToolPrimitive {
    ToolPrimitive::ball(Vec3::new(0.2, 0.32, 0.1), 0.25)
}

fn total(accum: &[Vec3]) -> Vec3 {
    accum.iter().copied().sum()
}

/// The tools reach the kernel: a dimple in the table changes the picture, and a stone with
/// more tools than the kernels trace is declined with `accum` untouched.
#[test]
fn the_backend_traces_tools_and_declines_a_stone_beyond_the_limit() {
    let Some(backend) =
        acquire_or_skip("the_backend_traces_tools_and_declines_a_stone_beyond_the_limit")
    else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let scene = scene(&camera, &planes, &material);
    let pixels = (SIZE * SIZE) as usize;

    let mut flat = vec![Vec3::ZERO; pixels];
    assert!(backend.try_accumulate_geom(&scene, &[], 0, 8, &mut flat));
    let mut cut = vec![Vec3::ZERO; pixels];
    assert!(backend.try_accumulate_geom(&scene, &[dimple()], 0, 8, &mut cut));
    assert_ne!(flat, cut, "the dimple must change the picture");
    assert!(total(&cut).length() > 0.0);

    let too_many = vec![dimple(); GPU_MAX_TOOL_PRIMITIVES + 1];
    let mut untouched = vec![Vec3::ZERO; pixels];
    assert!(!backend.try_accumulate_geom(&scene, &too_many, 0, 8, &mut untouched));
    assert!(untouched.iter().all(|p| *p == Vec3::ZERO));

    // The limit itself is accepted.
    let at_limit = vec![dimple(); GPU_MAX_TOOL_PRIMITIVES];
    let mut full = vec![Vec3::ZERO; pixels];
    assert!(backend.try_accumulate_geom(&scene, &at_limit, 0, 2, &mut full));
}

/// A concave picture inside a batch is bit-identical to the same picture traced alone, as
/// every other picture of a batch is (`a_batch_of_different_scenes_equals_each_scene_alone`).
#[test]
fn a_concave_picture_in_a_batch_equals_the_same_picture_alone() {
    let Some(backend) =
        acquire_or_skip("a_concave_picture_in_a_batch_equals_the_same_picture_alone")
    else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let scene = scene(&camera, &planes, &material);
    let pixels = (SIZE * SIZE) as usize;
    let tools = [dimple()];

    let mut alone = vec![Vec3::ZERO; pixels];
    assert!(backend.try_accumulate_geom(&scene, &tools, 16, 8, &mut alone));

    let mut flat_out = vec![Vec3::ZERO; pixels];
    let mut cut_out = vec![Vec3::ZERO; pixels];
    let mut items = [
        GpuBatchItem {
            scene: self::scene(&camera, &planes, &material),
            tools: &[],
            first_sample: 0,
            samples: 4,
            out: &mut flat_out,
        },
        GpuBatchItem {
            scene: self::scene(&camera, &planes, &material),
            tools: &tools,
            first_sample: 16,
            samples: 8,
            out: &mut cut_out,
        },
    ];
    let mut outcomes = Vec::new();
    backend.try_accumulate_batch_cancellable(&mut items, &AtomicBool::new(false), &mut |i, o| {
        outcomes.push((i, o));
    });
    assert_eq!(
        outcomes,
        vec![(0, GpuAccumulate::Done), (1, GpuAccumulate::Done)]
    );
    assert_eq!(
        cut_out, alone,
        "the batch must trace the tools like the single path"
    );
    assert_ne!(flat_out, cut_out);
}

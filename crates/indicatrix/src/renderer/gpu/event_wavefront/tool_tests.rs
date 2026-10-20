//! Concave stones (tool primitives) on the event-wavefront kernels.
//!
//! Host-runnable (they need the `gpu` feature to compile, not an adapter): the tool limit, the
//! binding sizes of the packed `planes` binding, and the text invariants that keep the tool
//! records out of every kernel's binding table. Hardware tests (skipped without an adapter,
//! failing instead with `INDICATRIX_REQUIRE_GPU=1`): per tool primitive (cylinder, cone, ball,
//! disc; plunge and both reciprocating sweeps; one mixed scene) the event-wavefront picture
//! equals the megakernel's under the §7.2 tolerance, and the CPU tracer's tool effect is
//! reproduced end to end. The megakernel itself is held to the CPU per primitive by
//! `renderer::gpu::tool_check`, so the two checks together tie every primitive to the CPU.

use glam::Vec3;

use super::{
    EventWavefrontRenderer, EwError, EwKernel, accumulate::check_tool_limit,
    chunk::scene_binding_bytes, compare::compare_frames,
};
use crate::{
    geometry::{
        GpuFacetPlane,
        cuts::StandardGemCuts,
        tool::{
            GPU_MAX_TOOL_PRIMITIVES, TOOL_RECORD_VECS, ToolPrimitive, pack_planes_and_tools,
            packed_entry_count,
        },
    },
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa},
    },
    renderer::{
        cpu_frame::trace_pixels_interleaved_geom,
        frame_scene::FrameScene,
        gpu::{
            GpuContext,
            merge_tests::renderer_or_skip,
            tool_check::{image_check_tools, scenarios},
        },
    },
};

const REQUIRE_GPU_ENV: &str = "INDICATRIX_REQUIRE_GPU";

fn ew_or_skip(test_name: &str) -> Option<EventWavefrontRenderer> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(EventWavefrontRenderer::new(ctx)),
        Err(error) => {
            assert!(
                !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v == "1"),
                "{test_name}: {REQUIRE_GPU_ENV}=1 but no usable GPU adapter: {error}"
            );
            println!("skipping {test_name}: no GPU adapter ({error})");
            None
        }
    }
}

fn ball() -> ToolPrimitive {
    ToolPrimitive::ball(Vec3::new(0.2, 0.32, 0.1), 0.25)
}

// ---- host-runnable ----

#[test]
fn the_tool_limit_is_the_megakernels_and_names_both_counts() {
    let at_limit = vec![ball(); GPU_MAX_TOOL_PRIMITIVES];
    assert!(check_tool_limit(&at_limit).is_ok());
    assert!(check_tool_limit(&[]).is_ok());

    let over = vec![ball(); GPU_MAX_TOOL_PRIMITIVES + 1];
    let error = check_tool_limit(&over).expect_err("one tool over the limit is refused");
    assert!(
        matches!(
            error,
            EwError::TooManyTools { tools, limit }
                if tools == GPU_MAX_TOOL_PRIMITIVES + 1 && limit == GPU_MAX_TOOL_PRIMITIVES
        ),
        "{error:?}"
    );
    let text = error.to_string();
    assert!(
        text.contains(&(GPU_MAX_TOOL_PRIMITIVES + 1).to_string()),
        "{text}"
    );
    assert!(
        text.contains(&GPU_MAX_TOOL_PRIMITIVES.to_string()),
        "{text}"
    );
}

/// The two unsized scene arrays are bound at the packed length (planes, then five entries per
/// tool): a stale or capacity-sized binding would make `arrayLength(&planes)` -- and so
/// `scene_plane_count()` -- wrong. A planar stone is bound exactly as before.
#[test]
fn the_planes_binding_is_sized_to_the_planes_and_the_tool_records() {
    let as_address =
        |bytes: usize| u64::try_from(bytes).expect("a buffer size fits a buffer address");
    let planes = StandardGemCuts::standard_round_brilliant();
    let (plane_bytes, finish_bytes) = scene_binding_bytes(planes.len());
    assert_eq!(plane_bytes, as_address(size_of_val(planes.as_slice())));
    assert_eq!(finish_bytes, as_address(planes.len() * size_of::<u32>()));

    for tools in [vec![], vec![ball()], vec![ball(); 7]] {
        let packed = pack_planes_and_tools(&planes, &tools);
        assert_eq!(packed.len(), packed_entry_count(planes.len(), tools.len()));
        let (bytes, finishes) = scene_binding_bytes(packed.len());
        assert_eq!(
            bytes,
            as_address(
                (planes.len() + TOOL_RECORD_VECS * tools.len()) * size_of::<GpuFacetPlane>()
            )
        );
        assert_eq!(finishes, as_address(packed.len() * size_of::<u32>()));
    }
}

/// The tools ride in the existing `planes` binding, so no kernel gained a slot: the union of
/// the host tables is exactly the slots the kernels had before (0 to 14 and the four ray
/// buffers), and `planes` (slot 3) is bound by every kernel that traces a ray against the stone.
#[test]
fn no_kernel_binds_anything_for_the_tools() {
    use super::chunk::kernel_slots as slots;

    let mut union: Vec<u32> = EwKernel::ALL
        .into_iter()
        .flat_map(|kernel| slots(kernel).iter().copied())
        .collect();
    union.sort_unstable();
    union.dedup();
    let known: Vec<u32> = (0..=14).chain(40..=43).collect();
    assert_eq!(union, known, "a slot was added to a kernel's bind table");

    for kernel in [
        EwKernel::Extend,
        EwKernel::Scatter,
        EwKernel::NeeHg,
        EwKernel::Surface,
        EwKernel::ArmFrosted,
        EwKernel::SplitResolve,
    ] {
        assert!(
            slots(kernel).contains(&3),
            "{kernel:?} reads the stone, so it needs the planes binding the tools ride in"
        );
    }
}

/// In every generated event-wavefront unit the plane count comes from `scene_plane_count()`:
/// the one `arrayLength(&planes)` left (outside comments) is the definition itself, which
/// subtracts the tool records. A second use would count the records as planes.
#[test]
fn every_event_kernel_counts_planes_through_scene_plane_count() {
    for kernel in EwKernel::ALL {
        let code: String = kernel
            .source()
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            code.matches("arrayLength(&planes)").count(),
            1,
            "{kernel:?}: only scene_plane_count() may take the length of the planes binding"
        );
        assert!(
            code.contains("arrayLength(&planes) - CT_RECORD_VECS * params.num_tools"),
            "{kernel:?}: scene_plane_count() must leave the tool records out"
        );
    }
}

// ---- hardware ----

const SIZE: u32 = 32;

fn render_scene<'a>(
    camera: &'a Camera,
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
) -> FrameScene<'a> {
    FrameScene {
        camera,
        width: SIZE,
        height: SIZE,
        planes,
        facet_finishes: &[],
        material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    }
}

fn diamond() -> GemMaterial {
    GemMaterial::by_name("Diamond").expect("Diamond is a built-in material")
}

/// Per tool primitive and sweep, and for a mixed scene, the event-wavefront picture of the
/// concave stone equals the megakernel's (same samples, §7.2 tolerance), and the tools
/// really change it (the flat stone's picture differs).
#[test]
fn ew_traces_every_tool_primitive_like_the_megakernel() {
    let name = "ew_traces_every_tool_primitive_like_the_megakernel";
    let Some(mut ew) = ew_or_skip(name) else {
        return;
    };
    let Some(mut mega) = renderer_or_skip(name) else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = diamond();
    let scene = render_scene(&camera, &planes, &material);
    let pixels = (SIZE * SIZE) as usize;
    let spp = 4;

    let mut flat = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &[], 0, spp, &mut flat))
        .expect("the flat stone");

    let mut failures = Vec::new();
    let mut unchanged = Vec::new();
    for (scenario, tools) in scenarios() {
        let mut from_ew = vec![Vec3::ZERO; pixels];
        pollster::block_on(ew.accumulate_with_tools(&scene, &tools, 0, spp, &mut from_ew))
            .unwrap_or_else(|e| panic!("[{scenario}] event wavefront: {e}"));
        let mut from_mega = vec![Vec3::ZERO; pixels];
        mega.accumulate_with_tools(&scene, &tools, 0, spp, &mut from_mega)
            .unwrap_or_else(|e| panic!("[{scenario}] megakernel: {e:?}"));

        let diff = compare_frames(&from_mega, &from_ew, spp);
        if !diff.tol_passed(pixels) {
            failures.push(format!("[{scenario}] {diff:?}"));
        }
        if from_ew == flat {
            unchanged.push(scenario);
        }
    }
    assert!(
        failures.is_empty(),
        "EW differs from the megakernel: {failures:#?}"
    );
    assert!(
        unchanged.is_empty(),
        "the tools never reached the event kernels (picture equals the flat stone): {unchanged:?}"
    );
}

fn mean_y(sums: &[Vec3], spp: u32) -> f64 {
    let total: f64 = sums.iter().map(|v| f64::from(v.y)).sum();
    total / (sums.len() as f64 * f64::from(spp))
}

/// The CPU tracer's concave tool effect (cut minus flat) is reproduced by the event kernels,
/// and the total brightness agrees: the end-to-end check of the intersection, the tool
/// normals, the frosted shadow probe, the Henyey-Greenstein exit walk and the edge-rounding
/// rule on the event path.
#[test]
fn ew_concave_image_reproduces_the_cpu_tool_effect() {
    const IMAGE: u32 = 48;
    const SPP: u32 = 48;
    let Some(mut ew) = ew_or_skip("ew_concave_image_reproduces_the_cpu_tool_effect") else {
        return;
    };
    let planes = StandardGemCuts::standard_round_brilliant();
    let tools = image_check_tools();
    let material = diamond();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let scene = FrameScene {
        camera: &camera,
        width: IMAGE,
        height: IMAGE,
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
    let pixels = (IMAGE * IMAGE) as usize;
    let mut ew_flat = vec![Vec3::ZERO; pixels];
    let mut ew_cut = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &[], 0, SPP, &mut ew_flat))
        .expect("the flat stone");
    pollster::block_on(ew.accumulate_with_tools(&scene, &tools, 0, SPP, &mut ew_cut))
        .expect("the concave stone");

    let (cpu_flat, cpu_cut) = (mean_y(&cpu(&[]), SPP), mean_y(&cpu(&tools), SPP));
    let (ew_flat, ew_cut) = (mean_y(&ew_flat, SPP), mean_y(&ew_cut, SPP));
    println!("cpu flat {cpu_flat} cut {cpu_cut}; event flat {ew_flat} cut {ew_cut}");
    let effect = cpu_cut - cpu_flat;
    assert!(
        effect.abs() > 0.01 * cpu_flat,
        "test premise: the tools must visibly change the CPU image"
    );
    assert!(
        ((ew_cut - ew_flat) - effect).abs() <= 0.25 * effect.abs(),
        "event-wavefront tool effect {} vs CPU {effect}",
        ew_cut - ew_flat
    );
    assert!(
        (ew_cut - cpu_cut).abs() <= 0.03 * cpu_cut,
        "event-wavefront cut mean {ew_cut} vs CPU {cpu_cut}"
    );
}

/// A stone beyond the tool limit is refused before any GPU work and leaves `accum` alone; the
/// limit itself is traced; empty tools are exactly the planar entry point (bit for bit).
#[test]
fn ew_declines_beyond_the_limit_and_empty_tools_are_the_planar_path() {
    let Some(mut ew) =
        ew_or_skip("ew_declines_beyond_the_limit_and_empty_tools_are_the_planar_path")
    else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = diamond();
    let scene = render_scene(&camera, &planes, &material);
    let pixels = (SIZE * SIZE) as usize;

    let over = vec![ball(); GPU_MAX_TOOL_PRIMITIVES + 1];
    let mut untouched = vec![Vec3::ZERO; pixels];
    let error = pollster::block_on(ew.accumulate_with_tools(&scene, &over, 0, 2, &mut untouched))
        .expect_err("one tool over the limit is refused");
    assert!(matches!(error, EwError::TooManyTools { .. }), "{error:?}");
    assert!(untouched.iter().all(|p| *p == Vec3::ZERO));

    let at_limit = vec![ball(); GPU_MAX_TOOL_PRIMITIVES];
    let mut full = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &at_limit, 0, 2, &mut full))
        .expect("the limit itself is traced");
    assert!(full.iter().all(|p| p.is_finite()));

    let mut planar = vec![Vec3::ZERO; pixels];
    let mut empty = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate(&scene, 0, 2, &mut planar)).expect("planar");
    pollster::block_on(ew.accumulate_with_tools(&scene, &[], 0, 2, &mut empty))
        .expect("empty tools");
    assert_eq!(planar, empty);
}

/// Switching a renderer between a concave and a planar stone (the packed binding shrinks and
/// grows) never leaves the old tool records visible to the planar scene.
#[test]
fn ew_planar_after_concave_is_the_planar_picture_again() {
    let Some(mut ew) = ew_or_skip("ew_planar_after_concave_is_the_planar_picture_again") else {
        return;
    };
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = diamond();
    let scene = render_scene(&camera, &planes, &material);
    let pixels = (SIZE * SIZE) as usize;

    let mut before = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &[], 0, 2, &mut before)).expect("planar");
    let mut concave = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &image_check_tools(), 0, 2, &mut concave))
        .expect("concave");
    let mut after = vec![Vec3::ZERO; pixels];
    pollster::block_on(ew.accumulate_with_tools(&scene, &[], 0, 2, &mut after)).expect("planar");
    assert_ne!(before, concave);
    assert_eq!(
        before, after,
        "the planar picture must not depend on an earlier concave scene"
    );
}

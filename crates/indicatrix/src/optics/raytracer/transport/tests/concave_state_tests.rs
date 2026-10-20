//! The bounce loop's inside/outside state on a concave stone.
//!
//! `inside_gem` is toggled at every crossing, and the loop restarts a ray `1e-4` past the
//! surface it crossed. Where a tool meets a facet or another tool (a rim, a crease) a
//! second surface can lie within that `1e-4`: the restart then lands in the other medium
//! and the toggle names the wrong one for the whole next stretch. The debug check in
//! `inner::debug_assert_inside_gem_matches_geometry` caught this on a brilliant with a ball
//! dimple on the table and a cylinder tunnel through the crown (about one path in 10^5 hits
//! the rim), which is the stone below. The loop now takes the state from the intersection
//! (`intersect_stone_soa_side`), so the check holds for every path.

use crate::{
    geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, Camera, LightingPreset, build_plane_soa},
    },
    renderer::{cpu_frame::trace_pixels_interleaved_geom, frame_scene::FrameScene},
};
use glam::Vec3;

const SIZE: u32 = 48;
const SPP: u32 = 48;

/// A ball dimple centred on the table plane (so its rim is a circle on the table) and a
/// cylinder tunnel that opens into the dimple and leaves the stone through the crown.
fn dimple_and_tunnel() -> [ToolPrimitive; 2] {
    [
        ToolPrimitive::ball(Vec3::new(0.25, 0.32, 0.1), 0.22),
        ToolPrimitive::cylinder(
            Vec3::new(-0.3, 0.2, -0.2),
            Vec3::new(1.0, 0.0, 0.2).normalize(),
            0.08,
            0.5,
        ),
    ]
}

/// 48 x 48 pixels at 48 samples each, the workload of the GPU image checks' CPU reference.
/// In a debug build every bounce checks the loop's state against a point classification;
/// before the loop adopted the intersection's state this panicked ("`inside_gem` = true
/// disagrees with the point classification false") on a path whose reflection off the
/// table landed inside the dimple.
#[test]
fn a_dimple_and_tunnel_trace_without_losing_track_of_the_medium() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let soa = build_plane_soa(&planes);
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let scene = FrameScene {
        camera: &camera,
        width: SIZE,
        height: SIZE,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::LightTent
            .studio(1.0, 0.4, 0.35)
            .with_backdrop(BACKDROP_GREY),
    };
    let tools = dimple_and_tunnel();
    let cut =
        trace_pixels_interleaved_geom(&scene, &tools, Fluorescence::none(), &soa, 0, 1, 0, SPP);
    let flat = trace_pixels_interleaved_geom(&scene, &[], Fluorescence::none(), &soa, 0, 1, 0, SPP);

    assert_eq!(cut.len(), (SIZE * SIZE) as usize);
    assert!(cut.iter().all(|p| p.is_finite()), "a non-finite pixel");
    // The test premise: the tools change the picture.
    assert_ne!(
        cut.iter().map(|p| p.y).sum::<f32>().to_bits(),
        flat.iter().map(|p| p.y).sum::<f32>().to_bits(),
        "the dimple and tunnel must show in the picture"
    );
}

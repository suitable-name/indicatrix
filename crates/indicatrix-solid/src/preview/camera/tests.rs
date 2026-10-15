//! The "Fit" pose against real stones: every vertex of the mesh, projected the way the
//! rasterizer projects it at the fitted distance, lands inside the image with a margin,
//! for wide, square and tall images and from any orbit angle.

use super::{
    RESET_POSE, fit_distance_for_radius, fit_distance_for_view, orbit_distance_bounds,
    planar_bounding_radius, standard_view,
};
use crate::{
    mesh_cache::MeshCache,
    preview::{StoneGeometryBuf, view::with_min_aspect},
    raster::project_point,
};
use glam::{DVec3, Vec3};
use indicatrix::{
    geometry::{
        ToolPrimitive,
        cuts::StandardGemCuts,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::raytracer::{Camera, DEFAULT_FOV_DEG},
};

/// Image height in pixels; the width follows from the aspect.
const HEIGHT: u32 = 600;
/// Wide (the Live view's 16:9 render), square and tall (a narrow docked viewport).
const ASPECTS: [f32; 3] = [16.0 / 9.0, 1.0, 0.5];
/// Orbit angles, including views from below and straight down.
const YAWS: [f32; 4] = [0.0, 0.6, 1.3, 2.5];
const PITCHES: [f32; 4] = [-1.2, 0.0, 0.45, 1.5];

fn mesh_positions(stone: &StoneGeometryBuf) -> Vec<DVec3> {
    let planes: Vec<_> = stone
        .planes
        .iter()
        .map(|plane| plane.to_halfspace_f64())
        .collect();
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh.positions,
        other => panic!("the fixture must close: {other:?}"),
    }
}

fn image_size(aspect: f32) -> (u32, u32) {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a test image width from a small positive aspect"
    )]
    let width = (HEIGHT as f32 * aspect).round() as u32;
    (width, HEIGHT)
}

/// The projected bounding box `(min_x, max_x, min_y, max_y)` of `positions`, panicking
/// when a vertex is behind the camera.
fn projected_box(positions: &[DVec3], camera: &Camera, size: (u32, u32)) -> (f32, f32, f32, f32) {
    positions.iter().fold(
        (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
        |(min_x, max_x, min_y, max_y), position| {
            let (x, y, _) = project_point(camera, position.as_vec3(), size.0, size.1)
                .expect("a vertex of the fitted stone is in front of the camera");
            (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
        },
    )
}

fn stones() -> [(&'static str, StoneGeometryBuf); 2] {
    [
        (
            "round brilliant",
            StoneGeometryBuf::planes_only(StandardGemCuts::standard_round_brilliant()),
        ),
        (
            "emerald cut",
            StoneGeometryBuf::planes_only(StandardGemCuts::emerald_cut()),
        ),
    ]
}

/// The whole stone is inside the image, at least 5 % of the shorter side away from
/// every edge, for every stone, aspect and orbit angle.
#[test]
fn the_fitted_stone_is_inside_the_image_from_every_angle() {
    for (name, stone) in stones() {
        let positions = mesh_positions(&stone);
        let radius = planar_bounding_radius(&stone).expect("the fixture closes");
        for aspect in ASPECTS {
            let size = image_size(aspect);
            let inset = 0.05 * size.0.min(size.1) as f32;
            let distance = fit_distance_for_view(radius, aspect);
            for yaw in YAWS {
                for pitch in PITCHES {
                    let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
                    let (min_x, max_x, min_y, max_y) = projected_box(&positions, &camera, size);
                    let (width, height) = (size.0 as f32, size.1 as f32);
                    assert!(
                        min_x >= inset
                            && max_x <= width - inset
                            && min_y >= inset
                            && max_y <= height - inset,
                        "{name} at aspect {aspect}, yaw {yaw}, pitch {pitch}: x {min_x}..{max_x} \
                         y {min_y}..{max_y} leaves the {width}x{height} image (inset {inset})"
                    );
                }
            }
        }
    }
}

/// The margin is small: a round stone, whose outline is close to its bounding sphere's
/// from any angle, spans at least 70 % of the image's shorter side.
#[test]
fn the_fitted_round_stone_fills_most_of_the_shorter_side() {
    let stone = StoneGeometryBuf::planes_only(StandardGemCuts::standard_round_brilliant());
    let positions = mesh_positions(&stone);
    let radius = planar_bounding_radius(&stone).expect("the fixture closes");
    for aspect in ASPECTS {
        let size = image_size(aspect);
        let short = size.0.min(size.1) as f32;
        let distance = fit_distance_for_view(radius, aspect);
        for yaw in YAWS {
            for pitch in PITCHES {
                let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
                let (min_x, max_x, min_y, max_y) = projected_box(&positions, &camera, size);
                let fill = (max_x - min_x).max(max_y - min_y) / short;
                assert!(
                    fill >= 0.7,
                    "aspect {aspect}, yaw {yaw}, pitch {pitch}: the stone spans only {fill} of \
                     the shorter side"
                );
            }
        }
    }
}

/// The web and mobile Fit ([`standard_view`] kind 5) frames like the desktop's on a
/// picture at least as wide as tall, and those apps never draw a tall one: their Solid
/// raster (and mobile's Render trace) is widened to at least `SOLID_MIN_ASPECT` (1.6 in
/// both apps) by [`with_min_aspect`] and letterboxed in a tall view.
#[test]
fn the_web_and_mobile_fit_is_inside_their_min_aspect_raster() {
    const SOLID_MIN_ASPECT: f32 = 1.6;
    for (name, stone) in stones() {
        let positions = mesh_positions(&stone);
        let radius = planar_bounding_radius(&stone).expect("the fixture closes");
        let distance = standard_view(5, RESET_POSE, radius).distance;
        assert!(
            (distance - fit_distance_for_view(radius, SOLID_MIN_ASPECT)).abs() < 1e-6,
            "{name}: the shared Fit is the desktop's for a wide picture"
        );
        // A phone held upright, a square view and a wide one.
        for view in [(400, 800), (600, 600), (1200, 600)] {
            let size = with_min_aspect(view, SOLID_MIN_ASPECT, 1);
            let inset = 0.05 * size.0.min(size.1) as f32;
            for yaw in YAWS {
                for pitch in PITCHES {
                    let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
                    let (min_x, max_x, min_y, max_y) = projected_box(&positions, &camera, size);
                    let (width, height) = (size.0 as f32, size.1 as f32);
                    assert!(
                        min_x >= inset
                            && max_x <= width - inset
                            && min_y >= inset
                            && max_y <= height - inset,
                        "{name} in view {view:?} (raster {size:?}), yaw {yaw}, pitch {pitch}: \
                         x {min_x}..{max_x} y {min_y}..{max_y}"
                    );
                }
            }
        }
    }
}

/// Why the desktop Fit pose no longer uses [`fit_distance_for_radius`]: it fits the
/// vertical field of view only, so in a tall image the stone runs off the sides (the
/// desktop draws its viewports at their own aspect, with no minimum).
#[test]
fn the_vertical_only_fit_crops_a_tall_image() {
    let stone = StoneGeometryBuf::planes_only(StandardGemCuts::standard_round_brilliant());
    let positions = mesh_positions(&stone);
    let radius = planar_bounding_radius(&stone).expect("the fixture closes");
    let size = image_size(0.5);
    let camera = Camera::new(0.0, 0.0, fit_distance_for_radius(radius), DEFAULT_FOV_DEG);
    let (min_x, max_x, _, _) = projected_box(&positions, &camera, size);
    assert!(
        min_x < 0.0 && max_x > size.0 as f32,
        "x {min_x}..{max_x} should overflow the {} px wide image",
        size.0
    );
}

/// A very tall image needs a distance past the orbit clamp's far end; the caller keeps
/// the fitted distance there rather than clamping the stone back out of the frame.
#[test]
fn a_very_tall_image_fits_beyond_the_orbit_clamp() {
    let (_, far) = orbit_distance_bounds(1.0);
    assert!(fit_distance_for_view(1.0, 0.25) > far);
    assert!(fit_distance_for_view(1.0, 1.0) < far);
}

fn box_planes(half: Vec3) -> Vec<(Vec3, f32)> {
    vec![
        (Vec3::X, half.x),
        (Vec3::NEG_X, half.x),
        (Vec3::Y, half.y),
        (Vec3::NEG_Y, half.y),
        (Vec3::Z, half.z),
        (Vec3::NEG_Z, half.z),
    ]
}

/// The radius measured from the planes is the one the solid worker reports for the
/// mesh it draws from the same planes (`CachedMesh::bounding_radius`).
#[test]
fn the_planar_radius_matches_the_drawn_mesh() {
    for (name, stone) in stones() {
        let mut cache = MeshCache::default();
        let drawn = cache
            .get_or_build_geometry_keyed(stone.cache_key(), &stone)
            .expect("the fixture closes")
            .bounding_radius();
        let planar = planar_bounding_radius(&stone).expect("the fixture closes");
        assert!((planar - drawn).abs() < 1e-9, "{name}: {planar} vs {drawn}");
    }
}

/// A concave tool only removes material, so the planar radius encloses the carved
/// mesh; planes that close nothing have no radius.
#[test]
fn the_planar_radius_encloses_a_carved_stone_and_needs_a_closed_solid() {
    let carved = StoneGeometryBuf {
        tools: vec![ToolPrimitive::cylinder(
            Vec3::new(0.0, 0.6, 0.0),
            Vec3::Z,
            0.3,
            2.0,
        )],
        placements: vec![(0, 0)],
        ..StoneGeometryBuf::from_halfspaces(&box_planes(Vec3::new(1.0, 0.6, 1.0)))
    };
    let mut cache = MeshCache::default();
    let drawn = cache
        .get_or_build_geometry_keyed(carved.cache_key(), &carved)
        .expect("the grooved box closes")
        .bounding_radius();
    let planar = planar_bounding_radius(&carved).expect("the box closes");
    assert!(planar >= drawn - 1e-9, "{planar} < {drawn}");

    let open = StoneGeometryBuf::from_halfspaces(&[(Vec3::X, 1.0), (Vec3::NEG_X, 1.0)]);
    assert!(planar_bounding_radius(&open).is_none());
}

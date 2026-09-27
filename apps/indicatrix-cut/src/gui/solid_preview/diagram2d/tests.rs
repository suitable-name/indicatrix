//! Unit tests for the panel visibility predicates, the index-wheel screen
//! mapping, full-frame rendering, and meet-marker point resolution.

use super::{
    fill::meet_marker_points,
    layout::{crown_visible, pavilion_visible, profile_visible, wheel_direction},
    *,
};
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh};

/// "Visible" here means "selected by the panel's own facet-visibility
/// predicate" (see the parent module's doc comment) -- a cube's front profile face
/// still fully occludes its back face once rasterized, exactly like
/// `raster.rs`'s own crown/pavilion visibility test does before its depth
/// test runs. This is the count the feature brief's "4 visible side facets in
/// profile and 1 in crown" describes.
#[test]
fn cube_visibility_predicates_select_one_crown_facet_and_four_profile_facets() {
    let cube_normals = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    let crown_count = cube_normals.iter().filter(|&&n| crown_visible(n)).count();
    let pavilion_count = cube_normals
        .iter()
        .filter(|&&n| pavilion_visible(n))
        .count();
    let profile_count = cube_normals.iter().filter(|&&n| profile_visible(n)).count();

    assert_eq!(crown_count, 1, "only +Y should read as crown-facing");
    assert_eq!(pavilion_count, 1, "only -Y should read as pavilion-facing");
    assert_eq!(
        profile_count, 4,
        "the four vertical faces (+-X, +-Z) should all read as profile-facing"
    );
}

/// Gear 96, no reference-angle offset: index 0 must land at 12 o'clock
/// (straight up) on the crown panel, and index 24 (a quarter turn) at 3
/// o'clock -- see the parent module's doc comment for why crown sweeps clockwise.
#[test]
fn crown_index_wheel_places_index_0_at_top_and_index_24_at_three_oclock() {
    let gear_teeth = 96.0f32;
    let phi0 = 2.0 * std::f32::consts::PI * 0.0 / gear_teeth;
    let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;

    let (su0, sv0) = wheel_direction(phi0, false);
    assert!(su0.abs() < 1e-5, "index 0 must have no horizontal offset");
    assert!(sv0 > 0.99, "index 0 must point straight up (screen_up > 0)");

    let (su24, sv24) = wheel_direction(phi24, false);
    assert!(
        su24 > 0.99,
        "index 24 must point straight right (3 o'clock)"
    );
    assert!(sv24.abs() < 1e-5, "index 24 must have no vertical offset");
}

/// The pavilion panel mirrors the horizontal axis, so the same index lands on
/// the OPPOSITE side (9 o'clock instead of 3 o'clock) while index 0 stays at
/// the top on both panels.
#[test]
fn pavilion_index_wheel_mirrors_the_horizontal_axis() {
    let gear_teeth = 96.0f32;
    let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;
    let (su24, sv24) = wheel_direction(phi24, true);
    assert!(
        su24 < -0.99,
        "index 24 must point left (9 o'clock) when mirrored"
    );
    assert!(sv24.abs() < 1e-5);
}

/// A closed box's crown/pavilion/profile panels each pick back the expected
/// facet id at their own panel center -- the pick-buffer round trip the
/// hover/click wiring depends on.
#[test]
fn pick_buffer_round_trips_each_panels_center_to_the_right_facet() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: true,
    };
    let style = DiagramStyle::default();
    let frame = render_diagram(&mesh, &config, &style);

    // Panel centers sit at column midpoints (300 / 3 = 100px per column).
    assert_eq!(
        frame.pick_at(50, 60),
        Some(2),
        "crown panel center must show the +Y facet (id 2)"
    );
    assert_eq!(
        frame.pick_at(150, 60),
        Some(3),
        "pavilion panel center must show the -Y facet (id 3)"
    );
    assert_eq!(
        frame.pick_at(250, 60),
        Some(4),
        "profile panel center must show the nearer of +-Z (id 4, +Z)"
    );

    assert_eq!(frame.panel_at(50, 60), Some(PanelKind::Crown));
    assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Pavilion));
    assert_eq!(frame.panel_at(250, 60), Some(PanelKind::Profile));
}

#[test]
fn an_empty_or_zero_sized_config_never_panics() {
    let mesh = SolidMesh::default();
    let config = DiagramConfig {
        width: 0,
        height: 0,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let frame = render_diagram(&mesh, &config, &DiagramStyle::default());
    assert_eq!(frame.width, 0);
    assert_eq!(frame.pick_at(0, 0), None);
}

/// The enlarged panel must fill the WHOLE frame width, not the one-third
/// column [`render_diagram`] gives it -- otherwise "enlarge this panel"
/// would just be a relabeled crop of the existing image.
#[test]
fn render_diagram_single_panel_fills_the_whole_frame() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let style = DiagramStyle::default();
    let frame = render_diagram_single_panel(&mesh, &config, &style, PanelKind::Crown);

    // The crown panel's own center, now the FRAME's center (150, 60) rather
    // than a one-third column's (50, 60) -- see `render_diagram`'s own
    // matching assertion at column-center (50, 60).
    assert_eq!(
        frame.pick_at(150, 60),
        Some(2),
        "the enlarged panel's own center must show the +Y facet"
    );
    assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Crown));
    // "Fills the whole frame" in the sense the API can actually express:
    // `panel_at` tags only pixels a facet paints (see its own doc comment), so a
    // background pixel is `None` by contract, not `Some`. What must hold is that
    // no pixel anywhere belongs to a DIFFERENT panel -- in the three-column
    // layout, x = 200 would have been the pavilion column.
    for y in 0..config.height {
        for x in 0..config.width {
            let tag = frame.panel_at(x, y);
            assert!(
                tag.is_none() || tag == Some(PanelKind::Crown),
                "pixel ({x}, {y}) is tagged {tag:?}, but only the enlarged crown panel may paint in a single-panel frame"
            );
        }
    }
}

/// [`meet_marker_points`] must find the actual shared vertices between two
/// touching facets, not merely trust that a candidate pair (from
/// `FacetMap::meeting_facet_pairs`) really touches.
#[test]
fn meet_marker_points_finds_the_shared_edge_between_two_touching_facets() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    // Facet 0 (+X) and facet 2 (+Y) share the edge x=1, y=0.6, z in [-1, 1] --
    // two corners, (1, 0.6, -1) and (1, 0.6, 1).
    let points = meet_marker_points(&mesh, &[(0, 2)], 1e-4);
    assert_eq!(points.len(), 2, "got: {points:?}");
    for expected_z in [-1.0, 1.0] {
        assert!(
            points.iter().any(|p| (p.x - 1.0).abs() < 1e-6
                && (p.y - 0.6).abs() < 1e-6
                && (p.z - expected_z).abs() < 1e-6),
            "expected a marker at z={expected_z}, got: {points:?}"
        );
    }

    // An unrelated pair (facets that never touch) must find nothing.
    assert_eq!(meet_marker_points(&mesh, &[(0, 1)], 1e-4).len(), 0);
}

//! Tests for the Diagram view's drag handles: which panel offers which handle, where the
//! tips sit, how the pixel travel turns into degrees / mast / teeth, and how the index
//! handle turns the tier about the panel centre.

use super::*;
use glam::Vec3;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::ConstraintTier;
use indicatrix_solid::diagram2d::{DiagramLayout, PanelKind, PanelLayout};
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

const TEETH: u32 = 96;
const SCALE: f32 = 100.0;
const WHEEL_PX: f32 = 100.0;

fn tier(angle_deg: f64, indices: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: "T".to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A panel in column `index` of a 720 x 380 frame, 240 px wide.
fn panel(kind: PanelKind, index: i32) -> PanelLayout {
    PanelLayout {
        kind,
        center_x: 240.0_f32.mul_add(index as f32, 120.0),
        center_y: 190.0,
        scale: SCALE,
        wheel_radius_px: WHEEL_PX,
        clip: (240 * index, 0, 240 * (index + 1) - 1, 379),
    }
}

fn layout_with_reference(reference: f32) -> DiagramLayout {
    DiagramLayout {
        panels: vec![
            panel(PanelKind::Crown, 0),
            panel(PanelKind::Pavilion, 1),
            panel(PanelKind::Profile, 2),
        ],
        enlarged: false,
        gear_teeth: TEETH,
        gear_reference_angle: reference,
    }
}

fn layout() -> DiagramLayout {
    layout_with_reference(0.0)
}

fn close(a: f32, b: f32, eps: f32) {
    assert!((a - b).abs() < eps, "{a} is not within {eps} of {b}");
}

/// A facet centroid on the wheel at `index` (plus the layout's reference), `radius` from
/// the axis and `y` up it.
fn centroid_at(layout: &DiagramLayout, index: f32, radius: f32, y: f32) -> Vec3 {
    let phi = TAU * (index + layout.gear_reference_angle) / layout.gear_teeth as f32;
    Vec3::new(radius * phi.cos(), y, radius * phi.sin())
}

/// The frame of `tier`'s facet at `index` on `layout`'s wheel, with its centroid there.
fn frame_at(layout: &DiagramLayout, tier: &ConstraintTier, index: f32, y: f32) -> FacetFrame {
    facet_frame(
        tier,
        f64::from(index),
        layout,
        centroid_at(layout, index, 0.5, y),
    )
}

fn handles_on(layout: &DiagramLayout, kind: PanelKind, frame: &FacetFrame) -> PanelHandles {
    panel_handles(frame, layout.panel(kind).expect("the layout has the panel"))
        .expect("the panel carries the facet")
}

fn at(angle: f32) -> ScreenPoint {
    ScreenPoint::new(
        100.0_f32.mul_add(angle.cos(), 100.0),
        100.0_f32.mul_add(angle.sin(), 100.0),
    )
}

fn centre() -> ScreenPoint {
    ScreenPoint::new(100.0, 100.0)
}

fn start(
    kind: HandleKind,
    angle_deg: f64,
    mast: f64,
    handles: &PanelHandles,
    pointer: ScreenPoint,
) -> DragStart {
    DragStart {
        kind,
        start_angle_deg: angle_deg,
        start_mast: mast,
        pointer,
        layout: handles.layout,
    }
}

fn moved(from: ScreenPoint, dir: (f32, f32), pixels: f32) -> ScreenPoint {
    ScreenPoint::new(dir.0.mul_add(pixels, from.x), dir.1.mul_add(pixels, from.y))
}

// --- the facet frame and the wheel's reference angle ---

#[test]
fn the_frame_points_where_the_wheel_draws_the_facet_whatever_the_reference_angle() {
    let azimuth = |v: Vec3| v.z.atan2(v.x);
    for reference in [0.0_f32, 1.5, 7.0] {
        let layout = layout_with_reference(reference);
        let crown = tier(34.5, &[0.0, 3.0]);
        for index in [0.0_f32, 3.0] {
            let frame = frame_at(&layout, &crown, index, 0.3);
            close(azimuth(frame.normal), azimuth(frame.centroid), 1e-4);
        }
    }
}

#[test]
fn the_plain_frame_builder_would_miss_by_the_reference_angle() {
    // `FacetFrame::from_tier` ignores the wheel's reference angle; `facet_frame` is the
    // builder that adds it, so the two differ exactly by the reference.
    let layout = layout_with_reference(2.0);
    let crown = tier(34.5, &[0.0]);
    let centroid = centroid_at(&layout, 0.0, 0.5, 0.3);
    let shifted = facet_frame(&crown, 0.0, &layout, centroid);
    let plain = FacetFrame::from_tier(&crown, 0.0, TEETH, centroid);
    assert_ne!(shifted.normal, plain.normal);
    let by_hand = FacetFrame::from_tier(&crown, 2.0, TEETH, centroid);
    assert_eq!(shifted.normal, by_hand.normal);
    assert_eq!(shifted.gear_teeth, TEETH);
}

// --- the crown and pavilion panels ---

#[test]
fn a_crown_facet_gets_radial_angle_and_depth_handles_and_a_tangential_index_handle() {
    let layout = layout();
    let crown = tier(34.5, &[0.0]);
    let frame = frame_at(&layout, &crown, 0.0, 0.3);
    let handles = handles_on(&layout, PanelKind::Crown, &frame);
    assert_eq!(handles.panel, PanelKind::Crown);
    assert!(handles.available.angle && handles.available.depth && handles.available.index);
    let l = handles.layout;
    // Index 0 lies along +X, which the crown panel draws straight up from the centre.
    close(l.anchor.x, 120.0, 1e-3);
    close(l.anchor.y, 190.0 - 50.0, 1e-3);
    close(l.angle_dir.0, 0.0, 1e-4);
    close(l.angle_dir.1, -1.0, 1e-4);
    close(l.depth_dir.1, -1.0, 1e-4);
    close(l.index_dir.0, 1.0, 1e-4);
    close(l.index_dir.1, 0.0, 1e-4);
    // The angle tip is 0.32 of the wheel radius out, the depth tip 0.62: never on top of
    // each other.
    close(l.angle_tip.y, l.anchor.y - 32.0, 1e-3);
    close(l.depth_tip.y, l.anchor.y - 62.0, 1e-3);
    close(l.index_tip.x, l.anchor.x + 32.0, 1e-3);
    // Pixels per unit: cos(theta) of the lever per degree, sin(theta) per mast unit, one
    // tooth is a 96th of the circle through the anchor.
    let theta = 34.5_f32.to_radians();
    close(
        l.pixels_per_degree,
        (theta.cos() * SCALE).to_radians(),
        1e-3,
    );
    close(l.pixels_per_mast_unit, theta.sin() * SCALE, 1e-2);
    close(l.pixels_per_tooth, SCALE * 0.5 * TAU / TEETH as f32, 1e-3);
    assert!(handles.rotation.is_some());
}

#[test]
fn the_pavilion_panel_mirrors_the_crown_horizontally() {
    let layout = layout();
    let crown = tier(34.5, &[24.0]);
    let pavilion = tier(-41.0, &[24.0]);
    let on_crown = frame_at(&layout, &crown, 24.0, 0.3);
    let on_pavilion = frame_at(&layout, &pavilion, 24.0, -0.3);
    let crown_handles = handles_on(&layout, PanelKind::Crown, &on_crown);
    let pavilion_handles = handles_on(&layout, PanelKind::Pavilion, &on_pavilion);
    // A quarter turn round the wheel: right of the crown centre, left of the pavilion's.
    assert!(crown_handles.layout.anchor.x > 120.0 + 40.0);
    assert!(pavilion_handles.layout.anchor.x < 360.0 - 40.0);
    assert!(
        crown_handles.layout.index_dir.1 > 0.9,
        "clockwise runs down"
    );
    assert!(pavilion_handles.layout.index_dir.1 > 0.9);
    // Each panel turns the tier about its own centre.
    let crown_turn = crown_handles.rotation.expect("rotation");
    let pavilion_turn = pavilion_handles.rotation.expect("rotation");
    assert_eq!(crown_turn.center(), ScreenPoint::new(120.0, 190.0));
    assert_eq!(pavilion_turn.center(), ScreenPoint::new(360.0, 190.0));
}

#[test]
fn a_facet_is_only_on_the_panels_that_draw_it() {
    let layout = layout();
    let crown = tier(34.5, &[0.0]);
    let pavilion = tier(-41.0, &[0.0]);
    let on_crown = frame_at(&layout, &crown, 0.0, 0.3);
    let on_pavilion = frame_at(&layout, &pavilion, 0.0, -0.3);
    let panel_of = |kind| layout.panel(kind).expect("panel");
    assert!(panel_handles(&on_crown, panel_of(PanelKind::Pavilion)).is_none());
    assert!(panel_handles(&on_pavilion, panel_of(PanelKind::Crown)).is_none());
}

// --- degenerate levers ---

#[test]
fn the_table_has_no_depth_handle_from_above_and_no_index_handle_at_all() {
    let layout = layout();
    let table = tier(0.0, &[]);
    let frame = frame_at(&layout, &table, 0.0, 0.4);
    let handles = handles_on(&layout, PanelKind::Crown, &frame);
    assert!(handles.available.angle);
    assert!(!handles.available.depth, "its normal points at the viewer");
    assert!(!handles.available.index, "no index positions");
    assert!(handles.layout.depth_tip.x.is_nan() && handles.layout.index_tip.y.is_nan());
    assert!(!handles.layout.angle_tip.x.is_nan());
}

#[test]
fn a_very_steep_facet_loses_its_angle_handle_and_a_very_flat_one_its_depth_handle() {
    let layout = layout();
    let steep = frame_at(&layout, &tier(85.0, &[0.0]), 0.0, 0.05);
    let on_steep = handles_on(&layout, PanelKind::Crown, &steep);
    assert!(!on_steep.available.angle && on_steep.available.depth);
    assert!(on_steep.layout.angle_tip.x.is_nan());

    let flat = frame_at(&layout, &tier(5.0, &[0.0]), 0.0, 0.4);
    let on_flat = handles_on(&layout, PanelKind::Crown, &flat);
    assert!(on_flat.available.angle && !on_flat.available.depth);
    assert!(on_flat.layout.depth_tip.x.is_nan());

    let middle = frame_at(&layout, &tier(30.0, &[0.0]), 0.0, 0.3);
    let on_middle = handles_on(&layout, PanelKind::Crown, &middle);
    assert!(on_middle.available.angle && on_middle.available.depth);
}

#[test]
fn the_availability_helpers_agree_with_the_flags() {
    let none = HandleAvailability::default();
    assert!(!none.any());
    let angle_only = HandleAvailability {
        angle: true,
        ..none
    };
    assert!(angle_only.any());
    assert!(angle_only.offers(HandleKind::Angle));
    assert!(!angle_only.offers(HandleKind::Depth));
    assert!(!angle_only.offers(HandleKind::Index));
}

// --- the profile ---

#[test]
fn only_an_edge_on_facet_has_handles_in_the_profile() {
    let layout = layout();
    let crown = tier(34.5, &[0.0, 24.0]);
    let edge_on = frame_at(&layout, &crown, 0.0, 0.3);
    let facing = frame_at(&layout, &crown, 24.0, 0.3);
    let profile = layout.panel(PanelKind::Profile).expect("profile");
    assert!(
        panel_handles(&facing, profile).is_none(),
        "faces the viewer"
    );
    let handles = panel_handles(&edge_on, profile).expect("edge-on");
    assert!(handles.available.angle && handles.available.depth);
    assert!(!handles.available.index && handles.rotation.is_none());
    // The anchor is where the profile draws the centroid; the two levers are
    // perpendicular and equally long.
    close(handles.layout.anchor.x, 600.0 + 50.0, 1e-3);
    close(handles.layout.anchor.y, 190.0 - 30.0, 1e-3);
    let l = handles.layout;
    close(
        l.angle_dir
            .0
            .mul_add(l.depth_dir.0, l.angle_dir.1 * l.depth_dir.1),
        0.0,
        1e-4,
    );
    let tip = |p: ScreenPoint| (p.x - l.anchor.x).hypot(p.y - l.anchor.y);
    close(tip(l.angle_tip), 32.0, 1e-3);
    close(tip(l.depth_tip), 32.0, 1e-3);
}

#[test]
fn the_girdle_facing_the_viewer_has_no_panel_at_all() {
    let layout = layout();
    let girdle = tier(90.0, &[0.0, 24.0]);
    let edge_on = frame_at(&layout, &girdle, 0.0, 0.0);
    let facing = frame_at(&layout, &girdle, 24.0, 0.0);
    let placed = place_handles(&edge_on, &layout, None).expect("edge-on in the profile");
    assert_eq!(placed.panel, PanelKind::Profile);
    assert!(place_handles(&facing, &layout, None).is_none());
}

// --- choosing the panel ---

#[test]
fn the_preferred_panel_wins_only_when_it_carries_the_facet() {
    let layout = layout();
    let crown = tier(34.5, &[0.0, 24.0]);
    let edge_on = frame_at(&layout, &crown, 0.0, 0.3);
    let facing = frame_at(&layout, &crown, 24.0, 0.3);
    let panel_for = |frame: &FacetFrame, preferred| {
        place_handles(frame, &layout, preferred).map(|placed| placed.panel)
    };
    assert_eq!(panel_for(&edge_on, None), Some(PanelKind::Crown));
    assert_eq!(
        panel_for(&edge_on, Some(PanelKind::Profile)),
        Some(PanelKind::Profile)
    );
    assert_eq!(
        panel_for(&edge_on, Some(PanelKind::Pavilion)),
        Some(PanelKind::Crown),
        "the pavilion does not draw a crown facet"
    );
    assert_eq!(
        panel_for(&facing, Some(PanelKind::Profile)),
        Some(PanelKind::Crown),
        "a facet facing the viewer is no line in the profile"
    );
}

#[test]
fn an_enlarged_frame_offers_only_the_panel_it_drew() {
    let mut layout = layout();
    layout
        .panels
        .retain(|panel| panel.kind == PanelKind::Pavilion);
    layout.enlarged = true;
    let pavilion = tier(-41.0, &[0.0]);
    let crown = tier(34.5, &[0.0]);
    let on_pavilion = frame_at(&layout, &pavilion, 0.0, -0.3);
    let on_crown = frame_at(&layout, &crown, 0.0, 0.3);
    let placed = place_handles(&on_pavilion, &layout, Some(PanelKind::Crown)).expect("pavilion");
    assert_eq!(placed.panel, PanelKind::Pavilion);
    assert!(place_handles(&on_crown, &layout, None).is_none());
}

#[test]
fn the_candidate_facets_put_the_remembered_one_first_when_it_belongs_to_the_tier() {
    let tier_of = |id: u32| Some(if id < 10 { 1 } else { 2 });
    assert_eq!(
        candidate_facets(Some(3), 1, &[1, 2, 3, 4], tier_of),
        [3, 1, 2, 4]
    );
    assert_eq!(
        candidate_facets(Some(12), 1, &[1, 2, 3], tier_of),
        [1, 2, 3],
        "a facet of another tier is ignored"
    );
    assert_eq!(candidate_facets(None, 1, &[5, 6], tier_of), [5, 6]);
    assert_eq!(candidate_facets(None, 1, &[], tier_of).len(), 0);
}

// --- zoom ---

#[test]
fn a_zoomed_in_view_grabs_markers_within_a_smaller_radius_of_its_own_pixels() {
    close(zoomed_hit_radius(18.0, 1.0), 18.0, 1e-6);
    close(zoomed_hit_radius(18.0, 2.0), 9.0, 1e-6);
    close(zoomed_hit_radius(18.0, 4.0), 4.5, 1e-6);
    // Zoomed OUT the markers do not grow, and a nonsense zoom changes nothing.
    close(zoomed_hit_radius(18.0, 0.5), 18.0, 1e-6);
    close(zoomed_hit_radius(18.0, 0.0), 18.0, 1e-6);
    close(zoomed_hit_radius(18.0, f32::NAN), 18.0, 1e-6);
}

// --- dragging with the shared mapping ---

#[test]
fn an_angle_drag_on_the_crown_panel_snaps_like_in_the_3d_view() {
    let layout = layout();
    let crown = tier(34.5, &[0.0]);
    let frame = frame_at(&layout, &crown, 0.0, 0.3);
    let handles = handles_on(&layout, PanelKind::Crown, &frame);
    let l = handles.layout;
    let drag = start(HandleKind::Angle, 34.5, 0.59, &handles, l.angle_tip);
    let pixels = 5.04 * l.pixels_per_degree;
    let pointer = moved(l.angle_tip, l.angle_dir, pixels);
    let DragValue::AngleDeg(coarse) = drag_value(&drag, pointer, SnapMode::Coarse) else {
        panic!("an angle drag yields an angle");
    };
    assert!((coarse - 39.5).abs() < 1e-9, "snapped to 0.1: {coarse}");
    let DragValue::AngleDeg(off) = drag_value(&drag, pointer, SnapMode::Off) else {
        panic!("an angle drag yields an angle");
    };
    assert!((off - 39.54).abs() < 0.01, "unsnapped: {off}");
}

#[test]
fn a_pavilion_angle_drag_outward_makes_the_angle_more_negative() {
    let layout = layout();
    let pavilion = tier(-41.0, &[0.0]);
    let frame = frame_at(&layout, &pavilion, 0.0, -0.3);
    let handles = handles_on(&layout, PanelKind::Pavilion, &frame);
    let l = handles.layout;
    let drag = start(HandleKind::Angle, -41.0, 0.67, &handles, l.angle_tip);
    let pointer = moved(l.angle_tip, l.angle_dir, 5.04 * l.pixels_per_degree);
    let DragValue::AngleDeg(angle) = drag_value(&drag, pointer, SnapMode::Coarse) else {
        panic!("an angle drag yields an angle");
    };
    assert!((angle + 46.0).abs() < 1e-9, "{angle}");
}

#[test]
fn a_depth_drag_along_the_normal_grows_the_mast() {
    let layout = layout();
    let crown = tier(34.5, &[0.0]);
    let frame = frame_at(&layout, &crown, 0.0, 0.3);
    let handles = handles_on(&layout, PanelKind::Crown, &frame);
    let l = handles.layout;
    let drag = start(HandleKind::Depth, 34.5, 0.59, &handles, l.depth_tip);
    let pointer = moved(l.depth_tip, l.depth_dir, 0.104 * l.pixels_per_mast_unit);
    let DragValue::Mast(coarse) = drag_value(&drag, pointer, SnapMode::Coarse) else {
        panic!("a depth drag yields a mast");
    };
    assert!((coarse - 0.69).abs() < 1e-9, "snapped to 0.01: {coarse}");
    let DragValue::Mast(fine) = drag_value(&drag, pointer, SnapMode::Fine) else {
        panic!("a depth drag yields a mast");
    };
    assert!((fine - 0.694).abs() < 1e-9, "snapped to 0.001: {fine}");
}

// --- the index handle's rotation ---

#[test]
fn a_clockwise_quarter_turn_on_the_crown_panel_is_a_quarter_of_the_teeth() {
    let mut rotation = IndexRotation::new(centre(), false, TEETH);
    rotation.begin(at(0.0));
    assert_eq!(rotation.teeth_at(at(0.0)), 0);
    assert_eq!(rotation.teeth_at(at(FRAC_PI_2)), 24);
    // Back to the press: no net turn.
    assert_eq!(rotation.teeth_at(at(0.0)), 0);
}

#[test]
fn the_pavilion_panel_turns_the_other_way() {
    let mut rotation = IndexRotation::new(centre(), true, TEETH);
    rotation.begin(at(0.0));
    assert_eq!(rotation.teeth_at(at(FRAC_PI_2)), -24);
}

#[test]
fn the_turn_is_whole_teeth_rounded() {
    let tooth = TAU / TEETH as f32;
    let mut rotation = IndexRotation::new(centre(), false, TEETH);
    rotation.begin(at(0.0));
    assert_eq!(rotation.teeth_at(at(0.4 * tooth)), 0);
    assert_eq!(rotation.teeth_at(at(0.6 * tooth)), 1);
    assert_eq!(rotation.teeth_at(at(3.0 * tooth)), 3);
    assert_eq!(rotation.teeth_at(at(-tooth)), -1);
}

#[test]
fn a_drag_can_go_round_the_wheel_more_than_once() {
    let mut rotation = IndexRotation::new(centre(), false, TEETH);
    rotation.begin(at(0.0));
    let mut teeth = 0;
    // Twelve eighth-turns: a turn and a half, each step well under half a revolution.
    for step in 1..=12 {
        teeth = rotation.teeth_at(at(step as f32 * FRAC_PI_4));
    }
    assert_eq!(teeth, 144);
    // And back the way it came.
    for step in (0..12).rev() {
        teeth = rotation.teeth_at(at(step as f32 * FRAC_PI_4));
    }
    assert_eq!(teeth, 0);
}

#[test]
fn a_pointer_near_the_centre_has_no_azimuth_and_changes_nothing() {
    let mut rotation = IndexRotation::new(centre(), false, TEETH);
    rotation.begin(at(0.0));
    assert_eq!(rotation.teeth_at(ScreenPoint::new(101.0, 101.0)), 0);
    // The turn resumes from where the pointer last had an azimuth.
    assert_eq!(rotation.teeth_at(at(FRAC_PI_2)), 24);

    // A press in the dead zone starts from the first usable pointer position.
    let mut late = IndexRotation::new(centre(), false, TEETH);
    late.begin(centre());
    assert_eq!(late.teeth_at(at(0.0)), 0);
    assert_eq!(late.teeth_at(at(FRAC_PI_2)), 24);
}

#[test]
fn begin_forgets_an_earlier_turn() {
    let mut rotation = IndexRotation::new(centre(), false, TEETH);
    rotation.begin(at(0.0));
    assert_eq!(rotation.teeth_at(at(FRAC_PI_2)), 24);
    rotation.begin(at(FRAC_PI_2));
    assert_eq!(rotation.teeth_at(at(FRAC_PI_2)), 0);
}

#[test]
fn dragging_from_one_facet_to_the_next_turns_by_the_index_gap_on_both_panels() {
    for reference in [0.0_f32, 1.5] {
        let layout = layout_with_reference(reference);
        let crown = tier(34.5, &[0.0, 3.0]);
        let pavilion = tier(-41.0, &[0.0, 3.0]);
        for (kind, facet_tier, y) in [
            (PanelKind::Crown, &crown, 0.3),
            (PanelKind::Pavilion, &pavilion, -0.3),
        ] {
            let from = handles_on(&layout, kind, &frame_at(&layout, facet_tier, 0.0, y));
            let to = handles_on(&layout, kind, &frame_at(&layout, facet_tier, 3.0, y));
            let mut rotation = from.rotation.expect("a wheel panel turns the tier");
            rotation.begin(from.layout.anchor);
            assert_eq!(
                rotation.teeth_at(to.layout.anchor),
                3,
                "{kind:?}, reference {reference}: three teeth on, three teeth turned"
            );
        }
    }
}

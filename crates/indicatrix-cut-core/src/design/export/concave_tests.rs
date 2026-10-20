//! Tests for [`super::concave_frame`] and the concave resolver in
//! [`super::concave`]: the frame algebra, the contact point, the tool order and
//! the planar no-op guarantee.

use super::{
    ConcaveResolveError,
    concave_frame::{
        CONCAVE_FRAME_VERSION, contact_point, dop_frame, stone_width, support_vertex, tool_axis,
        tool_centre,
    },
};
use crate::{
    design::{ConcaveTier, ConcaveTool, ConstraintTier, Design, ScheduleMeta, TierRef},
    preform::PreformSpec,
};
use glam::DVec3;
use indicatrix::geometry::{
    stone_metrics::measure_solid,
    tool::{MAX_TOOL_PRIMITIVES, ToolKind, ToolPrimitive, ToolSweep},
};
use indicatrix_formats::native::design::CONCAVE_FRAME_V0;

fn close(a: DVec3, b: DVec3, tol: f64) -> bool {
    (a - b).length() <= tol
}

/// `|x|, |y|, |z| <= 1`.
fn cube() -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 1.0),
        (DVec3::NEG_Y, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]
}

#[test]
fn dop_frame_is_right_handed_and_u_points_toward_the_girdle() {
    // Crown (+40) and pavilion (-42), on a 96 wheel at a few azimuths.
    for (angle, index) in [(40.0, 0.0), (40.0, 17.0), (-42.0, 0.0), (-42.0, 61.5)] {
        let (u, v, n) = dop_frame(angle, index, 0.0, 96);
        for (name, axis) in [("u", u), ("v", v), ("n", n)] {
            // `n` is the flat path's f32 normal, so it is unit only to f32 rounding.
            assert!(
                (axis.length() - 1.0).abs() < 1e-6,
                "{name} is not a unit vector at ({angle}, {index})"
            );
        }
        assert!(u.dot(n).abs() < 1e-6, "u is not in the facet plane");
        assert!(
            close(u.cross(v), n, 1e-6),
            "(u, v, n) is not right-handed at ({angle}, {index})"
        );
        assert!(close(n.cross(u), v, 1e-6), "v is not n x u");
        // Girdle-ward: down for the crown, up for the pavilion.
        if angle > 0.0 {
            assert!(u.y < 0.0 && n.y > 0.0, "crown frame {u:?} {n:?}");
        } else {
            assert!(u.y > 0.0 && n.y < 0.0, "pavilion frame {u:?} {n:?}");
        }
    }
    // The normal is StandardGemCuts': index 24 of 96 is azimuth 90 degrees.
    let (_, _, n) = dop_frame(40.0, 24.0, 0.0, 96);
    let theta = 40.0_f64.to_radians();
    assert!(close(n, DVec3::new(0.0, theta.cos(), theta.sin()), 1e-6));
}

#[test]
fn contact_point_is_the_support_vertex_with_deterministic_tie_break() {
    let planes = cube();
    // A unique maximum.
    let corner = contact_point(&planes, DVec3::new(1.0, 1.0, 1.0)).expect("cube closes");
    assert!(close(corner, DVec3::new(1.0, 1.0, 1.0), 1e-9), "{corner:?}");
    // An edge direction ties two corners: the lexicographically smallest wins.
    let tie = contact_point(&planes, DVec3::new(1.0, 1.0, 0.0)).expect("cube closes");
    assert!(close(tie, DVec3::new(1.0, 1.0, -1.0), 1e-9), "{tie:?}");
    // A face direction ties four.
    let face = contact_point(&planes, DVec3::Y).expect("cube closes");
    assert!(close(face, DVec3::new(-1.0, 1.0, -1.0), 1e-9), "{face:?}");
    // The answer does not depend on the order the planes (hence vertices) come in.
    let mut reversed = planes;
    reversed.reverse();
    let again = contact_point(&reversed, DVec3::new(1.0, 1.0, 0.0)).expect("cube closes");
    assert!(close(again, tie, 1e-9));
    // Bit-exact tie handling on a plain vertex list.
    let vertices = [
        DVec3::new(1.0, 2.0, 3.0),
        DVec3::new(1.0, 2.0, -3.0),
        DVec3::new(0.0, 5.0, 0.0),
    ];
    assert_eq!(
        support_vertex(&vertices, DVec3::new(1.0, 1.0, 0.0), 10.0),
        Some(DVec3::new(0.0, 5.0, 0.0)),
        "5 beats 1 + 2 on this direction"
    );
    assert_eq!(
        support_vertex(&vertices[..2], DVec3::new(1.0, 1.0, 0.0), 10.0),
        Some(DVec3::new(1.0, 2.0, -3.0))
    );
    assert_eq!(support_vertex(&[], DVec3::X, 1.0), None);
}

/// A concave tier on the same (phi, index) as a flat facet gets that facet's normal
/// exactly, so every corner of the facet ties and the lexicographically smallest
/// corner is the contact point, however the corners round.
#[test]
fn a_concave_tier_on_a_flat_facets_phi_and_index_picks_the_deterministic_corner() {
    use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let (metrics, vertices) = measure_solid_with_vertices(&planes).expect("closes");
    // The fixture's flat pavilion tier -42 sits at indices 0, 4, 8, 12 of a 16
    // wheel; so does the concave tier's reference, at every one of them.
    let gear = design.meta.gear_teeth;
    let reference = design.meta.gear_reference_angle;
    for index in [0.0, 4.0, 8.0, 12.0] {
        let (_, _, n) = dop_frame(-42.0, index, reference, gear);
        let (flat_normal, offset) = planes
            .iter()
            .copied()
            .find(|(normal, _)| (*normal - n).length() < 1e-12)
            .unwrap_or_else(|| panic!("no flat facet has the frame normal at index {index}"));
        assert_eq!(
            flat_normal, n,
            "the frame normal is the flat normal exactly"
        );
        let corners: Vec<DVec3> = vertices
            .iter()
            .copied()
            .filter(|v| (v.dot(n) - offset).abs() < 1e-9)
            .collect();
        assert!(corners.len() >= 3, "a facet has at least three corners");
        let expected = corners
            .iter()
            .copied()
            .min_by(|a, b| {
                a.x.total_cmp(&b.x)
                    .then(a.y.total_cmp(&b.y))
                    .then(a.z.total_cmp(&b.z))
            })
            .expect("corners");
        let picked = support_vertex(&vertices, n, metrics.width_axis).expect("vertices");
        assert_eq!(picked, expected, "index {index}");
    }
}

#[test]
fn tool_centre_moves_into_the_stone_for_positive_z() {
    // Pavilion facet on a unit-ish block: a positive Z must go against the
    // outward normal, i.e. into the stone.
    let frame = dop_frame(-45.0, 0.0, 0.0, 96);
    let (u, v, n) = frame;
    let c0 = DVec3::new(1.0, -1.0, -1.0);
    let width = 2.0;
    let deeper = tool_centre(c0, width, frame, [0.0, 0.0, 0.25]);
    assert!(close(deeper - c0, -n * 0.5, 1e-12), "{deeper:?}");
    assert!((deeper - c0).dot(n) < 0.0);
    // X and Y move along the in-plane axes, scaled by the width.
    let moved = tool_centre(c0, width, frame, [0.1, -0.2, 0.0]);
    assert!(close(moved - c0, u * 0.2 - v * 0.4, 1e-12));
    // No displacement leaves the tool on the contact point.
    assert_eq!(tool_centre(c0, width, frame, [0.0; 3]), c0);
}

#[test]
fn tool_axis_follows_theta_and_a_plunged_cone_points_into_the_stone() {
    let frame = dop_frame(-42.0, 4.0, 0.0, 16);
    let (u, v, n) = frame;
    let mut tier = Design::concave_fixture().concave_tiers.remove(0);
    tier.tool_azimuth_deg = 0.0;
    assert!(close(tool_axis(&tier, frame), u, 1e-12));
    tier.tool_azimuth_deg = 90.0;
    assert!(close(tool_axis(&tier, frame), v, 1e-12));
    tier.tool_azimuth_deg = 450.0;
    assert!(close(tool_axis(&tier, frame), v, 1e-12), "450 is 90");
    tier.tool_azimuth_deg = -90.0;
    assert!(close(tool_axis(&tier, frame), -v, 1e-12), "-90 is 270");
    tier.tool = ConcaveTool::Cone;
    tier.tool_angle_deg = Some(60.0);
    tier.motion = crate::design::ToolMotion::Plunge;
    assert!(close(tool_axis(&tier, frame), -n, 1e-12));
    tier.motion = crate::design::ToolMotion::Reciprocating;
    assert!(close(tool_axis(&tier, frame), -v, 1e-12));
}

#[test]
fn concave_frame_version_matches_the_formats_constant() {
    assert_eq!(CONCAVE_FRAME_VERSION, CONCAVE_FRAME_V0);
}

/// Guard against a silent frame bump: the width rule is settled (§11a Q3), so the frame
/// string stays `v0` until a deliberate decision, a reader shim and a migration say otherwise.
#[test]
fn concave_frame_version_is_still_v0() {
    assert_eq!(CONCAVE_FRAME_VERSION, "v0");
    assert_eq!(CONCAVE_FRAME_V0, "v0");
}

/// `f32` narrowing exactly as `concave_frame::narrow`, for the expectations below.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the resolver narrows its f64 geometry to f32 once; the expectations mirror that"
)]
const fn narrow(x: f64) -> f32 {
    x as f32
}

/// The width rule (§11a Q3): every ratio is taken of the finished flat stone's
/// `width_axis`, computed in the resolver's own order so the comparison is exact.
#[test]
fn tools_are_scaled_by_the_finished_stones_axis_width() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let metrics = measure_solid(&planes).expect("the fixture closes");
    let w = metrics.width_axis;
    assert_eq!(stone_width(&metrics).to_bits(), w.to_bits());

    let (tools, _) = design
        .concave_tools_from_solved(&solved)
        .expect("the fixture resolves");
    assert_eq!(tools.len(), 12);
    // Groove: CYL, D/W = 0.25, reciprocating (half-stroke 2 W).
    assert_eq!(tools[0].kind(), Some(ToolKind::Cylinder));
    assert_eq!(
        tools[0].profile[0].to_bits(),
        narrow(0.5 * (w * 0.25)).to_bits()
    );
    assert_eq!(tools[0].profile[2].to_bits(), narrow(2.0 * w).to_bits());
    // Dimple: SPH, D/W = 0.1, plunge.
    assert_eq!(tools[8].kind(), Some(ToolKind::Ball));
    assert_eq!(
        tools[8].profile[0].to_bits(),
        narrow(0.5 * (w * 0.1)).to_bits()
    );
}

/// A partial stone is not re-measured: the tools at any step are a prefix of the
/// finished stone's tools, bit for bit.
#[test]
fn a_partial_stones_tools_are_the_finished_stones_tools() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let (full, _) = design
        .concave_tools_from_solved(&solved)
        .expect("the fixture resolves");
    assert_eq!(full.len(), 12);
    assert_eq!(
        design
            .concave_tools_after_steps(&solved, 4)
            .expect("resolves")
            .0,
        full[..8]
    );
    assert_eq!(
        design
            .concave_tools_through_tier(&solved, TierRef::Concave(1))
            .expect("resolves")
            .0,
        full[..8]
    );
    assert_eq!(
        design
            .concave_tools_after_steps(&solved, 7)
            .expect("resolves")
            .0,
        full
    );
}

/// The test that tells the finished-stone rule (`v0`) from a per-step width (`v1`): with no
/// girdle facet before the pavilion groove, the half-cut stone is the preform's width, wider
/// than the finished stone, and the tools still follow the finished one.
#[test]
fn the_width_is_the_finished_stones_even_with_no_girdle_facet_before_the_tier() {
    let mut design = Design::concave_fixture();
    design.ensure_tier_ids();
    // Drop the phi = 90 girdle tier: crown and pavilion now meet in a knife edge.
    design.tiers.remove(0);
    design.tier_ids.remove(0);
    let solved = design.solve().expect("the remaining tiers are pinned");
    let planes = design.planes_from_solved(&solved);
    let w_finished = measure_solid(&planes)
        .expect("the knife-edge stone closes")
        .width_axis;

    let k = design
        .cutting_order()
        .iter()
        .position(|tier| *tier == TierRef::Concave(0))
        .expect("the groove is in the cutting order");
    let prefix = design
        .try_planes_after_steps(&solved, k)
        .expect("one solved tier per flat tier");
    let w_prefix = measure_solid(&prefix)
        .expect("the preform closes the half-cut stone")
        .width_axis;
    assert!(
        w_prefix > 1.05 * w_finished,
        "the half-cut stone must be wider than the finished one for this test to mean anything \
         (prefix {w_prefix}, finished {w_finished})"
    );

    let (tools, _) = design
        .concave_tools_from_solved(&solved)
        .expect("the fixture resolves");
    assert_eq!(
        tools[0].profile[0].to_bits(),
        narrow(0.5 * (w_finished * 0.25)).to_bits()
    );
    assert_ne!(
        tools[0].profile[0].to_bits(),
        narrow(0.5 * (w_prefix * 0.25)).to_bits(),
        "the groove must not be sized from the half-cut stone"
    );
    assert_eq!(
        design
            .concave_tools_after_steps(&solved, k + 1)
            .expect("resolves")
            .0,
        tools[..8]
    );
}

/// The tools depend on the finished flat stone alone, not on the order the flat tiers are
/// stored in. Not bit-exact on purpose: plane order changes the vertex walk's rounding.
#[test]
fn the_tools_do_not_follow_the_stored_order_of_the_flat_tiers() {
    let a = Design::concave_fixture();
    let mut b = a.clone();
    b.ensure_tier_ids();
    // Flat tiers 1 and 2 are both pavilion tiers with pinned masts.
    b.tiers.swap(1, 2);
    b.tier_ids.swap(1, 2);
    let solved_a = a.solve().expect("the fixture pins every mast");
    let solved_b = b.solve().expect("the swapped fixture pins every mast");
    let wa = measure_solid(&a.planes_from_solved(&solved_a))
        .expect("closes")
        .width_axis;
    let wb = measure_solid(&b.planes_from_solved(&solved_b))
        .expect("closes")
        .width_axis;
    assert!((wa - wb).abs() <= 1e-12 * wa.abs(), "{wa} vs {wb}");

    let (tools_a, _) = a.concave_tools_from_solved(&solved_a).expect("resolves");
    let (tools_b, _) = b.concave_tools_from_solved(&solved_b).expect("resolves");
    assert_eq!(tools_a.len(), tools_b.len());
    for (k, (x, y)) in tools_a.iter().zip(&tools_b).take(8).enumerate() {
        assert_eq!(x.kind, y.kind, "tool {k}");
        assert_eq!(x.sweep_kind, y.sweep_kind, "tool {k}");
        for (name, p, q) in [
            ("origin", x.origin, y.origin),
            ("axis", x.axis, y.axis),
            ("profile", x.profile, y.profile),
        ] {
            for (c, (&u, &v)) in p.iter().zip(&q).enumerate() {
                assert!(
                    (f64::from(u) - f64::from(v)).abs() <= 1e-6,
                    "tool {k} {name}[{c}]: {u} vs {v}"
                );
            }
        }
    }
}

#[test]
fn concave_tools_from_solved_emits_one_primitive_per_index_in_cutting_order() {
    let mut design = Design::concave_fixture();
    // Store the crown dimple first: cutting order still puts the pavilion
    // groove (now concave tier 1) ahead of it.
    design.concave_tiers.reverse();
    design.concave_tier_ids.reverse();
    assert_eq!(
        design
            .cutting_order()
            .into_iter()
            .filter(|t| matches!(t, TierRef::Concave(_)))
            .collect::<Vec<_>>(),
        vec![TierRef::Concave(1), TierRef::Concave(0)]
    );
    let solved = design.solve().expect("the fixture pins every mast");
    let (tools, placements) = design
        .concave_tools_from_solved(&solved)
        .expect("the fixture resolves");

    assert_eq!(tools.len(), 12);
    assert_eq!(placements.len(), 12);
    let expected: Vec<(usize, usize)> = (0..8)
        .map(|p| (1, p))
        .chain((0..4).map(|p| (0, p)))
        .collect();
    assert_eq!(placements, expected);
    for (k, tool) in tools.iter().enumerate() {
        tool.validate()
            .unwrap_or_else(|e| panic!("tool {k} is not a valid primitive: {e}"));
        let (cylinder, ball) = (k < 8, k >= 8);
        assert_eq!(
            tool.kind() == Some(ToolKind::Cylinder),
            cylinder,
            "tool {k}"
        );
        assert_eq!(tool.kind() == Some(ToolKind::Ball), ball, "tool {k}");
        let sweep = if cylinder {
            ToolSweep::AlongAxis
        } else {
            ToolSweep::None
        };
        assert_eq!(tool.sweep(), Some(sweep), "tool {k}");
    }
    // Deterministic: resolving twice gives the same tools.
    let again = design.concave_tools_from_solved(&solved).expect("resolves");
    assert_eq!(again.0, tools);

    // `geometry_from_solved` is the same tools plus the planar arrangement.
    let (planes, geometry_tools, geometry_placements) =
        design.geometry_from_solved(&solved).expect("resolves");
    assert_eq!(planes, design.planes_from_solved(&solved));
    assert_eq!(geometry_tools, tools);
    assert_eq!(geometry_placements, placements);
}

#[test]
fn concave_tools_through_tier_includes_only_the_tiers_that_precede_it() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    // Groove (concave 0, pavilion) precedes Dimple (concave 1, crown).
    let (before_dimple, placements) = design
        .concave_tools_through_tier(&solved, TierRef::Concave(1))
        .expect("resolves");
    assert_eq!(before_dimple.len(), 8);
    assert!(placements.iter().all(|&(tier, _)| tier == 0));
    let (before_groove, _) = design
        .concave_tools_through_tier(&solved, TierRef::Concave(0))
        .expect("resolves");
    assert!(
        before_groove.is_empty(),
        "only flat pavilion tiers precede it"
    );
    // A stale reference means everything.
    let (all, _) = design
        .concave_tools_through_tier(&solved, TierRef::Flat(99))
        .expect("resolves");
    assert_eq!(all.len(), 12);
}

#[test]
fn a_design_without_concave_tiers_resolves_to_no_tools_and_the_same_planes() {
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let solved = design.solve().expect("every tier is pinned");
    let (tools, placements) = design.concave_tools_from_solved(&solved).expect("no tiers");
    assert!(tools.is_empty() && placements.is_empty());
    let (planes, tools, placements) = design.geometry_from_solved(&solved).expect("no tiers");
    assert!(tools.is_empty() && placements.is_empty());
    let want = design.planes_from_solved(&solved);
    assert_eq!(planes.len(), want.len());
    for (got, want) in planes.iter().zip(&want) {
        assert_eq!(
            got.0.to_array().map(f64::to_bits),
            want.0.to_array().map(f64::to_bits)
        );
        assert_eq!(got.1.to_bits(), want.1.to_bits());
    }
    let (through, _) = design
        .concave_tools_through_tier(&solved, TierRef::Flat(0))
        .expect("no tiers");
    assert_eq!(through, [] as [ToolPrimitive; 0]);
}

#[test]
fn concave_tools_from_solved_rejects_an_invalid_tier_and_too_many_placements() {
    let base = Design::concave_fixture();
    let solved = base.solve().expect("the fixture pins every mast");

    let mut invalid = base.clone();
    invalid.concave_tiers[1].diameter_ratio = 0.0;
    assert!(matches!(
        invalid.concave_tools_from_solved(&solved),
        Err(ConcaveResolveError::Invalid { tier: 1, .. })
    ));

    let mut crowded = base;
    crowded.concave_tiers[0].indices = vec![0.0; MAX_TOOL_PRIMITIVES];
    // 128 + the dimple's 4 placements.
    assert_eq!(
        crowded.concave_tools_from_solved(&solved),
        Err(ConcaveResolveError::TooManyPlacements {
            count: MAX_TOOL_PRIMITIVES + 4,
            max: MAX_TOOL_PRIMITIVES,
        })
    );
}

#[test]
fn the_fixture_is_a_valid_design_with_two_concave_tiers() {
    let design = Design::concave_fixture();
    design
        .validate_concave_tiers()
        .expect("the fixture is valid");
    assert_eq!(design.concave_tiers.len(), 2);
    assert_eq!(design.concave_tier_ids.len(), 2);
    assert_eq!(design.concave_placement_count(), 12);
    let ConcaveTier { name, indices, .. } = &design.concave_tiers[0];
    assert_eq!(name, "Groove");
    assert_eq!(indices.len(), 8);
}

#[test]
fn facet_geometry_drops_the_preform_planes_and_places_tools_on_the_facet_stone() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let preform_count = design.preform.planes().len();
    let (all_planes, all_tools, _) = design.geometry_from_solved(&solved).expect("resolves");
    let (facets, tools) = design
        .facet_geometry_from_solved(&solved)
        .expect("resolves");
    assert_eq!(facets, all_planes[preform_count..]);
    assert_eq!(tools.len(), 12);
    // The fixture's facets close inside its preform, so the preform changes
    // nothing about where the tools go.
    assert_eq!(tools, all_tools);

    // A preform that trims the stone moves the contact points and the width, so
    // the editor's tools differ from the facet stone's; the facet-only rule
    // ignores the preform entirely.
    let mut trimmed = design;
    trimmed.preform = PreformSpec::block(0.4, 0.4, 0.4);
    let (_, trimmed_all_tools, _) = trimmed.geometry_from_solved(&solved).expect("resolves");
    let (trimmed_facets, trimmed_tools) = trimmed
        .facet_geometry_from_solved(&solved)
        .expect("resolves");
    assert_ne!(trimmed_all_tools, all_tools, "the trimming preform matters");
    assert_eq!(trimmed_tools, all_tools, "the facet-only tools do not");
    assert_eq!(trimmed_facets, facets);
}

#[test]
fn facet_geometry_of_a_planar_design_is_its_facet_planes_alone() {
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let solved = design.solve().expect("every tier is pinned");
    let (facets, tools) = design.facet_geometry_from_solved(&solved).expect("planar");
    assert_eq!(tools, [] as [ToolPrimitive; 0]);
    assert_eq!(
        facets,
        design.planes_from_solved(&solved)[design.preform.planes().len()..]
    );
}

#[test]
fn facet_geometry_of_an_open_facet_stone_has_no_tools_but_bad_tiers_still_fail() {
    let mut design = Design::concave_fixture();
    // Only the girdle tier's facets: no crown or pavilion, so they cannot close.
    design.tiers.truncate(1);
    let solved = design.solve().expect("one pinned tier");
    let (_, tools) = design
        .facet_geometry_from_solved(&solved)
        .expect("an open stone is not an error");
    assert_eq!(tools, [] as [ToolPrimitive; 0]);

    design.concave_tiers[0].diameter_ratio = 0.0;
    assert!(matches!(
        design.facet_geometry_from_solved(&solved),
        Err(ConcaveResolveError::Invalid { tier: 0, .. })
    ));
}

/// A tier that passes its own validation can still narrow to an unusable `f32`
/// primitive (a one-in-1e300 degree cone is infinitely tall); the resolver refuses
/// it instead of handing the kernel an infinite tool.
#[test]
fn the_resolver_maps_a_failed_primitive_validation_to_invalid() {
    let mut design = Design::concave_fixture();
    {
        let tier = &mut design.concave_tiers[0];
        tier.tool = ConcaveTool::Cone;
        tier.tool_angle_deg = Some(1e-300);
    }
    design.concave_tiers[0]
        .validate(design.meta.gear_teeth)
        .expect("valid tier");
    let solved = design.solve().expect("the fixture pins every mast");
    assert!(matches!(
        design.concave_tools_from_solved(&solved),
        Err(ConcaveResolveError::Invalid {
            tier: 0,
            error: crate::design::ConcaveTierError::ToolGeometry { .. }
        })
    ));
    // A huge ratio is refused by validation before any geometry is built.
    let mut huge = Design::concave_fixture();
    huge.concave_tiers[1].diameter_ratio = 1e300;
    assert!(matches!(
        huge.concave_tools_from_solved(&solved),
        Err(ConcaveResolveError::Invalid {
            tier: 1,
            error: crate::design::ConcaveTierError::DiameterTooLarge { .. }
        })
    ));
}

/// The scoring paths take their tools from the flat planes they already hold: the
/// same primitives the solve-based resolver gives, nothing for a flat design, and the
/// flat stone (no tools) when the tiers do not resolve.
#[test]
fn tools_for_scoring_are_the_tools_of_the_solve_and_empty_without_them() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let (from_solved, placements) = design
        .concave_tools_from_solved(&solved)
        .expect("the fixture resolves");
    let (from_planes, planes_placements) = design
        .concave_tools_from_planes(&planes)
        .expect("the fixture resolves");
    assert_eq!(from_planes, from_solved);
    assert_eq!(planes_placements, placements);
    assert_eq!(design.tools_for_scoring(&planes), from_solved);
    assert_eq!(from_solved.len(), 12);

    let flat = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    let flat_solved = flat.solve().expect("every tier is pinned");
    let flat_planes = flat.planes_from_solved(&flat_solved);
    assert_eq!(flat.tools_for_scoring(&flat_planes).len(), 0);

    let mut invalid = design;
    invalid.concave_tiers[1].diameter_ratio = 0.0;
    assert!(
        invalid.concave_tools_from_planes(&planes).is_err(),
        "the tier does not resolve"
    );
    assert!(
        invalid.tools_for_scoring(&planes).is_empty(),
        "so the stone is scored flat"
    );
}

/// Tiers held outside a design (a library record's angle-table rows) are placed by the
/// very rule a design applies.
#[test]
fn loose_tiers_resolve_to_the_tools_a_design_holding_them_makes() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture pins every mast");
    let planes = design.planes_from_solved(&solved);
    let (tools, placements) = design
        .concave_tools_from_planes(&planes)
        .expect("the fixture resolves");
    // The fixture stores its tiers in cutting order, so the loose order is the same.
    let (loose, loose_placements) = super::resolve_tiers_on_planes(
        &planes,
        &design.concave_tiers,
        design.meta.gear_teeth,
        design.meta.gear_reference_angle,
    )
    .expect("the fixture's tiers resolve");
    assert_eq!(loose, tools);
    assert_eq!(loose_placements, placements);

    let mut bad = design.concave_tiers.clone();
    bad[0].diameter_ratio = 0.0;
    assert!(matches!(
        super::resolve_tiers_on_planes(
            &planes,
            &bad,
            design.meta.gear_teeth,
            design.meta.gear_reference_angle
        ),
        Err(ConcaveResolveError::Invalid { tier: 0, .. })
    ));

    let mut crowded = design.concave_tiers.clone();
    crowded[0].indices = vec![0.0; MAX_TOOL_PRIMITIVES];
    assert_eq!(
        super::resolve_tiers_on_planes(
            &planes,
            &crowded,
            design.meta.gear_teeth,
            design.meta.gear_reference_angle
        ),
        Err(ConcaveResolveError::TooManyPlacements {
            count: MAX_TOOL_PRIMITIVES + 4,
            max: MAX_TOOL_PRIMITIVES,
        })
    );

    assert_eq!(
        super::resolve_tiers_on_planes(
            &[],
            &design.concave_tiers,
            design.meta.gear_teeth,
            design.meta.gear_reference_angle
        ),
        Err(ConcaveResolveError::NoFlatSolid),
        "a stone that does not close has no width to place the tools against"
    );
}

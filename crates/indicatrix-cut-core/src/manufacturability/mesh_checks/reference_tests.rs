//! The concave-tool check (check 6) against the straightforward version it replaced.
//!
//! The reference below is the implementation as it was before the bounding-box pruning:
//! every mesh is built from ALL the stone planes and ALL the tool planes, and a pair of
//! tools is pre-tested with two bounding spheres. The pruned check must give the same
//! warnings, so the tests compare the two on the fixtures and on a set of hand-made stones
//! covering every kind of warning. The only difference allowed is the new
//! `ToolEnclosed`, which the reference never reports.

use super::*;
use crate::{
    design::{
        Design,
        concave_frame::{dop_frame, support_vertex},
    },
    manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
};
use glam::Vec3;
use indicatrix::geometry::tool::ToolSweep;

/// A sphere that certainly contains `tool` with its sweep: centre, and the sum of its
/// largest radius, half-length and half-stroke.
fn reference_bounding_sphere(tool: &ToolPrimitive) -> (DVec3, f64) {
    let centre = DVec3::new(
        f64::from(tool.origin[0]),
        f64::from(tool.origin[1]),
        f64::from(tool.origin[2]),
    );
    let radius = tool.origin[3].max(tool.profile[0]).max(tool.profile[1]);
    (centre, f64::from(radius + tool.axis[3] + tool.profile[2]))
}

/// The check as it was: full plane lists, bounding-sphere pair pre-test, no `ToolEnclosed`.
fn reference_warnings(
    planes: &[(DVec3, f64)],
    preform_len: usize,
    tools: &[ToolPrimitive],
    placements: &[(usize, usize)],
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let Some((metrics, vertices)) = measure_solid_with_vertices(planes) else {
        return warnings;
    };
    if tools.is_empty() {
        return warnings;
    }
    let area_threshold = min_facet_area_fraction_of_w2 * metrics.width_axis * metrics.width_axis;
    let min_removed = REMOVED_VOLUME_FRACTION * metrics.volume;
    let eps = VERTEX_EPS * (1.0 + metrics.width_axis);
    let lo = vertices
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |a, &v| a.min(v));
    let hi = vertices
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |a, &v| a.max(v));
    let clipped = |polytopes: &[&[(DVec3, f64)]]| {
        let mut all = planes.to_vec();
        for polytope in polytopes {
            all.extend_from_slice(polytope);
        }
        match build_solid_mesh(&all) {
            SolidStatus::Closed(mesh) => Some(mesh),
            _ => None,
        }
    };

    let polytopes: Vec<Vec<(DVec3, f64)>> = tools
        .iter()
        .map(|tool| tessellate_tool(tool, TOOL_SEGMENTS))
        .collect();
    for (k, polytope) in polytopes.iter().enumerate() {
        let (tier, placement) = placements[k];
        let removed = if polytope.is_empty() {
            None
        } else {
            clipped(&[polytope]).filter(|mesh| mesh_volume(mesh) > min_removed)
        };
        let Some(removed) = removed else {
            warnings.push(ManufacturabilityWarning::ToolMissesStone { tier, placement });
            continue;
        };

        let touched: Vec<usize> = facet_areas(&removed)
            .into_iter()
            .filter(|&(id, area)| id < planes.len() && area >= area_threshold)
            .map(|(id, _)| id)
            .collect();
        let stroke_axis = (tools[k].sweep_kind != 0).then(|| tool_axis_f64(&tools[k]));
        if let Some(facet) = broken_through_facet(planes, &touched, stroke_axis) {
            warnings.push(ManufacturabilityWarning::ToolBreaksThrough {
                tier,
                placement,
                facet,
            });
        }

        let mut removes_hull_vertex = false;
        for (vertex, &v) in vertices.iter().enumerate() {
            if !polytope.iter().all(|&(n, m)| n.dot(v) - m < -eps) {
                continue;
            }
            let meeting = planes[preform_len.min(planes.len())..]
                .iter()
                .filter(|&&(n, m)| (n.dot(v) - m).abs() <= eps)
                .count();
            if meeting >= 3 {
                warnings.push(ManufacturabilityWarning::ToolRemovesMeet {
                    tier,
                    placement,
                    vertex,
                });
            }
            removes_hull_vertex |= (0..3)
                .any(|axis| (v[axis] - lo[axis]).abs() <= eps || (v[axis] - hi[axis]).abs() <= eps);
        }
        if removes_hull_vertex {
            warnings.push(ManufacturabilityWarning::ToolRemovesHullVertex { tier, placement });
        }
    }

    let spheres: Vec<(DVec3, f64)> = tools.iter().map(reference_bounding_sphere).collect();
    let coarse: Vec<Vec<(DVec3, f64)>> = tools
        .iter()
        .map(|tool| tessellate_tool(tool, OVERLAP_SEGMENTS))
        .collect();
    for a in 0..tools.len() {
        for b in a + 1..tools.len() {
            let ((ca, ra), (cb, rb)) = (spheres[a], spheres[b]);
            if coarse[a].is_empty() || coarse[b].is_empty() || ca.distance(cb) > ra + rb {
                continue;
            }
            if clipped(&[&coarse[a], &coarse[b]])
                .is_some_and(|mesh| mesh_volume(&mesh) > min_removed)
            {
                warnings.push(ManufacturabilityWarning::ToolsOverlap { a, b });
            }
        }
    }
    warnings.extend(sliver_warnings(planes, tools, area_threshold));
    warnings
}

/// `|x|, |y|, |z| <= 1`, in the order x, -x, y, -y, z, -z.
fn unit_cube() -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 1.0),
        (DVec3::NEG_Y, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]
}

/// The pruned check's warnings without the new `ToolEnclosed`, which the reference lacks.
fn pruned_without_enclosed(
    planes: &[(DVec3, f64)],
    preform_len: usize,
    tools: &[ToolPrimitive],
    placements: &[(usize, usize)],
) -> Vec<ManufacturabilityWarning> {
    concave_tool_warnings(
        planes,
        preform_len,
        tools,
        placements,
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    )
    .into_iter()
    .filter(|warning| !matches!(warning, ManufacturabilityWarning::ToolEnclosed { .. }))
    .collect()
}

fn placements_of(count: usize) -> Vec<(usize, usize)> {
    (0..count).map(|k| (k / 4, k % 4)).collect()
}

/// A scenario per kind of warning (and some that raise none), each a tool list on the unit cube.
fn hand_made_stones() -> Vec<(&'static str, Vec<ToolPrimitive>)> {
    let across = Vec3::X;
    vec![
        (
            "ball outside",
            vec![ToolPrimitive::ball(Vec3::new(5.0, 0.0, 0.0), 0.5)],
        ),
        (
            "ball on a facet",
            vec![ToolPrimitive::ball(Vec3::new(0.0, 0.0, 1.0), 0.4)],
        ),
        (
            "ball on the corner",
            vec![ToolPrimitive::ball(Vec3::new(1.0, 1.0, 1.0), 0.5)],
        ),
        (
            "through hole",
            vec![ToolPrimitive::cylinder(Vec3::ZERO, Vec3::Z, 0.3, 2.0)],
        ),
        (
            "reciprocating groove on a side",
            vec![
                ToolPrimitive::cylinder(Vec3::new(0.0, 1.0, 0.0), Vec3::X, 0.25, 0.5).with_sweep(
                    ToolSweep::AlongAxis,
                    0.4,
                    Vec3::ZERO,
                ),
            ],
        ),
        (
            "across sweep",
            vec![
                ToolPrimitive::cylinder(Vec3::new(0.0, 0.0, 1.0), Vec3::Z, 0.2, 0.3).with_sweep(
                    ToolSweep::AcrossAxis,
                    0.5,
                    across,
                ),
            ],
        ),
        (
            "cone and bicone",
            vec![
                ToolPrimitive::frustum(Vec3::new(0.0, 0.8, 0.0), Vec3::Y, 0.4, 0.1, 0.5),
                ToolPrimitive::bicone(Vec3::new(0.9, 0.0, 0.0), Vec3::X, 0.5, 0.6),
            ],
        ),
        (
            "two balls at one spot",
            vec![
                ToolPrimitive::ball(Vec3::ZERO, 0.3),
                ToolPrimitive::ball(Vec3::ZERO, 0.3),
            ],
        ),
        (
            "two balls that just overlap",
            vec![
                ToolPrimitive::ball(Vec3::new(-0.2, 0.0, 1.0), 0.3),
                ToolPrimitive::ball(Vec3::new(0.2, 0.0, 1.0), 0.3),
            ],
        ),
        (
            "two balls that miss each other",
            vec![
                ToolPrimitive::ball(Vec3::new(-0.6, 0.0, 1.0), 0.2),
                ToolPrimitive::ball(Vec3::new(0.6, 0.0, 1.0), 0.2),
            ],
        ),
        (
            "a ring of grooves",
            (0..8)
                .map(|k| {
                    let angle = std::f32::consts::TAU * k as f32 / 8.0;
                    let dir = Vec3::new(angle.cos(), angle.sin(), 0.0);
                    ToolPrimitive::cylinder(dir * 0.9, Vec3::Z, 0.12, 0.6).with_sweep(
                        ToolSweep::AlongAxis,
                        0.3,
                        Vec3::ZERO,
                    )
                })
                .collect(),
        ),
    ]
}

#[test]
fn the_pruned_check_gives_the_reference_warnings_on_hand_made_stones() {
    let planes = unit_cube();
    for (label, tools) in hand_made_stones() {
        let placements = placements_of(tools.len());
        let reference = reference_warnings(
            &planes,
            0,
            &tools,
            &placements,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        );
        assert_eq!(
            pruned_without_enclosed(&planes, 0, &tools, &placements),
            reference,
            "{label}"
        );
    }
}

#[test]
fn the_pruned_check_gives_the_reference_warnings_on_the_concave_fixture_and_its_variants() {
    let base = Design::concave_fixture();
    let mut variants = vec![("fixture", base.clone())];
    let mut wider = base.clone();
    wider.concave_tiers[0].diameter_ratio = 0.9;
    variants.push(("wide grooves", wider));
    let mut deep = base.clone();
    deep.concave_tiers[1].displacement[2] = 0.3;
    deep.concave_tiers[1].diameter_ratio = 0.5;
    variants.push(("deep large dimples", deep));
    let mut outside = base;
    outside.concave_tiers[1].displacement = [3.0, 0.0, 0.0];
    variants.push(("dimples far off", outside));
    for (label, design) in variants {
        let solved = design.solve().expect("the fixture solves");
        let (planes, tools, placements) = design
            .geometry_from_solved(&solved)
            .expect("the tiers resolve");
        let preform_len = design.preform.planes().len();
        let reference = reference_warnings(
            &planes,
            preform_len,
            &tools,
            &placements,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        );
        assert_eq!(
            pruned_without_enclosed(&planes, preform_len, &tools, &placements),
            reference,
            "{label}"
        );
        // Deterministic: the same inputs give the same list, in the same order.
        assert_eq!(
            concave_tool_warnings(
                &planes,
                preform_len,
                &tools,
                &placements,
                DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2
            ),
            concave_tool_warnings(
                &planes,
                preform_len,
                &tools,
                &placements,
                DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2
            ),
            "{label}"
        );
    }
}

/// F3-5: a tool that removes volume but never reaches a flat facet is an internal void.
#[test]
fn a_tool_wholly_inside_the_stone_warns_it_is_enclosed() {
    let planes = unit_cube();
    for tool in [
        ToolPrimitive::ball(Vec3::new(0.1, 0.0, 0.2), 0.3),
        ToolPrimitive::cylinder(Vec3::ZERO, Vec3::Y, 0.2, 0.4),
        ToolPrimitive::cylinder(Vec3::ZERO, Vec3::X, 0.2, 0.3).with_sweep(
            ToolSweep::AlongAxis,
            0.3,
            Vec3::ZERO,
        ),
    ] {
        let warnings = concave_tool_warnings(
            &planes,
            0,
            &[tool],
            &[(1, 2)],
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        );
        assert_eq!(
            warnings,
            vec![ManufacturabilityWarning::ToolEnclosed {
                tier: 1,
                placement: 2
            }],
            "{tool:?}"
        );
        assert_eq!(warnings[0].concave_tier(), Some(1));
        let text = warnings[0].to_string();
        assert!(
            text.contains("concave tier 2 placement 3") && text.contains("inside the stone"),
            "{text}"
        );
    }
    // The same ball grown until it reaches the +z facet is a plain cut, not a void.
    let reaching = ToolPrimitive::ball(Vec3::new(0.1, 0.0, 0.8), 0.3);
    let warnings = concave_tool_warnings(
        &planes,
        0,
        &[reaching],
        &[(0, 0)],
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    );
    assert!(
        !warnings
            .iter()
            .any(|w| matches!(w, ManufacturabilityWarning::ToolEnclosed { .. })),
        "{warnings:?}"
    );
}

/// The `[X, Y, Z]` displacement that puts concave tier `tier`'s first tool at `target`.
///
/// It inverts `tool_centre` (`c0 + W * (X u + Y v - Z n)`) with the very frame and contact
/// point the resolver uses.
fn displacement_to(design: &Design, solved: &[SolvedTier], tier: usize, target: DVec3) -> [f64; 3] {
    let (metrics, vertices) = measure_solid_with_vertices(&design.planes_from_solved(solved))
        .expect("the fixture closes");
    let concave = &design.concave_tiers[tier];
    let (u, v, n) = dop_frame(
        concave.angle_deg,
        concave.indices[0],
        design.meta.gear_reference_angle,
        design.meta.gear_teeth,
    );
    let contact = support_vertex(&vertices, n, metrics.width_axis).expect("the stone has corners");
    let offset = (target - contact) / metrics.width_axis;
    [offset.dot(u), offset.dot(v), -offset.dot(n)]
}

/// F3-5 through the design API: an enclosed dimple is reported against its tier, per placement.
///
/// The position is computed, not scanned. A dop-frame tool starts at the stone's contact point
/// for its facet normal and moves `Z * W` along `-n`. On the fixture that contact point is the
/// knife-edge girdle corner (about `(0.877, -0.018, -0.032)`), where `-n` already points out
/// through the pavilion facet: the ball's centre is outside the stone at every depth from `0.04`
/// to `0.6` (the stone is `1.11` deep along `n`, `W` is `1.754`), so no `Z` alone ever gives an
/// enclosed dimple. `X` and `Y` carry the tool into the body; `displacement_to` solves for them.
#[test]
fn an_enclosed_concave_tier_is_reported_by_the_design_check() {
    let fixture = Design::concave_fixture();
    let solved = fixture.solve().expect("the fixture solves");
    assert!(
        !check_concave_tools(&fixture, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .iter()
            .any(|w| matches!(w, ManufacturabilityWarning::ToolEnclosed { .. })),
        "the fixture's own tools all cut the surface"
    );

    // Inside the stone, which spans +-0.877 in x and z and -0.70 .. 0.52 in y (mast units),
    // a third of the way out from its axis at mid-height.
    let mut design = fixture;
    design.concave_tiers[1].displacement =
        displacement_to(&design, &solved, 1, DVec3::new(0.3, -0.05, 0.0));

    // The premise: every dimple ball clears every facet by well over its own radius.
    let (planes, tools, placements) = design
        .geometry_from_solved(&solved)
        .expect("the tiers resolve");
    let dimples: Vec<_> = tools
        .iter()
        .zip(&placements)
        .filter(|&(_, &(tier, _))| tier == 1)
        .collect();
    assert_eq!(dimples.len(), 4, "one ball per dimple placement");
    for (tool, &(_, placement)) in dimples {
        let centre = DVec3::new(
            f64::from(tool.origin[0]),
            f64::from(tool.origin[1]),
            f64::from(tool.origin[2]),
        );
        let radius = f64::from(tool.origin[3]);
        let clearance = planes
            .iter()
            .map(|&(normal, offset)| (offset - normal.dot(centre)) / normal.length())
            .fold(f64::INFINITY, f64::min);
        assert!(
            clearance > 2.0 * radius,
            "placement {placement}: the ball (radius {radius}) at {centre:?} is only {clearance} \
             inside the nearest facet, so the fixture does not enclose it"
        );
    }

    let enclosed: Vec<_> =
        check_concave_tools(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter(|w| matches!(w, ManufacturabilityWarning::ToolEnclosed { .. }))
            .collect();
    let expected: Vec<_> = (0..4)
        .map(|placement| ManufacturabilityWarning::ToolEnclosed { tier: 1, placement })
        .collect();
    assert_eq!(
        enclosed, expected,
        "one per dimple placement, none for the grooves"
    );
}

/// A hash collision must not hand one stone another's warnings: a cache hit needs every
/// input equal, not only the 64-bit hash.
#[test]
fn a_cache_hit_needs_every_input_to_match_not_only_the_hash() {
    let threshold = DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2;
    let key = ConcaveWarningsKey::new(&unit_cube(), 0, &[], &[], threshold);
    let stored = vec![ManufacturabilityWarning::ToolMissesStone {
        tier: 0,
        placement: 0,
    }];
    let cache = vec![(key.clone(), stored.clone())];
    assert_eq!(cached_concave_warnings(&cache, &key), Some(stored));

    // Another stone under the same hash. A real 64-bit collision cannot be searched for, so
    // the key is forged: same hash, one input bit different.
    let mut collider = key.clone();
    collider.inputs[4] ^= 1;
    assert_eq!(collider.hash, key.hash);
    assert_eq!(cached_concave_warnings(&cache, &collider), None);
}

/// Every input the warnings read is in the key, so a change to any of them is a miss.
#[test]
fn the_cache_key_sees_every_input() {
    let threshold = DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2;
    let ball = |radius: f32| ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), radius);
    let base = ConcaveWarningsKey::new(&unit_cube(), 0, &[ball(0.1)], &[(0, 0)], threshold);
    assert_eq!(
        base,
        ConcaveWarningsKey::new(&unit_cube(), 0, &[ball(0.1)], &[(0, 0)], threshold),
        "the same inputs give the same key"
    );
    let mut moved = unit_cube();
    moved[0].1 = 1.5;
    let others = [
        ConcaveWarningsKey::new(&moved, 0, &[ball(0.1)], &[(0, 0)], threshold),
        ConcaveWarningsKey::new(&unit_cube(), 1, &[ball(0.1)], &[(0, 0)], threshold),
        ConcaveWarningsKey::new(&unit_cube(), 0, &[ball(0.2)], &[(0, 0)], threshold),
        ConcaveWarningsKey::new(&unit_cube(), 0, &[], &[], threshold),
        ConcaveWarningsKey::new(&unit_cube(), 0, &[ball(0.1)], &[(0, 1)], threshold),
        ConcaveWarningsKey::new(&unit_cube(), 0, &[ball(0.1)], &[(0, 0)], 2.0 * threshold),
    ];
    for (position, other) in others.iter().enumerate() {
        assert_ne!(*other, base, "input {position} is not in the key");
    }
}

/// A stone asked about from another thread gives the same answer (the cache is shared and
/// must never change a result).
#[test]
fn the_warnings_are_the_same_on_another_thread() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture solves");
    let here = check_concave_tools(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
    let there = std::thread::scope(|scope| {
        scope
            .spawn(|| check_concave_tools(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2))
            .join()
            .expect("the check does not panic")
    });
    assert_eq!(here, there);
}

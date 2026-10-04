//! [`build_solid_mesh_geom`] coverage: the planar path is untouched, and
//! tool-carved meshes have the right volume, are closed, wind outward and
//! draw only facet boundaries.

use std::f64::consts::{PI, TAU};

use glam::{DVec3, Vec3};

use super::super::concave::polytope_volume;
use crate::geometry::{
    stone_metrics::{
        SolidMesh, SolidStatus, TOOL_ICOSPHERE_LEVEL, TOOL_SEGMENTS, build_solid_mesh,
        build_solid_mesh_geom, mesh_volume, tessellate_tool,
    },
    tool::ToolPrimitive,
};

/// Axis-aligned box `|x| <= hx, |y| <= hy, |z| <= hz`. The half-extents used
/// below are multiples of `2^-1`, so they sit exactly on the snap grid.
fn slab(hx: f64, hy: f64, hz: f64) -> Vec<(DVec3, f64)> {
    vec![
        (DVec3::X, hx),
        (DVec3::NEG_X, hx),
        (DVec3::Y, hy),
        (DVec3::NEG_Y, hy),
        (DVec3::Z, hz),
        (DVec3::NEG_Z, hz),
    ]
}

fn carve(planes: &[(DVec3, f64)], tools: &[ToolPrimitive]) -> SolidMesh {
    match build_solid_mesh_geom(planes, tools) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("expected a closed concave mesh, got {other:?}"),
    }
}

fn bits(v: DVec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

fn assert_bit_identical(a: &SolidMesh, b: &SolidMesh, label: &str) {
    let vs = |m: &SolidMesh| m.positions.iter().copied().map(bits).collect::<Vec<_>>();
    let ns = |m: &SolidMesh| m.normals.iter().copied().map(bits).collect::<Vec<_>>();
    let rs = |m: &SolidMesh| {
        m.rings
            .iter()
            .map(|(f, r)| (*f, r.iter().copied().map(bits).collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    };
    assert_eq!(vs(a), vs(b), "{label}: positions");
    assert_eq!(ns(a), ns(b), "{label}: normals");
    assert_eq!(a.facet_id, b.facet_id, "{label}: facet ids");
    assert_eq!(a.indices, b.indices, "{label}: indices");
    assert_eq!(rs(a), rs(b), "{label}: rings");
    assert!(
        b.piece_normals.is_none() && b.edge_visible.is_none(),
        "{label}: the planar path must leave the concave fields empty"
    );
}

#[test]
fn build_solid_mesh_geom_with_no_tools_equals_build_solid_mesh() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let fixtures = [
        ("plain box", slab(1.0, 0.6, 1.0)),
        (
            "hip-roofed block",
            vec![
                (DVec3::X, 1.0),
                (DVec3::NEG_X, 1.0),
                (DVec3::Z, 1.0),
                (DVec3::NEG_Z, 1.0),
                (DVec3::NEG_Y, 0.5),
                (DVec3::new(s, s, 0.0), s),
                (DVec3::new(-s, s, 0.0), s),
                (DVec3::new(0.0, s, s), s),
                (DVec3::new(0.0, s, -s), s),
            ],
        ),
    ];
    for (label, planes) in &fixtures {
        let (SolidStatus::Closed(want), SolidStatus::Closed(got)) =
            (build_solid_mesh(planes), build_solid_mesh_geom(planes, &[]))
        else {
            panic!("{label}: fixture must close");
        };
        assert_bit_identical(&want, &got, label);
    }
}

#[test]
fn tessellated_cylinder_groove_through_slab_matches_closed_form() {
    let planes = slab(2.0, 0.5, 2.0);
    // Axis along the slab's thickness, ends well outside it.
    let tool = ToolPrimitive::cylinder(Vec3::new(0.3, 0.0, 0.2), Vec3::Y, 0.7, 1.0);
    let mesh = carve(&planes, &[tool]);
    let r = f64::from(0.7_f32);
    let want = (PI * r).mul_add(-r, 16.0);
    let got = mesh_volume(&mesh);
    assert!(
        ((got - want) / want).abs() < 1e-9,
        "volume {got} vs closed form {want}"
    );
}

/// Area of the part of a disc of radius `rho` beyond a chord at distance `d`
/// from its centre.
fn circular_segment_area(rho: f64, d: f64) -> f64 {
    d.mul_add(
        -d.mul_add(-d, rho * rho).sqrt(),
        rho * rho * (d / rho).acos(),
    )
}

#[test]
fn tessellated_partial_cylinder_groove_is_within_polygon_bound() {
    let planes = slab(2.0, 1.0, 0.5);
    let r = 0.5_f64;
    // Axis centre 0.4 r above the top face: the groove is 0.6 r deep.
    let cy = f32::from(1u8) + 0.2;
    let tool = ToolPrimitive::cylinder(Vec3::new(0.0, cy, 0.0), Vec3::Z, 0.5, 1.0);
    let mesh = carve(&planes, &[tool]);
    let got = mesh_volume(&mesh);

    let n = TOOL_SEGMENTS as f64;
    let circum = (TAU / (n * (TAU / n).sin())).sqrt();
    let apothem = circum * (PI / n).cos();
    let delta = f64::from(cy) - 1.0;
    // The polygon contains the circle of its apothem and lies inside the
    // circle of its circumradius, so the removed area (times the 1.0 thick
    // slab) is bracketed by the two circular segments.
    let lo = circular_segment_area(apothem * r, delta);
    let hi = circular_segment_area(circum * r, delta);
    let full = 4.0 * 2.0 * 1.0;
    let slack = 1e-9 * full;
    assert!(
        full - hi - slack <= got && got <= full - lo + slack,
        "volume {got} outside [{}, {}]",
        full - hi,
        full - lo
    );
    // And the bracket is tight: it bounds the true groove to well under 1 %.
    let exact = circular_segment_area(r, delta);
    assert!((hi - lo) / exact < 0.01, "bracket {lo}..{hi} vs {exact}");
}

#[test]
fn cube_minus_interior_ball_matches_icosphere_volume_constant() {
    let planes = slab(1.0, 1.0, 1.0);
    let tool = ToolPrimitive::ball(Vec3::new(0.1, -0.05, 0.2), 0.5);
    let constant = polytope_volume(&tool, TOOL_SEGMENTS);
    let sphere = 4.0 * PI * 0.5_f64.powi(3) / 3.0;
    println!(
        "icosphere level {TOOL_ICOSPHERE_LEVEL} scaled volume constant = {constant} (sphere {sphere})"
    );
    assert!(
        ((constant - sphere) / sphere).abs() < 0.01,
        "constant {constant} vs sphere {sphere}"
    );
    let got = mesh_volume(&carve(&planes, &[tool]));
    assert!(
        ((got - (8.0 - constant)) / constant).abs() < 1e-9,
        "volume {got} vs {}",
        8.0 - constant
    );
}

/// A slab with a through groove, two overlapping dimples and a swept tool: the
/// fixture for the closure, winding and order tests.
fn busy_fixture() -> (Vec<(DVec3, f64)>, Vec<ToolPrimitive>) {
    let tools =
        vec![
            ToolPrimitive::cylinder(Vec3::new(-0.8, 0.0, 0.1), Vec3::Y, 0.5, 1.5),
            ToolPrimitive::ball(Vec3::new(2.0, 0.2, 0.1), 0.6),
            ToolPrimitive::ball(Vec3::new(1.7, 0.4, 0.3), 0.5),
            ToolPrimitive::frustum(Vec3::new(0.4, -1.0, -0.4), Vec3::Y, 0.15, 0.45, 0.3)
                .with_sweep(crate::geometry::tool::ToolSweep::AlongAxis, 0.1, Vec3::X),
        ];
    (slab(2.0, 1.0, 1.0), tools)
}

/// Every directed edge must have a reversed twin once edges are split at the
/// mesh vertices lying on them (the decomposition's T-junctions), matching
/// endpoints within `tol`. This is the closedness the divergence volume needs.
fn assert_closed(mesh: &SolidMesh, tol: f64) {
    let mut corners: Vec<DVec3> = mesh
        .rings
        .iter()
        .flat_map(|(_, r)| r.iter().copied())
        .collect();
    corners.sort_by(|a, b| a.x.total_cmp(&b.x));
    let mut edges: Vec<(DVec3, DVec3)> = Vec::new();
    for (_, ring) in &mesh.rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let ab = b - a;
            let len2 = ab.length_squared();
            if len2 < tol * tol {
                continue;
            }
            let lo = corners.partition_point(|c| c.x < a.x.min(b.x) - tol);
            let hi = corners.partition_point(|c| c.x <= a.x.max(b.x) + tol);
            let mut cuts: Vec<(f64, DVec3)> = vec![(0.0, a), (1.0, b)];
            for &p in &corners[lo..hi] {
                let t = (p - a).dot(ab) / len2;
                if t > 0.0 && t < 1.0 && (p - (a + ab * t)).length() <= tol {
                    cuts.push((t, p));
                }
            }
            cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
            for w in cuts.windows(2) {
                if (w[1].1 - w[0].1).length() > tol {
                    edges.push((w[0].1, w[1].1));
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by(|&i, &j| edges[i].0.x.total_cmp(&edges[j].0.x));
    let starts: Vec<f64> = order.iter().map(|&i| edges[i].0.x).collect();
    let mut used = vec![false; edges.len()];
    for i in 0..edges.len() {
        if used[i] {
            continue;
        }
        let (a, b) = edges[i];
        let lo = starts.partition_point(|&x| x < b.x - tol);
        let hi = starts.partition_point(|&x| x <= b.x + tol);
        let twin = order[lo..hi].iter().copied().find(|&j| {
            j != i
                && !used[j]
                && (edges[j].0 - b).length() <= tol
                && (edges[j].1 - a).length() <= tol
        });
        let Some(j) = twin else {
            panic!("edge {a:?} -> {b:?} has no reversed twin: the mesh is open");
        };
        used[i] = true;
        used[j] = true;
    }
}

#[test]
fn concave_mesh_is_closed() {
    let (planes, tools) = busy_fixture();
    let mesh = carve(&planes, &tools);
    let width = 4.0;
    assert_closed(&mesh, 5e-9 * width);
    // Independent of edge matching: a closed surface has zero total vector area.
    let mut flux = DVec3::ZERO;
    for (_, ring) in &mesh.rings {
        for w in ring[1..].windows(2) {
            flux += 0.5 * (w[0] - ring[0]).cross(w[1] - ring[0]);
        }
    }
    assert!(
        flux.length() < 1e-9 * width * width,
        "net area vector {flux:?}"
    );
}

#[test]
fn concave_mesh_volume_is_independent_of_tool_order() {
    let planes = slab(1.5, 1.5, 1.5);
    let t1 = ToolPrimitive::ball(Vec3::new(0.3, 0.0, 0.0), 0.6);
    let t2 = ToolPrimitive::ball(Vec3::new(-0.2, 0.1, 0.1), 0.6);
    let t3 = ToolPrimitive::cylinder(Vec3::new(0.1, 0.2, 0.0), Vec3::Z, 0.4, 2.0);
    let base = mesh_volume(&carve(&planes, &[t1, t2, t3]));
    for order in [[t3, t2, t1], [t2, t3, t1], [t3, t1, t2]] {
        let v = mesh_volume(&carve(&planes, &order));
        assert!(
            ((v - base) / base).abs() < 1e-9,
            "volume {v} differs from {base}"
        );
    }
}

#[test]
fn concave_mesh_piece_windings_point_out_of_the_stone() {
    let (planes, tools) = busy_fixture();
    let mesh = carve(&planes, &tools);
    let normals = mesh
        .piece_normals
        .as_ref()
        .expect("concave path fills normals");
    assert_eq!(normals.len(), mesh.rings.len());
    for (i, (facet, ring)) in mesh.rings.iter().enumerate() {
        let mut area = DVec3::ZERO;
        for w in ring[1..].windows(2) {
            area += (w[0] - ring[0]).cross(w[1] - ring[0]);
        }
        assert!(
            area.dot(normals[i]) > 0.0,
            "ring {i} (facet {facet}) winds against its outward normal"
        );
    }
}

#[test]
fn edge_visibility_hides_tessellation_seams_and_shows_facet_tool_boundaries() {
    let planes = slab(2.0, 0.5, 2.0);
    let tool = ToolPrimitive::cylinder(Vec3::new(0.3, 0.0, 0.2), Vec3::Y, 0.7, 1.0);
    let mesh = carve(&planes, &[tool]);
    let visible = mesh
        .edge_visible
        .as_ref()
        .expect("concave path fills visibility");
    assert_eq!(visible.len(), mesh.rings.len());
    let tool_planes = tessellate_tool(&tool, TOOL_SEGMENTS);
    let tol = 1e-7 * 4.0;
    let on_p = |p: DVec3| {
        planes
            .iter()
            .filter(|&&(n, m)| (n.dot(p) - m).abs() <= tol)
            .count()
    };
    let on_tool = |p: DVec3| {
        let d: Vec<f64> = tool_planes.iter().map(|&(n, m)| n.dot(p) - m).collect();
        d.iter().all(|&x| x <= tol) && d.iter().any(|&x| x.abs() <= tol)
    };
    // [flat hidden, flat visible, tool hidden, tool visible]
    let mut seen = [0usize; 4];
    for (i, (facet, ring)) in mesh.rings.iter().enumerate() {
        assert_eq!(visible[i].len(), ring.len());
        for (j, &vis) in visible[i].iter().enumerate() {
            let mid = 0.5 * (ring[j] + ring[(j + 1) % ring.len()]);
            let is_tool = *facet >= planes.len();
            // A flat piece draws its stone edges and the groove rim; a tool
            // piece draws only where it meets the flat facets.
            let want = if is_tool {
                on_p(mid) >= 1
            } else {
                on_p(mid) >= 2 || on_tool(mid)
            };
            assert_eq!(vis, want, "ring {i} (facet {facet}) edge {j} at {mid:?}");
            seen[usize::from(is_tool) * 2 + usize::from(vis)] += 1;
        }
    }
    assert!(seen.iter().all(|&c| c > 0), "every class occurs: {seen:?}");
}

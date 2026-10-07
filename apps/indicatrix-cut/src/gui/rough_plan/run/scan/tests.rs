use super::{
    BTreeMap, CataloguePlanesSource, GpuFacetPlane, SolidExtents, SolidExtentsSource, SolidHull,
    StoredSolidExtents, ids_needing_scan, measure_planes,
};
use glam::Vec3;
use indicatrix::geometry::tool::ToolPrimitive;

/// The six planes of an axis-aligned box centred on the origin with the given half sizes.
fn box_planes(half: [f32; 3]) -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .into_iter()
    .map(|normal| {
        let along = normal.abs().dot(Vec3::from(half));
        GpuFacetPlane::new(normal, -along)
    })
    .collect()
}

/// The bounding box `(min, max)` of a hull's vertices.
fn hull_bounds(hull: &SolidHull) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &hull.vertices {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }
    (min, max)
}

#[test]
fn a_design_file_is_measured_from_its_facet_planes_without_the_preform() {
    // A preform box that is tighter than the facets along z: measuring every plane
    // would give a 1 mm long stone instead of the facets' 3 mm.
    let mut planes = box_planes([0.5, 0.5, 0.5]);
    let preform_plane_count = planes.len();
    planes.extend(box_planes([0.5, 0.25, 1.5]));

    let (extents, source, hull) = measure_planes(
        CataloguePlanesSource::DesignFile,
        preform_plane_count,
        &planes,
        &[],
    );
    assert_eq!(source, SolidExtentsSource::DesignFile);
    let extents = extents.expect("the facet planes close");
    for (got, want) in [
        (extents.width_caliper, 1.0),
        (extents.length_caliper, 3.0),
        (extents.height, 0.5),
        (extents.volume, 1.5),
    ] {
        assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    }

    let hull = hull.expect("a design file keeps its hull");
    assert_eq!(hull.vertices.len(), 8);
    let (min, max) = hull_bounds(&hull);
    for (axis, half) in [0.5_f32, 0.25, 1.5].into_iter().enumerate() {
        assert!(
            (max[axis] - half).abs() < 1e-6,
            "axis {axis} max {}",
            max[axis]
        );
        assert!(
            (min[axis] + half).abs() < 1e-6,
            "axis {axis} min {}",
            min[axis]
        );
    }
}

#[test]
fn hull_vertices_are_the_solids_vertices_narrowed_to_f32() {
    let half = 0.1_f32;
    let planes = box_planes([half; 3]);
    let (_, _, hull) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[]);
    let hull = hull.expect("a hull");
    assert_eq!(hull.vertices.len(), 8);
    for v in &hull.vertices {
        for c in v {
            assert_eq!(
                c.abs(),
                half,
                "each coordinate is the plane offset, as an f32"
            );
        }
    }
}

#[test]
fn facet_planes_that_do_not_close_leave_the_design_unbounded_even_with_a_closed_preform() {
    let mut planes = box_planes([0.5, 0.5, 0.5]);
    let preform_plane_count = planes.len();
    // Only the x and y faces: open along z.
    planes.extend(box_planes([0.5, 0.25, 1.5]).into_iter().take(4));

    let (extents, source, hull) = measure_planes(
        CataloguePlanesSource::DesignFile,
        preform_plane_count,
        &planes,
        &[],
    );
    assert_eq!(source, SolidExtentsSource::Unbounded);
    assert!(extents.is_none() && hull.is_none());
}

#[test]
fn a_preform_count_past_the_end_measures_nothing_instead_of_panicking() {
    let planes = box_planes([0.5, 0.5, 0.5]);
    let (extents, source, hull) =
        measure_planes(CataloguePlanesSource::DesignFile, 100, &planes, &[]);
    assert_eq!(source, SolidExtentsSource::Unbounded);
    assert!(extents.is_none() && hull.is_none());
}

#[test]
fn angle_table_planes_are_measured_whole_and_never_get_a_hull() {
    // The preform count means nothing for the angle table's synthetic planes.
    let planes = box_planes([0.5, 0.25, 1.5]);
    let (extents, source, hull) =
        measure_planes(CataloguePlanesSource::AngleTable, 3, &planes, &[]);
    assert_eq!(source, SolidExtentsSource::AngleTable);
    assert!(hull.is_none());
    let extents = extents.expect("the box closes");
    assert!((extents.volume - 1.5).abs() < 1e-6);
}

/// A ball tool of `radius` centred at `centre`.
fn ball(centre: [f32; 3], radius: f32) -> ToolPrimitive {
    ToolPrimitive {
        kind: 0,
        sweep_kind: 0,
        _pad: [0; 2],
        origin: [centre[0], centre[1], centre[2], radius],
        axis: [0.0, 1.0, 0.0, 0.0],
        profile: [0.0; 4],
        sweep_dir: [0.0; 4],
    }
}

#[test]
fn a_concave_tool_inside_the_outline_lowers_the_volume_and_nothing_else() {
    // A 1 x 0.5 x 3 box with a ball dimple (radius 0.2) centred on its top face: the
    // lower half of the ball is carved out of the stone.
    let planes = box_planes([0.5, 0.25, 1.5]);
    let tool = ball([0.0, 0.25, 0.0], 0.2);
    let (flat, _, flat_hull) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[]);
    let (carved, source, carved_hull) =
        measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[tool]);
    assert_eq!(source, SolidExtentsSource::DesignFile);
    let (flat, carved) = (flat.expect("closes"), carved.expect("still closes"));

    let removed = flat.volume - carved.volume;
    let hemisphere = 2.0 / 3.0 * std::f64::consts::PI * 0.2_f64.powi(3);
    assert!(
        (removed - hemisphere).abs() < 0.1 * hemisphere,
        "removed {removed} vs a hemisphere's {hemisphere}"
    );
    // The dimple sits inside the outline, so width, length and height stay the flat
    // stone's (the carved hull is measured from the carved mesh, so to rounding noise).
    for (got, want) in [
        (carved.width_caliper, flat.width_caliper),
        (carved.length_caliper, flat.length_caliper),
        (carved.width_axis, flat.width_axis),
        (carved.length_axis, flat.length_axis),
        (carved.height, flat.height),
    ] {
        // The carved mesh's corners carry the clipping's rounding (a few 1e-9 mm).
        assert!((got - want).abs() < 1e-8, "{got} vs {want}");
    }
    let (carved_hull, flat_hull) = (carved_hull.expect("hull"), flat_hull.expect("hull"));
    assert_eq!(carved_hull.vertices.len(), flat_hull.vertices.len());
    assert_eq!(
        carved_hull.vertices.len(),
        8,
        "the fit outline is still the box"
    );
    let (carved_min, carved_max) = hull_bounds(&carved_hull);
    let (flat_min, flat_max) = hull_bounds(&flat_hull);
    for axis in 0..3 {
        assert!((carved_min[axis] - flat_min[axis]).abs() < 1e-6);
        assert!((carved_max[axis] - flat_max[axis]).abs() < 1e-6);
    }
}

#[test]
fn a_concave_tool_that_removes_a_hull_vertex_gives_the_carved_hull() {
    // A ball centred on one top corner of the box removes that vertex, so the stored hull
    // is no longer the flat box's eight corners, and it stays inside the flat box.
    let planes = box_planes([0.5, 0.25, 1.5]);
    let tool = ball([0.5, 0.25, 1.5], 0.3);
    let (flat, _, flat_hull) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[]);
    let (carved, _, carved_hull) =
        measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[tool]);
    let (flat, carved) = (flat.expect("closes"), carved.expect("still closes"));
    let (carved_hull, flat_hull) = (carved_hull.expect("hull"), flat_hull.expect("hull"));
    assert_ne!(carved_hull.vertices.len(), flat_hull.vertices.len());
    let (flat_min, flat_max) = hull_bounds(&flat_hull);
    let (carved_min, carved_max) = hull_bounds(&carved_hull);
    for axis in 0..3 {
        assert!(carved_min[axis] >= flat_min[axis] - 1e-6);
        assert!(carved_max[axis] <= flat_max[axis] + 1e-6);
    }
    // Within the carved mesh's rounding (a few 1e-9 mm), as in the dimple test.
    for (got, flat) in [
        (carved.width_caliper, flat.width_caliper),
        (carved.length_caliper, flat.length_caliper),
        (carved.height, flat.height),
    ] {
        assert!(got <= flat + 1e-8, "{got} vs {flat}");
    }
    assert!(carved.volume < flat.volume);
}

#[test]
fn a_design_without_tools_keeps_its_exact_flat_vertices_and_extents() {
    use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;
    let planes = box_planes([0.3, 0.17, 1.1]);
    let halfspaces: Vec<_> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let (plain, verts) = measure_solid_with_vertices(&halfspaces).expect("closes");
    let (extents, _, hull) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[]);
    let extents = extents.expect("closes");
    assert_eq!(
        extents.width_caliper.to_bits(),
        plain.width_caliper.to_bits()
    );
    assert_eq!(
        extents.length_caliper.to_bits(),
        plain.length_caliper.to_bits()
    );
    assert_eq!(extents.height.to_bits(), plain.total_height.to_bits());
    let expected: Vec<[f32; 3]> = verts
        .iter()
        .map(|v| [v.x as f32, v.y as f32, v.z as f32])
        .collect();
    assert_eq!(hull.expect("hull").vertices, expected);
}

#[test]
fn tools_are_measured_against_the_facets_not_the_preform() {
    // The leading preform box is tighter than the facets; the carve must still
    // use the facet stone (a tool in the facet stone's top face).
    let mut planes = box_planes([0.5, 0.1, 0.5]);
    let preform_plane_count = planes.len();
    planes.extend(box_planes([0.5, 0.25, 1.5]));
    let tool = ball([0.0, 0.25, 0.0], 0.2);
    let (extents, _, _) = measure_planes(
        CataloguePlanesSource::DesignFile,
        preform_plane_count,
        &planes,
        &[tool],
    );
    let volume = extents.expect("closes").volume;
    assert!(volume < 1.5 && volume > 1.45, "{volume}");
}

#[test]
fn tools_that_remove_the_whole_stone_make_it_unmeasurable() {
    let planes = box_planes([0.5, 0.25, 1.5]);
    let everything = ball([0.0, 0.0, 0.0], 10.0);
    let (extents, source, hull) =
        measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[everything]);
    assert_eq!(source, SolidExtentsSource::Unbounded);
    assert!(extents.is_none() && hull.is_none());
}

#[test]
fn a_planar_design_keeps_the_volume_of_the_plain_measure_bit_for_bit() {
    use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;
    let planes = box_planes([0.3, 0.17, 1.1]);
    let halfspaces: Vec<_> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let (plain, _) = measure_solid_with_vertices(&halfspaces).expect("closes");
    let (extents, _, _) = measure_planes(CataloguePlanesSource::DesignFile, 0, &planes, &[]);
    assert_eq!(
        extents.expect("closes").volume.to_bits(),
        plain.volume.to_bits()
    );
}

fn extents() -> SolidExtents {
    SolidExtents {
        width_caliper: 1.0,
        length_caliper: 1.5,
        width_axis: 1.0,
        length_axis: 1.5,
        height: 0.7,
        volume: 0.9,
    }
}

fn row(source: SolidExtentsSource, usable: bool) -> StoredSolidExtents {
    StoredSolidExtents {
        extents: usable.then(extents),
        source,
    }
}

fn hull() -> SolidHull {
    SolidHull {
        vertices: vec![[0.0, 0.0, 0.0]; 4],
    }
}

#[test]
fn a_design_without_any_row_needs_a_full_scan() {
    let (missing, outlines_only) = ids_needing_scan(&[1, 2], &BTreeMap::new(), &BTreeMap::new());
    assert_eq!(missing, vec![1, 2]);
    assert!(!outlines_only);
}

#[test]
fn a_design_file_row_without_a_hull_needs_an_outline_scan() {
    let extents_map = BTreeMap::from([(7, row(SolidExtentsSource::DesignFile, true))]);
    let (missing, outlines_only) = ids_needing_scan(&[7], &extents_map, &BTreeMap::new());
    assert_eq!(missing, vec![7]);
    assert!(outlines_only);

    let hulls = BTreeMap::from([(7, hull())]);
    let (missing, outlines_only) = ids_needing_scan(&[7], &extents_map, &hulls);
    assert_eq!(missing, Vec::<i64>::new());
    assert!(!outlines_only, "nothing to scan is not an outline scan");
}

#[test]
fn unbounded_and_angle_table_rows_never_get_a_hull_so_are_not_rescanned() {
    let extents_map = BTreeMap::from([
        (1, row(SolidExtentsSource::Unbounded, false)),
        (2, row(SolidExtentsSource::AngleTable, true)),
        (3, row(SolidExtentsSource::DesignFile, false)),
    ]);
    let (missing, outlines_only) = ids_needing_scan(&[1, 2, 3], &extents_map, &BTreeMap::new());
    assert_eq!(missing, Vec::<i64>::new());
    assert!(!outlines_only);
}

#[test]
fn a_mixed_set_is_a_full_scan_of_exactly_the_missing_ids() {
    let extents_map = BTreeMap::from([
        (1, row(SolidExtentsSource::DesignFile, true)),
        (2, row(SolidExtentsSource::DesignFile, true)),
        (3, row(SolidExtentsSource::Unbounded, false)),
    ]);
    let hulls = BTreeMap::from([(1, hull())]);
    let (missing, outlines_only) = ids_needing_scan(&[1, 2, 3, 4], &extents_map, &hulls);
    assert_eq!(missing, vec![2, 4]);
    assert!(!outlines_only, "id 4 has no extents at all");
}

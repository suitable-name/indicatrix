//! [`build_solid_mesh`] coverage: agreement with `measure_solid`'s volume,
//! determinism, and the `Unbounded`/`Degenerate` status variants.

use glam::DVec3;

use super::{
    assert_mesh_matches_measure_solid, assert_watertight, mesh_divergence_volume,
    planes_from_asc_schedule,
};
use crate::geometry::stone_metrics::{SolidStatus, build_solid_mesh, measure_solid};

#[test]
fn build_solid_mesh_matches_measure_solid_on_a_plain_box() {
    assert_mesh_matches_measure_solid(
        &[
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ],
        "plain box",
    );
}

#[test]
fn build_solid_mesh_matches_measure_solid_on_a_hip_roofed_block() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    assert_mesh_matches_measure_solid(
        &[
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
        "hip-roofed block",
    );
}

#[test]
fn build_solid_mesh_matches_measure_solid_on_a_real_schedule() {
    assert_mesh_matches_measure_solid(
        &planes_from_asc_schedule(
            "GemCad 5.0\n\
             g 96 0.0\n\
             y 6 y\n\
             I 1.72\n\
             H Bench design\n\
             a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
             a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
             a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
             a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
             a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
             a 0.000000 0.44000000 n T\n",
        ),
        "real schedule: Bench design",
    );
}

/// The vertex-id-only fields (`facet_id`, `indices`) and geometry
/// (`positions`, `normals`) must be byte-identical (not just
/// numerically close) across two calls with the same input -- the same
/// determinism contract `measurement_is_deterministic` checks for
/// `measure_solid`.
#[test]
fn build_solid_mesh_is_deterministic() {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
        (DVec3::NEG_Y, 0.5),
        (DVec3::new(s, s, 0.0), s),
        (DVec3::new(-s, s, 0.0), s),
        (DVec3::new(0.0, s, s), s),
        (DVec3::new(0.0, s, -s), s),
    ];
    let (SolidStatus::Closed(a), SolidStatus::Closed(b)) =
        (build_solid_mesh(&planes), build_solid_mesh(&planes))
    else {
        panic!("fixture must close");
    };
    assert_eq!(a.facet_id, b.facet_id);
    assert_eq!(a.indices, b.indices);
    assert_eq!(a.positions.len(), b.positions.len());
    for (pa, pb) in a.positions.iter().zip(&b.positions) {
        assert_eq!(
            (pa.x.to_bits(), pa.y.to_bits(), pa.z.to_bits()),
            (pb.x.to_bits(), pb.y.to_bits(), pb.z.to_bits())
        );
    }
    for (na, nb) in a.normals.iter().zip(&b.normals) {
        assert_eq!(
            (na.x.to_bits(), na.y.to_bits(), na.z.to_bits()),
            (nb.x.to_bits(), nb.y.to_bits(), nb.z.to_bits())
        );
    }
}

/// A schedule missing its closing planes (no floor) must report
/// `Unbounded` naming at least one real (non-blank) plane index, not
/// silently blank or panic -- the editor-facing case this status exists
/// for.
#[test]
fn build_solid_mesh_reports_unbounded_with_escaping_planes() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    match build_solid_mesh(&planes) {
        SolidStatus::Unbounded { escaping } => {
            assert!(!escaping.is_empty(), "expected at least one escaping plane");
            assert!(escaping.iter().all(|&i| i < planes.len()));
            let mut sorted = escaping.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                escaping, sorted,
                "escaping indices must be sorted and deduped"
            );
        }
        other => panic!("expected Unbounded, got {other:?}"),
    }
}

/// Six planes all through the origin collapse the "solid" to a single
/// point: bounded (nothing escapes the blank), but with 1 distinct
/// vertex, far short of the 4 a real polytope needs.
#[test]
fn build_solid_mesh_reports_degenerate_for_too_few_vertices() {
    let planes = vec![
        (DVec3::X, 0.0),
        (DVec3::NEG_X, 0.0),
        (DVec3::Y, 0.0),
        (DVec3::NEG_Y, 0.0),
        (DVec3::Z, 0.0),
        (DVec3::NEG_Z, 0.0),
    ];
    match build_solid_mesh(&planes) {
        SolidStatus::Degenerate {
            vertex_count,
            volume,
        } => {
            assert!(vertex_count < 4, "vertex_count {vertex_count}");
            assert!(volume.is_none() || volume == Some(0.0));
        }
        other => panic!("expected Degenerate, got {other:?}"),
    }
}

/// A box flattened to zero height (`y` pinned to exactly 0 by both the
/// `+Y` and `-Y` planes) has 4 distinct vertices -- enough to pass the
/// vertex-count gate -- but zero volume: bounded, not too few vertices,
/// yet still not a real solid.
#[test]
fn build_solid_mesh_reports_degenerate_for_zero_volume() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.0),
        (DVec3::NEG_Y, 0.0),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    match build_solid_mesh(&planes) {
        SolidStatus::Degenerate {
            vertex_count,
            volume,
        } => {
            assert_eq!(vertex_count, 4);
            assert_eq!(volume, Some(0.0));
        }
        other => panic!("expected Degenerate, got {other:?}"),
    }
}

/// A duplicated plane (the same normal and offset listed twice, e.g. a
/// tier appearing in two rows) must not produce duplicate overlapping
/// geometry -- `build_solid_mesh` must dedup exactly like `measure_solid`
/// does, and the escaping/facet indices it reports must still refer to
/// the ORIGINAL (pre-dedup) plane list position, not the deduped one.
#[test]
fn build_solid_mesh_dedups_planes_and_maps_indices_to_the_original_list() {
    let planes = vec![
        (DVec3::X, 1.0),
        (DVec3::X, 1.0), // duplicate of index 0
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ];
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        panic!("fixture must close");
    };
    assert_watertight(&mesh, "duplicate-plane box");
    // Exactly one of the two bit-identical `+X` planes (index 0 or 1)
    // should own a face -- never both, and never any index >= len.
    let x_owners: Vec<usize> = mesh
        .rings
        .iter()
        .map(|&(idx, _)| idx)
        .filter(|&idx| idx == 0 || idx == 1)
        .collect();
    assert_eq!(
        x_owners.len(),
        1,
        "expected exactly one +X facet owner among indices [0, 1], got {x_owners:?}"
    );
    assert!(mesh.facet_id.iter().all(|&f| f < planes.len()));
    let want_volume = measure_solid(&planes).expect("must measure").volume;
    let got_volume = mesh_divergence_volume(&mesh);
    assert!((got_volume - want_volume).abs() < 1e-9);
}

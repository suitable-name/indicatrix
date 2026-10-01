use std::f64::consts::PI;

use glam::DVec3;

use super::{
    BoxFace, RoughBase, RoughCut, RoughModel, ShapeError,
    base::cylinder_halfspaces,
    sampling::{half_step_cos, sphere_directions, unit_circle},
};
use crate::rough_plan::Axis;

fn block_model(cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 9.0,
            z_mm: 8.0,
        },
        cuts,
    )
}

fn cube_model(size: f64, cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: size,
            y_mm: size,
            z_mm: size,
        },
        cuts,
    )
}

#[test]
fn test_sampling_unit_circle_properties() {
    for &n in &[4, 8, 16, 32, 64, 128, 256, 512, 1024] {
        let pts = unit_circle(n);
        assert_eq!(pts.len(), n);
        for pt in &pts {
            let len = pt[0].hypot(pt[1]);
            assert!(
                (len - 1.0).abs() < 2e-15,
                "circle point out of tolerance for n={n}: {pt:?}, len={len}"
            );
        }
    }

    // n = 64 point 8 equals (sqrt(2)/2, sqrt(2)/2) to 1e-15
    let pts64 = unit_circle(64);
    let expected = (0.5_f64).sqrt();
    assert!((pts64[8][0] - expected).abs() < 1e-15);
    assert!((pts64[8][1] - expected).abs() < 1e-15);

    // Bitwise determinism across two calls
    let pts64_again = unit_circle(64);
    for (a, b) in pts64.iter().zip(&pts64_again) {
        assert_eq!(a[0].to_bits(), b[0].to_bits());
        assert_eq!(a[1].to_bits(), b[1].to_bits());
    }
}

#[test]
fn test_sampling_unit_circle_symmetry() {
    // Point j + n/2 is the exact negation of point j, and point j + n/4 is point j turned
    // by a quarter turn: (x, y) -> (-y, x). This holds for every j, including the points
    // that are re-derived from the half-angle table.
    for &n in &[16_usize, 64, 256, 1024] {
        let pts = unit_circle(n);
        for (j, &p) in pts.iter().enumerate() {
            let half = pts[(j + n / 2) % n];
            let quarter = pts[(j + n / 4) % n];
            assert!(
                (half[0] + p[0]).abs() < 2e-15 && (half[1] + p[1]).abs() < 2e-15,
                "n={n} j={j}: point at +n/2 is {half:?}, expected {:?}",
                [-p[0], -p[1]]
            );
            assert!(
                (quarter[0] + p[1]).abs() < 2e-15 && (quarter[1] - p[0]).abs() < 2e-15,
                "n={n} j={j}: point at +n/4 is {quarter:?}, expected {:?}",
                [-p[1], p[0]]
            );
        }
    }
}

#[test]
fn test_sampling_half_step_cos() {
    let c4 = half_step_cos(4);
    let expected4 = (0.5_f64).sqrt();
    assert!((c4 - expected4).abs() < 1e-15);
    let c64 = half_step_cos(64);
    assert!((c64 - (PI / 64.0).cos()).abs() < 1e-15);
}

/// Unit directions `(i, j, k) / |(i, j, k)|` with `|i| + |j| + |k| = level`, enumerated by
/// brute force in ascending lexicographic order of the integer triple.
fn expected_sphere_directions(level: i32) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for i in -level..=level {
        for j in -level..=level {
            for k in -level..=level {
                if i.abs() + j.abs() + k.abs() == level {
                    let len = f64::from(i * i + j * j + k * k).sqrt();
                    out.push([f64::from(i) / len, f64::from(j) / len, f64::from(k) / len]);
                }
            }
        }
    }
    out
}

#[test]
fn test_sampling_sphere_directions() {
    let d3 = sphere_directions(3);
    assert_eq!(d3.len(), 38);

    let d4 = sphere_directions(4);
    assert_eq!(d4.len(), 66);

    let d6 = sphere_directions(6);
    assert_eq!(d6.len(), 146);

    let d8 = sphere_directions(8);
    assert_eq!(d8.len(), 258);

    // Order and content: the integer triples in ascending lexicographic order, normalised.
    for (level, dirs) in [(3, &d3), (4, &d4), (6, &d6), (8, &d8)] {
        let expected = expected_sphere_directions(level);
        assert_eq!(dirs.len(), expected.len());
        for (idx, (got, want)) in dirs.iter().zip(&expected).enumerate() {
            for (g, w) in got.iter().zip(want) {
                assert!(
                    (g - w).abs() < 1e-15,
                    "level {level} direction {idx}: got {got:?}, expected {want:?}"
                );
            }
        }
    }

    // Check presence of all six unit axis directions
    let axes = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    for axis in axes {
        assert!(
            d6.iter().any(|&d| {
                (d[0] - axis[0]).abs() < 1e-15
                    && (d[1] - axis[1]).abs() < 1e-15
                    && (d[2] - axis[2]).abs() < 1e-15
            }),
            "missing axis direction {axis:?}"
        );
    }
}

#[test]
fn test_block_extents_and_volume() {
    let model = block_model(Vec::new());
    let measure = model.measure().expect("measure block");
    assert!((measure.volume_mm3 - 864.0).abs() < 1e-9);
    assert!((measure.extents_mm[0] - 12.0).abs() < 1e-9);
    assert!((measure.extents_mm[1] - 9.0).abs() < 1e-9);
    assert!((measure.extents_mm[2] - 8.0).abs() < 1e-9);
    assert_eq!(measure.plane_count, 6);
}

#[test]
fn test_edge_chamfer_removes_wedge() {
    let model = block_model(vec![RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [2.0, 3.0],
    }]);
    let measure = model.measure().expect("measure chamfered block");
    // Top-Front edge runs along X (length 12).
    // Setback along Top is 2.0 (Z direction), setback along Front is 3.0 (Y direction).
    // Wedge removed = 0.5 * 2 * 3 * 12 = 36 mm^3. Remaining = 864 - 36 = 828 mm^3.
    assert!((measure.volume_mm3 - 828.0).abs() < 1e-7);
}

#[test]
fn test_edge_chamfer_on_high_faces() {
    // Top (+Y) and Right (+X) meet in an edge along Z (length 8). The setback along Top is
    // measured in x (2.0), the setback along Right in y (3.0).
    let model = block_model(vec![RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Right],
        setbacks_mm: [2.0, 3.0],
    }]);
    let measure = model.measure().expect("measure chamfered block");
    let removed = 0.5 * 2.0 * 3.0 * 8.0;
    assert!((measure.volume_mm3 - (864.0 - removed)).abs() < 1e-7);
    assert!((measure.extents_mm[0] - 12.0).abs() < 1e-9);
    assert!((measure.extents_mm[1] - 9.0).abs() < 1e-9);

    let planes = model.halfspaces().expect("halfspaces");
    let (n, m) = planes[6];
    assert!((n.length() - 1.0).abs() < 1e-12);
    assert!(n.x > 0.0 && n.y > 0.0 && n.z.abs() < 1e-12, "normal {n:?}");
    for on_plane in [
        DVec3::new(10.0, 9.0, 4.0),
        DVec3::new(12.0, 6.0, 0.0),
        DVec3::new(11.0, 7.5, 8.0),
    ] {
        assert!(
            (n.dot(on_plane) - m).abs() < 1e-12,
            "{on_plane:?} off plane"
        );
    }
    assert!(n.dot(DVec3::new(12.0, 9.0, 4.0)) > m, "edge point kept");
    assert!(n.dot(DVec3::new(6.0, 4.5, 4.0)) < m, "centre point removed");
}

#[test]
fn test_corner_cut_removes_pyramid() {
    let model = cube_model(
        1.0,
        vec![RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
            setbacks_mm: [1.0, 1.0, 1.0],
        }],
    );
    let measure = model.measure().expect("measure corner-cut block");
    // Unit cube minus 1/6 pyramid = 5/6
    assert!((measure.volume_mm3 - (5.0 / 6.0)).abs() < 1e-9);
}

#[test]
fn test_corner_cut_on_high_faces() {
    // Top (+Y), Right (+X), Front (+Z): setbacks 1.5 along y, 2.0 along x, 2.5 along z.
    let model = block_model(vec![RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Right, BoxFace::Front],
        setbacks_mm: [1.5, 2.0, 2.5],
    }]);
    let measure = model.measure().expect("measure corner-cut block");
    let removed = 1.5 * 2.0 * 2.5 / 6.0;
    assert!((measure.volume_mm3 - (864.0 - removed)).abs() < 1e-9);

    let planes = model.halfspaces().expect("halfspaces");
    let (n, m) = planes[6];
    assert!(n.x > 0.0 && n.y > 0.0 && n.z > 0.0, "normal {n:?}");
    for on_plane in [
        DVec3::new(10.0, 9.0, 8.0),
        DVec3::new(12.0, 7.5, 8.0),
        DVec3::new(12.0, 9.0, 5.5),
    ] {
        assert!(
            (n.dot(on_plane) - m).abs() < 1e-12,
            "{on_plane:?} off plane"
        );
    }
    assert!(n.dot(DVec3::new(12.0, 9.0, 8.0)) > m, "corner point kept");

    // A mixed corner (Bottom is low, Right and Front are high) removes the same volume.
    let mixed = block_model(vec![RoughCut::Corner {
        faces: [BoxFace::Bottom, BoxFace::Right, BoxFace::Front],
        setbacks_mm: [1.5, 2.0, 2.5],
    }]);
    let mixed_measure = mixed.measure().expect("measure mixed corner");
    assert!((mixed_measure.volume_mm3 - (864.0 - removed)).abs() < 1e-9);
    let (mn, mm) = mixed.halfspaces().expect("halfspaces")[6];
    assert!(mn.dot(DVec3::new(12.0, 0.0, 8.0)) > mm, "corner point kept");
    assert!((mn.dot(DVec3::new(10.0, 0.0, 8.0)) - mm).abs() < 1e-12);
    assert!((mn.dot(DVec3::new(12.0, 1.5, 8.0)) - mm).abs() < 1e-12);
    assert!((mn.dot(DVec3::new(12.0, 0.0, 5.5)) - mm).abs() < 1e-12);
}

#[test]
fn test_cylinder_volume_ratio() {
    let diam = 10.0;
    let length = 20.0;
    let model = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: diam,
            length_mm: length,
            axis: Axis::Y,
        },
        Vec::new(),
    );
    let measure = model.measure().expect("measure cylinder");
    let true_vol = PI * (diam * 0.5) * (diam * 0.5) * length;
    let ratio = measure.volume_mm3 / true_vol;
    assert!(
        (ratio - 0.99839).abs() < 1e-4,
        "cylinder volume ratio {ratio} not within 0.99839 +/- 1e-4"
    );
    assert_eq!(measure.plane_count, 66);
}

#[test]
fn test_cylinder_has_64_distinct_side_normals() {
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        let planes = cylinder_halfspaces(10.0, 20.0, axis, 64);
        assert_eq!(planes.len(), 66);
        let sides: Vec<DVec3> = planes[2..].iter().map(|&(n, _)| n).collect();

        let axis_vec = planes[0].0;
        let mut sum = DVec3::ZERO;
        for n in &sides {
            assert!((n.length() - 1.0).abs() < 1e-14);
            assert!(
                n.dot(axis_vec).abs() < 1e-14,
                "side normal not perpendicular"
            );
            sum += *n;
        }
        // Evenly spread normals cancel; a missing or duplicated arc would not.
        assert!(sum.length() < 1e-12, "side normals sum to {sum:?}");

        let mut min_dist = f64::INFINITY;
        for (a, na) in sides.iter().enumerate() {
            for nb in &sides[a + 1..] {
                min_dist = min_dist.min((*na - *nb).length());
            }
        }
        // Neighbouring normals are 2 sin(pi/64) = 0.0981 apart.
        assert!(
            min_dist > 0.09,
            "two side normals nearly coincide: {min_dist}"
        );
        assert!(
            min_dist < 0.1,
            "side normals are not evenly spaced: {min_dist}"
        );
    }
}

#[test]
fn test_pebble_volume_is_within_three_percent_of_the_ellipsoid() {
    let (dim_x, dim_y, dim_z) = (20.0, 14.0, 10.0);
    let model = RoughModel::new(
        RoughBase::Pebble {
            x_mm: dim_x,
            y_mm: dim_y,
            z_mm: dim_z,
        },
        Vec::new(),
    );
    let measure = model.measure().expect("measure pebble");
    let true_vol = (4.0 / 3.0) * PI * (dim_x * 0.5) * (dim_y * 0.5) * (dim_z * 0.5);
    // The pebble is the unit polytope (162 planes, every vertex inside the unit ball, each
    // face pushed out until its own vertices touch the sphere) mapped by the linear map
    // `semi * u` onto the ellipsoid. A linear map scales every volume by its determinant, so
    // the pebble holds the unit polytope's fraction of the unit ball, about 0.973, of any
    // ellipsoid's volume, and being inscribed it stays below 1.
    let ratio = measure.volume_mm3 / true_vol;
    assert!(
        (0.97..1.0).contains(&ratio),
        "pebble volume ratio {ratio} not in [0.97, 1.0)"
    );
}

#[test]
fn test_face_cut_on_cylinder() {
    let base = RoughBase::Cylinder {
        diameter_mm: 10.0,
        length_mm: 20.0,
        axis: Axis::Y,
    };
    let whole = RoughModel::new(base, Vec::new())
        .measure()
        .expect("measure cylinder");
    let cut = RoughModel::new(
        base,
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 4.0,
        }],
    )
    .measure()
    .expect("measure cut cylinder");

    // The prism has a constant cross-section, so a 4 mm slab off a 20 mm length leaves 16/20.
    let expected = whole.volume_mm3 * 0.8;
    assert!(
        (cut.volume_mm3 - expected).abs() < 1e-9 * whole.volume_mm3,
        "volume {} vs expected {expected}",
        cut.volume_mm3
    );
    assert!((cut.extents_mm[1] - 16.0).abs() < 1e-9);
    assert!((cut.extents_mm[0] - whole.extents_mm[0]).abs() < 1e-9);
    assert!((cut.extents_mm[2] - whole.extents_mm[2]).abs() < 1e-9);
    assert_eq!(cut.plane_count, 67);
}

#[test]
fn test_face_cut_on_pebble() {
    let base = RoughBase::Pebble {
        x_mm: 20.0,
        y_mm: 14.0,
        z_mm: 10.0,
    };
    let whole = RoughModel::new(base, Vec::new())
        .measure()
        .expect("measure pebble");
    let cut = RoughModel::new(
        base,
        vec![RoughCut::Face {
            normal: [1.0, 0.0, 0.0],
            depth_mm: 5.0,
        }],
    )
    .measure()
    .expect("measure cut pebble");

    // The plane sits 5 mm inside the pebble's outermost point along +x.
    assert!(
        (cut.extents_mm[0] - (whole.extents_mm[0] - 5.0)).abs() < 1e-9,
        "x extent {} vs {}",
        cut.extents_mm[0],
        whole.extents_mm[0] - 5.0
    );
    assert!((cut.extents_mm[1] - whole.extents_mm[1]).abs() < 1e-9);
    assert!((cut.extents_mm[2] - whole.extents_mm[2]).abs() < 1e-9);
    assert!(cut.volume_mm3 < whole.volume_mm3);
    assert!(cut.volume_mm3 > 0.5 * whole.volume_mm3);
    // The 162 geodesic pebble planes plus the cut plane.
    assert_eq!(cut.plane_count, 163);
}

#[test]
fn test_usable_halfspaces_inset() {
    let inset = 0.75;
    let models = [
        block_model(vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Right],
            setbacks_mm: [2.0, 3.0],
        }]),
        RoughModel::new(
            RoughBase::Cylinder {
                diameter_mm: 10.0,
                length_mm: 20.0,
                axis: Axis::Z,
            },
            Vec::new(),
        ),
    ];
    for model in &models {
        let full = model.halfspaces().expect("halfspaces");
        let usable = model.usable_halfspaces(inset).expect("usable halfspaces");
        assert_eq!(full.len(), usable.len());
        for (&(n_full, m_full), &(n_use, m_use)) in full.iter().zip(&usable) {
            assert_eq!(n_full, n_use);
            assert!((m_full - inset - m_use).abs() < 1e-15);
        }

        let coarse = model.coarse_halfspaces().expect("coarse halfspaces");
        let coarse_usable = model
            .coarse_usable_halfspaces(inset)
            .expect("coarse usable halfspaces");
        assert_eq!(coarse.len(), coarse_usable.len());
        for (&(n_c, m_c), &(n_u, m_u)) in coarse.iter().zip(&coarse_usable) {
            assert_eq!(n_c, n_u);
            assert!((m_c - inset - m_u).abs() < 1e-15);
        }
    }

    // On a block the six planes' offsets show which way the inset points: the high planes
    // move down, the low planes (offset 0) become negative.
    let usable = cube_model(1.0, Vec::new())
        .usable_halfspaces(0.2)
        .expect("usable cube");
    assert_eq!(usable.len(), 6);
    assert!((usable[0].1 - 0.8).abs() < 1e-15);
    assert!((usable[1].1 + 0.2).abs() < 1e-15);
}

#[test]
fn test_error_face_depth_zero() {
    let model = cube_model(
        10.0,
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 0.0,
        }],
    );
    assert_eq!(
        model.measure(),
        Err(ShapeError::BadDepth {
            index: 0,
            depth_mm: 0.0,
            thickness_mm: None,
        })
    );
}

#[test]
fn test_error_face_depth_too_large() {
    let model = cube_model(
        10.0,
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 12.0,
        }],
    );
    let err = model.measure().expect_err("depth exceeds the rough");
    match &err {
        ShapeError::BadDepth {
            index: 0,
            depth_mm,
            thickness_mm: Some(thickness),
        } => {
            assert_eq!(*depth_mm, 12.0);
            assert!((thickness - 10.0).abs() < 1e-9, "thickness {thickness}");
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(
        err.to_string(),
        "Cut 1: the depth 12.0 mm reaches through the rough (10.0 mm thick)."
    );
}

#[test]
fn test_error_setback_longer_than_face() {
    // The Front face is 8 mm across, so a 9 mm setback along it is too long.
    let model = block_model(vec![RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [9.0, 2.0],
    }]);
    let err = model.measure().expect_err("setback too long");
    assert_eq!(
        err,
        ShapeError::BadSetback {
            index: 0,
            setback_mm: 9.0,
            face_mm: 8.0,
        }
    );
    assert_eq!(
        err.to_string(),
        "Cut 1: the setback 9.0 mm is longer than the face (8.0 mm)."
    );

    let negative = block_model(vec![RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Front, BoxFace::Left],
        setbacks_mm: [1.0, -2.0, 1.0],
    }]);
    let err = negative.measure().expect_err("negative setback");
    assert_eq!(
        err.to_string(),
        "Cut 1: the setback (-2) must be a positive, finite length."
    );
}

#[test]
fn test_error_edge_on_pebble() {
    let model = RoughModel::new(
        RoughBase::Pebble {
            x_mm: 10.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [2.0, 2.0],
        }],
    );
    assert_eq!(model.measure(), Err(ShapeError::CutNotForBase { index: 0 }));
}

#[test]
fn test_error_nothing_left() {
    // Two opposite face cuts removing the entire block
    let model = cube_model(
        10.0,
        vec![
            RoughCut::Face {
                normal: [0.0, 1.0, 0.0],
                depth_mm: 6.0,
            },
            RoughCut::Face {
                normal: [0.0, -1.0, 0.0],
                depth_mm: 6.0,
            },
        ],
    );
    assert_eq!(model.measure(), Err(ShapeError::NothingLeft));
}

fn assert_same_measure(reference: &super::RoughMeasure, other: &super::RoughMeasure, label: &str) {
    assert_eq!(
        reference.volume_mm3.to_bits(),
        other.volume_mm3.to_bits(),
        "{label}: volumes must match bit for bit"
    );
    for (axis, (a, b)) in reference
        .extents_mm
        .iter()
        .zip(&other.extents_mm)
        .enumerate()
    {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "{label}: extent {axis} must match bit for bit"
        );
    }
    assert_eq!(reference.plane_count, other.plane_count);
    assert_eq!(reference.vertex_count, other.vertex_count);
}

#[test]
fn test_cuts_order_independence() {
    let cut_a = RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [2.0, 3.0],
    };
    let cut_b = RoughCut::Corner {
        faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
        setbacks_mm: [1.5, 2.0, 2.5],
    };
    let cut_c = RoughCut::Face {
        normal: [1.0, 0.7, -0.3],
        depth_mm: 1.3,
    };
    let cut_d = RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Right, BoxFace::Front],
        setbacks_mm: [1.1, 2.3, 1.7],
    };

    let cuts = [cut_a, cut_b, cut_c, cut_d];
    let orders: [[usize; 4]; 6] = [
        [0, 1, 2, 3],
        [3, 2, 1, 0],
        [1, 0, 3, 2],
        [2, 3, 0, 1],
        [1, 2, 3, 0],
        [3, 0, 2, 1],
    ];
    let reference = block_model(orders[0].iter().map(|&i| cuts[i].clone()).collect())
        .measure()
        .expect("measure reference order");
    for order in &orders[1..] {
        let model = block_model(order.iter().map(|&i| cuts[i].clone()).collect());
        let measure = model.measure().expect("measure permuted order");
        assert_same_measure(&reference, &measure, &format!("order {order:?}"));
    }

    // A Face cut on a cylinder is measured against the base only, so it commutes too.
    let cylinder = RoughBase::Cylinder {
        diameter_mm: 10.0,
        length_mm: 20.0,
        axis: Axis::X,
    };
    let face_1 = RoughCut::Face {
        normal: [0.3, 1.0, 0.2],
        depth_mm: 2.0,
    };
    let face_2 = RoughCut::Face {
        normal: [-0.4, -0.2, 1.0],
        depth_mm: 1.5,
    };
    let forward = RoughModel::new(cylinder, vec![face_1.clone(), face_2.clone()])
        .measure()
        .expect("measure forward");
    let backward = RoughModel::new(cylinder, vec![face_2, face_1])
        .measure()
        .expect("measure backward");
    assert_same_measure(&forward, &backward, "cylinder face cuts");
}

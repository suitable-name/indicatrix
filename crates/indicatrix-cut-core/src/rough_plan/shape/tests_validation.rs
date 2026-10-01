//! Tests of the rough model's input validation: sampling counts, cut faces on a repeated
//! axis, base sizes and face-cut normals.
//!
//! Every expectation is derived by hand in the test that uses it.

use super::{
    BoxFace, RoughBase, RoughCut, RoughModel, ShapeError,
    sampling::{half_step_cos, unit_circle},
    tests_geometry::{block, face},
};
use crate::rough_plan::Axis;

#[test]
fn unit_circle_and_half_step_cos_reject_counts_that_are_not_powers_of_two() {
    for n in [0_usize, 1, 3, 5, 6, 12, 48, 100, 1000] {
        assert!(
            std::panic::catch_unwind(|| unit_circle(n)).is_err(),
            "unit_circle({n}) must panic"
        );
    }
    // Powers of two outside 4..=1024 are rejected as well.
    for n in [2_usize, 2048] {
        assert!(
            std::panic::catch_unwind(|| unit_circle(n)).is_err(),
            "unit_circle({n}) must panic"
        );
    }
    for n in [0_usize, 1, 3, 6, 100] {
        assert!(
            std::panic::catch_unwind(|| half_step_cos(n)).is_err(),
            "half_step_cos({n}) must panic"
        );
    }
}

#[test]
fn edge_and_corner_cuts_on_a_repeated_axis_are_bad_faces() {
    let edges = [
        [BoxFace::Top, BoxFace::Bottom],
        [BoxFace::Left, BoxFace::Right],
        [BoxFace::Front, BoxFace::Back],
        [BoxFace::Top, BoxFace::Top],
    ];
    for faces in edges {
        // The bad cut is second, so the error names cut 2.
        let good = RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [1.0, 1.0],
        };
        let bad = RoughCut::Edge {
            faces,
            setbacks_mm: [1.0, 1.0],
        };
        let err = block(12.0, 9.0, 8.0, vec![good, bad])
            .measure()
            .expect_err("repeated axis");
        assert_eq!(err, ShapeError::BadFaces { index: 1 }, "{faces:?}");
        assert_eq!(err.to_string(), "Cut 2: faces must be on different axes.");
    }

    let corners = [
        [BoxFace::Top, BoxFace::Bottom, BoxFace::Front],
        [BoxFace::Top, BoxFace::Front, BoxFace::Bottom],
        [BoxFace::Front, BoxFace::Top, BoxFace::Back],
        [BoxFace::Left, BoxFace::Right, BoxFace::Top],
        [BoxFace::Right, BoxFace::Right, BoxFace::Right],
    ];
    for faces in corners {
        let bad = RoughCut::Corner {
            faces,
            setbacks_mm: [1.0, 1.0, 1.0],
        };
        let err = block(12.0, 9.0, 8.0, vec![bad])
            .measure()
            .expect_err("repeated axis");
        assert_eq!(err, ShapeError::BadFaces { index: 0 }, "{faces:?}");
        assert_eq!(err.to_string(), "Cut 1: faces must be on different axes.");
    }
}

#[test]
fn base_sizes_that_are_not_positive_and_finite_are_rejected() {
    let bad = [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
    for len in bad {
        let bases = [
            RoughBase::Block {
                x_mm: len,
                y_mm: 1.0,
                z_mm: 1.0,
            },
            RoughBase::Block {
                x_mm: 1.0,
                y_mm: 1.0,
                z_mm: len,
            },
            RoughBase::Pebble {
                x_mm: 1.0,
                y_mm: len,
                z_mm: 1.0,
            },
            RoughBase::Cylinder {
                diameter_mm: len,
                length_mm: 5.0,
                axis: Axis::Y,
            },
            RoughBase::Cylinder {
                diameter_mm: 5.0,
                length_mm: len,
                axis: Axis::X,
            },
        ];
        for base in bases {
            assert_eq!(
                base.validate(),
                Err(ShapeError::NonPositiveSize),
                "{base:?}"
            );
            let model = RoughModel::new(base, Vec::new());
            assert_eq!(model.halfspaces(), Err(ShapeError::NonPositiveSize));
            assert_eq!(model.measure(), Err(ShapeError::NonPositiveSize));
        }
    }
    assert_eq!(
        ShapeError::NonPositiveSize.to_string(),
        "All dimensions must be positive and finite."
    );
}

#[test]
fn base_sizes_over_two_thousand_millimetres_are_too_large() {
    let at_limit = block(2000.0, 2000.0, 2000.0, Vec::new());
    assert_eq!(at_limit.base.validate(), Ok(()));
    assert!(at_limit.halfspaces().is_ok());

    for base in [
        RoughBase::Block {
            x_mm: 2_000.000_1,
            y_mm: 1.0,
            z_mm: 1.0,
        },
        RoughBase::Pebble {
            x_mm: 1.0,
            y_mm: 1.0,
            z_mm: 2_000.000_1,
        },
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 2_000.000_1,
            axis: Axis::Z,
        },
        RoughBase::Cylinder {
            diameter_mm: 2_000.000_1,
            length_mm: 10.0,
            axis: Axis::Z,
        },
    ] {
        assert_eq!(base.validate(), Err(ShapeError::TooLarge), "{base:?}");
        assert_eq!(base.to_halfspaces(false), Err(ShapeError::TooLarge));
    }
    let edge = RoughBase::Cylinder {
        diameter_mm: 2000.0,
        length_mm: 2000.0,
        axis: Axis::X,
    };
    assert_eq!(edge.validate(), Ok(()));
    assert_eq!(
        ShapeError::TooLarge.to_string(),
        "Dimensions must not exceed 2000 mm."
    );
}

#[test]
fn face_cut_normals_that_are_zero_or_not_finite_are_bad_normals() {
    for normal in [
        [0.0, 0.0, 0.0],
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::NAN, 1.0],
        [f64::INFINITY, 0.0, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        let err = block(10.0, 10.0, 10.0, vec![face(normal, 1.0)])
            .measure()
            .expect_err("bad normal");
        assert_eq!(err, ShapeError::BadNormal { index: 0 }, "{normal:?}");
        assert_eq!(
            err.to_string(),
            "Cut 1: normal vector must be non-zero and finite."
        );
    }
}

#[test]
fn a_scaled_model_scales_every_length_and_its_volume_by_the_cube() {
    let base = |x_mm: f64, y_mm: f64, z_mm: f64| RoughBase::Block { x_mm, y_mm, z_mm };
    let model = RoughModel::new(
        base(10.0, 8.0, 6.0),
        vec![
            RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [1.0, 1.5],
            },
            RoughCut::Corner {
                faces: [BoxFace::Top, BoxFace::Front, BoxFace::Left],
                setbacks_mm: [1.0, 1.0, 2.0],
            },
            RoughCut::Face {
                normal: [0.0, -1.0, 0.0],
                depth_mm: 0.5,
            },
        ],
    );
    let scaled = model.scaled(1.5);
    assert_eq!(scaled.base, base(15.0, 12.0, 9.0));
    assert_eq!(
        scaled.cuts,
        vec![
            RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [1.5, 2.25],
            },
            RoughCut::Corner {
                faces: [BoxFace::Top, BoxFace::Front, BoxFace::Left],
                setbacks_mm: [1.5, 1.5, 3.0],
            },
            RoughCut::Face {
                normal: [0.0, -1.0, 0.0],
                depth_mm: 0.75,
            },
        ]
    );
    // Every length scales by 1.5, so the volume scales by 1.5^3 = 3.375.
    let before = model.measure().expect("measure the model").volume_mm3;
    let after = scaled
        .measure()
        .expect("measure the scaled model")
        .volume_mm3;
    assert!(
        (after / before - 3.375).abs() < 1e-9,
        "volume {before} -> {after}"
    );
}

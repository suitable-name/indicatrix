//! Tests of the rough model's vertices, cut consistency between resolutions, canonical plane
//! order, input validation and closed-form volumes.
//!
//! Every expectation is derived by hand in the test that uses it.

use std::f64::consts::PI;

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::{
    BoxFace, RoughBase, RoughCut, RoughModel, ShapeError,
    base::{
        COARSE_PEBBLE_FREQUENCY, COARSE_SIDES, FINE_PEBBLE_FREQUENCY, FINE_SIDES, pebble_vertices,
    },
    is_sliver,
    pebble_offsets::{PEBBLE_OFFSETS_COARSE, PEBBLE_OFFSETS_FINE},
    plane_order,
    sampling::{half_step_cos, pebble_directions},
};
use crate::rough_plan::{Axis, PlanSettings, shaped::ShapedCtx};

pub(super) fn block(x: f64, y: f64, z: f64, cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        },
        cuts,
    )
}

fn cylinder(axis: Axis, cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 20.0,
            axis,
        },
        cuts,
    )
}

fn pebble(x: f64, y: f64, z: f64, cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Pebble {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        },
        cuts,
    )
}

pub(super) fn face(normal: [f64; 3], depth_mm: f64) -> RoughCut {
    RoughCut::Face { normal, depth_mm }
}

/// The vertices of `base` found by enumerating plane triples, in the rough frame.
fn enumerated_vertices(base: &RoughBase, coarse: bool) -> Vec<DVec3> {
    let planes = base.to_halfspaces(coarse).expect("base planes");
    let c = base.bounding_box_centre();
    let shifted: Vec<(DVec3, f64)> = planes.iter().map(|&(n, m)| (n, m - n.dot(c))).collect();
    let (_, verts) = measure_solid_with_vertices(&shifted).expect("base polytope");
    verts.into_iter().map(|v| v + c).collect()
}

/// Distance from `v` to the nearest point of `set`.
fn nearest(v: DVec3, set: &[DVec3]) -> f64 {
    set.iter()
        .map(|&w| (w - v).length())
        .fold(f64::INFINITY, f64::min)
}

/// The exact bit pattern of every plane, for bitwise comparison.
fn plane_bits(planes: &[(DVec3, f64)]) -> Vec<[u64; 4]> {
    planes
        .iter()
        .map(|&(n, m)| [n.x.to_bits(), n.y.to_bits(), n.z.to_bits(), m.to_bits()])
        .collect()
}

fn bits3(v: [f64; 3]) -> [u64; 3] {
    v.map(f64::to_bits)
}

/// The pinned offset `h` of the pebble plane at geodesic `frequency` whose direction is the
/// coordinate `axis` (the unit polytope's extent along that axis).
fn axis_offset(frequency: usize, axis: [f64; 3]) -> f64 {
    let table: &[f64] = if frequency == FINE_PEBBLE_FREQUENCY {
        &PEBBLE_OFFSETS_FINE
    } else {
        &PEBBLE_OFFSETS_COARSE
    };
    let index = pebble_directions(frequency)
        .iter()
        .position(|d| (0..3).all(|a| (d[a] - axis[a]).abs() < 1e-9))
        .expect("an even frequency contains the coordinate axes");
    table[index]
}

#[test]
fn analytic_base_vertices_are_the_enumerated_ones() {
    let bases = [
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 9.0,
            z_mm: 8.0,
        },
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 20.0,
            axis: Axis::X,
        },
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 20.0,
            axis: Axis::Y,
        },
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 20.0,
            axis: Axis::Z,
        },
        RoughBase::Pebble {
            x_mm: 20.0,
            y_mm: 14.0,
            z_mm: 10.0,
        },
    ];
    for base in &bases {
        for coarse in [false, true] {
            let analytic = base.vertices(coarse).expect("analytic vertices");
            let enumerated = enumerated_vertices(base, coarse);
            for &v in &analytic {
                assert!(
                    nearest(v, &enumerated) < 1e-9,
                    "{base:?} coarse={coarse}: analytic vertex {v:?} is not an enumerated one"
                );
            }
            for &v in &enumerated {
                assert!(
                    nearest(v, &analytic) < 1e-9,
                    "{base:?} coarse={coarse}: enumerated vertex {v:?} is missing"
                );
            }

            // Each vertex is feasible and lies on at least three planes.
            let planes = base.to_halfspaces(coarse).expect("base planes");
            for &v in &analytic {
                let mut on_plane = 0;
                for &(n, m) in &planes {
                    let slack = m - n.dot(v);
                    assert!(slack > -1e-9, "{base:?}: vertex {v:?} outside a plane");
                    if slack < 1e-9 {
                        on_plane += 1;
                    }
                }
                assert!(on_plane >= 3, "{base:?}: vertex {v:?} on {on_plane} planes");
            }
        }
    }
}

#[test]
fn base_vertex_counts_follow_the_resolution_constants() {
    let block_base = RoughBase::Block {
        x_mm: 3.0,
        y_mm: 4.0,
        z_mm: 5.0,
    };
    assert_eq!(block_base.vertices(false).expect("block").len(), 8);

    // A prism with n sides has 2 n vertices; its planes are the two caps and n sides.
    let cyl = RoughBase::Cylinder {
        diameter_mm: 10.0,
        length_mm: 20.0,
        axis: Axis::Z,
    };
    for (coarse, sides) in [(true, COARSE_SIDES), (false, FINE_SIDES)] {
        assert_eq!(cyl.vertices(coarse).expect("cylinder").len(), 2 * sides);
        assert_eq!(cyl.to_halfspaces(coarse).expect("planes").len(), 2 + sides);
    }

    // Frequency f has 10 f^2 + 2 directions: 42 at frequency 2, 162 at frequency 4.
    let peb = RoughBase::Pebble {
        x_mm: 10.0,
        y_mm: 10.0,
        z_mm: 10.0,
    };
    for (coarse, frequency) in [
        (true, COARSE_PEBBLE_FREQUENCY),
        (false, FINE_PEBBLE_FREQUENCY),
    ] {
        let expected = 10 * frequency * frequency + 2;
        assert_eq!(pebble_directions(frequency).len(), expected);
        assert_eq!(peb.to_halfspaces(coarse).expect("planes").len(), expected);
    }
    assert_eq!(COARSE_PEBBLE_FREQUENCY, 2);
    assert_eq!(FINE_PEBBLE_FREQUENCY, 4);
    assert_eq!(COARSE_SIDES, 16);
    assert_eq!(FINE_SIDES, 64);
}

#[test]
fn pebble_vertices_are_cached_and_scale_with_the_box() {
    let first = pebble_vertices(20.0, 14.0, 10.0, FINE_PEBBLE_FREQUENCY).expect("vertices");
    let again = pebble_vertices(20.0, 14.0, 10.0, FINE_PEBBLE_FREQUENCY).expect("vertices");
    let bits = |vs: &[DVec3]| -> Vec<[u64; 3]> { vs.iter().map(|v| bits3(v.to_array())).collect() };
    assert_eq!(bits(&first), bits(&again));

    // The +x plane is x <= 10 + 10 h_x (the x axis is a direction of the frequency-4 set and
    // the plane offset is centre + semi * h), so the largest vertex x is exactly that; the
    // -z plane is z >= 5 - 5 h_z.
    let h_x = axis_offset(FINE_PEBBLE_FREQUENCY, [1.0, 0.0, 0.0]);
    let h_z = axis_offset(FINE_PEBBLE_FREQUENCY, [0.0, 0.0, -1.0]);
    let max_x = first.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (max_x - 10.0f64.mul_add(h_x, 10.0)).abs() < 1e-9,
        "max x {max_x}"
    );
    let min_z = first.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    assert!(
        (min_z - 5.0f64.mul_add(-h_z, 5.0)).abs() < 1e-9,
        "min z {min_z}"
    );
}

#[test]
fn invalid_bases_have_no_vertices() {
    let flat = RoughBase::Block {
        x_mm: 0.0,
        y_mm: 1.0,
        z_mm: 1.0,
    };
    assert_eq!(flat.vertices(false), Err(ShapeError::NonPositiveSize));
    let huge = RoughBase::Pebble {
        x_mm: 2001.0,
        y_mm: 1.0,
        z_mm: 1.0,
    };
    assert_eq!(huge.vertices(true), Err(ShapeError::TooLarge));
}

#[test]
fn a_face_cut_without_base_vertices_is_rejected() {
    let base = RoughBase::Block {
        x_mm: 10.0,
        y_mm: 10.0,
        z_mm: 10.0,
    };
    let cut = face([0.0, 1.0, 0.0], 1.0);
    assert_eq!(
        cut.to_halfspace(0, &base, &[]),
        Err(ShapeError::NothingLeft)
    );
}

#[test]
fn a_face_cut_is_the_same_plane_on_the_coarse_and_the_fine_pebble() {
    // The +x plane of a 10 mm pebble sits at 5 + 5 h and the -x plane at 5 - 5 h, with h the
    // pinned offset of the x axis, so the thickness along x is 10 h. A 9.4 mm deep cut fits
    // the fine pebble (thickness above 9.4 mm) but not the coarse one (below it).
    let h_fine = axis_offset(FINE_PEBBLE_FREQUENCY, [1.0, 0.0, 0.0]);
    let h_coarse = axis_offset(COARSE_PEBBLE_FREQUENCY, [1.0, 0.0, 0.0]);
    println!(
        "x thickness: fine {} mm, coarse {} mm",
        10.0 * h_fine,
        10.0 * h_coarse
    );
    assert!(10.0 * h_fine > 9.4 && 10.0 * h_coarse < 9.4);
    let cut = face([1.0, 0.0, 0.0], 9.4);
    let model = pebble(10.0, 10.0, 10.0, vec![cut.clone()]);
    let base = model.base;

    let coarse_only = cut.to_halfspace(0, &base, &base.vertices(true).expect("coarse vertices"));
    assert!(
        matches!(coarse_only, Err(ShapeError::BadDepth { .. })),
        "cutting against the coarse pebble alone is too deep: {coarse_only:?}"
    );

    let fine = model.halfspaces().expect("fine region");
    let coarse = model.coarse_halfspaces().expect("coarse region");
    assert_eq!(fine.len(), 162 + 1);
    assert_eq!(coarse.len(), 42 + 1);
    let fine_cut = fine.last().copied().expect("cut plane");
    let coarse_cut = coarse.last().copied().expect("cut plane");
    assert_eq!(plane_bits(&[fine_cut]), plane_bits(&[coarse_cut]));

    assert_eq!(fine_cut.0, DVec3::X);
    assert!(
        (fine_cut.1 - (5.0f64.mul_add(h_fine, 5.0) - 9.4)).abs() < 1e-9,
        "cut offset {}",
        fine_cut.1
    );
    model.measure().expect("the fine model has material left");
}

#[test]
fn a_face_cut_is_the_same_plane_on_the_coarse_and_the_fine_cylinder() {
    let model = cylinder(Axis::Y, vec![face([0.3, 1.0, 0.2], 2.0)]);
    let fine = model.halfspaces().expect("fine region");
    let coarse = model.coarse_halfspaces().expect("coarse region");
    assert_eq!(fine.len(), 2 + FINE_SIDES + 1);
    assert_eq!(coarse.len(), 2 + COARSE_SIDES + 1);
    assert_eq!(
        plane_bits(&[*fine.last().expect("plane")]),
        plane_bits(&[*coarse.last().expect("plane")])
    );

    // The plane is 2 mm inside the fine prism's outermost point along the normal.
    let (n, m) = fine[fine.len() - 1];
    let verts = model.base.vertices(false).expect("vertices");
    let h = verts
        .iter()
        .map(|&v| n.dot(v))
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((m - (h - 2.0)).abs() < 1e-12);
}

#[test]
fn an_oblique_face_cut_removes_the_closed_form_wedge() {
    // Cube of 10 mm, normal (1, 1, 0) / sqrt 2, depth 3. The outermost vertex along the
    // normal is (10, 10, z) at 20 / sqrt 2, so the kept side is (x + y) / sqrt 2 <= 20 / sqrt 2
    // - 3, i.e. x + y <= 20 - t with t = 3 sqrt 2 = 4.243 < 10. The removed part is a
    // triangle of legs t (area t^2 / 2 = 9) extruded over z = 10: 90 mm^3.
    let model = block(10.0, 10.0, 10.0, vec![face([1.0, 1.0, 0.0], 3.0)]);
    let measure = model.measure().expect("measure");
    assert!(
        (measure.volume_mm3 - 910.0).abs() < 1e-8,
        "volume {}",
        measure.volume_mm3
    );
    for extent in measure.extents_mm {
        assert!(
            (extent - 10.0).abs() < 1e-9,
            "extents {:?}",
            measure.extents_mm
        );
    }
}

#[test]
fn a_redundant_cut_changes_nothing_but_the_plane_count() {
    // Both face cuts are measured from the base: y <= 10 - 2 = 8 and y <= 10 - 1 = 9. The
    // second is implied by the first. Kept: 10 x 8 x 10 = 800 mm^3 with 8 vertices.
    let deep = block(10.0, 10.0, 10.0, vec![face([0.0, 1.0, 0.0], 2.0)]);
    let both = block(
        10.0,
        10.0,
        10.0,
        vec![face([0.0, 1.0, 0.0], 2.0), face([0.0, 1.0, 0.0], 1.0)],
    );
    let reference = deep.measure().expect("one cut");
    let redundant = both.measure().expect("two cuts");
    assert!((reference.volume_mm3 - 800.0).abs() < 1e-9);
    assert!((redundant.volume_mm3 - 800.0).abs() < 1e-9);
    assert_eq!(reference.vertex_count, 8);
    assert_eq!(redundant.vertex_count, 8);
    assert_eq!(reference.plane_count + 1, redundant.plane_count);
    for (a, b) in reference.extents_mm.iter().zip(&redundant.extents_mm) {
        assert!((a - b).abs() < 1e-9);
    }
}

#[test]
fn cylinders_along_every_axis_have_the_prism_volume_and_extents() {
    // A regular 64-gon of circumradius r = 5 has area (64 / 2) r^2 sin(2 pi / 64). Its
    // planes have normals at multiples of 2 pi / 64 and apothem r cos(pi / 64), so the
    // extent across the axis is 2 r cos(pi / 64) in both perpendicular directions.
    let r = 5.0;
    let area = 32.0 * r * r * (2.0 * PI / 64.0).sin();
    let cross = 2.0 * r * half_step_cos(64);
    for (axis, axis_index) in [(Axis::X, 0), (Axis::Y, 1), (Axis::Z, 2)] {
        let measure = cylinder(axis, Vec::new()).measure().expect("measure");
        assert!(
            area.mul_add(-20.0, measure.volume_mm3).abs() < 1e-9 * area * 20.0,
            "{axis:?}: volume {}",
            measure.volume_mm3
        );
        let mut expected = [cross; 3];
        expected[axis_index] = 20.0;
        for (got, want) in measure.extents_mm.iter().zip(expected) {
            assert!(
                (got - want).abs() < 1e-9,
                "{axis:?}: {:?}",
                measure.extents_mm
            );
        }
        assert_eq!(measure.plane_count, 2 + FINE_SIDES);
        assert_eq!(measure.vertex_count, 2 * FINE_SIDES);
    }
}

/// `measure()` is defined as `measure_with_halfspaces(&halfspaces())`, so the two agree bit
/// for bit. The expectation needs no derived number: both sides are compared through
/// `to_bits`. The models are a cylinder with a face cut (edge cuts exist only on blocks),
/// a block with an edge cut, and a pebble with a face cut, each cut well inside its rough
/// (a 1 mm face cut on a pebble about 10 mm thick along y). The cut-order list is passed, not the canonical one, and a
/// reversed copy is passed as well, so the sort inside the call is exercised.
#[test]
fn measure_with_halfspaces_is_bit_identical_to_measure() {
    let models = [
        cylinder(Axis::Y, vec![face([0.3, 1.0, 0.2], 2.0)]),
        block(
            12.0,
            9.0,
            8.0,
            vec![RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [2.0, 3.0],
            }],
        ),
        pebble(12.0, 10.0, 8.0, vec![face([0.0, 1.0, 0.0], 1.0)]),
    ];
    for model in &models {
        let reference = model.measure().expect("measure");
        let planes = model.halfspaces().expect("planes");
        let mut reversed = planes.clone();
        reversed.reverse();
        for list in [&planes, &reversed] {
            let other = model.measure_with_halfspaces(list).expect("measure planes");
            assert_eq!(reference.volume_mm3.to_bits(), other.volume_mm3.to_bits());
            for (a, b) in reference.extents_mm.iter().zip(&other.extents_mm) {
                assert_eq!(a.to_bits(), b.to_bits());
            }
            assert_eq!(reference.plane_count, other.plane_count);
            assert_eq!(reference.vertex_count, other.vertex_count);
        }
    }
}

/// The four-cut block of the order-independence tests, in the given `order`.
fn four_cut_block(order: &[usize; 4]) -> RoughModel {
    let cuts = [
        RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [2.0, 3.0],
        },
        RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
            setbacks_mm: [1.5, 2.0, 2.5],
        },
        face([1.0, 0.7, -0.3], 1.3),
        RoughCut::Corner {
            faces: [BoxFace::Top, BoxFace::Right, BoxFace::Front],
            setbacks_mm: [1.1, 2.3, 1.7],
        },
    ];
    block(
        12.0,
        9.0,
        8.0,
        order.iter().map(|&i| cuts[i].clone()).collect(),
    )
}

/// A cylinder cut by three faces, in the given `order`.
fn three_face_cylinder(order: &[usize; 3]) -> RoughModel {
    let cuts = [
        face([0.3, 1.0, 0.2], 2.0),
        face([-0.4, -0.2, 1.0], 1.5),
        face([0.5, -1.0, -0.3], 1.0),
    ];
    cylinder(Axis::X, order.iter().map(|&i| cuts[i].clone()).collect())
}

/// Asserts that `ctx` is bit-identical to `reference` and to the model's own measurement.
fn assert_ctx_matches(
    model: &RoughModel,
    settings: &PlanSettings,
    reference: &ShapedCtx,
    label: &str,
) {
    let ctx = ShapedCtx::new(model, settings).expect("ctx");
    assert_eq!(
        ctx.model_volume.to_bits(),
        reference.model_volume.to_bits(),
        "{label}: volume"
    );
    assert_eq!(bits3(ctx.bbox_min), bits3(reference.bbox_min), "{label}");
    assert_eq!(
        bits3(ctx.bbox_extents),
        bits3(reference.bbox_extents),
        "{label}"
    );
    assert_eq!(
        plane_bits(&ctx.usable),
        plane_bits(&reference.usable),
        "{label}"
    );
    assert_eq!(
        plane_bits(&ctx.non_box),
        plane_bits(&reference.non_box),
        "{label}"
    );

    let measure = model.measure().expect("measure");
    assert_eq!(ctx.model_volume.to_bits(), measure.volume_mm3.to_bits());
    assert_eq!(bits3(ctx.bbox_extents), bits3(measure.extents_mm));

    let inset = settings.skin_mm + settings.allowance_mm;
    let canonical = model.canonical_usable_halfspaces(inset).expect("canonical");
    assert_eq!(plane_bits(&ctx.usable), plane_bits(&canonical), "{label}");
}

#[test]
fn the_shaped_context_does_not_depend_on_the_cut_order() {
    let settings = PlanSettings {
        skin_mm: 0.1,
        ..PlanSettings::default()
    };

    let block_orders: [[usize; 4]; 6] = [
        [0, 1, 2, 3],
        [3, 2, 1, 0],
        [1, 0, 3, 2],
        [2, 3, 0, 1],
        [1, 2, 3, 0],
        [3, 0, 2, 1],
    ];
    let reference = ShapedCtx::new(&four_cut_block(&block_orders[0]), &settings).expect("ctx");
    assert_eq!(
        reference.non_box.len(),
        4,
        "exactly the four cut planes are not box planes"
    );
    for order in &block_orders {
        assert_ctx_matches(
            &four_cut_block(order),
            &settings,
            &reference,
            &format!("{order:?}"),
        );
    }

    let cylinder_orders: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let reference =
        ShapedCtx::new(&three_face_cylinder(&cylinder_orders[0]), &settings).expect("ctx");
    for order in &cylinder_orders {
        assert_ctx_matches(
            &three_face_cylinder(order),
            &settings,
            &reference,
            &format!("{order:?}"),
        );
    }
}

#[test]
fn canonical_halfspaces_are_sorted_and_cut_order_halfspaces_are_not() {
    let forward = four_cut_block(&[0, 1, 2, 3]);
    let backward = four_cut_block(&[3, 2, 1, 0]);

    // Six base planes come first and the cuts follow in the order they were given.
    let f = forward.halfspaces().expect("planes");
    let b = backward.halfspaces().expect("planes");
    assert_eq!(f.len(), 10);
    for k in 0..4 {
        assert_eq!(
            plane_bits(&[f[6 + k]]),
            plane_bits(&[b[6 + 3 - k]]),
            "cut {k} moves with its position"
        );
    }

    for model in [&forward, &backward] {
        for coarse in [false, true] {
            let result = if coarse {
                model.canonical_coarse_halfspaces()
            } else {
                model.canonical_halfspaces()
            };
            let planes = result.expect("canonical planes");
            assert!(
                planes.windows(2).all(|w| plane_order(&w[0], &w[1]).is_le()),
                "coarse={coarse}: not in canonical order"
            );
        }
    }
    assert_eq!(
        plane_bits(&forward.canonical_halfspaces().expect("planes")),
        plane_bits(&backward.canonical_halfspaces().expect("planes"))
    );
    assert_eq!(
        plane_bits(
            &forward
                .canonical_coarse_usable_halfspaces(0.3)
                .expect("planes")
        ),
        plane_bits(
            &backward
                .canonical_coarse_usable_halfspaces(0.3)
                .expect("planes")
        )
    );

    // The canonical list holds the same planes as the cut-order list.
    let mut sorted = f;
    sorted.sort_by(plane_order);
    assert_eq!(
        plane_bits(&sorted),
        plane_bits(&forward.canonical_halfspaces().expect("planes"))
    );
}

#[test]
fn a_negative_or_non_finite_inset_is_rejected() {
    let model = block(10.0, 10.0, 10.0, vec![face([0.0, 1.0, 0.0], 2.0)]);
    for inset in [-0.1, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        let results = [
            model.usable_halfspaces(inset),
            model.coarse_usable_halfspaces(inset),
            model.canonical_usable_halfspaces(inset),
            model.canonical_coarse_usable_halfspaces(inset),
        ];
        for result in results {
            assert!(
                matches!(result, Err(ShapeError::BadInset { inset_mm }) if inset_mm.to_bits() == inset.to_bits()),
                "inset {inset}: {result:?}"
            );
        }
    }
    assert_eq!(
        ShapeError::BadInset { inset_mm: -0.1 }.to_string(),
        "The inset (-0.1) must be a non-negative, finite length."
    );

    // Zero is a valid inset and moves nothing.
    let plain = model.halfspaces().expect("planes");
    let zero = model.usable_halfspaces(0.0).expect("zero inset");
    assert_eq!(plane_bits(&plain), plane_bits(&zero));

    // The shaped context insets by skin + allowance and refuses a negative sum.
    let settings = PlanSettings {
        allowance_mm: -0.5,
        ..PlanSettings::default()
    };
    assert_eq!(
        ShapedCtx::new(&model, &settings),
        Err(ShapeError::BadInset { inset_mm: -0.5 })
    );
}

#[test]
fn slivers_are_nothing_left() {
    // The limit is a billionth of the base bounding box: 1e-9 x 10^3 = 1e-6 mm^3 for a
    // 10 mm cube.
    let cube = [10.0, 10.0, 10.0];
    assert!(is_sliver(9e-7, cube));
    assert!(is_sliver(0.0, cube));
    assert!(!is_sliver(1.1e-6, cube));
    assert!(!is_sliver(1.0, cube));

    // A 1000 mm cube cut to a slab 1e-7 mm thick holds 1e6 x 1e-7 = 0.1 mm^3, under the
    // 1 mm^3 limit; the vertex enumeration may already fail on such planes, which is also
    // "nothing left".
    let sliver = block(
        1000.0,
        1000.0,
        1000.0,
        vec![face([0.0, 1.0, 0.0], 1000.0 - 1e-7)],
    );
    assert_eq!(sliver.measure(), Err(ShapeError::NothingLeft));
    assert_eq!(sliver.solid(), Err(ShapeError::NothingLeft));

    // A slab 1 mm thick holds 1e6 mm^3 and is fine.
    let slab = block(1000.0, 1000.0, 1000.0, vec![face([0.0, 1.0, 0.0], 999.0)]);
    let measure = slab.measure().expect("a 1 mm slab is a stone-sized piece");
    assert!(((measure.volume_mm3 - 1.0e6) / 1.0e6).abs() < 1e-9);
}

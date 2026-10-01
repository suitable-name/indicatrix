//! Tests of the fit's edges: cancelling, degenerate designs, the width and weight of the
//! result, curved and cut regions, and the order of the region's planes.

use glam::DVec3;

use super::{
    DesignHull, EXACT_ORIENTATIONS, FitStage, POLL_EVERY, SingleFit, fit_single_stones,
    screen_designs,
    tests::{assert_rotation, fit_bits, make_box_hull},
};
use crate::{
    rough_plan::{
        BoxFace, PlanProgress, PlanSettings, RoughBase, RoughCut, RoughModel,
        shape::{
            base::{COARSE_PEBBLE_FREQUENCY, block_halfspaces, pebble_halfspaces},
            sampling::sphere_directions,
        },
    },
    yield_metrics::carat_weight,
};

/// Settings with the given minimum width and everything else at its default.
fn settings_with_min_width(min_width_mm: f64) -> PlanSettings {
    PlanSettings {
        min_width_mm,
        ..PlanSettings::default()
    }
}

/// The fits of `hulls` in `region` (used as its own coarse region), up to `keep` of them.
fn run_fit(
    region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
) -> Vec<SingleFit> {
    fit_single_stones(region, region, hulls, settings, keep, &mut |_| true)
        .expect("the fit is not cancelled")
}

/// Asserts that every vertex of `hull`, placed by `fit`, satisfies every plane of `region`
/// within the LP's own tolerance.
fn assert_inside(fit: &SingleFit, hull: &DesignHull, region: &[(DVec3, f64)]) {
    assert_rotation(&fit.pose.axes, 1e-12, "returned pose");
    let center = DVec3::from(fit.pose.center_mm);
    let axes = fit.pose.axes.map(DVec3::from);
    for vert in &hull.vertices {
        let point = center
            + fit.pose.mm_per_unit * (vert[0] * axes[0] + vert[1] * axes[1] + vert[2] * axes[2]);
        for &(normal, offset) in region {
            assert!(
                normal.dot(point) <= 1e-9_f64.mul_add(1.0 + offset.abs(), offset),
                "vertex {vert:?} of entry {} leaves the region",
                fit.entry_id
            );
        }
    }
}

#[test]
fn a_cancel_in_any_stage_ends_the_fit() {
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings::default();

    for stage in [FitStage::Screen, FitStage::Exact, FitStage::Polish] {
        let mut events_of_stage = 0;
        let result = fit_single_stones(
            &region,
            &region,
            std::slice::from_ref(&hull),
            &settings,
            1,
            &mut |event| match event {
                PlanProgress::Fit { stage: seen, .. } if seen == stage => {
                    events_of_stage += 1;
                    false
                }
                _ => true,
            },
        );
        assert!(
            result.is_none(),
            "cancelling in {stage:?} did not stop the fit"
        );
        assert_eq!(events_of_stage, 1, "{stage:?} went on after the cancel");
    }

    // The same run, left alone, completes.
    let fits = run_fit(&region, std::slice::from_ref(&hull), &settings, 1);
    assert_eq!(fits.len(), 1);
}

#[test]
fn the_exact_stage_polls_for_a_cancel_inside_a_design() {
    // Orientation indices 256, 512, ..., 8192 are polls: (8,256 - 1) / 256 = 32 of them, each
    // an `Exact` event with `done == 0`, besides the one that opens the stage.
    let polls_per_design = (EXACT_ORIENTATIONS - 1) / POLL_EVERY;
    assert_eq!(polls_per_design, 32);

    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings::default();

    let (mut zero_done, mut finished_designs) = (0, 0);
    let fits = fit_single_stones(
        &region,
        &region,
        std::slice::from_ref(&hull),
        &settings,
        1,
        &mut |event| {
            match event {
                PlanProgress::Fit {
                    stage: FitStage::Exact,
                    done: 0,
                    ..
                } => zero_done += 1,
                PlanProgress::Fit {
                    stage: FitStage::Exact,
                    ..
                } => finished_designs += 1,
                _ => {}
            }
            true
        },
    );
    assert!(fits.is_some());
    assert_eq!(zero_done, 1 + polls_per_design);
    assert_eq!(finished_designs, 1);

    // Cancelling on the first poll (the second `done == 0` event) stops inside the design,
    // before it is reported finished.
    let (mut zero_done, mut finished_designs) = (0, 0);
    let fits = fit_single_stones(
        &region,
        &region,
        std::slice::from_ref(&hull),
        &settings,
        1,
        &mut |event| match event {
            PlanProgress::Fit {
                stage: FitStage::Exact,
                done: 0,
                ..
            } => {
                zero_done += 1;
                zero_done < 2
            }
            PlanProgress::Fit {
                stage: FitStage::Exact,
                ..
            } => {
                finished_designs += 1;
                true
            }
            _ => true,
        },
    );
    assert!(fits.is_none());
    assert_eq!(zero_done, 2);
    assert_eq!(finished_designs, 0);
}

/// Designs that cannot be fitted: no vertices, a single point, two points (no volume), a
/// vertex that is NaN, and a single point that claims a volume (its supports are all zero,
/// so the scale is unbounded and the LP gives up).
fn degenerate_hulls() -> Vec<DesignHull> {
    let mut nan_box = make_box_hull(4, 1.0, 1.0, 1.0);
    nan_box.vertices[3][1] = f64::NAN;
    vec![
        DesignHull {
            entry_id: 1,
            vertices: Vec::new(),
            volume: 1.0,
            width: 1.0,
        },
        DesignHull {
            entry_id: 2,
            vertices: vec![[0.0; 3]],
            volume: 0.0,
            width: 0.0,
        },
        DesignHull {
            entry_id: 3,
            vertices: vec![[-0.5, 0.0, 0.0], [0.5, 0.0, 0.0]],
            volume: 0.0,
            width: 0.0,
        },
        nan_box,
        DesignHull {
            entry_id: 5,
            vertices: vec![[0.0; 3]],
            volume: 1.0,
            width: 1.0,
        },
    ]
}

#[test]
fn degenerate_designs_get_no_fit_and_no_panic() {
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = settings_with_min_width(0.5);

    for hull in degenerate_hulls() {
        let id = hull.entry_id;
        let single = std::slice::from_ref(&hull);
        assert!(
            run_fit(&region, single, &settings, 3).is_empty(),
            "degenerate design {id} was fitted"
        );
        let scores =
            screen_designs(&region, single, &settings, &mut |_| true).expect("screening completes");
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].0, id);
        assert_eq!(
            scores[0].1.to_bits(),
            0.0_f64.to_bits(),
            "degenerate design {id} scored"
        );
    }

    // Among good designs they are skipped, and the good ones are unaffected.
    let good = make_box_hull(9, 1.0, 2.0, 1.5);
    let alone = run_fit(&region, std::slice::from_ref(&good), &settings, 10);
    let mut hulls = degenerate_hulls();
    hulls.push(good);
    let mixed = run_fit(&region, &hulls, &settings, 10);
    assert_eq!(alone.len(), 1);
    assert_eq!(fit_bits(&mixed), fit_bits(&alone));
}

#[test]
fn a_width_exactly_at_the_minimum_is_kept() {
    // A unit cube in a 10 unit cube fills it: k = 10, a stone 10 wide. The search does not
    // depend on the minimum width, so the same fit comes back every time and the filter is
    // the only thing that differs: `>=` keeps it at its own width, and drops it for a
    // minimum a part in 10^12 above.
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);

    let reference = run_fit(
        &region,
        std::slice::from_ref(&hull),
        &settings_with_min_width(0.5),
        1,
    );
    assert_eq!(reference.len(), 1);
    let width = reference[0].pose.mm_per_unit * hull.width;
    assert!((width - 10.0).abs() < 1e-6, "width {width}");

    let at_width = run_fit(
        &region,
        std::slice::from_ref(&hull),
        &settings_with_min_width(width),
        1,
    );
    assert_eq!(fit_bits(&at_width), fit_bits(&reference));

    let above = run_fit(
        &region,
        std::slice::from_ref(&hull),
        &settings_with_min_width(width * (1.0 + 1e-12)),
        1,
    );
    assert!(above.is_empty(), "a fit narrower than the minimum was kept");
}

#[test]
fn the_carat_weight_is_the_shared_formula() {
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings {
        min_width_mm: 0.5,
        specific_gravity: 3.52,
        ..PlanSettings::default()
    };

    let fits = run_fit(&region, std::slice::from_ref(&hull), &settings, 1);
    assert_eq!(fits.len(), 1);
    let fit = &fits[0];
    assert_eq!(
        fit.carat.to_bits(),
        carat_weight(fit.volume_mm3, settings.specific_gravity).to_bits()
    );
    // 1000 mm^3 at specific gravity 3.52 is 1000 * 3.52 / 200 = 17.6 ct.
    assert!((fit.carat - 17.6).abs() < 1e-5, "carat {}", fit.carat);
}

#[test]
fn a_ball_like_stone_fills_a_pebble_to_its_inscribed_ball() {
    // The pebble is the coarse (frequency-2) polytope {d_j . (p - c) <= 5 h_j} for the 42
    // directions d_j with their pinned offsets h_j, c the centre of a 10 mm box. Its plane
    // distances from c are 5 h_j, the smallest of them the radius `inradius` of the ball
    // around c that lies inside the region (d . q <= |q|), so a stone inside the unit ball,
    // centred, fits at k = inradius. Every vertex of the polytope lies inside the unit ball,
    // so the region lies inside the ball of radius 5; the 258-point stone holds a ball of
    // radius at least 0.97 (its faces are triangles of angular circumradius about 10
    // degrees, cos 10 degrees = 0.985), which fits in the ball of radius 5 only for
    // k <= 5 / 0.97 = 5.155.
    let region =
        pebble_halfspaces(10.0, 10.0, 10.0, COARSE_PEBBLE_FREQUENCY).expect("pebble halfspaces");
    assert_eq!(region.len(), 42);
    let centre = DVec3::splat(5.0);
    let inradius = region
        .iter()
        .map(|&(normal, offset)| offset - normal.dot(centre))
        .fold(f64::INFINITY, f64::min);

    let vertices: Vec<[f64; 3]> = sphere_directions(8);
    let hull = DesignHull {
        entry_id: 1,
        vertices,
        volume: 4.0 / 3.0 * std::f64::consts::PI,
        width: 2.0,
    };
    let fits = run_fit(
        &region,
        std::slice::from_ref(&hull),
        &settings_with_min_width(0.5),
        1,
    );
    assert_eq!(fits.len(), 1);

    let scale = fits[0].pose.mm_per_unit;
    assert!(
        scale >= inradius * (1.0 - 1e-9),
        "k = {scale} below the inscribed ball ({inradius})"
    );
    assert!(scale <= 5.0 / 0.97, "k = {scale} above the inradius bound");
    assert_inside(&fits[0], &hull, &region);
}

/// A 10 mm block with a flat face sawn 4 mm off the top (+Y).
fn block_with_sawn_top() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 4.0,
        }],
    )
}

#[test]
fn a_face_cut_limits_the_stone() {
    // Inset 0.5 mm the block keeps x and z in [0.5, 9.5] and, with the top sawn to 6 mm, y in
    // [0.5, 5.5]: a 9 x 5 x 9 box with two negative low-side offsets. A cube cannot be wider
    // than the 5 mm slab in any orientation and fits it axis-aligned, so k = 5, volume 125.
    let model = block_with_sawn_top();
    let region = model.usable_halfspaces(0.5).expect("usable halfspaces");
    let coarse = model
        .coarse_usable_halfspaces(0.5)
        .expect("coarse usable halfspaces");
    assert_eq!(region.len(), 7);

    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let fits = fit_single_stones(
        &region,
        &coarse,
        std::slice::from_ref(&hull),
        &settings_with_min_width(0.5),
        1,
        &mut |_| true,
    )
    .expect("the fit is not cancelled");
    assert_eq!(fits.len(), 1);
    assert!(
        (fits[0].pose.mm_per_unit - 5.0).abs() < 1e-6,
        "k = {}",
        fits[0].pose.mm_per_unit
    );
    assert!((fits[0].volume_mm3 - 125.0).abs() < 1e-3);
    assert_inside(&fits[0], &hull, &region);
}

/// Every bit of a plane list.
fn plane_bits(planes: &[(DVec3, f64)]) -> Vec<[u64; 4]> {
    planes
        .iter()
        .map(|&(n, m)| [n.x.to_bits(), n.y.to_bits(), n.z.to_bits(), m.to_bits()])
        .collect()
}

#[test]
fn the_order_of_the_cuts_does_not_change_the_fit() {
    // The same three cuts listed in two orders make the same plane set in two orders. The
    // LP's rows follow the plane order, so the fit is only bit-identical when the planes
    // reach it in a canonical order: the model's canonical halfspaces are that order (the
    // plain `usable_halfspaces` keep the cut order, which the mesh's facet ids index).
    let cuts = vec![
        RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [4.0, 4.0],
        },
        RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
            setbacks_mm: [6.0, 6.0, 6.0],
        },
        RoughCut::Face {
            normal: [1.0, 0.0, 1.0],
            depth_mm: 3.0,
        },
    ];
    let mut reversed = cuts.clone();
    reversed.reverse();
    let base = RoughBase::Block {
        x_mm: 20.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };

    let regions = [cuts, reversed].map(|list| {
        let model = RoughModel::new(base, list);
        let usable = model
            .canonical_usable_halfspaces(0.5)
            .expect("canonical usable halfspaces");
        let coarse = model
            .canonical_coarse_usable_halfspaces(0.5)
            .expect("canonical coarse usable halfspaces");
        (usable, coarse)
    });
    assert_eq!(plane_bits(&regions[0].0), plane_bits(&regions[1].0));
    assert_eq!(plane_bits(&regions[0].1), plane_bits(&regions[1].1));

    let hulls = [
        make_box_hull(1, 2.0, 3.0, 4.0),
        make_box_hull(2, 1.0, 1.0, 1.0),
    ];
    let settings = settings_with_min_width(0.1);
    let fits: Vec<Vec<SingleFit>> = regions
        .iter()
        .map(|(region, coarse)| {
            fit_single_stones(region, coarse, &hulls, &settings, 2, &mut |_| true)
                .expect("the fit is not cancelled")
        })
        .collect();
    assert_eq!(fits[0].len(), 2);
    assert_eq!(fit_bits(&fits[0]), fit_bits(&fits[1]));
}

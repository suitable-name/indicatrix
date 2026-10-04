use std::f64::consts::PI;

use glam::DVec3;

use super::{
    DesignHull, EXACT_LEVEL, EXACT_ORIENTATIONS, EXACT_SPINS, SCREEN_LEVEL, SCREEN_ORIENTATIONS,
    SCREEN_SPINS, SingleFit, StonePose, fit_shortlisted, fit_single_stones, merge_fits,
    orient::{Quat, generate_orientations},
    polish_orientation, screen_designs, shortlist,
    support::{SupportWorkspace, compute_proxy_vertices},
};
use crate::rough_plan::{
    Axis, PlanSettings,
    piece::{Norm, stone_scale},
    shape::{
        BoxFace, RoughBase, RoughCut, RoughModel, base::block_halfspaces,
        sampling::sphere_directions,
    },
};

/// A box design `width` (x) by `height` (y, the table normal) by `length` (z), centred on
/// the origin; its volume and width are the box's own.
pub(super) fn make_box_hull(entry_id: i64, width: f64, height: f64, length: f64) -> DesignHull {
    let half_w = width * 0.5;
    let half_h = height * 0.5;
    let half_l = length * 0.5;
    let vertices = vec![
        [-half_w, -half_h, -half_l],
        [-half_w, -half_h, half_l],
        [-half_w, half_h, -half_l],
        [-half_w, half_h, half_l],
        [half_w, -half_h, -half_l],
        [half_w, -half_h, half_l],
        [half_w, half_h, -half_l],
        [half_w, half_h, half_l],
    ];
    let volume = width * height * length;
    DesignHull {
        entry_id,
        vertices,
        volume,
        width,
    }
}

/// Asserts that `axes` are three orthonormal vectors forming a right-handed frame.
pub(super) fn assert_rotation(axes: &[[f64; 3]; 3], tol: f64, what: &str) {
    let frame: Vec<DVec3> = axes.iter().map(|a| DVec3::from(*a)).collect();
    for (i, a) in frame.iter().enumerate() {
        assert!(
            (a.length() - 1.0).abs() < tol,
            "{what}: axis {i} has length {}",
            a.length()
        );
        for (j, b) in frame.iter().enumerate().skip(i + 1) {
            assert!(
                a.dot(*b).abs() < tol,
                "{what}: axes {i} and {j} are not orthogonal"
            );
        }
    }
    let det = frame[0].dot(frame[1].cross(frame[2]));
    assert!((det - 1.0).abs() < tol, "{what}: determinant {det}");
}

/// Bit patterns of everything a [`SingleFit`] carries, so fits compare exactly.
pub(super) fn fit_bits(fits: &[SingleFit]) -> Vec<(i64, Vec<u64>)> {
    fits.iter()
        .map(|fit| {
            let mut bits = vec![
                fit.volume_mm3.to_bits(),
                fit.carat.to_bits(),
                fit.pose.mm_per_unit.to_bits(),
            ];
            bits.extend(fit.pose.center_mm.iter().map(|x| x.to_bits()));
            bits.extend(fit.pose.axes.iter().flatten().map(|x| x.to_bits()));
            (fit.entry_id, bits)
        })
        .collect()
}

#[test]
fn test_cube_stone_in_cube_region() {
    // Unit cube stone in a unit cube region: k = 1, volume equal.
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(1.0, 1.0, 1.0);
    let coarse_region = region.clone();
    // A minimum width below the stone's width, so rounding cannot drop the fit.
    let settings = PlanSettings {
        min_width_mm: 0.5,
        ..PlanSettings::default()
    };

    let fits = fit_single_stones(&region, &coarse_region, &[hull], &settings, 1, &mut |_| {
        true
    })
    .expect("fit must succeed");

    assert_eq!(fits.len(), 1);
    assert!(
        (fits[0].pose.mm_per_unit - 1.0).abs() < 1e-9,
        "expected k = 1.0, got {}",
        fits[0].pose.mm_per_unit
    );
    assert!(
        (fits[0].volume_mm3 - 1.0).abs() < 1e-9,
        "expected volume = 1.0, got {}",
        fits[0].volume_mm3
    );
    assert_rotation(&fits[0].pose.axes, 1e-12, "cube pose");
}

#[test]
fn test_box_stone_in_box_region() {
    // Box stone in box region: matches stone_scale of best of 6 assignments to 1e-9.
    let hull = make_box_hull(1, 1.0, 1.5, 2.0);
    let box_dims = [12.0, 15.0, 8.0];
    let region = block_halfspaces(box_dims[0], box_dims[1], box_dims[2]);
    let coarse_region = region.clone();
    let settings = PlanSettings::default();

    let norm = Norm {
        l: 2.0,
        h: 1.5,
        f: 3.0,
    };
    let expected_k = (0..6)
        .map(|orient| stone_scale(&norm, orient, box_dims))
        .fold(0.0_f64, f64::max);

    let fits = fit_single_stones(&region, &coarse_region, &[hull], &settings, 1, &mut |_| {
        true
    })
    .expect("fit must succeed");

    assert_eq!(fits.len(), 1);
    assert!(
        (fits[0].pose.mm_per_unit - expected_k).abs() < 1e-9,
        "expected k = {expected_k}, got {}",
        fits[0].pose.mm_per_unit
    );
}

#[test]
fn test_long_thin_box_in_cube() {
    // A long thin box in a cube fits diagonally with volume strictly greater than axis-aligned.
    let hull = make_box_hull(1, 1.0, 1.0, 12.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let coarse_region = region.clone();
    let settings = PlanSettings::default();

    // Axis-aligned max scale is limited by cube extent (10.0) over length (12.0)
    let axis_k = 10.0 / 12.0;
    let max_axis_aligned_vol = axis_k * axis_k * axis_k * hull.volume;

    let fits = fit_single_stones(&region, &coarse_region, &[hull], &settings, 1, &mut |_| {
        true
    })
    .expect("fit must succeed");

    assert_eq!(fits.len(), 1);
    assert!(
        fits[0].volume_mm3 > max_axis_aligned_vol + 0.5,
        "diagonal fit volume {} must exceed axis-aligned max {}",
        fits[0].volume_mm3,
        max_axis_aligned_vol
    );
    assert_rotation(&fits[0].pose.axes, 1e-12, "polished long-box pose");
}

#[test]
fn test_octahedron_stone_in_cylinder() {
    // Cylinder along Y, diameter 10 mm (R = 5), length 20 mm, modelled by the fine 64-gon
    // prism whose apothem is a = R cos(pi/64). A regular octahedron with vertices at
    // +-s e_i seen along a 3-fold axis (1, 1, 1) is a hexagon of circumradius s sqrt(2/3),
    // and that is the smallest circumradius over all viewing directions. Standing the
    // octahedron on a 3-fold axis along the cylinder axis therefore fits when the hexagon
    // fits the prism cross-section: s = a sqrt(3/2) (about 6.12 mm; its length along the
    // axis, 2 s / sqrt(3) = 7.07 mm, is far below 20 mm). Every hexagon vertex may sit
    // slightly outside the inscribed circle of the polygon, so the optimum is a hair
    // above V = 4/3 s^3, by at most 1 / cos^3(pi/64).
    let base = RoughBase::Cylinder {
        diameter_mm: 10.0,
        length_mm: 20.0,
        axis: Axis::Y,
    };
    let region = base.to_halfspaces(false).expect("fine cylinder halfspaces");
    let coarse_region = base
        .to_halfspaces(true)
        .expect("coarse cylinder halfspaces");
    let settings = PlanSettings::default();

    let vertices = vec![
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let hull = DesignHull {
        entry_id: 1,
        vertices,
        volume: 4.0 / 3.0,
        width: std::f64::consts::SQRT_2,
    };

    let fits = fit_single_stones(&region, &coarse_region, &[hull], &settings, 1, &mut |_| {
        true
    })
    .expect("fit must succeed");
    assert_eq!(fits.len(), 1);
    let fitted = fits[0].volume_mm3;

    let cos_half = (PI / 64.0).cos();
    let apothem = 5.0 * cos_half;
    let scale_3fold = apothem * 1.5_f64.sqrt();
    let vol_3fold = 4.0 / 3.0 * scale_3fold * scale_3fold * scale_3fold;
    let prism_volume = RoughModel::new(base, Vec::new())
        .measure()
        .expect("measure cylinder")
        .volume_mm3;

    // The bound stays at 98 %. The polish ends at a step of 0.0016 rad, but the scale is not
    // smooth at the optimum (six hexagon vertices touch the prism at once), so how close a
    // pattern search stops to it cannot be derived from the step alone.
    assert!(
        fitted >= 0.98 * vol_3fold,
        "fitted volume {fitted} below 98 % of the 3-fold configuration {vol_3fold}"
    );
    assert!(
        fitted <= vol_3fold / (cos_half * cos_half * cos_half) + 1e-9,
        "fitted volume {fitted} above the analytic ceiling for the 3-fold configuration {vol_3fold}"
    );
    assert!(fitted <= prism_volume, "stone larger than the prism");
    // Axis-aligned (a vertex on the cylinder axis) reaches only 500/3.
    assert!(fitted > 500.0 / 3.0 + 50.0, "no better than axis-aligned");
    assert_rotation(&fits[0].pose.axes, 1e-12, "octahedron pose");
}

#[test]
fn test_every_returned_stone_inside_region() {
    // All rotated, scaled, translated vertices must satisfy every region plane, within the
    // tolerance the LP post-check applies (1e-9 relative to the offset).
    let base = RoughBase::Cylinder {
        diameter_mm: 12.0,
        length_mm: 18.0,
        axis: Axis::Z,
    };
    let region = base.to_halfspaces(false).expect("region halfspaces");
    let coarse_region = base.to_halfspaces(true).expect("coarse halfspaces");
    let settings = PlanSettings::default();

    let hulls = vec![
        make_box_hull(1, 2.0, 3.0, 4.0),
        make_box_hull(2, 4.0, 4.0, 4.0),
    ];

    let fits = fit_single_stones(&region, &coarse_region, &hulls, &settings, 2, &mut |_| true)
        .expect("fit must succeed");

    assert_eq!(fits.len(), 2);
    for fit in &fits {
        let hull = hulls
            .iter()
            .find(|h| h.entry_id == fit.entry_id)
            .expect("fit refers to an input design");
        let scale = fit.pose.mm_per_unit;
        let center = DVec3::from(fit.pose.center_mm);
        let ax0 = DVec3::from(fit.pose.axes[0]);
        let ax1 = DVec3::from(fit.pose.axes[1]);
        let ax2 = DVec3::from(fit.pose.axes[2]);
        assert_rotation(&fit.pose.axes, 1e-12, "returned pose");

        for vert in &hull.vertices {
            let point = center + scale * (vert[0] * ax0 + vert[1] * ax1 + vert[2] * ax2);

            for &(normal, offset) in &region {
                let dot = normal.dot(point);
                assert!(
                    dot <= 1e-9_f64.mul_add(1.0 + offset.abs(), offset),
                    "plane violation for entry {}: dot={dot}, offset={offset}",
                    fit.entry_id
                );
            }
        }
    }
}

#[test]
fn test_minimum_width_filter() {
    // A long, very thin bar in a 10 mm cube cannot be scaled past its space diagonal
    // (17.3 mm / 12 units = 1.44 mm per unit), so its width stays below 0.08 mm, under the
    // 2 mm minimum. The big cube fits with a 10 mm wide stone.
    let large_hull = make_box_hull(1, 5.0, 5.0, 5.0);
    let thin_hull = make_box_hull(2, 0.05, 0.05, 12.0);

    let region = block_halfspaces(10.0, 10.0, 10.0);
    let coarse_region = region.clone();
    let hulls = [large_hull, thin_hull];

    let strict = PlanSettings {
        min_width_mm: 2.0,
        ..PlanSettings::default()
    };
    let fits = fit_single_stones(&region, &coarse_region, &hulls, &strict, 10, &mut |_| true)
        .expect("fit must succeed");
    assert_eq!(fits.len(), 1);
    assert_eq!(fits[0].entry_id, 1);

    // With no minimum the thin bar is kept, so it was the filter that dropped it.
    let lenient = PlanSettings {
        min_width_mm: 0.0,
        ..PlanSettings::default()
    };
    let fits = fit_single_stones(&region, &coarse_region, &hulls, &lenient, 10, &mut |_| true)
        .expect("fit must succeed");
    assert_eq!(fits.len(), 2);
    let thin_fit = fits
        .iter()
        .find(|fit| fit.entry_id == 2)
        .expect("thin bar present without a minimum");
    assert!(
        thin_fit.pose.mm_per_unit * 0.05 < 0.08,
        "thin bar unexpectedly wide: {} mm",
        thin_fit.pose.mm_per_unit * 0.05
    );
}

#[test]
fn test_shuffled_input_gives_identical_output() {
    let h1 = make_box_hull(1, 1.0, 1.0, 1.0);
    let h2 = make_box_hull(2, 2.0, 1.0, 1.5);
    let h3 = make_box_hull(3, 1.5, 2.0, 1.0);

    let region = block_halfspaces(10.0, 10.0, 10.0);
    let coarse_region = region.clone();
    let settings = PlanSettings::default();

    let fits_123 = fit_single_stones(
        &region,
        &coarse_region,
        &[h1.clone(), h2.clone(), h3.clone()],
        &settings,
        3,
        &mut |_| true,
    )
    .expect("fit 123");

    let fits_312 = fit_single_stones(
        &region,
        &coarse_region,
        &[h3, h1, h2],
        &settings,
        3,
        &mut |_| true,
    )
    .expect("fit 312");

    assert_eq!(
        fit_bits(&fits_123),
        fit_bits(&fits_312),
        "shuffled input order must produce identical output"
    );
}

/// `count` boxes of clearly different proportions with ids `first_id..`.
fn varied_box_hulls(count: u32, first_id: i64) -> Vec<DesignHull> {
    (0..count)
        .map(|i| {
            let width = 0.5 + 0.5 * f64::from((7 * i) % 11) / 10.0;
            let height = 0.5 + 0.9 * f64::from((3 * i) % 13) / 12.0;
            let length = 1.0 + 3.0 * f64::from((5 * i) % 17) / 16.0;
            make_box_hull(first_id + i64::from(i), width, height, length)
        })
        .collect()
}

/// Screens `hulls` in `lanes` contiguous chunks, takes one global shortlist over all the
/// scores, fits each chunk's shortlisted designs and merges: what a caller with `lanes`
/// workers does.
fn run_in_lanes(
    region: &[(DVec3, f64)],
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
    lanes: usize,
) -> Vec<SingleFit> {
    let chunks: Vec<&[DesignHull]> = hulls.chunks(hulls.len().div_ceil(lanes)).collect();
    let mut scores = Vec::new();
    for chunk in &chunks {
        scores.extend(
            screen_designs(coarse_region, chunk, settings, &mut |_| true).expect("screen chunk"),
        );
    }
    let chosen = shortlist(&scores, keep);
    let mut fits = Vec::new();
    for chunk in &chunks {
        let subset: Vec<DesignHull> = chunk
            .iter()
            .filter(|hull| chosen.contains(&hull.entry_id))
            .cloned()
            .collect();
        fits.extend(fit_shortlisted(region, &subset, settings, &mut |_| true).expect("fit chunk"));
    }
    merge_fits(fits, keep)
}

#[test]
fn test_chunked_lanes_equal_sequential() {
    // A cut block: seven planes, the low-side offsets negative after the inset.
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 20.0,
            z_mm: 20.0,
        },
        vec![RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
            setbacks_mm: [6.0, 6.0, 6.0],
        }],
    );
    let region = model.usable_halfspaces(0.5).expect("usable halfspaces");
    let coarse_region = model
        .coarse_usable_halfspaces(0.5)
        .expect("coarse usable halfspaces");
    let settings = PlanSettings::default();
    let hulls = varied_box_hulls(52, 100);
    let keep = 5;

    // More designs than the shortlist floor, so a per-chunk cut-off would keep more.
    let scores: Vec<(i64, f64)> = {
        let mut all = Vec::new();
        for chunk in hulls.chunks(18) {
            all.extend(
                screen_designs(&coarse_region, chunk, &settings, &mut |_| true)
                    .expect("screen chunk"),
            );
        }
        all
    };
    let global = shortlist(&scores, keep);
    assert_eq!(global.len(), 48);
    let per_chunk: usize = scores
        .chunks(18)
        .map(|chunk| shortlist(chunk, keep).len())
        .sum();
    assert!(
        per_chunk > global.len(),
        "per-chunk shortlists ({per_chunk}) must exceed the global one ({})",
        global.len()
    );

    let (region_ref, coarse_ref, hulls_ref, settings_ref) =
        (&region, &coarse_region, &hulls, &settings);
    let (wrapper, one, three, seven) = std::thread::scope(|scope| {
        let wrapper = scope.spawn(move || {
            fit_single_stones(
                region_ref,
                coarse_ref,
                hulls_ref,
                settings_ref,
                keep,
                &mut |_| true,
            )
            .expect("sequential fit")
        });
        let lanes = |count: usize| {
            scope.spawn(move || {
                run_in_lanes(region_ref, coarse_ref, hulls_ref, settings_ref, keep, count)
            })
        };
        let one = lanes(1);
        let three = lanes(3);
        let seven = lanes(7);
        (
            wrapper.join().expect("wrapper thread"),
            one.join().expect("1-lane thread"),
            three.join().expect("3-lane thread"),
            seven.join().expect("7-lane thread"),
        )
    });

    assert_eq!(wrapper.len(), keep);
    let reference = fit_bits(&wrapper);
    assert_eq!(fit_bits(&one), reference, "1 lane differs from sequential");
    assert_eq!(
        fit_bits(&three),
        reference,
        "3 lanes differ from sequential"
    );
    assert_eq!(
        fit_bits(&seven),
        reference,
        "7 lanes differ from sequential"
    );
}

#[test]
fn test_screening_recall() {
    // 51 designs of different proportions in a 20 mm cube (three more than the shortlist
    // floor of 48, so screening must discard some). Three are near-cubes and clearly the
    // best; the other 48 are long bars. The shortlisted search must return the same top
    // three as an exhaustive exact search of every design over all 8,256 orientations.
    let mut hulls = vec![
        make_box_hull(1, 1.0, 1.0, 1.0),
        make_box_hull(2, 1.0, 1.0, 1.25),
        make_box_hull(3, 1.0, 1.25, 1.25),
    ];
    for i in 0..48_u32 {
        let width = 0.4 + 0.6 * f64::from((7 * i) % 11) / 10.0;
        let height = 0.4 + 0.6 * f64::from((3 * i) % 13) / 12.0;
        let length = 2.0 + 2.0 * f64::from((5 * i) % 17) / 16.0;
        hulls.push(make_box_hull(4 + i64::from(i), width, height, length));
    }
    assert_eq!(hulls.len(), 51);

    let region = block_halfspaces(20.0, 20.0, 20.0);
    let coarse_region = region.clone();
    let settings = PlanSettings::default();

    let mut workspace = SupportWorkspace::new(region.len());
    let exact_orients = generate_orientations(EXACT_LEVEL, EXACT_SPINS);
    let mut exhaustive: Vec<(f64, i64)> = hulls
        .iter()
        .map(|hull| {
            let best_k = exact_orients
                .iter()
                .filter_map(|o| workspace.evaluate(&o.axes, &region, &hull.vertices))
                .map(|(k, _)| k)
                .fold(0.0_f64, f64::max);
            (best_k * best_k * best_k * hull.volume, hull.entry_id)
        })
        .collect();
    exhaustive.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let exhaustive_ids: Vec<i64> = exhaustive.iter().take(3).map(|&(_, id)| id).collect();
    assert_eq!(exhaustive_ids, [1, 3, 2]);
    assert!(
        exhaustive[2].0 > 1.5 * exhaustive[3].0,
        "top three must be clearly separated: {:?}",
        &exhaustive[..4]
    );

    // Screening really discards designs, and keeps the exhaustive top three.
    let scores = screen_designs(&coarse_region, &hulls, &settings, &mut |_| true)
        .expect("screening completes");
    let chosen = shortlist(&scores, 3);
    assert_eq!(chosen.len(), 48);
    for id in &exhaustive_ids {
        assert!(
            chosen.contains(id),
            "design {id} missing from the shortlist"
        );
    }

    let fits = fit_single_stones(&region, &coarse_region, &hulls, &settings, 3, &mut |_| true)
        .expect("fit must succeed");
    let fit_ids: Vec<i64> = fits.iter().map(|fit| fit.entry_id).collect();
    assert_eq!(fit_ids, exhaustive_ids);
    for (fit, &(exact_volume, _)) in fits.iter().zip(&exhaustive) {
        // Polishing never loses volume against the exact grid.
        assert!(
            fit.volume_mm3 >= exact_volume * (1.0 - 1e-9),
            "fit {} below exhaustive {exact_volume}",
            fit.volume_mm3
        );
    }
}

#[test]
fn test_shortlist_is_global_sorted_and_capped() {
    let scores: Vec<(i64, f64)> = (0..100_u32)
        .map(|i| (i64::from(i), f64::from(i % 10)))
        .collect();

    let picked = shortlist(&scores, 1);
    assert_eq!(picked.len(), 48, "floor of 48 designs");
    // Score 9 first (ids 9, 19, ...), ties by ascending id.
    assert_eq!(&picked[..4], &[9, 19, 29, 39]);
    let score_of = |id: i64| scores.iter().find(|s| s.0 == id).expect("known id").1;
    for pair in picked.windows(2) {
        let (a, b) = (score_of(pair[0]), score_of(pair[1]));
        assert!(
            a > b || (a == b && pair[0] < pair[1]),
            "order broken at {pair:?}"
        );
    }
    let worst_kept = picked
        .iter()
        .map(|&id| score_of(id))
        .fold(f64::INFINITY, f64::min);
    for (id, score) in &scores {
        if !picked.contains(id) {
            assert!(*score <= worst_kept, "design {id} dropped above a kept one");
        }
    }

    assert_eq!(shortlist(&scores, 20).len(), 80, "four times keep");
    assert_eq!(
        shortlist(&scores, 40).len(),
        100,
        "capped at the design count"
    );
    assert_eq!(
        shortlist(&scores[..10], 1).len(),
        10,
        "fewer designs than the floor"
    );
    assert_eq!(shortlist(&scores, 0), Vec::<i64>::new());
}

#[test]
fn test_fit_shortlisted_is_untruncated_and_merge_ranks() {
    let hulls = [
        make_box_hull(3, 1.5, 2.0, 1.0),
        make_box_hull(1, 1.0, 1.0, 1.0),
        make_box_hull(2, 2.0, 1.0, 1.5),
    ];
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings::default();

    let fits = fit_shortlisted(&region, &hulls, &settings, &mut |_| true).expect("fit");
    let ids: Vec<i64> = fits.iter().map(|fit| fit.entry_id).collect();
    assert_eq!(ids, [3, 1, 2], "one fit per design, in input order");

    let merged = merge_fits(fits.clone(), 2);
    assert_eq!(merged.len(), 2);
    assert!(merged[0].volume_mm3 >= merged[1].volume_mm3);
    for fit in &merged {
        assert!(fits.contains(fit));
    }

    // Equal volumes rank by ascending entry id.
    let pose = StonePose {
        center_mm: [0.0; 3],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit: 1.0,
    };
    let tied: Vec<SingleFit> = [7_i64, 4, 9]
        .iter()
        .map(|&entry_id| SingleFit {
            entry_id,
            pose,
            volume_mm3: 2.0,
            carat: 0.1,
        })
        .collect();
    let ranked = merge_fits(tied, 2);
    let ranked_ids: Vec<i64> = ranked.iter().map(|fit| fit.entry_id).collect();
    assert_eq!(ranked_ids, [4, 7]);
}

#[test]
fn test_orientation_sets_are_proper_rotations() {
    let screen = generate_orientations(SCREEN_LEVEL, SCREEN_SPINS);
    let exact = generate_orientations(EXACT_LEVEL, EXACT_SPINS);
    assert_eq!(screen.len(), SCREEN_ORIENTATIONS);
    assert_eq!(exact.len(), EXACT_ORIENTATIONS);
    assert_eq!(
        SCREEN_ORIENTATIONS,
        sphere_directions(SCREEN_LEVEL).len() * SCREEN_SPINS
    );
    assert_eq!(
        EXACT_ORIENTATIONS,
        sphere_directions(EXACT_LEVEL).len() * EXACT_SPINS
    );

    for (idx, orient) in screen.iter().chain(&exact).enumerate() {
        assert_rotation(&orient.axes, 1e-12, &format!("orientation {idx}"));
        // The stored quaternion describes the same frame.
        let from_quat = orient.quat.columns();
        for (got, want) in from_quat.iter().flatten().zip(orient.axes.iter().flatten()) {
            assert!(
                (got - want).abs() < 1e-12,
                "orientation {idx}: quaternion frame differs from stored axes"
            );
        }
    }
}

#[test]
fn test_proxy_has_at_most_26_unique_extreme_vertices() {
    // 258 vertices on a sphere of radius 2.
    let vertices: Vec<[f64; 3]> = sphere_directions(8)
        .iter()
        .map(|d| [2.0 * d[0], 2.0 * d[1], 2.0 * d[2]])
        .collect();
    assert_eq!(vertices.len(), 258);
    let proxy = compute_proxy_vertices(&vertices);
    assert!(proxy.len() <= 26, "proxy has {} vertices", proxy.len());
    assert!(proxy.len() >= 6, "proxy lost its axis extremes");

    let mut bits: Vec<[u64; 3]> = proxy.iter().map(|v| v.map(f64::to_bits)).collect();
    bits.sort_unstable();
    bits.dedup();
    assert_eq!(bits.len(), proxy.len(), "proxy vertices must be unique");
    for v in &proxy {
        assert!(
            vertices.contains(v),
            "proxy vertex {v:?} is not a hull vertex"
        );
    }
    for (axis, sign) in (0..3).flat_map(|axis| [(axis, 2.0), (axis, -2.0)]) {
        let mut want = [0.0; 3];
        want[axis] = sign;
        assert!(proxy.contains(&want), "missing axis extreme {want:?}");
    }
    assert_eq!(proxy, compute_proxy_vertices(&vertices), "deterministic");

    // A box keeps all eight corners; an empty hull gives an empty proxy.
    let cube = make_box_hull(1, 2.0, 3.0, 4.0);
    assert_eq!(compute_proxy_vertices(&cube.vertices).len(), 8);
    assert_eq!(compute_proxy_vertices(&[]), Vec::<[f64; 3]>::new());
}

#[test]
fn test_polish_never_decreases_scale() {
    let hull = make_box_hull(1, 1.0, 1.0, 12.0);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let mut workspace = SupportWorkspace::new(region.len());
    let orients = generate_orientations(SCREEN_LEVEL, SCREEN_SPINS);

    let mut checked = 0;
    let mut improved = 0;
    for orient in orients.iter().step_by(37) {
        let Some((k_start, t_start)) = workspace.evaluate(&orient.axes, &region, &hull.vertices)
        else {
            continue;
        };
        if k_start <= 0.0 {
            continue;
        }
        let (k_end, _, quat_end) = polish_orientation(
            orient.quat,
            k_start,
            t_start,
            &hull.vertices,
            &region,
            &mut workspace,
            None,
        );
        assert!(
            k_end >= k_start,
            "polish lowered the scale from {k_start} to {k_end}"
        );

        // The polished scale is what the polished frame really achieves.
        let axes_end = quat_end.columns();
        assert_rotation(&axes_end, 1e-12, "polished frame");
        let (k_check, _) = workspace
            .evaluate(&axes_end, &region, &hull.vertices)
            .expect("polished frame is feasible");
        assert!(
            (k_check - k_end).abs() < 1e-9 * (1.0 + k_end),
            "polished scale {k_end} not reproduced by its frame: {k_check}"
        );

        checked += 1;
        if k_end > k_start + 1e-9 {
            improved += 1;
        }
    }
    assert!(checked >= 20, "only {checked} orientations checked");
    assert!(improved > 0, "polish never improved any orientation");
}

#[test]
fn test_quat_math() {
    let q = Quat::IDENTITY;
    let v = [1.0, 2.0, 3.0];
    assert_eq!(q.rotate_vec(v), v);

    // 90 degrees around Z: (1, 0, 0) -> (0, 1, 0)
    let qz90 = Quat {
        w: std::f64::consts::FRAC_1_SQRT_2,
        x: 0.0,
        y: 0.0,
        z: std::f64::consts::FRAC_1_SQRT_2,
    };
    let rotated = qz90.rotate_vec([1.0, 0.0, 0.0]);
    assert!((rotated[0] - 0.0).abs() < 1e-12);
    assert!((rotated[1] - 1.0).abs() < 1e-12);
    assert!((rotated[2] - 0.0).abs() < 1e-12);

    let mat = qz90.to_matrix();
    let axes = qz90.columns();
    assert_eq!(axes[0], [mat[0][0], mat[1][0], mat[2][0]]);

    let q_roundtrip = Quat::from_axes(&axes);
    assert!((q_roundtrip.w - qz90.w).abs() < 1e-12);
    assert!((q_roundtrip.z - qz90.z).abs() < 1e-12);
}

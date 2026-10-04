//! Tests of the orientation search: pruning, basins, the polish and the screening score.

use std::f64::consts::PI;

use glam::DVec3;

use super::{
    BASIN_ALIGNMENT, EXACT_LEVEL, EXACT_ORIENTATIONS, EXACT_SPINS, OrientCandidate,
    POLISH_DIAGONAL_BELOW, POLISH_MIN_STEP, POLISH_START_STEP, SCREEN_LEVEL, SCREEN_ORIENTATIONS,
    SCREEN_SPINS, TOP_KEEP, insert_candidate,
    orient::{
        Quat, diagonal_perturbation, direction_count, generate_orientations, perturbation,
        screening_orientations,
    },
    polish_moves, polish_orientation, screen_designs, search_orientations, shortlist,
    support::{SupportWorkspace, compute_proxy_vertices, opposite_pairs},
    tests::{assert_rotation, make_box_hull},
};
use crate::rough_plan::{
    PlanSettings,
    shape::{
        BoxFace, RoughBase, RoughCut, RoughModel, base::block_halfspaces,
        sampling::sphere_directions,
    },
};

/// A rotation of `degrees` about world axis `axis` (0, 1 or 2).
fn about(axis: usize, degrees: f64) -> Quat {
    let (sin_half, cos_half) = (degrees.to_radians() * 0.5).sin_cos();
    let mut vector = [0.0; 3];
    vector[axis] = sin_half;
    Quat {
        w: cos_half,
        x: vector[0],
        y: vector[1],
        z: vector[2],
    }
}

/// The scales of `kept`, in order.
fn scales(kept: &[OrientCandidate]) -> Vec<f64> {
    kept.iter().map(|candidate| candidate.0).collect()
}

/// Every bit of a list of candidates, so two searches compare exactly.
fn candidate_bits(kept: &[OrientCandidate]) -> Vec<[u64; 8]> {
    kept.iter()
        .map(|&(k, quat, centre)| {
            [
                k.to_bits(),
                quat.w.to_bits(),
                quat.x.to_bits(),
                quat.y.to_bits(),
                quat.z.to_bits(),
                centre[0].to_bits(),
                centre[1].to_bits(),
                centre[2].to_bits(),
            ]
        })
        .collect()
}

#[test]
fn a_basin_keeps_only_its_best_orientation() {
    // cos(7.5 degrees), the alignment of two orientations 15 degrees apart.
    assert!((BASIN_ALIGNMENT - 7.5_f64.to_radians().cos()).abs() < 1e-12);

    let centre = [0.0; 3];
    let mut kept: Vec<OrientCandidate> = Vec::new();
    insert_candidate(&mut kept, (1.0, Quat::IDENTITY, centre), 4);
    // 2 degrees away (alignment cos 1 degree = 0.99985) and worse: the same basin, dropped.
    insert_candidate(&mut kept, (0.99, about(1, 2.0), centre), 4);
    assert_eq!(scales(&kept), [1.0]);
    // 3 degrees away and better: replaces the held orientation of its basin.
    insert_candidate(&mut kept, (1.01, about(0, 3.0), centre), 4);
    assert_eq!(scales(&kept), [1.01]);
    // 90 degrees away (alignment cos 45 degrees = 0.707): a new basin, even though worse.
    insert_candidate(&mut kept, (0.5, about(2, 90.0), centre), 4);
    // 1 degree from that one and worse: dropped.
    insert_candidate(&mut kept, (0.4, about(2, 91.0), centre), 4);
    assert_eq!(scales(&kept), [1.01, 0.5]);
}

#[test]
fn a_full_list_only_admits_a_better_basin() {
    // Rotations about z at 0, 30, 60 and 90 degrees are pairwise at least 30 degrees apart
    // (alignment cos 15 degrees = 0.966 < 0.9914), so each is a basin of its own.
    let centre = [0.0; 3];
    let mut kept: Vec<OrientCandidate> = Vec::new();
    for (scale, degrees) in [(5.0, 0.0), (4.0, 30.0), (3.0, 60.0), (2.0, 90.0)] {
        insert_candidate(&mut kept, (scale, about(2, degrees), centre), 4);
    }
    assert_eq!(scales(&kept), [5.0, 4.0, 3.0, 2.0]);

    // A fifth basin that does not beat the worst one changes nothing ...
    insert_candidate(&mut kept, (1.5, about(2, 120.0), centre), 4);
    assert_eq!(scales(&kept), [5.0, 4.0, 3.0, 2.0]);
    // ... one that does takes the place of the worst.
    insert_candidate(&mut kept, (2.5, about(2, 120.0), centre), 4);
    assert_eq!(scales(&kept), [5.0, 4.0, 3.0, 2.5]);
}

#[test]
fn a_candidate_between_two_basins_replaces_both() {
    // 0 and 16 degrees about z: alignment cos 8 degrees = 0.9903 < 0.9914, two basins.
    // A candidate at 8 degrees is 4 degrees (alignment 0.9976) from each, so it is in both.
    let centre = [0.0; 3];
    let mut kept: Vec<OrientCandidate> = Vec::new();
    insert_candidate(&mut kept, (2.0, about(2, 0.0), centre), 4);
    insert_candidate(&mut kept, (1.5, about(2, 16.0), centre), 4);
    assert_eq!(scales(&kept), [2.0, 1.5]);

    // Worse than one of them: dropped.
    insert_candidate(&mut kept, (1.8, about(2, 8.0), centre), 4);
    assert_eq!(scales(&kept), [2.0, 1.5]);

    // Better than both: they make way for it.
    insert_candidate(&mut kept, (3.0, about(2, 8.0), centre), 4);
    assert_eq!(scales(&kept), [3.0]);
}

#[test]
fn a_cube_in_a_cube_keeps_distinct_orientations() {
    // A unit cube in a unit cube fits with k = 1 in exactly the 24 orientations that map the
    // cube's axes onto the region's. The exact grid contains all of them: the six axis
    // directions, each with the spins 0, 90, 180 and 270 degrees of its 32. They are pairwise
    // at least 90 degrees apart, and every other grid orientation is tilted by at least a
    // few degrees and so has k < 0.95. The search therefore holds TOP_KEEP orientations of
    // scale 1, each in a basin of its own.
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(1.0, 1.0, 1.0);
    let orients = generate_orientations(EXACT_LEVEL, EXACT_SPINS);
    let pairs = opposite_pairs(&region);
    let mut workspace = SupportWorkspace::new(region.len());

    let (kept, _) = search_orientations(
        &hull,
        &orients,
        &region,
        &pairs,
        &mut workspace,
        None,
        &mut || true,
    )
    .expect("not cancelled");

    assert_eq!(kept.len(), TOP_KEEP);
    for (idx, (scale, quat, _)) in kept.iter().enumerate() {
        assert!(*scale > 1.0 - 1e-6, "orientation {idx} has k = {scale}");
        for (other_idx, (_, other, _)) in kept.iter().enumerate().skip(idx + 1) {
            assert!(
                quat.alignment(*other) < BASIN_ALIGNMENT,
                "orientations {idx} and {other_idx} share a basin"
            );
        }
    }
}

#[test]
fn opposite_planes_are_found_where_they_exist() {
    let block = block_halfspaces(12.0, 15.0, 8.0);
    assert_eq!(opposite_pairs(&block), [(0, 1), (2, 3), (4, 5)]);

    // A face cut facing up (+Y) is opposite the block's bottom (-Y, index 3) as well.
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 10.0,
            z_mm: 10.0,
        },
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 4.0,
        }],
    );
    let region = model.usable_halfspaces(0.5).expect("usable halfspaces");
    assert_eq!(region.len(), 7);
    assert_eq!(opposite_pairs(&region), [(0, 1), (2, 3), (3, 6), (4, 5)]);

    assert_eq!(opposite_pairs(&[]).len(), 0);
}

#[test]
fn the_scale_bound_is_the_tightest_slab() {
    // A unit cube with its axes on the region's: supports 0.5 + 0.5 = 1 along every axis, so
    // the slabs 12, 15 and 8 wide bound k by 12, 15 and 8, and the smallest is 8 -- which is
    // also what the LP returns.
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(12.0, 15.0, 8.0);
    let pairs = opposite_pairs(&region);
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    let mut workspace = SupportWorkspace::new(region.len());
    assert!(workspace.prepare(&identity, &region, &hull.vertices));
    let bound = workspace.scale_bound(&pairs);
    assert!((bound - 8.0).abs() < 1e-12, "bound {bound}");
    assert!(
        workspace.scale_bound(&[]).is_infinite(),
        "no pairs, no bound"
    );

    let (k, _) = workspace.solve().expect("feasible");
    assert!(k <= bound * (1.0 + 1e-9));
    assert!((k - 8.0).abs() < 1e-9, "k = {k}");

    // Nothing to fit: no rows, no bound.
    assert!(!workspace.prepare(&identity, &region, &[]));
    assert!(workspace.scale_bound(&pairs).is_infinite());
}

#[test]
fn pruning_leaves_the_search_result_bit_identical() {
    // The same search with and without the pairs (no pairs: no bound, every LP is solved)
    // must hold the same orientations, bit for bit; with pairs fewer LPs are solved.
    let fixtures = [
        (
            make_box_hull(1, 1.0, 1.0, 12.0),
            block_halfspaces(10.0, 10.0, 10.0),
        ),
        (
            make_box_hull(2, 1.0, 1.5, 2.0),
            block_halfspaces(12.0, 15.0, 8.0),
        ),
        (
            make_box_hull(3, 1.0, 1.0, 1.0),
            block_halfspaces(1.0, 1.0, 1.0),
        ),
    ];
    let orients = generate_orientations(EXACT_LEVEL, EXACT_SPINS);

    for (index, (hull, region)) in fixtures.iter().enumerate() {
        let pairs = opposite_pairs(region);
        assert_eq!(pairs.len(), 3, "fixture {index}");
        let mut workspace = SupportWorkspace::new(region.len());

        let (full, full_solves) = search_orientations(
            hull,
            &orients,
            region,
            &[],
            &mut workspace,
            None,
            &mut || true,
        )
        .expect("not cancelled");
        let (pruned, pruned_solves) = search_orientations(
            hull,
            &orients,
            region,
            &pairs,
            &mut workspace,
            None,
            &mut || true,
        )
        .expect("not cancelled");

        assert_eq!(full_solves, EXACT_ORIENTATIONS, "fixture {index}");
        assert!(!full.is_empty(), "fixture {index} found nothing");
        assert_eq!(
            candidate_bits(&pruned),
            candidate_bits(&full),
            "fixture {index}: pruning changed the result"
        );
        if index == 0 {
            // A 12 unit bar along a coordinate axis (scale bound 10 / 12) cannot beat the
            // four basins on the space diagonals (scale 1.2 or so), and the grid has such
            // orientations late in its order.
            assert!(
                pruned_solves < full_solves,
                "nothing was pruned: {pruned_solves} solves"
            );
        }
    }
}

#[test]
fn the_screening_table_adds_the_corner_diagonals() {
    assert_eq!(direction_count(3), 38);
    assert_eq!(direction_count(4), 66);
    assert_eq!(direction_count(6), 146);
    assert_eq!(direction_count(8), 258);

    // A level-4 lattice point has |i| + |j| + |k| = 4, which no corner diagonal
    // (+-1, +-1, +-1) satisfies; the axes and the edge diagonals are lattice points. So 8
    // directions are added.
    let table = screening_orientations();
    assert_eq!(table.len(), SCREEN_ORIENTATIONS + 8 * SCREEN_SPINS);
    assert_eq!(
        generate_orientations(SCREEN_LEVEL, SCREEN_SPINS).len(),
        SCREEN_ORIENTATIONS
    );

    let diagonal = DVec3::splat(1.0 / 3.0_f64.sqrt());
    let nearest = |orients: &[super::orient::Orientation]| {
        orients
            .iter()
            .map(|orient| (DVec3::from(orient.axes[1]) - diagonal).length())
            .fold(f64::INFINITY, f64::min)
    };
    assert!(
        nearest(&table) < 1e-12,
        "no table direction on the diagonal"
    );
    // The nearest lattice direction is (1, 1, 2) / sqrt 6, 0.34 away.
    let grid = generate_orientations(SCREEN_LEVEL, SCREEN_SPINS);
    assert!(nearest(&grid) > 0.3, "the grid already had the diagonal");

    for (idx, orient) in table.iter().enumerate() {
        assert_rotation(&orient.axes, 1e-12, &format!("screening orientation {idx}"));
    }
}

#[test]
fn the_polish_steps_end_at_a_sixtyfourth_with_diagonal_moves_on_the_last_two() {
    // 0.105 halved while it is at least 0.0008725: 0.105 / 2^n for n = 0..=6 (n = 7 gives
    // 0.00082, below the threshold). Diagonal moves for steps below 4 * 0.0008725 = 0.00349:
    // 0.105 / 32 = 0.00328 and 0.105 / 64 = 0.00164, but not 0.105 / 16 = 0.0066.
    let mut steps = Vec::new();
    let mut step = POLISH_START_STEP;
    loop {
        if step < POLISH_MIN_STEP {
            break;
        }
        steps.push((step, step < POLISH_DIAGONAL_BELOW));
        step *= 0.5;
    }
    let flags: Vec<bool> = steps.iter().map(|&(_, diagonal)| diagonal).collect();
    assert_eq!(flags, [false, false, false, false, false, true, true]);
    let last = steps.last().expect("steps").0;
    assert!((last - 0.105 / 64.0).abs() < 1e-15, "last step {last}");

    assert_eq!(polish_moves(0.01, false).len(), 6);
    assert_eq!(polish_moves(0.01, true).len(), 18);

    // Every diagonal move turns by the same angle as a single-axis move of that step:
    // the quaternion's vector part has length step / 2 in both.
    let axis_move = perturbation(2, 1.0, 0.01).alignment(Quat::IDENTITY);
    for (first, second) in [(0, 1), (0, 2), (1, 2)] {
        for signs in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let diagonal_move = diagonal_perturbation(first, second, signs.0, signs.1, 0.01)
                .alignment(Quat::IDENTITY);
            assert!(
                (diagonal_move - axis_move).abs() < 1e-15,
                "diagonal move ({first}, {second}, {signs:?}) turns differently"
            );
        }
    }
}

#[test]
fn the_polish_ends_within_half_a_step_of_a_ridge() {
    // A unit cube in a unit cube turned by theta about its own (vertical) y axis has
    // k = 1 / (cos theta + sin theta), which falls linearly with |theta| away from the ridge
    // at 0. Turning about x or z raises the y extent at once, and a move about two axes cannot
    // beat the pure y move, so the search walks along y: a step of size h is repeated while
    // it reduces |theta|, i.e. while |theta| > h / 2, and the search stalls at |theta| <= h / 2.
    // From 0.03 rad it ends within half of the last step, 0.0016406 / 2 = 0.00082 rad:
    // k >= 1 / (1 + 0.00082) = 0.99918. (With the old threshold the last step was 0.0033 and
    // the bound 0.9984.)
    let hull = make_box_hull(1, 1.0, 1.0, 1.0);
    let region = block_halfspaces(1.0, 1.0, 1.0);
    let mut workspace = SupportWorkspace::new(region.len());

    let (sin_theta, cos_theta) = 0.03_f64.sin_cos();
    let start = about(1, 0.03_f64.to_degrees());
    let (k_start, t_start) = workspace
        .evaluate(&start.columns(), &region, &hull.vertices)
        .expect("the start is feasible");
    assert!(
        (k_start - 1.0 / (cos_theta + sin_theta)).abs() < 1e-9,
        "k_start = {k_start}"
    );

    let (k_end, _, quat_end) = polish_orientation(
        start,
        k_start,
        t_start,
        &hull.vertices,
        &region,
        &mut workspace,
        None,
    );
    assert!(k_end >= 0.999, "polish stopped at k = {k_end}");
    assert!(k_end <= 1.0 + 1e-9, "k = {k_end} exceeds the ridge");
    // The polished frame is the aligned cube to within cos(0.00041) = 1 - 8.4e-8.
    assert!(quat_end.alignment(Quat::IDENTITY) > 1.0 - 1e-6);
}

/// A sphere-like design of radius `radius`: the 258 lattice directions of level 8 scaled,
/// with the volume of the true ball (an input, not derived from the vertices).
fn round_hull(entry_id: i64, radius: f64) -> super::DesignHull {
    let vertices: Vec<[f64; 3]> = sphere_directions(8)
        .iter()
        .map(|dir| [radius * dir[0], radius * dir[1], radius * dir[2]])
        .collect();
    assert_eq!(vertices.len(), 258);
    super::DesignHull {
        entry_id,
        vertices,
        volume: 4.0 / 3.0 * PI * radius.powi(3),
        width: 2.0 * radius,
    }
}

#[test]
fn a_round_design_is_scored_by_the_stone_that_fits_not_by_its_proxy() {
    // A ball-like stone of radius r = 1.5 in a 10 unit cube. Every vertex lies at radius r,
    // so the support in any direction is at most r and a centred copy fits with
    // k = 5 / r = 3.333: the score is at least (10 / 3)^3 V. Every face of the 258-point
    // polytope is at least 0.97 r from the centre (its faces are equilateral-ish triangles of
    // angular circumradius about 10 degrees, cos 10 degrees = 0.985), so the support is at
    // least 0.97 r and k <= 5 / (0.97 r) = 3.436: the score is at most 40.6 V (41 below).
    let hull = round_hull(1, 1.5);
    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings::default();

    let scores = screen_designs(&region, std::slice::from_ref(&hull), &settings, &mut |_| {
        true
    })
    .expect("screening completes");
    assert_eq!(scores.len(), 1);
    let score = scores[0].1;
    assert!(
        score >= (10.0_f64 / 3.0).powi(3) * hull.volume * (1.0 - 1e-9),
        "score {score} below the centred ball"
    );
    assert!(
        score <= 41.0 * hull.volume,
        "score {score} above the inradius bound"
    );

    // The proxy sits inside the stone, so its best scale is never below the outline's, and
    // the old score (proxy scale cubed times the full volume) is an upper bound.
    let proxy = compute_proxy_vertices(&hull.vertices);
    let mut workspace = SupportWorkspace::new(region.len());
    let proxy_k = screening_orientations()
        .iter()
        .filter_map(|orient| workspace.evaluate(&orient.axes, &region, &proxy))
        .map(|(k, _)| k)
        .fold(0.0_f64, f64::max);
    assert!(score <= proxy_k.powi(3) * hull.volume * (1.0 + 1e-9));
}

#[test]
fn a_long_design_on_a_cube_diagonal_survives_the_shortlist() {
    // 50 designs in a 10 unit cube, so the shortlist (48) drops two.
    //  * 47 cubes of side 1.0 to 1.5: aligned with the cube they fit with k = 10 / side, a
    //    score of 1000 each.
    //  * Design 1, a 0.5 x 12 x 0.5 bar (long axis = the table normal, volume 3). Standing on
    //    the corner diagonal d = (1, 1, 1) / sqrt 3 the extent along a world axis is
    //    12 / sqrt 3 + 0.5 (|a_i| + |b_i|) <= 6.928 + 0.5 * sqrt(4/3) = 7.506 for the two
    //    cross-section axes a, b (a_i^2 + b_i^2 = 1 - 1/3), so k >= 10 / 7.506 = 1.332 and the
    //    score is at least 1.332^3 * 3 = 7.09 -- whatever the spin. That direction is in the
    //    screening table only because the proxy's corner directions were added.
    //  * Designs 2 and 3, bars 0.5 x 17 x 0.5 and 0.5 x 16.9 x 0.5. The longest segment in
    //    the cube is its diagonal, 17.32, so k <= 17.32 / 16.9 = 1.0249 and the score is at
    //    most 1.0249^3 * 4.225 = 4.55.
    // So the bar on the diagonal ranks 48th of 50 and is shortlisted; the other two are cut.
    let mut hulls = vec![
        make_box_hull(1, 0.5, 12.0, 0.5),
        make_box_hull(2, 0.5, 17.0, 0.5),
        make_box_hull(3, 0.5, 16.9, 0.5),
    ];
    for i in 0..47_u32 {
        let side = 1.0 + 0.5 * f64::from(i) / 46.0;
        hulls.push(make_box_hull(10 + i64::from(i), side, side, side));
    }
    assert_eq!(hulls.len(), 50);

    let region = block_halfspaces(10.0, 10.0, 10.0);
    let settings = PlanSettings::default();
    let scores =
        screen_designs(&region, &hulls, &settings, &mut |_| true).expect("screening completes");
    let score_of = |id: i64| scores.iter().find(|s| s.0 == id).expect("scored").1;

    assert!(
        score_of(1) >= 7.0,
        "the diagonal bar scores {}",
        score_of(1)
    );
    assert!(score_of(2) <= 4.6, "bar 2 scores {}", score_of(2));
    assert!(score_of(3) <= 4.6, "bar 3 scores {}", score_of(3));
    for i in 0..47_i64 {
        let score = score_of(10 + i);
        assert!((score - 1000.0).abs() < 1.0, "cube {i} scores {score}");
    }

    let chosen = shortlist(&scores, 1);
    assert_eq!(chosen.len(), 48);
    assert!(chosen.contains(&1), "the diagonal bar was cut");
    assert!(!chosen.contains(&2) && !chosen.contains(&3));
}

#[test]
fn edge_and_corner_cut_planes_have_no_partner() {
    // An edge plane (normal along (0, 1, 1)) and a corner plane (normal along (-1, -1, -1))
    // face no other plane, so only the six box planes pair up.
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: 20.0,
            y_mm: 20.0,
            z_mm: 20.0,
        },
        vec![
            RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [4.0, 4.0],
            },
            RoughCut::Corner {
                faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
                setbacks_mm: [6.0, 6.0, 6.0],
            },
        ],
    );
    let region = model.usable_halfspaces(0.5).expect("usable halfspaces");
    assert_eq!(region.len(), 8);
    assert_eq!(opposite_pairs(&region), [(0, 1), (2, 3), (4, 5)]);
}

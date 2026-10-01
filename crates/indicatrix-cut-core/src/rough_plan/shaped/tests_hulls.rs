//! Plan-level tests with convex outlines of 100 or more vertices: the owner's timing run,
//! a quick smoke run over the four model kinds, and a cancel sweep.

use std::{collections::BTreeSet, num::NonZeroUsize, panic, thread, time::Instant};

use super::tests::assert_carats;
use crate::rough_plan::{
    Axis, BoxFace, CandidateDesign, DesignHull, PlanInput, PlanProgress, PlanSettings, RoughBase,
    RoughCut, RoughLayout, RoughModel, plan,
    tests::{Lcg, random_designs, settings_with},
};

/// Greatest common divisor.
const fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Lattice points `0..=5` per axis on the surface of the box with half extents `half`:
/// `6^3 - 4^3 = 152` points.
fn box_points(half: [f64; 3]) -> Vec<[f64; 3]> {
    const STEPS: i32 = 5;
    let at = |c: i32, h: f64| f64::from(2 * c - STEPS) / f64::from(STEPS) * h;
    let mut points = Vec::new();
    for i in 0..=STEPS {
        for j in 0..=STEPS {
            for k in 0..=STEPS {
                if [i, j, k].iter().any(|&c| c == 0 || c == STEPS) {
                    points.push([at(i, half[0]), at(j, half[1]), at(k, half[2])]);
                }
            }
        }
    }
    points
}

/// Lattice points `(i, j, k)` with `|i| + |j| + |k| = 6` on the octahedron with half
/// extents `half`: `4 * 36 + 2 = 146` points.
fn octahedron_points(half: [f64; 3]) -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for i in -6_i32..=6 {
        for j in -6_i32..=6 {
            for k in -6_i32..=6 {
                if i.abs() + j.abs() + k.abs() == 6 {
                    let at = |c: i32, h: f64| f64::from(c) / 6.0 * h;
                    points.push([at(i, half[0]), at(j, half[1]), at(k, half[2])]);
                }
            }
        }
    }
    points
}

/// The directions `(i, j, k)` of the lattice `-3..=3` with no common factor, pushed onto
/// the ellipsoid with half extents `half`: 290 points in convex position.
fn round_points(half: [f64; 3]) -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for i in -3_i32..=3 {
        for j in -3_i32..=3 {
            for k in -3_i32..=3 {
                let common = gcd(gcd(i.unsigned_abs(), j.unsigned_abs()), k.unsigned_abs());
                if common == 1 {
                    let len = f64::from(i * i + j * j + k * k).sqrt();
                    let at = |c: i32, h: f64| f64::from(c) / len * h;
                    points.push([at(i, half[0]), at(j, half[1]), at(k, half[2])]);
                }
            }
        }
    }
    points
}

/// The outline of `design` as shape `kind % 3` (box, octahedron, round): more than 100
/// points, a volume that is at most the outline's true volume (so a fit never claims
/// more stone than fits) and a width at most its true caliper width.
fn hull_of(design: &CandidateDesign, kind: usize) -> DesignHull {
    let (w, h, l) = (design.width, design.height, design.length);
    let half = [0.5 * w, 0.5 * h, 0.5 * l];
    let smallest = w.min(h).min(l);
    // Box: w h l. Octahedron with half extents a, b, c: 4/3 a b c = w h l / 6. The round
    // outline contains that octahedron, so a b c = w h l / 8 is below its volume.
    let (vertices, volume, width) = match kind % 3 {
        0 => (box_points(half), w * h * l, smallest),
        1 => {
            // The caliper width of the octahedron is 2 / sqrt(1/a^2 + 1/b^2 + 1/c^2) / 2.
            let width = 1.0 / (1.0 / (w * w) + 1.0 / (h * h) + 1.0 / (l * l)).sqrt();
            (octahedron_points(half), w * h * l / 6.0, 0.9 * width)
        }
        _ => (round_points(half), w * h * l / 8.0, 0.8 * smallest),
    };
    DesignHull {
        entry_id: design.entry_id,
        vertices,
        volume,
        width,
    }
}

/// A plain 8-vertex box outline of `design`, cheap to fit.
fn small_hull_of(design: &CandidateDesign) -> DesignHull {
    let half = [0.5 * design.width, 0.5 * design.height, 0.5 * design.length];
    let vertices = (0..8)
        .map(|corner| {
            let sign = |bit: usize| if corner & bit == 0 { -1.0 } else { 1.0 };
            [sign(1) * half[0], sign(2) * half[1], sign(4) * half[2]]
        })
        .collect();
    DesignHull {
        entry_id: design.entry_id,
        vertices,
        volume: design.width * design.height * design.length,
        width: design.width.min(design.height).min(design.length),
    }
}

/// The four model kinds of the timing runs: an uncut block (the plain path), a cut
/// block, a cylinder and a pebble.
fn model_fixtures() -> [(&'static str, RoughModel); 4] {
    [
        (
            "1. Block 12x9x8",
            RoughModel::new(
                RoughBase::Block {
                    x_mm: 12.0,
                    y_mm: 9.0,
                    z_mm: 8.0,
                },
                vec![],
            ),
        ),
        (
            "2. Block 20x14x10 cut",
            RoughModel::new(
                RoughBase::Block {
                    x_mm: 20.0,
                    y_mm: 14.0,
                    z_mm: 10.0,
                },
                vec![
                    RoughCut::Edge {
                        faces: [BoxFace::Top, BoxFace::Front],
                        setbacks_mm: [3.0, 2.0],
                    },
                    RoughCut::Edge {
                        faces: [BoxFace::Bottom, BoxFace::Left],
                        setbacks_mm: [2.0, 2.0],
                    },
                    RoughCut::Corner {
                        faces: [BoxFace::Top, BoxFace::Back, BoxFace::Right],
                        setbacks_mm: [3.0, 3.0, 3.0],
                    },
                ],
            ),
        ),
        (
            "3. Cylinder D10x24 Y",
            RoughModel::new(
                RoughBase::Cylinder {
                    diameter_mm: 10.0,
                    length_mm: 24.0,
                    axis: Axis::Y,
                },
                vec![],
            ),
        ),
        (
            "4. Pebble 18.4x11x9.6",
            RoughModel::new(
                RoughBase::Pebble {
                    x_mm: 18.4,
                    y_mm: 11.0,
                    z_mm: 9.6,
                },
                vec![RoughCut::Face {
                    normal: [0.0, -1.0, 0.0],
                    depth_mm: 1.2,
                }],
            ),
        ),
    ]
}

/// Plans every model of [`model_fixtures`] with `designs` and their outlines, and
/// asserts what must hold of any plan: some layout, at most ten, no layout above the
/// stone cap, none yielding more than the rough and all with consistent carats.
/// Returns the elapsed time and result count per model.
fn plan_every_model(
    designs: &[CandidateDesign],
    hulls: &[DesignHull],
    settings: &PlanSettings,
) -> Vec<(&'static str, std::time::Duration, usize)> {
    let mut timings = Vec::new();
    for (name, model) in &model_fixtures() {
        let input = PlanInput {
            model,
            settings,
            designs,
            hulls,
        };
        let start = Instant::now();
        let results = plan(&input, &mut |_| true).expect("plan");
        let elapsed = start.elapsed();
        assert!(!results.is_empty(), "{name}: no layout");
        assert!(results.len() <= 10, "{name}: {} layouts", results.len());
        for layout in &results {
            assert!(
                (1..=settings.count_usize()).contains(&layout.stone_count()),
                "{name}: {} stones",
                layout.stone_count()
            );
            assert!(
                layout.yield_fraction > 0.0 && layout.yield_fraction <= 1.0 + 1e-9,
                "{name}: yield {}",
                layout.yield_fraction
            );
            assert_carats(layout, settings);
        }
        timings.push((*name, elapsed, results.len()));
    }
    timings
}

#[test]
fn the_outline_shapes_have_more_than_100_points_inside_their_design_box() {
    let designs = random_designs(&mut Lcg(77), 3);
    for (kind, design) in designs.iter().enumerate() {
        let hull = hull_of(design, kind);
        assert!(
            hull.vertices.len() >= 100,
            "shape {kind}: {} points",
            hull.vertices.len()
        );
        let half = [0.5 * design.width, 0.5 * design.height, 0.5 * design.length];
        for vertex in &hull.vertices {
            for (c, h) in vertex.iter().zip(half) {
                assert!(c.abs() <= h + 1e-12, "shape {kind}: {vertex:?} vs {half:?}");
            }
        }
        assert!(hull.volume > 0.0 && hull.width > 0.0);
        assert!(hull.volume <= design.width * design.height * design.length * (1.0 + 1e-12));
    }
}

#[test]
fn a_k3_plan_over_the_four_model_kinds_with_large_outlines_is_sane() {
    let designs = random_designs(&mut Lcg(1234), 4);
    let hulls: Vec<DesignHull> = designs
        .iter()
        .enumerate()
        .map(|(kind, design)| hull_of(design, kind))
        .collect();
    let settings = settings_with(3, 0.3, 0.2, 0.5);
    let timings = plan_every_model(&designs, &hulls, &settings);
    assert_eq!(timings.len(), 4);
}

#[test]
#[ignore = "owner timing run on release build"]
fn timing_fixtures() {
    let designs = random_designs(&mut Lcg(1234), 50);
    let hulls: Vec<DesignHull> = designs
        .iter()
        .enumerate()
        .map(|(kind, design)| hull_of(design, kind))
        .collect();
    let settings = settings_with(12, 0.3, 0.2, 0.5);
    for (name, elapsed, count) in plan_every_model(&designs, &hulls, &settings) {
        println!("{name}: planned in {elapsed:?} with {count} results");
    }
}

/// The number of progress calls of an uncancelled plan.
fn count_calls(input: &PlanInput<'_>) -> usize {
    let mut calls = 0;
    let results = plan(input, &mut |_: PlanProgress| {
        calls += 1;
        true
    });
    assert!(results.is_some_and(|layouts| !layouts.is_empty()));
    calls
}

/// The stop counts a sweep over `calls` progress calls visits: the first dozen, a
/// dozen spread over the run and the last six, so both ends are covered exactly and the
/// middle is sampled.
fn sweep_stops(calls: usize) -> BTreeSet<usize> {
    let mut stops: BTreeSet<usize> = (1..=calls.min(12)).collect();
    stops.extend((1..=12).map(|i| (calls * i / 12).max(1)));
    stops.extend(calls.saturating_sub(5).max(1)..=calls);
    stops
}

#[test]
fn a_cancel_at_any_progress_call_gives_none_for_a_cut_block_and_a_pebble() {
    let designs = random_designs(&mut Lcg(31), 2);
    let hulls: Vec<DesignHull> = designs.iter().map(small_hull_of).collect();
    let settings = settings_with(2, 0.3, 0.2, 0.5);
    let models = [
        RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [2.0, 2.0],
            }],
        ),
        RoughModel::new(
            RoughBase::Pebble {
                x_mm: 9.0,
                y_mm: 7.0,
                z_mm: 6.0,
            },
            vec![],
        ),
    ];
    for model in &models {
        let input = PlanInput {
            model,
            settings: &settings,
            designs: &designs,
            hulls: &hulls,
        };
        let calls = count_calls(&input);
        assert!(calls > 20, "only {calls} progress calls");
        let stops: Vec<usize> = sweep_stops(calls).into_iter().collect();
        assert_every_stop_cancels(&input, calls, &stops);
    }
}

/// Plans `input` once per stop of `stops`, cancelling at that progress call, and panics
/// for any stop that still gave a result. The plans are independent and deterministic,
/// so they run on scoped threads, one per unit of the machine's parallelism, with the
/// stops dealt round-robin so that every thread gets early (cheap) and late (expensive)
/// stops alike; the thread count changes only the wall time.
fn assert_every_stop_cancels(input: &PlanInput<'_>, calls: usize, stops: &[usize]) {
    let threads = thread::available_parallelism()
        .map_or(4, NonZeroUsize::get)
        .clamp(1, stops.len().max(1));
    thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|lane| {
                let mine: Vec<usize> = stops.iter().copied().skip(lane).step_by(threads).collect();
                scope.spawn(move || {
                    for stop in mine {
                        let mut seen = 0;
                        let results: Option<Vec<RoughLayout>> = plan(input, &mut |_| {
                            seen += 1;
                            seen < stop
                        });
                        assert!(
                            results.is_none(),
                            "a cancel at call {stop} of {calls} still gave a result"
                        );
                    }
                })
            })
            .collect();
        for handle in handles {
            if let Err(payload) = handle.join() {
                panic::resume_unwind(payload);
            }
        }
    });
}

//! Tests of the pebble's geodesic directions and of its pinned plane offsets.
//!
//! The offsets are the fixed point of an iteration implemented once here
//! ([`fixed_point_offsets`]); the ignored generator test prints its result and the regular
//! test checks the pinned tables against it.

use std::f64::consts::PI;

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::{
    ShapeError,
    base::{COARSE_PEBBLE_FREQUENCY, FINE_PEBBLE_FREQUENCY, pebble_halfspaces, pebble_vertices},
    pebble_offsets::{PEBBLE_OFFSETS_COARSE, PEBBLE_OFFSETS_FINE},
    sampling::pebble_directions,
};

/// The angle in radians between two unit vectors.
fn angle(p: [f64; 3], q: [f64; 3]) -> f64 {
    p[2].mul_add(q[2], p[1].mul_add(q[1], p[0] * q[0]))
        .clamp(-1.0, 1.0)
        .acos()
}

#[test]
fn geodesic_direction_counts_are_ten_f_squared_plus_two() {
    for (frequency, expected) in [(1, 12), (2, 42), (3, 92), (4, 162)] {
        assert_eq!(
            pebble_directions(frequency).len(),
            expected,
            "frequency {frequency}"
        );
    }
    assert_eq!(COARSE_PEBBLE_FREQUENCY, 2);
    assert_eq!(FINE_PEBBLE_FREQUENCY, 4);
    assert_eq!(PEBBLE_OFFSETS_COARSE.len(), 42);
    assert_eq!(PEBBLE_OFFSETS_FINE.len(), 162);
}

#[test]
#[should_panic(expected = "frequency must be positive")]
fn geodesic_directions_reject_frequency_zero() {
    let _ = pebble_directions(0);
}

#[test]
fn geodesic_directions_are_unit_vectors() {
    for frequency in 1..=4 {
        for d in pebble_directions(frequency) {
            let len = d[2].mul_add(d[2], d[1].mul_add(d[1], d[0] * d[0])).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-12,
                "frequency {frequency}: |{d:?}| = {len}"
            );
        }
    }
}

#[test]
fn geodesic_directions_are_evenly_spaced() {
    // The nearest-neighbour angle of every direction at frequency 4 runs from 14.5 to 17.2
    // degrees (the icosahedron corners have five neighbours, the lattice interior six), a
    // ratio of 0.847; the octahedral lattice this set replaced varies far more. The bound
    // 0.8 keeps that evenness without pinning the exact figure.
    let dirs = pebble_directions(4);
    let nearest: Vec<f64> = dirs
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            dirs.iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, &q)| angle(p, q))
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let smallest = nearest.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = nearest.iter().copied().fold(0.0, f64::max);
    println!(
        "nearest-neighbour angles at frequency 4: {:.3} to {:.3} degrees, ratio {:.4}",
        smallest.to_degrees(),
        largest.to_degrees(),
        smallest / largest
    );
    assert!(
        smallest > 0.8 * largest,
        "spacing {smallest} .. {largest} is uneven"
    );
}

#[test]
fn geodesic_directions_are_deterministic_sorted_and_symmetric() {
    for frequency in [2, 4] {
        let first = pebble_directions(frequency);
        let again = pebble_directions(frequency);
        let bits = |dirs: &[[f64; 3]]| -> Vec<[u64; 3]> {
            dirs.iter().map(|d| d.map(f64::to_bits)).collect()
        };
        assert_eq!(bits(&first), bits(&again), "frequency {frequency}");

        for pair in first.windows(2) {
            let in_order = pair[0][0]
                .total_cmp(&pair[1][0])
                .then(pair[0][1].total_cmp(&pair[1][1]))
                .then(pair[0][2].total_cmp(&pair[1][2]))
                .is_le();
            assert!(in_order, "{:?} before {:?}", pair[0], pair[1]);
        }

        // No duplicate survives, and every direction has its opposite in the set.
        for (i, &p) in first.iter().enumerate() {
            for &q in &first[i + 1..] {
                assert!(angle(p, q) > 0.1, "{p:?} and {q:?} nearly coincide");
            }
            let opposite = [-p[0], -p[1], -p[2]];
            assert!(
                first.iter().any(|&q| angle(opposite, q) < 1e-6),
                "{p:?} has no opposite"
            );
        }
        // The coordinate axes are edge midpoints of the icosahedron, so an even frequency
        // contains all six.
        for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            assert!(
                first.iter().any(|&q| angle(axis, q) < 1e-6),
                "frequency {frequency} lacks the axis {axis:?}"
            );
        }
    }
}

/// The unit polytope `{ u : d_j . u <= h_j }`: its volume and its vertices.
fn unit_polytope(dirs: &[[f64; 3]], offsets: &[f64]) -> (f64, Vec<DVec3>) {
    let planes: Vec<(DVec3, f64)> = dirs
        .iter()
        .zip(offsets)
        .map(|(&[x, y, z], &h)| (DVec3::new(x, y, z), h))
        .collect();
    let (metrics, vertices) = measure_solid_with_vertices(&planes).expect("unit polytope");
    (metrics.volume, vertices)
}

/// For every face `j`, the largest radius among the vertices on its plane
/// (`|d_j . v - h_j| < 1e-9`), or 1 when no vertex lies on it.
fn face_radii(dirs: &[[f64; 3]], offsets: &[f64], vertices: &[DVec3]) -> Vec<f64> {
    dirs.iter()
        .zip(offsets)
        .map(|(&[x, y, z], &h)| {
            let d = DVec3::new(x, y, z);
            vertices
                .iter()
                .filter(|&&v| (d.dot(v) - h).abs() < 1e-9)
                .map(|v| v.length())
                .reduce(f64::max)
                .unwrap_or(1.0)
        })
        .collect()
}

/// The largest vertex radius.
fn max_radius(vertices: &[DVec3]) -> f64 {
    vertices.iter().map(|v| v.length()).fold(0.0, f64::max)
}

/// The fixed point of the per-face offset iteration over the directions of `frequency`,
/// and the number of iterations it took.
///
/// Start with every `h_j = 1`; for up to 200 rounds measure the polytope, take each face's
/// largest vertex radius `r_j`, stop when every `r_j` is within `1e-12` of 1 and otherwise
/// replace `h_j` by `h_j / r_j`. Finally divide all offsets by the largest vertex radius so
/// every vertex lies inside the unit ball.
fn fixed_point_offsets(frequency: usize) -> (Vec<f64>, usize) {
    let dirs = pebble_directions(frequency);
    let mut offsets = vec![1.0; dirs.len()];
    let mut rounds = 0;
    while rounds < 200 {
        let (_, vertices) = unit_polytope(&dirs, &offsets);
        let radii = face_radii(&dirs, &offsets, &vertices);
        if radii.iter().all(|r| (r - 1.0).abs() < 1e-12) {
            break;
        }
        for (h, r) in offsets.iter_mut().zip(&radii) {
            *h /= r;
        }
        rounds += 1;
    }
    let (_, vertices) = unit_polytope(&dirs, &offsets);
    let shrink = max_radius(&vertices);
    for h in &mut offsets {
        *h /= shrink;
    }
    (offsets, rounds)
}

/// Prints the two pinned offset tables as Rust array literals.
///
/// Run `cargo test --release -p indicatrix-cut-core -- regenerate_pebble_offsets --ignored
/// --nocapture` and paste the output over the constants in `pebble_offsets.rs`.
/// `value` as a Rust literal with the fractional digits in groups of three (`0.984_820_7`),
/// the form the pinned tables use.
fn grouped_literal(value: f64) -> String {
    let text = format!("{value:?}");
    let Some((whole, fraction)) = text.split_once('.') else {
        return text;
    };
    let digits: Vec<char> = fraction.chars().collect();
    let groups: Vec<String> = digits
        .chunks(3)
        .map(|chunk| chunk.iter().collect())
        .collect();
    format!("{whole}.{}", groups.join("_"))
}

#[test]
#[ignore = "prints the pinned offset tables; run it with --ignored --nocapture"]
fn regenerate_pebble_offsets() {
    for (name, frequency) in [
        ("PEBBLE_OFFSETS_FINE", FINE_PEBBLE_FREQUENCY),
        ("PEBBLE_OFFSETS_COARSE", COARSE_PEBBLE_FREQUENCY),
    ] {
        let (offsets, rounds) = fixed_point_offsets(frequency);
        assert!(rounds < 200, "frequency {frequency} did not converge");
        let dirs = pebble_directions(frequency);
        let (volume, _) = unit_polytope(&dirs, &offsets);
        println!(
            "// frequency {frequency}: {rounds} iterations, volume fraction {:.6}",
            volume / (4.0 / 3.0 * PI)
        );
        println!("pub(super) const {name}: [f64; {}] = [", offsets.len());
        for h in &offsets {
            println!("    {},", grouped_literal(*h));
        }
        println!("];");
    }
}

#[test]
fn pinned_pebble_offsets_are_the_fixed_point() {
    let ball = 4.0 / 3.0 * PI;
    // The fine table holds at least 0.97 of the ball (measured 0.973); the coarse one is
    // a 42-plane polytope and holds less.
    for (frequency, table, min_fraction) in [
        (FINE_PEBBLE_FREQUENCY, &PEBBLE_OFFSETS_FINE[..], 0.97),
        (
            COARSE_PEBBLE_FREQUENCY,
            &PEBBLE_OFFSETS_COARSE[..],
            COARSE_MIN_FRACTION,
        ),
    ] {
        let dirs = pebble_directions(frequency);
        assert_eq!(dirs.len(), table.len(), "frequency {frequency}");
        let (volume, vertices) = unit_polytope(&dirs, table);
        let fraction = volume / ball;
        println!(
            "frequency {frequency}: {} planes, {} vertices, volume fraction {fraction:.6}",
            dirs.len(),
            vertices.len()
        );

        // Every vertex is inside the ball.
        let rmax = max_radius(&vertices);
        assert!(
            rmax <= 1.0 + 1e-9,
            "frequency {frequency}: vertex radius {rmax}"
        );

        // Every face's largest vertex radius reaches the sphere.
        let radii = face_radii(&dirs, table, &vertices);
        for (j, r) in radii.iter().enumerate() {
            assert!(
                *r >= 1.0 - 1e-6,
                "frequency {frequency}: face {j} reaches {r}"
            );
        }

        // One more iteration step moves no offset: the table is the generator's output.
        for (j, (&h, r)) in table.iter().zip(&radii).enumerate() {
            let stepped = h / r;
            assert!(
                (stepped - h).abs() <= 1e-9,
                "frequency {frequency}: offset {j} would move from {h} to {stepped}"
            );
        }

        assert!(
            fraction >= min_fraction && fraction < 1.0,
            "frequency {frequency}: volume fraction {fraction} below {min_fraction}"
        );
    }
}

/// The lower bound on the coarse (42-plane) unit polytope's volume fraction of the ball,
/// just under what the pinned table measures.
const COARSE_MIN_FRACTION: f64 = 0.89;

#[test]
fn frequencies_without_a_pinned_table_have_no_pebble() {
    for frequency in [1, 3, 6] {
        assert_eq!(
            pebble_halfspaces(10.0, 10.0, 10.0, frequency),
            Err(ShapeError::NothingLeft)
        );
        assert_eq!(
            pebble_vertices(10.0, 10.0, 10.0, frequency),
            Err(ShapeError::NothingLeft)
        );
    }
}

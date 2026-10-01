//! Deterministic circle and sphere direction sampling without trigonometry.
//!
//! Provides deterministic sampling of points on the 2D unit circle and the 3D unit sphere
//! without relying on transcendental functions from `libm`. Only IEEE-754 arithmetic,
//! square root, and angle-sum formulas are used.

/// Returns `cos(pi / n)` computed via repeated half-angle formula starting from `cos(pi/2) = 0`.
///
/// `n` must be a power of two greater than or equal to 2.
///
/// # Panics
///
/// Panics if `n < 2` or `n` is not a power of two.
#[must_use]
pub fn half_step_cos(n: usize) -> f64 {
    assert!(
        n >= 2 && n.is_power_of_two(),
        "n must be a power of two >= 2"
    );
    let mut cos_val: f64 = 0.0;
    let mut current_n = 2;
    while current_n < n {
        cos_val = f64::midpoint(1.0_f64, cos_val).sqrt();
        current_n *= 2;
    }
    cos_val
}

/// Generates `n` evenly spaced points on the unit circle starting at `[1.0, 0.0]` and
/// proceeding counter-clockwise.
///
/// `n` must be a power of two in `4..=1024`.
/// The points are generated using half-angle formulas and angle sums, re-deriving
/// every eighth point from the half-angle table to eliminate floating-point drift.
///
/// # Panics
///
/// Panics if `n` is not a power of two in `4..=1024`.
#[must_use]
pub fn unit_circle(n: usize) -> Vec<[f64; 2]> {
    assert!(
        (4..=1024).contains(&n) && n.is_power_of_two(),
        "n must be a power of two in 4..=1024"
    );

    // Number of half-angle steps from pi/2 down to delta = 2*pi/n.
    // For n=4, delta = pi/2 (0 steps). For n=64, delta = pi/32 (4 steps).
    let steps = n.trailing_zeros() as usize - 2;
    let mut table: Vec<(f64, f64)> = Vec::with_capacity(steps + 1);
    table.resize(steps + 1, (0.0_f64, 0.0_f64));

    // table[steps] corresponds to angle pi/2
    table[steps] = (0.0_f64, 1.0_f64);
    for k in (0..steps).rev() {
        let (c_prev, s_prev) = table[k + 1];
        let c_half = f64::midpoint(1.0_f64, c_prev).sqrt();
        let s_half = s_prev / (2.0_f64 * c_half);
        table[k] = (c_half, s_half);
    }

    let (c_base, s_base) = table[0];
    let quarter = n / 4;
    let half = n / 2;
    let three_quarters = 3 * quarter;

    let mut points = Vec::with_capacity(n);
    let mut curr_cos = 1.0;
    let mut curr_sin = 0.0;

    for j in 0..n {
        if j == 0 {
            curr_cos = 1.0;
            curr_sin = 0.0;
        } else if j == quarter {
            curr_cos = 0.0;
            curr_sin = 1.0;
        } else if j == half {
            curr_cos = -1.0;
            curr_sin = 0.0;
        } else if j == three_quarters {
            curr_cos = 0.0;
            curr_sin = -1.0;
        } else if j % 8 == 0 {
            (curr_cos, curr_sin) = rederive_point(j, &table, steps);
        } else {
            // Advance by one base step using angle-sum formula
            let next_c = curr_sin.mul_add(-s_base, curr_cos * c_base);
            let next_s = curr_cos.mul_add(s_base, curr_sin * c_base);
            curr_cos = next_c;
            curr_sin = next_s;
        }

        points.push([curr_cos, curr_sin]);
    }

    points
}

/// Re-derives point `j` of the circle from the half-angle `table` by binary decomposition.
///
/// Bit `k <= steps` of `j` is a rotation by `table[k]` (bit `steps` is the exact quarter
/// turn); bit `steps + 1` is the exact half turn, applied last as a negation of both
/// components. `j` must be below `4 << steps`.
fn rederive_point(j: usize, table: &[(f64, f64)], steps: usize) -> (f64, f64) {
    let (mut acc_c, mut acc_s) = (1.0_f64, 0.0_f64);
    for (k, &(table_c, table_s)) in table.iter().enumerate().take(steps + 1) {
        if ((j >> k) & 1) == 1 {
            if k == steps {
                // Rotation by pi/2: (acc_c, acc_s) -> (-acc_s, acc_c)
                (acc_c, acc_s) = (-acc_s, acc_c);
            } else {
                let next_c = acc_s.mul_add(-table_s, acc_c * table_c);
                let next_s = acc_c.mul_add(table_s, acc_s * table_c);
                (acc_c, acc_s) = (next_c, next_s);
            }
        }
    }
    if ((j >> (steps + 1)) & 1) == 1 {
        (acc_c, acc_s) = (-acc_c, -acc_s);
    }
    (acc_c, acc_s)
}

/// Generates unit direction vectors on the sphere via regular octahedron subdivision.
///
/// Directions are normalized integer triples `(i, j, k)` satisfying `|i| + |j| + |k| = level`,
/// returned in ascending lexicographical order (`i`, then `j`, then `k`).
///
/// Level 3 produces 38 directions; level 4 produces 66; level 6 produces 146; level 8 produces 258.
/// The octahedral lattice is far denser near the octahedron's vertices than near its face
/// centres; [`pebble_directions`] gives the more even set a polytope approximating a ball
/// wants.
///
/// # Panics
///
/// Panics if `level == 0`.
#[must_use]
pub fn sphere_directions(level: usize) -> Vec<[f64; 3]> {
    assert!(level > 0, "level must be positive");
    let mut dirs = Vec::with_capacity(4 * level * level + 2);
    let lvl = level as i64;

    for i in -lvl..=lvl {
        let rem_j = lvl - i.abs();
        for j in -rem_j..=rem_j {
            let rem_k = rem_j - j.abs();
            if rem_k == 0 {
                let len = ((i * i + j * j) as f64).sqrt();
                dirs.push([i as f64 / len, j as f64 / len, 0.0]);
            } else {
                let len = ((i * i + j * j + rem_k * rem_k) as f64).sqrt();
                dirs.push([i as f64 / len, j as f64 / len, -rem_k as f64 / len]);
                dirs.push([i as f64 / len, j as f64 / len, rem_k as f64 / len]);
            }
        }
    }

    dirs
}

/// The 12 unit vertices of a regular icosahedron: the cyclic permutations of
/// `(0, +-1, +-phi)` with `phi = (1 + sqrt 5) / 2`, each normalised, in a fixed order.
fn icosahedron_vertices() -> Vec<[f64; 3]> {
    let phi = f64::midpoint(1.0, 5.0_f64.sqrt());
    let len = phi.mul_add(phi, 1.0).sqrt();
    let (a, b) = (1.0 / len, phi / len);
    let mut vertices = Vec::with_capacity(12);
    for sa in [-a, a] {
        for sb in [-b, b] {
            vertices.push([0.0, sa, sb]);
            vertices.push([sa, sb, 0.0]);
            vertices.push([sb, 0.0, sa]);
        }
    }
    vertices
}

/// Dot product of two 3-vectors.
fn dot3(p: [f64; 3], q: [f64; 3]) -> f64 {
    p[0].mul_add(q[0], p[1].mul_add(q[1], p[2] * q[2]))
}

/// The 20 faces of the icosahedron with the given `vertices`, as index triples `i < j < k`
/// in index order: the triples whose three pairwise dot products all exceed `0.4`
/// (adjacent vertices have the dot product `1 / sqrt 5 = 0.447`, all others at most
/// `-1 / sqrt 5`).
fn icosahedron_faces(vertices: &[[f64; 3]]) -> Vec<[usize; 3]> {
    let adjacent = |i: usize, j: usize| dot3(vertices[i], vertices[j]) > 0.4;
    let mut faces = Vec::with_capacity(20);
    for i in 0..vertices.len() {
        for j in (i + 1)..vertices.len() {
            for k in (j + 1)..vertices.len() {
                if adjacent(i, j) && adjacent(j, k) && adjacent(i, k) {
                    faces.push([i, j, k]);
                }
            }
        }
    }
    faces
}

/// Generates the unit direction vectors of a class-I geodesic subdivision of the icosahedron
/// at `frequency` `f`: the sphere points that are most evenly spread for a given count.
///
/// Every icosahedron face `(A, B, C)` contributes the lattice points
/// `(i A + j B + k C) / f` for `i + j + k = f`, each normalised onto the sphere. Points on
/// shared edges and corners come up more than once; a point is kept only if no earlier
/// kept point lies within `1e-9` of it. The result is sorted ascending by `(x, y, z)`, so
/// the order is canonical, and has `10 f^2 + 2` directions (12 at `f = 1`, 42 at `f = 2`,
/// 162 at `f = 4`). An even `f` includes the six coordinate-axis directions (the edge
/// midpoints of the icosahedron lie on the axes), so the set is symmetric under `d -> -d`.
///
/// # Panics
///
/// Panics if `frequency == 0`.
#[must_use]
pub fn pebble_directions(frequency: usize) -> Vec<[f64; 3]> {
    assert!(frequency > 0, "frequency must be positive");
    let vertices = icosahedron_vertices();
    let scale = frequency as f64;
    let mut kept: Vec<[f64; 3]> = Vec::with_capacity(10 * frequency * frequency + 2);

    for [ia, ib, ic] in icosahedron_faces(&vertices) {
        let (va, vb, vc) = (vertices[ia], vertices[ib], vertices[ic]);
        for i in 0..=frequency {
            for j in 0..=(frequency - i) {
                let (wa, wb, wc) = (
                    i as f64 / scale,
                    j as f64 / scale,
                    (frequency - i - j) as f64 / scale,
                );
                let mut point = [0.0; 3];
                for (axis, slot) in point.iter_mut().enumerate() {
                    *slot = wa.mul_add(va[axis], wb.mul_add(vb[axis], wc * vc[axis]));
                }
                let len = dot3(point, point).sqrt();
                let point = point.map(|component| component / len);
                let seen = kept.iter().any(|&earlier| {
                    let gap = [
                        point[0] - earlier[0],
                        point[1] - earlier[1],
                        point[2] - earlier[2],
                    ];
                    dot3(gap, gap) < 1e-18
                });
                if !seen {
                    kept.push(point);
                }
            }
        }
    }

    kept.sort_by(|p, q| {
        p[0].total_cmp(&q[0])
            .then(p[1].total_cmp(&q[1]))
            .then(p[2].total_cmp(&q[2]))
    });
    kept
}

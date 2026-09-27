//! Numeric helpers for `analyze_one`: total arrangement degeneracy at a given mast
//! vector, Gaussian-elimination rank of a 4-column row set, and least squares via
//! regularized normal equations.

use glam::DVec3;

use crate::{
    candidates::enumerate_candidates,
    types::{BLANK, EPS_INCIDENT, MAX_PLANES, Plane},
};

/// Total degeneracy of the arrangement at the given masts: enumerate feasible
/// vertices (violated-set empty), dedup by position, and sum `incident - 3` over
/// vertices, where `incident` counts planes within EPS_INCIDENT.
pub fn total_degeneracy(normals: &[Vec<DVec3>], masts: &[f64]) -> f64 {
    let mut planes: Vec<Plane> = [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .map(|n| Plane {
        n,
        m: BLANK,
        owner: usize::MAX,
    })
    .collect();
    for (i, ns) in normals.iter().enumerate() {
        for &n in ns {
            planes.push(Plane {
                n,
                m: masts[i],
                owner: i,
            });
        }
    }
    if planes.len() > MAX_PLANES {
        return f64::NAN;
    }
    let cands = enumerate_candidates(&planes, 6);
    let mut verts: Vec<DVec3> = Vec::new();
    for c in &cands {
        if c.violated.is_some() {
            continue;
        }
        if !verts.iter().any(|v| (*v - c.v).abs().max_element() < 1e-6) {
            verts.push(c.v);
        }
    }
    let mut d = 0.0;
    for v in &verts {
        let mut inc = 0usize;
        for (t, ns) in normals.iter().enumerate() {
            for &n in ns {
                if (n.dot(*v) - masts[t]).abs() < EPS_INCIDENT {
                    inc += 1;
                }
            }
            let _ = t;
        }
        if inc > 3 {
            d += (inc - 3) as f64;
        }
    }
    d
}

/// Numeric rank (pivot threshold 1e-9) of a set of 4-column rows, via Gaussian
/// elimination with partial pivoting. Mutates `rows` in place.
pub fn rank4(rows: &mut [[f64; 4]]) -> usize {
    let mut rank = 0usize;
    for col in 0..4 {
        let Some(pivot) = (rank..rows.len())
            .max_by(|&a, &b| rows[a][col].abs().partial_cmp(&rows[b][col].abs()).unwrap())
        else {
            break;
        };
        if rows[pivot][col].abs() < 1e-9 {
            continue;
        }
        rows.swap(rank, pivot);
        let head = rows[rank];
        for r in rows.iter_mut().skip(rank + 1) {
            let f = r[col] / head[col];
            for c in col..4 {
                r[c] -= f * head[c];
            }
        }
        rank += 1;
        if rank == rows.len() {
            break;
        }
    }
    rank
}

/// Least squares via regularized normal equations; returns `None` on an empty or
/// singular system.
pub fn solve_normal_equations(rows: &[Vec<f64>], rhs: &[f64], k: usize) -> Option<Vec<f64>> {
    if rows.is_empty() || k == 0 {
        return None;
    }
    let mut ata = vec![vec![0.0f64; k]; k];
    let mut atb = vec![0.0f64; k];
    for (row, &b) in rows.iter().zip(rhs) {
        for i in 0..k {
            if row[i] == 0.0 {
                continue;
            }
            atb[i] += row[i] * b;
            for j in 0..k {
                ata[i][j] += row[i] * row[j];
            }
        }
    }
    let scale = (0..k).map(|i| ata[i][i]).fold(1e-12, f64::max);
    let reg = 1e-10 * scale;
    for (i, row) in ata.iter_mut().enumerate() {
        row[i] += reg;
    }
    // Gaussian elimination with partial pivoting.
    let mut a = ata;
    let mut b = atb;
    for col in 0..k {
        let pivot =
            (col..k).max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())?;
        if a[pivot][col].abs() < 1e-14 * scale.max(1.0) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for r in (col + 1)..k {
            let f = a[r][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..k {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0f64; k];
    for i in (0..k).rev() {
        let mut s = b[i];
        for j in (i + 1)..k {
            s -= a[i][j] * x[j];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

//! Orientation generation, quaternion representation, and angular perturbation.
//!
//! Provides deterministic orientation grids on the unit sphere combined with spins
//! around the table normal, quaternion math without trigonometry, and local rotational
//! perturbations for pattern search polishing.

use std::f64::consts::FRAC_1_SQRT_2;

use super::support::proxy_unit_directions;
use crate::rough_plan::shape::sampling::{sphere_directions, unit_circle};

/// The number of directions [`sphere_directions`] returns at `level`: the lattice points
/// with `|i| + |j| + |k| = level` (`4 level^2 + 2`; 66 at level 4, 258 at level 8).
#[must_use]
pub const fn direction_count(level: usize) -> usize {
    4 * level * level + 2
}

/// Octahedron subdivision level for coarse screening directions (66 directions).
pub const SCREEN_LEVEL: usize = 4;
/// Number of spins around the table normal for coarse screening.
pub const SCREEN_SPINS: usize = 16;
/// Orientations of the octahedral screening grid (66 * 16 = 1,056). The screening table
/// ([`screening_orientations`]) adds the proxy's own directions on top of these.
pub const SCREEN_ORIENTATIONS: usize = direction_count(SCREEN_LEVEL) * SCREEN_SPINS;

/// Octahedron subdivision level for exact directions (258 directions).
pub const EXACT_LEVEL: usize = 8;
/// Number of spins around the table normal for exact search.
pub const EXACT_SPINS: usize = 32;
/// Total number of orientations in the exact set (258 * 32 = 8,256).
pub const EXACT_ORIENTATIONS: usize = direction_count(EXACT_LEVEL) * EXACT_SPINS;

/// Private quaternion for exact-fit 3D orientations.
///
/// Uses only IEEE floating-point arithmetic and `sqrt` (no transcendental libm trigonometry).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub(crate) w: f64,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) z: f64,
}

impl Quat {
    /// The identity quaternion representing zero rotation.
    pub(crate) const IDENTITY: Self = Self {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Multiplies two quaternions: `self * rhs`.
    #[must_use]
    pub(crate) fn mul(self, rhs: Self) -> Self {
        let (qw1, qx1, qy1, qz1) = (self.w, self.x, self.y, self.z);
        let (qw2, qx2, qy2, qz2) = (rhs.w, rhs.x, rhs.y, rhs.z);
        Self {
            w: f64::mul_add(
                qz1,
                -qz2,
                f64::mul_add(qy1, -qy2, f64::mul_add(qx1, -qx2, qw1 * qw2)),
            ),
            x: f64::mul_add(
                qz1,
                -qy2,
                f64::mul_add(qy1, qz2, f64::mul_add(qx1, qw2, qw1 * qx2)),
            ),
            y: f64::mul_add(
                qz1,
                qx2,
                f64::mul_add(qy1, qw2, f64::mul_add(qx1, -qz2, qw1 * qy2)),
            ),
            z: f64::mul_add(
                qz1,
                qw2,
                f64::mul_add(qy1, -qx2, f64::mul_add(qx1, qy2, qw1 * qz2)),
            ),
        }
    }

    /// Normalizes the quaternion to unit length using only `sqrt`.
    #[must_use]
    pub(crate) fn normalize(self) -> Self {
        let norm_sq = self.z.mul_add(
            self.z,
            self.y
                .mul_add(self.y, self.x.mul_add(self.x, self.w * self.w)),
        );
        if norm_sq > 1e-30 {
            let inv_len = 1.0 / norm_sq.sqrt();
            Self {
                w: self.w * inv_len,
                x: self.x * inv_len,
                y: self.y * inv_len,
                z: self.z * inv_len,
            }
        } else {
            Self::IDENTITY
        }
    }

    /// `|self . other|` for unit quaternions: `cos(theta / 2)` for the rotation angle
    /// `theta` between the two orientations (`1` for equal or sign-flipped quaternions,
    /// which are the same rotation).
    #[must_use]
    pub(crate) fn alignment(self, other: Self) -> f64 {
        self.w
            .mul_add(
                other.w,
                self.x
                    .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z)),
            )
            .abs()
    }

    /// Rotates a 3D vector by this unit quaternion.
    #[must_use]
    pub(crate) fn rotate_vec(self, vec: [f64; 3]) -> [f64; 3] {
        let qv = [self.x, self.y, self.z];
        let cx = 2.0 * qv[2].mul_add(-vec[1], qv[1] * vec[2]);
        let cy = 2.0 * qv[0].mul_add(-vec[2], qv[2] * vec[0]);
        let cz = 2.0 * qv[1].mul_add(-vec[0], qv[0] * vec[1]);
        [
            self.w.mul_add(cx, vec[0]) + qv[2].mul_add(-cy, qv[1] * cz),
            self.w.mul_add(cy, vec[1]) + qv[0].mul_add(-cz, qv[2] * cx),
            self.w.mul_add(cz, vec[2]) + qv[1].mul_add(-cx, qv[0] * cy),
        ]
    }

    /// Converts this unit quaternion to a 3x3 row-major rotation matrix `[[f64; 3]; 3]`.
    #[must_use]
    pub(crate) fn to_matrix(self) -> [[f64; 3]; 3] {
        let col0 = self.rotate_vec([1.0, 0.0, 0.0]);
        let col1 = self.rotate_vec([0.0, 1.0, 0.0]);
        let col2 = self.rotate_vec([0.0, 0.0, 1.0]);
        [
            [col0[0], col1[0], col2[0]],
            [col0[1], col1[1], col2[1]],
            [col0[2], col1[2], col2[2]],
        ]
    }

    /// Returns the columns of the 3x3 rotation matrix represented by this quaternion.
    ///
    /// Column 0 is the stone's x axis, column 1 is the stone's y axis (table normal),
    /// and column 2 is the stone's z axis.
    #[must_use]
    pub(crate) fn columns(&self) -> [[f64; 3]; 3] {
        let mat = self.to_matrix();
        [
            [mat[0][0], mat[1][0], mat[2][0]],
            [mat[0][1], mat[1][1], mat[2][1]],
            [mat[0][2], mat[1][2], mat[2][2]],
        ]
    }

    /// Constructs a unit quaternion from orthonormal column vectors `[col0, col1, col2]`.
    #[must_use]
    pub(crate) fn from_axes(axes: &[[f64; 3]; 3]) -> Self {
        let r00 = axes[0][0];
        let r10 = axes[0][1];
        let r20 = axes[0][2];
        let r01 = axes[1][0];
        let r11 = axes[1][1];
        let r21 = axes[1][2];
        let r02 = axes[2][0];
        let r12 = axes[2][1];
        let r22 = axes[2][2];

        let trace = r00 + r11 + r22;
        let quat = if trace > 0.0 {
            let scale = (trace + 1.0).sqrt() * 2.0;
            Self {
                w: 0.25 * scale,
                x: (r21 - r12) / scale,
                y: (r02 - r20) / scale,
                z: (r10 - r01) / scale,
            }
        } else if r00 > r11 && r00 > r22 {
            let scale = (1.0 + r00 - r11 - r22).max(0.0).sqrt() * 2.0;
            Self {
                w: (r21 - r12) / scale,
                x: 0.25 * scale,
                y: (r01 + r10) / scale,
                z: (r02 + r20) / scale,
            }
        } else if r11 > r22 {
            let scale = (1.0 + r11 - r00 - r22).max(0.0).sqrt() * 2.0;
            Self {
                w: (r02 - r20) / scale,
                x: (r01 + r10) / scale,
                y: 0.25 * scale,
                z: (r12 + r21) / scale,
            }
        } else {
            let scale = (1.0 + r22 - r00 - r11).max(0.0).sqrt() * 2.0;
            Self {
                w: (r10 - r01) / scale,
                x: (r02 + r20) / scale,
                y: (r12 + r21) / scale,
                z: 0.25 * scale,
            }
        };
        quat.normalize()
    }
}

/// A precomputed discrete orientation consisting of a quaternion and its frame axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orientation {
    pub(crate) quat: Quat,
    pub(crate) axes: [[f64; 3]; 3],
}

/// Generates discrete orientations from table directions on the sphere and spins.
///
/// For each table direction `d`:
/// - Reference rough axis `r` is the axis with the smallest `|d · r|`, ties going to the lower index.
/// - `e1 = normalise(r - (r · d) d)`.
/// - `e2 = d x e1`.
/// - For each spin angle `phi`: stone x is `cos(phi) e1 + sin(phi) e2`, y is `d`, and z is `x x y`.
#[must_use]
pub fn generate_orientations(level: usize, spins: usize) -> Vec<Orientation> {
    orientations_for(&sphere_directions(level), spins)
}

/// The orientations of the screening stage: the octahedral grid of [`SCREEN_LEVEL`] plus
/// every direction of the 26-point screening proxy that the grid does not already contain,
/// each with [`SCREEN_SPINS`] spins.
///
/// The proxy picks its vertices along 6 axes, 12 edge diagonals and 8 corner diagonals. A
/// level-4 lattice point has `|i| + |j| + |k| = 4`, which holds for the axes and the edge
/// diagonals (for example `(2, 2, 0)`) but for no corner diagonal `(+-1, +-1, +-1)` (that
/// would need a sum of 3), so exactly the 8 corner diagonals are appended after the grid
/// directions. A design that is long along a cube diagonal is then screened with its long
/// axis on that diagonal.
#[must_use]
pub fn screening_orientations() -> Vec<Orientation> {
    let mut dirs = sphere_directions(SCREEN_LEVEL);
    for extra in proxy_unit_directions() {
        let present = dirs.iter().any(|dir| {
            dir[2].mul_add(extra[2], dir[1].mul_add(extra[1], dir[0] * extra[0])) > 1.0 - 1e-12
        });
        if !present {
            dirs.push(extra);
        }
    }
    orientations_for(&dirs, SCREEN_SPINS)
}

/// One orientation per `(table direction, spin)` pair, directions in the order given;
/// see [`generate_orientations`] for the frame construction.
#[must_use]
pub fn orientations_for(dirs: &[[f64; 3]], spins: usize) -> Vec<Orientation> {
    let circle = unit_circle(spins);
    let mut out = Vec::with_capacity(dirs.len() * circle.len());

    for &dir in dirs {
        let abs_x = dir[0].abs();
        let abs_y = dir[1].abs();
        let abs_z = dir[2].abs();
        let ref_axis = if abs_x <= abs_y && abs_x <= abs_z {
            [1.0, 0.0, 0.0]
        } else if abs_y <= abs_z {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };

        let r_dot_d = f64::mul_add(
            ref_axis[2],
            dir[2],
            f64::mul_add(ref_axis[1], dir[1], ref_axis[0] * dir[0]),
        );
        let v_proj = [
            f64::mul_add(r_dot_d, -dir[0], ref_axis[0]),
            f64::mul_add(r_dot_d, -dir[1], ref_axis[1]),
            f64::mul_add(r_dot_d, -dir[2], ref_axis[2]),
        ];
        let len_proj = f64::mul_add(
            v_proj[2],
            v_proj[2],
            f64::mul_add(v_proj[1], v_proj[1], v_proj[0] * v_proj[0]),
        )
        .sqrt();
        let basis_e1 = [
            v_proj[0] / len_proj,
            v_proj[1] / len_proj,
            v_proj[2] / len_proj,
        ];

        let basis_e2 = [
            f64::mul_add(dir[2], -basis_e1[1], dir[1] * basis_e1[2]),
            f64::mul_add(dir[0], -basis_e1[2], dir[2] * basis_e1[0]),
            f64::mul_add(dir[1], -basis_e1[0], dir[0] * basis_e1[1]),
        ];

        for spin in &circle {
            let cos_phi = spin[0];
            let sin_phi = spin[1];
            let st_x = [
                f64::mul_add(sin_phi, basis_e2[0], cos_phi * basis_e1[0]),
                f64::mul_add(sin_phi, basis_e2[1], cos_phi * basis_e1[1]),
                f64::mul_add(sin_phi, basis_e2[2], cos_phi * basis_e1[2]),
            ];
            let st_y = dir;
            let st_z = [
                f64::mul_add(st_x[2], -st_y[1], st_x[1] * st_y[2]),
                f64::mul_add(st_x[0], -st_y[2], st_x[2] * st_y[0]),
                f64::mul_add(st_x[1], -st_y[0], st_x[0] * st_y[1]),
            ];

            let axes = [st_x, st_y, st_z];
            let quat = Quat::from_axes(&axes);
            out.push(Orientation { quat, axes });
        }
    }

    out
}

/// Computes an infinitesimal rotational perturbation quaternion for pattern search polishing.
///
/// Returns `normalise(Quat { w: 1, x: dx/2, y: dy/2, z: dz/2 })` about stone axis 0, 1, or 2.
#[must_use]
pub fn perturbation(axis: usize, sign: f64, step: f64) -> Quat {
    let half_step = sign * step * 0.5;
    let (offset_x, offset_y, offset_z) = match axis {
        0 => (half_step, 0.0, 0.0),
        1 => (0.0, half_step, 0.0),
        _ => (0.0, 0.0, half_step),
    };
    Quat {
        w: 1.0,
        x: offset_x,
        y: offset_y,
        z: offset_z,
    }
    .normalize()
}

/// A perturbation about two stone axes at once (`first` and `second`, each 0, 1 or 2 and
/// different), with the signs given.
///
/// Each component is scaled by `1 / sqrt(2)`, so the rotation angle is the same `step`
/// (to first order) as that of a single-axis [`perturbation`].
#[must_use]
pub fn diagonal_perturbation(
    first: usize,
    second: usize,
    sign_first: f64,
    sign_second: f64,
    step: f64,
) -> Quat {
    let half_step = step * 0.5 * FRAC_1_SQRT_2;
    let mut offset = [0.0_f64; 3];
    offset[first.min(2)] = sign_first * half_step;
    offset[second.min(2)] = sign_second * half_step;
    Quat {
        w: 1.0,
        x: offset[0],
        y: offset[1],
        z: offset[2],
    }
    .normalize()
}

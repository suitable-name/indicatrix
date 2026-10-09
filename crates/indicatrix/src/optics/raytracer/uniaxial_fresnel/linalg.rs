//! Boundary-matching linear algebra.
//!
//! Gauss-Jordan elimination for the 4x4 complex systems every entry/internal solve
//! reduces to, and the tangential-field-continuity projection that builds each
//! system's rows.

use super::{complex::Cplx, frame::ModeFields};
use glam::Vec3;

/// Solves a 4x4 complex linear system `A x = b` via Gauss-Jordan elimination with
/// partial pivoting. Every boundary-value solve in this module reduces to exactly this
/// shape (4 tangential-field-continuity equations, 4 unknown amplitudes) -- this is the
/// literal closed-form Lekner solution (his `r_ss` etc. ARE the ratios of 4x4
/// determinants this produces), not a general/iterative numerical eigensolve.
pub(super) fn solve4(mut a: [[Cplx; 4]; 4], mut b: [Cplx; 4]) -> [Cplx; 4] {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b[col] = b[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            #[allow(
                clippy::needless_range_loop,
                reason = "row `row` is updated from row `col` of the same matrix; an iterator would need a split borrow"
            )]
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b[row] = b[row].sub(factor.mul(b[col]));
        }
    }
    b
}

/// [`solve4`] for TWO right-hand sides against the SAME matrix `a` -- one Gaussian
/// elimination instead of two (requirement 7: `entry_solve_pair`'s own performance
/// note). Bit-identical to calling `solve4(a, b1)` and `solve4(a, b2)` separately: the
/// row-reduction operations performed on `a` do not depend on either right-hand side,
/// so applying each pivot/elimination step to both `b` vectors in lockstep is the same
/// arithmetic in the same order, just walked once instead of twice.
pub(super) fn solve4_two_rhs(
    mut a: [[Cplx; 4]; 4],
    mut b1: [Cplx; 4],
    mut b2: [Cplx; 4],
) -> ([Cplx; 4], [Cplx; 4]) {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b1.swap(col, piv);
        b2.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b1[col] = b1[col].div(pivot);
        b2[col] = b2[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            #[allow(
                clippy::needless_range_loop,
                reason = "row `row` is updated from row `col` of the same matrix; an iterator would need a split borrow"
            )]
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b1[row] = b1[row].sub(factor.mul(b1[col]));
            b2[row] = b2[row].sub(factor.mul(b2[col]));
        }
    }
    (b1, b2)
}

#[inline]
pub(super) fn tangential_components(fields: &ModeFields, that: Vec3, s_axis: Vec3) -> [Cplx; 4] {
    [
        fields.e.dot_real(that),
        fields.e.dot_real(s_axis),
        fields.h.dot_real(that),
        fields.h.dot_real(s_axis),
    ]
}

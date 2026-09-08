//! Shared ULP-distance helper for the GPU self-tests.
//!
//! ULP (not a relative epsilon) is the right unit here: "how many representable `f32`
//! values apart are these two results" stays meaningful near zero and near large
//! magnitudes alike, unlike a relative tolerance. Unlike `rng_check`'s private
//! `ulp_distance`, this version handles sign/negative-zero correctly across the zero
//! boundary -- see `to_ordered`'s doc comment.

/// Distance, in ULP (representable `f32` steps), between two floats of any sign,
/// including a pair that straddles zero. Several Phase-1 quantities this is used on
/// (ray directions and normal components in `[-1, 1]`) legitimately cross zero, so
/// sign and negative-zero are handled explicitly via [`to_ordered`] rather than assumed
/// away.
#[must_use]
pub fn ulp_distance(a: f32, b: f32) -> u32 {
    // Fast path: bit-pattern equality (not `==`, which disagrees with `to_bits()` on
    // NaN and +0.0/-0.0). Sound because `to_ordered` below independently maps both
    // +0.0/-0.0 to the same ordered value, and identical-bits NaNs already fall out
    // to 0 ULP via `to_ordered` too -- this is just a shortcut, not a behavior change.
    if a.to_bits() == b.to_bits() {
        return 0;
    }
    let ai = to_ordered(a);
    let bi = to_ordered(b);
    ai.abs_diff(bi) as u32
}

/// Whether `cpu`/`gpu` agree closely enough to not be a bug, under a hybrid rule: EITHER
/// their ULP distance is within `budget`, OR their absolute difference is under
/// `abs_floor`.
///
/// The second clause exists because ULP is a poor metric where a value legitimately
/// crosses (or nearly crosses) zero, or is too small to be physically meaningful.
/// Examples: `sin(x)` near a multiple of π, where CPU/GPU trig can round to opposite
/// sides of `0.0` (an absolute difference of ~1e-7 registering as billions of ULP);
/// and `cie_1931_cmf` deep in a Gaussian tail (~1e-24), where relative precision is
/// inherently poor but the value is far below anything visibly distinguishable.
/// `abs_floor` must sit well below the magnitude a real algebra bug would produce (a
/// deliberately wrong formula measured at 8,552,444 ULP) -- each call site documents
/// its own choice.
#[must_use]
pub fn within_tolerance(cpu: f32, gpu: f32, budget: u32, abs_floor: f32) -> bool {
    ulp_distance(cpu, gpu) <= budget || (cpu - gpu).abs() < abs_floor
}

/// Maps an `f32`'s bit pattern to a monotonically-ordered `i64` (standard trick: the raw
/// bit pattern reinterpreted as signed is already monotonic for non-negative floats;
/// reflecting through `i32::MIN` restores monotonicity for negative floats). Handles
/// negative operands and negative zero correctly, unlike a bare `to_bits()` difference.
///
/// `+0.0` and `-0.0` map to the *same* ordered value (0 ULP apart, not 1), matching
/// `a == b`'s treatment of the pair as plain floats.
///
/// Bug once here: `i64::from(x.to_bits())` zero-extends a `u32`, so the result is
/// always `>= 0` as an `i64` regardless of sign -- the intended negative-branch flip
/// was unreachable dead code, silently degenerating to a raw bit-pattern diff that's
/// only monotonic within same-sign ranges and breaks across the sign boundary (a
/// ~2^31 jump from `0x7FFF_FFFF` to `0x8000_0000` unrelated to actual step count).
fn to_ordered(x: f32) -> i64 {
    let bits = x.to_bits() as i32;
    let ordered = if bits < 0 {
        i32::MIN.wrapping_sub(bits)
    } else {
        bits
    };
    i64::from(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_values_are_zero_ulp_apart() {
        assert_eq!(ulp_distance(1.0, 1.0), 0);
        assert_eq!(ulp_distance(0.0, 0.0), 0);
        assert_eq!(ulp_distance(0.0, -0.0), 0);
    }

    #[test]
    fn adjacent_representable_floats_are_one_ulp_apart() {
        let a = 1.0f32;
        let b = f32::from_bits(a.to_bits() + 1);
        assert_eq!(ulp_distance(a, b), 1);
        assert_eq!(ulp_distance(b, a), 1);
    }

    #[test]
    fn crossing_zero_counts_the_full_step_distance() {
        let a = f32::from_bits(1); // smallest positive subnormal
        let b = -a;
        // One step to +0.0 in each direction -> 2 total.
        assert_eq!(ulp_distance(a, b), 2);
    }

    #[test]
    fn positive_zero_and_negative_zero_are_zero_ulp_apart() {
        // +0.0 and -0.0 are the same point (0 ULP), matching IEEE-754 `+0.0 == -0.0`.
        assert_eq!(ulp_distance(0.0f32, -0.0f32), 0);
        assert_eq!(ulp_distance(-0.0f32, 0.0f32), 0);
    }

    #[test]
    fn same_sign_positive_pairs_are_unaffected_by_the_fix() {
        let a = 100.0f32;
        let b = f32::from_bits(a.to_bits() + 5);
        assert_eq!(ulp_distance(a, b), 5);
        assert_eq!(ulp_distance(b, a), 5);
    }

    #[test]
    fn same_sign_negative_pairs_are_unaffected_by_the_fix() {
        let a = -100.0f32;
        let b = f32::from_bits(a.to_bits() + 5); // more negative than `a` by 5 steps
        assert_eq!(ulp_distance(a, b), 5);
        assert_eq!(ulp_distance(b, a), 5);
    }

    #[test]
    fn opposite_sign_pairs_away_from_zero_count_the_full_path_through_zero() {
        // -1.0 to 1.0: sum of the two contiguous ranges [0, 1.0] and [-1.0, -0.0].
        let steps_zero_to_one = ulp_distance(0.0f32, 1.0f32);
        let steps_neg_one_to_zero = ulp_distance(-1.0f32, -0.0f32);
        let combined = steps_zero_to_one + steps_neg_one_to_zero;
        assert_eq!(ulp_distance(-1.0f32, 1.0f32), combined);
        assert_eq!(ulp_distance(1.0f32, -1.0f32), combined);
        // 1.0 and -1.0 share the same magnitude bit pattern, so this is 2x that value.
        assert_eq!(combined, 2 * 1.0f32.to_bits());
    }

    #[test]
    fn denormals_straddling_zero_step_correctly() {
        let a = f32::from_bits(3); // 3rd smallest positive subnormal
        let b = f32::from_bits(0x8000_0002); // 2nd smallest negative subnormal
        // 3 steps from a to +0.0, 2 steps from -0.0 to b -> 5 total.
        assert_eq!(ulp_distance(a, b), 5);
        assert_eq!(ulp_distance(b, a), 5);
    }

    #[test]
    fn denormals_same_sign_step_correctly() {
        let a = f32::from_bits(2);
        let b = f32::from_bits(9);
        assert_eq!(ulp_distance(a, b), 7);
        let na = f32::from_bits(0x8000_0002);
        let nb = f32::from_bits(0x8000_0009);
        assert_eq!(ulp_distance(na, nb), 7);
    }

    #[test]
    fn values_near_f32_max_step_correctly_same_sign() {
        let max = f32::MAX;
        let one_below = f32::from_bits(max.to_bits() - 1);
        assert_eq!(ulp_distance(max, one_below), 1);

        let neg_max = -f32::MAX;
        let one_above = f32::from_bits(neg_max.to_bits() - 1); // one step toward zero
        assert_eq!(ulp_distance(neg_max, one_above), 1);
    }

    #[test]
    fn values_near_f32_max_step_correctly_opposite_sign() {
        // Full span straddling zero: twice f32::MAX's bit pattern, comfortably under u32::MAX.
        let d = ulp_distance(f32::MAX, -f32::MAX);
        let expected = 2u32 * f32::MAX.to_bits();
        assert_eq!(d, expected);
        assert_eq!(ulp_distance(-f32::MAX, f32::MAX), d);
    }
}

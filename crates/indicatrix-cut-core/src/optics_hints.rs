//! Pure critical-angle math shared by the material retarget proposal and the
//! solid preview's risk overlay.
//!
//! Both need the same "how close is this facet to windowing" question answered from a
//! tier's angle and the design's refractive index, so it lives here once rather than
//! in either caller.
//!
//! # Angle convention
//!
//! Every angle here is a [`crate::design::ConstraintTier::angle_deg`] value: signed
//! degrees from the girdle plane, negative for a pavilion facet, non-negative for
//! crown (an unsigned `0.0` inherits the previous tier's side -- see that field's
//! own doc comment, which mirrors `indicatrix_formats::asc::AscTier::angle_deg` exactly).
//! [`tier_margin_deg`], [`windowing_risk`] and [`retarget_angle_deg`] all read the
//! MAGNITUDE of that angle and preserve the input's own sign on any angle they
//! return.
//!
//! Windowing is a pavilion phenomenon: light entering through the crown that hits
//! a pavilion facet below the critical angle refracts straight out instead of
//! reflecting back to the eye. A caller is expected to apply these functions only
//! to pavilion tiers (`angle_deg < 0.0`); nothing here enforces it, since a crown
//! tier's own risk is a distinct, unmodeled concern left to crown-specific UI policy.
//!
//! # Domain
//!
//! `n` (a refractive index) is assumed `> 1.0` -- true for every real gem material
//! this crate can produce. [`critical_angle_deg`] is not meaningful outside that
//! domain (`asin` of an argument `>= 1.0` saturates to `NaN`/`90.0`); this module
//! does not guard against `n <= 1.0` since every real caller's `n` traces back to a
//! resolved material, never a raw, unvalidated user-typed number.

/// The critical angle for total internal reflection at refractive index `n`.
///
/// In degrees: `asin(1/n)`. Below this angle (measured from the facet's own
/// normal), light hitting a facet from inside the stone refracts out instead of
/// reflecting -- see the module doc comment's "Domain" section for this
/// function's `n > 1.0` assumption.
#[must_use]
pub fn critical_angle_deg(n: f64) -> f64 {
    (1.0 / n).asin().to_degrees()
}

/// How many degrees `tier_angle_deg` sits above index `n`'s own
/// [`critical_angle_deg`].
///
/// `tier_angle_deg` is a tier's own authored angle from the girdle plane -- see
/// the module doc comment's "Angle convention" section. Positive means the facet
/// sits past the critical angle with margin to spare; negative means it has
/// already crossed below it (light windows through).
#[must_use]
pub fn tier_margin_deg(tier_angle_deg: f64, n: f64) -> f64 {
    tier_angle_deg.abs() - critical_angle_deg(n)
}

/// A tier's windowing risk at refractive index `n`, from its margin over the
/// critical angle (see [`tier_margin_deg`]) -- meaningful for a pavilion tier's
/// angle specifically (see the module doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Margin at least 2 degrees -- comfortably past the critical angle.
    Safe,
    /// Margin in `[0, 2)` degrees -- past the critical angle, but thin enough
    /// that a small edit, a manufacturing tolerance, or a different material
    /// could tip it into windowing.
    Marginal,
    /// Margin below zero -- already below the critical angle for `n`: light
    /// leaks straight through this facet instead of reflecting.
    Windows,
}

/// Classifies a tier's windowing risk at index `n` -- see [`Risk`]'s own doc
/// comment for the three bands and their boundaries (`< 0.0` is
/// [`Risk::Windows`], `[0.0, 2.0)` is [`Risk::Marginal`], `>= 2.0` is
/// [`Risk::Safe`]).
#[must_use]
pub fn windowing_risk(tier_angle_deg: f64, n: f64) -> Risk {
    let margin = tier_margin_deg(tier_angle_deg, n);
    if margin < 0.0 {
        Risk::Windows
    } else if margin < 2.0 {
        Risk::Marginal
    } else {
        Risk::Safe
    }
}

/// The critical-angle-shift retarget proposal for one tier moving from
/// refractive index `n_from` to `n_to`.
///
/// The "critical-angle shift (deterministic, default)" algorithm:
/// `theta' = critical_angle_deg(n_to) + (theta - critical_angle_deg(n_from))`,
/// which preserves the tier's own margin over the critical angle exactly (see
/// this module's own tests). Reads and returns a signed `angle_deg` (see the
/// module doc comment's "Angle convention" section): the input's own sign is
/// preserved on the output, so this is safe to call with a pavilion tier's
/// (negative) `angle_deg` directly.
///
/// This is the pure math half only. Clamping to the optimizer's safety bound
/// (89.5 degrees), flagging a tier that would fall below the new critical angle,
/// and the crown "scale by ratio" alternative are all dialog-level policy
/// (`apps/indicatrix-cut`'s own retarget proposal UI) layered on top of this, not
/// modeled here.
#[must_use]
pub fn retarget_angle_deg(tier_angle_deg: f64, n_from: f64, n_to: f64) -> f64 {
    // `is_sign_negative`, not `< 0.0`: this crate's own templates author a
    // pavilion culet at exactly `-0.0` to record its side (see
    // `ConstraintTier::standard_round_brilliant`). Since `-0.0 < 0.0` is `false`,
    // using `<` would drop that marker and retarget it to `+new_theta` (a crown
    // angle) instead of preserving the pavilion side.
    let sign = if tier_angle_deg.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    let theta = tier_angle_deg.abs();
    let new_theta = critical_angle_deg(n_to) + (theta - critical_angle_deg(n_from));
    sign * new_theta
}

/// The crown's own windowing estimate.
///
/// How many degrees of margin remain at a PAVILION facet
/// (angle `pavilion_angle_deg`, from the girdle) for light that entered
/// through a CROWN facet (angle `crown_angle_deg`, from the girdle) rather
/// than straight down through the table, at refractive index `n`.
///
/// # Derivation (Snell's law)
///
/// Both angles are read as magnitudes (see the module doc comment's "Angle
/// convention"): a facet tilted `theta` degrees from the (horizontal) girdle
/// plane has its own normal tilted `theta` degrees from the vertical viewing
/// axis -- the same reading [`tier_margin_deg`] already gives the plain
/// (table-only) pavilion model.
///
/// Follow one ray, in the 2-D radial cross-section through one crown facet and
/// the pavilion facet on the same azimuthal index: it starts vertical (straight
/// down, in air), hits the crown facet at incidence angle `c = crown_angle_deg`
/// from the crown facet's own normal (since that normal sits `c` degrees off
/// vertical), and refracts per Snell's law,
/// `sin(c) = n * sin(r)`, so `r = asin(sin(c) / n)`.
///
/// Because refraction bends the ray TOWARD the normal (entering the denser
/// stone), the ray's new direction is tilted `(c - r)` degrees from vertical
/// (less than the crown facet's own `c`, never zero) -- a vertical ray only ever
/// picks up tilt by refracting through a facet that is not flat, and `r < c` for
/// any `n > 1`.
///
/// That ray then reaches the pavilion facet, whose own normal sits
/// `p = pavilion_angle_deg` degrees off vertical, tilted toward the SAME side
/// the crown bent the ray toward (the two facets face each other across one
/// radial cross-section). Resolving both directions as unit vectors in that
/// plane and taking their dot product gives the angle of incidence at the
/// pavilion directly: `theta_pavilion = p + (c - r)` (see this module's own
/// tests for the vector algebra spelled out numerically). Compare that against
/// [`critical_angle_deg`] exactly as [`tier_margin_deg`] compares the plain
/// table-only incidence angle `p` -- entering through a tilted crown facet
/// only ever ADDS incidence angle over the straight-through-the-table case
/// (`c - r >= 0`), so this margin is never smaller than the table-only
/// pavilion margin at the same `p`.
///
/// # This is an estimate, not a full ray trace
///
/// This follows exactly one ray (vertical incidence on the crown, in the
/// crown/pavilion facets' own shared meridian plane) and ignores every other
/// path light actually takes through a real stone -- internal reflections off
/// OTHER facets, the crown facet's own azimuthal curvature away from that one
/// meridian, dispersion (every wavelength refracts by a slightly different
/// `r`), and any facet that is not radially opposite the one being typed. A
/// caller-facing label should say "estimate", never "risk" alone -- see the
/// module doc comment's own crown caveat, which this function narrows but does
/// not remove.
#[must_use]
pub fn crown_window_margin_deg(pavilion_angle_deg: f64, crown_angle_deg: f64, n: f64) -> f64 {
    let c = crown_angle_deg.abs();
    let p = pavilion_angle_deg.abs();
    let r = (c.to_radians().sin() / n).asin().to_degrees();
    (p + (c - r)) - critical_angle_deg(n)
}

/// [`crown_window_margin_deg`] classified into the same three [`Risk`] bands as
/// [`windowing_risk`] -- an ESTIMATE (see that function's own doc comment), not
/// a full ray trace.
#[must_use]
pub fn crown_windowing_risk(pavilion_angle_deg: f64, crown_angle_deg: f64, n: f64) -> Risk {
    let margin = crown_window_margin_deg(pavilion_angle_deg, crown_angle_deg, n);
    if margin < 0.0 {
        Risk::Windows
    } else if margin < 2.0 {
        Risk::Marginal
    } else {
        Risk::Safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diamond's own well-known critical angle (~24.4 degrees) is a real sanity
    /// check on the formula itself, not just a boundary case.
    #[test]
    fn critical_angle_matches_the_well_known_diamond_figure() {
        let angle = critical_angle_deg(2.417);
        assert!((angle - 24.4).abs() < 0.05, "{angle}");
    }

    /// A lower-index material has a WIDER critical angle: quartz (~1.544) must
    /// report a larger critical angle than diamond (~2.417).
    #[test]
    fn critical_angle_increases_as_refractive_index_decreases() {
        assert!(critical_angle_deg(1.544) > critical_angle_deg(2.417));
    }

    /// Only the magnitude of the angle matters -- a crown tier at +30 and a
    /// pavilion tier at -30 must report the identical margin at the same n.
    #[test]
    fn tier_margin_is_symmetric_for_crown_and_pavilion_signs() {
        let n = 1.62;
        assert_eq!(tier_margin_deg(30.0, n), tier_margin_deg(-30.0, n));
    }

    // --- windowing_risk boundaries ---

    /// Exercises both boundaries (`Windows`/`Marginal` at margin 0, `Marginal`/`Safe`
    /// at margin 2) as half-open on the low side, using an `n` chosen so
    /// `critical_angle_deg(n)` is a clean 20 degrees.
    #[test]
    fn windowing_risk_boundaries() {
        let n = 1.0 / 20.0f64.to_radians().sin();
        assert!((critical_angle_deg(n) - 20.0).abs() < 1e-9);

        assert_eq!(windowing_risk(-19.0, n), Risk::Windows);
        assert_eq!(windowing_risk(-(20.0 - 1e-9), n), Risk::Windows);
        assert_eq!(windowing_risk(-20.0, n), Risk::Marginal);
        assert_eq!(windowing_risk(-21.999, n), Risk::Marginal);
        assert_eq!(windowing_risk(-22.0, n), Risk::Safe);
        assert_eq!(windowing_risk(-22.001, n), Risk::Safe);
    }

    /// Sign-agnostic (see the module doc comment) -- a caller decides whether to
    /// apply it to a crown tier at all.
    #[test]
    fn windowing_risk_reads_crown_tiers_the_same_way_as_pavilion() {
        let n = 1.0 / 20.0f64.to_radians().sin();
        assert_eq!(windowing_risk(20.0, n), windowing_risk(-20.0, n));
    }

    // --- retarget_angle_deg ---

    /// Retargeting a synthetic pavilion tier from one index to another must
    /// preserve its margin over the critical angle to `1e-9`.
    #[test]
    fn retarget_preserves_margin_on_a_synthetic_pavilion_tier() {
        let n_from = 2.417; // diamond
        let n_to = 1.544; // quartz
        let original_angle = -45.0; // a synthetic pavilion tier
        let margin_before = tier_margin_deg(original_angle, n_from);

        let new_angle = retarget_angle_deg(original_angle, n_from, n_to);
        assert!(
            new_angle < 0.0,
            "pavilion sign must be preserved: {new_angle}"
        );
        let margin_after = tier_margin_deg(new_angle, n_to);

        assert!(
            (margin_after - margin_before).abs() < 1e-9,
            "margin drifted: before={margin_before} after={margin_after}"
        );
    }

    /// Same property, but for a crown-signed (non-negative) angle -- the sign
    /// handling must be symmetric, not pavilion-only.
    #[test]
    fn retarget_preserves_margin_and_sign_on_a_crown_tier_too() {
        let n_from = 1.7;
        let n_to = 2.0;
        let original_angle = 35.0;
        let margin_before = tier_margin_deg(original_angle, n_from);

        let new_angle = retarget_angle_deg(original_angle, n_from, n_to);
        assert!(
            new_angle >= 0.0,
            "crown sign must be preserved: {new_angle}"
        );
        let margin_after = tier_margin_deg(new_angle, n_to);

        assert!((margin_after - margin_before).abs() < 1e-9);
    }

    /// Retargeting to the SAME index must be a no-op on the angle itself.
    #[test]
    fn retarget_to_the_same_index_is_a_no_op() {
        let n = 1.62;
        let angle = -38.5;
        assert!((retarget_angle_deg(angle, n, n) - angle).abs() < 1e-9);
    }

    /// A culet authored at exactly `-0.0` (this crate's own
    /// pavilion-side marker -- see `ConstraintTier::standard_round_brilliant`)
    /// must retarget to a NEGATIVE (pavilion) angle, not `+new_theta`: a plain
    /// `tier_angle_deg < 0.0` test is `false` for `-0.0`, which would silently drop
    /// the marker.
    #[test]
    fn retarget_preserves_the_negative_zero_pavilion_marker() {
        let n_from = 2.417;
        let n_to = 1.544;
        let new_angle = retarget_angle_deg(-0.0, n_from, n_to);
        assert!(
            new_angle.is_sign_negative(),
            "a -0.0 (pavilion) origin must retarget to a negative angle: {new_angle}"
        );
    }

    // --- crown_window_margin_deg / crown_windowing_risk ---

    /// A classic 41-degree pavilion / 34-degree crown round brilliant is a real,
    /// working (non-windowing) cut in both quartz and sapphire -- the "known-good
    /// case" this function must not falsely flag, in EITHER material.
    #[test]
    fn crown_window_reports_safe_for_a_standard_41_34_round_brilliant() {
        for n in [1.544_f64, 1.76_f64] {
            let risk = crown_windowing_risk(41.0, 34.0, n);
            assert_eq!(
                risk,
                Risk::Safe,
                "n={n}: margin={}",
                crown_window_margin_deg(41.0, 34.0, n)
            );
        }
    }

    /// A flat (0-degree) crown -- i.e. a table-only path -- must reduce exactly
    /// to the plain table-only pavilion margin: `r = asin(sin(0)/n) = 0`, so
    /// `theta_pavilion = p + (0 - 0) = p`, identical to [`tier_margin_deg`].
    #[test]
    fn crown_window_margin_reduces_to_the_table_only_margin_at_zero_crown_angle() {
        let n = 1.62;
        let p = 40.75;
        let table_only = tier_margin_deg(-p, n);
        let via_crown = crown_window_margin_deg(p, 0.0, n);
        assert!(
            (table_only - via_crown).abs() < 1e-9,
            "table_only={table_only} via_crown={via_crown}"
        );
    }

    /// Entering through ANY tilted crown facet must never report a SMALLER
    /// margin than the table-only path at the same pavilion angle -- the
    /// derivation's own `c - r >= 0` property (refraction only ever adds
    /// incidence angle here, never removes it).
    #[test]
    fn crown_window_margin_is_never_below_the_table_only_margin() {
        let n = 1.76;
        let p = 38.0;
        let table_only = tier_margin_deg(-p, n);
        for c in [5.0, 15.0, 25.0, 34.0, 40.0] {
            let via_crown = crown_window_margin_deg(p, c, n);
            assert!(
                via_crown >= table_only - 1e-9,
                "c={c}: via_crown={via_crown} < table_only={table_only}"
            );
        }
    }

    /// Sign-agnostic on the pavilion angle, matching [`tier_margin_deg`]'s own
    /// convention -- a pavilion tier's authored `angle_deg` is negative.
    #[test]
    fn crown_window_margin_is_symmetric_in_the_pavilion_angles_sign() {
        let n = 1.7;
        assert_eq!(
            crown_window_margin_deg(41.0, 34.0, n),
            crown_window_margin_deg(-41.0, 34.0, n)
        );
    }
}

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
    let sign = if tier_angle_deg < 0.0 { -1.0 } else { 1.0 };
    let theta = tier_angle_deg.abs();
    let new_theta = critical_angle_deg(n_to) + (theta - critical_angle_deg(n_from));
    sign * new_theta
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
}

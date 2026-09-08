//! Per-block scale-reference anchoring from printed proportions.
//!
//! Meet-vertex incidence alone cannot recover a design's per-block translation
//! (see the module docs on anchors); [`apply_ratio_anchors`] fills in whatever a
//! schedule left unanchored from its printed `C/W`/`P/W` ratios, using real
//! per-design plane geometry ([`geometric_ratio_anchor`],
//! [`girdle_radius_for_anchor`]) rather than a corpus-fitted constant.

use super::{
    MeetConstraint, MeetTierInput,
    blocks::{Block, classify_blocks},
};

/// Reference mast the girdle block's anchor is pegged to by
/// [`apply_ratio_anchors`]. A design has exactly one true scale degree of freedom
/// (angles fix shape up to uniform scale), so this value is arbitrary -- what
/// matters is the crown/pavilion anchors being expressed *relative* to it via
/// [`geometric_ratio_anchor`].
const GIRDLE_REFERENCE_MAST: f64 = 1.0;

/// Girdle radius a crown/pavilion ratio anchor is measured against: the mast of
/// this design's own already-anchored girdle tier. When more than one girdle
/// tier already carries a known scale reference (e.g. distinct width/length
/// girdle rows on a step cut), the smallest is used -- a printed `C/W`/`P/W`
/// ratio's `W` denominator is the stone's *width*, the narrower outline
/// dimension. Falls back to [`GIRDLE_REFERENCE_MAST`] if the design has no
/// girdle tier at all.
///
/// Treats the one known radius as representative of the whole girdle (an
/// approximation; other girdle facets are still unresolved at this point)
/// rather than attempting azimuth-matched per-facet radii -- still a real
/// per-design geometric quantity, unlike a corpus-fitted constant.
fn girdle_radius_for_anchor(tiers: &[MeetTierInput], blocks: &[Block]) -> f64 {
    let mut radii: Vec<f64> = (0..tiers.len())
        .filter(|&i| blocks[i] == Block::Girdle)
        .filter_map(|i| match tiers[i].constraint {
            MeetConstraint::ScaleReference(v) => Some(v.abs()),
            _ => None,
        })
        .collect();
    radii.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    radii.first().copied().unwrap_or(GIRDLE_REFERENCE_MAST)
}

/// Converts a printed proportion ratio (`C/W` or `P/W`) into an anchor mast for
/// one tier by direct plane geometry -- the tier's own angle and the design's
/// girdle radius -- rather than a corpus-fitted scalar. Which relation applies
/// depends on `is_flat`, i.e. *which* tier [`apply_ratio_anchors`] is anchoring.
///
/// **`is_flat = true`** (an explicit flat table/culet tier, always `theta = 0`):
/// its mast literally *is* the block's height/depth above/below the girdle
/// plane, since a horizontal plane's mast is its height by definition. With the
/// girdle anchored at `girdle_r` (see [`girdle_radius_for_anchor`]),
/// `W = 2 * girdle_r`, so `mast = ratio * 2.0 * girdle_r` directly.
///
/// **`is_flat = false`** (the block's first tier, when it has no flat
/// table/culet): rejected summing a girdle-rim term and a height term
/// (`sin(theta) * girdle_r + cos(theta) * (ratio * 2.0 * girdle_r)`) -- a
/// design's first-listed tier is almost always girdle-adjacent and reaches
/// nowhere near the block's full height/depth, so the height term overshoots
/// badly (43% crown / 107% pavilion median relative error). The pure
/// girdle-rim relation `mast = sin(theta) * girdle_r` (no ratio) measures
/// 10-14%, beating the old fitted constants' 18.2% crown / 10.2% pavilion.
fn geometric_ratio_anchor(theta_deg: f64, ratio: f64, girdle_r: f64, is_flat: bool) -> f64 {
    let theta = theta_deg.abs().to_radians();
    if is_flat {
        ratio * 2.0 * girdle_r
    } else {
        theta.sin() * girdle_r
    }
}

/// Finds a block's explicit flat table/culet tier -- angle exactly `0.0` (signed
/// per [`tier_sides`](super::blocks::tier_sides)'s convention) with at most one
/// index instance, i.e. a single azimuth-independent facet, which is what a
/// printed `C/W`/`P/W` ratio actually measures. `None` when the block has no
/// such tier (a pointed table/culet, common on the pavilion side).
pub(super) fn explicit_table_or_culet(
    tiers: &[MeetTierInput],
    blocks: &[Block],
    want: Block,
) -> Option<usize> {
    let want_negative_zero = want == Block::Pavilion;
    (0..tiers.len()).find(|&i| {
        blocks[i] == want
            && tiers[i].angle_deg == 0.0
            && tiers[i].angle_deg.is_sign_negative() == want_negative_zero
            && tiers[i].indices.len() <= 1
    })
}

/// Fills in per-block [`MeetConstraint::ScaleReference`] anchors from a design's
/// printed proportions rather than from a real recorded mast.
///
/// `C/W`/`P/W` (crown height and pavilion depth, each over girdle width) are what
/// let [`solve_meet_points`](super::solve_meet_points) run on a design that has
/// no `.asc` file at all, and are exactly the free per-block dimensions
/// meet-vertex incidence alone cannot recover (see the module docs).
///
/// Only touches a block that doesn't already carry an explicit
/// [`MeetConstraint::ScaleReference`] (a schedule's own stated anchor always
/// wins over an estimate) and that has at least one tier:
///
/// - **Girdle**: anchors the block's first tier (file order) at
///   [`GIRDLE_REFERENCE_MAST`], unconditionally -- a design has only one real
///   scale degree of freedom, so this is just the reference unit the crown and
///   pavilion anchors below are expressed against.
/// - **Crown**: if `cw_ratio` is `Some`, anchors an explicit flat table tier
///   ([`explicit_table_or_culet`]) when present, else the block's first tier --
///   via [`geometric_ratio_anchor`] (that tier's own angle, `cw_ratio`, and
///   [`girdle_radius_for_anchor`]), not a fitted constant.
/// - **Pavilion**: the same, via `pw_ratio` and an explicit flat culet tier when
///   present (a minority of designs have one; most pavilions are pointed).
///
/// Returns `(crown_anchored, pavilion_anchored)`: whether each was actually
/// able to anchor (`false` when the ratio was `None` or the block has no tiers).
///
/// # Why per-design geometry instead of a fitted constant
///
/// Rejected: four corpus-fitted constants (median `real_mast / ratio`), one per
/// anchor-tier case -- implicitly assumes every design's anchor tier sits at the
/// same angle, which is false in general; measured worse end-to-end than the
/// real-mast baseline (corpus median relative error 0.2125 -> 0.3446).
///
/// [`geometric_ratio_anchor`] uses the anchor tier's own angle and this design's
/// own girdle radius instead (corpus median relative error 0.2866, better than
/// the fitted constants though short of the 0.2125 real-mast baseline, since a
/// printed ratio is a noisier signal). The residual is a known, accepted limit:
/// printed `C/W` is measured from the girdle band's top edge, not the
/// mast-origin plane a flat table's mast is measured from, so
/// `ratio * 2 * girdle_r` is exact for the wrong reference plane, and `C/W` is
/// invariant under coherent block translation so it cannot pin the crown
/// anchor absolutely (see
/// [`solve_meet_points_verified`](super::solve_meet_points_verified)).
///
/// Still the only anchor source available for designs with no `.asc` file.
pub fn apply_ratio_anchors(
    tiers: &mut [MeetTierInput],
    cw_ratio: Option<f64>,
    pw_ratio: Option<f64>,
) -> (bool, bool) {
    let blocks = classify_blocks(tiers);
    let is_anchored = |t: &MeetTierInput| matches!(t.constraint, MeetConstraint::ScaleReference(_));

    let girdle_idxs: Vec<usize> = (0..tiers.len())
        .filter(|&i| blocks[i] == Block::Girdle)
        .collect();
    if let Some(&first) = girdle_idxs.first()
        && !girdle_idxs.iter().any(|&i| is_anchored(&tiers[i]))
    {
        tiers[first].constraint = MeetConstraint::ScaleReference(GIRDLE_REFERENCE_MAST);
    }

    let girdle_r = girdle_radius_for_anchor(tiers, &blocks);

    let mut anchor_block = |block: Block, ratio: Option<f64>| -> bool {
        let Some(ratio) = ratio else { return false };
        let idxs: Vec<usize> = (0..tiers.len()).filter(|&i| blocks[i] == block).collect();
        if idxs.is_empty() || idxs.iter().any(|&i| is_anchored(&tiers[i])) {
            return false;
        }
        let explicit = explicit_table_or_culet(tiers, &blocks, block);
        let target = explicit.unwrap_or(idxs[0]);
        let theta_deg = tiers[target].angle_deg;
        let mast = geometric_ratio_anchor(theta_deg, ratio, girdle_r, explicit.is_some());
        tiers[target].constraint = MeetConstraint::ScaleReference(mast);
        true
    };

    let crown_anchored = anchor_block(Block::Crown, cw_ratio);
    let pavilion_anchored = anchor_block(Block::Pavilion, pw_ratio);
    (crown_anchored, pavilion_anchored)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `apply_ratio_anchors` must anchor the crown block's explicit table tier
    /// (angle exactly `0.0`, one instance) with `geometric_ratio_anchor(0.0,
    /// cw_ratio, girdle_r)`, which for a flat table (`theta = 0`) reduces exactly
    /// to `cw_ratio * 2.0 * girdle_r`, and the girdle block's first tier with the
    /// fixed reference mast -- reproducing the design-4-shaped case from the corpus
    /// (flat table, no explicit culet: a pointed pavilion).
    #[test]
    fn apply_ratio_anchors_uses_the_explicit_table_tier_when_present() {
        let mut tiers = vec![
            MeetTierInput {
                angle_deg: -43.0,
                indices: vec![4.0, 12.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["P1".to_string()],
            },
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![4.0, 12.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["G1".to_string()],
            },
            MeetTierInput {
                angle_deg: 44.92,
                indices: vec![4.0, 12.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["A".to_string()],
            },
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["Table".to_string()],
            },
        ];

        let (crown_anchored, pavilion_anchored) =
            apply_ratio_anchors(&mut tiers, Some(0.167), None);
        assert!(crown_anchored, "crown should anchor: cw_ratio was supplied");
        assert!(
            !pavilion_anchored,
            "pavilion should not anchor: pw_ratio was None and no tier already anchored"
        );

        // Girdle: first (only) girdle tier, fixed reference mast.
        match tiers[1].constraint {
            MeetConstraint::ScaleReference(v) => {
                assert!((v - GIRDLE_REFERENCE_MAST).abs() < 1e-12);
            }
            _ => panic!("girdle tier should have been anchored"),
        }
        // Crown: the explicit table tier (index 3), not the first crown tier
        // (index 2). Flat table (theta = 0.0) => mast = cw_ratio * 2.0 * girdle_r
        // exactly, with girdle_r = GIRDLE_REFERENCE_MAST here (only girdle tier).
        match tiers[3].constraint {
            MeetConstraint::ScaleReference(v) => {
                let expected = 0.167 * 2.0 * GIRDLE_REFERENCE_MAST;
                assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
            }
            _ => panic!("table tier should have been anchored"),
        }
        assert!(
            matches!(tiers[2].constraint, MeetConstraint::MeetExisting),
            "the non-table crown tier must be left alone"
        );
        // Pavilion untouched (pw_ratio was None).
        assert!(matches!(tiers[0].constraint, MeetConstraint::MeetExisting));
    }

    /// With no explicit table/culet tier, `apply_ratio_anchors` falls back to the
    /// block's first tier in file order, using [`geometric_ratio_anchor`] with that
    /// tier's own (nonzero) angle.
    #[test]
    fn apply_ratio_anchors_falls_back_to_the_first_tier_of_the_block() {
        let mut tiers = vec![
            MeetTierInput {
                angle_deg: -41.0,
                indices: vec![1.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["P1".to_string()],
            },
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![1.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["G1".to_string()],
            },
            MeetTierInput {
                angle_deg: 38.42,
                indices: vec![1.0],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["A".to_string()],
            },
        ];

        let (crown_anchored, pavilion_anchored) =
            apply_ratio_anchors(&mut tiers, Some(0.184), Some(0.435));
        assert!(crown_anchored);
        assert!(pavilion_anchored);

        match tiers[2].constraint {
            MeetConstraint::ScaleReference(v) => {
                let expected = geometric_ratio_anchor(38.42, 0.184, GIRDLE_REFERENCE_MAST, false);
                assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
            }
            _ => panic!("only crown tier should have been anchored (first-tier fallback)"),
        }
        match tiers[0].constraint {
            MeetConstraint::ScaleReference(v) => {
                let expected = geometric_ratio_anchor(-41.0, 0.435, GIRDLE_REFERENCE_MAST, false);
                assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
            }
            _ => panic!("only pavilion tier should have been anchored (first-tier fallback)"),
        }
        match tiers[1].constraint {
            MeetConstraint::ScaleReference(v) => assert!((v - GIRDLE_REFERENCE_MAST).abs() < 1e-12),
            _ => panic!("girdle tier should have been anchored"),
        }
    }

    /// [`geometric_ratio_anchor`] is pure plane geometry, not a fitted scalar, and
    /// picks between two distinct relations by `is_flat`: a flat tier
    /// (`is_flat = true`, always `theta = 0` in practice, but the formula itself
    /// doesn't even look at `theta_deg` in this branch) must equal `ratio * 2 *
    /// girdle_r` exactly (a horizontal plane's mast *is* its height); a
    /// girdle-adjacent tier (`is_flat = false`) must equal the pure girdle-rim
    /// formula `sin(theta) * girdle_r`, completely independent of `ratio` --
    /// mixing in a height term there was tried and measured to overshoot badly
    /// (see the doc comment on [`geometric_ratio_anchor`]).
    #[test]
    fn geometric_ratio_anchor_matches_its_two_cases() {
        let girdle_r = 1.37;
        let ratio = 0.21;

        // is_flat = true: mast must equal the block height exactly, regardless of
        // theta_deg (a real caller only ever passes 0.0 here, but the formula
        // itself doesn't depend on it).
        let flat = geometric_ratio_anchor(123.0, ratio, girdle_r, true);
        assert!((ratio * 2.0).mul_add(-girdle_r, flat).abs() < 1e-12);

        // is_flat = false: mast must equal sin(theta) * girdle_r, completely
        // independent of ratio -- two different ratios must give the same result.
        let theta_deg = 41.0;
        let a = geometric_ratio_anchor(theta_deg, 0.0, girdle_r, false);
        let b = geometric_ratio_anchor(theta_deg, 0.9, girdle_r, false);
        let expected = theta_deg.to_radians().sin() * girdle_r;
        assert!((a - expected).abs() < 1e-12);
        assert!((b - expected).abs() < 1e-12);

        // Sign on theta_deg must not matter (crown vs. pavilion angles are always
        // passed unsigned magnitude by the caller after block classification
        // already consumed the sign).
        let neg = geometric_ratio_anchor(-theta_deg, ratio, girdle_r, false);
        assert!((neg - expected).abs() < 1e-12);
    }

    /// [`girdle_radius_for_anchor`] must read this design's *own* girdle radius,
    /// not a hardcoded value: when the schedule already states more than one
    /// girdle tier's real mast (distinct width/length girdle rows, as a step cut
    /// can have), the anchor must use the smallest one (`W` is the narrower
    /// outline dimension), and the resulting crown anchor mast must actually
    /// change with it -- not stay pinned at `GIRDLE_REFERENCE_MAST`.
    #[test]
    fn apply_ratio_anchors_uses_this_designs_own_girdle_radius_not_a_fixed_one() {
        // Table listed first so its unsigned-zero angle inherits the crown side
        // from the default initial `last_crown = true` (see `tier_sides`), keeping
        // this test's classification independent of the girdle tiers' own sign.
        let mut tiers = vec![
            MeetTierInput {
                angle_deg: 0.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec!["Table".to_string()],
            },
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![0.0],
                constraint: MeetConstraint::ScaleReference(0.75), // narrower girdle row (W)
                names: vec!["G1".to_string()],
            },
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![4.0],
                constraint: MeetConstraint::ScaleReference(1.20), // wider girdle row (L)
                names: vec!["G2".to_string()],
            },
        ];

        let (crown_anchored, _) = apply_ratio_anchors(&mut tiers, Some(0.20), None);
        assert!(crown_anchored);

        // Flat table: mast = cw_ratio * 2 * girdle_r, and girdle_r must be the
        // smaller of the two stated girdle masts (0.75), not the larger (1.20) and
        // not GIRDLE_REFERENCE_MAST (neither girdle tier needed the fallback, since
        // both already carried a real scale reference).
        match tiers[0].constraint {
            MeetConstraint::ScaleReference(v) => {
                let expected = 0.20 * 2.0 * 0.75;
                assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
            }
            _ => panic!("table tier should have been anchored"),
        }
        // Both stated girdle references must be left exactly as given.
        assert!(
            matches!(tiers[1].constraint, MeetConstraint::ScaleReference(v) if (v - 0.75).abs() < 1e-12)
        );
        assert!(
            matches!(tiers[2].constraint, MeetConstraint::ScaleReference(v) if (v - 1.20).abs() < 1e-12)
        );
    }

    /// A block that already carries an explicit scale reference (a real "Set stone
    /// size"/"Set girdle thickness" instruction) must never be overwritten by a
    /// ratio-derived estimate.
    #[test]
    fn apply_ratio_anchors_never_overrides_an_existing_scale_reference() {
        let mut tiers = vec![
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![1.0],
                constraint: MeetConstraint::ScaleReference(0.789),
                names: vec!["G1".to_string()],
            },
            MeetTierInput {
                angle_deg: 40.0,
                indices: vec![1.0],
                constraint: MeetConstraint::ScaleReference(0.709),
                names: vec!["C1".to_string()],
            },
        ];

        let (crown_anchored, _) = apply_ratio_anchors(&mut tiers, Some(0.244), None);
        assert!(!crown_anchored, "the crown block already had an anchor");
        assert!(
            matches!(tiers[0].constraint, MeetConstraint::ScaleReference(v) if (v - 0.789).abs() < 1e-12)
        );
        assert!(
            matches!(tiers[1].constraint, MeetConstraint::ScaleReference(v) if (v - 0.709).abs() < 1e-12)
        );
    }
}

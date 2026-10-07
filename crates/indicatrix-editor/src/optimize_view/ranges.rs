//! The Optimize tab's "Angle ranges": which tiers a run may move, the default range of
//! each, and reading a range a cutter typed.
//!
//! A range is a signed pair of angles in degrees, `(min, max)` with `min <= max`, the
//! form `OptimizeOptions::angle_bounds` takes. A pavilion tier has a negative angle, so
//! its range is negative too (`(-46.0, -36.0)` for a tier at `-41.0` and a spread of 5).

use crate::loading::eval_number;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    Design,
    optimize::{MAX_SAFE_CANDIDATE_ANGLE_DEG, angle_is_variable},
};
use std::collections::BTreeMap;

/// How far either side of its current angle a tier may move unless the cutter says
/// otherwise, in degrees.
pub const DEFAULT_RANGE_DEG: f64 = 5.0;

/// The smallest angle magnitude a range may reach: half a degree off horizontal.
///
/// It is the same half-degree margin the optimizer keeps from vertical
/// ([`MAX_SAFE_CANDIDATE_ANGLE_DEG`]). A range never crosses zero or the girdle.
pub const MIN_RANGE_ANGLE_DEG: f64 = 0.5;

/// One tier of the "Angle ranges" table.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeRow {
    /// The tier's position in the design.
    pub tier: usize,
    /// What the tier table calls the tier: its own name, or its standard code when the name is
    /// empty or old-style ([`crate::retarget::plan::tier_display_names`]).
    pub name: String,
    /// The tier's current angle, signed (shown as a magnitude).
    pub angle_deg: f64,
    /// Whether the angle follows a relation: listed, never moved.
    pub driven: bool,
    /// The default range's lower end, signed.
    pub min_deg: f64,
    /// The default range's upper end, signed.
    pub max_deg: f64,
}

impl RangeRow {
    /// The tier as a label for a message: `#5 (P2)`, or `#5` for an unnamed tier.
    #[must_use]
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            format!("#{}", self.tier + 1)
        } else {
            format!("#{} ({})", self.tier + 1, self.name)
        }
    }
}

/// What the cutter typed into one row of the table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RangeInput {
    /// The tier's position in the design.
    pub tier: usize,
    /// The minimum field, as typed.
    pub min_text: String,
    /// The maximum field, as typed.
    pub max_text: String,
}

/// Puts a pair of angle magnitudes on the side of the girdle `angle_deg` is on.
fn signed_range(angle_deg: f64, low: f64, high: f64) -> (f64, f64) {
    if angle_deg.is_sign_negative() {
        (-high, -low)
    } else {
        (low, high)
    }
}

/// The range a tier at `angle_deg` gets unless the cutter edits it.
///
/// Five degrees either side, kept between half a degree and the 89.5 degrees the optimizer
/// never passes, so it never crosses zero or the girdle. It always holds the current angle.
#[must_use]
pub fn default_range(angle_deg: f64) -> (f64, f64) {
    let magnitude = angle_deg.abs();
    let low = magnitude.min((magnitude - DEFAULT_RANGE_DEG).max(MIN_RANGE_ANGLE_DEG));
    let high = magnitude.max((magnitude + DEFAULT_RANGE_DEG).min(MAX_SAFE_CANDIDATE_ANGLE_DEG));
    signed_range(angle_deg, low, high)
}

/// Every tier the table lists: the tiers a run could move, and the tiers that follow a
/// relation (listed so the cutter sees why they are not offered).
///
/// A tier a run could move is not horizontal or vertical, does not follow a relation, and
/// is either free (not pinned to a scale reference) or pinned while `vary_anchored` is on.
/// Rows come in design order.
#[must_use]
pub fn range_rows(design: &Design, vary_anchored: bool) -> Vec<RangeRow> {
    // What the tier table calls each tier: its own name, or its standard code when the name
    // is empty or old-style. The messages about a range name the tier this way.
    let names = crate::retarget::plan::tier_display_names(design);
    design
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, tier)| angle_is_variable(tier.angle_deg))
        .filter_map(|(index, tier)| {
            let driven = design.is_tier_driven(index);
            let pinned = matches!(tier.constraint, MeetConstraint::ScaleReference(_));
            if pinned && !driven && !vary_anchored {
                return None;
            }
            let (min_deg, max_deg) = default_range(tier.angle_deg);
            Some(RangeRow {
                tier: index,
                name: names.get(index).cloned().unwrap_or_default(),
                angle_deg: tier.angle_deg,
                driven,
                min_deg,
                max_deg,
            })
        })
        .collect()
}

/// A number for an angle field: two decimals at most, no trailing zeros (`41.5`, `-36`).
///
/// Zero is always `0`, never `-0`: that covers a negative zero and any small negative value
/// that rounds to zero at two decimals (`-0.004`). A range end ignores its sign anyway.
#[must_use]
pub fn format_range_value(value: f64) -> String {
    let text = format!("{value:.2}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" || trimmed == "-0" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Reads one end of a range. The sign is ignored: a range lies on the side of the girdle
/// the tier is on, so `40` and `-40` mean the same for a pavilion tier.
fn read_angle_magnitude(label: &str, text: &str) -> Result<f64, String> {
    let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
    if value.is_finite() {
        Ok(value.abs())
    } else {
        Err(format!("{label} '{}' is not a number.", text.trim()))
    }
}

/// Reads the range a cutter typed for a tier at `angle_deg`.
///
/// Each field may hold arithmetic (`41.5 + 0.5`). The ends are put in order, kept between
/// [`MIN_RANGE_ANGLE_DEG`] and the optimizer's 89.5 degree limit, put on the tier's side of
/// the girdle, and widened to hold the current angle: a range that left the tier outside
/// itself would not describe the stone being optimized.
///
/// # Errors
///
/// A message naming the field that is not a number.
pub fn parse_range(angle_deg: f64, min_text: &str, max_text: &str) -> Result<(f64, f64), String> {
    let first = read_angle_magnitude("Minimum angle", min_text)?;
    let second = read_angle_magnitude("Maximum angle", max_text)?;
    let magnitude = angle_deg.abs();
    let low = first
        .min(second)
        .clamp(MIN_RANGE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG)
        .min(magnitude);
    let high = first
        .max(second)
        .clamp(MIN_RANGE_ANGLE_DEG, MAX_SAFE_CANDIDATE_ANGLE_DEG)
        .max(magnitude);
    Ok(signed_range(angle_deg, low, high))
}

/// The `angle_bounds` for a run: the range of every tier the run may move.
///
/// A tier the cutter edited uses what was typed (`inputs`); every other tier uses its
/// default range. Tiers that follow a relation get no bound: they are never moved.
///
/// # Errors
///
/// A message naming the tier and the field of the first range that cannot be read.
pub fn range_bounds(
    design: &Design,
    vary_anchored: bool,
    inputs: &[RangeInput],
) -> Result<BTreeMap<usize, (f64, f64)>, String> {
    let mut bounds = BTreeMap::new();
    for row in range_rows(design, vary_anchored)
        .into_iter()
        .filter(|row| !row.driven)
    {
        let range = match inputs.iter().find(|input| input.tier == row.tier) {
            Some(input) => parse_range(row.angle_deg, &input.min_text, &input.max_text)
                .map_err(|reason| format!("Angle range of tier {}: {reason}", row.label()))?,
            None => (row.min_deg, row.max_deg),
        };
        bounds.insert(row.tier, range);
    }
    Ok(bounds)
}

/// One sentence above the table: how many tiers a run could move and how many follow a
/// relation.
#[must_use]
pub fn range_summary(rows: &[RangeRow]) -> String {
    let movable = rows.iter().filter(|row| !row.driven).count();
    let driven = rows.len() - movable;
    let change = if movable == 1 {
        "1 tier can change".to_string()
    } else {
        format!("{movable} tiers can change")
    };
    let follow = if driven == 1 {
        "1 tier follows a relation and stays put".to_string()
    } else {
        format!("{driven} tiers follow a relation and stay put")
    };
    match (movable, driven) {
        (0, 0) => "No tier can change with these settings.".to_string(),
        (_, 0) => format!("{change}."),
        (0, _) => format!("{follow}."),
        _ => format!("{change}; {follow}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;
    use indicatrix_cut_core::ConstraintTier;

    fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// A pinned crown tier `A`, then a free pavilion tier `P` at -41, a pinned girdle tier
    /// at -90 and a pinned table.
    fn design() -> Design {
        let mut design = EditorSession::fresh().design;
        design
            .tiers
            .push(tier("A", 40.0, MeetConstraint::ScaleReference(0.6)));
        design
            .tiers
            .push(tier("P", -41.0, MeetConstraint::MeetExisting));
        design
            .tiers
            .push(tier("G", -90.0, MeetConstraint::ScaleReference(0.9)));
        design
            .tiers
            .push(tier("T", 0.0, MeetConstraint::ScaleReference(0.4)));
        design
    }

    // --- the default range ---

    #[test]
    fn the_default_range_is_five_degrees_either_side() {
        assert_eq!(default_range(40.0), (35.0, 45.0));
        assert_eq!(default_range(-41.0), (-46.0, -36.0));
    }

    #[test]
    fn the_default_range_never_crosses_zero_or_the_girdle() {
        assert_eq!(default_range(3.0), (0.5, 8.0));
        assert_eq!(default_range(87.0), (82.0, 89.5));
        assert_eq!(default_range(-2.0), (-7.0, -0.5));
        assert_eq!(default_range(-88.0), (-89.5, -83.0));
    }

    #[test]
    fn the_default_range_always_holds_the_current_angle() {
        for angle in [0.2, 0.5, 1.0, 44.3, 89.0, -0.3, -45.0, -89.4] {
            let (low, high) = default_range(angle);
            assert!(
                low <= angle && angle <= high,
                "{angle} outside ({low}, {high})"
            );
        }
    }

    // --- which tiers are listed ---

    #[test]
    fn a_design_with_no_pinned_tiers_lists_every_movable_tier() {
        let design = design();
        let rows = range_rows(&design, false);
        assert_eq!(rows.iter().map(|r| r.tier).collect::<Vec<_>>(), vec![1]);
        // The tier is named `P` (an old-style single letter), so the row carries the standard
        // code the tier table shows.
        let codes = indicatrix_cut_core::compute_tier_labels(&design.tiers);
        assert_eq!(rows[0].name, codes[1].code);
        assert_ne!(rows[0].name, "P");
        assert_eq!((rows[0].min_deg, rows[0].max_deg), (-46.0, -36.0));
        assert!(!rows[0].driven);
    }

    #[test]
    fn varying_anchored_tiers_adds_the_pinned_ones_but_never_the_table_or_girdle() {
        let design = design();
        let rows = range_rows(&design, true);
        assert_eq!(rows.iter().map(|r| r.tier).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn a_tier_that_follows_a_relation_is_listed_but_gets_no_bound() {
        let mut design = design();
        design
            .tiers
            .push(tier("B", 38.0, MeetConstraint::MeetExisting));
        design.ensure_tier_ids();
        let relation = design.parse_relation("A - 2").expect("the relation reads");
        let id = design.tier_ids[4];
        design.tier_relations.insert(id, relation);

        let rows = range_rows(&design, false);
        let follower = rows.iter().find(|row| row.tier == 4).expect("listed");
        assert!(follower.driven);
        assert_eq!(rows.iter().filter(|row| row.driven).count(), 1);

        let bounds = range_bounds(&design, false, &[]).expect("defaults read");
        assert!(bounds.contains_key(&1));
        assert!(!bounds.contains_key(&4), "a driven tier is never bounded");
    }

    #[test]
    fn a_pinned_tier_that_follows_a_relation_is_still_listed_as_driven() {
        let mut design = design();
        design.ensure_tier_ids();
        let relation = design.parse_relation("P - 1").expect("the relation reads");
        let id = design.tier_ids[0];
        design.tier_relations.insert(id, relation);
        let rows = range_rows(&design, false);
        assert!(rows.iter().any(|row| row.tier == 0 && row.driven));
    }

    // --- reading what was typed ---

    #[test]
    fn a_typed_range_may_hold_arithmetic() {
        assert_eq!(
            parse_range(40.0, "38 + 0.5", "(80 + 10) / 2"),
            Ok((38.5, 45.0))
        );
    }

    #[test]
    fn a_typed_range_is_put_in_order() {
        assert_eq!(parse_range(40.0, "45", "35"), Ok((35.0, 45.0)));
    }

    #[test]
    fn a_pavilion_range_may_be_typed_with_or_without_the_sign() {
        assert_eq!(parse_range(-41.0, "36", "46"), Ok((-46.0, -36.0)));
        assert_eq!(parse_range(-41.0, "-46", "-36"), Ok((-46.0, -36.0)));
    }

    #[test]
    fn a_typed_range_is_clamped_so_nothing_crosses_zero_or_the_girdle() {
        assert_eq!(parse_range(10.0, "0", "95"), Ok((0.5, 89.5)));
        assert_eq!(parse_range(-10.0, "0", "120"), Ok((-89.5, -0.5)));
    }

    #[test]
    fn a_typed_range_is_widened_to_hold_the_current_angle() {
        assert_eq!(parse_range(40.0, "42", "44"), Ok((40.0, 44.0)));
        assert_eq!(parse_range(40.0, "30", "35"), Ok((30.0, 40.0)));
    }

    #[test]
    fn a_range_that_is_not_a_number_names_the_field() {
        let error = parse_range(40.0, "abc", "45").unwrap_err();
        assert!(error.starts_with("Minimum angle"), "{error}");
        assert!(error.contains("is not a number"), "{error}");
        let error = parse_range(40.0, "35", "").unwrap_err();
        assert!(error.starts_with("Maximum angle"), "{error}");
        assert!(parse_range(40.0, "inf", "45").is_err());
        assert!(parse_range(40.0, "NaN", "45").is_err());
    }

    #[test]
    fn the_bounds_use_what_was_typed_and_the_defaults_for_the_rest() {
        let design = design();
        let inputs = [RangeInput {
            tier: 1,
            min_text: "38".to_string(),
            max_text: "-40".to_string(),
        }];
        let bounds = range_bounds(&design, true, &inputs).expect("reads");
        assert_eq!(bounds.get(&1), Some(&(-41.0, -38.0)));
        assert_eq!(bounds.get(&0), Some(&(35.0, 45.0)));
        assert_eq!(bounds.len(), 2);
    }

    #[test]
    fn a_bad_range_names_the_tier() {
        let design = design();
        let inputs = [RangeInput {
            tier: 1,
            min_text: "x".to_string(),
            max_text: "40".to_string(),
        }];
        let error = range_bounds(&design, false, &inputs).unwrap_err();
        // The tier is named `P` (old-style), so the message shows its standard code `P1`.
        assert!(error.contains("#2 (P1)"), "{error}");
    }

    // --- text ---

    #[test]
    fn range_values_are_shown_without_trailing_zeros() {
        assert_eq!(format_range_value(41.5), "41.5");
        assert_eq!(format_range_value(-36.0), "-36");
        assert_eq!(format_range_value(35.0), "35");
        assert_eq!(format_range_value(0.126), "0.13");
        assert_eq!(format_range_value(-0.001), "0");
    }

    #[test]
    fn zero_is_never_shown_with_a_minus_sign() {
        // A negative zero and negative values that round to zero at two decimals.
        for value in [0.0, -0.0, 0.004, -0.0004, -0.004, -1e-12] {
            assert_eq!(format_range_value(value), "0", "{value:e}");
        }
        // A real negative value keeps its sign, and a trailing zero before the point stays.
        assert_eq!(format_range_value(-0.01), "-0.01");
        assert_eq!(format_range_value(-10.0), "-10");
        assert_eq!(format_range_value(100.0), "100");
    }

    #[test]
    fn the_summary_counts_movable_and_following_tiers() {
        let row = |driven| RangeRow {
            tier: 0,
            name: String::new(),
            angle_deg: 40.0,
            driven,
            min_deg: 35.0,
            max_deg: 45.0,
        };
        assert_eq!(
            range_summary(&[]),
            "No tier can change with these settings."
        );
        assert_eq!(range_summary(&[row(false)]), "1 tier can change.");
        assert_eq!(
            range_summary(&[row(false), row(false), row(true)]),
            "2 tiers can change; 1 tier follows a relation and stays put."
        );
        assert_eq!(
            range_summary(&[row(true), row(true)]),
            "2 tiers follow a relation and stay put."
        );
    }

    #[test]
    fn a_row_label_names_the_tier() {
        let mut row = RangeRow {
            tier: 4,
            name: "P2".to_string(),
            angle_deg: -40.0,
            driven: false,
            min_deg: -45.0,
            max_deg: -35.0,
        };
        assert_eq!(row.label(), "#5 (P2)");
        row.name.clear();
        assert_eq!(row.label(), "#5");
    }
}

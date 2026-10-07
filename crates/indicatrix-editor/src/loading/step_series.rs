//! The tier table's "Generate steps" form: parsing and validating its numeric fields.

use super::number_expr::{NumberExprError, eval_number};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::ConstraintTier;
use std::fmt;

/// The steepest angle magnitude a tier can have: angles are measured from the girdle
/// plane, so no crown or pavilion facet is steeper than a right angle.
const MAX_ANGLE_DEG: f64 = 90.0;

/// Why the "Generate steps" form was refused.
///
/// Its [`fmt::Display`] text is the message to show the cutter, and it converts
/// into a `String` of that text for callers that only display it.
#[derive(Debug, Clone, PartialEq)]
pub enum StepSeriesError {
    /// The start angle field is not a number.
    StartNotANumber(String),
    /// The angle step field is not a number.
    StepNotANumber(String),
    /// The start angle or the angle step is infinite or NaN.
    NotFinite,
    /// The tier count is below 1 or above [`ConstraintTier::MAX_STEP_SERIES`].
    CountOutOfRange {
        /// The count that was asked for.
        count: i32,
    },
    /// A generated tier's angle is 0 degrees or steeper than 90 degrees (either side).
    AngleOutOfRange {
        /// The 1-based position of the offending tier in the ladder.
        tier: usize,
        /// Its angle, in degrees.
        angle_deg: f64,
    },
    /// A generated tier's angle falls on the other side of the girdle from the start
    /// angle, which would put the ladder across the crown/pavilion boundary.
    CrossesBlock {
        /// The 1-based position of the offending tier in the ladder.
        tier: usize,
        /// Its angle, in degrees.
        angle_deg: f64,
    },
    /// The anchor field is not a number.
    AnchorNotANumber(String),
    /// The anchor is infinite or NaN.
    AnchorNotFinite,
    /// A field holds a calculation (`40 + 2`) that cannot be read or done.
    CannotCalculate {
        /// The field's name as the message shows it ("Start angle", "Angle step",
        /// "Anchor").
        field: &'static str,
        /// What was typed, trimmed.
        text: String,
        /// Why it cannot be done, as a phrase without a final full stop.
        reason: String,
    },
}

impl fmt::Display for StepSeriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartNotANumber(text) => write!(f, "Start angle '{text}' is not a number."),
            Self::StepNotANumber(text) => write!(f, "Angle step '{text}' is not a number."),
            Self::NotFinite => f.write_str("Start angle and angle step must be finite numbers."),
            Self::CountOutOfRange { count } => write!(
                f,
                "Tier count {count} is out of range -- enter a whole number from 1 to {}.",
                ConstraintTier::MAX_STEP_SERIES
            ),
            // Angles in a message are magnitudes: the side of the girdle is the block's, never
            // a sign (the stored `angle_deg` stays signed).
            Self::AngleOutOfRange { tier, angle_deg } => write!(
                f,
                "Tier {tier} of the ladder would sit at {:.2}\u{b0} -- every generated \
                 angle must be more than 0\u{b0} and at most {MAX_ANGLE_DEG}\u{b0} from the \
                 girdle plane.",
                angle_deg.abs()
            ),
            Self::CrossesBlock { tier, angle_deg } => write!(
                f,
                "Tier {tier} of the ladder would cross the girdle (it would sit at {:.2}\u{b0} \
                 on the other side of the start angle's block) -- a ladder stays within one \
                 block (crown or pavilion).",
                angle_deg.abs()
            ),
            Self::AnchorNotANumber(text) => write!(f, "Anchor '{text}' is not a number."),
            Self::AnchorNotFinite => f.write_str("Anchor must be a finite number."),
            Self::CannotCalculate {
                field,
                text,
                reason,
            } => write!(f, "{field} '{text}' cannot be calculated: {reason}."),
        }
    }
}

impl std::error::Error for StepSeriesError {}

impl From<StepSeriesError> for String {
    fn from(error: StepSeriesError) -> Self {
        error.to_string()
    }
}

/// The "Generate steps" form's parsing half.
///
/// Everything the form needs turned into real
/// values BEFORE `ConstraintTier::step_series` can be called, except the indices list,
/// which the caller parses separately via [`super::parse_index_list`] (that parse also
/// needs the design's own `gear_teeth_abs`, not available here). An empty anchor field
/// means [`MeetConstraint::MeetExisting`] (the same "blank means use whatever's already
/// anchored" convention -- the caller is relying on an anchor elsewhere in the design); a
/// non-empty one is parsed and pinned via [`MeetConstraint::ScaleReference`].
///
/// The ladder it describes must be one a cutter can actually cut: `count` is from 1 to
/// [`ConstraintTier::MAX_STEP_SERIES`], and every generated angle (`start_angle` plus
/// `n * angle_step`, the way `ConstraintTier::step_series` computes it) has a magnitude
/// above 0 and at most 90 degrees and stays on the start angle's side of the girdle --
/// crown (positive) or pavilion (negative), never across.
///
/// Moved here from the desktop's `tier_actions/tier_generation.rs` so the web
/// app's step generator parses identically.
///
/// # Errors
///
/// A [`StepSeriesError`] naming the offending field or tier, ready to show the cutter.
pub fn parse_step_series_form(
    start_angle_text: &str,
    angle_step_text: &str,
    count: i32,
    anchor_text: &str,
) -> Result<(f64, f64, usize, MeetConstraint), StepSeriesError> {
    let start_angle = read_field(
        "Start angle",
        start_angle_text,
        StepSeriesError::StartNotANumber,
    )?;
    let angle_step = read_field(
        "Angle step",
        angle_step_text,
        StepSeriesError::StepNotANumber,
    )?;
    if !start_angle.is_finite() || !angle_step.is_finite() {
        return Err(StepSeriesError::NotFinite);
    }
    let ladder_len = usize::try_from(count)
        .ok()
        .filter(|&len| (1..=ConstraintTier::MAX_STEP_SERIES).contains(&len))
        .ok_or(StepSeriesError::CountOutOfRange { count })?;
    let anchor_text = anchor_text.trim();
    let first_constraint = if anchor_text.is_empty() {
        MeetConstraint::MeetExisting
    } else {
        let anchor = read_field("Anchor", anchor_text, StepSeriesError::AnchorNotANumber)?;
        if !anchor.is_finite() {
            return Err(StepSeriesError::AnchorNotFinite);
        }
        MeetConstraint::ScaleReference(anchor)
    };
    check_ladder_angles(start_angle, angle_step, ladder_len)?;
    Ok((start_angle, angle_step, ladder_len, first_constraint))
}

/// Reads one numeric field of the form, which may be arithmetic (`40 + 2`). Text that
/// is not a number and holds no calculation becomes the field's historical
/// "not a number" error (`not_a_number`); a calculation that cannot be done becomes
/// [`StepSeriesError::CannotCalculate`].
fn read_field(
    label: &'static str,
    text: &str,
    not_a_number: fn(String) -> StepSeriesError,
) -> Result<f64, StepSeriesError> {
    eval_number(text, None).map_err(|error| match error {
        NumberExprError::NotANumber => not_a_number(text.to_string()),
        NumberExprError::Invalid(reason) => StepSeriesError::CannotCalculate {
            field: label,
            text: text.trim().to_string(),
            reason,
        },
    })
}

/// Checks every angle of a `len`-tier ladder from `start_angle` by `angle_step`: each
/// within (0, 90] degrees of magnitude and on the start angle's side of the girdle.
fn check_ladder_angles(
    start_angle: f64,
    angle_step: f64,
    len: usize,
) -> Result<(), StepSeriesError> {
    let start_is_pavilion = start_angle.is_sign_negative();
    for n in 0..len {
        let angle_deg = angle_step.mul_add(n as f64, start_angle);
        let magnitude = angle_deg.abs();
        let within_range = magnitude > 0.0 && magnitude <= MAX_ANGLE_DEG;
        if !within_range {
            return Err(StepSeriesError::AngleOutOfRange {
                tier: n + 1,
                angle_deg,
            });
        }
        if angle_deg.is_sign_negative() != start_is_pavilion {
            return Err(StepSeriesError::CrossesBlock {
                tier: n + 1,
                angle_deg,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_anchor_means_meet_existing() {
        let (start, step, count, constraint) =
            parse_step_series_form("40", " 0.5 ", 3, "  ").expect("parses");
        assert_eq!(start.to_bits(), 40.0_f64.to_bits());
        assert_eq!(step.to_bits(), 0.5_f64.to_bits());
        assert_eq!(count, 3);
        assert_eq!(constraint, MeetConstraint::MeetExisting);
    }

    #[test]
    fn an_anchor_pins_the_first_tier() {
        let (_, _, _, constraint) = parse_step_series_form("30", "1", 1, "0.32").expect("parses");
        assert_eq!(constraint, MeetConstraint::ScaleReference(0.32));
    }

    #[test]
    fn each_bad_field_is_named() {
        assert_eq!(
            parse_step_series_form("x", "1", 1, "").unwrap_err(),
            StepSeriesError::StartNotANumber("x".to_string())
        );
        assert!(
            parse_step_series_form("x", "1", 1, "")
                .unwrap_err()
                .to_string()
                .starts_with("Start angle 'x'")
        );
        assert_eq!(
            parse_step_series_form("30", "y", 1, "").unwrap_err(),
            StepSeriesError::StepNotANumber("y".to_string())
        );
        assert_eq!(
            parse_step_series_form("30", "1", 1, "z").unwrap_err(),
            StepSeriesError::AnchorNotANumber("z".to_string())
        );
        assert_eq!(
            parse_step_series_form("inf", "1", 1, "").unwrap_err(),
            StepSeriesError::NotFinite
        );
        assert_eq!(
            parse_step_series_form("30", "1", 1, "inf").unwrap_err(),
            StepSeriesError::AnchorNotFinite
        );
    }

    #[test]
    fn the_count_is_held_to_one_through_the_cap() {
        let cap = i32::try_from(ConstraintTier::MAX_STEP_SERIES).expect("the cap fits an i32");
        assert!(parse_step_series_form("10", "0.5", cap, "").is_ok());
        for bad in [0, -1, cap + 1, i32::MAX, i32::MIN] {
            assert_eq!(
                parse_step_series_form("10", "0.5", bad, "").unwrap_err(),
                StepSeriesError::CountOutOfRange { count: bad },
                "count {bad}"
            );
        }
        assert!(
            parse_step_series_form("10", "0.5", 0, "")
                .unwrap_err()
                .to_string()
                .contains("1 to 64")
        );
    }

    #[test]
    fn a_crown_ladder_must_stay_above_zero_and_within_ninety() {
        // 40, 30, 20, 10, 0: the fifth rung is the table, not a crown facet.
        assert_eq!(
            parse_step_series_form("40", "-10", 5, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange {
                tier: 5,
                angle_deg: 0.0
            }
        );
        // 80, 85, 90 fits; one more rung (95) does not.
        assert!(parse_step_series_form("80", "5", 3, "").is_ok());
        assert_eq!(
            parse_step_series_form("80", "5", 4, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange {
                tier: 4,
                angle_deg: 95.0
            }
        );
        // A start of 0 is already outside the range.
        assert!(matches!(
            parse_step_series_form("0", "1", 1, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange { tier: 1, .. }
        ));
        assert!(matches!(
            parse_step_series_form("91", "0", 1, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange { tier: 1, .. }
        ));
    }

    #[test]
    fn a_pavilion_ladder_keeps_its_sign_and_the_same_limits() {
        assert!(parse_step_series_form("-40", "-5", 10, "").is_ok());
        // -40, -60, -80, -100: past the girdle on the fourth rung.
        assert_eq!(
            parse_step_series_form("-40", "-20", 4, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange {
                tier: 4,
                angle_deg: -100.0
            }
        );
        // -90 is the girdle itself and is allowed.
        assert!(parse_step_series_form("-70", "-10", 3, "").is_ok());
        // -0.0 is the culet, not a pavilion facet.
        assert!(matches!(
            parse_step_series_form("-0", "0", 1, "").unwrap_err(),
            StepSeriesError::AngleOutOfRange { tier: 1, .. }
        ));
    }

    #[test]
    fn a_ladder_may_not_cross_into_the_other_block() {
        // -40, -10, +20: the third rung is a crown facet.
        assert_eq!(
            parse_step_series_form("-40", "30", 3, "").unwrap_err(),
            StepSeriesError::CrossesBlock {
                tier: 3,
                angle_deg: 20.0
            }
        );
        // 10, -5: the second rung is a pavilion facet.
        assert_eq!(
            parse_step_series_form("10", "-15", 2, "").unwrap_err(),
            StepSeriesError::CrossesBlock {
                tier: 2,
                angle_deg: -5.0
            }
        );
    }

    #[test]
    fn the_refusal_messages_show_angles_as_positive_numbers() {
        // The error keeps the stored angle (-100); the message a person reads does not.
        let past_the_girdle = parse_step_series_form("-40", "-20", 4, "").unwrap_err();
        assert_eq!(
            past_the_girdle.to_string(),
            "Tier 4 of the ladder would sit at 100.00\u{b0} -- every generated angle must be \
             more than 0\u{b0} and at most 90\u{b0} from the girdle plane."
        );
        let across = parse_step_series_form("10", "-15", 2, "").unwrap_err();
        assert_eq!(
            across.to_string(),
            "Tier 2 of the ladder would cross the girdle (it would sit at 5.00\u{b0} on the \
             other side of the start angle's block) -- a ladder stays within one block (crown \
             or pavilion)."
        );
    }

    #[test]
    fn the_numeric_fields_take_arithmetic() {
        let (start, step, count, constraint) =
            parse_step_series_form("40 + 2", " -(1 / 2) ", 3, "0.3 + 0.02").expect("parses");
        assert_eq!(start.to_bits(), 42.0_f64.to_bits());
        assert_eq!(step.to_bits(), (-0.5_f64).to_bits());
        assert_eq!(count, 3);
        assert_eq!(constraint, MeetConstraint::ScaleReference(0.32));
    }

    #[test]
    fn a_calculation_that_fails_names_its_field() {
        let error = parse_step_series_form("40", "1 / 0", 3, "").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Angle step '1 / 0' cannot be calculated: it divides by zero."
        );
        let error = parse_step_series_form("40", "1", 3, "(2").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Anchor '(2' cannot be calculated:"),
            "{error}"
        );
        let error = parse_step_series_form("4 0 +", "1", 3, "").unwrap_err();
        assert!(
            error.to_string().starts_with("Start angle '4 0 +'"),
            "{error}"
        );
    }

    #[test]
    fn the_error_text_converts_to_a_string() {
        let message: String = StepSeriesError::NotFinite.into();
        assert_eq!(
            message,
            "Start angle and angle step must be finite numbers."
        );
    }
}

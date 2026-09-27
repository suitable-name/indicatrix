//! [`AscParseError`]: everything that can go wrong parsing an `.asc` cutting
//! schedule with [`super::parse_asc`].

use std::fmt;

/// Everything that can go wrong parsing an `.asc` cutting schedule with [`super::parse_asc`].
///
/// Every variant's [`Display`](fmt::Display) reproduces, verbatim, the same
/// human-readable message a plain `String` would -- callers that only ever
/// print or match on substrings of that message keep working unchanged; callers
/// that want to branch on the specific failure (e.g. distinguishing "missing header"
/// from "bad numeric token") can now match on the variant instead.
#[derive(Debug, Clone, PartialEq)]
pub enum AscParseError {
    /// `content` was empty or contained only whitespace.
    EmptyInput,
    /// A `g` (gear) line had fewer than the two required fields (tooth count,
    /// reference angle). `text` is the offending line, trimmed.
    GearLineMissingFields { line: usize, text: String },
    /// The `g` line's tooth-count field did not parse as a number.
    GearTeethNotNumeric { line: usize, token: String },
    /// The `g` line's reference-angle field did not parse as a number.
    GearReferenceAngleNotNumeric { line: usize, token: String },
    /// A `y` (symmetry) line had fewer than the two required fields (order, mirror
    /// flag). `text` is the offending line, trimmed.
    SymmetryLineMissingFields { line: usize, text: String },
    /// The `y` line's order field did not parse as a number.
    SymmetryOrderNotNumeric { line: usize, token: String },
    /// An `I` (refractive index) line had no value at all.
    RefractiveIndexMissing { line: usize },
    /// The `I` line's value did not parse as a number.
    RefractiveIndexNotNumeric { line: usize, token: String },
    /// A field required to be a whole number (the `g` line's gear tooth count, or the
    /// `y` line's symmetry order) was fractional or out of range. `field` names which
    /// one (e.g. `"gear tooth count"`, `"symmetry order"`).
    NotWholeNumber {
        line: usize,
        field: &'static str,
        token: String,
        value: f64,
    },
    /// The `g` line's gear tooth count was exactly zero. `phi = 2*pi*index/gear_teeth_abs()`
    /// (see [`super::AscSchedule::gear_teeth_abs`]) divides by this value downstream, so a
    /// zero-tooth gear is nonsensical, not merely unusual.
    GearTeethZero { line: usize },
    /// The `y` line's mirror-flag field was neither `y` nor `n` (case-insensitive).
    MirrorFlagInvalid { line: usize, token: String },
    /// The file had no `g` (gear teeth) header line at all.
    MissingGearLine,
    /// The file had no `y` (symmetry) header line at all.
    MissingSymmetryLine,
    /// The file had no `I` (refractive index) header line at all.
    MissingRefractiveIndexLine,
    /// The file contained no valid `a` (facet tier) records.
    NoTierRecords,
    /// An `a` record had fewer than the two required fields (angle, mast distance).
    TierRecordTooShort { line: usize, field_count: usize },
    /// An `a` record's angle field did not parse as a number.
    AngleNotNumeric { line: usize, token: String },
    /// An `a` record's mast-distance field did not parse as a number.
    MastNotNumeric { line: usize, token: String },
}

impl fmt::Display for AscParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
            Self::GearLineMissingFields { line, text } => write!(
                f,
                "line {line}: 'g' (gear) line needs a tooth count and a reference angle, got {text:?}"
            ),
            Self::GearTeethNotNumeric { line, token } => {
                write!(f, "line {line}: gear tooth count {token:?} is not numeric")
            }
            Self::GearReferenceAngleNotNumeric { line, token } => write!(
                f,
                "line {line}: gear reference angle {token:?} is not numeric"
            ),
            Self::SymmetryLineMissingFields { line, text } => write!(
                f,
                "line {line}: 'y' (symmetry) line needs an order and a mirror flag, got {text:?}"
            ),
            Self::SymmetryOrderNotNumeric { line, token } => {
                write!(f, "line {line}: symmetry order {token:?} is not numeric")
            }
            Self::RefractiveIndexMissing { line } => {
                write!(f, "line {line}: 'I' (refractive index) line has no value")
            }
            Self::RefractiveIndexNotNumeric { line, token } => {
                write!(f, "line {line}: refractive index {token:?} is not numeric")
            }
            Self::NotWholeNumber {
                line,
                field,
                token,
                value,
            } => write!(
                f,
                "line {line}: {field} {token:?} must be a whole number in range, got {value}"
            ),
            Self::GearTeethZero { line } => {
                write!(f, "line {line}: 'g' (gear) tooth count must not be zero")
            }
            Self::MirrorFlagInvalid { line, token } => write!(
                f,
                "line {line}: 'y' (symmetry) line's mirror flag {token:?} is neither 'y' nor 'n'"
            ),
            Self::MissingGearLine => write!(f, "missing required 'g' (gear teeth) header line"),
            Self::MissingSymmetryLine => {
                write!(f, "missing required 'y' (symmetry) header line")
            }
            Self::MissingRefractiveIndexLine => {
                write!(f, "missing required 'I' (refractive index) header line")
            }
            Self::NoTierRecords => write!(f, "no valid 'a' (facet tier) records found"),
            Self::TierRecordTooShort { line, field_count } => write!(
                f,
                "line {line}: 'a' record needs at least an angle and a mast distance, got {field_count} field(s)"
            ),
            Self::AngleNotNumeric { line, token } => {
                write!(f, "line {line}: angle {token:?} is not numeric")
            }
            Self::MastNotNumeric { line, token } => {
                write!(f, "line {line}: mast distance {token:?} is not numeric")
            }
        }
    }
}

impl std::error::Error for AscParseError {}

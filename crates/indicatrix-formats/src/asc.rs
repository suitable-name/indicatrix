//! Reader and writer for `GemCAD`-style `.asc` cutting-schedule files.
//!
//! `GemCAD` is Robert Strickland's faceting-design software; this module is not
//! produced, endorsed, or affiliated with `GemCAD` or its author (see the crate-level
//! docs for the full affiliation note). It exists because the `.asc` text format
//! `GemCAD` popularized is a de facto standard across the faceting community --
//! shared, archived, and re-published by many independent designers and sites -- and
//! reading (or writing) a cutting schedule shouldn't require pulling in any
//! particular renderer, database, or GUI toolkit.
//!
//! An `.asc` file's `a` records are the only place a schedule's "mast" distance --
//! how far a facet plane is cut from the stone's center -- actually lives; a
//! design's angle/index metadata is sometimes available elsewhere (e.g. scraped from
//! a catalog site) without the depth, which is exactly the gap this format fills.
//!
//! # Format (as verified against a real-world corpus of 5,759 `.asc` files spanning
//! 2,881 distinct designs, not just the format sketch)
//!
//! ```text
//! GemCad 5.0
//! g 96 0.0                                       <- gear teeth, reference angle
//! y 6 y                                           <- symmetry order, mirror flag (y/n)
//! I 1.72                                          <- refractive index
//! H PC 45.149  Round Trichecker-12                <- header/title lines (repeatable)
//! H by Fred W. Van Sant, X 51, Extra Designs 2000
//! a -41.000000 0.64991234 92 n 1 84 76 68 60 ...  <- tier: angle, mast, indices/name
//! F "For small stones"                            <- footnote (repeatable)
//! ```
//!
//! Each `a` record is: a signed `angle` (degrees from the girdle plane; `GemCAD`'s
//! convention is negative = pavilion, non-negative = crown), the `mast` distance (the
//! field this crate exists to extract reliably), then a mix of index-wheel positions
//! (usually integers, occasionally fractional) and `n <name>` markers, and finally an
//! optional `G <notes...>` free-text tail. A record can list its facet's indices in
//! more than one `n <name>` group (e.g. `92 n c ... 94 n d ...` when a compact,
//! single-tier encoding would otherwise need two rows at an identical angle/mast);
//! all of them are folded into one [`AscTier`]'s `indices`, since every one of them
//! wants a plane at the same angle and depth -- only the azimuth (index) differs.
//!
//! Long index lists wrap onto continuation lines that do not start with `a` (verified
//! against real files: continuation lines starting with a bare number, with `n`, or
//! with `G`). [`parse_asc`] treats any line that doesn't start with one of the known
//! record keywords (`GemCad`, `g`, `y`, `I`, `H`, `F`, `a`) as a continuation of
//! whatever `a` record is currently open.
//!
//! Beyond the read path, [`parse_asc`]'s lenient handling absorbs several corpus
//! realities a naive reading of the format sketch would miss: continuation lines with
//! no `a` prefix, facet names that are themselves plain numbers (name-vs-index told
//! apart only by position right after an `n` marker), fractional index positions,
//! negative gear-teeth counts (an internal handedness convention), one real file
//! missing its `g` keyword entirely, and a rare negative mast value.
//!
//! # Writing schedules
//!
//! [`to_asc_string`] serializes an [`AscSchedule`] back to `.asc` text. It is not
//! byte-identical to hand-authored `GemCAD` output (whitespace, field order within a
//! tier, and how repeated `n <name>` groups get folded back down are all
//! normalized), but it round-trips semantically: parsing its output reproduces an
//! equal [`AscSchedule`]. See the `round_trip` tests at the bottom of this file,
//! which exercise that property against real schedules pulled from the corpus.

use std::fmt;

/// Everything that can go wrong parsing an `.asc` cutting schedule with [`parse_asc`].
///
/// Every variant's [`Display`](fmt::Display) reproduces, verbatim, the human-readable
/// message the parser used to return as a plain `String` -- callers that only ever
/// printed or matched on substrings of that message keep working unchanged; callers
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

/// One `a` record: a single facet tier at a given angle and mast (height) setting,
/// occurring at one or more index-wheel positions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscTier {
    /// Signed angle from the girdle plane, in degrees. `GemCAD` convention: negative is
    /// pavilion, non-negative is crown (0 deg is a flat facet -- table on the crown
    /// side, culet on the pavilion side; distinguishing the two when the file doesn't
    /// bother signing the culet's zero is the caller's job, see `cuts.rs`).
    pub angle_deg: f64,
    /// The "mast" / height setting: how far the facet plane is cut from the stone's
    /// center, always stored as `GemCAD` wrote it (a small fraction of files use a
    /// negative mast for one special near-zero-angle facet; callers should take the
    /// magnitude when turning this into a plane offset).
    pub mast: f64,
    /// Facet name(s) as written in the file (e.g. "P1", "C7", "G1", "1", "U"). Empty
    /// when the file leaves the facet unnamed, which is common -- roughly two-thirds
    /// of real tier records never name a facet at all. When more than one distinct
    /// name shares a single tier (rare, but real), they are joined with `/`.
    pub name: String,
    /// Every index-wheel position at which this tier's facet occurs. Usually
    /// integers, but `GemCAD` allows fractional positions (about 0.2% of index tokens
    /// in the sampled corpus) for angles that don't land exactly on a gear tooth.
    pub indices: Vec<f64>,
    /// Free-text notes after the `G` marker, if any (e.g. "Cut to mast depth X.").
    pub notes: String,
}

impl AscTier {
    /// Every distinct name this tier is known by, split back out of the joined
    /// `name` field (see that field's doc comment for why more than one name folds
    /// into a single `/`-joined string). Empty if the tier is unnamed.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// Parses `notes` (the raw text after this tier's `G` marker, if any) into a
    /// structured [`MeetInstruction`]. Returns `None` when there are no notes at all.
    ///
    /// This is computed on demand from the same text [`to_asc_string`] writes back
    /// out verbatim -- there is no separate stored field, so there is nothing that
    /// could drift out of sync with the raw text or put the round-trip property at
    /// risk.
    #[must_use]
    pub fn meet_instruction(&self) -> Option<MeetInstruction> {
        parse_meet_instruction(&self.notes)
    }
}

/// A parsed `G`-field cutting/meet instruction.
///
/// `GemCAD` schedules record these as free text after a tier's `a` record (e.g. `a
/// -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1`); this is what
/// [`AscTier::meet_instruction`] parses that text into.
///
/// Parsing is deliberately lenient (case-insensitive keyword matching, not a strict
/// grammar) since these are hand-typed free-text notes -- an instruction this module
/// doesn't recognize lands in [`MeetInstruction::Other`] rather than erroring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetInstruction {
    /// `"Meet <name>[, <name>...]"` -- explicit facet-name references this tier is
    /// stated to close against (e.g. `"Meet P1, P2, G1"` -> `["P1", "P2", "G1"]`).
    /// Names are kept exactly as written; resolving them against other tiers' names
    /// is the caller's job (this module has no notion of "other tiers").
    Meet(Vec<String>),
    /// `"Cut to centerpoint"` / `"Cut to TCP"` / `"Cut to PCP"` / a bare `"TCP"` /
    /// `"PCP"` -- meets at the crown or pavilion's central closing point. Distinct
    /// from [`Self::Meet`] only in that no specific facet names are given; it is
    /// still support-function tangency against the solid formed so far (see
    /// `indicatrix::geometry::meet_solver`'s module docs).
    CutToCenterpoint,
    /// `"GMP"` / `"Girdle meet point"` -- meets at the girdle edge. Same tangency
    /// semantics as [`Self::CutToCenterpoint`], just at a different point.
    GirdleMeetPoint,
    /// `"Level girdle[.]"` -- this facet is cut to bring the (already-mounted, still
    /// rough) girdle to a true, level plane. Conventionally one of the very first
    /// cuts made, before anything else exists to meet against, so its mast is a
    /// directly chosen/measured value rather than a meet-derived one.
    LevelGirdle,
    /// `"Set girdle width"` / `"Set girdle thickness"` / `"Establish girdle
    /// thickness"` / `"Set stone size"` -- an externally supplied scale choice, not
    /// derivable from other facets.
    ScaleReference,
    /// Any other free-text note that doesn't match a recognized instruction verb
    /// (e.g. `"Cut to mast depth X."`, `"Or continuous girdle"`, a stray comment).
    Other(String),
}

/// Parses one tier's raw `G`-field text into a [`MeetInstruction`]. See
/// [`MeetInstruction`]'s doc comment for the recognized verbs and real examples.
fn parse_meet_instruction(notes: &str) -> Option<MeetInstruction> {
    let text = notes.trim();
    if text.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();

    if lower.starts_with("meet") {
        return Some(MeetInstruction::Meet(extract_meet_names(text)));
    }
    if lower.contains("gmp")
        || (lower.contains("girdle") && lower.contains("meet") && lower.contains("point"))
    {
        return Some(MeetInstruction::GirdleMeetPoint);
    }
    if lower.contains("level") && lower.contains("girdle") {
        return Some(MeetInstruction::LevelGirdle);
    }
    if lower.contains("centerpoint")
        || lower.contains("center point")
        || lower == "tcp"
        || lower == "pcp"
        || lower.contains("cut to tcp")
        || lower.contains("cut to pcp")
    {
        return Some(MeetInstruction::CutToCenterpoint);
    }
    if lower.contains("set girdle")
        || lower.contains("set stone size")
        || lower.contains("girdle thickness")
        || lower.contains("girdle width")
        || lower.contains("establish girdle")
    {
        return Some(MeetInstruction::ScaleReference);
    }
    Some(MeetInstruction::Other(text.to_string()))
}

/// Extracts the facet-name list from a `"Meet ..."` instruction's text (original case
/// preserved). Tolerant of both comma- and whitespace-separated lists, and strips
/// stray leading/trailing punctuation from each name.
///
/// Also drops lowercase English connector words ("Meet 2 and the culet" -> `["2",
/// "culet"]`): measured against the corpus, these were 17% of every unresolved name
/// token, and since a caller resolving a `Meet` instruction typically requires every
/// listed name to resolve, one spurious "and" was silently sinking the whole tier.
/// Case-sensitive and restricted to unambiguous, multi-letter connector words only --
/// lowercase single-letter facet names are common here, so "a"/"an" are never
/// filtered despite also being articles.
fn extract_meet_names(text: &str) -> Vec<String> {
    // `text` is known (by the caller) to start with an ASCII case-insensitive match
    // of "meet", which is 4 ASCII bytes, so this slice is always on a char boundary.
    let after = text.get(4..).unwrap_or("");
    after
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty() && !is_connector_word(s))
        .map(str::to_string)
        .collect()
}

/// Lowercase-only English connector words that appear in hand-typed `"Meet ..."`
/// prose but are never a facet name -- see [`extract_meet_names`]. Case-sensitive:
/// an uppercase `"And"`/`"THE"` never occurs in the sampled corpus's connector usage.
fn is_connector_word(token: &str) -> bool {
    matches!(token, "and" | "the" | "or")
}

/// A fully parsed `GemCAD` `.asc` cutting schedule.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscSchedule {
    /// The `GemCad` version string from the file's first line (e.g. "5.0", "4.51").
    pub gemcad_version: String,
    /// Index-wheel tooth count, as encoded in the file's `g` line. `GemCAD` sometimes
    /// encodes this as a negative number (an internal handedness/direction
    /// convention); use [`Self::gear_teeth_abs`] for the actual tooth count.
    pub gear_teeth: i32,
    /// The `g` line's second field: the index wheel's reference angle.
    pub gear_reference_angle: f64,
    /// Rotational symmetry order (the `y` line's first field).
    pub symmetry_order: u32,
    /// Mirror-symmetry flag (the `y` line's second field, `y`/`n`).
    pub mirror: bool,
    /// Refractive index (the `I` line).
    pub refractive_index: f64,
    /// Every `H` (header/title) line, in file order, with the leading `H` stripped.
    pub headers: Vec<String>,
    /// Every `F` (footnote) line, in file order, with the leading `F` stripped.
    pub footnotes: Vec<String>,
    /// Every facet tier, in file order.
    pub tiers: Vec<AscTier>,
}

impl AscSchedule {
    /// The index wheel's tooth count as an unsigned magnitude, for azimuth
    /// computations (`phi = 2*pi*index/gear_teeth_abs()`).
    #[must_use]
    pub const fn gear_teeth_abs(&self) -> u32 {
        self.gear_teeth.unsigned_abs()
    }

    /// Total number of individual facet planes this schedule describes (the sum of
    /// each tier's index count, or 1 for a tier with no explicit index).
    #[must_use]
    pub fn facet_plane_count(&self) -> usize {
        self.tiers.iter().map(|t| t.indices.len().max(1)).sum()
    }
}

/// Splits `content` into physical lines and reassembles `a` records that `GemCAD`
/// wrapped across multiple lines, returning the parsed schedule.
///
/// # Errors
///
/// Returns `Err` (as a human-readable message, matching the other lenient parsers in
/// this crate) if:
/// - `content` is empty or whitespace-only;
/// - a required header field (`g`/gear-teeth, `y`/symmetry, `I`/refractive-index) is
///   missing or has a non-numeric value where a number is required -- these are the
///   fields the rest of the crate depends on, so a file missing them is treated as
///   corrupt rather than silently defaulted;
/// - an `a` record's angle or mast field is missing or non-numeric (the two fields
///   this parser exists to extract reliably);
/// - the file contains no valid `a` records at all.
///
/// Everything else is handled leniently: unrecognized lines are ignored, a facet name
/// with no indices is kept as a single azimuth-0 tier (matching how table/culet rows
/// are commonly written), and the well-documented but rare `g` line missing its
/// leading keyword (seen once in the real corpus, as a bare `"96 0.0"`) is tolerated
/// as an implicit gear line.
#[expect(
    clippy::too_many_lines,
    reason = "one state machine over every record keyword; splitting it would scatter the parsing logic"
)]
pub fn parse_asc(content: &str) -> Result<AscSchedule, AscParseError> {
    if content.trim().is_empty() {
        return Err(AscParseError::EmptyInput);
    }

    let mut schedule = AscSchedule::default();
    let mut gear_teeth: Option<i32> = None;
    let mut symmetry_order: Option<u32> = None;
    let mut refractive_index: Option<f64> = None;
    let mut seen_any_tier = false;

    // Tokens accumulated for the 'a' record currently being assembled (possibly
    // across several continuation lines), plus the physical line number it started
    // on (for error messages).
    let mut pending: Option<(usize, Vec<String>)> = None;

    for (i, raw_line) in content.lines().enumerate() {
        let line_no = i + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        let mut tokens = line.split_whitespace();
        let Some(first) = tokens.next() else { continue };

        match first {
            "GemCad" | "GemCAD" | "Gemcad" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.gemcad_version = tokens.collect::<Vec<_>>().join(" ");
            }
            "g" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let rest: Vec<&str> = tokens.collect();
                if rest.len() < 2 {
                    return Err(AscParseError::GearLineMissingFields {
                        line: line_no,
                        text: line.to_string(),
                    });
                }
                let teeth: f64 =
                    rest[0]
                        .parse()
                        .map_err(|_| AscParseError::GearTeethNotNumeric {
                            line: line_no,
                            token: rest[0].to_string(),
                        })?;
                gear_teeth = Some(require_integral_i32(
                    teeth,
                    line_no,
                    "gear tooth count",
                    rest[0],
                )?);
                schedule.gear_reference_angle =
                    rest[1]
                        .parse()
                        .map_err(|_| AscParseError::GearReferenceAngleNotNumeric {
                            line: line_no,
                            token: rest[1].to_string(),
                        })?;
            }
            "y" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let rest: Vec<&str> = tokens.collect();
                if rest.len() < 2 {
                    return Err(AscParseError::SymmetryLineMissingFields {
                        line: line_no,
                        text: line.to_string(),
                    });
                }
                let order: f64 =
                    rest[0]
                        .parse()
                        .map_err(|_| AscParseError::SymmetryOrderNotNumeric {
                            line: line_no,
                            token: rest[0].to_string(),
                        })?;
                symmetry_order = Some(require_integral_u32(
                    order,
                    line_no,
                    "symmetry order",
                    rest[0],
                )?);
                schedule.mirror = rest[1].eq_ignore_ascii_case("y");
            }
            "I" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let val = tokens
                    .next()
                    .ok_or(AscParseError::RefractiveIndexMissing { line: line_no })?;
                refractive_index =
                    Some(
                        val.parse()
                            .map_err(|_| AscParseError::RefractiveIndexNotNumeric {
                                line: line_no,
                                token: val.to_string(),
                            })?,
                    );
            }
            "H" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.headers.push(line[1..].trim().to_string());
            }
            "F" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.footnotes.push(line[1..].trim().to_string());
            }
            "a" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                seen_any_tier = true;
                pending = Some((line_no, tokens.map(str::to_string).collect()));
            }
            _ => {
                if let Some((_, buf)) = pending.as_mut() {
                    // Continuation of the currently-open 'a' record.
                    buf.extend(line.split_whitespace().map(str::to_string));
                } else if gear_teeth.is_none() && !seen_any_tier {
                    // Tolerate the one real-world quirk seen in the corpus: a 'g' line
                    // that lost its leading keyword, e.g. "96 0.0" instead of
                    // "g 96 0.0". Only attempted before the first tier, and only for a
                    // line that looks exactly like a bare gear record.
                    let bare: Vec<&str> = line.split_whitespace().collect();
                    if bare.len() == 2
                        && let (Ok(teeth), Ok(refang)) =
                            (bare[0].parse::<f64>(), bare[1].parse::<f64>())
                    {
                        gear_teeth = Some(teeth as i32);
                        schedule.gear_reference_angle = refang;
                    }
                    // Otherwise: an unrecognized header-area line. Ignore leniently --
                    // decades of hand-edited files carry stray annotations.
                }
                // else: unrecognized line with no open tier and gear already known;
                // ignore leniently.
            }
        }
    }
    finalize_pending(&mut pending, &mut schedule.tiers)?;

    schedule.gear_teeth = gear_teeth.ok_or(AscParseError::MissingGearLine)?;
    schedule.symmetry_order = symmetry_order.ok_or(AscParseError::MissingSymmetryLine)?;
    schedule.refractive_index =
        refractive_index.ok_or(AscParseError::MissingRefractiveIndexLine)?;

    if schedule.tiers.is_empty() {
        return Err(AscParseError::NoTierRecords);
    }

    Ok(schedule)
}

/// Parses one accumulated `a` record's token buffer (everything after the leading
/// `a`, with continuation-line tokens already appended) into an [`AscTier`], and
/// pushes it onto `tiers`.
/// How far `value` may sit from its own nearest whole number and still count as
/// "integral" -- generous enough to absorb ordinary `f64` parse/arithmetic noise,
/// tight enough that anything a real `.asc` file would write on purpose (e.g. `96.5`)
/// still fails it.
const INTEGRAL_TOLERANCE: f64 = 1e-9;

/// Rejects `value` unless it is both integral (within [`INTEGRAL_TOLERANCE`]) and
/// small enough to fit an `i32` -- used for the `g` (gear teeth) header field, which
/// can legitimately be negative (an internal handedness convention), so this checks
/// magnitude, not sign. A plain `as i32` would silently truncate or saturate; this
/// instead reports the exact line and offending token.
fn require_integral_i32(
    value: f64,
    line_no: usize,
    field: &'static str,
    token: &str,
) -> Result<i32, AscParseError> {
    if (value - value.round()).abs() > INTEGRAL_TOLERANCE || value.abs() > f64::from(i32::MAX) {
        return Err(AscParseError::NotWholeNumber {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    Ok(value.round() as i32)
}

/// Same contract as [`require_integral_i32`], narrowed to `u32` instead -- used for
/// the `y` (symmetry order) header field, which (unlike gear teeth) has no negative
/// convention, so this also rejects negative values outright.
fn require_integral_u32(
    value: f64,
    line_no: usize,
    field: &'static str,
    token: &str,
) -> Result<u32, AscParseError> {
    if (value - value.round()).abs() > INTEGRAL_TOLERANCE
        || !(0.0..=f64::from(u32::MAX)).contains(&value)
    {
        return Err(AscParseError::NotWholeNumber {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    Ok(value.round() as u32)
}

fn finalize_pending(
    pending: &mut Option<(usize, Vec<String>)>,
    tiers: &mut Vec<AscTier>,
) -> Result<(), AscParseError> {
    let Some((line_no, tokens)) = pending.take() else {
        return Ok(());
    };
    tiers.push(parse_tier(line_no, &tokens)?);
    Ok(())
}

fn parse_tier(line_no: usize, tokens: &[String]) -> Result<AscTier, AscParseError> {
    if tokens.len() < 2 {
        return Err(AscParseError::TierRecordTooShort {
            line: line_no,
            field_count: tokens.len(),
        });
    }

    let angle_deg: f64 = tokens[0]
        .parse()
        .map_err(|_| AscParseError::AngleNotNumeric {
            line: line_no,
            token: tokens[0].clone(),
        })?;
    let mast: f64 = tokens[1]
        .parse()
        .map_err(|_| AscParseError::MastNotNumeric {
            line: line_no,
            token: tokens[1].clone(),
        })?;

    let mut indices = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut note_tokens: Vec<&str> = Vec::new();
    let mut in_notes = false;
    // Set right after consuming an "n" marker: GemCAD facet names can themselves be
    // plain numbers (e.g. "n 1", "n 4" -- see sample_4208.asc), so a token cannot be
    // classified as an index vs. a name by whether it parses as a number alone. The
    // token immediately following "n" is unconditionally the name; everything else
    // that parses as a number is an index.
    let mut expect_name = false;

    for tok in &tokens[2..] {
        if in_notes {
            note_tokens.push(tok);
            continue;
        }
        // Must run before the "n"/"G" marker checks below: GemCAD facet names can
        // themselves be exactly "G" (e.g. a girdle tier literally named "G" -- see
        // sample corpus files where "n G" is followed by indices or by another "n G"
        // group), so the token right after "n" is unconditionally consumed as the
        // name even when it reads like the notes marker.
        if expect_name {
            expect_name = false;
            if names.last().map(String::as_str) != Some(tok.as_str()) {
                // Repeated occurrences of the same name (common when a tier's indices
                // are split across more than one `n <name>` group) are folded
                // together rather than duplicated.
                names.push(tok.clone());
            }
            continue;
        }
        if tok == "n" {
            expect_name = true; // marker: the next token is a facet name, not an index
            continue;
        }
        if tok == "G" {
            in_notes = true;
            continue;
        }
        if let Ok(v) = tok.parse::<f64>() {
            indices.push(v);
        } else if names.last().map(String::as_str) != Some(tok.as_str()) {
            // A facet-name token that showed up without an "n" marker ahead of it.
            // Not seen in the sampled corpus, but tolerated for robustness.
            names.push(tok.clone());
        }
    }

    Ok(AscTier {
        angle_deg,
        mast,
        name: names.join("/"),
        indices,
        notes: note_tokens.join(" "),
    })
}

/// Serializes an [`AscSchedule`] back to `.asc` text.
///
/// Not byte-identical to hand-authored `GemCAD` output -- whitespace, the exact
/// token order within a tier line, and how repeated `n <name>` groups collapse back
/// down are all normalized -- but it round-trips semantically:
/// `parse_asc(&to_asc_string(s))` reproduces a schedule equal to `s`. Every numeric
/// field is written with `f64`'s/`i32`'s default `Display` formatting, which Rust
/// guarantees produces the shortest decimal string that reads back to the exact same
/// value, so no precision is lost across the round trip.
#[must_use]
pub fn to_asc_string(schedule: &AscSchedule) -> String {
    use fmt::Write as _;

    let mut out = String::new();

    let _ = writeln!(out, "GemCad {}", schedule.gemcad_version);
    let _ = writeln!(
        out,
        "g {} {}",
        schedule.gear_teeth, schedule.gear_reference_angle
    );
    let _ = writeln!(
        out,
        "y {} {}",
        schedule.symmetry_order,
        if schedule.mirror { "y" } else { "n" }
    );
    let _ = writeln!(out, "I {}", schedule.refractive_index);
    for header in &schedule.headers {
        let _ = writeln!(out, "H {header}");
    }

    for tier in &schedule.tiers {
        let _ = write!(out, "a {} {}", tier.angle_deg, tier.mast);
        for idx in &tier.indices {
            let _ = write!(out, " {idx}");
        }
        if !tier.name.is_empty() {
            let _ = write!(out, " n {}", tier.name);
        }
        if !tier.notes.is_empty() {
            let _ = write!(out, " G {}", tier.notes);
        }
        out.push('\n');
    }

    for footnote in &schedule.footnotes {
        let _ = writeln!(out, "F {footnote}");
    }

    out
}

/// Prepends a clear "this is derived, not authored" marker to `schedule`'s headers.
///
/// A reconstructed `.asc` (mast distances solved from angles and meet constraints,
/// not the original design's own measured masts) must never be mistaken for a
/// hand-authored, verified cutting schedule -- a user must be able to tell the two
/// apart before cutting a stone from either. Idempotent: calling it again on a
/// schedule that already carries a `RECONSTRUCTED` marker as its first header does
/// not add a second one.
///
/// `note` should say how the schedule was derived (e.g. which solver, and any caveat
/// about accuracy); it is appended after the marker on the same header line.
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str) {
    let already_marked = schedule
        .headers
        .first()
        .is_some_and(|h| h.starts_with("RECONSTRUCTED"));
    if already_marked {
        return;
    }
    schedule.headers.insert(
        0,
        format!("RECONSTRUCTED -- mast distances are solved, not original -- {note}"),
    );
}

impl fmt::Display for AscSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AscSchedule(gear={}, order={}, mirror={}, RI={}, tiers={})",
            self.gear_teeth,
            self.symmetry_order,
            self.mirror,
            self.refractive_index,
            self.tiers.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real file from the database (`pc46019.asc` / `attached_files` id 4210):
    /// "For Fun" by Michiko Huyhn, RI 1.54, gear 96, symmetry order 1 with mirror.
    const REAL_SAMPLE: &str = "GemCad 5.0\r\ng 96 0.0\r\ny 1 y\r\nI 1.54\r\nH PC 46.019  For Fun\r\nH by Michiko Huyhn\r\nH Gemology Online Faceting Design Competition 2013\r\nH Vindictive Entry 3\r\na -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\r\na -50.185680 0.48593919 71 n P2 25 G Cut to mast depth X.\r\na -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\r\na 0.000000 0.44755829 96 n U\r\nF Also USFG Newsletter Sep 2013, Facets Jan 2014\r\nF This diagram is in the public domain, and may be reproduced freely with full credit given to the \r\nF author.\r\n";

    #[test]
    fn parses_real_sample_header_fields() {
        let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
        assert_eq!(schedule.gemcad_version, "5.0");
        assert_eq!(schedule.gear_teeth, 96);
        assert_eq!(schedule.symmetry_order, 1);
        assert!(schedule.mirror);
        assert!((schedule.refractive_index - 1.54).abs() < 1e-9);
        assert_eq!(schedule.headers.len(), 4);
        assert_eq!(schedule.headers[0], "PC 46.019  For Fun");
        assert_eq!(schedule.footnotes.len(), 3);
    }

    #[test]
    fn parses_real_sample_tier_count_and_fields() {
        let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
        assert_eq!(schedule.tiers.len(), 4);

        let p1 = &schedule.tiers[0];
        assert!((p1.angle_deg - (-44.864_054)).abs() < 1e-6);
        assert!((p1.mast - 0.537_910_82).abs() < 1e-9);
        assert_eq!(p1.name, "P1");
        assert_eq!(p1.indices, vec![84.0, 12.0]);
        assert_eq!(p1.notes, "Cut to mast depth X.");
    }

    #[test]
    fn folds_repeated_name_groups_into_one_tier() {
        // The "-90 ... 36 12 84 n G1 60 n G1 ..." row: G1's indices are split across
        // two "n G1" groups on the same physical line. All four index positions must
        // land on the single resulting tier, not be split or deduplicated away.
        let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
        let g1 = &schedule.tiers[2];
        assert_eq!(g1.name, "G1");
        assert_eq!(g1.indices, vec![36.0, 12.0, 84.0, 60.0]);
        assert_eq!(g1.notes, "Set stone size.");
    }

    #[test]
    fn single_index_culet_like_tier_parses() {
        let schedule = parse_asc(REAL_SAMPLE).expect("real sample must parse");
        let u = &schedule.tiers[3];
        assert!((u.angle_deg - 0.0).abs() < 1e-9);
        assert_eq!(u.name, "U");
        assert_eq!(u.indices, vec![96.0]);
        assert_eq!(u.notes, "");
    }

    #[test]
    fn handles_continuation_lines_for_wrapped_index_lists() {
        // Verified real wrap pattern: a bare-number continuation line, then the next
        // record starts fresh on its own "a" line.
        let content = "GemCad 5.0\n\
                        g 96 0.0\n\
                        y 1 n\n\
                        I 1.72\n\
                        H Test\n\
                        a 90.000000 1.08976142 96 n G1 91 85 80 75 69 64 59 53 48 43 37 32 27 21 16\n\
                         11 5 G Or continuous girdle\n\
                        a -44.001549 0.51438487 96 n P1 85 75 64 53 43 32 21 11 G Cut to TCP\n";
        let schedule = parse_asc(content).expect("must parse continuation lines");
        assert_eq!(schedule.tiers.len(), 2);
        let g1 = &schedule.tiers[0];
        // 96 (gear ref) + 91..16 (15 more on the first line) + 11, 5 (continuation) = 18.
        assert_eq!(
            g1.indices.len(),
            18,
            "wrapped continuation indices must be folded into the same tier"
        );
        assert_eq!(g1.notes, "Or continuous girdle");
    }

    #[test]
    fn handles_n_name_continuation_line() {
        let content = "GemCad 5.0\n\
                        g 96 0.0\n\
                        y 1 n\n\
                        I 1.72\n\
                        H Test\n\
                        a -53.00 0.83825 93 87 81 75 69 63 57 51 45 39 33 27 21 15 9 3\n\
                         n 4\n\
                        a -48.00 0.82316 0 n 3 90 84 78 72 66 60 54 48 42 36 30 24 18 12 6\n";
        let schedule = parse_asc(content).expect("must parse 'n <name>' continuation");
        assert_eq!(schedule.tiers.len(), 2);
        assert_eq!(schedule.tiers[0].name, "4");
        assert_eq!(schedule.tiers[0].indices.len(), 16);
    }

    #[test]
    fn tolerates_missing_g_keyword_prefix() {
        // Real quirk seen once in the corpus (attached_files "Astryx Star" file): the
        // gear line lost its leading "g" and reads as a bare "96 0.0".
        let content = "GemCad 5.0\n\
                        96 0.0\n\
                        y 8 n\n\
                        I 1.54\n\
                        H Astryx Star\n\
                        a -42.800507 0.53960274 92 n P1 84 76 68 60 52 44 36 28 20 12 4 G Cut to centerpoint.\n";
        let schedule = parse_asc(content).expect("must tolerate a bare gear line");
        assert_eq!(schedule.gear_teeth, 96);
    }

    #[test]
    fn parses_fractional_index_positions() {
        let content = "GemCad 4.41\n\
                        g 96 48.0\n\
                        y 1 y\n\
                        I 1.54\n\
                        H Triolette Replica\n\
                        a -90.00 1.02050 88.8 7.2\n";
        let schedule = parse_asc(content).expect("must parse fractional indices");
        assert_eq!(schedule.tiers[0].indices, vec![88.8, 7.2]);
    }

    #[test]
    fn distinct_names_on_one_tier_are_merged_not_dropped() {
        // Real pattern: two different facet names sharing one angle/mast row, each
        // with its own index group. Geometrically this is still one tier (same
        // angle+depth), so both index groups must survive.
        let content = "GemCad 5.0\n\
                        g 96 0.0\n\
                        y 1 n\n\
                        I 1.72\n\
                        H Test\n\
                        a -40.00000 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle\n";
        let schedule = parse_asc(content).expect("must parse distinct names on one tier");
        assert_eq!(schedule.tiers.len(), 1);
        assert_eq!(schedule.tiers[0].name, "c/d");
        assert_eq!(
            schedule.tiers[0].indices,
            vec![92.0, 84.0, 76.0, 94.0, 90.0, 86.0]
        );
    }

    /// Real corpus tier line (girdle tier literally named "G"): `parse_tier`'s token
    /// loop used to check for the "G" notes marker before honoring a pending
    /// `expect_name` from the preceding "n" marker, so the name token "G" was misread
    /// as the start of the notes tail and every index after it was swallowed into
    /// `notes` instead. 2,296 of 5,759 corpus files hit this because "G" is a very
    /// common girdle-tier name.
    const ASC_GIRDLE_NAMED_G: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H Test\n\
a -90.000000 0.78956831 36 12 84 n G 60 n G G Set stone size.\n";

    #[test]
    fn tier_named_g_is_not_swallowed_by_notes_marker() {
        let schedule = parse_asc(ASC_GIRDLE_NAMED_G).expect("must parse a tier named \"G\"");
        assert_eq!(schedule.tiers.len(), 1);
        let tier = &schedule.tiers[0];
        assert_eq!(tier.name, "G");
        assert_eq!(tier.indices, vec![36.0, 12.0, 84.0, 60.0]);
        assert_eq!(tier.notes, "Set stone size.");
    }

    /// Same "G"-named-tier corpus quirk, but with no "G <notes>" tail at all -- every
    /// index after the name token must still parse as an index, not get swallowed as
    /// (empty) notes text.
    const ASC_GIRDLE_NAMED_G_NO_NOTES: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H Test\n\
a -90.000000 1.02 4 n G 12 20 28 36 44 52 60 68 76 84 92\n";

    #[test]
    fn tier_named_g_with_no_notes_keeps_all_indices() {
        let schedule = parse_asc(ASC_GIRDLE_NAMED_G_NO_NOTES)
            .expect("must parse a tier named \"G\" with no notes tail");
        assert_eq!(schedule.tiers.len(), 1);
        let tier = &schedule.tiers[0];
        assert_eq!(tier.name, "G");
        assert_eq!(tier.indices.len(), 12);
        assert_eq!(tier.notes, "");
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_asc("").is_err());
        assert!(parse_asc("   \n  \n").is_err());
    }

    #[test]
    fn rejects_truncated_file_with_no_tiers() {
        let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Truncated design\n";
        let err = parse_asc(content)
            .expect_err("a file with headers but no 'a' records must error")
            .to_string();
        assert!(err.contains("no valid"), "unexpected error message: {err}");
    }

    #[test]
    fn rejects_missing_mast_field() {
        let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000\n";
        let err = parse_asc(content)
            .expect_err("an 'a' record with no mast field must error")
            .to_string();
        assert!(
            err.contains("mast") || err.contains("field"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn rejects_garbage_angle_field() {
        let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na not-a-number 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a non-numeric angle must error")
            .to_string();
        assert!(err.contains("angle"), "unexpected error message: {err}");
    }

    #[test]
    fn rejects_missing_gear_line() {
        let content = "GemCad 5.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a missing 'g' line must error")
            .to_string();
        assert!(err.contains("gear"), "unexpected error message: {err}");
    }

    #[test]
    fn accepts_whole_number_gear_tooth_counts_written_as_plain_or_decimal() {
        let plain = "GemCad 5.0\ng 96 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        assert_eq!(
            parse_asc(plain)
                .expect("plain whole number must parse")
                .gear_teeth,
            96
        );

        let decimal = "GemCad 5.0\ng 96.0 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        assert_eq!(
            parse_asc(decimal)
                .expect("a decimal that is exactly whole must parse")
                .gear_teeth,
            96
        );
    }

    #[test]
    fn rejects_fractional_gear_tooth_count_with_the_line_number_and_token() {
        // Line 2 (1-indexed): the 'g' line itself.
        let content = "GemCad 5.0\ng 96.5 0.0\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a fractional gear tooth count must error")
            .to_string();
        assert!(
            err.contains("line 2"),
            "error must name the offending line: {err}"
        );
        assert!(
            err.contains("96.5"),
            "error must name the offending token: {err}"
        );
    }

    #[test]
    fn accepts_whole_number_symmetry_order_written_as_decimal() {
        let content = "GemCad 5.0\ng 96 0.0\ny 6.0 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        assert_eq!(
            parse_asc(content)
                .expect("a decimal symmetry order that is exactly whole must parse")
                .symmetry_order,
            6
        );
    }

    #[test]
    fn rejects_fractional_symmetry_order_with_the_line_number_and_token() {
        // Line 3 (1-indexed): the 'y' line itself.
        let content = "GemCad 5.0\ng 96 0.0\ny 6.5 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a fractional symmetry order must error")
            .to_string();
        assert!(
            err.contains("line 3"),
            "error must name the offending line: {err}"
        );
        assert!(
            err.contains("6.5"),
            "error must name the offending token: {err}"
        );
    }

    #[test]
    fn rejects_garbage_gear_reference_angle_with_the_line_number_and_token() {
        // Line 2 (1-indexed): the 'g' line itself. Previously this silently defaulted
        // to 0.0 via `unwrap_or`; it must now error like the other numeric fields.
        let content =
            "GemCad 5.0\ng 96 not-a-number\ny 1 y\nI 1.54\nH Test\na -41.000000 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a non-numeric gear reference angle must error")
            .to_string();
        assert!(
            err.contains("line 2"),
            "error must name the offending line: {err}"
        );
        assert!(
            err.contains("not-a-number"),
            "error must name the offending token: {err}"
        );
    }

    #[test]
    fn rejects_missing_refractive_index_line() {
        let content = "GemCad 5.0\ng 96 0.0\ny 1 y\nH Test\na -41.000000 0.5 92 n P1\n";
        let err = parse_asc(content)
            .expect_err("a missing 'I' line must error")
            .to_string();
        assert!(
            err.contains("refractive"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn does_not_panic_on_arbitrary_garbage() {
        // Fuzz-ish smoke test: a grab-bag of binary-ish / malformed content must never
        // panic, only ever return Ok or Err.
        let samples = [
            "\u{0}\u{1}\u{2}garbage\u{ff}",
            "a a a a a a a a\n",
            "GemCad\ng\ny\nI\n",
            "g 96 0.0\ny 1 y\nI abc\na 1 2\n",
        ];
        for s in samples {
            let _ = parse_asc(s);
        }
    }

    // -----------------------------------------------------------------------
    // `to_asc_string` round-trip tests, against real `.asc` files pulled verbatim
    // from `facet_diagrams.sqlite` (the same fixtures indicatrix's geometry tests use to
    // exercise `from_asc_schedule`'s sign conventions). `to_asc_string` is not
    // byte-identical to the original file, but `parse_asc(&to_asc_string(parse_asc(x)))`
    // must equal `parse_asc(x)` for every one of them.
    // -----------------------------------------------------------------------

    /// `attached_files` id 4208 ("pc45149.asc") -- "PC 45.149 Round Trichecker-12" by
    /// Fred W. Van Sant.
    const ASC_ROUND_TRICHECKER_12: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H PC 45.149  Round Trichecker-12\n\
H by Fred W. Van Sant, X 51, Extra Designs 2000\n\
H Released into the public domain in memory of Charles L. Moon\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60 52 44 36 28 20 12 4\n\
a -90.000000 1.07325092 92 n 2 84 76 68 60 52 44 36 28 20 12 4\n\
a 29.730000 0.65249790 4 n A 12 20 28 36 44 52 60 68 76 84 92\n\
a 25.000000 0.59508784 96 n B 16 32 48 64 80\n\
a 10.000000 0.48799664 96 n C 16 32 48 64 80\n\
F \"For small stones\"\n";

    /// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko
    /// Huyhn. Exercises an unsigned zero-angle culet-like tier ("U") with no explicit
    /// crown/pavilion marker, and several tiers with repeated `n <name>` groups.
    const ASC_FOR_FUN: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 1 y\n\
I 1.54\n\
H PC 46.019  For Fun\n\
H by Michiko Huyhn\n\
a -44.864054 0.53791082 84 n P1 12 G Cut to mast depth X.\n\
a -90.000000 0.78956831 36 12 84 n G1 60 n G1 G Set stone size.\n\
a 54.575729 0.70935195 12 n C1 84 G Set girdle width.\n\
a 0.000000 0.44755829 96 n U\n\
F Also USFG Newsletter Sep 2013, Facets Jan 2014\n";

    /// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
    /// Charles `McCoy`. Gear=80 (not the far more common 96), symmetry order 5, plus
    /// an explicit table tier at unsigned zero.
    const ASC_LARGE_TEXAS_STAR: &str = "GemCad 5.0\n\
g 80 0.0\n\
y 5 y\n\
I 1.61\n\
H PC 42.060  Large Texas Star\n\
H by Charles McCoy\n\
a -40.000000 0.54589773 76 n 1 68 60 52 44 36 28 20 12 4 G TCP\n\
a 40.000000 1.11585176 4 n A 12 20 28 36 44 52 60 68 76 G Establish girdle thickness\n\
a 0.000000 0.72641642 80 n T G Make table large enough to show all of the star\n\
F Leave #4 frosted\n";

    /// `attached_files` id 4422 ("pc43001a.asc") -- "PC 43.001A Shah (Replica)". No
    /// facet names anywhere (exercises the "no name at all" path), a rare
    /// negative-mast tier at an unsigned zero angle, and a fractional index (`1.7`).
    const ASC_SHAH_REPLICA_NO_NAMES: &str = "GemCad 4.51\n\
g 64 64.0\n\
y 1 n\n\
I 1.54\n\
H PC 43.001A Shah (Replica)\n\
a -90.00 1.00000 16\n\
a -90.00 0.44700 0 32\n\
a 0.00 -0.36800 0\n\
a 1.87 0.34210 49 47\n\
a 24.47 0.38860 1.7\n\
F Does not agree with Barbour's 43.001. Glass replica has rounded facets on the ends.\n";

    fn assert_round_trips(content: &str) {
        let original = parse_asc(content).expect("real sample must parse");
        let serialized = to_asc_string(&original);
        let reparsed = parse_asc(&serialized).unwrap_or_else(|e| {
            panic!("serialized output must itself parse: {e}\n--- serialized ---\n{serialized}")
        });
        assert_eq!(
            original, reparsed,
            "round trip must reproduce an equal AscSchedule\n--- serialized ---\n{serialized}"
        );
    }

    #[test]
    fn round_trips_real_sample() {
        assert_round_trips(REAL_SAMPLE);
    }

    #[test]
    fn round_trips_round_trichecker_12() {
        assert_round_trips(ASC_ROUND_TRICHECKER_12);
    }

    #[test]
    fn round_trips_for_fun() {
        assert_round_trips(ASC_FOR_FUN);
    }

    #[test]
    fn round_trips_large_texas_star() {
        assert_round_trips(ASC_LARGE_TEXAS_STAR);
    }

    #[test]
    fn round_trips_shah_replica_no_names() {
        assert_round_trips(ASC_SHAH_REPLICA_NO_NAMES);
    }

    #[test]
    fn round_trips_girdle_named_g() {
        assert_round_trips(ASC_GIRDLE_NAMED_G);
    }

    #[test]
    fn round_trips_girdle_named_g_no_notes() {
        assert_round_trips(ASC_GIRDLE_NAMED_G_NO_NOTES);
    }

    #[test]
    fn mark_reconstructed_prepends_marker_header_once() {
        let mut schedule = parse_asc(ASC_FOR_FUN).expect("real sample must parse");
        let original_header_count = schedule.headers.len();

        mark_reconstructed(&mut schedule, "solved via MeetPointSolver");
        assert_eq!(schedule.headers.len(), original_header_count + 1);
        assert!(schedule.headers[0].starts_with("RECONSTRUCTED"));
        assert!(schedule.headers[0].contains("solved via MeetPointSolver"));

        // Calling it again must not stack a second marker.
        mark_reconstructed(&mut schedule, "a different note");
        assert_eq!(schedule.headers.len(), original_header_count + 1);
    }

    #[test]
    fn parses_meet_instruction_with_comma_separated_names() {
        let schedule = parse_asc(
            "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.72\nH Test\n\
             a -90.000000 0.58736554 69 n G2 27 G Meet P1, P2, G1\n",
        )
        .expect("must parse");
        let instr = schedule.tiers[0]
            .meet_instruction()
            .expect("must have a G note");
        assert_eq!(
            instr,
            MeetInstruction::Meet(vec!["P1".to_string(), "P2".to_string(), "G1".to_string()])
        );
    }

    #[test]
    fn parses_meet_instruction_with_whitespace_separated_names() {
        let instr = parse_meet_instruction("Meet P2 P3 P5").expect("must parse");
        assert_eq!(
            instr,
            MeetInstruction::Meet(vec!["P2".to_string(), "P3".to_string(), "P5".to_string()])
        );
    }

    /// Real corpus prose drops connector words that were never facet names -- see
    /// `extract_meet_names`'s doc comment.
    #[test]
    fn parses_meet_instruction_drops_connector_words() {
        assert_eq!(
            parse_meet_instruction("Meet 2 and the culet"),
            Some(MeetInstruction::Meet(vec![
                "2".to_string(),
                "culet".to_string()
            ]))
        );
        assert_eq!(
            parse_meet_instruction("Meet P1 or P2"),
            Some(MeetInstruction::Meet(vec![
                "P1".to_string(),
                "P2".to_string()
            ]))
        );
        // Lowercase single-letter facet names are common in this corpus and must
        // never be dropped, even though "a"/"an" are also articles.
        assert_eq!(
            parse_meet_instruction("Meet a and b"),
            Some(MeetInstruction::Meet(vec![
                "a".to_string(),
                "b".to_string()
            ]))
        );
    }

    #[test]
    fn parses_meet_instruction_with_four_named_facets() {
        let instr = parse_meet_instruction("Meet G1, G2, C1, C2").expect("must parse");
        assert_eq!(
            instr,
            MeetInstruction::Meet(vec![
                "G1".to_string(),
                "G2".to_string(),
                "C1".to_string(),
                "C2".to_string()
            ])
        );
    }

    #[test]
    fn classifies_real_corpus_notes_text() {
        // Every one of these is verbatim (or near-verbatim) text seen in the real
        // `.asc` corpus fixtures above.
        assert_eq!(
            parse_meet_instruction("Cut to mast depth X."),
            Some(MeetInstruction::Other("Cut to mast depth X.".to_string()))
        );
        assert_eq!(
            parse_meet_instruction("Set stone size."),
            Some(MeetInstruction::ScaleReference)
        );
        assert_eq!(
            parse_meet_instruction("Set girdle width."),
            Some(MeetInstruction::ScaleReference)
        );
        assert_eq!(
            parse_meet_instruction("Establish girdle thickness"),
            Some(MeetInstruction::ScaleReference)
        );
        assert_eq!(
            parse_meet_instruction("TCP"),
            Some(MeetInstruction::CutToCenterpoint)
        );
        assert_eq!(
            parse_meet_instruction("Cut to centerpoint."),
            Some(MeetInstruction::CutToCenterpoint)
        );
        assert_eq!(
            parse_meet_instruction("Level girdle."),
            Some(MeetInstruction::LevelGirdle)
        );
        assert_eq!(
            parse_meet_instruction("GMP"),
            Some(MeetInstruction::GirdleMeetPoint)
        );
        assert_eq!(
            parse_meet_instruction("Meet girdle"),
            Some(MeetInstruction::Meet(vec!["girdle".to_string()]))
        );
        assert_eq!(
            parse_meet_instruction("Or continuous girdle"),
            Some(MeetInstruction::Other("Or continuous girdle".to_string()))
        );
        assert_eq!(parse_meet_instruction(""), None);
    }

    #[test]
    fn tier_names_splits_joined_names() {
        let schedule = parse_asc(
            "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.72\nH Test\n\
             a -40.00000 0.49897 92 n c 84 76 94 n d 90 86 G Meet girdle\n",
        )
        .expect("must parse");
        assert_eq!(schedule.tiers[0].names(), vec!["c", "d"]);
    }

    #[test]
    fn round_trips_fractional_indices_and_wrapped_continuation() {
        // A hand-assembled but format-faithful case combining fractional indices with
        // a wrapped continuation line, to make sure to_asc_string's single-line-per-tier
        // output still round-trips even though the source used a continuation.
        let content = "GemCad 5.0\n\
                        g 96 0.0\n\
                        y 1 n\n\
                        I 1.72\n\
                        H Test\n\
                        a 90.000000 1.08976142 96 n G1 91 85 80 75 69 64 59 53 48 43 37 32 27 21 16\n\
                         11 5 G Or continuous girdle\n\
                        a -90.00 1.02050 88.8 7.2\n";
        assert_round_trips(content);
    }
}

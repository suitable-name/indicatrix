//! The raw text view of a design's cutting instructions ("Edit as Text").
//!
//! It holds the design as the `.asc` text Export writes, a parser that tells the cutter
//! which line is wrong, a merge that applies an edited text back to the design in one undo
//! step, and a plain line diff for comparing two texts.
//!
//! - [`generate_text`] writes exactly what Export .asc writes (the effective refractive
//!   index, the solved masts, concave tiers as footnotes), with line feeds only, so the
//!   text box never has to guess a line ending. [`omitted_line`] says in one sentence what
//!   that text cannot carry.
//! - [`parse_text`] reads text back and reports a problem with its line number
//!   ([`TextProblem`]).
//! - [`plan_apply`] compares the edited text with the text the design produced and builds
//!   an [`ApplyPlan`]: the new schedule state for `Edit::ReplaceSchedule` plus an
//!   [`ApplyReport`] listing what changes and what is lost. A line that was not touched
//!   leaves the design data behind it exactly as it was, so a tier keeps its id, note,
//!   cheater offset, target, relation and meet rule unless the text changes something that
//!   contradicts them. [`apply_plan`] applies it through the session.
//! - [`diff_lines`] compares two texts line by line (longest common subsequence).
//!
//! The text carries no tier ids, so tiers are matched by name (then by angle and indices
//! for a renamed line, then by position when the tier count is unchanged). This module is
//! pure: no toolkit, no file access, no threads.

use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::{ConstraintTier, Design};
use indicatrix_formats::asc::{
    AscLineEnding, AscParseError, AscSchedule, parse_asc, parse_asc_with_gear_line, to_asc_string,
};
use std::fmt;

mod diff;
mod pairing;
mod plan;
#[cfg(test)]
mod tests;

pub use diff::{DiffKind, DiffRow, TextDiff, diff_lines};
pub use plan::{ApplyPlan, ApplyReport, apply_plan, plan_apply};

/// The most names a one-line summary lists before it says "and N more".
const MAX_LISTED_NAMES: usize = 4;

// --- problems ----------------------------------------------------------------------------

/// Something wrong with the text, with the line it is on when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextProblem {
    /// The 1-based line the problem is on; `None` for a problem with the text as a whole
    /// (nothing in it, no gear line).
    pub line: Option<usize>,
    /// What is wrong, as a sentence without the line number.
    pub message: String,
}

impl TextProblem {
    /// A problem on `line`.
    fn at(line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }

    /// A problem with the text as a whole.
    fn general(message: impl Into<String>) -> Self {
        Self::at(None, message)
    }
}

impl fmt::Display for TextProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "Line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for TextProblem {}

/// The line an `.asc` parse error is on, when it names one.
const fn error_line(error: &AscParseError) -> Option<usize> {
    match error {
        AscParseError::GearLineMissingFields { line, .. }
        | AscParseError::GearTeethNotNumeric { line, .. }
        | AscParseError::GearReferenceAngleNotNumeric { line, .. }
        | AscParseError::SymmetryLineMissingFields { line, .. }
        | AscParseError::SymmetryOrderNotNumeric { line, .. }
        | AscParseError::RefractiveIndexMissing { line }
        | AscParseError::RefractiveIndexNotNumeric { line, .. }
        | AscParseError::NotWholeNumber { line, .. }
        | AscParseError::GearTeethZero { line }
        | AscParseError::GearTeethTooLarge { line, .. }
        | AscParseError::MirrorFlagInvalid { line, .. }
        | AscParseError::TierRecordTooShort { line, .. }
        | AscParseError::AngleNotNumeric { line, .. }
        | AscParseError::MastNotNumeric { line, .. }
        | AscParseError::NonFiniteValue { line, .. }
        | AscParseError::SymmetryOrderZero { line }
        | AscParseError::RefractiveIndexOutOfRange { line, .. } => Some(*line),
        // The whole-text errors have no line of their own.
        _ => None,
    }
}

/// `text` with its first letter in upper case.
fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A parse error as a [`TextProblem`]: the parser's message without its "line N:" prefix
/// (the line goes in [`TextProblem::line`]), and a plain sentence for the errors that name
/// no line.
fn problem_from(error: &AscParseError) -> TextProblem {
    let line = error_line(error);
    let message = match error {
        AscParseError::EmptyInput => "The text is empty.".to_owned(),
        AscParseError::MissingGearLine => {
            "There is no gear line. Add one such as: g 96 0.0".to_owned()
        }
        AscParseError::MissingSymmetryLine => {
            "There is no symmetry line. Add one such as: y 8 y".to_owned()
        }
        AscParseError::MissingRefractiveIndexLine => {
            "There is no refractive index line. Add one such as: I 1.54".to_owned()
        }
        AscParseError::NoTierRecords => {
            "There are no facet tiers. Every tier is a line that starts with a.".to_owned()
        }
        other => {
            let full = other.to_string();
            let prefix = line.map(|number| format!("line {number}: "));
            let stripped = prefix
                .as_deref()
                .and_then(|prefix| full.strip_prefix(prefix))
                .unwrap_or(&full);
            capitalise(stripped)
        }
    };
    TextProblem::at(line, message)
}

// --- parsing -----------------------------------------------------------------------------

/// Text read back into a schedule, with the line each tier is on.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedText {
    /// The parsed schedule, parse notes included in [`AscSchedule::warnings`].
    pub schedule: AscSchedule,
    /// The 1-based line each tier starts on, in tier order.
    pub tier_lines: Vec<usize>,
    /// The 1-based line of the gear (`g`) record the parser kept: the last one when the
    /// text has several. It comes from the parser itself, so it is the line the parser read
    /// the gear from, whatever spelling that line has.
    pub gear_line: Option<usize>,
}

impl ParsedText {
    /// The line tier `index` starts on, when the parser and the line scan agree on the
    /// tier count (they always do for text the parser accepts).
    #[must_use]
    pub fn tier_line(&self, index: usize) -> Option<usize> {
        self.tier_lines.get(index).copied()
    }
}

/// The 1-based lines that start a facet tier, found the way the `.asc` reader finds them:
/// a line whose first word is `a`, ignoring blank and comment lines.
fn tier_line_numbers(text: &str) -> Vec<usize> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .enumerate()
        .filter(|(_, raw)| {
            let line = raw.trim();
            !(line.is_empty() || line.starts_with(';') || line.starts_with('#'))
                && line.split_whitespace().next() == Some("a")
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// Reads `text` as `.asc` cutting instructions.
///
/// # Errors
///
/// A [`TextProblem`] naming the line when the text cannot be read: a missing or
/// non-numeric field, a zero gear or symmetry order, a refractive index of 1 or less, a
/// number that is not finite, or no gear, symmetry, refractive index or tier line at all.
pub fn parse_text(text: &str) -> Result<ParsedText, TextProblem> {
    let (schedule, gear_line) =
        parse_asc_with_gear_line(text).map_err(|error| problem_from(&error))?;
    Ok(ParsedText {
        schedule,
        tier_lines: tier_line_numbers(text),
        gear_line,
    })
}

/// `text` read and written again with line feeds: the same instructions in the text view's
/// own layout, so a file written by another program compares line for line against the
/// generated text.
///
/// # Errors
///
/// A [`TextProblem`] when the text cannot be read or written back.
pub fn normalize_text(text: &str) -> Result<String, TextProblem> {
    let mut schedule = parse_asc(text).map_err(|error| problem_from(&error))?;
    schedule.line_ending = AscLineEnding::Lf;
    schedule.warnings.clear();
    to_asc_string(&schedule).map_err(|error| TextProblem::general(error.to_string()))
}

// --- generating --------------------------------------------------------------------------

/// The design as the text Export .asc writes.
///
/// That is the effective refractive index (with `custom` catalogue materials taken into
/// account), the already-solved masts, and the concave tiers as footnotes. Line feeds only.
///
/// # Errors
///
/// A sentence when `solved` does not have one mast per tier, or when a header, footnote or
/// note holds a line break or a number that cannot be written.
pub fn generate_text(
    design: &Design,
    solved: &[SolvedTier],
    custom: &[GemMaterial],
) -> Result<String, String> {
    let mut schedule = design
        .try_to_asc_schedule_from_solved_with(solved, custom)
        .map_err(|error| error.to_string())?;
    design.append_concave_footnotes(&mut schedule);
    schedule.line_ending = AscLineEnding::Lf;
    to_asc_string(&schedule).map_err(|error| error.to_string())
}

/// `count` followed by `noun`, pluralised with an "s".
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// What the text cannot carry for `design`, one phrase each. The preform, the material
/// and the girdle size are always among them.
#[must_use]
pub fn omitted_items(design: &Design) -> Vec<String> {
    let mut items = Vec::new();
    let notes = design
        .tier_notes
        .values()
        .filter(|note| !note.trim().is_empty())
        .count();
    if notes > 0 {
        items.push(counted(notes, "tier note"));
    }
    let offsets = design
        .cheater_offsets_deg
        .values()
        .filter(|offset| **offset != 0.0)
        .count();
    if offsets > 0 {
        items.push(counted(offsets, "cheater offset"));
    }
    if !design.tier_targets.is_empty() {
        items.push(format!(
            "{} (only the cut depths they give are written)",
            counted(design.tier_targets.len(), "depth or width target")
        ));
    }
    if !design.tier_relations.is_empty() {
        items.push(format!(
            "{} (only the angles they give are written)",
            counted(design.tier_relations.len(), "tier relation")
        ));
    }
    if !design.concave_tiers.is_empty() {
        items.push(format!(
            "{} (written as footnotes only)",
            counted(design.concave_tiers.len(), "concave tier")
        ));
    }
    items.push("the preform, the material and the girdle size".to_owned());
    items
}

/// One sentence saying what the text leaves out, for the line under the text box.
#[must_use]
pub fn omitted_line(design: &Design) -> String {
    format!(
        "Not in this text: {}. Tiers you keep by name keep these.",
        omitted_items(design).join(", ")
    )
}

// --- comparing numbers and words ----------------------------------------------------------

/// Whether two numbers are the same value, sign of zero included (a culet is a negative
/// zero angle and a table a positive one).
const fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

/// Whether two number lists are equal, number for number.
fn same_all(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(*x, *y))
}

/// Up to [`MAX_LISTED_NAMES`] of `names` joined with commas, then "and N more".
fn list(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(MAX_LISTED_NAMES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().saturating_sub(MAX_LISTED_NAMES) {
        0 => shown,
        rest => format!("{shown} and {rest} more"),
    }
}

/// `prefix` and the listed names as one sentence; `None` for no names.
fn sentence(prefix: &str, names: &[String]) -> Option<String> {
    (!names.is_empty()).then(|| format!("{prefix} {}.", list(names)))
}

/// How a message names a tier: its name, or its place in the list when it has none.
fn tier_label(tier: &ConstraintTier, position: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", position + 1)
    } else {
        tier.name.clone()
    }
}

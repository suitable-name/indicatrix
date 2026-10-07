//! The text comparison of two designs: the cutting instructions of each written as `.asc`
//! text (exactly what Export writes) and compared line by line.
//!
//! No Slint types, so the tests cover it directly.

use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use indicatrix_editor::raw_text::{DiffKind, TextDiff, diff_lines, generate_text};

/// Unchanged lines kept on each side of a change; longer runs are folded away.
const CONTEXT_LINES: usize = 3;

/// What the text leaves out, said once under every comparison.
const SCOPE_NOTE: &str = "The text holds the cutting instructions only. A different rough, girdle size or material shows in the pictures, not here.";

/// One line of the comparison as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffLine {
    /// `VariantDiffRow.kind`: 0 in both, 1 added, 2 removed, 3 folded unchanged lines.
    pub(super) kind: i32,
    /// The line number in the first design's text; empty for an added line.
    pub(super) old_line: String,
    /// The line number in the second design's text; empty for a removed line.
    pub(super) new_line: String,
    /// The line, or for a folded run how many lines it holds.
    pub(super) text: String,
}

/// The whole comparison view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffView {
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) rows: Vec<DiffLine>,
}

/// `VariantDiffRow.kind` of a row.
const fn kind_code(kind: DiffKind) -> i32 {
    match kind {
        DiffKind::Same => 0,
        DiffKind::Added => 1,
        DiffKind::Removed => 2,
        DiffKind::Skipped => 3,
    }
}

/// A line number as shown; empty where the line is not in that text.
fn number(line: Option<usize>) -> String {
    line.map_or_else(String::new, |line| line.to_string())
}

/// The heading over a comparison of `first` and `second`.
pub(super) fn title(first: &str, second: &str) -> String {
    format!("Cutting instructions: \"{first}\" and \"{second}\"")
}

/// The comparison of the text of `first` (the old side) with the text of `second`.
pub(super) fn diff_view(first: &str, second: &str, diff: &TextDiff) -> DiffView {
    let rows = if diff.is_identical() {
        diff.rows.clone()
    } else {
        diff.collapsed(CONTEXT_LINES)
    };
    let summary = if diff.is_identical() {
        format!("The cutting instructions are the same. {SCOPE_NOTE}")
    } else {
        format!(
            "{} Added lines are only in \"{second}\", removed lines only in \"{first}\". {SCOPE_NOTE}",
            diff.summary()
        )
    };
    DiffView {
        title: title(first, second),
        summary,
        rows: rows
            .into_iter()
            .map(|row| DiffLine {
                kind: kind_code(row.kind),
                old_line: number(row.old_line),
                new_line: number(row.new_line),
                text: row.text,
            })
            .collect(),
    }
}

/// The view shown instead of a comparison that could not be made.
pub(super) fn failure_view(first: &str, second: &str, reason: &str) -> DiffView {
    DiffView {
        title: title(first, second),
        summary: reason.to_owned(),
        rows: Vec::new(),
    }
}

/// The `.asc` text of `design`, solving it first.
///
/// # Errors
///
/// A plain sentence naming `label` when the design does not solve or cannot be written as
/// text.
pub(super) fn text_of(
    design: &Design,
    label: &str,
    custom: &[GemMaterial],
) -> Result<String, String> {
    let solved: Vec<SolvedTier> = design.solve().map_err(|error| {
        format!("\"{label}\" does not solve, so its text cannot be written. {error}")
    })?;
    generate_text(design, &solved, custom)
        .map_err(|error| format!("The text of \"{label}\" cannot be written. {error}"))
}

/// The comparison of two designs, worked out on a worker thread: both are solved, written
/// as text and compared. A design that cannot be written gives a view that says so.
pub(super) fn compare_designs(
    first: (&Design, &str),
    second: (&Design, &str),
    custom: &[GemMaterial],
) -> DiffView {
    let texts = text_of(first.0, first.1, custom)
        .and_then(|old| text_of(second.0, second.1, custom).map(|new| (old, new)));
    match texts {
        Ok((old, new)) => diff_view(first.1, second.1, &diff_lines(&old, &new)),
        Err(reason) => failure_view(first.1, second.1, &reason),
    }
}

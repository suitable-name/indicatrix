//! A plain line-by-line comparison of two texts.

use super::counted;

/// The largest line-by-line comparison table [`diff_lines`] builds (old lines times new
/// lines, after the shared start and end are cut away). A larger pair is shown as one
/// removed block and one added block instead of being compared line by line.
const MAX_DIFF_CELLS: usize = 4_000_000;

/// What happened to one line between two texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// The line is in both texts.
    Same,
    /// The line is only in the new text.
    Added,
    /// The line is only in the old text.
    Removed,
    /// A run of unchanged lines folded away by [`TextDiff::collapsed`]; the row's text says
    /// how many.
    Skipped,
}

/// One row of a line diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    /// What happened to the line.
    pub kind: DiffKind,
    /// The 1-based line number in the old text; `None` for an added line.
    pub old_line: Option<usize>,
    /// The 1-based line number in the new text; `None` for a removed line.
    pub new_line: Option<usize>,
    /// The line itself (for a skipped row, "12 unchanged lines").
    pub text: String,
}

/// A line diff of two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    /// Every line of both texts, in order, as [`DiffKind::Same`], [`DiffKind::Removed`] or
    /// [`DiffKind::Added`].
    pub rows: Vec<DiffRow>,
    /// How many lines the new text adds.
    pub added: usize,
    /// How many lines the new text removes.
    pub removed: usize,
    /// How many lines are the same.
    pub same: usize,
}

impl TextDiff {
    /// Whether the two texts have the same lines.
    #[must_use]
    pub const fn is_identical(&self) -> bool {
        self.added == 0 && self.removed == 0
    }

    /// The counts as one sentence.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.is_identical() {
            "The two texts are identical.".to_owned()
        } else {
            format!(
                "{} added, {} removed, {} unchanged.",
                counted(self.added, "line"),
                counted(self.removed, "line"),
                counted(self.same, "line"),
            )
        }
    }

    /// The rows with every run of unchanged lines more than `context` lines away from a
    /// change folded into one [`DiffKind::Skipped`] row.
    #[must_use]
    pub fn collapsed(&self, context: usize) -> Vec<DiffRow> {
        let mut keep = vec![false; self.rows.len()];
        for (position, row) in self.rows.iter().enumerate() {
            if row.kind == DiffKind::Same {
                continue;
            }
            let from = position.saturating_sub(context);
            let to = (position + context).min(self.rows.len() - 1);
            keep[from..=to].fill(true);
        }
        let mut out = Vec::new();
        let mut skipped = 0_usize;
        let flush = |out: &mut Vec<DiffRow>, skipped: &mut usize| {
            if *skipped > 0 {
                out.push(DiffRow {
                    kind: DiffKind::Skipped,
                    old_line: None,
                    new_line: None,
                    text: counted(*skipped, "unchanged line"),
                });
                *skipped = 0;
            }
        };
        for (row, kept) in self.rows.iter().zip(&keep) {
            if *kept {
                flush(&mut out, &mut skipped);
                out.push(row.clone());
            } else {
                skipped += 1;
            }
        }
        flush(&mut out, &mut skipped);
        out
    }
}

/// Builds diff rows with running line numbers.
struct RowBuilder {
    /// The rows so far.
    rows: Vec<DiffRow>,
    /// The next old line number.
    old_next: usize,
    /// The next new line number.
    new_next: usize,
}

impl RowBuilder {
    /// A line in both texts.
    fn same(&mut self, text: &str) {
        self.rows.push(DiffRow {
            kind: DiffKind::Same,
            old_line: Some(self.old_next),
            new_line: Some(self.new_next),
            text: text.to_owned(),
        });
        self.old_next += 1;
        self.new_next += 1;
    }

    /// A line only in the old text.
    fn removed(&mut self, text: &str) {
        self.rows.push(DiffRow {
            kind: DiffKind::Removed,
            old_line: Some(self.old_next),
            new_line: None,
            text: text.to_owned(),
        });
        self.old_next += 1;
    }

    /// A line only in the new text.
    fn added(&mut self, text: &str) {
        self.rows.push(DiffRow {
            kind: DiffKind::Added,
            old_line: None,
            new_line: Some(self.new_next),
            text: text.to_owned(),
        });
        self.new_next += 1;
    }
}

/// Compares the lines between the shared start and end of two texts with the longest common
/// subsequence, preferring to list a removed line before the added line that replaces it.
fn diff_middle(old: &[&str], new: &[&str], rows: &mut RowBuilder) {
    if (old.len() + 1).saturating_mul(new.len() + 1) > MAX_DIFF_CELLS {
        for text in old {
            rows.removed(text);
        }
        for text in new {
            rows.added(text);
        }
        return;
    }
    // table[i][j]: the length of the longest common subsequence of old[i..] and new[j..].
    let width = new.len() + 1;
    let mut table = vec![0_u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            rows.same(old[i]);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            rows.removed(old[i]);
            i += 1;
        } else {
            rows.added(new[j]);
            j += 1;
        }
    }
    for text in &old[i..] {
        rows.removed(text);
    }
    for text in &new[j..] {
        rows.added(text);
    }
}

/// Compares two texts line by line.
///
/// The lines are compared exactly (a changed space at the end of a line is a change). The
/// shared start and end are matched first, then the lines between them with the longest
/// common subsequence.
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> TextDiff {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let room = old_lines.len().min(new_lines.len()) - prefix;
    let suffix = old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let mut builder = RowBuilder {
        rows: Vec::with_capacity(old_lines.len().max(new_lines.len())),
        old_next: 1,
        new_next: 1,
    };
    for text in &old_lines[..prefix] {
        builder.same(text);
    }
    diff_middle(
        &old_lines[prefix..old_lines.len() - suffix],
        &new_lines[prefix..new_lines.len() - suffix],
        &mut builder,
    );
    for text in &old_lines[old_lines.len() - suffix..] {
        builder.same(text);
    }
    let count = |kind: DiffKind| builder.rows.iter().filter(|row| row.kind == kind).count();
    TextDiff {
        added: count(DiffKind::Added),
        removed: count(DiffKind::Removed),
        same: count(DiffKind::Same),
        rows: builder.rows,
    }
}

//! [`SheetDetails`]: the words a printed cutting sheet carries that the geometry does not --
//! title, subtitle, author, date, shape, comments and the two optional ranges.
//!
//! Everything here is plain data. Nothing reads a clock or a file: the caller (the desktop,
//! the command line) decides what the export date is and where the rest comes from.

use indicatrix_cut_core::{Design, native::DesignMetadata};

/// What a cutting sheet prints besides the design's own geometry.
///
/// An empty text field, an empty comment list and a `None` range are simply not printed;
/// nothing is invented to fill a gap. [`Self::from_design`] is the fallback every caller
/// starts from, and [`Self::from_metadata`] layers a native file's own details over it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SheetDetails {
    /// The design's title, the sheet's heading.
    pub title: String,
    /// A suite or subtitle, printed under the title.
    pub subtitle: String,
    /// The designer, printed as "by ...".
    pub author: String,
    /// The export date as the caller wants it written (`October 2026`).
    pub date_text: String,
    /// The shape (`Round`, `Oval`), in Design Data.
    pub shape: String,
    /// Free-text comment lines, printed in their own block.
    pub comments: Vec<String>,
    /// The refractive-index range the design is meant for, low to high.
    pub ri_range: Option<(f64, f64)>,
    /// The size range the design suits, in millimetres, small to large.
    pub size_range_mm: Option<(f64, f64)>,
}

/// The text after a leading `by ` (any case) of `line`, trimmed; `None` for any other line or
/// an empty name.
fn author_of(line: &str) -> Option<&str> {
    let line = line.trim();
    let head = line.get(..3)?;
    if !head.eq_ignore_ascii_case("by ") {
        return None;
    }
    let name = line[3..].trim();
    (!name.is_empty()).then_some(name)
}

impl SheetDetails {
    /// The details a design carries on its own, from its `.asc` header and footnote lines.
    ///
    /// The title is the first non-blank header line that is not a "by ..." line, the author is
    /// the first header line starting with "by " (what follows it), and the comments are the
    /// non-blank footnote lines. Subtitle, date, shape and both ranges stay empty: nothing is
    /// invented.
    #[must_use]
    pub fn from_design(design: &Design) -> Self {
        let headers = &design.meta.headers;
        let title = headers
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty() && author_of(line).is_none())
            .unwrap_or_default()
            .to_string();
        let author = headers
            .iter()
            .find_map(|line| author_of(line))
            .unwrap_or_default()
            .to_string();
        let comments = design
            .meta
            .footnotes
            .iter()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect();
        Self {
            title,
            author,
            comments,
            ..Self::default()
        }
    }

    /// [`Self::from_design`] with a native design file's own details laid over it: the file's
    /// title, designer and shape replace the fallback when they are not empty, the file's notes
    /// are appended to the comments (one comment per non-blank line), and `date_text` is set.
    #[must_use]
    pub fn from_metadata(design: &Design, metadata: &DesignMetadata, date_text: &str) -> Self {
        let mut details = Self::from_design(design);
        let title = metadata.title.trim();
        if !title.is_empty() {
            details.title = title.to_string();
        }
        let designer = metadata.designer.trim();
        if !designer.is_empty() {
            details.author = designer.to_string();
        }
        let shape = metadata.shape.trim();
        if !shape.is_empty() {
            details.shape = shape.to_string();
        }
        details.comments.extend(
            metadata
                .notes
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string),
        );
        details.date_text = date_text.trim().to_string();
        details
    }
}

/// The English month name for `month` (1 to 12), `None` outside that range.
const fn month_name(month: u32) -> Option<&'static str> {
    Some(match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => return None,
    })
}

/// `"October 2026"` for `(year, month)`; an empty string when `month` is not 1 to 12.
///
/// The caller supplies the numbers (from its own clock), so this stays pure.
#[must_use]
pub fn month_year_text(year: i64, month: u32) -> String {
    month_name(month).map_or_else(String::new, |name| format!("{name} {year}"))
}

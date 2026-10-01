//! Which attachment of a catalogue design carries its cutting instructions, and
//! reading it as `.asc` text.
//!
//! One rule for every reader of a stored design (the desktop editor's record
//! loader, the library detail view, the worker's `FetchDesignSource`): the first
//! `.asc` attachment, else the first `.gem` (`GemCAD`'s binary save file), else the
//! first `.gcs` (Gem Cut Studio). A `.gem`/`.gcs` is parsed and converted with
//! `indicatrix_formats::gem`/`gcs` and written as `.asc` text with
//! `indicatrix_formats::asc::to_asc_string`, so everything downstream keeps
//! consuming `.asc` cutting instructions.
//!
//! The functions here do no panic isolation of their own; callers wrap
//! [`design_file_to_asc_text`] in the same guard they use around `.asc` parsing.

use indicatrix_formats::{
    asc::{AscSchedule, decode_asc_bytes, to_asc_string},
    gcs::{gcs_to_asc_schedule, parse_gcs_bytes},
    gem::{gem_to_asc_schedule, parse_gem},
};
use std::path::Path;

/// A design-file attachment kind, by file extension (case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignFileKind {
    /// `.asc` cutting instructions.
    Asc,
    /// `GemCAD`'s binary `.gem` save file.
    Gem,
    /// Gem Cut Studio's XML `.gcs` file.
    Gcs,
}

impl DesignFileKind {
    /// The kind `file_name`'s extension names, or `None` for any other file.
    #[must_use]
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        let extension = Path::new(file_name).extension()?.to_str()?;
        [Self::Asc, Self::Gem, Self::Gcs]
            .into_iter()
            .find(|kind| extension.eq_ignore_ascii_case(&kind.dotted_extension()[1..]))
    }

    /// The kind's file extension with its leading dot, for messages.
    #[must_use]
    pub const fn dotted_extension(self) -> &'static str {
        match self {
            Self::Asc => ".asc",
            Self::Gem => ".gem",
            Self::Gcs => ".gcs",
        }
    }
}

/// Where a design's cutting instructions are read from among its attachments.
///
/// The position (in `names`' order) and kind of the first `.asc`, else the first
/// `.gem`, else the first `.gcs`. `None` when no attachment is a design file.
#[must_use]
pub fn design_attachment_position<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Option<(usize, DesignFileKind)> {
    let kinds: Vec<Option<DesignFileKind>> = names
        .into_iter()
        .map(DesignFileKind::from_file_name)
        .collect();
    [
        DesignFileKind::Asc,
        DesignFileKind::Gem,
        DesignFileKind::Gcs,
    ]
    .into_iter()
    .find_map(|wanted| {
        kinds
            .iter()
            .position(|kind| *kind == Some(wanted))
            .map(|position| (position, wanted))
    })
}

/// A design file read as `.asc` cutting instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignFileText {
    /// The `.asc` file name the design is recorded under: the attachment's own
    /// name for a `.asc`, `<stem>.asc` for a converted `.gem`/`.gcs`.
    pub asc_file_name: String,
    /// The cutting instructions as `.asc` text.
    pub asc_text: String,
    /// What the `.gem`/`.gcs` reader and converter noted (a preform section that
    /// is not converted, hidden or guide tiers, a missing refractive index,
    /// unknown `.gcs` elements). Always empty for a `.asc`, whose own parse
    /// diagnostics appear when it is parsed.
    pub warnings: Vec<String>,
}

/// `file_name`'s stem plus `.asc` (`round.gem` -> `round.asc`): the name a
/// converted `.gem`/`.gcs` design is stored and offered for saving under, so a
/// later save never proposes overwriting the file it came from.
#[must_use]
pub fn converted_asc_file_name(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "design".to_string());
    format!("{stem}.asc")
}

/// Reads one design-file attachment as `.asc` text.
///
/// A `.asc` is decoded as `decode_asc_bytes` does (Windows-1252 aware); a
/// `.gem`/`.gcs` is parsed, converted to cutting instructions, its line breaks
/// inside headers, footnotes and tier notes replaced by spaces (an `.asc` field
/// is one line), and written with `to_asc_string`.
///
/// # Errors
///
/// A message naming the format and the reader's typed error when a `.gem`/`.gcs`
/// does not parse, describes no facets, or cannot be written as `.asc`. A `.asc`
/// never errors here; it is parsed by the caller.
pub fn design_file_to_asc_text(
    file_name: &str,
    kind: DesignFileKind,
    bytes: &[u8],
) -> Result<DesignFileText, String> {
    let mut schedule = match kind {
        DesignFileKind::Asc => {
            return Ok(DesignFileText {
                asc_file_name: file_name.to_string(),
                asc_text: decode_asc_bytes(bytes).into_owned(),
                warnings: Vec::new(),
            });
        }
        DesignFileKind::Gem => parse_gem(bytes)
            .map(|design| gem_to_asc_schedule(&design))
            .map_err(|e| format!("not a readable .gem file: {e}"))?,
        DesignFileKind::Gcs => parse_gcs_bytes(bytes)
            .map(|design| gcs_to_asc_schedule(&design))
            .map_err(|e| format!("not a readable .gcs file: {e}"))?,
    };
    if schedule.tiers.is_empty() {
        return Err(format!(
            "the {} file describes no facets",
            kind.dotted_extension()
        ));
    }
    flatten_line_breaks(&mut schedule);
    let warnings = std::mem::take(&mut schedule.warnings);
    let asc_text = to_asc_string(&schedule)
        .map_err(|e| format!("its cutting instructions cannot be written as .asc: {e}"))?;
    Ok(DesignFileText {
        asc_file_name: converted_asc_file_name(file_name),
        asc_text,
        warnings,
    })
}

/// Replaces every run of CR/LF in the free-text fields `.asc` stores on one line
/// with a single space.
fn flatten_line_breaks(schedule: &mut AscSchedule) {
    let one_line = |text: &str| -> String {
        text.split(['\r', '\n'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    for header in &mut schedule.headers {
        *header = one_line(header);
    }
    for footnote in &mut schedule.footnotes {
        *footnote = one_line(footnote);
    }
    for tier in &mut schedule.tiers {
        tier.notes = one_line(&tier.notes);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DesignFileKind, converted_asc_file_name, design_attachment_position,
        design_file_to_asc_text,
    };

    #[test]
    fn asc_wins_over_gem_and_gem_over_gcs_case_insensitively() {
        assert_eq!(
            design_attachment_position(["a.GCS", "b.gem", "c.ASC", "d.asc"]),
            Some((2, DesignFileKind::Asc))
        );
        assert_eq!(
            design_attachment_position(["a.gcs", "notes.pdf", "b.Gem"]),
            Some((2, DesignFileKind::Gem))
        );
        assert_eq!(
            design_attachment_position(["notes.pdf", "a.gcs"]),
            Some((1, DesignFileKind::Gcs))
        );
        assert_eq!(design_attachment_position(["notes.pdf", "x.toml"]), None);
    }

    #[test]
    fn converted_names_take_the_stem() {
        assert_eq!(
            converted_asc_file_name("Round Brilliant.gem"),
            "Round Brilliant.asc"
        );
        assert_eq!(converted_asc_file_name(".gcs"), ".gcs.asc");
    }

    #[test]
    fn an_asc_is_decoded_unchanged_and_a_corrupt_gem_is_a_typed_error() {
        let asc = design_file_to_asc_text("x.asc", DesignFileKind::Asc, b"GemCad 5.0\n")
            .expect("an .asc never errors here");
        assert_eq!(asc.asc_file_name, "x.asc");
        assert_eq!(asc.asc_text, "GemCad 5.0\n");

        let err = design_file_to_asc_text("x.gem", DesignFileKind::Gem, &[1, 2, 3])
            .expect_err("three bytes are no .gem");
        assert!(err.starts_with("not a readable .gem file: byte 0"), "{err}");
    }
}

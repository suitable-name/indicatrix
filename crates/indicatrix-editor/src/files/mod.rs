//! File kinds, foreign-design conversion and file naming for a front end that opens
//! and saves designs as bytes and names rather than paths (the wasm web app).
//!
//! Everything here is a pure function of names, bytes and text:
//!
//! - [`InputFileKind::classify`] / [`InputFileKind::from_file_name`]: what an
//!   opened/dropped file is, by its content and name or by its name alone.
//! - [`convert_foreign_design`]: a `.gem`/`.gcs` design converted to `.asc` text,
//!   the same rule `indicatrix_vault::local::design_file_to_asc_text` applies for
//!   the desktop (the vault crate links SQLite, so a browser build cannot call it).
//! - [`suggested_asc_file_name`], [`cutting_sheet_file_name`], [`diagram_file_name`]:
//!   the desktop's default save
//!   names (`apps/indicatrix-cut`'s `native_io::picker::suggested_file_name` and its
//!   export callbacks), so a download is named the way the desktop's save dialog
//!   would propose.
//! - [`degenerate_marker_header`]: the "NOT A CLOSED SOLID" header line the desktop
//!   stamps into a written schedule once the cutter confirms writing a design that
//!   does not close.

use indicatrix_formats::{
    asc::{AscSchedule, to_asc_string},
    gcs::{GcsTierFrosting, gcs_frosting, gcs_to_asc_schedule, parse_gcs_bytes},
    gem::{gem_to_asc_schedule, parse_gem},
    native::design::{DESIGN_READ_EXTENSIONS, FileKind, detect_kind},
};

#[cfg(test)]
mod tests;

/// What an opened or dropped file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFileKind {
    /// `.asc` cutting instructions.
    Asc,
    /// A self-contained design file: `.isdd`, or the earlier `.indicatrix` (read the
    /// same way).
    Design,
    /// `GemCAD`'s binary `.gem` save file.
    Gem,
    /// Gem Cut Studio's `.gcs` file.
    Gcs,
    /// A Radiance `.hdr` environment map (lighting, not a design).
    Hdr,
}

impl InputFileKind {
    /// The kind `file_name` names (case-insensitive), or `None` for a file this app
    /// does not open. `.isdd` and the earlier `.indicatrix` are a [`Self::Design`]
    /// (both listed in [`DESIGN_READ_EXTENSIONS`]); the removed overlay sidecar
    /// (`.indicatrix.toml`, `.gemcut.toml`) and any other `.toml` are not opened.
    #[must_use]
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        let lower = file_name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, ext)| ext)?;
        match extension {
            "asc" => Some(Self::Asc),
            ext if DESIGN_READ_EXTENSIONS.contains(&ext) => Some(Self::Design),
            "gem" => Some(Self::Gem),
            "gcs" => Some(Self::Gcs),
            "hdr" => Some(Self::Hdr),
            _ => None,
        }
    }

    /// The kind of the file called `file_name` holding `bytes`, or `None` for a file
    /// the app does not open.
    ///
    /// `.hdr`, `.asc`, `.gem` and `.gcs` are judged by name. A design file name
    /// (`.isdd`, `.indicatrix`) stays a [`Self::Design`] whatever its header says, so the
    /// loader's own error message names what is wrong with it (a removed overlay sidecar
    /// saved under a design name, say). A `.toml` file is a [`Self::Design`] only when
    /// its header ([`detect_kind`]) says it is one; any other `.toml`, the removed
    /// overlay sidecar (`.indicatrix.toml`, `.gemcut.toml`) included, is not opened.
    #[must_use]
    pub fn classify(file_name: &str, bytes: &[u8]) -> Option<Self> {
        let by_name = Self::from_file_name(file_name);
        if by_name.is_none()
            && file_name.to_ascii_lowercase().ends_with(".toml")
            && detect_kind(bytes) == FileKind::Design
        {
            return Some(Self::Design);
        }
        by_name
    }

    /// Whether this kind describes a design (as opposed to an environment map).
    #[must_use]
    pub const fn is_design(self) -> bool {
        !matches!(self, Self::Hdr)
    }
}

/// A `.gem`/`.gcs` design converted to `.asc` cutting instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedDesign {
    /// The `.asc` name the design is recorded under: `<stem>.asc`, so a later save
    /// never proposes overwriting the file it came from.
    pub asc_file_name: String,
    /// The cutting instructions as `.asc` text.
    pub asc_text: String,
    /// What the reader and converter noted (a `.gem` preform section that is not
    /// converted, `.gcs` hidden or guide tiers, a missing refractive index, unknown
    /// `.gcs` elements). Empty for a clean file.
    pub warnings: Vec<String>,
    /// What a `.gcs` file states as frosted, by tier position and facet slot of the
    /// converted schedule (`.asc` has no notation for it). Empty for a `.gem` and for a
    /// `.gcs` that frosts nothing. Apply it with `Design::apply_imported_frosting` once the
    /// design is built from `asc_text`.
    pub frosting: Vec<(usize, GcsTierFrosting)>,
}

/// `file_name`'s stem plus `.asc` (`round.gem` -> `round.asc`); `design.asc` for a
/// name with no stem.
#[must_use]
pub fn converted_asc_file_name(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = base.rsplit_once('.').map_or(base, |(stem, _)| stem);
    if stem.is_empty() {
        format!("{base}.asc")
    } else {
        format!("{stem}.asc")
    }
}

/// Converts a `.gem` or `.gcs` design (`kind`) to `.asc` text.
///
/// Line breaks inside headers, footnotes and tier notes become single spaces (an
/// `.asc` field is one line), and the reader's warnings are returned alongside.
///
/// # Errors
///
/// A message naming the format when `kind` is not `Gem`/`Gcs`, the file does not
/// parse, describes no facets, or the converted schedule cannot be written as `.asc`.
pub fn convert_foreign_design(
    file_name: &str,
    kind: InputFileKind,
    bytes: &[u8],
) -> Result<ConvertedDesign, String> {
    let mut frosting = Vec::new();
    let (mut schedule, dotted) = match kind {
        InputFileKind::Gem => (
            parse_gem(bytes)
                .map(|design| gem_to_asc_schedule(&design))
                .map_err(|e| format!("not a readable .gem file: {e}"))?,
            ".gem",
        ),
        InputFileKind::Gcs => (
            parse_gcs_bytes(bytes)
                .map(|design| {
                    frosting = gcs_frosting(&design);
                    gcs_to_asc_schedule(&design)
                })
                .map_err(|e| format!("not a readable .gcs file: {e}"))?,
            ".gcs",
        ),
        other => return Err(format!("{other:?} is not a .gem or .gcs design")),
    };
    if schedule.tiers.is_empty() {
        return Err(format!("the {dotted} file describes no facets"));
    }
    flatten_line_breaks(&mut schedule);
    let warnings = std::mem::take(&mut schedule.warnings);
    let asc_text = to_asc_string(&schedule)
        .map_err(|e| format!("its cutting instructions cannot be written as .asc: {e}"))?;
    Ok(ConvertedDesign {
        asc_file_name: converted_asc_file_name(file_name),
        asc_text,
        warnings,
        frosting,
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

/// Replaces the characters Windows forbids in a file name (`\ / : * ? " < > |`) with
/// `_` and trims; `"design"` for a title with nothing left. The desktop's
/// `sanitize_filename`, character for character.
#[must_use]
pub fn sanitize_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "design".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The `.asc` name a save proposes.
///
/// It is the design's own recorded name when it has one,
/// else its first non-blank header sanitized plus `.asc`, else
/// `"edited_design.asc"` -- the desktop's `suggested_file_name` rule.
#[must_use]
pub fn suggested_asc_file_name(asc_filename: Option<&str>, headers: &[String]) -> String {
    if let Some(name) = asc_filename {
        return name.to_string();
    }
    match headers.first() {
        Some(header) if !header.trim().is_empty() => {
            format!("{}.asc", sanitize_file_name(header))
        }
        _ => "edited_design.asc".to_string(),
    }
}

/// The cutting-sheet download name: `<asc name without .asc>_cutting_sheet.html`.
#[must_use]
pub fn cutting_sheet_file_name(asc_file_name: &str) -> String {
    format!(
        "{}_cutting_sheet.html",
        asc_file_name.trim_end_matches(".asc")
    )
}

/// The diagram download name: `<asc name without .asc>_diagram.png`.
#[must_use]
pub fn diagram_file_name(asc_file_name: &str) -> String {
    format!("{}_diagram.png", asc_file_name.trim_end_matches(".asc"))
}

/// The leading header a confirmed "not a closed solid" write stamps into the
/// written schedule.
pub const NOT_CLOSED_SOLID_MARKER: &str = "NOT A CLOSED SOLID";

/// The header line itself (`"NOT A CLOSED SOLID -- <message>"`), or `None` when
/// `headers` already starts with one (never stamped twice).
#[must_use]
pub fn degenerate_marker_header(headers: &[String], message: &str) -> Option<String> {
    let already_marked = headers
        .first()
        .is_some_and(|h| h.starts_with(NOT_CLOSED_SOLID_MARKER));
    (!already_marked).then(|| format!("{NOT_CLOSED_SOLID_MARKER} -- {message}"))
}

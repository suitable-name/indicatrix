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
//! - [`paired_asc_index`]: which of several `.asc` files an older `.indicatrix.toml` sidecar
//!   pairs with.
//! - [`suggested_asc_file_name`], [`native_file_name_for_asc`] (the older sidecar's name),
//!   [`cutting_sheet_file_name`], [`diagram_file_name`]: the desktop's default save
//!   names (`apps/indicatrix-cut`'s `native_io::picker::suggested_file_name` and its
//!   export callbacks), so a download is named the way the desktop's save dialog
//!   would propose.
//! - [`degenerate_marker_header`]: the "NOT A CLOSED SOLID" header line the desktop
//!   stamps into a written schedule once the cutter confirms writing a design that
//!   does not close.

use indicatrix_formats::{
    asc::{AscSchedule, to_asc_string},
    gcs::{gcs_to_asc_schedule, parse_gcs_bytes},
    gem::{gem_to_asc_schedule, parse_gem},
    native::{
        LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX,
        design::{DESIGN_EXTENSION, FileKind, detect_kind},
    },
};

#[cfg(test)]
mod tests;

/// What an opened or dropped file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFileKind {
    /// `.asc` cutting instructions.
    Asc,
    /// A self-contained `.indicatrix` design file.
    Design,
    /// An older overlay sidecar (`.indicatrix.toml`, the `.gemcut.toml` before it, or
    /// any other `.toml` -- the desktop's Open dialog offers "All TOML files" too),
    /// read together with its `.asc`.
    Sidecar,
    /// `GemCAD`'s binary `.gem` save file.
    Gem,
    /// Gem Cut Studio's `.gcs` file.
    Gcs,
    /// A Radiance `.hdr` environment map (lighting, not a design).
    Hdr,
}

impl InputFileKind {
    /// The kind `file_name` names (case-insensitive), or `None` for a file this app
    /// does not open. `.indicatrix` is a [`Self::Design`], any `.toml` a
    /// [`Self::Sidecar`]; use [`Self::classify`] when the content is at hand, since a
    /// `.toml` may in fact be a design file.
    #[must_use]
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        let lower = file_name.to_ascii_lowercase();
        let extension = lower.rsplit_once('.').map(|(_, ext)| ext)?;
        match extension {
            "asc" => Some(Self::Asc),
            ext if ext == DESIGN_EXTENSION => Some(Self::Design),
            "toml" => Some(Self::Sidecar),
            "gem" => Some(Self::Gem),
            "gcs" => Some(Self::Gcs),
            "hdr" => Some(Self::Hdr),
            _ => None,
        }
    }

    /// The kind of the file called `file_name` holding `bytes`, or `None` for a file
    /// the app does not open.
    ///
    /// `.hdr`, `.asc`, `.gem` and `.gcs` are judged by name. A `.indicatrix` or `.toml`
    /// file is judged by its header ([`detect_kind`]): a design file is
    /// [`Self::Design`], an older overlay sidecar [`Self::Sidecar`], whatever the name.
    /// A file whose header says neither keeps the kind its extension suggests, so the
    /// loader's own error message names what is wrong with it.
    #[must_use]
    pub fn classify(file_name: &str, bytes: &[u8]) -> Option<Self> {
        let by_name = Self::from_file_name(file_name)?;
        if !matches!(by_name, Self::Design | Self::Sidecar) {
            return Some(by_name);
        }
        Some(match detect_kind(bytes) {
            FileKind::Design => Self::Design,
            FileKind::OverlaySidecar => Self::Sidecar,
            FileKind::Unknown => by_name,
        })
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
    let (mut schedule, dotted) = match kind {
        InputFileKind::Gem => (
            parse_gem(bytes)
                .map(|design| gem_to_asc_schedule(&design))
                .map_err(|e| format!("not a readable .gem file: {e}"))?,
            ".gem",
        ),
        InputFileKind::Gcs => (
            parse_gcs_bytes(bytes)
                .map(|design| gcs_to_asc_schedule(&design))
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

/// The `.asc` name an older sidecar named `native_file_name` guesses for its pair
/// (`round.indicatrix.toml` -> `round.asc`, legacy `.gemcut.toml` too); `None` when
/// the name ends in neither suffix.
#[must_use]
pub fn asc_name_for_native(native_file_name: &str) -> Option<String> {
    let lower = native_file_name.to_ascii_lowercase();
    [NATIVE_EXTENSION_SUFFIX, LEGACY_NATIVE_EXTENSION_SUFFIX]
        .into_iter()
        .find_map(|suffix| {
            let dotted = format!(".{suffix}");
            lower.ends_with(&dotted).then(|| {
                format!(
                    "{}.asc",
                    &native_file_name[..native_file_name.len() - dotted.len()]
                )
            })
        })
}

/// Which of `asc_names` an older sidecar pairs with, when several files were opened
/// together.
///
/// The sidecar's own recorded `asc_filename` first (exact, then case-insensitive),
/// then the name guessed from the sidecar's own file name ([`asc_name_for_native`]),
/// then -- when exactly one `.asc` was opened alongside it -- that one (the paired
/// file's fingerprint check still catches a wrong pick).
#[must_use]
pub fn paired_asc_index(
    recorded_asc_name: &str,
    native_file_name: &str,
    asc_names: &[&str],
) -> Option<usize> {
    let by_name = |wanted: &str| {
        asc_names
            .iter()
            .position(|name| *name == wanted)
            .or_else(|| {
                asc_names
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(wanted))
            })
    };
    by_name(recorded_asc_name)
        .or_else(|| asc_name_for_native(native_file_name).and_then(|guess| by_name(&guess)))
        .or_else(|| (asc_names.len() == 1).then_some(0))
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

/// The older sidecar name paired with `asc_file_name`: its extension replaced by
/// `.indicatrix.toml` (`round.asc` -> `round.indicatrix.toml`), or the suffix
/// appended to a name with no extension.
#[must_use]
pub fn native_file_name_for_asc(asc_file_name: &str) -> String {
    let stem = asc_file_name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(asc_file_name, |(stem, _)| stem);
    format!("{stem}.{NATIVE_EXTENSION_SUFFIX}")
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

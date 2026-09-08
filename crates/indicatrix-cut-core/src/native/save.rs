//! [`save_paired`]: building the pair of files a "Save" action should write,
//! and the [`SaveError`]/[`PairedSave`] types around it. See the parent
//! module's doc comment ("Preserving the original `.asc` text") for the
//! preserve-vs-regenerate decision this makes.

use super::convert::to_native_file;
use crate::design::{Design, MissingAnchor};
use indicatrix_formats::native::{NativeDesignFile, NativeFormatError, to_toml_string};
use std::fmt;

/// Why [`save_paired`] could not produce a pair of files to write.
#[derive(Debug)]
pub enum SaveError {
    /// `design` itself does not currently solve -- see [`Design::solve`]. Nothing to
    /// export either as fresh `.asc` text or as a semantic-equality check against
    /// preserved original text, so this is the one error both paths share.
    Design(MissingAnchor),
    /// The (always-succeeds-in-practice, see [`to_toml_string`]'s own doc comment)
    /// TOML serialization step failed.
    Toml(NativeFormatError),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Design(e) => write!(f, "cannot save: {e}"),
            Self::Toml(e) => write!(f, "cannot save: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Everything [`save_paired`] produced.
///
/// The `.asc` text to write, whether it is the caller's own original text preserved
/// byte for byte or a fresh export, and the native file's own struct/text form
/// (already fingerprinted against `asc_text`).
#[derive(Debug)]
pub struct PairedSave {
    pub asc_text: String,
    /// `true` iff `asc_text` is `original_asc_text` unchanged; `false` iff it was
    /// freshly regenerated -- see [`save_paired`]'s doc comment for exactly when
    /// each happens. Exposed so a caller can report which one happened (e.g. "saved,
    /// schedule unchanged" vs. "saved, schedule updated") rather than guessing from
    /// the text itself.
    pub asc_preserved: bool,
    pub native: NativeDesignFile,
    pub native_toml: String,
}

/// Builds the pair of files a "Save" action should write.
///
/// `asc_text` alongside `native_toml`, the latter fingerprinting exactly the former --
/// see [`to_native_file`]'s own `asc_bytes` parameter, fed `asc_text.as_bytes()` here
/// so the fingerprint can never disagree with the bytes it is saved next to.
///
/// `original_asc_text`, when given, is preserved verbatim in `asc_text` whenever
/// `design`'s current [`Design::to_asc_schedule`] is semantically equal to what
/// `original_asc_text` itself parses to -- see the module doc comment's "Preserving
/// the original `.asc` text" section. Text that fails to parse, or is absent (a
/// brand-new design with no prior `.asc`), falls straight through to a fresh export.
///
/// # Errors
///
/// [`SaveError::Design`] if `design` does not currently solve (needed either way: for
/// a fresh export, or to compare against `original_asc_text`). [`SaveError::Toml`] if
/// serializing the resulting [`NativeDesignFile`] fails.
pub fn save_paired(
    design: &Design,
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
) -> Result<PairedSave, SaveError> {
    let current_schedule = design.to_asc_schedule().map_err(SaveError::Design)?;

    let preserved = original_asc_text.and_then(|original| {
        let original_schedule = indicatrix_formats::asc::parse_asc(original).ok()?;
        (original_schedule == current_schedule).then(|| original.to_string())
    });

    let (asc_text, asc_preserved) = preserved.map_or_else(
        || {
            (
                indicatrix_formats::asc::to_asc_string(&current_schedule),
                false,
            )
        },
        |original| (original, true),
    );

    let native = to_native_file(design, asc_filename, asc_text.as_bytes());
    let native_toml = to_toml_string(&native).map_err(SaveError::Toml)?;

    Ok(PairedSave {
        asc_text,
        asc_preserved,
        native,
        native_toml,
    })
}

//! The text formats of the rough-colour side tables: JSON for the zones and the fit report.
//!
//! The vault stores these as opaque text (`rough_colour.zoned_json`, `.fit_json`,
//! `material_zoning.zoned_json`, `render_job_zoning.zoned_json`). Everything read back is hostile
//! input as far as the tracer is concerned (a hand-edited file, a newer build's row), so
//! [`decode_zoned`] validates what it parsed before returning it.

use crate::rough_plan::colour_fit::solve::{ColourFit, FIT_VERSION};
use indicatrix::optics::zoning::ZonedAbsorption;
use std::fmt;

/// The format version of the stored zones and fit report. Bump it when either layout changes
/// incompatibly; a reader refuses a row with a newer version instead of guessing.
pub const ZONING_FORMAT_VERSION: u32 = 1;

/// The longest zones or fit text a reader accepts, in bytes (a fit report with its residual
/// maps is a few hundred kilobytes; this is only a guard against garbage).
pub const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Why a stored text could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The text is not the expected JSON.
    Json(String),
    /// The text parsed but describes zones the tracer must not receive.
    Invalid(String),
    /// The row was written by a newer build.
    UnsupportedVersion(u32),
    /// The text is longer than [`MAX_TEXT_BYTES`].
    TooLarge(usize),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "the stored colour is not readable: {message}"),
            Self::Invalid(message) => write!(f, "the stored colour zones are invalid: {message}"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "the stored colour was written by a newer version (format {version}); \
                 this version reads format {ZONING_FORMAT_VERSION}"
            ),
            Self::TooLarge(bytes) => {
                write!(f, "the stored colour text is too large ({bytes} bytes)")
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// The zones as JSON text, after checking they are valid and per millimetre.
///
/// # Errors
///
/// [`CodecError::Invalid`] if [`ZonedAbsorption::validate`] refuses them, [`CodecError::Json`]
/// if serialisation fails.
pub fn encode_zoned(zoned: &ZonedAbsorption) -> Result<String, CodecError> {
    zoned
        .validate()
        .map_err(|e| CodecError::Invalid(e.to_string()))?;
    serde_json::to_string(zoned).map_err(|e| CodecError::Json(e.to_string()))
}

/// The zones of a stored text, validated.
///
/// # Errors
///
/// [`CodecError::TooLarge`], [`CodecError::Json`] for text that does not parse,
/// [`CodecError::Invalid`] for zones the kernels must not receive.
pub fn decode_zoned(text: &str) -> Result<ZonedAbsorption, CodecError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(CodecError::TooLarge(text.len()));
    }
    let zoned: ZonedAbsorption =
        serde_json::from_str(text).map_err(|e| CodecError::Json(e.to_string()))?;
    zoned
        .validate()
        .map_err(|e| CodecError::Invalid(e.to_string()))?;
    Ok(zoned)
}

/// A fit report as JSON text.
///
/// # Errors
///
/// [`CodecError::Json`] if serialisation fails.
pub fn encode_fit(fit: &ColourFit) -> Result<String, CodecError> {
    serde_json::to_string(fit).map_err(|e| CodecError::Json(e.to_string()))
}

/// A stored fit report.
///
/// # Errors
///
/// [`CodecError::TooLarge`] or [`CodecError::Json`].
pub fn decode_fit(text: &str) -> Result<ColourFit, CodecError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(CodecError::TooLarge(text.len()));
    }
    serde_json::from_str(text).map_err(|e| CodecError::Json(e.to_string()))
}

/// The rough colour of one saved plan, as the application holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoughColour {
    /// The zones in the rough frame (mm).
    pub zoned: ZonedAbsorption,
    /// The fit report text (see [`decode_fit`]); kept as text so a row round-trips byte for byte.
    pub fit_json: String,
    /// The format version of both texts.
    pub version: u32,
    /// Unix seconds when the colour was stored.
    pub created: i64,
}

impl RoughColour {
    /// A rough colour for storing: `zoned` is validated and `fit` serialised.
    ///
    /// # Errors
    ///
    /// Whatever [`encode_zoned`] and [`encode_fit`] return.
    pub fn new(zoned: ZonedAbsorption, fit: &ColourFit, created: i64) -> Result<Self, CodecError> {
        zoned
            .validate()
            .map_err(|e| CodecError::Invalid(e.to_string()))?;
        Ok(Self {
            zoned,
            fit_json: encode_fit(fit)?,
            version: ZONING_FORMAT_VERSION,
            created,
        })
    }

    /// The fit report.
    ///
    /// # Errors
    ///
    /// [`CodecError::Json`] for a stored text that does not parse, and
    /// [`CodecError::UnsupportedVersion`] when the report was written by a newer fit.
    pub fn fit(&self) -> Result<ColourFit, CodecError> {
        let fit = decode_fit(&self.fit_json)?;
        if fit.version > FIT_VERSION {
            return Err(CodecError::UnsupportedVersion(fit.version));
        }
        Ok(fit)
    }
}

/// Refuses a row written by a newer build.
///
/// # Errors
///
/// [`CodecError::UnsupportedVersion`] when `version` is above [`ZONING_FORMAT_VERSION`].
pub const fn check_version(version: u32) -> Result<(), CodecError> {
    if version > ZONING_FORMAT_VERSION {
        Err(CodecError::UnsupportedVersion(version))
    } else {
        Ok(())
    }
}

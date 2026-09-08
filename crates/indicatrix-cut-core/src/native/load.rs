//! [`load_paired`]: turning a native file's TOML text plus its paired `.asc`
//! text into a real [`Design`], and the [`TierOverlay`]/[`LoadPairedError`]/
//! [`LoadPairedResult`] types around it. See the parent module's doc comment
//! for why `.asc` stays canonical for geometry regardless of what the native
//! file says.

use super::convert::{
    material_selection_from_table, meet_constraint_from_native, preform_spec_from_table,
};
use crate::design::Design;
use indicatrix_formats::native::{
    FingerprintCheck, NativeFormatError, check_fingerprint, from_toml_str,
};
use std::fmt;

/// Whether [`load_paired`] actually applied this file's per-tier `constraint`/
/// `detached` overlay on top of the freshly-imported `.asc` schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierOverlay {
    /// Every tier's saved `constraint`/`detached` was written onto the freshly
    /// imported design.
    Applied,
    /// Skipped because [`check_fingerprint`] reported
    /// [`FingerprintCheck::Mismatch`] -- see [`load_paired`]'s doc comment for why
    /// tier-index correlation can't be trusted once the paired `.asc` itself has
    /// changed.
    SkippedFingerprintMismatch,
    /// Skipped because this file's `tiers` array and the (fingerprint-matched,
    /// presumably unchanged) `.asc`'s own tier count disagree anyway -- a defensive
    /// belt-and-suspenders case (e.g. a hand-edited native file), never expected in
    /// practice.
    SkippedTierCountMismatch {
        native_tiers: usize,
        asc_tiers: usize,
    },
}

impl fmt::Display for TierOverlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Applied => write!(f, "per-tier meet-intent overlay applied"),
            Self::SkippedFingerprintMismatch => {
                write!(
                    f,
                    "per-tier meet-intent overlay skipped (fingerprint mismatch)"
                )
            }
            Self::SkippedTierCountMismatch {
                native_tiers,
                asc_tiers,
            } => write!(
                f,
                "per-tier meet-intent overlay skipped: native file has {native_tiers} tier(s) \
                 but the paired .asc has {asc_tiers}"
            ),
        }
    }
}

/// Why [`load_paired`] could not even build a [`Design`] at all.
///
/// Unlike [`FingerprintCheck::Mismatch`]/[`TierOverlay::SkippedTierCountMismatch`]
/// (both still produce a usable [`Design`]), either variant here means there is
/// nothing to load.
#[derive(Debug)]
pub enum LoadPairedError {
    /// The native file's own TOML text didn't parse.
    Native(NativeFormatError),
    /// The paired `.asc` text didn't parse (see [`indicatrix_formats::asc::parse_asc`]'s own
    /// error type, a plain human-readable message).
    Asc(String),
}

impl fmt::Display for LoadPairedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native(e) => write!(f, "native file is not valid: {e}"),
            Self::Asc(e) => write!(f, "paired .asc file is not valid: {e}"),
        }
    }
}

impl std::error::Error for LoadPairedError {}

/// Everything [`load_paired`] found.
///
/// Bundled together so a caller (the editor) can report the fingerprint/overlay
/// status alongside the [`Design`] it gets to work with -- never a silent choice
/// between them.
#[derive(Debug)]
pub struct LoadPairedResult {
    pub design: Design,
    pub fingerprint: FingerprintCheck,
    pub tier_overlay: TierOverlay,
}

/// Loads a paired native file + `.asc` text into a [`Design`].
///
/// Always builds the base [`Design`] from the REAL, freshly parsed `.asc` text via
/// [`Design::from_asc_schedule`] -- `.asc` stays canonical for geometry no matter
/// what the native file says. On top of that base, this applies (in order):
/// `preform`/`material`/`girdle_diameter_mm` unconditionally (none are
/// tier-indexed, so a changed `.asc` can't make them wrong), then the per-tier
/// `constraint`/`detached` overlay ONLY when [`check_fingerprint`] reports
/// [`FingerprintCheck::Match`] AND the tier counts agree (see [`TierOverlay`]):
/// `tiers[i]` here is only meaningful paired with the `i`-th tier of the EXACT
/// `.asc` content it was saved against.
///
/// # Errors
///
/// [`LoadPairedError`] if either file's own text fails to parse at all. A fingerprint
/// mismatch or tier-count mismatch is NOT an error -- both still produce a real,
/// loadable [`Design`]; see [`LoadPairedResult`].
pub fn load_paired(asc_text: &str, native_text: &str) -> Result<LoadPairedResult, LoadPairedError> {
    let native = from_toml_str(native_text).map_err(LoadPairedError::Native)?;
    let schedule = indicatrix_formats::asc::parse_asc(asc_text)
        .map_err(|e| LoadPairedError::Asc(e.to_string()))?;

    let fingerprint = check_fingerprint(&native, asc_text.as_bytes());

    let mut design = Design::from_asc_schedule(preform_spec_from_table(&native.preform), &schedule);
    design.girdle_diameter_mm = native.girdle_diameter_mm;
    design.material = material_selection_from_table(&native.material);

    let tier_overlay = if !matches!(fingerprint, FingerprintCheck::Match) {
        TierOverlay::SkippedFingerprintMismatch
    } else if native.tiers.len() == design.tiers.len() {
        for (tier, saved) in design.tiers.iter_mut().zip(native.tiers) {
            tier.constraint = meet_constraint_from_native(saved.constraint);
            tier.detached = saved.detached;
        }
        TierOverlay::Applied
    } else {
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: native.tiers.len(),
            asc_tiers: design.tiers.len(),
        }
    };

    Ok(LoadPairedResult {
        design,
        fingerprint,
        tier_overlay,
    })
}

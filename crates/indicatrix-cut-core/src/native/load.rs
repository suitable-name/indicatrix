//! [`load_paired`]: turning a native file's TOML text plus its paired `.asc`
//! text into a real [`Design`], and the [`TierOverlay`]/[`LoadPairedError`]/
//! [`LoadPairedResult`] types around it. See the parent module's doc comment
//! for why `.asc` stays canonical for geometry regardless of what the native
//! file says.

use super::convert::{
    external_proportions_from_source, material_selection_from_table, meet_constraint_from_native,
    preform_spec_from_table,
};
use crate::design::{ConstraintTier, Design};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::{
    FORMAT_VERSION, FingerprintCheck, NativeFormatError, TierTable, check_fingerprint,
    from_toml_str,
};
use std::fmt;

/// Whether [`load_paired`] actually applied this file's per-tier `constraint`/
/// `detached` overlay on top of the freshly-imported `.asc` schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierOverlay {
    /// Every tier's saved `constraint`/`detached` was written onto the freshly
    /// imported design.
    Applied,
    /// Every tier's saved `constraint`/`detached` was written onto the freshly
    /// imported design ANYWAY, despite [`check_fingerprint`] reporting
    /// [`FingerprintCheck::Mismatch`], because the caller passed
    /// `apply_overlay_on_mismatch: true` to [`load_paired`] and the tier counts
    /// still happened to agree. Distinct from [`Self::Applied`] so a caller can
    /// still flag that this was the cutter's own explicit call, not an ordinary
    /// clean reload.
    AppliedDespiteMismatch,
    /// Skipped because [`check_fingerprint`] reported
    /// [`FingerprintCheck::Mismatch`] and the caller did not ask for the overlay to
    /// be applied anyway -- see [`load_paired`]'s doc comment for why tier-index
    /// correlation can't be trusted once the paired `.asc` itself has changed.
    SkippedFingerprintMismatch,
    /// Skipped because this file's `tiers` array and the paired `.asc`'s own tier
    /// count disagree -- a defensive belt-and-suspenders case (e.g. a hand-edited
    /// native file), never expected in practice. Reached whether or not the
    /// fingerprint matched, since a tier-count mismatch makes array-position
    /// correlation meaningless either way.
    SkippedTierCountMismatch {
        native_tiers: usize,
        asc_tiers: usize,
    },
    /// This file's `tiers` array was the design's ONLY source of tier data --
    /// [`indicatrix_formats::native::NativeDesignFile::draft`] was `true`, so the paired
    /// `.asc`'s own tiers (placeholder masts, saved by
    /// `indicatrix_cut_core::native::save_paired` because `design` did not solve) were
    /// never read at all. Not an "overlay" in the usual sense (nothing from `.asc`
    /// import survives underneath it), but reported through this same type since a
    /// caller cares about the same question either way: where did `design.tiers`
    /// actually come from?
    AppliedFromDraft,
}

impl fmt::Display for TierOverlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Applied => write!(f, "per-tier meet-intent overlay applied"),
            Self::AppliedDespiteMismatch => {
                write!(
                    f,
                    "per-tier meet-intent overlay applied despite a fingerprint mismatch, on request"
                )
            }
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
            Self::AppliedFromDraft => write!(
                f,
                "tier list rebuilt from the saved draft (the paired .asc's masts are \
                 placeholders and were not used)"
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

/// Whether the loaded design's [`crate::material::MaterialSelection::name`] resolves
/// to a built-in [`indicatrix::optics::materials::GemMaterial`] this process
/// recognizes outright.
///
/// Checked purely against the built-in preset table via
/// [`indicatrix::optics::materials::GemMaterial::by_name`] -- this crate has no
/// dependency on a custom-material catalogue (that lives several layers up, in
/// `indicatrix-vault`/the editor's own session-local registry), so
/// [`Self::Unresolved`] is a HEURISTIC, not a guarantee: a name reported unresolved
/// here may still resolve fine against the caller's own catalogue (a custom material
/// saved under that name -- see [`crate::material::MaterialSelection::resolve`]'s own
/// `catalogue` parameter). It exists so a caller with no such catalogue wired up yet
/// (or one that hasn't re-checked) can still warn "this design names a material this
/// build doesn't recognize on its own" rather than silently accepting
/// [`crate::material::MaterialSelection::resolve`]'s "always returns something
/// usable" fallback to [`indicatrix::optics::materials::GemMaterial::diamond`] --
/// exactly the silent Diamond substitution Item 83 exists to flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialResolution {
    /// `design.material.name` is `None` -- nothing to resolve.
    NoneSelected,
    /// `design.material.name` matches a built-in preset by exact name.
    Known,
    /// `design.material.name` matches no built-in preset. Likely (not certainly) a
    /// custom material this loading process's own catalogue would need to supply --
    /// see this type's own doc comment for why that can't be checked here.
    Unresolved,
}

impl fmt::Display for MaterialResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoneSelected => write!(f, "no material selected"),
            Self::Known => write!(f, "material recognized"),
            Self::Unresolved => write!(
                f,
                "material not recognized as a built-in preset -- it may render as \
                 Diamond until a matching custom material is restored"
            ),
        }
    }
}

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
    /// See [`MaterialResolution`] -- whether `design.material.name` is likely to
    /// still mean what it meant when this file was saved.
    pub material_resolution: MaterialResolution,
    /// The printed/measured proportions this design was loaded against, if the
    /// sidecar's `[source]` table has one (Item 178) -- `None` for a sidecar saved
    /// before that table existed, or one for a design that was never associated with
    /// a catalogue row at all. A caller restores this into its own equivalent of
    /// `EditorState::printed_proportions` so Deep Solve has something to verify
    /// against after a Save Native/Open Native round trip, not just on the design's
    /// very first "Load Selected" from the catalogue.
    pub printed_proportions: Option<ExternalProportions>,
    /// `true` iff the sidecar's own `format_version` is newer than this build's
    /// [`FORMAT_VERSION`] -- Item 181: the field was written but never checked, so a
    /// sidecar from a future build silently loaded as if it were this version, with
    /// any field that build understands and this one doesn't quietly parked in
    /// `unknown` (and re-serialized on the very next save, degrading it further on
    /// each round trip through an older install). A caller warns rather than
    /// refusing outright -- see [`FORMAT_VERSION`]'s own doc comment: a version bump
    /// means "a change `serde(default)` can't handle," not necessarily "this file is
    /// unreadable," and every named field here already tolerates being absent.
    pub written_by_newer_version: bool,
}

/// Loads a paired native file + `.asc` text into a [`Design`].
///
/// Always builds the base [`Design`] from the REAL, freshly parsed `.asc` text via
/// [`Design::from_asc_schedule`] -- `.asc` stays canonical for geometry no matter
/// what the native file says, UNLESS `native.draft` is `true` (see
/// [`indicatrix_formats::native::NativeDesignFile::draft`]'s own doc comment), in which case
/// `design.tiers` is replaced wholesale from the native file's own `tiers` instead:
/// a draft's paired `.asc` masts are placeholders `indicatrix_cut_core::native::save_paired`
/// invented because the design did not solve, never real cut instructions.
///
/// For a non-draft file, on top of the `.asc`-derived base this applies (in order):
/// `preform`/`material`/`girdle_diameter_mm` unconditionally (none are tier-indexed,
/// so a changed `.asc` can't make them wrong), then the per-tier `constraint`/
/// `detached` overlay when the tier counts agree AND either [`check_fingerprint`]
/// reports [`FingerprintCheck::Match`] or the caller passed
/// `apply_overlay_on_mismatch: true` (see [`TierOverlay`]): `tiers[i]` here is only
/// meaningful paired with the `i`-th tier of the EXACT `.asc` content it was saved
/// against, which is exactly what a fingerprint match promises; `true` lets a caller
/// (e.g. the editor, after the cutter confirms they still want it) apply it anyway.
///
/// # Errors
///
/// [`LoadPairedError`] if either file's own text fails to parse at all. A fingerprint
/// mismatch or tier-count mismatch is NOT an error -- both still produce a real,
/// loadable [`Design`]; see [`LoadPairedResult`].
pub fn load_paired(
    asc_text: &str,
    native_text: &str,
    apply_overlay_on_mismatch: bool,
) -> Result<LoadPairedResult, LoadPairedError> {
    let native = from_toml_str(native_text).map_err(LoadPairedError::Native)?;
    let schedule = indicatrix_formats::asc::parse_asc(asc_text)
        .map_err(|e| LoadPairedError::Asc(e.to_string()))?;

    let fingerprint = check_fingerprint(&native, asc_text.as_bytes());
    let fingerprint_matches = matches!(fingerprint, FingerprintCheck::Match);
    let native_tier_count = native.tiers.len();

    let mut design = Design::from_asc_schedule(preform_spec_from_table(&native.preform), &schedule);
    design.girdle_diameter_mm = native.girdle_diameter_mm;
    design.material = material_selection_from_table(&native.material);

    let tier_overlay = if native.draft {
        design.tiers = draft_tiers_from_native(native.tiers);
        TierOverlay::AppliedFromDraft
    } else if !fingerprint_matches && !apply_overlay_on_mismatch {
        TierOverlay::SkippedFingerprintMismatch
    } else if native_tier_count == design.tiers.len() {
        for (tier, saved) in design.tiers.iter_mut().zip(native.tiers) {
            tier.constraint = meet_constraint_from_native(saved.constraint);
            tier.detached = saved.detached;
        }
        if fingerprint_matches {
            TierOverlay::Applied
        } else {
            TierOverlay::AppliedDespiteMismatch
        }
    } else {
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: native_tier_count,
            asc_tiers: design.tiers.len(),
        }
    };

    let material_resolution = material_resolution_of(design.material.name.as_deref());
    let written_by_newer_version = native.format_version > FORMAT_VERSION;
    let printed_proportions = native.source.as_ref().map(external_proportions_from_source);

    Ok(LoadPairedResult {
        design,
        fingerprint,
        tier_overlay,
        material_resolution,
        printed_proportions,
        written_by_newer_version,
    })
}

/// [`MaterialResolution`]'s own computation -- see that type's doc comment for the
/// heuristic and its limits.
fn material_resolution_of(name: Option<&str>) -> MaterialResolution {
    match name {
        None => MaterialResolution::NoneSelected,
        Some(name) if indicatrix::optics::materials::GemMaterial::by_name(name).is_some() => {
            MaterialResolution::Known
        }
        Some(_) => MaterialResolution::Unresolved,
    }
}

/// Rebuilds a draft save's full tier list purely from the native sidecar, ignoring
/// whatever the paired `.asc` (placeholder masts and all) says -- see
/// [`TierOverlay::AppliedFromDraft`] and [`load_paired`]'s own doc comment. Missing
/// `angle_deg`/`indices` (a defensively-tolerated hand-edited or pre-draft file) fall
/// back to `0.0`/empty rather than panicking; there is no better number to invent.
fn draft_tiers_from_native(native_tiers: Vec<TierTable>) -> Vec<ConstraintTier> {
    native_tiers
        .into_iter()
        .map(|saved| ConstraintTier {
            angle_deg: saved.angle_deg.unwrap_or_default(),
            name: saved.name,
            indices: saved.indices.unwrap_or_default(),
            constraint: meet_constraint_from_native(saved.constraint),
            imported_meet: saved.imported_meet.map(meet_constraint_from_native),
            original_notes: saved.original_notes,
            detached: saved.detached,
        })
        .collect()
}

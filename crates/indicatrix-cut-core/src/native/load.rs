//! [`load_paired`]: turning a native file's TOML text plus its paired `.asc`
//! text into a real [`Design`], and the [`TierOverlay`]/[`LoadPairedError`]/
//! [`LoadPairedResult`] types around it. See the parent module's doc comment
//! for why `.asc` stays canonical for geometry regardless of what the native
//! file says.

use super::convert::{
    external_proportions_from_source, material_selection_from_table, meet_constraint_from_native,
    preform_spec_from_table, tier_target_from_native, unstash_schedule_meta,
};
use crate::design::{ConstraintTier, Design, TierId};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::{
    CustomMaterialSnapshot, FORMAT_VERSION, FingerprintCheck, NativeFormatError, NativeTierTarget,
    TierTable, check_fingerprint, from_toml_str,
};
use std::{collections::BTreeMap, fmt};

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
/// (both still produce a usable [`Design`]), every variant here means there is
/// nothing to load.
#[derive(Debug)]
pub enum LoadPairedError {
    /// The native file's own TOML text didn't parse.
    Native(NativeFormatError),
    /// The paired `.asc` text didn't parse (see [`indicatrix_formats::asc::parse_asc`]'s own
    /// error type, a plain human-readable message).
    Asc(String),
    /// A DRAFT sidecar's tier at this index has no `angle_deg`/`indices` recorded
    /// (`None` for one or both -- see
    /// [`indicatrix_formats::native::TierTable::angle_deg`]/[`indicatrix_formats::native::TierTable::indices`]'s
    /// own doc comments). For [`indicatrix_formats::native::NativeDesignFile::draft`]
    /// this tier list is the design's ONLY source of geometry (the paired `.asc`'s own masts are
    /// placeholders -- see [`TierOverlay::AppliedFromDraft`]), so there is no
    /// better number to fall back on: [`super::save::draft_tier_tables`] always
    /// writes both fields for every tier of a real draft save, so this only
    /// happens against a hand-edited or otherwise corrupted file. Instead of
    /// silently defaulting to `0.0`/an empty index list (which invents geometry a
    /// cutter never authored), this error is returned.
    DraftTierMissingGeometry { index: usize },
}

impl fmt::Display for LoadPairedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native(e) => write!(f, "native file is not valid: {e}"),
            Self::Asc(e) => write!(f, "paired .asc file is not valid: {e}"),
            Self::DraftTierMissingGeometry { index } => write!(
                f,
                "draft sidecar's tier {} has no recorded angle/indices to rebuild it from",
                index + 1
            ),
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
/// usable" fallback to [`indicatrix::optics::materials::GemMaterial::diamond`].
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
    /// sidecar's `[source]` table has one -- `None` for a sidecar saved
    /// before that table existed, or one for a design that was never associated with
    /// a catalogue row at all. A caller restores this into its own equivalent of
    /// `EditorState::printed_proportions` so Deep Solve has something to verify
    /// against after a Save Native/Open Native round trip, not just on the design's
    /// very first "Load Selected" from the catalogue.
    pub printed_proportions: Option<ExternalProportions>,
    /// `true` iff the sidecar's own `format_version` is newer than this build's
    /// [`FORMAT_VERSION`] -- without this check, a sidecar from a future build would
    /// silently load as if it were this version, with any field that build
    /// understands and this one doesn't quietly parked in `unknown` (and
    /// re-serialized on the very next save, degrading it further on each round trip
    /// through an older install). A caller warns rather than
    /// refusing outright -- see [`FORMAT_VERSION`]'s own doc comment: a version bump
    /// means "a change `serde(default)` can't handle," not necessarily "this file is
    /// unreadable," and every named field here already tolerates being absent.
    pub written_by_newer_version: bool,
    /// The sidecar's own `[material.custom]` snapshot, when
    /// [`Self::material_resolution`] is [`MaterialResolution::Unresolved`] AND the
    /// file actually carried one -- everything needed to reconstruct
    /// the design's real material instead of letting
    /// [`crate::material::MaterialSelection::resolve`] silently fall back to
    /// [`indicatrix::optics::materials::GemMaterial::diamond`]. A caller (the editor)
    /// turns this back into a real `GemMaterial` via
    /// [`super::gem_material_from_custom_snapshot`], registers it under
    /// `design.material.name` in its own catalogue, and toasts that it was restored
    /// from the file. `None` when the material resolved fine (nothing to restore) or
    /// when it did not and the file carried no snapshot either (an older save, or a
    /// design whose material was never actually custom).
    pub restorable_custom_material: Option<CustomMaterialSnapshot>,
    /// The sidecar's own `[history]` entries, oldest first --
    /// [`crate::edit::History::description_log`]'s bounded trail as of the save that
    /// wrote this file. Empty for a sidecar saved before the `[history]` table
    /// existed, or one whose design had no recorded history yet.
    pub history_entries: Vec<String>,
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
/// so a changed `.asc` can't make them wrong), then, whenever the tier counts agree
/// (regardless of fingerprint), the per-tier GEOMETRY-FREE fields --
/// [`indicatrix_formats::native::TierTable::note`]/`cheater_offset_deg`/`imported_meet`/
/// `original_notes` -- since array-position correlation is all any of those four
/// need (none feeds the solver, so a changed `.asc` can't make them wrong either,
/// only stale); a `note`/`cheater_offset_deg`/`original_notes` present in the file
/// overwrites whatever `.asc` import produced, while `imported_meet` only overwrites
/// when the file actually recorded one. Finally the per-tier `constraint`/`detached`
/// overlay applies ONLY when tier counts agree AND either [`check_fingerprint`]
/// reports [`FingerprintCheck::Match`] or the caller passed
/// `apply_overlay_on_mismatch: true` (see [`TierOverlay`]): `tiers[i]`'s
/// `constraint`/`detached` are only meaningful paired with the `i`-th tier of the
/// EXACT `.asc` content they were saved against, which is exactly what a
/// fingerprint match promises; `true` lets a caller (e.g. the editor, after the
/// cutter confirms they still want it) apply it anyway.
///
/// # Errors
///
/// [`LoadPairedError`] if either file's own text fails to parse at all, or (a draft
/// file only) a tier is missing the `angle_deg`/`indices` this is its only source
/// for (see [`LoadPairedError::DraftTierMissingGeometry`]). A fingerprint mismatch
/// or tier-count mismatch is NOT an error -- both still produce a real, loadable
/// [`Design`]; see [`LoadPairedResult`].
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
    design.preform_y_offset = native.preform.y_offset;
    design.material = material_selection_from_table(&native.material);
    // Restores the AUTHORED RI over whatever `Design::from_asc_schedule` just
    // derived from the paired `.asc`'s own `I` line (the EFFECTIVE RI at save
    // time -- see `NativeDesignFile::authored_refractive_index`'s own doc
    // comment). `None` (a file saved before this field existed) leaves `.asc`
    // import's own value in place -- a lossy but deliberate fallback for such a
    // file.
    if let Some(authored) = native.authored_refractive_index {
        design.meta.refractive_index = authored;
    }

    let tier_overlay = if native.draft {
        design.tier_notes = tier_notes_from_native(&native.tiers);
        design.cheater_offsets_deg = cheater_offsets_from_native(&native.tiers);
        let raw_ids = raw_tier_ids_from_native(&native.tiers);
        let raw_targets = raw_tier_targets_from_native(&native.tiers);
        design.tiers = draft_tiers_from_native(native.tiers)?;
        apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets);
        TierOverlay::AppliedFromDraft
    } else if native_tier_count != design.tiers.len() {
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: native_tier_count,
            asc_tiers: design.tiers.len(),
        }
    } else {
        // The tier counts agree, so array-position correlation is trustworthy
        // regardless of the fingerprint for the four GEOMETRY-FREE fields below
        // -- only `constraint`/`detached` need a fingerprint match
        // (or an explicit override) since those two feed the solver. `tier_id`/
        // `target` join that same geometry-free group: neither feeds the solver
        // either, so both restore on a tier-count match regardless of
        // fingerprint too.
        let apply_geometry_overlay = fingerprint_matches || apply_overlay_on_mismatch;
        let raw_ids = raw_tier_ids_from_native(&native.tiers);
        let raw_targets = raw_tier_targets_from_native(&native.tiers);
        for (index, saved) in native.tiers.into_iter().enumerate() {
            let tier = &mut design.tiers[index];
            if apply_geometry_overlay {
                tier.constraint = meet_constraint_from_native(saved.constraint);
                tier.detached = saved.detached;
            }
            if let Some(note) = saved.note {
                design.tier_notes.insert(index, note);
            }
            if let Some(offset) = saved.cheater_offset_deg {
                design.cheater_offsets_deg.insert(index, offset);
            }
            if let Some(imported_meet) = saved.imported_meet {
                tier.imported_meet = Some(meet_constraint_from_native(imported_meet));
            }
            if let Some(original_notes) = saved.original_notes {
                tier.original_notes = Some(original_notes);
            }
        }
        apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets);
        match (apply_geometry_overlay, fingerprint_matches) {
            (false, _) => TierOverlay::SkippedFingerprintMismatch,
            (true, true) => TierOverlay::Applied,
            (true, false) => TierOverlay::AppliedDespiteMismatch,
        }
    };

    let material_resolution = material_resolution_of(design.material.name.as_deref());
    let restorable_custom_material = matches!(material_resolution, MaterialResolution::Unresolved)
        .then(|| native.material.custom.clone())
        .flatten();
    let history_entries = native
        .history
        .as_ref()
        .map_or_else(Vec::new, |history| history.entries.clone());
    let written_by_newer_version = native.format_version > FORMAT_VERSION;
    let printed_proportions = native.source.as_ref().map(external_proportions_from_source);

    Ok(LoadPairedResult {
        design,
        fingerprint,
        tier_overlay,
        material_resolution,
        printed_proportions,
        written_by_newer_version,
        restorable_custom_material,
        history_entries,
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

/// Collects [`Design::tier_notes`] from a draft save's own `tiers` array, keyed by
/// array position -- the draft counterpart to the non-draft branch's per-index
/// `saved.note` read in [`load_paired`] itself. Takes a reference (not ownership)
/// since the caller ([`load_paired`]) still needs to move `native_tiers` itself into
/// [`draft_tiers_from_native`] afterward.
fn tier_notes_from_native(native_tiers: &[TierTable]) -> BTreeMap<usize, String> {
    native_tiers
        .iter()
        .enumerate()
        .filter_map(|(index, saved)| saved.note.clone().map(|note| (index, note)))
        .collect()
}

/// Collects [`Design::cheater_offsets_deg`] from a draft save's own `tiers` array,
/// keyed by array position -- the draft counterpart to the non-draft branch's
/// per-index `saved.cheater_offset_deg` read in [`load_paired`] itself. Exactly
/// [`tier_notes_from_native`]'s own shape, one field over.
fn cheater_offsets_from_native(native_tiers: &[TierTable]) -> BTreeMap<usize, f64> {
    native_tiers
        .iter()
        .enumerate()
        .filter_map(|(index, saved)| saved.cheater_offset_deg.map(|offset| (index, offset)))
        .collect()
}

/// Every tier's saved [`TierTable::tier_id`], in order -- `None` for a tier the
/// file never assigned one to (a file saved before this field existed, or a
/// hand-edited one). Non-consuming, like [`tier_notes_from_native`]/
/// [`cheater_offsets_from_native`], so a caller extracts this BEFORE
/// `native_tiers` itself is consumed to build the tier list -- see
/// [`apply_tier_ids_and_targets`], which this feeds.
fn raw_tier_ids_from_native(native_tiers: &[TierTable]) -> Vec<Option<u64>> {
    native_tiers.iter().map(|t| t.tier_id).collect()
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::target`].
fn raw_tier_targets_from_native(native_tiers: &[TierTable]) -> Vec<Option<NativeTierTarget>> {
    native_tiers.iter().map(|t| t.target).collect()
}

/// Restores `design.tier_ids`/`design.tier_targets` from `raw_ids`/`raw_targets`
/// (captured via [`raw_tier_ids_from_native`]/[`raw_tier_targets_from_native`]
/// before the native file's own `tiers` array was consumed) -- without this
/// restore, the native round trip would drop every
/// [`crate::design::TierId`]/[`crate::design::TierTarget`]
/// entirely, so a reopened design's ids/targets would always be freshly (and
/// arbitrarily) reassigned by whichever constructor (`Design::new`/
/// `Design::from_asc_schedule`) built it.
///
/// Each `Some(id)` in `raw_ids` is restored verbatim; each `None` (an old file,
/// or a tier the file never recorded one for) gets a genuinely fresh id via
/// [`Design::allocate_tier_id`] instead -- "assign a fresh id only when the file
/// has none," never reusing one a DIFFERENT tier in this same file already
/// claims. `design.tier_ids` is replaced wholesale (not patched position by
/// position) since a draft/self-contained load's `design.tiers` was itself just
/// rebuilt wholesale from this exact tier list and may not even be the length
/// whatever constructor built `design` first assumed.
///
/// `raw_ids`/`raw_targets`/`design.tiers` must all be the same length, in the
/// same order -- the same "tier counts agree" precondition every other
/// position-keyed overlay field in this module already requires.
fn apply_tier_ids_and_targets(
    design: &mut Design,
    raw_ids: Vec<Option<u64>>,
    raw_targets: Vec<Option<NativeTierTarget>>,
) {
    debug_assert_eq!(design.tiers.len(), raw_ids.len());
    debug_assert_eq!(raw_ids.len(), raw_targets.len());
    let ids: Vec<TierId> = raw_ids
        .into_iter()
        .map(|maybe_id| maybe_id.map_or_else(|| design.allocate_tier_id(), TierId))
        .collect();
    design.tier_targets = raw_targets
        .into_iter()
        .zip(&ids)
        .filter_map(|(maybe_target, &id)| maybe_target.map(|t| (id, tier_target_from_native(t))))
        .collect();
    design.tier_ids = ids;
}

/// Rebuilds a draft save's full tier list purely from the native sidecar, ignoring
/// whatever the paired `.asc` (placeholder masts and all) says -- see
/// [`TierOverlay::AppliedFromDraft`] and [`load_paired`]'s own doc comment.
///
/// # Errors
///
/// [`LoadPairedError::DraftTierMissingGeometry`] for any tier missing `angle_deg`
/// or `indices` -- this tier list is a draft's ONLY source of geometry, so there is
/// no better number to fall back on -- defaulting to `0.0`/empty would silently
/// invent geometry no cutter authored.
fn draft_tiers_from_native(
    native_tiers: Vec<TierTable>,
) -> Result<Vec<ConstraintTier>, LoadPairedError> {
    native_tiers
        .into_iter()
        .enumerate()
        .map(|(index, saved)| {
            tier_from_table_with_full_geometry(saved)
                .ok_or(LoadPairedError::DraftTierMissingGeometry { index })
        })
        .collect()
}

/// The one-tier body [`draft_tiers_from_native`] and [`load_native_only`] both need:
/// `None` iff `saved` is missing `angle_deg`/`indices`, the two fields a draft (or a
/// [`load_native_only`] self-contained save -- see that function's own doc comment)
/// tier list is the SOLE source for. Factored out so the two callers can only ever
/// differ in which error they wrap this in, never in what counts as "missing."
fn tier_from_table_with_full_geometry(saved: TierTable) -> Option<ConstraintTier> {
    Some(ConstraintTier {
        angle_deg: saved.angle_deg?,
        name: saved.name,
        indices: saved.indices?,
        constraint: meet_constraint_from_native(saved.constraint),
        imported_meet: saved.imported_meet.map(meet_constraint_from_native),
        original_notes: saved.original_notes,
        detached: saved.detached,
    })
}

/// Why [`load_native_only`] could not build a [`Design`] from a native file alone.
///
/// With no paired `.asc` involved at all -- the self-contained-save analog of
/// [`LoadPairedError`].
#[derive(Debug)]
pub enum LoadNativeOnlyError {
    /// The file's own TOML text didn't parse.
    Native(NativeFormatError),
    /// This file was never written by [`super::save_native_only`] (or the
    /// equivalent): it carries no [`super::convert::unstash_schedule_meta`] entry,
    /// so there is no reliable `gear_teeth`/`symmetry_order`/`mirror` to interpret
    /// its tiers' `indices` against. An ordinary (non-draft, paired-mode) native
    /// file always hits this -- [`super::load_paired`] is the right entry point for
    /// one of those.
    NotSelfContained,
    /// A tier at this index has no recorded `angle_deg`/`indices` -- see
    /// [`LoadPairedError::DraftTierMissingGeometry`], the identical condition on the
    /// paired-load draft path.
    TierMissingGeometry { index: usize },
}

impl fmt::Display for LoadNativeOnlyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native(e) => write!(f, "native file is not valid: {e}"),
            Self::NotSelfContained => write!(
                f,
                "this native file has no paired .asc data of its own -- it can only be \
                 opened alongside the .asc file it was saved with"
            ),
            Self::TierMissingGeometry { index } => write!(
                f,
                "tier {} has no recorded angle/indices to rebuild it from",
                index + 1
            ),
        }
    }
}

impl std::error::Error for LoadNativeOnlyError {}

/// Everything [`load_native_only`] found -- the self-contained-save analog of
/// [`LoadPairedResult`], with no [`FingerprintCheck`]/[`TierOverlay`] at all (there
/// is no paired `.asc` to check either against).
#[derive(Debug)]
pub struct LoadNativeOnlyResult {
    pub design: Design,
    /// See [`MaterialResolution`] -- same meaning as [`LoadPairedResult::material_resolution`].
    pub material_resolution: MaterialResolution,
    /// See [`LoadPairedResult::printed_proportions`].
    pub printed_proportions: Option<ExternalProportions>,
    /// See [`LoadPairedResult::written_by_newer_version`].
    pub written_by_newer_version: bool,
    /// See [`LoadPairedResult::restorable_custom_material`].
    pub restorable_custom_material: Option<CustomMaterialSnapshot>,
    /// See [`LoadPairedResult::history_entries`].
    pub history_entries: Vec<String>,
}

/// Loads a [`Design`] from a self-contained native file alone.
///
/// No paired `.asc` text needed, or even present on disk -- autosave restore must
/// not depend on a sidecar that may not exist.
///
/// Only accepts a file [`super::save_native_only`] (or a hand-built equivalent)
/// actually wrote: [`native.tiers`](indicatrix_formats::native::NativeDesignFile::tiers)
/// must carry every tier's real `angle_deg`/`indices` (exactly
/// [`super::save::draft_tier_tables`]'s own full-field shape -- an ordinary,
/// overlay-only native file, `native.draft == false`, never has these) AND the file
/// must carry a [`super::convert::unstash_schedule_meta`] entry (an ordinary native
/// file never does either, since `ScheduleMeta` normally lives only in the paired
/// `.asc`). A file missing either is almost certainly an ordinary paired-mode
/// native file -- reported as [`LoadNativeOnlyError::NotSelfContained`] rather than
/// silently reconstructing a design with fabricated `gear_teeth`/`symmetry_order`
/// defaults that would misinterpret every tier's `indices`.
///
/// # Errors
///
/// See [`LoadNativeOnlyError`].
pub fn load_native_only(native_text: &str) -> Result<LoadNativeOnlyResult, LoadNativeOnlyError> {
    let native = from_toml_str(native_text).map_err(LoadNativeOnlyError::Native)?;
    let meta =
        unstash_schedule_meta(&native.unknown).ok_or(LoadNativeOnlyError::NotSelfContained)?;

    let tier_notes = tier_notes_from_native(&native.tiers);
    let cheater_offsets = cheater_offsets_from_native(&native.tiers);
    let raw_ids = raw_tier_ids_from_native(&native.tiers);
    let raw_targets = raw_tier_targets_from_native(&native.tiers);
    let preform = preform_spec_from_table(&native.preform);
    let tiers = native
        .tiers
        .into_iter()
        .enumerate()
        .map(|(index, saved)| {
            tier_from_table_with_full_geometry(saved)
                .ok_or(LoadNativeOnlyError::TierMissingGeometry { index })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut design = Design::new(preform, meta, tiers);
    design.girdle_diameter_mm = native.girdle_diameter_mm;
    design.preform_y_offset = native.preform.y_offset;
    design.material = material_selection_from_table(&native.material);
    design.tier_notes = tier_notes;
    design.cheater_offsets_deg = cheater_offsets;
    apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets);
    // See `load_paired`'s matching comment.
    if let Some(authored) = native.authored_refractive_index {
        design.meta.refractive_index = authored;
    }

    let material_resolution = material_resolution_of(design.material.name.as_deref());
    let restorable_custom_material = matches!(material_resolution, MaterialResolution::Unresolved)
        .then(|| native.material.custom.clone())
        .flatten();
    let history_entries = native
        .history
        .as_ref()
        .map_or_else(Vec::new, |history| history.entries.clone());
    let written_by_newer_version = native.format_version > FORMAT_VERSION;
    let printed_proportions = native.source.as_ref().map(external_proportions_from_source);

    Ok(LoadNativeOnlyResult {
        design,
        material_resolution,
        printed_proportions,
        written_by_newer_version,
        restorable_custom_material,
        history_entries,
    })
}

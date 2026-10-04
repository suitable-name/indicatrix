//! [`load_paired`]: turning a native file's TOML text plus its paired `.asc`
//! text into a real [`Design`], and the [`TierOverlay`]/[`LoadPairedError`]/
//! [`LoadPairedResult`] types around it. See the parent module's doc comment
//! for why `.asc` stays canonical for geometry regardless of what the native
//! file says.

use super::convert::{
    check_concave_flat_fingerprint, concave_tier_from_table, external_proportions_from_source,
    material_selection_from_table, meet_constraint_from_native, preform_spec_from_table,
    tier_target_from_native, unstash_concave_tiers, unstash_schedule_meta,
};
use crate::design::{ConstraintTier, Design, TierId};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::{
    ConcaveTierTable, CustomMaterialSnapshot, FORMAT_VERSION, FingerprintCheck, NativeFormatError,
    NativeTierTarget, TierTable, check_fingerprint, from_toml_str,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

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
    /// The sidecar's stashed concave tiers were saved against a different flat
    /// schedule than the one just loaded (see [`LoadNativeOnlyError::ConcaveStale`]).
    ConcaveStale {
        /// What changed.
        reason: String,
    },
    /// The sidecar's stashed concave tier at this position cannot be used (an
    /// unknown tool, a malformed record, or a value that fails validation).
    ConcaveTier {
        /// Zero-based position among the concave tiers.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for LoadPairedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native(e) => write!(f, "sidecar is not valid: {e}"),
            Self::Asc(e) => write!(f, "paired .asc file is not valid: {e}"),
            Self::DraftTierMissingGeometry { index } => write!(
                f,
                "draft sidecar's tier {} has no recorded angle/indices to rebuild it from",
                index + 1
            ),
            Self::ConcaveStale { reason } => write!(f, "sidecar's concave tiers: {reason}"),
            Self::ConcaveTier { index, reason } => {
                write!(
                    f,
                    "sidecar's concave tier {} is not usable: {reason}",
                    index + 1
                )
            }
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
    /// The loaded design.
    pub design: Design,
    /// Result of checking the sidecar against the paired `.asc`.
    pub fingerprint: FingerprintCheck,
    /// Per-tier data restored from the sidecar.
    pub tier_overlay: TierOverlay,
    /// See [`MaterialResolution`] -- whether `design.material.name` is likely to
    /// still mean what it meant when this file was saved.
    pub material_resolution: MaterialResolution,
    /// The printed/measured proportions this design was loaded against, if the
    /// sidecar's `[source]` table has one -- `None` for a sidecar saved
    /// before that table existed, or one for a design that was never associated with
    /// a catalogue row at all. A caller restores this into its own equivalent of
    /// `EditorState::printed_proportions` so Deep Solve has something to verify
    /// against after a Save/Open round trip, not just on the design's
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
/// (regardless of fingerprint), the per-tier fields that feed nothing but display --
/// [`indicatrix_formats::native::TierTable::name`]/`note`/`imported_meet`/
/// `original_notes` -- since array-position correlation is all any of those four
/// need (none feeds the solver or the plane arrangement, so a changed `.asc` can't
/// make them wrong either, only stale); `name`/`note` present in the file overwrite
/// whatever `.asc` import produced, while `imported_meet`/`original_notes` are
/// assigned exactly as saved, `None` included, so a clean save/load/save round trip
/// stays byte-identical. Finally the per-tier fields that DO feed geometry --
/// `constraint`/`detached`, `indices` (a cheater offset is baked into a tier's
/// exported indices, not carried as a separate `.asc` field at all -- see
/// [`Design::to_asc_schedule_from_solved_with_cheater_offsets`]), `cheater_offset_deg`
/// itself, and [`crate::design::TierTarget`] -- apply ONLY when tier counts agree
/// AND either [`check_fingerprint`] reports [`FingerprintCheck::Match`] or the
/// caller passed `apply_overlay_on_mismatch: true` (see [`TierOverlay`]): `tiers[i]`'s
/// geometry is only meaningful paired with the `i`-th tier of the EXACT `.asc`
/// content it was saved against, which is exactly what a fingerprint match
/// promises; `true` lets a caller (e.g. the editor, after the cutter confirms they
/// still want it) apply it anyway. Restoring `indices`/`cheater_offset_deg`
/// unconditionally, instead of gating them the same way, would silently re-shift
/// the wrong facet on a fingerprint mismatch, and double the shift on a match.
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
        // A draft sidecar's tier list is the design's ONLY source of state --
        // there is no fingerprint concept to gate on (the paired `.asc` is a
        // placeholder), so its own recorded target always applies.
        apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets, true);
        TierOverlay::AppliedFromDraft
    } else if native_tier_count != design.tiers.len() {
        TierOverlay::SkippedTierCountMismatch {
            native_tiers: native_tier_count,
            asc_tiers: design.tiers.len(),
        }
    } else {
        // The tier counts agree, so array-position correlation is trustworthy
        // regardless of the fingerprint for the tier's IDENTITY (`tier_id`) and
        // the fields that feed nothing but display (`name`/`note`/
        // `imported_meet`/`original_notes`) -- none of those needs anything more
        // than "this is the same tier slot". Everything that DOES feed geometry
        // -- `constraint`/`detached`, `indices`, `cheater_offset_deg` and
        // `TierTarget` -- only restores when `apply_geometry_overlay` holds; see
        // this function's own doc comment for why.
        let apply_geometry_overlay = fingerprint_matches || apply_overlay_on_mismatch;
        let raw_ids = raw_tier_ids_from_native(&native.tiers);
        let raw_targets = raw_tier_targets_from_native(&native.tiers);
        for (index, saved) in native.tiers.into_iter().enumerate() {
            let tier = &mut design.tiers[index];
            // Restored whenever the tier counts agree, regardless of
            // fingerprint -- a name is not geometry, so a changed `.asc` cannot
            // make it wrong, only stale.
            tier.name = saved.name;
            if apply_geometry_overlay {
                tier.constraint = meet_constraint_from_native(saved.constraint);
                tier.detached = saved.detached;
                if let Some(indices) = saved.indices {
                    tier.indices = indices;
                }
                if let Some(offset) = saved.cheater_offset_deg {
                    design.cheater_offsets_deg.insert(index, offset);
                }
            }
            if let Some(note) = saved.note {
                design.tier_notes.insert(index, note);
            }
            // Assigned exactly as saved -- `None` included -- so a clean
            // save/load/save round trip is byte-identical instead of leaving
            // whatever `.asc` import produced (construct.rs's own
            // `Design::from_asc_schedule` always sets `original_notes` to
            // `Some`) in place just because the file happened to carry `None`.
            tier.imported_meet = saved.imported_meet.map(meet_constraint_from_native);
            tier.original_notes = saved.original_notes;
        }
        apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets, apply_geometry_overlay);
        match (apply_geometry_overlay, fingerprint_matches) {
            (false, _) => TierOverlay::SkippedFingerprintMismatch,
            (true, true) => TierOverlay::Applied,
            (true, false) => TierOverlay::AppliedDespiteMismatch,
        }
    };

    // The `.asc` cannot carry concave tiers; the sidecar stashes them (see
    // `stash_concave_tiers`), so they come back from there regardless of whether
    // the tier overlay applied: they have no `.asc` counterpart to disagree with.
    let stashed = unstash_concave_tiers(&native.unknown)
        .map_err(|(index, reason)| LoadPairedError::ConcaveTier { index, reason })?;
    check_concave_flat_fingerprint(&native.unknown, &design)
        .map_err(|reason| LoadPairedError::ConcaveStale { reason })?;
    restore_concave_tiers(&mut design, stashed)
        .map_err(|(index, reason)| LoadPairedError::ConcaveTier { index, reason })?;
    if !design.concave_tiers.is_empty() {
        // The `.asc` an earlier save exported carries the concave tiers as footnotes
        // (see `Design::append_concave_footnotes`); the sidecar's tiers are the
        // truth, and a re-export regenerates them, so the stale lines go now.
        crate::design::strip_generated_concave_footnotes(&mut design.meta.footnotes);
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
///
/// Exact, case-insensitive match only (via
/// [`crate::material::built_in_material_by_exact_name`]) -- NOT
/// [`indicatrix::optics::materials::GemMaterial::by_name`]'s own substring
/// fallback, which would report a name like "My Blue Sapphire" as `Known` (and
/// silently write Sapphire's own RI to a re-exported `.asc`'s `I` line) just
/// because it CONTAINS a built-in name.
pub(super) fn material_resolution_of(name: Option<&str>) -> MaterialResolution {
    match name {
        None => MaterialResolution::NoneSelected,
        Some(name) if crate::material::built_in_material_by_exact_name(name).is_some() => {
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
pub(super) fn tier_notes_from_native(native_tiers: &[TierTable]) -> BTreeMap<usize, String> {
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
pub(super) fn cheater_offsets_from_native(native_tiers: &[TierTable]) -> BTreeMap<usize, f64> {
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
pub(super) fn raw_tier_ids_from_native(native_tiers: &[TierTable]) -> Vec<Option<u64>> {
    native_tiers.iter().map(|t| t.tier_id).collect()
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::target`].
pub(super) fn raw_tier_targets_from_native(
    native_tiers: &[TierTable],
) -> Vec<Option<NativeTierTarget>> {
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
/// Each `Some(id)` in `raw_ids` is restored verbatim -- EXCEPT a duplicate (the
/// same id named by more than one position in this file, never produced by this
/// crate's own save path but not rejected by the file format either), where only
/// the FIRST occurrence keeps it and every later one gets a fresh id instead, the
/// same as a tier the file never recorded one for at all -- two tiers sharing a
/// [`TierId`] would make [`Design::index_of_tier_id`]/`design.tier_targets`
/// (keyed by id, not position) silently pick the wrong tier. Before any of that,
/// [`Design::next_tier_id`] is bumped past the highest id this file restores --
/// `Design::from_asc_schedule`/`Design::new` only ever set it to the fresh
/// design's own tier count, which is easily behind a restored id (e.g. a design
/// whose tiers were added and removed many times before this save), and without
/// this bump [`Design::allocate_tier_id`] could hand out an id a restored tier
/// already claims the very next time a tier is added.
///
/// `design.tier_ids` is replaced wholesale (not patched position by position)
/// since a draft/self-contained load's `design.tiers` was itself just rebuilt
/// wholesale from this exact tier list and may not even be the length whatever
/// constructor built `design` first assumed.
///
/// `design.tier_targets` is restored from `raw_targets` only when `apply_targets`
/// is `true` -- a draft/self-contained load (no fingerprint concept at all) and a
/// clean-fingerprint paired load always pass `true`; a paired load with a
/// mismatched fingerprint (and no override) passes `false`, since a
/// [`crate::design::TierTarget`] feeds the solver exactly like `constraint` does
/// and is only meaningful paired with the exact `.asc` content it was saved
/// against -- see [`load_paired`]'s own doc comment.
///
/// `raw_ids`/`raw_targets`/`design.tiers` must all be the same length, in the
/// same order -- the same "tier counts agree" precondition every other
/// position-keyed overlay field in this module already requires.
pub(super) fn apply_tier_ids_and_targets(
    design: &mut Design,
    raw_ids: Vec<Option<u64>>,
    raw_targets: Vec<Option<NativeTierTarget>>,
    apply_targets: bool,
) {
    debug_assert_eq!(design.tiers.len(), raw_ids.len());
    debug_assert_eq!(raw_ids.len(), raw_targets.len());

    if let Some(max_restored) = raw_ids.iter().filter_map(|id| *id).max() {
        design.next_tier_id = design.next_tier_id.max(max_restored + 1);
    }

    let mut seen = BTreeSet::new();
    let ids: Vec<TierId> = raw_ids
        .into_iter()
        .map(|maybe_id| {
            maybe_id
                .filter(|id| seen.insert(*id))
                .map_or_else(|| design.allocate_tier_id(), TierId)
        })
        .collect();
    design.tier_targets = if apply_targets {
        raw_targets
            .into_iter()
            .zip(&ids)
            .filter_map(|(maybe_target, &id)| {
                maybe_target.map(|t| (id, tier_target_from_native(t)))
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    design.tier_ids = ids;
}

/// Installs concave tiers read from a file onto `design`: converts each record,
/// validates the lot against the design's gear and flat tier names, and gives each
/// a fresh stable id (ids are regenerated on load, never stored -- plan §6.1).
///
/// The tiers are only installed when all of them pass, so a design is never left
/// half-loaded. An empty `tables` is a no-op.
///
/// # Errors
///
/// The position of the first unusable tier and why.
pub(super) fn restore_concave_tiers(
    design: &mut Design,
    tables: Vec<ConcaveTierTable>,
) -> Result<(), (usize, String)> {
    if tables.is_empty() {
        return Ok(());
    }
    design.concave_tiers = tables
        .into_iter()
        .enumerate()
        .map(|(index, table)| concave_tier_from_table(table).map_err(|reason| (index, reason)))
        .collect::<Result<_, _>>()?;
    if let Err((index, error)) = design.validate_concave_tiers() {
        design.concave_tiers.clear();
        return Err((index, error.to_string()));
    }
    design.concave_tier_ids.clear();
    design.ensure_concave_tier_ids();
    Ok(())
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
pub(super) fn tier_from_table_with_full_geometry(saved: TierTable) -> Option<ConstraintTier> {
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
    /// The stashed concave tiers were saved against a different flat schedule: an
    /// older build edited the flat tiers and re-saved the stash verbatim.
    ConcaveStale {
        /// What changed.
        reason: String,
    },
    /// See [`LoadPairedError::ConcaveTier`], the identical condition on the
    /// paired-load path.
    ConcaveTier {
        /// Zero-based position among the concave tiers.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for LoadNativeOnlyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native(e) => write!(f, "sidecar is not valid: {e}"),
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
            Self::ConcaveStale { reason } => write!(f, "concave tiers: {reason}"),
            Self::ConcaveTier { index, reason } => {
                write!(f, "concave tier {} is not usable: {reason}", index + 1)
            }
        }
    }
}

impl std::error::Error for LoadNativeOnlyError {}

/// Everything [`load_native_only`] found.
///
/// The self-contained-save analog of [`LoadPairedResult`], with no [`FingerprintCheck`]/[`TierOverlay`] at all (there
/// is no paired `.asc` to check either against).
#[derive(Debug)]
pub struct LoadNativeOnlyResult {
    /// The loaded design.
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
    // A self-contained save has no fingerprint concept at all (no paired `.asc`
    // to check against) -- its own recorded target always applies.
    apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets, true);
    // See `load_paired`'s matching comment.
    if let Some(authored) = native.authored_refractive_index {
        design.meta.refractive_index = authored;
    }
    let stashed = unstash_concave_tiers(&native.unknown)
        .map_err(|(index, reason)| LoadNativeOnlyError::ConcaveTier { index, reason })?;
    check_concave_flat_fingerprint(&native.unknown, &design)
        .map_err(|reason| LoadNativeOnlyError::ConcaveStale { reason })?;
    restore_concave_tiers(&mut design, stashed)
        .map_err(|(index, reason)| LoadNativeOnlyError::ConcaveTier { index, reason })?;

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

//! [`save_paired`]: building the pair of files a "Save" action should write,
//! and the [`SaveError`]/[`PairedSave`] types around it. See the parent
//! module's doc comment ("Preserving the original `.asc` text") for the
//! preserve-vs-regenerate decision this makes.

use super::convert::{
    SaveExtras, material_table_from_selection, native_meet_constraint_from,
    native_tier_target_from, preform_table_from_spec, source_table_from_proportions,
    stash_schedule_meta, to_native_file,
};
use crate::design::{Design, DesignSolveError};
use indicatrix::{
    geometry::{
        meet_solver::{SolveStrategy, SolvedTier},
        stone_metrics::ExternalProportions,
    },
    optics::materials::GemMaterial,
};
use indicatrix_formats::{
    asc::{AscSchedule, AscTier},
    native::{
        HistoryTable, NativeDesignFile, NativeFormatError, TierTable, sha256_hex, to_toml_string,
    },
};
use std::fmt;

/// Why [`save_paired`] could not produce a pair of files to write.
///
/// Not raised for `design` not currently solving -- see [`save_paired`]'s doc
/// comment: that case falls through to a draft save ([`PairedSave::draft_reason`])
/// instead of an error.
#[derive(Debug)]
pub enum SaveError {
    /// The (always-succeeds-in-practice, see [`to_toml_string`]'s own doc comment)
    /// TOML serialization step failed.
    Toml(NativeFormatError),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(e) => write!(f, "cannot save: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Why [`save_paired`] wrote a draft instead of an ordinary save -- see
/// [`PairedSave::draft_reason`].
#[derive(Debug)]
pub enum DraftReason {
    /// `design.solve()` failed: at least one crown/pavilion/girdle block has no
    /// explicit scale-reference tier (so its masts are genuinely undetermined), or
    /// a [`crate::design::TierTarget`] could not be resolved. See
    /// [`DesignSolveError`].
    Unsolved(DesignSolveError),
    /// `design.tiers` is empty. There is no [`DesignSolveError`] here -- a tier-less
    /// design's [`Design::to_asc_schedule`] actually succeeds, trivially, with zero
    /// facet records -- but `indicatrix_formats::asc::parse_asc` refuses to parse any
    /// `.asc` with no `a` (facet) records at all, so an ordinary save would write a
    /// file [`super::load::load_paired`] could never reopen. Handled the same way as
    /// [`Self::Unsolved`]: the native sidecar carries the (empty) tier list directly
    /// and the paired `.asc` is a placeholder with one dummy facet record, present
    /// only so the file parses at all.
    NoTiers,
}

impl fmt::Display for DraftReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsolved(e) => write!(f, "unsolved: {e}"),
            Self::NoTiers => write!(
                f,
                "no tiers yet -- add at least one (with a scale-reference anchor) before this \
                 file can be reopened"
            ),
        }
    }
}

impl std::error::Error for DraftReason {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsolved(e) => Some(e),
            Self::NoTiers => None,
        }
    }
}

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
    /// the text itself. Always `false` for a draft save (see
    /// [`Self::draft_reason`]) -- a placeholder-mast `.asc` is never the caller's own
    /// preserved text.
    pub asc_preserved: bool,
    pub native: NativeDesignFile,
    pub native_toml: String,
    /// `Some(reason)` iff this save fell back to a draft instead of an ordinary save
    /// -- either `design` did not currently solve (see [`Design::solve`]) or
    /// `design.tiers` was empty (see [`DraftReason::NoTiers`]). Either way `asc_text`
    /// is placeholder-mast `.asc` text (never a real cut instruction -- see
    /// [`indicatrix_formats::native::NativeDesignFile::draft`]'s own doc comment), and
    /// `native`'s tier list carries every `ConstraintTier` field in full, not just
    /// the usual overlay subset, so [`super::load::load_paired`] can rebuild
    /// `design.tiers` without the placeholder `.asc` at all. `None` for an ordinary
    /// save. A caller uses this to report e.g. "saved as draft ({reason})" rather
    /// than the ordinary saved-successfully toast.
    pub draft_reason: Option<DraftReason>,
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
/// `printed_proportions`, when `Some`, is mirrored into the sidecar's own `[source]`
/// table (an ordinary save via [`to_native_file`], or a draft save via
/// [`NativeDesignFile::with_source`] directly) -- see [`super::convert::to_native_file`]'s
/// own doc comment. Carried into a draft save too: a draft is exactly the "in progress,
/// not yet re-solving" state a cutter is most likely to close and reopen before
/// finishing, and Deep Solve's own verification only needs the printed figures, never
/// the current (possibly still-broken) geometry.
///
/// `placeholder_note`, when `Some`, marks the written `.asc` (via
/// [`indicatrix_formats::asc::mark_reconstructed`]) as derived rather than authored --
/// use this when `design` itself came from a placeholder reconstruction (e.g. a
/// catalogue entry with no attached `.asc`, only an angle table, so every mast is a
/// fabricated `0.0`) so the file this writes can never be mistaken for a real,
/// verified cutting schedule. Skips the preserve-original-text path entirely when
/// set: a placeholder-derived design's masts are never worth preserving verbatim even
/// if they happen to already be marked.
///
/// # Unsolved or tier-less designs: draft saves
///
/// Two cases produce draft saves instead of failures: `design` not currently solving
/// (see [`Design::solve`] -- a block missing its one required
/// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] tier, the
/// ordinary state of an in-progress crown with no anchor authored yet), and
/// `design.tiers` being empty (a brand-new design before its first tier -- see
/// [`DraftReason::NoTiers`]: `to_asc_schedule` actually succeeds here, but the result
/// has no facet records at all, which `indicatrix_formats::asc::parse_asc` refuses to
/// parse back). Either way [`PairedSave::draft_reason`] is `Some(reason)`, `asc_text`
/// is a placeholder (real angle/indices/index-wheel positions for the unsolved case,
/// via [`draft_asc_schedule`]; one dummy facet record for the tier-less case, via
/// [`draft_asc_schedule_for_no_tiers`]) never to be trusted as a cut instruction, and
/// `native`'s tier list carries every `ConstraintTier` field, not just the usual
/// overlay subset, via [`NativeDesignFile::with_draft`] -- see
/// [`super::load::load_paired`]'s own doc comment for the reload side of this.
///
/// # Errors
///
/// [`SaveError::Toml`] if serializing the resulting [`NativeDesignFile`] fails (in
/// practice unreachable -- see that variant's own doc comment).
pub fn save_paired(
    design: &Design,
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    printed_proportions: Option<&ExternalProportions>,
) -> Result<PairedSave, SaveError> {
    save_paired_extended(
        design,
        asc_filename,
        original_asc_text,
        placeholder_note,
        printed_proportions,
        &SaveExtras::default(),
    )
}

/// Like [`save_paired`], but also attaches extra sidecar data.
///
/// `extras`' custom-material snapshot, history trail and resolved catalogue (see
/// [`SaveExtras::custom_catalogue`], the wrong-`I`-line fix) get attached to the
/// resulting sidecar/schedule -- see [`SaveExtras`]'s own doc comment for why this is
/// a separate function rather than a wider [`save_paired`] itself.
/// `SaveExtras::default()` (`custom_catalogue: &[]` included) reproduces
/// [`save_paired`]'s own built-ins-only behaviour exactly.
///
/// # Errors
///
/// Same as [`save_paired`].
pub fn save_paired_extended(
    design: &Design,
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<PairedSave, SaveError> {
    let asc_filename = asc_filename.into();

    if design.tiers.is_empty() {
        return draft_save(
            design,
            asc_filename,
            &draft_asc_schedule_for_no_tiers(design, extras.custom_catalogue),
            DraftReason::NoTiers,
            printed_proportions,
            extras,
        );
    }

    match design.solve() {
        Ok(solved) => save_paired_extended_from_solved(
            design,
            &solved,
            asc_filename,
            original_asc_text,
            placeholder_note,
            printed_proportions,
            extras,
        ),
        Err(missing_anchor) => draft_save(
            design,
            asc_filename,
            &draft_asc_schedule(design, original_asc_text, extras.custom_catalogue),
            DraftReason::Unsolved(missing_anchor),
            printed_proportions,
            extras,
        ),
    }
}

/// [`save_paired_extended`]'s "design currently solves" half.
///
/// Takes an already-[`Design::solve`]'d (or [`Design::resolve_dirty`]'d) mast list
/// instead of solving `design` again -- a caller that already solved
/// once (e.g. `apps/indicatrix-cut`'s `confirm_status_before_write`, which must
/// solve anyway to report a save-time status) must not pay for a second
/// multi-second solve just to write the file.
///
/// Uses [`Design::to_asc_schedule_from_solved_with`] (`extras.custom_catalogue`,
/// see [`SaveExtras::custom_catalogue`]'s own doc comment) where
/// [`save_paired_extended`] uses [`Design::to_asc_schedule`] -- everything else
/// (the placeholder-note marking, the preserve-original-text comparison, building
/// the native sidecar) is identical, so the two functions can never drift apart in
/// behaviour for a design that solves.
///
/// Only handles that one case: the tier-less and does-not-solve draft branches stay
/// in [`save_paired_extended`], which calls this exactly once it has a `solved` list
/// in hand.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same order
/// -- see [`Design::to_asc_schedule_from_solved`]'s own panic contract, which this
/// inherits unchanged.
///
/// # Errors
///
/// Same as [`save_paired_extended`].
pub fn save_paired_extended_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<PairedSave, SaveError> {
    let asc_filename = asc_filename.into();
    let mut current_schedule =
        design.to_asc_schedule_from_solved_with(solved, extras.custom_catalogue);

    if let Some(note) = placeholder_note {
        indicatrix_formats::asc::mark_reconstructed(&mut current_schedule, note);
        let asc_text = indicatrix_formats::asc::to_asc_string(&current_schedule);
        let native = to_native_file(
            design,
            asc_filename,
            asc_text.as_bytes(),
            printed_proportions,
            extras,
        );
        let native_toml = to_toml_string(&native).map_err(SaveError::Toml)?;
        return Ok(PairedSave {
            asc_text,
            asc_preserved: false,
            native,
            native_toml,
            draft_reason: None,
        });
    }

    let preserved = original_asc_text.and_then(|original| {
        let original_schedule = indicatrix_formats::asc::parse_asc(original).ok()?;
        schedules_equal_ignoring_refractive_index(&original_schedule, &current_schedule)
            .then(|| original.to_string())
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

    let native = to_native_file(
        design,
        asc_filename,
        asc_text.as_bytes(),
        printed_proportions,
        extras,
    );
    let native_toml = to_toml_string(&native).map_err(SaveError::Toml)?;

    Ok(PairedSave {
        asc_text,
        asc_preserved,
        native,
        native_toml,
        draft_reason: None,
    })
}

/// `true` iff `original` and `current` are equal once `current`'s own
/// `refractive_index` is replaced with `original`'s -- the preservation test
/// [`save_paired_extended`] actually applies, so that picking a different
/// material alone (which changes [`Design::effective_refractive_index`], and
/// therefore [`Design::to_asc_schedule`]'s `refractive_index` field, without
/// touching any tier/preform data `.asc` itself carries) does not, by itself,
/// force a fresh export of a hand-authored file. The sidecar's own `[material]`
/// table already records the real selection (see [`super::convert::to_native_file`]),
/// so the exported `.asc`'s `I` line staying at the file's original value costs
/// nothing -- it is not lost, only not rewritten. See the parent module's doc
/// comment ("Preserving the original `.asc` text").
#[must_use]
fn schedules_equal_ignoring_refractive_index(
    original: &AscSchedule,
    current: &AscSchedule,
) -> bool {
    let masked_current = AscSchedule {
        refractive_index: original.refractive_index,
        ..current.clone()
    };
    *original == masked_current
}

/// The shared tail of [`save_paired_extended`]'s two draft paths (unsolved,
/// tier-less): builds `asc_text` from `draft_schedule` (marking it reconstructed
/// too, when the design itself is placeholder-derived, is deliberately NOT done
/// here -- a draft's `.asc` is already self-evidently a placeholder via
/// [`NativeDesignFile::draft`] and its own made-up masts, so `placeholder_note` is
/// not threaded into this path), and the native sidecar with every
/// `ConstraintTier` field intact.
fn draft_save(
    design: &Design,
    asc_filename: String,
    draft_schedule: &AscSchedule,
    reason: DraftReason,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<PairedSave, SaveError> {
    let asc_text = indicatrix_formats::asc::to_asc_string(draft_schedule);

    let material = material_table_from_selection(&design.material)
        .with_custom(extras.custom_material.cloned());
    let mut native = NativeDesignFile::new(
        asc_filename,
        sha256_hex(asc_text.as_bytes()),
        preform_table_from_spec(&design.preform, design.preform_y_offset),
        design.girdle_diameter_mm,
        material,
        draft_tier_tables(design),
    )
    .with_draft(true)
    .with_history(HistoryTable::new(extras.history_entries.to_vec()))
    .with_authored_refractive_index(Some(design.meta.refractive_index));
    if let Some(props) = printed_proportions {
        native = native.with_source(source_table_from_proportions(props));
    }
    let native_toml = to_toml_string(&native).map_err(SaveError::Toml)?;

    Ok(PairedSave {
        asc_text,
        asc_preserved: false,
        native,
        native_toml,
        draft_reason: Some(reason),
    })
}

/// The placeholder mast a draft save writes for a tier with no better number to fall
/// back on -- see [`draft_asc_schedule`]. Mirrors, in spirit only, the meet solver's
/// own last-resort "no scale reference anywhere" default: not a real dimension
/// either way, always paired with [`SolveStrategy::Failed`] (whose own doc comment
/// already says the mast it carries "should not be trusted"), never shown to a
/// cutter as a cut instruction.
const DRAFT_PLACEHOLDER_MAST: f64 = 1.0;

/// Builds the placeholder-mast [`AscSchedule`] a draft save writes as `asc_text`,
/// real angles/names/indices/index-wheel positions and all -- only the masts (and
/// therefore the notes `constraint_notes` derives a `ScaleReference` tier's text
/// from) are not to be trusted. Per tier, prefers `original_asc_text`'s own
/// previously-recorded mast at the same position when that text still parses AND
/// still has exactly `design`'s current tier count (the same alignment precondition
/// [`Design::to_asc_schedule_from_solved`] documents), falling back to
/// [`DRAFT_PLACEHOLDER_MAST`] for any tier without one -- e.g. a brand-new tier
/// added after the block lost its anchor.
///
/// `custom` -- a caller's own resolved catalogue materials -- is consulted for the
/// written `I` line exactly like [`Design::to_asc_schedule_from_solved_with`]; an
/// empty slice behaves exactly like [`Design::to_asc_schedule_from_solved`]'s own
/// built-ins-only resolution.
fn draft_asc_schedule(
    design: &Design,
    original_asc_text: Option<&str>,
    custom: &[GemMaterial],
) -> AscSchedule {
    let original_masts = original_asc_text
        .and_then(|text| indicatrix_formats::asc::parse_asc(text).ok())
        .filter(|schedule| schedule.tiers.len() == design.tiers.len())
        .map(|schedule| {
            schedule
                .tiers
                .into_iter()
                .map(|tier| tier.mast)
                .collect::<Vec<_>>()
        });

    let placeholder_solved: Vec<SolvedTier> = (0..design.tiers.len())
        .map(|i| SolvedTier {
            mast: original_masts
                .as_ref()
                .and_then(|masts| masts.get(i).copied())
                .unwrap_or(DRAFT_PLACEHOLDER_MAST),
            strategy: SolveStrategy::Failed,
            detail: "draft save: design does not currently solve".to_string(),
        })
        .collect();

    design.to_asc_schedule_from_solved_with(&placeholder_solved, custom)
}

/// A placeholder facet name/mast for the ONE dummy record
/// [`draft_asc_schedule_for_no_tiers`] writes -- see that function's own doc comment
/// for why a tier-less design needs one at all.
const NO_TIERS_PLACEHOLDER_NAME: &str = "placeholder";

/// Builds the placeholder [`AscSchedule`] a tier-less draft save writes as
/// `asc_text`. `design.tiers` is empty, so unlike [`draft_asc_schedule`] there is no
/// real angle/index/name data to carry over -- this exists purely so
/// `indicatrix_formats::asc::parse_asc` (which refuses to parse any file with no `a`
/// facet records at all) can still open the file; [`super::load::load_paired`] never
/// reads this dummy record back, since `native.draft` being `true` with an EMPTY
/// `tiers` array rebuilds `design.tiers` as empty too.
///
/// `custom` is consulted for the written `I` line exactly like
/// [`Design::effective_refractive_index_with`]; an empty slice behaves exactly like
/// [`Design::effective_refractive_index`]'s own built-ins-only resolution.
fn draft_asc_schedule_for_no_tiers(design: &Design, custom: &[GemMaterial]) -> AscSchedule {
    AscSchedule {
        gemcad_version: design.meta.gemcad_version.clone(),
        gear_teeth: design.meta.gear_teeth,
        gear_reference_angle: design.meta.gear_reference_angle,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
        refractive_index: design.effective_refractive_index_with(custom),
        headers: design.meta.headers.clone(),
        footnotes: design.meta.footnotes.clone(),
        tiers: vec![AscTier {
            angle_deg: 0.0,
            mast: DRAFT_PLACEHOLDER_MAST,
            name: NO_TIERS_PLACEHOLDER_NAME.to_string(),
            indices: Vec::new(),
            notes: "draft save: design has no tiers yet".to_string(),
        }],
    }
}

/// Builds a draft save's FULL per-tier native record -- every `ConstraintTier`
/// field, not just the usual overlay subset -- so [`super::load::load_paired`] can
/// rebuild `design.tiers` on reload without trusting [`draft_asc_schedule`]'s
/// placeholder masts (or even its `angle_deg`/`indices`, though those happen to
/// still be real) at all. See [`indicatrix_formats::native::TierTable`]'s own doc
/// comment.
///
/// Also carries `design.tier_notes`/`design.cheater_offsets_deg` per tier position,
/// the same
/// [`crate::design::Design::tier_notes`]/[`crate::design::Design::cheater_offsets_deg`]
/// lookups `super::convert::to_native_file` already does for an ordinary
/// (non-draft) save -- a draft save that dropped either would silently lose a
/// cutter's note or cheater offset the moment a design without a scale-reference
/// anchor got saved once.
fn draft_tier_tables(design: &Design) -> Vec<TierTable> {
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            TierTable::new(
                tier.name.clone(),
                native_meet_constraint_from(&tier.constraint),
                tier.detached.clone(),
            )
            .with_angle_deg(Some(tier.angle_deg))
            .with_indices(Some(tier.indices.clone()))
            .with_imported_meet(tier.imported_meet.as_ref().map(native_meet_constraint_from))
            .with_original_notes(tier.original_notes.clone())
            .with_note(design.tier_notes.get(&index).cloned())
            .with_cheater_offset_deg(design.cheater_offsets_deg.get(&index).copied())
            .with_tier_id(design.tier_id_at(index).map(crate::design::TierId::value))
            .with_target(design.tier_target(index).map(native_tier_target_from))
        })
        .collect()
}

/// A placeholder [`NativeDesignFile::asc_sha256`] for [`save_native_only`]'s own
/// sidecar-free save -- there is no paired `.asc` for this value to fingerprint at
/// all (see that function's own doc comment), so this is never compared against
/// anything; [`super::load::load_native_only`] never calls
/// [`indicatrix_formats::native::check_fingerprint`].
const NO_PAIRED_ASC_SHA256: &str = "(no paired .asc: self-contained save)";

/// Builds a self-contained native sidecar carrying the FULL design.
///
/// This means every tier's real
/// `angle_deg`/`indices` (via [`draft_tier_tables`], the same full-field
/// builder [`save_paired_extended`]'s own draft branch uses -- see that
/// function's doc comment) regardless of whether `design` currently solves,
/// plus `design.meta` itself ([`stash_schedule_meta`], since ordinarily that
/// lives only in the paired `.asc` this save has none of) -- so
/// [`super::load::load_native_only`] can rebuild `design` from this ONE file
/// alone, with no `.asc` sidecar present or even ever having existed.
///
/// Marked [`NativeDesignFile::draft`] `true` unconditionally: not because `design`
/// necessarily fails to solve (it may solve fine), but because that flag already
/// means exactly "the paired `.asc` is not the source of truth for `tiers`, this
/// file's own `tiers` array is" -- precisely this function's own contract, reused
/// rather than adding a second flag with the same meaning.
///
/// Used by `apps/indicatrix-cut`'s autosave tick in place of
/// [`save_paired_extended`]/[`save_paired_extended_from_solved`]: autosave exists
/// specifically to survive a crash or a moved/deleted `.asc`, so a restore that
/// itself depends on a sidecar surviving alongside it defeats the point (see
/// `native_io.rs`'s own autosave doc comment).
///
/// `printed_proportions`/`extras` mean exactly what they mean on
/// [`save_paired_extended`] -- see that function's own doc comment.
#[must_use]
pub fn save_native_only(
    design: &Design,
    asc_filename: impl Into<String>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> NativeDesignFile {
    let material = material_table_from_selection(&design.material)
        .with_custom(extras.custom_material.cloned());
    let mut native = NativeDesignFile::new(
        asc_filename,
        NO_PAIRED_ASC_SHA256,
        preform_table_from_spec(&design.preform, design.preform_y_offset),
        design.girdle_diameter_mm,
        material,
        draft_tier_tables(design),
    )
    .with_draft(true)
    .with_history(HistoryTable::new(extras.history_entries.to_vec()))
    .with_authored_refractive_index(Some(design.meta.refractive_index));
    if let Some(props) = printed_proportions {
        native = native.with_source(source_table_from_proportions(props));
    }
    stash_schedule_meta(&mut native.unknown, &design.meta);
    native
}

/// [`save_native_only`] plus [`to_toml_string`] -- the one call an autosave tick
/// actually needs (the struct alone is occasionally useful to a test, hence both
/// are exposed).
///
/// # Errors
///
/// [`SaveError::Toml`] under the same (in-practice-unreachable) condition
/// [`save_paired`]'s own doc comment names.
pub fn save_native_only_toml(
    design: &Design,
    asc_filename: impl Into<String>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<String, SaveError> {
    let native = save_native_only(design, asc_filename, printed_proportions, extras);
    to_toml_string(&native).map_err(SaveError::Toml)
}

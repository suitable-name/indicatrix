//! [`catalogue_asc`]: building the catalogue `.asc` text a "Save" action writes
//! (or hands to the library), and the [`SaveError`]/[`CatalogueAsc`] types around it.
//! The design file itself is built by [`super::design_to_string`]. See the parent
//! module's doc comment ("Preserving the original `.asc` text") for the
//! preserve-vs-regenerate decision this makes.

use super::convert::SaveExtras;
use crate::design::{Design, DesignSolveError};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_formats::asc::{AscSchedule, AscTier};
use std::fmt;

/// Why [`catalogue_asc`] could not produce the `.asc` text.
///
/// Not raised for `design` not currently solving -- see [`catalogue_asc`]'s doc
/// comment: that case falls through to a draft ([`CatalogueAsc::draft_reason`])
/// instead of an error.
#[derive(Debug)]
pub enum SaveError {
    /// The `.asc` text could not be written: a header, footnote, or notes
    /// string would not survive a `.asc` write/re-parse round trip unchanged (a
    /// tier name never fails this way -- see
    /// [`indicatrix_formats::asc::asc_safe_tier_name`]). See
    /// [`indicatrix_formats::asc::AscWriteError`].
    AscWrite(indicatrix_formats::asc::AscWriteError),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AscWrite(e) => write!(f, "cannot save: {e}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AscWrite(e) => Some(e),
        }
    }
}

/// Why [`catalogue_asc`] wrote a draft instead of an ordinary `.asc` -- see
/// [`CatalogueAsc::draft_reason`].
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
    /// `.asc` with no `a` (facet) records at all, so an ordinary `.asc` would be a
    /// file nothing could reopen. Handled the same way as [`Self::Unsolved`]: the
    /// `.asc` is a placeholder with one dummy facet record, present only so the text
    /// parses at all (the design file carries the empty tier list itself).
    NoTiers,
    /// `design.solve()` returned `Ok`, but at least one tier that is NOT itself a
    /// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] came
    /// back with [`SolveStrategy::Failed`] -- `Design::solve`'s own legacy
    /// above-`MAX_PLANES` fallback (see that method's doc comment) turns a real
    /// solver error into an `Ok` result full of fabricated masts (each
    /// [`SolveStrategy::Failed`]'s own doc comment already says "should not be
    /// trusted") instead of propagating it, so this case has to be detected here
    /// from the solved masts themselves rather than from `design.solve()`'s
    /// `Result`. Handled exactly like [`Self::Unsolved`]: none of these masts is a
    /// real cut instruction, so this is a draft too.
    TooManyPlanes,
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
            Self::TooManyPlanes => write!(
                f,
                "too many planes for the solver to verify -- the masts below are unsolved \
                 placeholders, not real cutting instructions"
            ),
        }
    }
}

impl std::error::Error for DraftReason {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsolved(e) => Some(e),
            Self::NoTiers | Self::TooManyPlanes => None,
        }
    }
}

/// `true` iff `solved` (a [`Design::solve`] result already known to be `Ok`)
/// contains at least one tier that is not itself a
/// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] yet came
/// back with [`SolveStrategy::Failed`] -- the legacy above-`MAX_PLANES` fallback
/// `Design::solve`'s own doc comment describes (see [`DraftReason::TooManyPlanes`]).
/// A `ScaleReference` tier is excluded: its own mast is authored, never solved, so
/// it is never itself the thing this check is trying to catch.
#[must_use]
fn is_too_many_planes(design: &Design, solved: &[SolvedTier]) -> bool {
    design.tiers.iter().zip(solved).any(|(tier, solved_tier)| {
        !matches!(tier.constraint, MeetConstraint::ScaleReference(_))
            && matches!(solved_tier.strategy, SolveStrategy::Failed)
    })
}

/// Everything [`catalogue_asc`] produced: the cutting-instructions text of the design
/// and whether it is a draft.
///
/// The `.asc` text is the catalogue copy a "Save" stores next to the design file (the
/// design file itself, see [`super::design_to_string`], is the source of truth and
/// carries the whole design).
#[derive(Debug)]
pub struct CatalogueAsc {
    /// Text of the `.asc` file.
    pub asc_text: String,
    /// `true` iff `asc_text` is `original_asc_text` unchanged; `false` iff it was
    /// freshly regenerated -- see [`catalogue_asc`]'s doc comment for exactly when
    /// each happens. Exposed so a caller can report which one happened (e.g. "saved,
    /// schedule unchanged" vs. "saved, schedule updated") rather than guessing from
    /// the text itself. Always `false` for a draft (see [`Self::draft_reason`]) -- a
    /// placeholder-mast `.asc` is never the caller's own preserved text.
    pub asc_preserved: bool,
    /// `Some(reason)` iff this fell back to a draft instead of an ordinary `.asc` --
    /// either `design` did not currently solve (see [`Design::solve`]) or
    /// `design.tiers` was empty (see [`DraftReason::NoTiers`]). Either way `asc_text`
    /// is placeholder-mast `.asc` text, never a real cut instruction, and the design
    /// file's `draft` flag ([`indicatrix_formats::native::design::DesignFile::draft`])
    /// should be set. `None` for an ordinary `.asc`. A caller uses this to report
    /// e.g. "saved as draft ({reason})" rather than the ordinary saved-successfully
    /// toast.
    pub draft_reason: Option<DraftReason>,
}

/// Builds the catalogue `.asc` text a "Save" action should store.
///
/// `original_asc_text`, when given, is preserved verbatim in `asc_text` whenever
/// `design`'s current [`Design::to_asc_schedule`] is semantically equal to what
/// `original_asc_text` itself parses to -- see the module doc comment's "Preserving
/// the original `.asc` text" section. Text that fails to parse, or is absent (a
/// brand-new design with no prior `.asc`), falls straight through to a fresh export.
///
/// `placeholder_note`, when `Some`, marks the written `.asc` (via
/// [`indicatrix_formats::asc::mark_reconstructed`]) as derived rather than authored --
/// use this when `design` itself came from a placeholder reconstruction (e.g. a
/// catalogue entry with no attached `.asc`, only an angle table, so every mast is a
/// fabricated `0.0`) so the file this writes can never be mistaken for a real,
/// verified cutting instructions. Skips the preserve-original-text path entirely when
/// set: a placeholder-derived design's masts are never worth preserving verbatim even
/// if they happen to already be marked.
///
/// # Unsolved or tier-less designs: drafts
///
/// Two cases produce a draft instead of a failure: `design` not currently solving
/// (see [`Design::solve`] -- a block missing its one required
/// [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] tier, the
/// ordinary state of an in-progress crown with no anchor authored yet), and
/// `design.tiers` being empty (a brand-new design before its first tier -- see
/// [`DraftReason::NoTiers`]: `to_asc_schedule` actually succeeds here, but the result
/// has no facet records at all, which `indicatrix_formats::asc::parse_asc` refuses to
/// parse back). Either way [`CatalogueAsc::draft_reason`] is `Some(reason)` and
/// `asc_text` is a placeholder (real angle/indices/index-wheel positions for the
/// unsolved case, via [`draft_asc_schedule`]; one dummy facet record for the
/// tier-less case, via [`draft_asc_schedule_for_no_tiers`]) never to be trusted as a
/// cut instruction.
///
/// # Errors
///
/// [`SaveError::AscWrite`] if the schedule cannot be written as `.asc` text.
pub fn catalogue_asc(
    design: &Design,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
) -> Result<CatalogueAsc, SaveError> {
    catalogue_asc_extended(
        design,
        original_asc_text,
        placeholder_note,
        &SaveExtras::default(),
    )
}

/// Like [`catalogue_asc`], but also takes [`SaveExtras`].
///
/// Only `extras.custom_catalogue` (see [`SaveExtras::custom_catalogue`], the
/// wrong-`I`-line fix) matters here: it is consulted for the written `I` line.
/// `SaveExtras::default()` (`custom_catalogue: &[]` included) reproduces
/// [`catalogue_asc`]'s own built-ins-only behaviour exactly.
///
/// # Errors
///
/// Same as [`catalogue_asc`].
pub fn catalogue_asc_extended(
    design: &Design,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    extras: &SaveExtras<'_>,
) -> Result<CatalogueAsc, SaveError> {
    if design.tiers.is_empty() {
        return draft_asc(
            &draft_asc_schedule_for_no_tiers(design, extras.custom_catalogue),
            DraftReason::NoTiers,
        );
    }

    match design.solve() {
        Ok(solved) if is_too_many_planes(design, &solved) => draft_asc(
            &draft_asc_schedule(design, original_asc_text, extras.custom_catalogue),
            DraftReason::TooManyPlanes,
        ),
        Ok(solved) => catalogue_asc_extended_from_solved(
            design,
            &solved,
            original_asc_text,
            placeholder_note,
            extras,
        ),
        Err(missing_anchor) => draft_asc(
            &draft_asc_schedule(design, original_asc_text, extras.custom_catalogue),
            DraftReason::Unsolved(missing_anchor),
        ),
    }
}

/// [`catalogue_asc_extended`]'s "design currently solves" half.
///
/// Takes an already-[`Design::solve`]'d (or [`Design::resolve_dirty`]'d) mast list
/// instead of solving `design` again -- a caller that already solved
/// once (e.g. `apps/indicatrix-cut`'s `confirm_status_before_write`, which must
/// solve anyway to report a save-time status) must not pay for a second
/// multi-second solve just to write the file.
///
/// Uses [`Design::to_asc_schedule_from_solved_with`] (`extras.custom_catalogue`,
/// see [`SaveExtras::custom_catalogue`]'s own doc comment) where
/// [`catalogue_asc_extended`] uses [`Design::to_asc_schedule`] -- everything else
/// (the placeholder-note marking, the preserve-original-text comparison) is
/// identical, so the two functions can never drift apart in behaviour for a design
/// that solves.
///
/// Only handles that one case: the tier-less and does-not-solve draft branches stay
/// in [`catalogue_asc_extended`], which calls this exactly once it has a `solved`
/// list in hand.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same order
/// -- see [`Design::to_asc_schedule_from_solved`]'s own panic contract, which this
/// inherits unchanged.
///
/// # Errors
///
/// Same as [`catalogue_asc_extended`].
pub fn catalogue_asc_extended_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    extras: &SaveExtras<'_>,
) -> Result<CatalogueAsc, SaveError> {
    // The cheater-offset-baking variant, not the plain `_with` this design's own
    // internal render/measurement path uses -- see
    // `Design::to_asc_schedule_from_solved_with_cheater_offsets`'s own doc
    // comment: only the real file-writing path (here) should fold an authored
    // offset into the exported indices at all.
    let mut current_schedule =
        design.to_asc_schedule_from_solved_with_cheater_offsets(solved, extras.custom_catalogue);
    // `.asc` cannot hold a concave tier: it is carried as two footnotes per tier
    // (and, losslessly, in the design file) instead.
    design.append_concave_footnotes(&mut current_schedule);

    if let Some(note) = placeholder_note {
        indicatrix_formats::asc::mark_reconstructed(&mut current_schedule, note);
        let asc_text = indicatrix_formats::asc::to_asc_string(&current_schedule)
            .map_err(SaveError::AscWrite)?;
        return Ok(CatalogueAsc {
            asc_text,
            asc_preserved: false,
            draft_reason: None,
        });
    }

    let preserved = original_asc_text.and_then(|original| {
        let original_schedule = indicatrix_formats::asc::parse_asc(original).ok()?;
        schedules_equal_ignoring_refractive_index(&original_schedule, &current_schedule)
            .then(|| original.to_string())
    });

    let (asc_text, asc_preserved) = match preserved {
        Some(original) => (original, true),
        None => (
            indicatrix_formats::asc::to_asc_string(&current_schedule)
                .map_err(SaveError::AscWrite)?,
            false,
        ),
    };

    Ok(CatalogueAsc {
        asc_text,
        asc_preserved,
        draft_reason: None,
    })
}

/// `true` iff `original` and `current` are equal once `current`'s own
/// `refractive_index` is replaced with `original`'s -- the preservation test
/// [`catalogue_asc_extended`] actually applies, so that picking a different
/// material alone (which changes [`Design::effective_refractive_index`], and
/// therefore [`Design::to_asc_schedule`]'s `refractive_index` field, without
/// touching any tier/preform data `.asc` itself carries) does not, by itself,
/// force a fresh export of a hand-authored file. The design file's `[material]`
/// table already records the real selection, so the exported `.asc`'s `I` line
/// staying at the file's original value costs nothing -- it is not lost, only not
/// rewritten. See the parent module's doc comment ("Preserving the original `.asc`
/// text").
#[must_use]
fn schedules_equal_ignoring_refractive_index(
    original: &AscSchedule,
    current: &AscSchedule,
) -> bool {
    // The `.asc` on disk can only ever carry the writer's sanitised tier names
    // (`indicatrix_formats::asc::asc_safe_tier_name`: "Crown Main" is written as
    // `Crown_Main`), and its parser-side `warnings` describe that text, not the
    // design -- so both are normalised to the original's form before comparing.
    //
    // Two more fields describe the original TEXT rather than the design and are
    // masked the same way: `line_ending` (the parser records LF or CRLF; a design
    // export defaults to CRLF, so an untouched LF catalogue file would otherwise
    // never count as untouched) and each tier's `index_names` (the per-index name
    // positions the parser recorded; a design export leaves them empty and lets the
    // writer place the tier name after the first index).
    let mut masked_current = AscSchedule {
        refractive_index: original.refractive_index,
        warnings: original.warnings.clone(),
        line_ending: original.line_ending,
        ..current.clone()
    };
    for (tier, orig) in masked_current.tiers.iter_mut().zip(&original.tiers) {
        let safe = indicatrix_formats::asc::asc_safe_tier_name(&tier.name);
        if safe != tier.name {
            tier.name = safe.into_owned();
        }
        if tier.index_names.is_empty() {
            tier.index_names.clone_from(&orig.index_names);
        }
    }
    *original == masked_current
}

/// The shared tail of [`catalogue_asc_extended`]'s draft paths (unsolved, tier-less,
/// too-many-planes): builds `asc_text` from `draft_schedule` -- marked
/// [`indicatrix_formats::asc::mark_reconstructed`] with `reason`'s own
/// [`DraftReason`] message, so the written `.asc` is unmistakably a placeholder even
/// to software that has never heard of the design file's `draft` flag.
fn draft_asc(draft_schedule: &AscSchedule, reason: DraftReason) -> Result<CatalogueAsc, SaveError> {
    let mut schedule = draft_schedule.clone();
    indicatrix_formats::asc::mark_reconstructed(&mut schedule, &reason.to_string());
    let asc_text =
        indicatrix_formats::asc::to_asc_string(&schedule).map_err(SaveError::AscWrite)?;
    Ok(CatalogueAsc {
        asc_text,
        asc_preserved: false,
        draft_reason: Some(reason),
    })
}

/// The placeholder mast a draft writes for a tier with no better number to fall
/// back on -- see [`draft_asc_schedule`]. Mirrors, in spirit only, the meet solver's
/// own last-resort "no scale reference anywhere" default: not a real dimension
/// either way, always paired with [`SolveStrategy::Failed`] (whose own doc comment
/// already says the mast it carries "should not be trusted"), never shown to a
/// cutter as a cut instruction.
const DRAFT_PLACEHOLDER_MAST: f64 = 1.0;

/// Builds the placeholder-mast [`AscSchedule`] a draft writes as `asc_text`,
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

    let mut schedule = design.to_asc_schedule_from_solved_with(&placeholder_solved, custom);
    design.append_concave_footnotes(&mut schedule);
    schedule
}

/// A placeholder facet name/mast for the ONE dummy record
/// [`draft_asc_schedule_for_no_tiers`] writes -- see that function's own doc comment
/// for why a tier-less design needs one at all.
const NO_TIERS_PLACEHOLDER_NAME: &str = "placeholder";

/// Builds the placeholder [`AscSchedule`] a tier-less draft writes as `asc_text`.
/// `design.tiers` is empty, so unlike [`draft_asc_schedule`] there is no real
/// angle/index/name data to carry over -- this exists purely so
/// `indicatrix_formats::asc::parse_asc` (which refuses to parse any file with no `a`
/// facet records at all) can still open the text.
///
/// `custom` is consulted for the written `I` line exactly like
/// [`Design::effective_refractive_index_with`]; an empty slice behaves exactly like
/// [`Design::effective_refractive_index`]'s own built-ins-only resolution.
fn draft_asc_schedule_for_no_tiers(design: &Design, custom: &[GemMaterial]) -> AscSchedule {
    let mut schedule = AscSchedule {
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
            index_names: Vec::new(),
            notes: "draft save: design has no tiers yet".to_string(),
        }],
        warnings: Vec::new(),
        line_ending: indicatrix_formats::asc::AscLineEnding::default(),
    };
    design.append_concave_footnotes(&mut schedule);
    schedule
}

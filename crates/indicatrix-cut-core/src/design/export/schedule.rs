//! Builds a real [`AscSchedule`] from a [`Design`]'s current state --
//! [`Design::to_asc_schedule`] and its already-solved/catalogue-aware/fallible
//! siblings -- plus the `G`-field notes text ([`constraint_notes`]) and the
//! round-trip safety check on a `MeetNamed` target's name
//! ([`meet_name_is_asc_safe`]).

use crate::{
    cutting_sheet::format_index,
    design::{ConstraintTier, Design, DesignSolveError, SolveMismatch, TierRef},
    manufacturability::ManufacturabilityWarning,
};
use indicatrix::{
    geometry::meet_solver::{MeetConstraint, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_formats::asc::{AscLineEnding, AscSchedule, AscTier};

impl Design {
    /// Builds a real [`AscSchedule`] from this design's current state: every
    /// tier's mast is [`Self::solve`]'d, and its `G`-field notes text comes from
    /// [`constraint_notes`] -- verbatim from [`ConstraintTier::original_notes`] for
    /// as long as a tier's constraint is still the pinned
    /// [`MeetConstraint::ScaleReference`] import produced, else synthesized fresh
    /// from the tier's current [`MeetConstraint`] kind (parsing that synthesized text
    /// back reclassifies to the same variant, so this still round-trips
    /// semantically even once a tier has actually been edited). This is what makes
    /// `.asc` export and [`Self::planes`] both work from the same authored state.
    ///
    /// A thin wrapper over [`Self::to_asc_schedule_from_solved`] that solves first --
    /// see that method for the caller ([`crate::manufacturability`]) that needs this
    /// without paying for a second solve.
    ///
    /// Built-ins-only for its `I` line -- see [`Self::effective_refractive_index`]'s
    /// own doc comment. A caller with a resolved custom catalogue on hand should call
    /// [`Self::to_asc_schedule_with`] instead.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error.
    pub fn to_asc_schedule(&self) -> Result<AscSchedule, DesignSolveError> {
        self.to_asc_schedule_with(&[])
    }

    /// Like [`Self::to_asc_schedule`], but also consults `custom` -- see
    /// [`Self::effective_refractive_index_with`] -- so the exported `I` line reflects
    /// a CUSTOM catalogue material's own `n_D` instead of silently falling back to
    /// the legacy schedule RI. Passing `&[]` behaves exactly like
    /// [`Self::to_asc_schedule`] (that method's own implementation, in fact).
    ///
    /// # Errors
    ///
    /// Propagates [`Self::solve`]'s error.
    pub fn to_asc_schedule_with(
        &self,
        custom: &[GemMaterial],
    ) -> Result<AscSchedule, DesignSolveError> {
        let solved = self.solve()?;
        Ok(self.to_asc_schedule_from_solved_with(&solved, custom))
    }

    /// [`Self::to_asc_schedule`]'s schedule-building half, taking an
    /// already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list instead of
    /// solving `self` again.
    ///
    /// Exists so [`crate::manufacturability::check_manufacturability`] can run
    /// checks off an already-solved design rather than forcing a fresh solve -- a
    /// design with real meet-derived structure costs 5-6 seconds to solve (see
    /// `Design::solve`), and an editor with a `Vec<SolvedTier>` already in hand
    /// should never pay that cost twice. [`Self::to_asc_schedule`] just solves and
    /// delegates here, so the two can never drift apart.
    ///
    /// Built-ins-only for its `I` line, same caveat as [`Self::to_asc_schedule`] --
    /// see [`Self::to_asc_schedule_from_solved_with`] for the catalogue-aware sibling.
    ///
    /// # Panics
    ///
    /// `solved` must have one entry per tier `self` currently has, in the same order
    /// -- the same alignment contract [`Self::resolve_dirty`] documents on its own
    /// `previous` parameter. Mismatched lengths panic here rather than zipping a
    /// tier to the wrong mast.
    #[must_use]
    pub fn to_asc_schedule_from_solved(&self, solved: &[SolvedTier]) -> AscSchedule {
        self.to_asc_schedule_from_solved_with(solved, &[])
    }

    /// Catalogue-aware sibling of [`Self::to_asc_schedule_from_solved`] -- see
    /// [`Self::to_asc_schedule_with`] for why this exists. Passing `custom` as `&[]`
    /// is exactly [`Self::to_asc_schedule_from_solved`] itself (that method's own
    /// implementation, in fact), so built-in-material behaviour is byte-identical
    /// between the two.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`].
    #[must_use]
    pub fn to_asc_schedule_from_solved_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> AscSchedule {
        self.try_to_asc_schedule_from_solved_with(solved, custom)
            .unwrap_or_else(|e| {
                panic!(
                    "to_asc_schedule_from_solved: `solved` ({} masts) is not aligned with this \
                     design's current {} tier(s) -- only valid for a mast list this exact \
                     design (or one that differs only via ModifyTier/SetConstraint) actually \
                     produced",
                    e.got_tiers, e.expected_tiers
                )
            })
    }

    /// Fallible sibling of [`Self::to_asc_schedule_from_solved`]: returns
    /// [`SolveMismatch`] instead of panicking when `solved` is not aligned
    /// with [`Self::tiers`]. Identical construction otherwise -- the two
    /// share this method's body, so they can never drift apart.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_to_asc_schedule_from_solved(
        &self,
        solved: &[SolvedTier],
    ) -> Result<AscSchedule, SolveMismatch> {
        self.try_to_asc_schedule_from_solved_with(solved, &[])
    }

    /// Catalogue-aware, fallible sibling of [`Self::to_asc_schedule_from_solved_with`]
    /// -- returns [`SolveMismatch`] instead of panicking, exactly like
    /// [`Self::try_to_asc_schedule_from_solved`] but also consulting `custom` for the
    /// `I` line (see [`Self::effective_refractive_index_with`]). Every other `_with`
    /// entry point in this module (and [`Self::try_to_asc_schedule_from_solved`]
    /// itself) ultimately calls through here, so this is the one place the `I` line
    /// is actually decided.
    ///
    /// # Errors
    ///
    /// [`SolveMismatch`] when `solved.len() != self.tiers.len()`.
    pub fn try_to_asc_schedule_from_solved_with(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> Result<AscSchedule, SolveMismatch> {
        if solved.len() != self.tiers.len() {
            return Err(SolveMismatch {
                expected_tiers: self.tiers.len(),
                got_tiers: solved.len(),
            });
        }
        let tiers = self
            .tiers
            .iter()
            .zip(solved)
            .map(|(tier, solved)| AscTier {
                angle_deg: tier.angle_deg,
                mast: solved.mast,
                name: tier.name.clone(),
                indices: tier.indices.clone(),
                // A design keeps only the folded tier name; the writer puts it
                // after the first index, GemCAD's default label placement.
                index_names: Vec::new(),
                notes: constraint_notes(tier),
            })
            .collect();
        Ok(AscSchedule {
            gemcad_version: self.meta.gemcad_version.clone(),
            gear_teeth: self.meta.gear_teeth,
            gear_reference_angle: self.meta.gear_reference_angle,
            symmetry_order: self.meta.symmetry_order,
            mirror: self.meta.mirror,
            // The effective RI (see `Self::effective_refractive_index_with`), not the
            // raw legacy field -- catalogue-aware when `custom` names a match.
            refractive_index: self.effective_refractive_index_with(custom),
            headers: self.meta.headers.clone(),
            footnotes: self.meta.footnotes.clone(),
            tiers,
            warnings: Vec::new(),
            // `GemCAD`'s own line ending; see `AscLineEnding`'s doc comment.
            line_ending: AscLineEnding::default(),
        })
    }

    /// The schedule a file-writing path should write: [`Self::to_asc_schedule_from_solved`]
    /// plus the concave tiers as footnotes (see [`Self::append_concave_footnotes`]).
    ///
    /// A separate method, not a change to [`Self::to_asc_schedule_from_solved`],
    /// because that one also feeds [`Self::planes_from_solved`] (which re-imports its
    /// own schedule) and the manufacturability prefix schedules; footnotes there
    /// would only be noise. A design without concave tiers gets exactly the plain
    /// schedule.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`].
    #[must_use]
    pub fn to_asc_schedule_for_export(&self, solved: &[SolvedTier]) -> AscSchedule {
        let mut schedule = self.to_asc_schedule_from_solved(solved);
        self.append_concave_footnotes(&mut schedule);
        schedule
    }

    /// Appends two `F` footnotes per concave tier to `schedule`, in
    /// [`Self::cutting_order`] (plan §6.2): the facet line
    /// `"{name}  {φ:.2}  {indices}  {instructions}"` and the tool line from
    /// [`ConcaveTier::second_line_fields`](crate::design::ConcaveTier::second_line_fields),
    /// both without the `F ` prefix the writer adds. Concave tiers are never emitted
    /// as `.asc` tiers.
    ///
    /// Idempotent across re-import: a previous export's footnotes come back as
    /// ordinary [`ScheduleMeta::footnotes`](crate::design::ScheduleMeta::footnotes),
    /// possibly describing tiers that have since been edited. Every footnote of the
    /// generated shape ([`strip_generated_concave_footnotes`]: a tool line and the
    /// facet line before it) is therefore dropped from `schedule` first (never from
    /// the design), along with any entry byte-equal to a line about to be written;
    /// the user's own footnotes are otherwise untouched. Line breaks inside free text become spaces because the
    /// writer refuses a footnote that spans lines. Does nothing for a design
    /// without concave tiers.
    pub fn append_concave_footnotes(&self, schedule: &mut AscSchedule) {
        if self.concave_tiers.is_empty() {
            return;
        }
        let single_line = |text: &str| -> String {
            text.split(['\r', '\n'])
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_owned()
        };
        let mut generated = Vec::with_capacity(self.concave_tiers.len() * 2);
        for tier_ref in self.cutting_order() {
            let TierRef::Concave(index) = tier_ref else {
                continue;
            };
            let tier = &self.concave_tiers[index];
            let indices = tier
                .indices
                .iter()
                .map(|&i| format_index(i))
                .collect::<Vec<_>>()
                .join(" ");
            generated.push(single_line(&format!(
                "{}  {:.2}  {}  {}",
                tier.name, tier.angle_deg, indices, tier.instructions
            )));
            generated.push(single_line(&tier.second_line_fields().join("  ")));
        }
        strip_generated_concave_footnotes(&mut schedule.footnotes);
        schedule
            .footnotes
            .retain(|footnote| !generated.contains(footnote));
        schedule.footnotes.extend(generated);
    }

    /// What a file export of this design loses, for the UI to say before saving:
    /// `.asc`, `.gem` and `.gcs` cannot represent concave tiers, so they are
    /// omitted from the tier records. Empty for a planar design.
    #[must_use]
    pub fn export_warnings(&self) -> Vec<ManufacturabilityWarning> {
        if self.concave_tiers.is_empty() {
            Vec::new()
        } else {
            vec![ManufacturabilityWarning::ConcaveTiersOmittedFromExport {
                count: self.concave_tiers.len(),
            }]
        }
    }

    /// [`Self::to_asc_schedule_from_solved_with`], but also bakes every tier's
    /// authored [`Self::cheater_offsets_deg`] into that tier's own exported
    /// `indices` -- see [`Self::cheater_offsets_deg`]'s own doc comment for why:
    /// `.asc`/`GemCAD` have no field for a cheater/azimuth angle at all, so the
    /// only way an authored offset survives opening this exact `.asc` file in
    /// software that has never heard of this crate's own native sidecar is to
    /// fold it into the one thing `.asc` DOES carry per facet -- its index-wheel
    /// position. The shift is `offset_deg / 360 * gear_teeth` (the fraction of a
    /// full index-wheel rotation `offset_deg` covers, in the same tooth-position
    /// units [`indicatrix_formats::asc::AscTier::indices`] already uses), added to
    /// every index and rounded to 3 decimals. A positive offset therefore moves the
    /// facet toward higher index numbers (azimuth `phi + offset`), the one sign
    /// convention `design::export::planes::apply_cheater_offsets` documents and
    /// applies to the rendered geometry. A tier without indices (a single facet
    /// with no azimuth of its own) has nothing to shift, so its offset appears only
    /// in the geometry and the native sidecar, not in the exported `.asc`.
    ///
    /// Only [`crate::native::save_paired`] (the real file-writing path) should
    /// call this. [`Self::planes_from_solved`]/[`Self::planes_through_tier`] --
    /// and every OTHER `to_asc_schedule*`/`try_to_asc_schedule*` entry point in
    /// this module -- build the same kind of schedule for rendering/measurement
    /// via [`Self::to_asc_schedule_from_solved`] (deliberately NOT this method)
    /// and apply the offset as a real plane rotation instead
    /// (`design::export::planes::apply_cheater_offsets`, which rotates the plane
    /// NORMAL directly rather than the index-wheel position that produced it);
    /// baking the offset into indices there too would rotate every one of those
    /// consumers TWICE.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`].
    #[must_use]
    pub fn to_asc_schedule_from_solved_with_cheater_offsets(
        &self,
        solved: &[SolvedTier],
        custom: &[GemMaterial],
    ) -> AscSchedule {
        let mut schedule = self.to_asc_schedule_from_solved_with(solved, custom);
        self.bake_cheater_offsets_into_indices(&mut schedule);
        schedule
    }

    /// [`Self::to_asc_schedule_from_solved_with_cheater_offsets`]'s own mutation
    /// half -- see that method's doc comment.
    fn bake_cheater_offsets_into_indices(&self, schedule: &mut AscSchedule) {
        if self.cheater_offsets_deg.is_empty() {
            return;
        }
        let gear_teeth = f64::from(self.meta.gear_teeth_abs());
        for (index, tier) in schedule.tiers.iter_mut().enumerate() {
            let Some(offset_deg) = self.cheater_offset_deg(index).filter(|&deg| deg != 0.0) else {
                continue;
            };
            let shift = offset_deg / 360.0 * gear_teeth;
            for idx in &mut tier.indices {
                *idx = ((*idx + shift) * 1000.0).round() / 1000.0;
            }
        }
    }
}

/// Builds one tier's `.asc` `G`-field text.
///
/// While `tier.constraint` is still the pinned [`MeetConstraint::ScaleReference`]
/// [`Design::from_asc_schedule`] produced for it, this returns
/// [`ConstraintTier::original_notes`] verbatim (whatever the file actually said --
/// `"Cut to TCP"`, `"GMP"`, `"Cut to mast depth X."`, anything at all, not just the
/// handful of instructions this crate classifies), so an untouched import's schedule
/// re-exports with the exact notes a human reads while cutting. `original_notes` is
/// `None` for a tier [`Design::from_asc_schedule`] never built (a brand-new tier, or
/// one already replaced by an edit), which falls through to the synthesized text
/// below same as a tier whose constraint has actually changed.
///
/// Synthesized text reclassifies to the same [`MeetConstraint`] variant via
/// `AscTier::meet_instruction`/`meet_tier_inputs_from_asc`, so this still round-trips
/// semantically (not byte-for-byte) once a tier has actually been edited:
///
/// - [`MeetConstraint::MeetExisting`] -> empty (no `G` field -- the common case).
/// - [`MeetConstraint::MeetNamed`] -> `"Meet <names>"`, the exact instruction text
///   `indicatrix_formats::asc::MeetInstruction::Meet` parses -- PROVIDED every name in
///   the list passes [`meet_name_is_asc_safe`]; a name containing whitespace/`,`/
///   `;`, or with leading/trailing punctuation, splits into the wrong token(s) or a
///   different tier's name on re-parse (`indicatrix_formats::asc`'s own tokenizer
///   splits and trims exactly those characters -- see that module's
///   `extract_meet_names`). This function does not itself refuse such a name (the
///   export format is unchanged -- see
///   [`crate::manufacturability::check_meet_name_asc_safety`] for the warning that
///   flags it instead), so a caller relying on byte-for-byte reclassification must
///   check that separately.
/// - [`MeetConstraint::ScaleReference`] -> `"Set stone size."`, one of several real
///   phrasings `indicatrix_formats::asc::MeetInstruction::ScaleReference` recognizes (this
///   crate does not track which specific phrasing a freshly authored -- not
///   imported -- scale reference should use, only that it is one).
fn constraint_notes(tier: &ConstraintTier) -> String {
    if let (MeetConstraint::ScaleReference(_), Some(original_notes)) =
        (&tier.constraint, &tier.original_notes)
    {
        return original_notes.clone();
    }
    match &tier.constraint {
        MeetConstraint::MeetExisting => String::new(),
        MeetConstraint::MeetNamed(names) => format!("Meet {}", names.join(", ")),
        MeetConstraint::ScaleReference(_) => "Set stone size.".to_string(),
    }
}

/// `true` iff `name` would survive a plain `.asc` export/re-import as this exact,
/// single token -- i.e. [`constraint_notes`]'s `"Meet <names>"` text, re-parsed by
/// `indicatrix_formats::asc`'s own `G`-field tokenizer (splits a `Meet` instruction's
/// text on `,`/`;`/whitespace, then trims each token of leading/trailing
/// non-alphanumeric characters -- see that crate's `extract_meet_names`), would
/// resolve back to `name` and nothing else.
///
/// Unsafe: an empty name (nothing to resolve at all); one containing whitespace,
/// `,` or `;` (the parser's own token separators -- a compound tier name like
/// `"Crown Main"` would split into two unrelated tokens, neither of which names
/// this tier); or one with a leading or trailing character that is not
/// alphanumeric (stripped by the parser's `trim_matches`, so e.g. the mirrored-tier
/// convention `"Main'"` -- see [`ConstraintTier::mirrored_to_other_block`] --
/// reparses as `"Main"`, a DIFFERENT tier's name entirely).
///
/// Used by [`crate::manufacturability::check_meet_name_asc_safety`] to warn on an
/// authored [`MeetConstraint::MeetNamed`] target that would not survive this round
/// trip; [`constraint_notes`] itself keeps writing the name verbatim regardless
/// (the export format is unchanged -- see that function's own doc comment).
#[must_use]
pub fn meet_name_is_asc_safe(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == ',' || c == ';')
    {
        return false;
    }
    let first = name.chars().next().expect("checked non-empty above");
    let last = name.chars().next_back().expect("checked non-empty above");
    first.is_alphanumeric() && last.is_alphanumeric()
}

/// Whether `text` is `digits.decimals` with an optional sign and exactly `decimals`
/// fractional digits: the shape of every number [`ConcaveTier::second_line_fields`]
/// and the facet line print.
fn is_fixed_number(text: &str, decimals: usize) -> bool {
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    digits.split_once('.').is_some_and(|(whole, fraction)| {
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.len() == decimals
            && fraction.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Whether `line` is exactly the shape of a generated concave tool line
/// (`CYL  +10.00°  X = 0.000, Y = 0.000, Z = 0.000  D/W = 0.400, reciprocating`, with
/// `, angle = 60.00°` before the motion for `CON` and `DSC`). Matches the structure,
/// not the values, so a line from an earlier version of the tier is recognised.
fn is_generated_tool_line(line: &str) -> bool {
    let fields: Vec<&str> = line.split("  ").collect();
    let [code, theta, displacement, size] = fields[..] else {
        return false;
    };
    if !["CYL", "CON", "CIR", "DSC", "SPH"].contains(&code) {
        return false;
    }
    if !theta
        .strip_suffix('\u{b0}')
        .is_some_and(|n| is_fixed_number(n, 2))
    {
        return false;
    }
    let displacement_ok = displacement
        .strip_prefix("X = ")
        .and_then(|rest| rest.split_once(", Y = "))
        .and_then(|(x, rest)| Some((x, rest.split_once(", Z = ")?)))
        .is_some_and(|(x, (y, z))| [x, y, z].iter().all(|n| is_fixed_number(n, 3)));
    if !displacement_ok {
        return false;
    }
    let Some(rest) = size.strip_prefix("D/W = ") else {
        return false;
    };
    let parts: Vec<&str> = rest.split(", ").collect();
    let (ratio, middle, motion) = match parts[..] {
        [ratio, motion] => (ratio, None, motion),
        [ratio, angle, motion] => (ratio, Some(angle), motion),
        _ => return false,
    };
    is_fixed_number(ratio, 3)
        && ["reciprocating", "plunge"].contains(&motion)
        && middle.is_none_or(|angle| {
            angle
                .strip_prefix("angle = ")
                .and_then(|n| n.strip_suffix('\u{b0}'))
                .is_some_and(|n| is_fixed_number(n, 2))
        })
}

/// Whether `line` could be a generated concave facet line
/// (`{name}  {phi:.2}  {indices}  {instructions}`): one of its double-space
/// separated fields is a two-decimal number. Only ever asked about the line that
/// precedes a recognised tool line.
fn is_generated_facet_line(line: &str) -> bool {
    line.split("  ")
        .any(|field| is_fixed_number(field.trim(), 2))
}

/// Removes every generated concave footnote from `footnotes`: each line that has
/// the shape of a tool line ([`Design::append_concave_footnotes`] writes one per
/// concave tier) together with the facet line immediately before it. Anything else
/// is a user's footnote and stays, in order.
///
/// A paired load runs this on the footnotes an earlier export left in the `.asc`,
/// so edited tiers cannot leave stale lines behind that contradict the new ones.
pub fn strip_generated_concave_footnotes(footnotes: &mut Vec<String>) {
    let mut drop = vec![false; footnotes.len()];
    for (i, line) in footnotes.iter().enumerate() {
        if !is_generated_tool_line(line) {
            continue;
        }
        drop[i] = true;
        if i > 0
            && !is_generated_tool_line(&footnotes[i - 1])
            && is_generated_facet_line(&footnotes[i - 1])
        {
            drop[i - 1] = true;
        }
    }
    let mut keep = drop.into_iter().map(|d| !d);
    footnotes.retain(|_| keep.next().unwrap_or(true));
}

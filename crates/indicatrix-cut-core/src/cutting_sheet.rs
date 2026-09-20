//! [`Design::cutting_sheet`]: a design's printable cutting sequence -- the
//! library half of what today only exists as raw `.asc` text or a catalogue
//! TSV, neither meant for a cutter to read at the machine.
//!
//! [`Design::cutting_sheet`] takes a design and its already-solved masts and
//! returns a [`CuttingSheet`]: every tier, in cutting order, with its angle,
//! index list, solved mast (cutting depth), and a printable meet
//! instruction. The editor's only remaining job is to show [`CuttingSheet::rows`]
//! in a table and hand [`CuttingSheet::to_text`]'s string to Save/Print --
//! see that method's doc comment for the exact layout.
//!
//! [`Design::facet_meets`] is the companion read: which other tiers a given
//! tier's own [`MeetConstraint::MeetNamed`] resolves to, using the exact same
//! [`MeetNameResolver`] the solver itself uses (girdle/culet/table synonyms,
//! side-prefix and plural stripping, compound vertex specs all included) --
//! not a naive name match.
//!
//! [`diff_tiers`] lives here too: a positional before/after tier comparison
//! (e.g. a saved snapshot against a design's current state) for a "compare
//! two designs" view, sharing this module's [`ConstraintTier`]/[`SolvedTier`]
//! imports.

use crate::{
    design::{ConstraintTier, Design},
    edit::EditError,
};
use indicatrix::geometry::meet_solver::{
    MeetConstraint, MeetNameResolver, MeetTierInput, SolvedTier,
};
use std::fmt::Write as _;

/// One line of a printable cutting sequence: everything a cutter needs to
/// know about a single facet tier, already in cutting order (see
/// [`CuttingSheet::rows`]).
#[derive(Debug, Clone, PartialEq)]
pub struct CutSheetRow {
    /// 1-based position in the cutting sequence -- `design.tiers`' own
    /// order, the same order a real `.asc` file lists tiers in and the order
    /// [`Design::to_asc_schedule_from_solved`] preserves.
    pub sequence: usize,
    /// The tier's own name(s), verbatim (see [`ConstraintTier::name`]).
    /// Empty for an unnamed tier.
    pub name: String,
    /// Signed angle from the girdle plane, in degrees -- the same `GemCad`
    /// convention [`ConstraintTier::angle_deg`] documents.
    pub angle_deg: f64,
    /// Index-wheel positions this facet occurs at, verbatim from
    /// [`ConstraintTier::indices`]. Empty means a single facet at azimuth 0.
    pub indices: Vec<f64>,
    /// The solved mast (cutting depth), in the design's mast units -- from
    /// [`SolvedTier::mast`].
    pub mast: f64,
    /// What this facet's plane closes against, as printable prose. Prefers
    /// the tier's own recorded `.asc` notes while its constraint is still
    /// the pinned import value (see [`ConstraintTier::original_notes`]),
    /// else synthesizes text from its current [`MeetConstraint`] -- never
    /// empty, unlike the raw `.asc` `G` field, since a blank entry on a
    /// printed sheet reads as "nothing recorded" rather than the actual
    /// default a cutter assumes ("meets whatever the solver finds").
    pub meet_instruction: String,
    /// Which other tiers (indices into `design.tiers`) this row's own
    /// [`MeetConstraint::MeetNamed`] resolves to -- see [`Design::facet_meets`].
    /// Always empty for [`MeetConstraint::MeetExisting`]/
    /// [`MeetConstraint::ScaleReference`], which have no named target.
    pub meets_tiers: Vec<usize>,
    /// This row's own cheater/azimuth offset (degrees), from
    /// [`Design::cheater_offset_deg`] -- `None` when the tier has no recorded
    /// offset (the common case). See that method's own doc comment (CAD
    /// audit item 213) for why a cutter needs this printed alongside the
    /// angle/index/mast figures already here.
    pub cheater_offset_deg: Option<f64>,
}

/// A design's printable cutting sequence: every tier, in cutting order, with
/// enough information to cut it.
///
/// See the module docs for the problem this solves. Built once by
/// [`Design::cutting_sheet`] from an already-solved mast list.
#[derive(Debug, Clone, PartialEq)]
pub struct CuttingSheet {
    /// Schedule-wide context a cutter needs before the first tier: material,
    /// effective refractive index, index-gear tooth count and symmetry, and
    /// the real-world girdle diameter when one is set. One entry per printed
    /// line, in this order -- see [`Design::cutting_sheet`] for exactly what
    /// populates it.
    pub header: Vec<String>,
    /// Every tier, in cutting order -- see [`CutSheetRow`].
    pub rows: Vec<CutSheetRow>,
}

impl CuttingSheet {
    /// Renders the sheet as plain text ready to print or save: the header
    /// block, a blank line, then one line per [`CutSheetRow`] with fixed
    /// field labels (sequence, name, angle, indices, mast, meet). See this
    /// module's own tests for a worked example of the exact layout.
    ///
    /// Deterministic: identical input always produces a byte-identical
    /// string, matching this crate's own determinism requirement -- every
    /// field is either already a plain value or formatted with a fixed
    /// float precision, and rows are printed in `self.rows`' own order
    /// (never re-sorted).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for line in &self.header {
            out.push_str(line);
            out.push('\n');
        }
        if !self.header.is_empty() {
            out.push('\n');
        }
        for row in &self.rows {
            let indices = format_indices(&row.indices);
            let name = if row.name.is_empty() {
                "(unnamed)"
            } else {
                row.name.as_str()
            };
            // `write!` into a `String` is infallible; nothing to propagate.
            let _ = write!(
                out,
                "{:>3}. {name:<16} angle {:>7.2} deg  indices [{indices}]  mast {:>8.4}  meet: {}",
                row.sequence, row.angle_deg, row.mast, row.meet_instruction
            );
            if let Some(cheater) = row.cheater_offset_deg {
                let _ = write!(out, "  cheater: {cheater:+.2} deg");
            }
            out.push('\n');
        }
        out
    }
}

/// Formats a tier's whole index list for [`CuttingSheet::to_text`]: `"-"`
/// for an empty list (a single facet at azimuth 0), else each position
/// through [`format_index`] joined with `", "`.
fn format_indices(indices: &[f64]) -> String {
    if indices.is_empty() {
        return "-".to_string();
    }
    indices
        .iter()
        .map(|&i| format_index(i))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Formats one index-wheel position: a whole tooth prints with no decimal
/// point (`"12"`, matching how a cutter reads an index-wheel scale), a
/// fractional position keeps two decimals (`"11.50"`).
fn format_index(index: f64) -> String {
    if (index - index.round()).abs() < 1e-9 {
        format!("{:.0}", index.round())
    } else {
        format!("{index:.2}")
    }
}

/// Builds one tier's printable meet instruction -- see
/// [`CutSheetRow::meet_instruction`]. Mirrors
/// `design::export`'s own notes-vs-synthesized precedence (recorded notes
/// text while the constraint is still the pinned import value, else
/// synthesized from the constraint), but only ever accepts non-blank
/// recorded notes and always falls back to a readable phrase for
/// [`MeetConstraint::MeetExisting`] instead of an empty string.
fn meet_instruction(tier: &ConstraintTier) -> String {
    if let (MeetConstraint::ScaleReference(_), Some(notes)) =
        (&tier.constraint, &tier.original_notes)
        && !notes.trim().is_empty()
    {
        return notes.clone();
    }
    match &tier.constraint {
        MeetConstraint::MeetExisting => "Meet at previously cut facets".to_string(),
        MeetConstraint::MeetNamed(names) => format!("Meet {}", names.join(", ")),
        MeetConstraint::ScaleReference(mast) => format!("Set to mast depth {mast:.4}"),
    }
}

/// Builds one [`MeetTierInput`] per tier of `tiers`, for
/// [`MeetNameResolver`] -- the same conversion
/// `indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc` does from a
/// parsed `.asc` schedule, done here directly from a [`Design`]'s own
/// authoritative [`ConstraintTier::constraint`] instead (no solved mast
/// needed: a tier's constraint already carries its `ScaleReference` value
/// when it has one, so this never has to solve first).
fn meet_tier_inputs(tiers: &[ConstraintTier]) -> Vec<MeetTierInput> {
    tiers
        .iter()
        .map(|tier| MeetTierInput {
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            constraint: tier.constraint.clone(),
            names: tier.names().into_iter().map(str::to_string).collect(),
        })
        .collect()
}

/// Resolves `tier`'s own [`MeetConstraint::MeetNamed`] against an
/// already-built `resolver` -- the shared body of [`Design::facet_meets`]
/// and [`Design::cutting_sheet`], so the latter builds one
/// [`MeetNameResolver`] for the whole design instead of one per row.
/// `MeetExisting`/`ScaleReference` both resolve to an empty list, same as
/// [`Design::facet_meets`] documents.
fn resolve_meets(tier: &ConstraintTier, resolver: &MeetNameResolver<'_>) -> Vec<usize> {
    let MeetConstraint::MeetNamed(names) = &tier.constraint else {
        return Vec::new();
    };
    resolver.resolve_names(names).refs
}

impl Design {
    /// Which other tiers (by index into [`Self::tiers`]) the tier at
    /// `tier_index` actually meets, resolved the same way [`Self::solve`]
    /// itself resolves a `MeetNamed` instruction -- [`MeetNameResolver`], not
    /// a naive name match, so girdle/culet/table fallbacks, side-prefix and
    /// plural stripping, and compound vertex specs all behave identically to
    /// the solver.
    ///
    /// [`MeetConstraint::MeetExisting`] (the solver picks a candidate vertex
    /// without ever naming facets) and [`MeetConstraint::ScaleReference`]
    /// (an authored dimension, not a meet at all) both resolve to an empty
    /// list -- there is nothing named to report for either. Only
    /// [`MeetConstraint::MeetNamed`] has a real answer.
    ///
    /// # Errors
    ///
    /// [`EditError`] if `tier_index` is out of range.
    pub fn facet_meets(&self, tier_index: usize) -> Result<Vec<usize>, EditError> {
        let tier_count = self.tiers.len();
        let tier = self.tiers.get(tier_index).ok_or(EditError {
            index: tier_index,
            tier_count,
        })?;
        let inputs = meet_tier_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        Ok(resolve_meets(tier, &resolver))
    }

    /// Builds this design's printable cutting sequence from an
    /// already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list --
    /// see [`CuttingSheet`] for what it carries and why a cutter needs it.
    /// Never solves again itself, same reasoning as
    /// [`Self::to_asc_schedule_from_solved`].
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::to_asc_schedule_from_solved`]:
    /// `solved` must have one entry per tier `self` currently has, in the
    /// same order.
    #[must_use]
    pub fn cutting_sheet(&self, solved: &[SolvedTier]) -> CuttingSheet {
        assert_eq!(
            solved.len(),
            self.tiers.len(),
            "cutting_sheet: `solved` ({} masts) is not aligned with this design's current {} \
             tier(s)",
            solved.len(),
            self.tiers.len()
        );

        let mut header = vec![format!(
            "Material: {}",
            self.material.name.as_deref().unwrap_or("(unset)")
        )];
        header.push(format!(
            "Refractive index: {:.3}",
            self.effective_refractive_index()
        ));
        header.push(format!(
            "Index gear: {} teeth, symmetry {}{}",
            self.meta.gear_teeth_abs(),
            self.meta.symmetry_order,
            if self.meta.mirror { ", mirrored" } else { "" }
        ));
        if let Some(mm) = self.girdle_diameter_mm {
            header.push(format!("Girdle diameter: {mm:.3} mm"));
        }

        let inputs = meet_tier_inputs(&self.tiers);
        let resolver = MeetNameResolver::new(&inputs);
        let rows = self
            .tiers
            .iter()
            .zip(solved)
            .enumerate()
            .map(|(i, (tier, solved_tier))| CutSheetRow {
                sequence: i + 1,
                name: tier.name.clone(),
                angle_deg: tier.angle_deg,
                indices: tier.indices.clone(),
                mast: solved_tier.mast,
                meet_instruction: meet_instruction(tier),
                meets_tiers: resolve_meets(tier, &resolver),
                cheater_offset_deg: self.cheater_offset_deg(i),
            })
            .collect();

        CuttingSheet { header, rows }
    }
}

/// One tier's before/after comparison, position by position -- see
/// [`diff_tiers`].
#[derive(Debug, Clone, PartialEq)]
pub struct TierDelta {
    /// Position in both tier lists (see [`diff_tiers`]'s positional-only
    /// contract).
    pub index: usize,
    /// The tier's name in the "after" list, or the "before" list's if the
    /// tier was removed (see [`Self::removed`]).
    pub name: String,
    /// `before`'s angle, or `None` if `index` is past the end of `before`
    /// (this tier was added).
    pub angle_before: Option<f64>,
    /// `after`'s angle, or `None` if `index` is past the end of `after`
    /// (this tier was removed).
    pub angle_after: Option<f64>,
    /// `before`'s index-wheel positions, when this tier existed in `before`.
    pub indices_before: Option<Vec<f64>>,
    /// `after`'s index-wheel positions, when this tier still exists in
    /// `after`.
    pub indices_after: Option<Vec<f64>>,
    /// `before`'s solved mast, when a `before_solved` list was supplied and
    /// this tier existed in `before`.
    pub mast_before: Option<f64>,
    /// `after`'s solved mast, when an `after_solved` list was supplied and
    /// this tier still exists in `after`.
    pub mast_after: Option<f64>,
}

impl TierDelta {
    /// `true` iff this tier exists in both lists and its angle actually
    /// changed.
    #[must_use]
    pub fn angle_changed(&self) -> bool {
        match (self.angle_before, self.angle_after) {
            (Some(before), Some(after)) => (before - after).abs() > f64::EPSILON,
            _ => false,
        }
    }

    /// `true` iff this tier exists in both lists and its index-wheel
    /// positions actually changed.
    #[must_use]
    pub fn indices_changed(&self) -> bool {
        matches!((&self.indices_before, &self.indices_after), (Some(b), Some(a)) if b != a)
    }

    /// `true` iff both masts are known and differ by more than `tolerance`
    /// (model units) -- a caller compares against a tolerance rather than
    /// exact equality since a solved mast is a floating-point result, not an
    /// authored value.
    #[must_use]
    pub fn mast_changed(&self, tolerance: f64) -> bool {
        match (self.mast_before, self.mast_after) {
            (Some(before), Some(after)) => (before - after).abs() > tolerance,
            _ => false,
        }
    }

    /// `true` iff `index` is past the end of the "before" list -- this tier
    /// was added.
    #[must_use]
    pub const fn added(&self) -> bool {
        self.angle_before.is_none()
    }

    /// `true` iff `index` is past the end of the "after" list -- this tier
    /// was removed.
    #[must_use]
    pub const fn removed(&self) -> bool {
        self.angle_after.is_none()
    }
}

/// Compares two tier lists position by position (e.g. a saved snapshot's
/// `before` against a design's current `after`), producing one [`TierDelta`]
/// per position either list has a tier at.
///
/// **Positional only**: a tier inserted or removed partway through shifts
/// every later position, so this reports every tier from that point on as
/// "changed" rather than following the renamed/moved tier -- the same
/// trade-off a plain `zip` over two `Vec`s always makes. A caller that wants
/// insert/delete-aware alignment (matching tiers by name first) builds that
/// on top of this; nothing here hides the limitation, since `TierDelta` names
/// exactly which positions were compared.
///
/// `before_solved`/`after_solved`, when supplied, must have one entry per
/// tier in `before`/`after` respectively -- the same alignment contract
/// [`Design::planes_from_solved`] documents; a mismatched length is treated
/// as "no solved masts" (`mast_before`/`mast_after` stay `None`) rather than
/// panicking, since a diff is a read-only report and should degrade
/// gracefully rather than crash on stale solved state.
#[must_use]
pub fn diff_tiers(
    before: &[ConstraintTier],
    before_solved: Option<&[SolvedTier]>,
    after: &[ConstraintTier],
    after_solved: Option<&[SolvedTier]>,
) -> Vec<TierDelta> {
    let before_masts = before_solved.filter(|s| s.len() == before.len());
    let after_masts = after_solved.filter(|s| s.len() == after.len());
    let len = before.len().max(after.len());
    (0..len)
        .map(|index| {
            let b = before.get(index);
            let a = after.get(index);
            TierDelta {
                index,
                name: a.or(b).map_or_else(String::new, |t| t.name.clone()),
                angle_before: b.map(|t| t.angle_deg),
                angle_after: a.map(|t| t.angle_deg),
                indices_before: b.map(|t| t.indices.clone()),
                indices_after: a.map(|t| t.indices.clone()),
                mast_before: before_masts.and_then(|s| s.get(index)).map(|s| s.mast),
                mast_after: after_masts.and_then(|s| s.get(index)).map(|s| s.mast),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{design::ScheduleMeta, preform::PreformSpec};

    fn round_brilliant_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    /// `cutting_sheet` must produce one row per tier, in tier order, with
    /// the solved mast carried through and a non-empty meet instruction for
    /// every row (this template's tiers are all `ScaleReference`).
    #[test]
    fn cutting_sheet_has_one_row_per_tier_in_order() {
        let design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let sheet = design.cutting_sheet(&solved);
        assert_eq!(sheet.rows.len(), 8);
        for (i, row) in sheet.rows.iter().enumerate() {
            assert_eq!(row.sequence, i + 1);
            assert_eq!(row.name, design.tiers[i].name);
            assert_eq!(row.mast, solved[i].mast);
            assert_ne!(row.meet_instruction, "");
            assert_eq!(row.meets_tiers, Vec::<usize>::new()); // all ScaleReference here
        }
        assert!(sheet.header.iter().any(|l| l.starts_with("Index gear: 96")));
    }

    /// A tier with a recorded [`Design::cheater_offset_deg`] must carry it
    /// through to its own [`CutSheetRow`] and appear in [`CuttingSheet::to_text`];
    /// every other row's `cheater_offset_deg` must stay `None` and print
    /// nothing extra.
    #[test]
    fn cutting_sheet_carries_the_cheater_offset_into_its_own_row_and_text() {
        let mut design = round_brilliant_design();
        design.cheater_offsets_deg.insert(1, -0.75);
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let sheet = design.cutting_sheet(&solved);
        assert_eq!(sheet.rows[0].cheater_offset_deg, None);
        assert_eq!(sheet.rows[1].cheater_offset_deg, Some(-0.75));

        let text = sheet.to_text();
        let lines: Vec<&str> = text.lines().collect();
        let row1_line = lines
            .iter()
            .find(|l| l.trim_start().starts_with("2."))
            .expect("row 2 must be printed");
        assert!(row1_line.contains("cheater: -0.75 deg"), "{row1_line}");
        let row0_line = lines
            .iter()
            .find(|l| l.trim_start().starts_with("1."))
            .expect("row 1 must be printed");
        assert!(!row0_line.contains("cheater"), "{row0_line}");
    }

    /// `facet_meets` must resolve a `MeetNamed` reference through the real
    /// solver-grade resolver (here: an exact name match), return empty for
    /// `MeetExisting`/`ScaleReference`, and error on an out-of-range index.
    #[test]
    fn facet_meets_resolves_named_references() {
        let tiers = vec![
            ConstraintTier {
                angle_deg: 34.5,
                name: "Crown Main".to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
            ConstraintTier {
                angle_deg: 41.0,
                name: "Star".to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetNamed(vec!["Crown Main".to_string()]),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            },
        ];
        let design = Design::new(
            PreformSpec::block(1.0, 1.0, 1.0),
            ScheduleMeta::standard_round_brilliant(),
            tiers,
        );
        assert_eq!(design.facet_meets(1).unwrap(), vec![0]);
        assert_eq!(design.facet_meets(0).unwrap(), Vec::<usize>::new());
        assert!(design.facet_meets(2).is_err());
    }

    /// `to_text` must render a header block followed by one line per row,
    /// containing the row's own name, angle and mast figures.
    #[test]
    fn to_text_renders_header_and_rows() {
        let design = round_brilliant_design();
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        let text = design.cutting_sheet(&solved).to_text();
        assert!(text.contains("Material: (unset)"));
        assert!(text.contains("Table"));
        assert!(text.contains("Girdle"));
        assert!(text.contains("mast"));
        // 3 header lines + 1 blank separator + 8 rows.
        assert_eq!(text.lines().count(), 3 + 1 + 8);
    }

    /// An index list is formatted as `"-"` when empty and with whole-tooth
    /// integers otherwise.
    #[test]
    fn format_indices_matches_whole_vs_fractional_convention() {
        assert_eq!(format_indices(&[]), "-");
        assert_eq!(format_indices(&[12.0, 24.0]), "12, 24");
        assert_eq!(format_indices(&[11.5]), "11.50");
    }

    fn named_tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: Vec::new(),
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// `diff_tiers` must report a changed angle for a common position, and
    /// mark a tier past either list's end as added/removed.
    #[test]
    fn diff_tiers_reports_changes_and_added_removed_positions() {
        let before = vec![
            named_tier("Table", 0.0, MeetConstraint::ScaleReference(0.3)),
            named_tier("Star", 15.0, MeetConstraint::ScaleReference(0.4)),
        ];
        let mut after = before.clone();
        after[1].angle_deg = 16.0;
        after.push(named_tier(
            "Main",
            34.5,
            MeetConstraint::ScaleReference(0.5),
        ));

        let deltas = diff_tiers(&before, None, &after, None);
        assert_eq!(deltas.len(), 3);
        assert!(!deltas[0].angle_changed());
        assert!(deltas[1].angle_changed());
        assert!(!deltas[1].added());
        assert!(!deltas[1].removed());
        assert!(deltas[2].added());
        assert!(!deltas[2].removed());
    }

    /// A mismatched `solved` length must be treated as "no solved masts",
    /// not a panic.
    #[test]
    fn diff_tiers_ignores_a_mismatched_solved_length() {
        let before = vec![named_tier(
            "Table",
            0.0,
            MeetConstraint::ScaleReference(0.3),
        )];
        let after = before.clone();
        let bogus_solved = [];
        let deltas = diff_tiers(&before, Some(&bogus_solved), &after, None);
        assert_eq!(deltas.len(), 1);
        assert!(deltas[0].mast_before.is_none());
        assert!(deltas[0].mast_after.is_none());
    }
}

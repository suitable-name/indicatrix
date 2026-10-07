//! The cheap, synchronous half of a retarget: which tiers move, where to, and what risk
//! the new angle carries in the target material.
//!
//! Nothing here solves the design, so the dialog can rebuild a [`RetargetPlan`] on every
//! slider tick. Everything that needs a solved stone (are the facets still valid, which
//! masts keep the girdle in place) is in [`super::check`].
//!
//! # What moves and what does not
//!
//! - A crown or pavilion tier at a real slope moves by the critical-angle shift (pavilion)
//!   or by [`CrownShift`]'s policy (crown): by default every crown angle follows the
//!   pavilion's vertical stretch ([`pavilion_stretch`]), so the stone keeps its silhouette
//!   and its table size. A shifted angle never crosses the horizontal,
//!   never gets flatter than [`MIN_RETARGET_ANGLE_DEG`] and never gets steeper than
//!   [`MAX_RETARGET_ANGLE_DEG`]; a row held at one of those limits says so.
//! - A flat tier (the table at `+0.0`, the culet at `-0.0`, any other horizontal plane) is
//!   listed so the cutter sees it stays put, but it is never moved: the angle of a flat
//!   plane has no critical-angle meaning, and "shifting" it is what used to flip the
//!   table onto the pavilion side.
//! - Girdle tiers are not listed at all: they are structural, not optical.
//! - A tier whose angle follows a relation (`P2 = P1 - 2`) is listed too, but it is never
//!   shifted on its own: it moves with the tiers it reads. Its row carries the relation
//!   text in [`PlanRow::follows`], shows the angle the relation gives once the other rows
//!   have moved, and stays out of the proposal (the editor session works the relation out
//!   again when the retarget is applied, in the same undo step).
//!
//! # Risk
//!
//! A pavilion row reads the table-up critical-angle margin. A crown row reads the
//! crown-window estimate against the representative pavilion angle of the CANDIDATE
//! design (the one with every row's new angle in it), the same policy the tier table
//! uses. Flat rows carry no risk, and neither does a crown row of a design that has no
//! pavilion to measure it against.

use super::{CrownShift, RetargetMode, RetargetProposal, RetargetRow, build_notes};
use crate::view_model::row_format::representative_crown_and_pavilion_angles_deg;
use indicatrix::{
    geometry::meet_solver::{Block, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, ResolvedMaterial, Risk, critical_angle_deg, crown_window_margin_deg,
    crown_windowing_risk,
    optics_hints::{
        AngleGuard, MAX_RETARGET_ANGLE_DEG, MIN_RETARGET_ANGLE_DEG, guard_retargeted_angle_deg,
        is_horizontal_angle_deg,
    },
    retarget_angle_deg, tier_margin_deg, windowing_risk,
};

/// The note shown when the plan lists flat tiers.
const FLAT_TIERS_NOTE: &str =
    "Flat facets (the table and the culet) keep their angle: a retarget never tilts them.";

/// The note shown when a crown row carries the crown-window estimate (marked "est."): what
/// the estimate does and does not look at, in plain words.
const CROWN_ESTIMATE_NOTE: &str = "Crown margins marked est. are rough estimates. Each follows one ray down through the crown facet and onto the main pavilion facet on the same side of the stone, and nothing else, so read it as a guide, not a measurement.";

/// One tier's shifted angle, before and after the guard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShiftedAngle {
    /// The shift formula's own result, before the guard.
    pub raw_deg: f64,
    /// The angle the plan uses: `raw_deg` after [`guard_retargeted_angle_deg`].
    pub angle_deg: f64,
    /// What the guard did.
    pub guard: AngleGuard,
}

impl ShiftedAngle {
    /// A tier that is not shifted at all.
    const fn held(angle_deg: f64) -> Self {
        Self {
            raw_deg: angle_deg,
            angle_deg,
            guard: AngleGuard::Within,
        }
    }
}

/// The pavilion's vertical stretch `s = tan(|P'|) / tan(|P|)`.
///
/// `P` is the design's representative pavilion angle (the main tier, else the steepest) and
/// `P'` the angle the shift gives it after the guard. A pavilion turning about its girdle
/// edge reaches `s` times as deep, so a crown that scales its tangents by the same `s` keeps
/// the stone's silhouette (see [`CrownShift`]).
///
/// `1.0` when the design has no pavilion, when the pavilion does not move (the two indices
/// are the same, or `P'` is bitwise `P`), or when `tan(|P|)` is not a finite positive
/// number. Pure arithmetic: nothing is solved.
#[must_use]
pub fn pavilion_stretch(design: &Design, n_from: f64, n_to: f64) -> f64 {
    let Some(magnitude) = representative_crown_and_pavilion_angles_deg(design).1 else {
        return 1.0;
    };
    // `critical + (theta - critical)` need not give `theta` back to the last bit, so the
    // same-index case is told apart by the indices themselves.
    if n_from.to_bits() == n_to.to_bits() {
        return 1.0;
    }
    let raw = retarget_angle_deg(-magnitude, n_from, n_to);
    let (guarded, _) = guard_retargeted_angle_deg(-magnitude, raw);
    if guarded.abs().to_bits() == magnitude.to_bits() {
        return 1.0;
    }
    let before = magnitude.to_radians().tan();
    let after = guarded.abs().to_radians().tan();
    let stretch = after / before;
    if before.is_finite() && before > 0.0 && stretch.is_finite() && stretch > 0.0 {
        stretch
    } else {
        1.0
    }
}

/// The crown counterpart to the pavilion critical-angle shift -- see [`CrownShift`].
///
/// `stretch` is [`pavilion_stretch`]; only the follow-the-pavilion rule reads it.
fn raw_crown_angle(old_angle: f64, n_from: f64, n_to: f64, crown: CrownShift, stretch: f64) -> f64 {
    if crown.scale_by_ratio {
        old_angle * critical_angle_deg(n_to) / critical_angle_deg(n_from)
    } else if crown.follow_pavilion {
        // Nothing to stretch: hand the angle back as it was, not through `atan(tan(..))`.
        if stretch.to_bits() == 1.0_f64.to_bits() {
            return old_angle;
        }
        // Keep the side the way `retarget_angle_deg` does: `-0.0` is a pavilion marker.
        let sign = if old_angle.is_sign_negative() {
            -1.0
        } else {
            1.0
        };
        sign * (stretch * old_angle.abs().to_radians().tan())
            .atan()
            .to_degrees()
    } else {
        crown.fraction.mul_add(
            critical_angle_deg(n_to) - critical_angle_deg(n_from),
            old_angle,
        )
    }
}

/// The shifted angle for one tier of `block`.
///
/// Girdle tiers and flat tiers come back unchanged: neither is ever shifted. `stretch` is
/// the design's [`pavilion_stretch`] (pass `1.0` for none).
#[must_use]
pub fn shift_angle(
    old_angle: f64,
    block: Block,
    n_from: f64,
    n_to: f64,
    crown: CrownShift,
    stretch: f64,
) -> ShiftedAngle {
    if block == Block::Girdle || is_horizontal_angle_deg(old_angle) {
        return ShiftedAngle::held(old_angle);
    }
    let raw_deg = if block == Block::Pavilion {
        retarget_angle_deg(old_angle, n_from, n_to)
    } else {
        raw_crown_angle(old_angle, n_from, n_to, crown, stretch)
    };
    let (angle_deg, guard) = guard_retargeted_angle_deg(old_angle, raw_deg);
    ShiftedAngle {
        raw_deg,
        angle_deg,
        guard,
    }
}

/// One listed tier of a [`RetargetPlan`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRow {
    /// This tier's position in `design.tiers`.
    pub tier_index: usize,
    /// The tier's block (never `Block::Girdle`).
    pub block: Block,
    /// The tier's name.
    pub name: String,
    /// The tier's angle when the plan was built.
    pub old_angle: f64,
    /// The planned angle (equal to `old_angle` for a flat tier).
    pub new_angle: f64,
    /// `false` for a flat tier: listed, never moved.
    pub moves: bool,
    /// What the angle guard did to this row's shifted angle.
    pub guard: AngleGuard,
    /// The row's margin in the target material, when it has one: the critical-angle
    /// margin for a pavilion row, the crown-window estimate for a crown row.
    pub margin_deg: Option<f64>,
    /// The risk band of `margin_deg`.
    pub risk: Option<Risk>,
    /// `true` when `margin_deg` is the crown-window estimate rather than the plain
    /// critical-angle margin.
    pub margin_is_estimate: bool,
    /// The relation this tier's angle follows (`P1 - 2`), when it follows one. Such a row
    /// never `moves` on its own; `new_angle` is the angle the relation gives.
    pub follows: Option<String>,
}

impl PlanRow {
    /// The row as the older [`RetargetRow`] the web app and the Optimize path use.
    ///
    /// A row with no margin (a crown row of a design with no pavilion) reads `0.0` and
    /// [`Risk::Safe`] there: that type has no way to say "not applicable".
    #[must_use]
    pub fn to_legacy(&self) -> RetargetRow {
        RetargetRow {
            tier_index: self.tier_index,
            block: self.block,
            name: self.name.clone(),
            old_angle: self.old_angle,
            new_angle: self.new_angle,
            margin_deg: self.margin_deg.unwrap_or(0.0),
            risk: self.risk.unwrap_or(Risk::Safe),
        }
    }
}

/// What [`build_plan`] returns: every listed tier, the target material, the two
/// refractive indices and the notes to show.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetPlan {
    /// One row per crown or pavilion tier (flat ones included), in schedule order.
    pub rows: Vec<PlanRow>,
    /// The resolved target material.
    pub target: ResolvedMaterial,
    /// The design's refractive index before the retarget.
    pub n_from: f64,
    /// The target's refractive index.
    pub n_to: f64,
    /// The crown policy the plan was built with.
    pub crown: CrownShift,
    /// The design's pavilion stretch ([`pavilion_stretch`]) for this move; `1.0` when
    /// there is nothing to stretch.
    pub stretch: f64,
    /// Plain-English notes: the material move, rows the guard held, the flat tiers, the
    /// tiers that follow a relation.
    pub notes: Vec<String>,
    /// Why the tiers that follow a relation could not follow it once the others had moved
    /// (the result is not a facet angle, or the relations read each other in a loop).
    /// `None` when there are no relations, or they all work out.
    pub relation_error: Option<String>,
}

impl RetargetPlan {
    /// How many rows move on their own (the tiers a search may vary).
    #[must_use]
    pub fn moving_count(&self) -> usize {
        self.rows.iter().filter(|row| row.moves).count()
    }

    /// `(tier_index, angle)` of every moving row at `fraction` of the way from its old
    /// angle to its planned one (`0.0` is the design as it is, `1.0` the plan).
    ///
    /// Both ends are inside the allowed range and on the same side of the horizontal, so
    /// every point between them is too.
    #[must_use]
    pub fn moving_angles_at(&self, fraction: f64) -> Vec<(usize, f64)> {
        self.rows
            .iter()
            .filter(|row| row.moves)
            .map(|row| {
                (
                    row.tier_index,
                    (row.new_angle - row.old_angle).mul_add(fraction, row.old_angle),
                )
            })
            .collect()
    }

    /// This plan with the angles in `angles` (`(tier_index, angle)`) in place of the
    /// planned ones, for showing a result that came from somewhere other than the shift
    /// formula (an Optimize candidate).
    ///
    /// A moving row that `angles` does not name keeps its old angle. The tiers that follow
    /// a relation, the margins and the risks are worked out again for the new angles; the
    /// notes are rebuilt (the shift guard's notes no longer apply).
    #[must_use]
    pub fn with_angles(&self, design: &Design, angles: &[(usize, f64)]) -> Self {
        let mut rows = self.rows.clone();
        for row in &mut rows {
            if row.moves {
                row.new_angle = angles
                    .iter()
                    .find(|&&(index, _)| index == row.tier_index)
                    .map_or(row.old_angle, |&(_, angle)| angle);
                row.guard = AngleGuard::Within;
            }
            if row.moves || row.follows.is_some() {
                row.margin_deg = None;
                row.risk = None;
                row.margin_is_estimate = false;
            }
        }
        let relation_error = complete_rows(design, &mut rows, self.n_to);
        // The angles no longer come from the shift formula, so the crown-follows-pavilion
        // note does not apply: no stretch is passed.
        let mut notes = build_notes(RetargetMode::Shift, self.n_from, self.n_to, self.crown, 1.0);
        if rows.iter().any(|row| !row.moves && row.follows.is_none()) {
            notes.push(FLAT_TIERS_NOTE.to_string());
        }
        push_crown_estimate_note(&mut notes, &rows);
        notes.extend(relation_notes(
            &rows,
            &tier_display_names(design),
            relation_error.as_deref(),
        ));
        Self {
            rows,
            target: self.target.clone(),
            n_from: self.n_from,
            n_to: self.n_to,
            crown: self.crown,
            stretch: self.stretch,
            notes,
            relation_error,
        }
    }

    /// The rows that actually move, as the older proposal type.
    #[must_use]
    pub fn proposal(&self) -> RetargetProposal {
        RetargetProposal {
            rows: self
                .rows
                .iter()
                .filter(|row| row.moves)
                .map(PlanRow::to_legacy)
                .collect(),
            target: self.target.clone(),
            notes: self.notes.clone(),
        }
    }

    /// `(tier_index, new_angle)` of every row that moves.
    #[must_use]
    pub fn moving_angles(&self) -> Vec<(usize, f64)> {
        self.rows
            .iter()
            .filter(|row| row.moves)
            .map(|row| (row.tier_index, row.new_angle))
            .collect()
    }

    /// `true` when no row moves, so there is nothing to apply.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|row| !row.moves)
    }
}

/// The margin, risk and whether the margin is the crown estimate, for one row.
///
/// `pavilion_partner_deg` is the representative pavilion angle of the candidate design.
/// `None` for a girdle row, and for a crown row when there is no pavilion partner.
#[must_use]
pub fn margin_and_risk(
    block: Block,
    angle_deg: f64,
    n_to: f64,
    pavilion_partner_deg: Option<f64>,
) -> Option<(f64, Risk, bool)> {
    match block {
        Block::Pavilion => Some((
            tier_margin_deg(angle_deg, n_to),
            windowing_risk(angle_deg, n_to),
            false,
        )),
        Block::Crown => pavilion_partner_deg.map(|partner| {
            (
                crown_window_margin_deg(partner, angle_deg, n_to),
                crown_windowing_risk(partner, angle_deg, n_to),
                true,
            )
        }),
        Block::Girdle => None,
    }
}

/// The representative pavilion angle of `design` once every `(tier_index, angle)` of
/// `angles` is applied to a copy of it -- the partner a crown row's risk is read against.
#[must_use]
pub fn pavilion_partner_after(design: &Design, angles: &[(usize, f64)]) -> Option<f64> {
    let mut candidate = design.clone();
    for &(index, angle) in angles {
        if let Some(tier) = candidate.tiers.get_mut(index) {
            tier.angle_deg = angle;
        }
    }
    representative_crown_and_pavilion_angles_deg(&candidate).1
}

/// Sets every moving row's margin and risk, against the candidate design's pavilion.
fn fill_risks(design: &Design, rows: &mut [PlanRow], n_to: f64) {
    let angles: Vec<(usize, f64)> = rows
        .iter()
        .map(|row| (row.tier_index, row.new_angle))
        .collect();
    let partner = pavilion_partner_after(design, &angles);
    for row in rows
        .iter_mut()
        .filter(|row| row.moves || row.follows.is_some())
    {
        if let Some((margin, risk, estimate)) =
            margin_and_risk(row.block, row.new_angle, n_to, partner)
        {
            row.margin_deg = Some(margin);
            row.risk = Some(risk);
            row.margin_is_estimate = estimate;
        }
    }
}

/// Gives every row that follows a relation the angle its relation produces once the other
/// rows have their new angles. Returns the reason when the relations cannot be satisfied
/// (the followers then keep their old angle).
fn follow_relations(design: &Design, rows: &mut [PlanRow]) -> Option<String> {
    if rows.iter().all(|row| row.follows.is_none()) {
        return None;
    }
    let mut candidate = design.clone();
    for row in rows.iter().filter(|row| row.follows.is_none()) {
        if let Some(tier) = candidate.tiers.get_mut(row.tier_index) {
            tier.angle_deg = row.new_angle;
        }
    }
    match candidate.evaluate_relations() {
        Ok(updates) => {
            for (index, angle) in updates {
                if let Some(row) = rows.iter_mut().find(|row| row.tier_index == index) {
                    row.new_angle = angle;
                }
            }
            None
        }
        Err(error) => Some(error.to_string()),
    }
}

/// Works out the followers' angles, then every listed margin and risk. Returns what
/// [`follow_relations`] returns.
fn complete_rows(design: &Design, rows: &mut [PlanRow], n_to: f64) -> Option<String> {
    let relation_error = follow_relations(design, rows);
    fill_risks(design, rows, n_to);
    relation_error
}

/// The name a person reads for every tier of `design`, in schedule order.
///
/// The tier's own name, or its standard code (`P1`, `C2`, `T`) when the name is empty or an
/// old-style label (`1`, `A`). The same rule as the tier table's NAME column.
#[must_use]
pub fn tier_display_names(design: &Design) -> Vec<String> {
    let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            if tier.name.trim().is_empty() || indicatrix_cut_core::is_legacy_123_abc(&tier.name) {
                labels
                    .get(index)
                    .map_or_else(|| tier.name.clone(), |label| label.code.clone())
            } else {
                tier.name.clone()
            }
        })
        .collect()
}

/// The name a person reads for tier `index` of `design` ([`tier_display_names`]), or
/// [`Design::relation_label`] (`tier 7`) when there is no such tier.
#[must_use]
pub fn tier_display_name(design: &Design, index: usize) -> String {
    tier_display_names(design)
        .into_iter()
        .nth(index)
        .unwrap_or_else(|| design.relation_label(index))
}

/// The notes about tiers that follow a relation: one line per follower, and the reason
/// when the relations cannot be satisfied. `names` is [`tier_display_names`].
fn relation_notes(rows: &[PlanRow], names: &[String], relation_error: Option<&str>) -> Vec<String> {
    let mut notes: Vec<String> = rows
        .iter()
        .filter_map(|row| {
            row.follows.as_ref().map(|relation| {
                let name = names.get(row.tier_index).unwrap_or(&row.name);
                format!("{name} follows {relation}, so it moves with the tiers it reads.")
            })
        })
        .collect();
    if let Some(error) = relation_error {
        notes.push(format!(
            "A tier that follows a relation cannot follow it after this change: {}",
            error.trim_end_matches('.')
        ));
    }
    notes
}

/// Adds [`CROWN_ESTIMATE_NOTE`] when any row's margin is the crown-window estimate.
fn push_crown_estimate_note(notes: &mut Vec<String>, rows: &[PlanRow]) {
    if rows.iter().any(|row| row.margin_is_estimate) {
        notes.push(CROWN_ESTIMATE_NOTE.to_string());
    }
}

/// The note that explains why a row was held at a limit, if it was. The held angle is
/// written as a magnitude, like every angle a person reads.
fn guard_note(name: &str, old_angle: f64, shifted: &ShiftedAngle) -> Option<String> {
    let held = shifted.angle_deg.abs();
    match shifted.guard {
        AngleGuard::Within => None,
        AngleGuard::HeldAtMinimum => {
            let sign = if old_angle.is_sign_negative() {
                -1.0
            } else {
                1.0
            };
            if shifted.raw_deg * sign <= 0.0 {
                Some(format!(
                    "{name}: the shift would tilt it past the horizontal, so it is held at {held:.2}\u{b0}."
                ))
            } else {
                Some(format!(
                    "{name}: the shift would make it flatter than {MIN_RETARGET_ANGLE_DEG:.0}\u{b0}, so it is held at {held:.2}\u{b0}."
                ))
            }
        }
        AngleGuard::HeldAtMaximum => Some(format!(
            "{name}: the shift would make it steeper than {MAX_RETARGET_ANGLE_DEG:.1}\u{b0}, so it is held at {held:.2}\u{b0}."
        )),
    }
}

/// [`build_plan`] with the "from" index already resolved.
#[must_use]
pub fn build_plan_from(
    design: &Design,
    n_from: f64,
    target: &ResolvedMaterial,
    crown: CrownShift,
) -> RetargetPlan {
    let n_to = target.n_d;
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let stretch = pavilion_stretch(design, n_from, n_to);
    let mut notes = build_notes(RetargetMode::Shift, n_from, n_to, crown, stretch);
    let names = tier_display_names(design);
    let mut rows = Vec::new();
    let mut any_flat = false;
    for (index, tier) in design.tiers.iter().enumerate() {
        let block = blocks[index];
        if block == Block::Girdle {
            continue;
        }
        let follows = design.relation_text(index);
        let flat = is_horizontal_angle_deg(tier.angle_deg);
        // A tier that follows a relation is never shifted on its own.
        let shifted = if follows.is_some() {
            ShiftedAngle::held(tier.angle_deg)
        } else {
            shift_angle(tier.angle_deg, block, n_from, n_to, crown, stretch)
        };
        any_flat |= flat;
        if follows.is_none()
            && let Some(note) = guard_note(&names[index], tier.angle_deg, &shifted)
        {
            notes.push(note);
        }
        rows.push(PlanRow {
            tier_index: index,
            block,
            name: tier.name.clone(),
            old_angle: tier.angle_deg,
            new_angle: shifted.angle_deg,
            moves: follows.is_none() && !flat,
            guard: shifted.guard,
            margin_deg: None,
            risk: None,
            margin_is_estimate: false,
            follows,
        });
    }
    if any_flat {
        notes.push(FLAT_TIERS_NOTE.to_string());
    }
    let relation_error = complete_rows(design, &mut rows, n_to);
    push_crown_estimate_note(&mut notes, &rows);
    notes.extend(relation_notes(&rows, &names, relation_error.as_deref()));
    RetargetPlan {
        rows,
        target: target.clone(),
        n_from,
        n_to,
        crown,
        stretch,
        notes,
        relation_error,
    }
}

/// Builds the Shift-mode plan for `design` against `target`.
///
/// The "from" index resolves through [`Design::effective_refractive_index_with`] against
/// `custom_materials`, exactly as [`super::build_proposal`] does.
#[must_use]
pub fn build_plan(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    custom_materials: &[GemMaterial],
) -> RetargetPlan {
    build_plan_from(
        design,
        design.effective_refractive_index_with(custom_materials),
        target,
        crown,
    )
}

/// Legacy rows for tiers whose final angles come from somewhere other than the shift
/// formula (the Optimize search).
///
/// `entries` is `(tier_index, block, new_angle)` per tier; each row's `old_angle` is read
/// from `original`, and its risk from the design with every entry's angle applied.
#[must_use]
pub fn legacy_rows(
    original: &Design,
    entries: &[(usize, Block, f64)],
    n_to: f64,
) -> Vec<RetargetRow> {
    let angles: Vec<(usize, f64)> = entries
        .iter()
        .map(|&(index, _, angle)| (index, angle))
        .collect();
    let partner = pavilion_partner_after(original, &angles);
    entries
        .iter()
        .map(|&(index, block, new_angle)| {
            let (margin, risk, _) = margin_and_risk(block, new_angle, n_to, partner).unwrap_or((
                0.0,
                Risk::Safe,
                false,
            ));
            RetargetRow {
                tier_index: index,
                block,
                name: original.tiers[index].name.clone(),
                old_angle: original.tiers[index].angle_deg,
                new_angle,
                margin_deg: margin,
                risk,
            }
        })
        .collect()
}

#[cfg(test)]
mod guard_note_tests {
    use super::*;

    fn shifted(raw_deg: f64, angle_deg: f64, guard: AngleGuard) -> ShiftedAngle {
        ShiftedAngle {
            raw_deg,
            angle_deg,
            guard,
        }
    }

    #[test]
    fn a_pavilion_row_held_at_the_steep_limit_reads_the_held_angle_as_positive() {
        let note = guard_note(
            "P1",
            -88.0,
            &shifted(-92.0, -MAX_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMaximum),
        );
        assert_eq!(
            note.as_deref(),
            Some(
                "P1: the shift would make it steeper than 89.5\u{b0}, so it is held at 89.50\u{b0}."
            )
        );
    }

    #[test]
    fn a_pavilion_row_held_at_the_flat_limit_reads_the_held_angle_as_positive() {
        // Stored as -3 degrees, the raw shift stays on the same side but is flatter than 1.
        let flatter = guard_note(
            "P1",
            -3.0,
            &shifted(-0.5, -MIN_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMinimum),
        );
        assert_eq!(
            flatter.as_deref(),
            Some("P1: the shift would make it flatter than 1\u{b0}, so it is held at 1.00\u{b0}.")
        );
        // The raw shift crosses to the other side of the horizontal.
        let crossed = guard_note(
            "P1",
            -3.0,
            &shifted(0.4, -MIN_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMinimum),
        );
        assert_eq!(
            crossed.as_deref(),
            Some("P1: the shift would tilt it past the horizontal, so it is held at 1.00\u{b0}.")
        );
    }

    #[test]
    fn a_row_within_the_limits_has_no_note() {
        assert_eq!(guard_note("P1", -40.0, &ShiftedAngle::held(-40.0)), None);
    }

    #[test]
    fn display_names_turn_old_style_names_into_codes_and_keep_descriptive_ones() {
        use indicatrix::geometry::meet_solver::MeetConstraint;
        use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

        let tier = |name: &str, angle_deg: f64| ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        };
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::default(),
            vec![
                tier("Pavilion Main", -41.0),
                tier("1", -38.0),
                tier("", 32.0),
            ],
        );
        let names = tier_display_names(&design);
        let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
        assert_eq!(names[0], "Pavilion Main", "a descriptive name stays");
        assert_eq!(
            names[1], labels[1].code,
            "an old-style name becomes the code"
        );
        assert_eq!(names[2], labels[2].code, "an empty name becomes the code");
        assert!(
            names[1].starts_with('P') && names[2].starts_with('C'),
            "{names:?}"
        );
    }
}

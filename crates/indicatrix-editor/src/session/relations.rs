//! Parametric tier relations in the editor session: a tier's angle driven by an
//! expression over other tiers' angles (`P2 = P1 - 2`), kept true through every edit,
//! undo and redo.
//!
//! The engine lives in `indicatrix_cut_core::design` (`TierRelation`,
//! `Design::evaluate_relations`) and the file format carries relations; this module is
//! the part that makes them behave in an editor:
//!
//! - Every edit that goes through [`EditorSession::try_apply`] (and so
//!   [`EditorSession::apply`], [`EditorSession::apply_coalescing`] and every convenience
//!   action built on them) is checked against the design's relations first. The edit is
//!   tried on a copy; the angles the relations then give are folded into the SAME
//!   [`Edit::Batch`], so one undo step reverts the edit and every tier that followed it,
//!   exactly. A design without relations skips all of this.
//! - An edit that directly changes a tier whose angle follows a relation is refused with
//!   [`SessionEditError::Driven`] ("This angle follows a relation (P2 = P1 - 2). Edit
//!   the relation or remove it."). An edit after which the relations cannot be satisfied
//!   (a loop, a result outside `(0, 90]` degrees) is refused with
//!   [`SessionEditError::Relation`]. Nothing changes in either case.
//! - An edit that replaces the relations wholesale ([`Edit::ReplaceSchedule`], alone or
//!   inside a batch) is the exception to that refusal: the tiers it moves are moved to the
//!   angles it carries, and the relations it carries are the ones that hold afterwards (the
//!   fold below still makes them true). Only a later part of the same batch that moves a
//!   driven tier of the NEW schedule is refused.
//! - Removing a tier that other relations read also clears those relations in the same
//!   undo step: the tiers keep their current angles, and
//!   [`EditorSession::take_relation_notice`] says which ones. A notice nobody took lasts
//!   until the next edit, undo, redo or jump; it never reaches a later, unrelated message.
//!
//! # Errors of the old `apply` family
//!
//! [`EditorSession::apply`] and the actions built on it return a bare `EditError`, which
//! has no room for a message. A refusal there carries the driven tier's index and keeps
//! its typed reason for [`EditorSession::take_refusal`]; new code should call
//! [`EditorSession::try_apply`] and show the [`SessionEditError`]'s text.

use super::{EditChange, EditorSession};
use indicatrix_cut_core::{
    Design, Edit, EditError, History, OptimizeOutcome, TierId,
    design::{RelationError, TierRelation},
};
use std::{fmt, time::Duration};

#[cfg(test)]
mod tests;

/// How far a driven tier's angle may differ from its old value in an edit and still
/// count as untouched (a form round trip through text adds noise this small).
const DRIVEN_ANGLE_TOLERANCE_DEG: f64 = 1e-6;

/// A tier whose angle follows a relation, refused a direct edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRefusal {
    /// The driven tier's position.
    pub tier: usize,
    /// Its name for messages (`P2`, or `tier 3` when it has none).
    pub label: String,
    /// Its relation as a cutter reads it (`P1 - 2`).
    pub relation: String,
}

impl fmt::Display for RelationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "This angle follows a relation ({} = {}). Edit the relation or remove it.",
            self.label, self.relation
        )
    }
}

/// Why [`EditorSession::try_apply`] (or a relation method) changed nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEditError {
    /// The edit itself is invalid (a tier that does not exist).
    Edit(EditError),
    /// The edit changes the angle of a tier that follows a relation.
    Driven(RelationRefusal),
    /// The relations cannot be satisfied after the edit, or the relation asked for is
    /// not usable.
    Relation(RelationError),
}

impl fmt::Display for SessionEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edit(error) => fmt::Display::fmt(error, f),
            Self::Driven(refusal) => fmt::Display::fmt(refusal, f),
            Self::Relation(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for SessionEditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Edit(error) => Some(error),
            Self::Driven(_) => None,
            Self::Relation(error) => Some(error),
        }
    }
}

impl From<EditError> for SessionEditError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

impl From<RelationError> for SessionEditError {
    fn from(error: RelationError) -> Self {
        Self::Relation(error)
    }
}

/// One relation an edit freed because a tier it read was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearedRelation {
    /// The tier that followed it (`P2`).
    pub tier: String,
    /// What it read, in the names before the removal (`P1 - 2`).
    pub relation: String,
}

/// The relations an edit cleared because a tier they read was removed: the tiers keep
/// their current angles but follow nothing now. See
/// [`EditorSession::take_relation_notice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationNotice {
    /// The freed relations, in tier order.
    pub cleared: Vec<ClearedRelation>,
}

impl fmt::Display for RelationNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = self
            .cleared
            .iter()
            .map(|cleared| {
                format!(
                    "{} (was {} = {})",
                    cleared.tier, cleared.tier, cleared.relation
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if self.cleared.len() == 1 {
            write!(
                f,
                "{list} no longer follows a relation because a tier it read was removed. \
                 It keeps its current angle."
            )
        } else {
            write!(
                f,
                "{list} no longer follow a relation because a tier they read was removed. \
                 They keep their current angles."
            )
        }
    }
}

/// Whether `edit` (or a part of a batch) sets a relation: one relation, or a whole-schedule
/// replacement that brings relations with it.
///
/// The second kind must be evaluated like the first, or a replacement whose driven angles do
/// not match its relations (a variant or an import made elsewhere) would stand as given
/// until some later edit happened to fix them.
fn sets_a_relation(edit: &Edit) -> bool {
    match edit {
        Edit::SetTierRelation {
            relation: Some(_), ..
        } => true,
        Edit::ReplaceSchedule(state) => !state.tier_relations.is_empty(),
        Edit::Batch(edits) => edits.iter().any(sets_a_relation),
        _ => false,
    }
}

/// Whether `edit` (or a part of a batch) replaces the whole schedule, relations included.
fn replaces_the_schedule(edit: &Edit) -> bool {
    match edit {
        Edit::ReplaceSchedule(_) => true,
        Edit::Batch(edits) => edits.iter().any(replaces_the_schedule),
        _ => false,
    }
}

/// Refuses an edit (already tried, giving `trial`) that changed the angle of a tier that
/// follows a relation in `before`. A tier whose relation the same edit clears is free.
fn refuse_direct_change_of_driven(before: &Design, trial: &Design) -> Result<(), SessionEditError> {
    for id in before.tier_relations.keys() {
        if !trial.tier_relations.contains_key(id) {
            continue;
        }
        let (Some(was), Some(now)) = (before.index_of_tier_id(*id), trial.index_of_tier_id(*id))
        else {
            continue;
        };
        let moved = (trial.tiers[now].angle_deg - before.tiers[was].angle_deg).abs();
        if moved > DRIVEN_ANGLE_TOLERANCE_DEG {
            return Err(SessionEditError::Driven(RelationRefusal {
                tier: was,
                label: before.relation_label(was),
                relation: before.relation_text(was).unwrap_or_default(),
            }));
        }
    }
    Ok(())
}

/// `design` as it stands right after the last whole-schedule replacement inside `edit`
/// (the edit itself or a part of a batch), or `None` when `edit` replaces no schedule.
///
/// That state is what the driven tiers of the edit's result are compared with: a
/// replacement defines which tiers follow a relation and where they stand, and only what
/// the edit does AFTER it can count as a direct change of a driven tier.
fn state_after_schedule_replacement(design: &Design, edit: &Edit) -> Option<Design> {
    if !replaces_the_schedule(edit) {
        return None;
    }
    match edit {
        Edit::Batch(edits) => {
            let mut running = design.clone();
            let mut baseline = None;
            for part in edits {
                if let Some(found) = state_after_schedule_replacement(&running, part) {
                    baseline = Some(found);
                }
                running.apply_edit(part.clone()).ok()?;
            }
            baseline
        }
        replacement => {
            let mut after = design.clone();
            after.apply_edit(replacement.clone()).ok()?;
            Some(after)
        }
    }
}

impl EditorSession {
    /// Applies `edit` through [`History::apply`], keeping tier relations true: see the
    /// module documentation. Like [`Self::apply`], but a refusal comes back typed.
    ///
    /// # Errors
    ///
    /// [`SessionEditError::Edit`] with [`History::apply`]'s error;
    /// [`SessionEditError::Driven`] when the edit changes a tier that follows a
    /// relation; [`SessionEditError::Relation`] when the relations cannot be satisfied
    /// afterwards. The design and history are untouched on `Err`.
    pub fn try_apply(&mut self, edit: Edit) -> Result<EditChange, SessionEditError> {
        self.try_apply_mapped(edit, None).map(|(change, _)| change)
    }

    /// [`Self::try_apply`], also returning how the edit (with the relation updates folded
    /// into it) renumbered the tier rows, for a caller that keeps a row selection of its
    /// own (the desktop's single selected row) and must make it follow its tier the way
    /// [`Self::multi_selected`] does. See [`super::TierIndexMap::map_table_row`].
    ///
    /// `label`, when it holds any text, words the history step instead of
    /// [`Edit::describe`] ([`History::apply_labeled`]): for an edit whose own words do not
    /// say what the cutter did, like opening a saved variant.
    ///
    /// # Errors
    ///
    /// As [`Self::try_apply`].
    pub fn try_apply_mapped(
        &mut self,
        edit: Edit,
        label: Option<&str>,
    ) -> Result<(EditChange, super::TierIndexMap), SessionEditError> {
        self.expire_relation_notice();
        let before = self.tier_counts();
        let (edit, notice) = self.with_relations_folded(edit)?;
        let renumbering = super::TierIndexMap::of(&edit);
        let applied = match label {
            Some(label) => self.history.apply_labeled(&mut self.design, edit, label),
            None => self.history.apply(&mut self.design, edit),
        };
        applied.map_err(SessionEditError::Edit)?;
        self.keep_relation_notice(notice);
        let change = self.record_change(before, Some(&renumbering));
        Ok((change, renumbering))
    }

    /// [`Self::try_apply`] through [`History::apply_coalescing`]: the same `key` within
    /// the window merges into the previous undo step, folded relation updates and all.
    ///
    /// # Errors
    ///
    /// As [`Self::try_apply`].
    pub fn try_apply_coalescing(
        &mut self,
        edit: Edit,
        key: u64,
        now: Duration,
    ) -> Result<EditChange, SessionEditError> {
        self.expire_relation_notice();
        let before = self.tier_counts();
        let (edit, notice) = self.with_relations_folded(edit)?;
        let renumbering = super::TierIndexMap::of(&edit);
        self.history
            .apply_coalescing(&mut self.design, edit, key, now)
            .map_err(SessionEditError::Edit)?;
        self.keep_relation_notice(notice);
        Ok(self.record_change(before, Some(&renumbering)))
    }

    /// The bare `EditError` the old `apply` family returns for `error`. A refusal keeps
    /// its typed reason for [`Self::take_refusal`].
    pub(super) fn legacy_edit_error(&mut self, error: SessionEditError) -> EditError {
        let tier_count = self.design.tiers.len();
        match error {
            SessionEditError::Edit(error) => error,
            refusal => {
                let index = match &refusal {
                    SessionEditError::Driven(driven) => driven.tier,
                    _ => 0,
                };
                self.refusal = Some(refusal);
                EditError { index, tier_count }
            }
        }
    }

    /// The typed reason the last edit of the old `apply` family was refused because of
    /// a tier relation, if any; taking it clears it. Call it right after an `Err` from
    /// [`Self::apply`] (or an action built on it) to show the cutter the real message.
    pub const fn take_refusal(&mut self) -> Option<SessionEditError> {
        self.refusal.take()
    }

    /// The relations the last edit cleared because a tier they read was removed; taking
    /// it clears it. The edits of one command that removes several tiers add up.
    ///
    /// A notice nobody took does not wait for a later, unrelated message: the next edit,
    /// undo, redo or jump drops it. So take it right after the removal.
    pub const fn take_relation_notice(&mut self) -> Option<RelationNotice> {
        self.relation_notice.take()
    }

    /// Drops the notice of an earlier edit, unless a command of several edits is running
    /// (its edits add up). Called where every edit, undo, redo and jump starts.
    pub(super) fn expire_relation_notice(&mut self) {
        if !self.notice_command_open {
            self.relation_notice = None;
        }
    }

    /// Starts a command of several edits that report one notice together: drops the
    /// notice of anything earlier, then keeps adding to it until
    /// [`Self::end_relation_notice_command`].
    pub(super) fn begin_relation_notice_command(&mut self) {
        self.relation_notice = None;
        self.notice_command_open = true;
    }

    /// Ends what [`Self::begin_relation_notice_command`] started. The notice stays for
    /// [`Self::take_relation_notice`].
    pub(super) const fn end_relation_notice_command(&mut self) {
        self.notice_command_open = false;
    }

    /// Adds the notice of an edit that has just been applied to the one waiting.
    fn keep_relation_notice(&mut self, notice: Option<RelationNotice>) {
        if let Some(notice) = notice {
            self.add_relation_notice(notice);
        }
    }

    /// Makes `edit` safe for a design with relations and folds in what the relations
    /// then require, with the notice for the relations it clears. Returns `edit`
    /// unchanged when nothing needs folding (and always for a design without relations
    /// that sets none).
    fn with_relations_folded(
        &self,
        edit: Edit,
    ) -> Result<(Edit, Option<RelationNotice>), SessionEditError> {
        if self.design.tier_relations.is_empty() && !sets_a_relation(&edit) {
            return Ok((edit, None));
        }
        let mut trial = self.design.clone();
        if trial.apply_edit(edit.clone()).is_err() {
            // The edit is invalid by itself; `History` reports it the usual way.
            return Ok((edit, None));
        }
        // A whole-schedule replacement says which tiers follow a relation: compare with
        // what it says, not with the design it replaces.
        let replaced = state_after_schedule_replacement(&self.design, &edit);
        refuse_direct_change_of_driven(replaced.as_ref().unwrap_or(&self.design), &trial)?;
        let (clears, notice) = self.dangling_relation_clears(&mut trial);
        let updates = trial
            .evaluate_relations()
            .map_err(SessionEditError::Relation)?;
        let changes: Vec<(usize, f64, f64)> = updates
            .into_iter()
            .filter_map(|(position, new_deg)| {
                let old_deg = trial.tiers.get(position)?.angle_deg;
                (old_deg.to_bits() != new_deg.to_bits()).then_some((position, old_deg, new_deg))
            })
            .collect();
        if clears.is_empty() && changes.is_empty() {
            return Ok((edit, None));
        }
        let mut edits = Vec::with_capacity(2 + clears.len());
        edits.push(edit);
        edits.extend(clears);
        if !changes.is_empty() {
            edits.push(Edit::RetargetAngles { changes });
        }
        Ok((Edit::Batch(edits), notice))
    }

    /// The `SetTierRelation` clears for every relation in `trial` that reads a tier
    /// the edit removed (also dropped from `trial`, so the evaluation that follows sees
    /// them gone), with the notice that names them.
    fn dangling_relation_clears(&self, trial: &mut Design) -> (Vec<Edit>, Option<RelationNotice>) {
        let mut dangling: Vec<(TierId, usize)> = trial
            .tier_relations
            .iter()
            .filter(|(_, relation)| {
                relation
                    .references()
                    .iter()
                    .any(|reference| trial.index_of_tier_id(*reference).is_none())
            })
            .filter_map(|(id, _)| trial.index_of_tier_id(*id).map(|position| (*id, position)))
            .collect();
        dangling.sort_by_key(|&(_, position)| position);
        let mut clears = Vec::with_capacity(dangling.len());
        let mut cleared = Vec::with_capacity(dangling.len());
        for (id, position) in dangling {
            // Names as they were before the removal, so the notice can still say what
            // the relation read.
            let before = self.design.index_of_tier_id(id);
            let describe = |design: &Design, index: usize| {
                (design.relation_label(index), design.relation_text(index))
            };
            let (tier, relation) = before.map_or_else(
                || describe(trial, position),
                |index| describe(&self.design, index),
            );
            cleared.push(ClearedRelation {
                tier,
                relation: relation.unwrap_or_default(),
            });
            trial.tier_relations.remove(&id);
            clears.push(Edit::SetTierRelation {
                index: position,
                relation: None,
            });
        }
        let notice = (!cleared.is_empty()).then_some(RelationNotice { cleared });
        (clears, notice)
    }

    /// Adds `notice` to the one waiting to be taken.
    fn add_relation_notice(&mut self, notice: RelationNotice) {
        match &mut self.relation_notice {
            Some(waiting) => waiting.cleared.extend(notice.cleared),
            None => self.relation_notice = Some(notice),
        }
    }

    /// Makes the angle of tier `tier` follow the relation written in `text` (`P1 - 2`,
    /// `(P1 + P3) / 2`; a leading `=` is ignored), as one undo step that also moves the
    /// tier to the angle the relation gives. Tier names in `text` resolve exactly
    /// first, then ignoring ASCII case; `[Crown Main]` names a tier with spaces.
    ///
    /// `Ok(None)` when the tier already follows exactly this relation.
    ///
    /// # Errors
    ///
    /// [`SessionEditError::Relation`] when the tier is a table, culet or girdle tier
    /// (they cannot follow a relation), `text` cannot be read, it names a tier that does
    /// not exist, tiers would read each other in a loop, or the result is not a facet
    /// angle (more than 0 and at most 90 degrees). Nothing changes on `Err`.
    pub fn set_tier_relation(
        &mut self,
        tier: usize,
        text: &str,
    ) -> Result<Option<EditChange>, SessionEditError> {
        self.design.check_relation_target(tier)?;
        let relation = self.design.parse_relation(text)?;
        if self.design.tier_relation(tier) == Some(&relation) {
            return Ok(None);
        }
        self.try_apply(Edit::SetTierRelation {
            index: tier,
            relation: Some(relation),
        })
        .map(Some)
    }

    /// Frees tier `tier` from its relation; it keeps its current angle. One undo step.
    ///
    /// `Ok(None)` when the tier follows no relation (or does not exist).
    ///
    /// # Errors
    ///
    /// [`Self::try_apply`]'s error.
    pub fn clear_tier_relation(
        &mut self,
        tier: usize,
    ) -> Result<Option<EditChange>, SessionEditError> {
        if self.design.tier_relation(tier).is_none() {
            return Ok(None);
        }
        self.try_apply(Edit::SetTierRelation {
            index: tier,
            relation: None,
        })
        .map(Some)
    }

    /// The relation of tier `tier` as a cutter reads it (`P1 - 2`), if it has one.
    #[must_use]
    pub fn tier_relation_display(&self, tier: usize) -> Option<String> {
        self.design.relation_text(tier)
    }

    /// Whether the angle of tier `tier` follows a relation.
    #[must_use]
    pub fn is_driven(&self, tier: usize) -> bool {
        self.design.is_tier_driven(tier)
    }

    /// The tiers (positions, ascending) tier `tier`'s relation reads; empty when it
    /// follows none.
    #[must_use]
    pub fn drivers_of(&self, tier: usize) -> Vec<usize> {
        self.design.relation_drivers(tier)
    }

    /// The tiers (positions, ascending) whose relation reads tier `tier` directly.
    #[must_use]
    pub fn dependants_of(&self, tier: usize) -> Vec<usize> {
        self.design.relation_dependants(tier)
    }

    /// [`Self::apply_optimize_outcome`] for a design with relations: the outcome's
    /// changes are worked out on a copy, then applied as one folded batch, so the tiers
    /// that follow a changed tier move with it, in the same undo step. A change to a
    /// tier that itself follows a relation is refused like any direct edit.
    pub(super) fn apply_optimize_outcome_with_relations(
        &mut self,
        outcome: &OptimizeOutcome,
    ) -> Result<usize, EditError> {
        let mut scratch = self.design.clone();
        let mut scratch_history = History::new();
        let applied = match indicatrix_cut_core::apply_optimize_outcome(
            &mut scratch_history,
            &mut scratch,
            outcome,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Err(error);
            }
        };
        if applied == 0 {
            return Ok(0);
        }
        let edits: Vec<Edit> = scratch
            .tiers
            .iter()
            .enumerate()
            .filter(|(index, tier)| self.design.tiers.get(*index) != Some(*tier))
            .map(|(index, tier)| Edit::ModifyTier {
                index,
                tier: tier.clone(),
            })
            .collect();
        if edits.is_empty() {
            return Ok(0);
        }
        match self.try_apply(Edit::Batch(edits)) {
            Ok(_) => Ok(applied),
            Err(error) => {
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(self.legacy_edit_error(error))
            }
        }
    }
}

/// The relation a linked step series gives rung `rung` (the first rung is 0): the first
/// rung's angle magnitude plus `rung` steps, so moving the first rung moves the whole
/// ladder. `side` is `-1.0` for a pavilion ladder (a step of -5 degrees away from the
/// girdle is +5 in magnitude) and `1.0` for a crown ladder.
#[must_use]
pub(super) fn linked_rung_relation(
    first: TierId,
    rung: usize,
    step_deg: f64,
    side: f64,
) -> TierRelation {
    let delta = indicatrix_cut_core::design::snap_noise(rung as f64 * step_deg * side);
    TierRelation::new(indicatrix_cut_core::design::RelationExpr::offset_from(
        first, delta,
    ))
}

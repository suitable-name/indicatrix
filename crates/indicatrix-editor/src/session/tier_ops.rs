//! The tier table's structural edits, as [`EditorSession`] methods: Duplicate, Remove,
//! Move, Detach/Reattach, the multi-select batch delete, Adopt (all/selected), the
//! "Generate steps" ladder, "Mirror to the other block" and the inline angle cell's text
//! commit.
//!
//! Every one goes through [`EditorSession::apply`] (so `History` stays the only thing
//! that mutates `Design`) and returns what the UI needs to announce the change and to
//! decide how much to refresh. The desktop's tier-list callbacks and the web app's tier
//! table both call these, so the two edit identically.

use super::{EditChange, EditorSession};
use crate::{
    loading::{
        naming::{series_names, unique_mirror_name},
        parse_angle_only, parse_index_list, parse_step_series_form, unique_duplicate_name,
    },
    manipulate::{dependents::clear_dependant_edits, tiers_meeting},
};
use indicatrix_cut_core::{ConstraintTier, Edit, EditError};
use std::{collections::BTreeSet, fmt};

#[cfg(test)]
mod tests;

/// The label the tier table and its toasts use for an unnamed tier.
const UNNAMED: &str = "(unnamed)";

fn display_name(name: &str) -> String {
    if name.is_empty() {
        UNNAMED.to_string()
    } else {
        name.to_string()
    }
}

/// What [`EditorSession::duplicate_tier`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateOutcome {
    /// See [`EditChange`].
    pub change: EditChange,
    /// Where the copy sits (right after its source).
    pub new_index: usize,
    /// The source tier's name (`"(unnamed)"` when it has none).
    pub source_label: String,
    /// The copy's generated name.
    pub duplicate_label: String,
}

/// What [`EditorSession::remove_tier`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedTier {
    /// See [`EditChange`].
    pub change: EditChange,
    /// The removed tier's name (`"(unnamed)"` when it had none).
    pub name: String,
    /// How many index-wheel positions (facets) it had.
    pub facet_count: usize,
}

/// What [`EditorSession::move_tier`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedTier {
    /// See [`EditChange`].
    pub change: EditChange,
    /// Where the tier is now.
    pub target: usize,
}

/// Why [`EditorSession::remove_tier`] (or another removal) did not remove anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveTierError {
    /// Other tiers still name what was to be removed as a meet target, and the caller
    /// did not ask for those references to be cleared. Nothing changed.
    HasDependants {
        /// What was to be removed: the tier's name, or `"the selected tiers"`.
        subject: String,
        /// Row indices of the tiers that still meet it by name, ascending.
        dependants: Vec<usize>,
        /// Their `"tier 5 (Girdle)"`-style labels ([`super::tier_nudge_label`]), in
        /// the same order.
        labels: Vec<String>,
    },
    /// The removal itself failed (a tier that does not exist).
    Edit(EditError),
}

impl fmt::Display for RemoveTierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HasDependants {
                subject, labels, ..
            } => write!(
                f,
                "Cannot remove {subject}: still named as a meet target by {}. Change those \
                 tiers first, or confirm the removal to clear the references.",
                labels.join(", ")
            ),
            Self::Edit(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for RemoveTierError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::HasDependants { .. } => None,
            Self::Edit(error) => Some(error),
        }
    }
}

impl From<EditError> for RemoveTierError {
    fn from(error: EditError) -> Self {
        Self::Edit(error)
    }
}

/// What [`EditorSession::toggle_detach`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachOutcome {
    /// See [`EditChange`].
    pub change: EditChange,
    /// Whether the tier is detached now (`false`: it was reattached).
    pub detached: bool,
    /// `"tier 5 (Girdle)"`-style label ([`super::tier_nudge_label`]).
    pub label: String,
}

/// What [`EditorSession::mirror_tier_to_other_block`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorOutcome {
    /// See [`EditChange`].
    pub change: EditChange,
    /// Where the mirrored tier was appended.
    pub new_index: usize,
    /// Its name.
    pub label: String,
}

/// What [`EditorSession::generate_step_series`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedSeries {
    /// See [`EditChange`].
    pub change: EditChange,
    /// Index of the first generated tier.
    pub start_index: usize,
    /// How many tiers were appended.
    pub added: usize,
}

/// What [`EditorSession::set_tier_angle_from_text`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineAngle {
    /// The tier does not exist (a stale row): nothing happened.
    Missing,
    /// The text parsed to exactly the tier's current angle (bit for bit): no edit was
    /// spent, and any nudge coalescing run was ended.
    NoChange,
    /// The angle was changed.
    Applied(EditChange),
}

/// The selection after tier `removed` was removed (every later tier shifted down by
/// one): the removed tier's own selection clears, a later one follows its tier down, an
/// earlier one (or none) stays.
#[must_use]
pub const fn selection_after_remove(selected: Option<usize>, removed: usize) -> Option<usize> {
    match selected {
        Some(index) if index == removed => None,
        Some(index) if index > removed => Some(index - 1),
        other => other,
    }
}

impl EditorSession {
    /// The tier list's Duplicate (and Ctrl+D): inserts a copy of tier `index` (named by
    /// [`unique_duplicate_name`], same indices/angle/constraint/detached set, its
    /// `imported_meet` cleared -- a new, user-authored row) immediately AFTER the source
    /// as one `Edit::AddTier`, since cut order is meaningful.
    ///
    /// `Ok(None)` when the tier does not exist.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn duplicate_tier(&mut self, index: usize) -> Result<Option<DuplicateOutcome>, EditError> {
        let Some(source) = self.design.tiers.get(index) else {
            return Ok(None);
        };
        let mut duplicate = source.clone();
        let source_label = display_name(&source.name);
        let existing_names: Vec<String> =
            self.design.tiers.iter().map(|t| t.name.clone()).collect();
        duplicate.name = unique_duplicate_name(&source.name, &existing_names);
        let duplicate_label = duplicate.name.clone();
        duplicate.imported_meet = None;
        let new_index = index + 1;
        let change = self.apply(Edit::AddTier {
            index: new_index,
            tier: duplicate,
        })?;
        Ok(Some(DuplicateOutcome {
            change,
            new_index,
            source_label,
            duplicate_label,
        }))
    }

    /// The tier list's Remove: [`Self::remove_tier_with`] without clearing references,
    /// so a tier another tier still meets by name is refused.
    ///
    /// # Errors
    ///
    /// [`Self::remove_tier_with`]'s error.
    pub fn remove_tier(&mut self, index: usize) -> Result<RemovedTier, RemoveTierError> {
        self.remove_tier_with(index, false)
    }

    /// `Edit::RemoveTier`, with the removed tier's name and facet count for the
    /// confirmation toast.
    ///
    /// A tier that other tiers meet by name (`MeetNamed`) cannot simply disappear: their
    /// lists would keep pointing at a name no tier bears, and the next solve would
    /// fail. With `cascade` false such a removal is refused with
    /// [`RemoveTierError::HasDependants`], naming the dependants, and nothing changes.
    /// With `cascade` true the removed tier's name is first taken out of every
    /// dependant's list (one that loses its last name meets whatever vertex the solver
    /// finds instead), all as ONE undoable `Edit::Batch`.
    ///
    /// # Errors
    ///
    /// [`RemoveTierError::HasDependants`] as above, or [`Self::apply`]'s error (also for
    /// a tier that does not exist).
    pub fn remove_tier_with(
        &mut self,
        index: usize,
        cascade: bool,
    ) -> Result<RemovedTier, RemoveTierError> {
        let (name, facet_count) = self.design.tiers.get(index).map_or_else(
            || (UNNAMED.to_string(), 0),
            |tier| (display_name(&tier.name), tier.indices.len()),
        );
        let dependants = tiers_meeting(&self.design, index);
        if !dependants.is_empty() && !cascade {
            return Err(self.has_dependants(name, dependants));
        }
        let edit = if dependants.is_empty() {
            Edit::RemoveTier { index }
        } else {
            let mut steps = clear_dependant_edits(&self.design, index);
            steps.push(Edit::RemoveTier { index });
            Edit::Batch(steps)
        };
        let change = self.apply(edit)?;
        Ok(RemovedTier {
            change,
            name,
            facet_count,
        })
    }

    /// [`RemoveTierError::HasDependants`] for `subject`, with the dependants' labels.
    fn has_dependants(&self, subject: String, dependants: Vec<usize>) -> RemoveTierError {
        let labels = dependants
            .iter()
            .filter_map(|&index| {
                self.design
                    .tiers
                    .get(index)
                    .map(|tier| super::tier_nudge_label(tier, index))
            })
            .collect();
        RemoveTierError::HasDependants {
            subject,
            dependants,
            labels,
        }
    }

    /// Row reorder: moves tier `index` one place up (`direction < 0`) or down as one
    /// `Edit::MoveTier` -- a single undo step.
    ///
    /// `Ok(None)` at either end of the list.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn move_tier(
        &mut self,
        index: usize,
        direction: i32,
    ) -> Result<Option<MovedTier>, EditError> {
        let tier_count = self.design.tiers.len();
        let target = if direction < 0 {
            index.checked_sub(1)
        } else {
            index.checked_add(1).filter(|&t| t < tier_count)
        };
        let Some(target) = target else {
            return Ok(None);
        };
        let change = self.apply(Edit::MoveTier {
            from: index,
            to: target,
        })?;
        Ok(Some(MovedTier { change, target }))
    }

    /// The row's Detach/Reattach toggle: `Design::detach_all_in_tier` when nothing is
    /// detached yet, `Design::reattach_all_in_tier` otherwise, as one edit.
    ///
    /// `Ok(None)` when the tier does not exist.
    ///
    /// # Errors
    ///
    /// The design method's error, or [`Self::apply`]'s.
    pub fn toggle_detach(&mut self, index: usize) -> Result<Option<DetachOutcome>, EditError> {
        let Some(tier) = self.design.tiers.get(index) else {
            return Ok(None);
        };
        let edit = if tier.detached.is_empty() {
            self.design.detach_all_in_tier(index)
        } else {
            self.design.reattach_all_in_tier(index)
        };
        let change = edit.and_then(|edit| self.apply(edit))?;
        let tier = self.design.tiers.get(index);
        Ok(Some(DetachOutcome {
            change,
            detached: tier.is_some_and(|tier| !tier.detached.is_empty()),
            label: tier.map_or_else(String::new, |tier| super::tier_nudge_label(tier, index)),
        }))
    }

    /// The multi-select "Delete": [`Self::remove_multi_selected_with`] without clearing
    /// references.
    ///
    /// # Errors
    ///
    /// [`Self::remove_multi_selected_with`]'s error.
    pub fn remove_multi_selected(&mut self) -> Result<usize, RemoveTierError> {
        self.remove_multi_selected_with(false)
    }

    /// Removes every tier in [`Self::multi_selected`], highest index first (so one
    /// removal never shifts an index still waiting), as several separately-undoable
    /// removals. A failing removal does not stop the others; the LAST failure is
    /// returned (the design may still have changed). `Ok(0)` when nothing is
    /// multi-selected.
    ///
    /// Tiers OUTSIDE the selection that meet a selected tier by name would be left
    /// pointing at a name that is gone. With `cascade` false the whole removal is refused
    /// up front with [`RemoveTierError::HasDependants`] (nothing changes); with `cascade`
    /// true each removal clears those references first, like [`Self::remove_tier_with`].
    /// Tiers inside the selection that meet each other need no clearing, since they all
    /// go.
    ///
    /// # Errors
    ///
    /// [`RemoveTierError::HasDependants`] as above, or the last [`EditError`] one of the
    /// removals returned.
    pub fn remove_multi_selected_with(&mut self, cascade: bool) -> Result<usize, RemoveTierError> {
        let targets: Vec<usize> = self.multi_selected.iter().rev().copied().collect();
        let removed = targets.len();
        let outside: BTreeSet<usize> = targets
            .iter()
            .flat_map(|&index| tiers_meeting(&self.design, index))
            .filter(|dependant| !self.multi_selected.contains(dependant))
            .collect();
        if !outside.is_empty() && !cascade {
            let dependants = outside.into_iter().collect();
            return Err(self.has_dependants("the selected tiers".to_string(), dependants));
        }
        let mut last_err = None;
        for index in targets {
            let outcome = if cascade {
                self.remove_tier_with(index, true).map(|_| ())
            } else {
                self.apply(Edit::RemoveTier { index })
                    .map(|_| ())
                    .map_err(RemoveTierError::from)
            };
            if let Err(e) = outcome {
                last_err = Some(e);
            }
        }
        last_err.map_or(Ok(removed), Err)
    }

    /// "Adopt all": switches every tier that still carries an unadopted `imported_meet`
    /// over to it, as ONE undoable `Edit::Batch` of `Edit::SetConstraint`s. `Ok(0)`
    /// (nothing applied) when no tier has one.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn adopt_all_imported_meets(&mut self) -> Result<usize, EditError> {
        self.adopt_imported_meets(None)
    }

    /// "Adopt sel.": [`Self::adopt_all_imported_meets`] restricted to
    /// [`Self::multi_selected`].
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn adopt_selected_imported_meets(&mut self) -> Result<usize, EditError> {
        let selected = self.multi_selected.clone();
        self.adopt_imported_meets(Some(&selected))
    }

    /// The row's "Adopt": switches tier `index` over to the meet instruction the source
    /// file actually stated for it (`ConstraintTier::imported_meet`) with one
    /// `Edit::SetConstraint`. `Ok(false)` (nothing applied) when the tier does not exist
    /// or has nothing to adopt.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn adopt_imported_meet(&mut self, index: usize) -> Result<bool, EditError> {
        let Some(constraint) = self
            .design
            .tiers
            .get(index)
            .and_then(|tier| tier.imported_meet.clone())
        else {
            return Ok(false);
        };
        self.apply(Edit::SetConstraint { index, constraint })?;
        Ok(true)
    }

    fn adopt_imported_meets(&mut self, only: Option<&BTreeSet<usize>>) -> Result<usize, EditError> {
        let edits: Vec<Edit> = self
            .design
            .tiers
            .iter()
            .enumerate()
            .filter(|(index, _)| only.is_none_or(|set| set.contains(index)))
            .filter_map(|(index, tier)| {
                tier.imported_meet
                    .clone()
                    .map(|constraint| Edit::SetConstraint { index, constraint })
            })
            .collect();
        if edits.is_empty() {
            return Ok(0);
        }
        let count = edits.len();
        self.apply(Edit::Batch(edits))?;
        Ok(count)
    }

    /// "Mirror tier to other block": duplicates tier `index` to the opposite block via
    /// `ConstraintTier::mirrored_to_other_block` (angle negated, same
    /// indices/constraint/detached set) and appends it as one `Edit::AddTier`.
    /// `Ok(None)` when the tier does not exist.
    ///
    /// The copy's name is [`unique_mirror_name`]'s: each of the source's names suffixed
    /// by `name_suffix`, made whitespace-free (so it survives a plain `.asc` export) and
    /// counted up until no tier already bears it (case-insensitively, per `/`-joined
    /// name), so a `MeetNamed` list can never bind to the wrong tier. An unnamed source
    /// gets the next free block name instead.
    ///
    /// # Errors
    ///
    /// [`Self::apply`]'s error.
    pub fn mirror_tier_to_other_block(
        &mut self,
        index: usize,
        name_suffix: &str,
    ) -> Result<Option<MirrorOutcome>, EditError> {
        let Some(source) = self.design.tiers.get(index) else {
            return Ok(None);
        };
        let mut mirrored = source.mirrored_to_other_block(name_suffix);
        let existing_names: Vec<String> =
            self.design.tiers.iter().map(|t| t.name.clone()).collect();
        mirrored.name = unique_mirror_name(
            &source.name,
            name_suffix,
            mirrored.angle_deg,
            &existing_names,
        );
        let label = mirrored.name.clone();
        let new_index = self.design.tiers.len();
        let change = self.apply(Edit::AddTier {
            index: new_index,
            tier: mirrored,
        })?;
        Ok(Some(MirrorOutcome {
            change,
            new_index,
            label,
        }))
    }

    /// "Generate steps": builds `count` tiers with `ConstraintTier::step_series` from
    /// the form's texts (see [`parse_step_series_form`] for what a blank anchor means
    /// and which counts and angles are refused; `indices_text` is parsed like the tier
    /// form's Indices field) and appends them after the last tier as one `Edit::Batch`
    /// of `Edit::AddTier`s.
    ///
    /// The tiers are named by [`series_names`]: `"<name_prefix><n>"` counting on past
    /// any number a tier already bears (so a second ladder with the same prefix
    /// continues the first instead of repeating its names), or block names when the
    /// prefix is blank.
    ///
    /// # Errors
    ///
    /// A message ready to show the cutter: a parse failure naming the field, or the
    /// apply error's text.
    pub fn generate_step_series(
        &mut self,
        name_prefix: &str,
        start_angle_text: &str,
        angle_step_text: &str,
        count: i32,
        indices_text: &str,
        anchor_text: &str,
    ) -> Result<GeneratedSeries, String> {
        let (start_angle, angle_step, count, first_constraint) =
            parse_step_series_form(start_angle_text, angle_step_text, count, anchor_text)?;
        let indices = parse_index_list(indices_text, self.design.meta.gear_teeth_abs())?;
        let mut tiers = ConstraintTier::step_series(
            name_prefix,
            start_angle,
            angle_step,
            count,
            &indices,
            &first_constraint,
        );
        let existing_names: Vec<String> =
            self.design.tiers.iter().map(|t| t.name.clone()).collect();
        let angles: Vec<f64> = tiers.iter().map(|tier| tier.angle_deg).collect();
        for (tier, name) in
            tiers
                .iter_mut()
                .zip(series_names(name_prefix, &angles, &existing_names))
        {
            tier.name = name;
        }
        let start_index = self.design.tiers.len();
        let edits: Vec<Edit> = tiers
            .into_iter()
            .enumerate()
            .map(|(offset, tier)| Edit::AddTier {
                index: start_index + offset,
                tier,
            })
            .collect();
        let added = edits.len();
        let change = self.apply(Edit::Batch(edits)).map_err(|e| e.to_string())?;
        Ok(GeneratedSeries {
            change,
            start_index,
            added,
        })
    }

    /// The inline angle cell's commit: parses `text` like the tier form's Angle field
    /// ([`parse_angle_only`]) and applies it as one `Edit::ModifyTier` (angle only,
    /// everything else on the tier untouched). Text that parses to the tier's current
    /// angle bit for bit spends no undo step; it ends any nudge coalescing run instead
    /// (opening the cell, looking and closing it is a real interaction boundary).
    ///
    /// # Errors
    ///
    /// A message ready to show the cutter: the parse failure, or the apply error's text.
    pub fn set_tier_angle_from_text(
        &mut self,
        index: usize,
        text: &str,
    ) -> Result<InlineAngle, String> {
        let Some(current) = self.design.tiers.get(index) else {
            return Ok(InlineAngle::Missing);
        };
        let angle_deg = parse_angle_only(text)?;
        // Bit-exact, not `==` (clippy::float_cmp): committing an unchanged value must
        // not spend an undo slot, and comparing bit patterns sidesteps an arbitrary
        // epsilon.
        if angle_deg.to_bits() == current.angle_deg.to_bits() {
            self.history.end_coalesce_run();
            return Ok(InlineAngle::NoChange);
        }
        let mut tier = current.clone();
        tier.angle_deg = angle_deg;
        self.apply(Edit::ModifyTier { index, tier })
            .map(InlineAngle::Applied)
            .map_err(|e| e.to_string())
    }
}

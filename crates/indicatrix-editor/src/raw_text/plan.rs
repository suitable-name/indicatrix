//! Applying an edited text to the design: the merge that keeps what the text left alone, the
//! summary of what changes and what is lost, and the plan the session applies in one step.

use super::{
    TextProblem, capitalise, counted,
    pairing::{check_edited, pair_tiers},
    parse_text, same, same_all, sentence, tier_label,
};
use crate::session::{EditChange, EditorSession, SessionEditError};
use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, ScheduleMeta, ScheduleState, TierId, built_in_refractive_index,
};
use indicatrix_formats::asc::{AscSchedule, AscTier, MeetInstruction};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

/// The most parse notes the apply summary lists one by one.
const MAX_LISTED_WARNINGS: usize = 5;

/// What changed on one paired tier.
#[derive(Debug, Default, Clone, Copy)]
struct TierDiff {
    /// The angle changed and was taken.
    angle: bool,
    /// The angle changed in the text, but a relation drives it, so it was not taken.
    angle_ignored: bool,
    /// The name changed.
    name: bool,
    /// The index list changed.
    indices: bool,
    /// The mast written in the text changed.
    depth: bool,
    /// The instruction text (the `G` part) changed.
    notes: bool,
    /// The tier followed a meet rule that the text's depth or instruction replaces.
    meet_replaced: bool,
    /// How many detached index marks were dropped because their indices are gone.
    detached_dropped: usize,
}

/// One tier of the new schedule.
struct MergedTier {
    /// The tier.
    tier: ConstraintTier,
    /// Its id: the old tier's, or a fresh one for a tier the text adds.
    id: TierId,
    /// The position in the design's current list; `None` for a tier the text adds.
    old: Option<usize>,
    /// Whether the text changed the tier's depth (so a depth target no longer applies).
    depth_changed: bool,
}

/// What a tier line's instruction text says about the meet rule it could be adopted as,
/// the way `.asc` import records it: `None` for a stated scale reference (nothing to
/// adopt), a meet constraint for anything else.
fn imported_meet_of(tier: &AscTier) -> Option<MeetConstraint> {
    match tier.meet_instruction() {
        Some(MeetInstruction::Meet(names)) => Some(MeetConstraint::MeetNamed(names)),
        Some(MeetInstruction::ScaleReference | MeetInstruction::LevelGirdle) => None,
        _ => Some(MeetConstraint::MeetExisting),
    }
}

/// A tier the text adds, built the way `.asc` import builds one: its depth is pinned to
/// the mast in the text.
fn new_tier(edited: &AscTier) -> ConstraintTier {
    ConstraintTier {
        angle_deg: edited.angle_deg,
        name: edited.name.clone(),
        indices: edited.indices.clone(),
        constraint: MeetConstraint::ScaleReference(edited.mast),
        imported_meet: imported_meet_of(edited),
        original_notes: Some(edited.notes.clone()),
        detached: Vec::new(),
    }
}

/// `old` with the fields the text changed (compared with `base`, the text the design
/// produced) taken from `edited`. A field the text left alone keeps the design's own value.
///
/// A changed depth or instruction text pins the tier to the depth in the text and keeps
/// the instruction text, exactly as importing the line from a file would.
fn merge_one(
    old: &ConstraintTier,
    base: &AscTier,
    edited: &AscTier,
    driven: bool,
) -> (ConstraintTier, TierDiff) {
    let mut tier = old.clone();
    let mut diff = TierDiff::default();
    if !same(base.angle_deg, edited.angle_deg) {
        if driven {
            diff.angle_ignored = true;
        } else {
            tier.angle_deg = edited.angle_deg;
            diff.angle = true;
        }
    }
    if base.name != edited.name {
        tier.name.clone_from(&edited.name);
        diff.name = true;
    }
    if !same_all(&base.indices, &edited.indices) {
        tier.indices.clone_from(&edited.indices);
        let before = tier.detached.len();
        tier.detached
            .retain(|mark| edited.indices.iter().any(|index| same(*index, *mark)));
        diff.detached_dropped = before - tier.detached.len();
        diff.indices = true;
    }
    diff.depth = !same(base.mast, edited.mast);
    diff.notes = base.notes != edited.notes;
    if diff.depth || diff.notes {
        diff.meet_replaced = !matches!(old.constraint, MeetConstraint::ScaleReference(_));
        tier.constraint = MeetConstraint::ScaleReference(edited.mast);
        tier.imported_meet = imported_meet_of(edited);
        tier.original_notes = Some(edited.notes.clone());
    }
    (tier, diff)
}

/// What [`plan_apply`] found, before it is worded.
#[derive(Default)]
struct Findings {
    /// Header, gear, symmetry and footnote lines that change.
    meta_changed: Vec<String>,
    /// What those changes cost or leave alone.
    meta_lost: Vec<String>,
    /// Tiers the text adds.
    added: Vec<String>,
    /// Tiers the text removes.
    removed: Vec<String>,
    /// Renamed tiers, as "old to new".
    renamed: Vec<String>,
    /// Tiers whose angle changes.
    angles: Vec<String>,
    /// Tiers whose index list changes.
    indices: Vec<String>,
    /// Tiers whose cut depth changes.
    depths: Vec<String>,
    /// Tiers whose instruction text changes.
    notes: Vec<String>,
    /// Whether the tier order changes.
    reordered: bool,
    /// Tiers that followed a meet rule and now keep a fixed depth.
    meet_replaced: Vec<String>,
    /// Tiers whose angle the text changed although a relation drives it.
    ignored_angles: Vec<String>,
    /// Tiers whose depth target is dropped because the text sets the depth.
    dropped_targets: Vec<String>,
    /// Tiers whose relation is dropped because a tier it reads is removed.
    dropped_relations: Vec<String>,
    /// Tiers that lose detached index marks.
    detached_dropped: Vec<String>,
    /// Meet rules that now name a tier that is gone.
    broken_meets: Vec<String>,
    /// Notes of removed tiers.
    removed_notes: usize,
    /// Cheater offsets of removed tiers.
    removed_offsets: usize,
    /// Targets of removed tiers.
    removed_targets: usize,
    /// Relations of removed tiers.
    removed_relations: usize,
    /// The parser's notes about lines it ignored.
    parse_notes: Vec<String>,
}

impl Findings {
    /// Records what changed on one paired tier.
    fn record(
        &mut self,
        old: &ConstraintTier,
        new: &ConstraintTier,
        position: usize,
        diff: TierDiff,
    ) {
        let label = tier_label(new, position);
        if diff.name {
            self.renamed
                .push(format!("{} to {label}", tier_label(old, position)));
        }
        if diff.angle {
            self.angles.push(label.clone());
        }
        if diff.angle_ignored {
            self.ignored_angles.push(label.clone());
        }
        if diff.indices {
            self.indices.push(label.clone());
        }
        if diff.depth {
            self.depths.push(label.clone());
        }
        if diff.notes {
            self.notes.push(label.clone());
        }
        if diff.meet_replaced {
            self.meet_replaced.push(label.clone());
        }
        if diff.detached_dropped > 0 {
            self.detached_dropped.push(label);
        }
    }

    /// Records the design tiers no edited tier continues, and what is attached to them.
    fn note_removed(&mut self, design: &Design, pairs: &[Option<usize>]) {
        let kept: BTreeSet<usize> = pairs.iter().flatten().copied().collect();
        for (index, tier) in design.tiers.iter().enumerate() {
            if kept.contains(&index) {
                continue;
            }
            self.removed.push(tier_label(tier, index));
            let id = design.tier_ids[index];
            self.removed_notes += usize::from(design.tier_notes.contains_key(&index));
            self.removed_offsets += usize::from(design.cheater_offsets_deg.contains_key(&index));
            self.removed_targets += usize::from(design.tier_targets.contains_key(&id));
            self.removed_relations += usize::from(design.tier_relations.contains_key(&id));
        }
    }

    /// The sentence about what goes with the removed tiers, when anything does.
    fn removed_with(&self) -> Option<String> {
        let parts: Vec<String> = [
            (self.removed_notes, "note"),
            (self.removed_offsets, "cheater offset"),
            (self.removed_targets, "target"),
            (self.removed_relations, "relation"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, noun)| counted(count, noun))
        .collect();
        (!parts.is_empty()).then(|| format!("Removed along with them: {}.", parts.join(", ")))
    }

    /// The parser's notes, the first few in full and a count for the rest.
    fn parse_note_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .parse_notes
            .iter()
            .take(MAX_LISTED_WARNINGS)
            .map(|note| capitalise(note))
            .collect();
        let rest = self.parse_notes.len().saturating_sub(MAX_LISTED_WARNINGS);
        if rest > 0 {
            lines.push(format!("{rest} more lines were ignored."));
        }
        lines
    }

    /// Words the findings for the person reading the summary.
    fn into_report(mut self) -> ApplyReport {
        let mut changed = std::mem::take(&mut self.meta_changed);
        changed.extend(sentence("Added tiers:", &self.added));
        changed.extend(sentence("Renamed:", &self.renamed));
        changed.extend(sentence("Angle changes on:", &self.angles));
        changed.extend(sentence("Index list changes on:", &self.indices));
        changed.extend(sentence("Cut depth changes on:", &self.depths));
        changed.extend(sentence("Instruction text changes on:", &self.notes));
        if self.reordered {
            changed.push("The order of the tiers changes.".to_owned());
        }

        let mut lost = Vec::new();
        lost.extend(sentence("Removed tiers:", &self.removed));
        lost.extend(self.removed_with());
        lost.extend(sentence(
            "These tiers followed a meet rule and now keep the fixed depth written in the text:",
            &self.meet_replaced,
        ));
        lost.extend(sentence(
            "These tiers have a depth target that the depth in the text replaces:",
            &self.dropped_targets,
        ));
        lost.extend(sentence(
            "These tiers follow a relation, so the angle in the text is ignored:",
            &self.ignored_angles,
        ));
        lost.extend(sentence(
            "These tiers lose their relation because a tier it reads is removed:",
            &self.dropped_relations,
        ));
        lost.extend(sentence(
            "These tiers lose their detached index marks because the indices changed:",
            &self.detached_dropped,
        ));
        lost.extend(sentence(
            "These meet rules now name a tier that is gone:",
            &self.broken_meets,
        ));
        lost.extend(std::mem::take(&mut self.meta_lost));
        lost.extend(self.parse_note_lines());
        ApplyReport { changed, lost }
    }
}

/// Whether the material, not the legacy `I` line, decides the design's refractive index.
fn material_decides_index(design: &Design, custom: &[GemMaterial]) -> bool {
    design.material.refractive_index_override.is_some()
        || design.material.name.as_deref().is_some_and(|name| {
            custom.iter().any(|gem| gem.name.eq_ignore_ascii_case(name))
                || built_in_refractive_index(name).is_some()
        })
}

/// The footnotes the user wrote: those of `schedule` that are not a line Export writes for
/// a concave tier.
fn user_footnotes(schedule: &AscSchedule, generated: &[String]) -> Vec<String> {
    schedule
        .footnotes
        .iter()
        .filter(|footnote| !generated.contains(footnote))
        .cloned()
        .collect()
}

/// `design`'s metadata with the header lines the text changed. A field the text left alone
/// keeps the design's own value.
fn merge_meta(
    design: &Design,
    base: &AscSchedule,
    edited: &AscSchedule,
    custom: &[GemMaterial],
    findings: &mut Findings,
) -> ScheduleMeta {
    let mut meta = design.meta.clone();
    if edited.gemcad_version != base.gemcad_version {
        meta.gemcad_version.clone_from(&edited.gemcad_version);
        findings
            .meta_changed
            .push("The GemCad version line changes.".to_owned());
    }
    if edited.gear_teeth != base.gear_teeth {
        meta.gear_teeth = edited.gear_teeth;
        findings.meta_changed.push(format!(
            "The gear changes from {} to {} teeth.",
            base.gear_teeth, edited.gear_teeth
        ));
        findings.meta_lost.push(
            "The index numbers are used as written. They are not rescaled to the new gear."
                .to_owned(),
        );
    }
    if !same(edited.gear_reference_angle, base.gear_reference_angle) {
        meta.gear_reference_angle = edited.gear_reference_angle;
        findings.meta_changed.push(format!(
            "The gear reference angle changes from {} to {}.",
            base.gear_reference_angle, edited.gear_reference_angle
        ));
    }
    if edited.symmetry_order != base.symmetry_order {
        meta.symmetry_order = edited.symmetry_order;
        findings.meta_changed.push(format!(
            "The symmetry order changes from {} to {}.",
            base.symmetry_order, edited.symmetry_order
        ));
    }
    if edited.mirror != base.mirror {
        meta.mirror = edited.mirror;
        let now = if edited.mirror { "on" } else { "off" };
        findings
            .meta_changed
            .push(format!("Mirror symmetry is now {now}."));
    }
    if !same(edited.refractive_index, base.refractive_index) {
        meta.refractive_index = edited.refractive_index;
        findings.meta_changed.push(format!(
            "The refractive index changes from {} to {}.",
            base.refractive_index, edited.refractive_index
        ));
        if material_decides_index(design, custom) {
            findings.meta_lost.push(
                "The material sets the refractive index, so the I line only changes the stored \
                 fallback value."
                    .to_owned(),
            );
        }
    }
    if edited.headers != base.headers {
        meta.headers.clone_from(&edited.headers);
        findings
            .meta_changed
            .push("The header lines change.".to_owned());
    }
    // The lines Export writes for concave tiers are not the user's footnotes: they are
    // left out of the comparison and never stored in the design.
    let mut generated = AscSchedule::default();
    design.append_concave_footnotes(&mut generated);
    let before = user_footnotes(base, &generated.footnotes);
    let after = user_footnotes(edited, &generated.footnotes);
    if before != after {
        meta.footnotes = after;
        findings
            .meta_changed
            .push("The footnote lines change.".to_owned());
    }
    meta
}

/// The new tier list: the design's own tier for every line the text continues (with the
/// fields the text changed), and a new tier for every line it adds. Records what changed
/// in `findings`.
fn merge_tiers(
    design: &Design,
    base: &AscSchedule,
    edited: &AscSchedule,
    pairs: &[Option<usize>],
    findings: &mut Findings,
) -> Vec<MergedTier> {
    let mut next_id = design.next_tier_id;
    let mut merged: Vec<MergedTier> = Vec::with_capacity(edited.tiers.len());
    for (edited_tier, pair) in edited.tiers.iter().zip(pairs) {
        let position = merged.len();
        if let Some(old_index) = *pair {
            let old = &design.tiers[old_index];
            let driven = design.is_tier_driven(old_index);
            let (tier, diff) = merge_one(old, &base.tiers[old_index], edited_tier, driven);
            findings.record(old, &tier, position, diff);
            merged.push(MergedTier {
                tier,
                id: design.tier_ids[old_index],
                old: Some(old_index),
                depth_changed: diff.depth,
            });
        } else {
            let tier = new_tier(edited_tier);
            findings.added.push(tier_label(&tier, position));
            merged.push(MergedTier {
                tier,
                id: TierId(next_id),
                old: None,
                depth_changed: false,
            });
            next_id += 1;
        }
    }
    findings.note_removed(design, pairs);
    findings.reordered = !pairs.iter().flatten().is_sorted();
    merged
}

/// Every lower-case name (a folded `a/b` name counts as two) the tiers answer to.
fn name_set<'a>(tiers: impl Iterator<Item = &'a ConstraintTier>) -> BTreeSet<String> {
    tiers
        .flat_map(ConstraintTier::names)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The meet rules in the new list that name a tier the design had and the new list does
/// not, as "tier (needs name)".
fn broken_meets(design: &Design, merged: &[MergedTier]) -> Vec<String> {
    let before = name_set(design.tiers.iter());
    let after = name_set(merged.iter().map(|entry| &entry.tier));
    let mut broken = Vec::new();
    for (position, entry) in merged.iter().enumerate() {
        let MeetConstraint::MeetNamed(wanted) = &entry.tier.constraint else {
            continue;
        };
        for name in wanted {
            let key = name.to_ascii_lowercase();
            if before.contains(&key) && !after.contains(&key) {
                broken.push(format!(
                    "{} (needs {name})",
                    tier_label(&entry.tier, position)
                ));
            }
        }
    }
    broken
}

/// The state `Edit::ReplaceSchedule` takes: `meta` and the merged tiers, with the notes and
/// cheater offsets that follow their tiers to their new positions, and the targets and
/// relations that survive. Records what is dropped in `findings`.
fn assemble(
    design: &Design,
    meta: ScheduleMeta,
    merged: &[MergedTier],
    findings: &mut Findings,
) -> ScheduleState {
    let new_ids: BTreeSet<TierId> = merged.iter().map(|entry| entry.id).collect();
    let added = merged.iter().filter(|entry| entry.old.is_none()).count();
    let mut state = ScheduleState {
        meta,
        tiers: merged.iter().map(|entry| entry.tier.clone()).collect(),
        tier_ids: merged.iter().map(|entry| entry.id).collect(),
        next_tier_id: design.next_tier_id + added as u64,
        cheater_offsets_deg: BTreeMap::new(),
        tier_notes: BTreeMap::new(),
        tier_targets: BTreeMap::new(),
        tier_relations: BTreeMap::new(),
    };
    let label_of = |id: TierId| {
        merged
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.id == id)
            .map_or_else(String::new, |(position, entry)| {
                tier_label(&entry.tier, position)
            })
    };
    for (position, entry) in merged.iter().enumerate() {
        let Some(old) = entry.old else {
            continue;
        };
        if let Some(offset) = design.cheater_offsets_deg.get(&old) {
            state.cheater_offsets_deg.insert(position, *offset);
        }
        if let Some(note) = design.tier_notes.get(&old) {
            state.tier_notes.insert(position, note.clone());
        }
    }
    let depth_set: BTreeSet<TierId> = merged
        .iter()
        .filter(|entry| entry.depth_changed)
        .map(|entry| entry.id)
        .collect();
    for (id, target) in &design.tier_targets {
        if !new_ids.contains(id) {
            continue;
        }
        if depth_set.contains(id) {
            findings.dropped_targets.push(label_of(*id));
        } else {
            state.tier_targets.insert(*id, *target);
        }
    }
    for (id, relation) in &design.tier_relations {
        if !new_ids.contains(id) {
            continue;
        }
        if relation
            .references()
            .iter()
            .all(|read| new_ids.contains(read))
        {
            state.tier_relations.insert(*id, relation.clone());
        } else {
            findings.dropped_relations.push(label_of(*id));
        }
    }
    state
}

// --- the plan ----------------------------------------------------------------------------

/// What applying an edited text changes and what it loses, as sentences for the summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// What changes.
    pub changed: Vec<String>,
    /// What is lost, dropped or ignored.
    pub lost: Vec<String>,
}

/// An edited text ready to apply: the state for `Edit::ReplaceSchedule` and the summary of
/// what it does.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyPlan {
    /// The whole new schedule.
    pub state: ScheduleState,
    /// What changes and what is lost.
    pub report: ApplyReport,
    /// Whether the state is the design's own state (nothing to apply).
    noop: bool,
}

impl ApplyPlan {
    /// Whether the text changes nothing in the design, so there is nothing to apply (and no
    /// undo step to make). The report can still list lines the reader ignored.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        self.noop
    }
}

/// `design` with every tier given an id (a design built by hand may lack some).
fn synced(design: &Design) -> Cow<'_, Design> {
    if design.tier_ids.len() == design.tiers.len() {
        Cow::Borrowed(design)
    } else {
        let mut copy = design.clone();
        copy.ensure_tier_ids();
        Cow::Owned(copy)
    }
}

/// The problem shown when the text the plan starts from no longer belongs to the design.
fn stale_problem() -> TextProblem {
    TextProblem::general(
        "This text no longer matches the open design. Use Revert to write it again.",
    )
}

/// Compares `edited_text` with `base_text` (the text [`generate_text`](super::generate_text) wrote for `design`)
/// and builds the plan that turns the design into what the edited text says.
///
/// A line that was not touched keeps everything the design holds behind it. A tier the
/// text changes keeps its id, note, cheater offset, target and relation unless the change
/// contradicts them (see [`ApplyReport::lost`]); a tier the text removes takes them with
/// it. `custom` is the custom material catalogue the text was generated with.
///
/// # Errors
///
/// A [`TextProblem`] when the edited text cannot be read, has an angle outside -90 to 90
/// degrees, an index off the gear or a name a concave tier already has, or when
/// `base_text` no longer belongs to `design`.
pub fn plan_apply(
    design: &Design,
    base_text: &str,
    edited_text: &str,
    custom: &[GemMaterial],
) -> Result<ApplyPlan, TextProblem> {
    let design = synced(design);
    let design: &Design = &design;
    let base = parse_text(base_text).map_err(|_| stale_problem())?;
    if base.schedule.tiers.len() != design.tiers.len() {
        return Err(stale_problem());
    }
    let edited = parse_text(edited_text)?;
    let pairs = pair_tiers(&base.schedule.tiers, &edited.schedule.tiers);
    check_edited(design, &base, &edited, &pairs)?;

    let mut findings = Findings::default();
    let meta = merge_meta(
        design,
        &base.schedule,
        &edited.schedule,
        custom,
        &mut findings,
    );
    let merged = merge_tiers(
        design,
        &base.schedule,
        &edited.schedule,
        &pairs,
        &mut findings,
    );
    let state = assemble(design, meta, &merged, &mut findings);
    findings.broken_meets = broken_meets(design, &merged);
    findings.parse_notes.clone_from(&edited.schedule.warnings);
    let noop = state == ScheduleState::of(design);
    Ok(ApplyPlan {
        state,
        report: findings.into_report(),
        noop,
    })
}

/// Applies `plan` to the session as one undoable edit; `Ok(None)` when the plan changes
/// nothing.
///
/// # Errors
///
/// The session's refusal, for example when a tier relation cannot hold after the change.
pub fn apply_plan(
    session: &mut EditorSession,
    plan: ApplyPlan,
) -> Result<Option<EditChange>, SessionEditError> {
    if plan.noop {
        return Ok(None);
    }
    session
        .try_apply(Edit::ReplaceSchedule(Box::new(plan.state)))
        .map(Some)
}

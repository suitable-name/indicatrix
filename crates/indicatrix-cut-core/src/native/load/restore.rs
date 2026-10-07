//! The helpers that restore per-tier state from a native file's tier tables onto a
//! freshly built [`Design`]: notes, cheater offsets, ids, targets, relations and concave
//! tiers. Shared by [`load_paired`], [`load_native_only`] and the self-contained design
//! file loader. Split from the parent module so the file stays readable; the functions
//! are unchanged.

use super::{LoadPairedError, MaterialResolution};
use crate::{
    design::{ConstraintTier, Design, RelationError, TierId, TierRelation},
    native::convert::{
        concave_tier_from_table, meet_constraint_from_native, tier_target_from_native,
    },
};
use indicatrix_formats::native::{ConcaveTierTable, NativeTierTarget, TierTable};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(doc)]
use super::{TierOverlay, load_native_only, load_paired};

/// [`MaterialResolution`]'s own computation -- see that type's doc comment for the
/// heuristic and its limits.
///
/// Exact, case-insensitive match only (via
/// [`crate::material::built_in_material_by_exact_name`]) -- NOT
/// [`indicatrix::optics::materials::GemMaterial::by_name`]'s own substring
/// fallback, which would report a name like "My Blue Sapphire" as `Known` (and
/// silently write Sapphire's own RI to a re-exported `.asc`'s `I` line) just
/// because it CONTAINS a built-in name.
pub(in crate::native) fn material_resolution_of(name: Option<&str>) -> MaterialResolution {
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
pub(in crate::native) fn tier_notes_from_native(
    native_tiers: &[TierTable],
) -> BTreeMap<usize, String> {
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
pub(in crate::native) fn cheater_offsets_from_native(
    native_tiers: &[TierTable],
) -> BTreeMap<usize, f64> {
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
pub(in crate::native) fn raw_tier_ids_from_native(native_tiers: &[TierTable]) -> Vec<Option<u64>> {
    native_tiers.iter().map(|t| t.tier_id).collect()
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::target`].
pub(in crate::native) fn raw_tier_targets_from_native(
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
pub(in crate::native) fn apply_tier_ids_and_targets(
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

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::angle_relation`].
pub(in crate::native) fn raw_tier_relations_from_native(
    native_tiers: &[TierTable],
) -> Vec<Option<String>> {
    native_tiers
        .iter()
        .map(|t| t.angle_relation.clone())
        .collect()
}

/// Restores `design.tier_relations` from `raw` (captured via
/// [`raw_tier_relations_from_native`] before the native file's own `tiers` array was
/// consumed), then lets every driven tier's angle follow its relation, so a file whose
/// stored angle drifted from its relation (a hand edit) opens consistent.
///
/// Must run after [`apply_tier_ids_and_targets`]: a relation names tiers by id.
///
/// # Errors
///
/// The position of the tier whose relation is unusable and why: text that is not
/// canonical, a tier that is gone, a loop, or a result that is not an angle. The position
/// is that of the tier the error is about, not of the first tier that has a relation. The
/// design is left without relations in that case.
pub(in crate::native) fn apply_tier_relations(
    design: &mut Design,
    raw: &[Option<String>],
) -> Result<(), (usize, String)> {
    design.tier_relations.clear();
    let mut first_driven = None;
    for (index, text) in raw.iter().enumerate() {
        let (Some(text), Some(&id)) = (text, design.tier_ids.get(index)) else {
            continue;
        };
        match TierRelation::parse_canonical(text) {
            Ok(relation) => {
                design.tier_relations.insert(id, relation);
                first_driven.get_or_insert(index);
            }
            Err(error) => {
                design.tier_relations.clear();
                return Err((index, error.to_string()));
            }
        }
    }
    let Some(first) = first_driven else {
        return Ok(());
    };
    match design.evaluate_relations() {
        Ok(updates) => {
            for (position, angle) in updates {
                design.tiers[position].angle_deg = angle;
            }
            Ok(())
        }
        Err(error) => {
            // Find the tier before the relations are cleared: it is found by its label
            // among the driven tiers.
            let position = failing_tier_position(design, &error, first);
            design.tier_relations.clear();
            Err((position, error.to_string()))
        }
    }
}

/// The position of the driven tier an evaluation `error` is about, or `fallback` when the
/// error names no tier that can be found.
///
/// The error names tiers by label (it is written for the cutter), so the first driven tier
/// with that label is taken; for a loop, the first tier named in it.
fn failing_tier_position(design: &Design, error: &RelationError, fallback: usize) -> usize {
    let label = match error {
        RelationError::MissingTier { tier }
        | RelationError::OutOfRange { tier, .. }
        | RelationError::HorizontalTier { tier }
        | RelationError::GirdleTier { tier }
        | RelationError::DivisionByZero { tier }
        | RelationError::NotFinite { tier } => tier.as_str(),
        RelationError::Cycle(names) => match names.first() {
            Some(name) => name.as_str(),
            None => return fallback,
        },
        RelationError::NoSuchTier { index } => return *index,
        RelationError::Parse(_) => return fallback,
    };
    (0..design.tiers.len())
        .find(|&position| {
            design
                .tier_ids
                .get(position)
                .is_some_and(|id| design.tier_relations.contains_key(id))
                && design.relation_label(position) == label
        })
        .unwrap_or(fallback)
}

/// Installs concave tiers read from a file onto `design`: converts each record,
/// validates the lot against the design's gear and flat tier names, and restores each
/// tier's stable id (the cutting-mode marks of a concave step hang on it).
///
/// The ids follow the rule [`apply_tier_ids_and_targets`] applies to flat tiers: a
/// recorded id is kept, [`Design::next_tier_id`] is first bumped past the highest one
/// (both lists draw from one counter, so a fresh id can never repeat a restored one),
/// and a tier whose record carries none -- a file saved before the key existed -- gets a
/// fresh id. An id that repeats within the file, or one a flat tier already holds, is
/// kept only by its first holder; every other claimant gets a fresh id too.
///
/// The tiers are only installed when all of them pass, so a design is never left
/// half-loaded. An empty `tables` is a no-op.
///
/// # Errors
///
/// The position of the first unusable tier and why.
pub(in crate::native) fn restore_concave_tiers(
    design: &mut Design,
    tables: Vec<ConcaveTierTable>,
) -> Result<(), (usize, String)> {
    if tables.is_empty() {
        return Ok(());
    }
    let raw_ids: Vec<Option<u64>> = tables.iter().map(|table| table.concave_tier_id).collect();
    design.concave_tiers = tables
        .into_iter()
        .enumerate()
        .map(|(index, table)| concave_tier_from_table(table).map_err(|reason| (index, reason)))
        .collect::<Result<_, _>>()?;
    if let Err((index, error)) = design.validate_concave_tiers() {
        design.concave_tiers.clear();
        return Err((index, error.to_string()));
    }
    if let Some(max_restored) = raw_ids.iter().filter_map(|id| *id).max() {
        design.next_tier_id = design.next_tier_id.max(max_restored.saturating_add(1));
    }
    let mut taken: BTreeSet<u64> = design.tier_ids.iter().map(|id| id.value()).collect();
    let ids: Vec<TierId> = raw_ids
        .into_iter()
        .map(|maybe_id| {
            maybe_id
                .filter(|id| taken.insert(*id))
                .map_or_else(|| design.allocate_tier_id(), TierId)
        })
        .collect();
    design.concave_tier_ids = ids;
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
pub(super) fn draft_tiers_from_native(
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
pub(in crate::native) fn tier_from_table_with_full_geometry(
    saved: TierTable,
) -> Option<ConstraintTier> {
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

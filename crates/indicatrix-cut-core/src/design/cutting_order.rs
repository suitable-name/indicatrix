//! The one order a design's tiers are cut in.
//!
//! [`Design::cutting_order`] is the order the printed cutting sheet, Cutting mode, the Cut
//! slider, the finished-stone-at-step export, "Build this design" and the tier codes
//! (`P1`, `G1`, `C1`, `T`, see [`super::labelling`]) all follow, so they cannot disagree.
//!
//! The order is the fantasy-cut template's: the pavilion section first (the girdle tiers sit
//! inside it, where they are cut), then the crown section, then the table. Concave tiers
//! come last in their section. Inside a section the stored order is kept, with one
//! correction: a tier that meets a facet named later in the same section is moved to
//! just after the last facet it meets, because a facet cannot meet a facet that is not
//! cut yet ([`flat_cutting_order`]).

use std::collections::BTreeSet;

use indicatrix::geometry::{
    meet_solver::{MeetConstraint, MeetNameResolver, MeetTierInput},
    plane::tier_is_crown_side,
};

use super::{ConstraintTier, Design, TierRef};

/// The section of the flat pavilion tiers and the girdle tiers.
const PAVILION_SECTION: usize = 0;
/// The section of the flat crown tiers, the table excepted.
const CROWN_SECTION: usize = 1;
/// The section of the table, which is always cut last.
const TABLE_SECTION: usize = 2;
/// How many sections the flat tiers fall into.
const SECTION_COUNT: usize = 3;

/// Where a flat tier is cut.
const fn section_of(tier: &ConstraintTier) -> usize {
    if tier.is_table() {
        TABLE_SECTION
    } else if !tier_is_crown_side(tier.angle_deg) || tier.angle_deg.abs() == 90.0 {
        // A flat tier with `|angle| == 90` is a girdle and is cut with the pavilion
        // even though `+90` reads as crown-side.
        PAVILION_SECTION
    } else {
        CROWN_SECTION
    }
}

/// One [`MeetTierInput`] per tier of `tiers`: the form the solver's [`MeetNameResolver`]
/// reads, built straight from the authored tiers (no solve needed).
///
/// The sheet, the tier codes and the cutting order all resolve a `MeetNamed` constraint
/// through the resolver built on this, so a name means the same tier everywhere.
#[must_use]
pub fn meet_inputs(tiers: &[ConstraintTier]) -> Vec<MeetTierInput> {
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

/// Walks `deps` once: the positions it could place in order, and which positions it could
/// not (those on a dependency cycle, and those waiting on one). `deps[a]` lists the
/// positions `a` has to come after.
///
/// Always releases the lowest waiting position first, so a position nothing holds back
/// keeps its place and a held-back one lands directly after the last position it waited
/// for. Deterministic: ordered sets only, no hashing.
fn release_order(deps: &[Vec<usize>]) -> (Vec<usize>, Vec<bool>) {
    let count = deps.len();
    let mut waiting = vec![0_usize; count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (position, targets) in deps.iter().enumerate() {
        for &target in targets {
            dependents[target].push(position);
            waiting[position] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..count)
        .filter(|&position| waiting[position] == 0)
        .collect();
    let mut order = Vec::with_capacity(count);
    while let Some(next) = ready.pop_first() {
        order.push(next);
        for &dependent in &dependents[next] {
            waiting[dependent] -= 1;
            if waiting[dependent] == 0 {
                ready.insert(dependent);
            }
        }
    }
    let stuck = waiting.iter().map(|&left| left > 0).collect();
    (order, stuck)
}

/// The order of the positions `0..deps.len()` with every position after the ones it depends
/// on and the stored order kept wherever nothing forces a change.
///
/// Positions on a dependency cycle (and those waiting on one) have no valid order, so they
/// lose their dependencies and keep their stored place.
fn dependency_order(deps: &[Vec<usize>]) -> Vec<usize> {
    let (order, stuck) = release_order(deps);
    if !stuck.contains(&true) {
        return order;
    }
    let freed: Vec<Vec<usize>> = deps
        .iter()
        .zip(&stuck)
        .map(|(targets, &is_stuck)| {
            if is_stuck {
                Vec::new()
            } else {
                targets.clone()
            }
        })
        .collect();
    release_order(&freed).0
}

/// The flat tiers of `tiers` (as stored indices) in each of the three sections -- pavilion
/// and girdle, crown, table -- in the order they are cut.
#[must_use]
pub fn flat_cutting_sections(tiers: &[ConstraintTier]) -> [Vec<usize>; SECTION_COUNT] {
    let mut sections: [Vec<usize>; SECTION_COUNT] = [Vec::new(), Vec::new(), Vec::new()];
    for (index, tier) in tiers.iter().enumerate() {
        sections[section_of(tier)].push(index);
    }
    let has_meets = tiers
        .iter()
        .any(|tier| matches!(tier.constraint, MeetConstraint::MeetNamed(_)));
    if !has_meets {
        return sections;
    }

    let inputs = meet_inputs(tiers);
    let resolver = MeetNameResolver::new(&inputs);
    let mut section_index = vec![0_usize; tiers.len()];
    let mut place = vec![0_usize; tiers.len()];
    for (section, members) in sections.iter().enumerate() {
        for (position, &member) in members.iter().enumerate() {
            section_index[member] = section;
            place[member] = position;
        }
    }
    for (section, members) in sections.iter_mut().enumerate() {
        let deps: Vec<Vec<usize>> = members
            .iter()
            .map(|&member| {
                let MeetConstraint::MeetNamed(names) = &tiers[member].constraint else {
                    return Vec::new();
                };
                // Only a facet of the same section constrains the order: a meet that
                // crosses sections is the section order's business.
                resolver
                    .resolve_names(names)
                    .refs
                    .into_iter()
                    .filter(|&target| target != member && section_index[target] == section)
                    .map(|target| place[target])
                    .collect()
            })
            .collect();
        if deps.iter().any(|targets| !targets.is_empty()) {
            let reordered: Vec<usize> = dependency_order(&deps)
                .into_iter()
                .map(|position| members[position])
                .collect();
            *members = reordered;
        }
    }
    sections
}

/// The flat tiers of `tiers` (stored indices) in the order they are cut.
///
/// The pavilion and girdle tiers first, then the crown tiers, the table last; inside each
/// section the stored order, except that a tier whose `MeetNamed` targets sit later in
/// the same section moves to just after the last of them. A cycle between meets, or a
/// target in another section, leaves the stored order alone for the tiers it touches.
///
/// Concave tiers are not part of a plain tier list; [`Design::cutting_order`] adds them
/// at the end of their section. Because they all come after every flat tier of their
/// section, the flat order here does not depend on them.
#[must_use]
pub fn flat_cutting_order(tiers: &[ConstraintTier]) -> Vec<usize> {
    flat_cutting_sections(tiers).into_iter().flatten().collect()
}

impl Design {
    /// The order tiers are cut in: the flat pavilion and girdle tiers, the concave
    /// pavilion-side tiers, the flat crown tiers except the table, the concave crown-side
    /// tiers, then the table.
    ///
    /// The one order every consumer uses (the module doc lists them). Within a section
    /// the stored order is kept, with the one correction [`flat_cutting_order`] makes for
    /// a facet that meets a facet cut later; the author controls the order inside the
    /// concave groups. A flat tier with `|angle| == 90` is a girdle and joins the first
    /// section even though `+90` reads as crown-side.
    #[must_use]
    pub fn cutting_order(&self) -> Vec<TierRef> {
        let [pavilion, crown, table] = flat_cutting_sections(&self.tiers);
        let concave = |crown_side: bool| {
            self.concave_tiers
                .iter()
                .enumerate()
                .filter(move |(_, tier)| tier.is_crown_side() == crown_side)
                .map(|(index, _)| TierRef::Concave(index))
        };
        let flat = |indices: Vec<usize>| indices.into_iter().map(TierRef::Flat);

        let mut order = Vec::with_capacity(self.tiers.len() + self.concave_tiers.len());
        order.extend(flat(pavilion));
        order.extend(concave(false));
        order.extend(flat(crown));
        order.extend(concave(true));
        order.extend(flat(table));
        order
    }
}

#[cfg(test)]
mod tests;

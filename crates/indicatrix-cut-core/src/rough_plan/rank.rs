//! Merging, ranking and deduplicating candidate layouts.
//!
//! Order: total volume descending (volumes within 2^-32 relative of each
//! other count as equal, so that float noise cannot split a genuine tie),
//! then FEWER stones, then the composition, then the cut order. Layouts with
//! the same composition (the `(entry_id, count)` multiset) collapse to the
//! best one, and at most [`SAME_SET_CAP`] (three) layouts may share one design
//! set (counts ignored), which keeps the list varied. That cap is the ONE
//! per-set limit of every ranked list (per cut order, per pass and final), so a
//! selection that uses a single design shows at most three layouts however many
//! stone counts are feasible.
//!
//! Every ranked list keeps only the layouts within the planner's [`LayoutCaps`] (the design
//! cap and the stone cap): the `_within` functions take both, the `_capped` ones the design
//! cap alone and the plain ones neither.

use std::{
    borrow::Borrow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use super::{
    SAME_SET_CAP,
    types::{CandidateDesign, LayoutCaps, LayoutGroup, RoughLayout},
};

/// Two volumes at most this far apart, relative to the larger, count as equal (2^-32).
const VOLUME_REL_TOL: f64 = 1.0 / 4_294_967_296.0;

/// Whether two layout volumes are equal for ranking: `|a - b| <= 2^-32 * max(|a|, |b|)`.
///
/// This is NOT transitive (`a ~ b` and `b ~ c` do not imply `a ~ c`), so it is only ever
/// applied between a volume and the anchor of its tie group, see [`rank_indices`].
fn volumes_tie(a: f64, b: f64) -> bool {
    (a - b).abs() <= VOLUME_REL_TOL * a.abs().max(b.abs())
}

/// The ranking order of two layouts of tied volume given their precomputed compositions:
/// fewer stones, then the composition, then the cut order.
fn tie_order(
    a: &RoughLayout,
    comp_a: &[(i64, usize)],
    b: &RoughLayout,
    comp_b: &[(i64, usize)],
) -> Ordering {
    a.stones
        .len()
        .cmp(&b.stones.len())
        .then_with(|| comp_a.cmp(comp_b))
        .then(a.cut_order.cmp(&b.cut_order))
}

/// The layout behind a candidate handle (a layout or a reference to one).
pub fn as_layout<L: Borrow<RoughLayout>>(handle: &L) -> &RoughLayout {
    handle.borrow()
}

/// Indices into `candidates` of the ranked, deduplicated, capped selection
/// (at most `limit`); empty layouts are skipped.
///
/// The candidates are first sorted by exact volume, largest first. Then the list is cut
/// into tie groups: a group starts at the first layout not yet placed and takes every
/// following layout whose volume is within 2^-32 relative of the START of the group.
/// Anchoring to the start keeps the relation a proper equivalence (a chain of volumes that
/// each differ from the next by less than the tolerance does not fuse into one group, and the
/// sort never sees an inconsistent order). Within a group the order is fewer stones,
/// composition, cut order and finally the input position, so the result does not depend on
/// float noise below the tolerance. Volumes that straddle a group edge are ranked by volume.
#[must_use]
pub fn rank_indices<L: Borrow<RoughLayout>>(candidates: &[L], limit: usize) -> Vec<usize> {
    rank_indices_within(candidates, limit, LayoutCaps::NONE)
}

/// [`rank_indices`] over the candidates that use at most `max_designs` different designs.
///
/// The design cap alone, any number of stones: [`rank_indices_within`] with
/// [`LayoutCaps::designs_only`]. `usize::MAX` is exactly [`rank_indices`] (empty layouts are
/// skipped either way).
#[must_use]
pub fn rank_indices_capped<L: Borrow<RoughLayout>>(
    candidates: &[L],
    limit: usize,
    max_designs: usize,
) -> Vec<usize> {
    rank_indices_within(candidates, limit, LayoutCaps::designs_only(max_designs))
}

/// [`rank_indices`] over the candidates `caps` admits: at most `caps.max_designs` different
/// designs and at most `caps.max_stones` stones.
///
/// This is the ONE place the planner's design cap and stone cap are applied to a ranked list.
/// Layouts above either are dropped before sorting, dedup and the per-set cap, so every slot
/// goes to a qualifying layout: it is the ranking over the capped candidate set.
/// [`LayoutCaps::NONE`] is exactly [`rank_indices`] (empty layouts are skipped either way).
#[must_use]
pub fn rank_indices_within<L: Borrow<RoughLayout>>(
    candidates: &[L],
    limit: usize,
    caps: LayoutCaps,
) -> Vec<usize> {
    let comps: Vec<Vec<(i64, usize)>> = candidates
        .iter()
        .map(|c| as_layout(c).composition())
        .collect();
    let volume = |i: usize| as_layout(&candidates[i]).total_volume_mm3;
    let mut order: Vec<usize> = (0..candidates.len())
        .filter(|&i| caps.admits(as_layout(&candidates[i])))
        .collect();
    order.sort_by(|&i, &j| volume(j).total_cmp(&volume(i)).then(i.cmp(&j)));
    let mut start = 0;
    while start < order.len() {
        let anchor = volume(order[start]);
        let end = order[start..]
            .iter()
            .position(|&i| !volumes_tie(anchor, volume(i)))
            .map_or(order.len(), |p| start + p)
            .max(start + 1);
        order[start..end].sort_by(|&i, &j| {
            tie_order(
                as_layout(&candidates[i]),
                &comps[i],
                as_layout(&candidates[j]),
                &comps[j],
            )
            .then(i.cmp(&j))
        });
        start = end;
    }
    let mut seen: BTreeSet<&[(i64, usize)]> = BTreeSet::new();
    let mut per_set: BTreeMap<Vec<i64>, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for i in order {
        if out.len() >= limit {
            break;
        }
        if !seen.insert(&comps[i]) {
            continue;
        }
        let set: Vec<i64> = comps[i].iter().map(|(id, _)| *id).collect();
        let used = per_set.entry(set).or_insert(0);
        if *used >= SAME_SET_CAP {
            continue;
        }
        *used += 1;
        out.push(i);
    }
    out
}

/// The best `limit` layouts of `candidates`, ranked and deduplicated.
///
/// The order is total volume descending (fewer stones first on a tie), one layout per
/// composition (the best), and at most [`SAME_SET_CAP`] (three) layouts per design set.
/// There are fewer than `limit` results when fewer distinct compositions exist or when the
/// per-set cap drops the rest: a list that uses one design never has more than three.
#[must_use]
pub fn merge_and_rank(candidates: Vec<RoughLayout>, limit: usize) -> Vec<RoughLayout> {
    merge_and_rank_within(candidates, limit, LayoutCaps::NONE)
}

/// [`merge_and_rank`] keeping only layouts of at most `max_designs` different designs (see
/// [`rank_indices_capped`]).
#[must_use]
pub fn merge_and_rank_capped(
    candidates: Vec<RoughLayout>,
    limit: usize,
    max_designs: usize,
) -> Vec<RoughLayout> {
    merge_and_rank_within(candidates, limit, LayoutCaps::designs_only(max_designs))
}

/// [`merge_and_rank`] keeping only the layouts `caps` admits: its design cap and its stone
/// cap (see [`rank_indices_within`]).
#[must_use]
pub fn merge_and_rank_within(
    candidates: Vec<RoughLayout>,
    limit: usize,
    caps: LayoutCaps,
) -> Vec<RoughLayout> {
    let keep = rank_indices_within(&candidates, limit, caps);
    take_indices(candidates, &keep)
}

/// The layouts of `candidates` at `keep`, in that order (each index at most once).
pub fn take_indices(candidates: Vec<RoughLayout>, keep: &[usize]) -> Vec<RoughLayout> {
    let mut slots: Vec<Option<RoughLayout>> = candidates.into_iter().map(Some).collect();
    keep.iter().filter_map(|&i| slots[i].take()).collect()
}

/// Every layout of `groups` in one list (group after group, layout order kept), and the
/// index of the group each came from. The layouts are borrowed, not copied.
#[must_use]
pub fn flatten_groups(groups: &[LayoutGroup]) -> (Vec<&RoughLayout>, Vec<usize>) {
    let mut flat = Vec::new();
    let mut group_of = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        for layout in &group.layouts {
            flat.push(layout);
            group_of.push(index);
        }
    }
    (flat, group_of)
}

/// The single best layout under the ranking order.
#[must_use]
pub fn best_layout(layouts: &[RoughLayout]) -> Option<&RoughLayout> {
    rank_indices(layouts, 1).first().map(|&i| &layouts[i])
}

/// The entry id used most in `layout` (the lowest id on a tie).
pub fn most_used_design(layout: &RoughLayout) -> Option<i64> {
    let mut best: Option<(i64, usize)> = None;
    for (id, count) in layout.composition() {
        if best.is_none_or(|(_, b)| count > b) {
            best = Some((id, count));
        }
    }
    best.map(|(id, _)| id)
}

/// The design that contributes the least finished volume to `layout` (the highest
/// `entry_id` on a tie): the one the design-cap chain drops next.
pub fn least_contributing_design(layout: &RoughLayout) -> Option<i64> {
    let mut volume: BTreeMap<i64, f64> = BTreeMap::new();
    for stone in &layout.stones {
        *volume.entry(stone.entry_id).or_insert(0.0) += stone.volume_mm3;
    }
    let mut least: Option<(i64, f64)> = None;
    for (id, v) in volume {
        if least.is_none_or(|(_, l)| v <= l) {
            least = Some((id, v));
        }
    }
    least.map(|(id, _)| id)
}

/// The designs of `designs` (kept in their order) that `layout` uses, without `drop`.
pub fn pool_without(
    designs: &[CandidateDesign],
    layout: &RoughLayout,
    drop: i64,
) -> Vec<CandidateDesign> {
    designs
        .iter()
        .filter(|d| d.entry_id != drop && layout.stones.iter().any(|s| s.entry_id == d.entry_id))
        .copied()
        .collect()
}

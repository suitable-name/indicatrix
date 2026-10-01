//! The clipped piece table for shaped roughs.
//!
//! Evaluates and caches the value, design and assignment of every possible piece
//! range `[a0, a1) x [b0, b1) x [c0, c1)` against the rough's geometry.
//!
//! Fast classification against non-bounding-box cut/prism planes classifies pieces into:
//! - Interior: all 8 corners satisfy all planes -> size-only table value O(1).
//! - Exterior: some plane has all 8 corners outside -> `-inf`.
//! - Partial: solves the LP for the best (design, orientation) pairs of the piece's size,
//!   taken in descending order of their unclipped box value (see [`CLIP_SHORTLIST`]).
//!
//! A corner counts as outside a plane only when it lies more than [`PLANE_EPS_MM`] beyond it,
//! so a box that touches a plane is interior however the two positions were rounded.

use std::ops::Range;

use glam::DVec3;

use super::{
    grid::ShapedGrid,
    parallel::{balanced_slices, run_lanes},
    rows::{ClipRegion, PartialSolver, caliper_extents},
};
use crate::rough_plan::{
    CandidateDesign, Grid, PieceTable, PlanProgress, PlanSettings,
    piece::{ASSIGNMENTS, Norm, stone_value},
};

/// Number of (design, orientation) candidate pairs evaluated via LP for a partial piece.
///
/// A piece whose first [`CLIP_SHORTLIST`] pairs all fail keeps trying down the bound
/// order, up to [`CLIP_SHORTLIST_MAX`] pairs, until one succeeds.
pub const CLIP_SHORTLIST: usize = 5;

/// Most pairs tried for one partial piece while none of them has produced a stone.
pub const CLIP_SHORTLIST_MAX: usize = 16;

/// Classification of a piece box relative to non-bounding-box rough planes.
pub const CLASS_INTERIOR: u8 = 0;
/// The piece box lies entirely outside at least one halfspace plane.
pub const CLASS_EXTERIOR: u8 = 1;
/// The piece box intersects one or more planes and must be scaled via LP.
pub const CLASS_PARTIAL: u8 = 2;

/// Distance in mm beyond a plane that a corner must exceed to count as outside it.
///
/// Piece boxes and planes reach the same position through different roundings, so an
/// exact comparison would flip the class of a box that merely touches a plane.
pub const PLANE_EPS_MM: f64 = 1e-9;

/// Pieces between two progress reports of [`build_clipped_table`]. The build polls for
/// cancellation at the same points.
pub const GRID_POLL_PIECES: usize = 1_024;

/// Progress events [`build_clipped_table`] reports for a build of `entries` table entries:
/// one before every [`GRID_POLL_PIECES`]-th entry, `ceil(entries / GRID_POLL_PIECES)` in all.
///
/// Event number `i` (from 1) is `PlanProgress::Grid { done: i, total }` with this count as
/// `total`. A table built in slices reports the sum of the slices' counts; a slice holds
/// `slice_entry_range(grid, &slice).len()` entries.
#[must_use]
pub const fn grid_poll_events(entries: usize) -> usize {
    entries.div_ceil(GRID_POLL_PIECES)
}

/// The planes of `halfspaces` (already inset by `inset_mm`) that are not one of
/// the six faces of the base box with the given `extents`.
#[must_use]
pub fn filter_non_box(
    halfspaces: &[(DVec3, f64)],
    extents: [f64; 3],
    inset_mm: f64,
) -> Vec<(DVec3, f64)> {
    let box_targets = [
        (DVec3::X, extents[0] - inset_mm),
        (DVec3::NEG_X, -inset_mm),
        (DVec3::Y, extents[1] - inset_mm),
        (DVec3::NEG_Y, -inset_mm),
        (DVec3::Z, extents[2] - inset_mm),
        (DVec3::NEG_Z, -inset_mm),
    ];
    let is_box_plane = |n: DVec3, m: f64| -> bool {
        box_targets
            .iter()
            .any(|&(bn, bm)| (n - bn).length_squared() < 1e-12 && (m - bm).abs() < 1e-9)
    };
    halfspaces
        .iter()
        .copied()
        .filter(|&(n, m)| !is_box_plane(n, m))
        .collect()
}

/// A precomputed clipped piece table over a grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ClippedTable {
    /// Grid cells `[gx, gy, gz]`.
    pub cells: [usize; 3],
    /// Piece values per range triple (length `pa * pb * pc`).
    pub values: Vec<f64>,
    /// Best design index in `front` per range triple.
    pub design: Vec<u32>,
    /// Best assignment (0..6) per range triple.
    pub orient: Vec<u8>,
    /// Corner classification cache per range triple.
    pub classes: Vec<u8>,
}

impl ClippedTable {
    /// Flat index of canonical range indices `[ra, rb, rc]`.
    #[must_use]
    pub const fn index(&self, ra: usize, rb: usize, rc: usize) -> usize {
        let pb = self.cells[1] * (self.cells[1] + 1) / 2;
        let pc = self.cells[2] * (self.cells[2] + 1) / 2;
        ra * (pb * pc) + rb * pc + rc
    }

    /// Evaluates the value for canonical range indices `(ra, rb, rc)`.
    #[must_use]
    pub fn value(&self, ra: usize, rb: usize, rc: usize) -> f64 {
        self.values[self.index(ra, rb, rc)]
    }

    /// The table of `cells` made of `parts`: the tables of consecutive `a0`
    /// slices (see [`BuildClipParams::slice`]) in slice order, concatenated.
    ///
    /// Slices are laid out one after another, so the result is bitwise the table
    /// one full-range build would produce.
    #[must_use]
    pub fn concat(cells: [usize; 3], parts: Vec<Self>) -> Self {
        let mut table = Self {
            cells,
            values: Vec::new(),
            design: Vec::new(),
            orient: Vec::new(),
            classes: Vec::new(),
        };
        for part in parts {
            table.values.extend(part.values);
            table.design.extend(part.design);
            table.orient.extend(part.orient);
            table.classes.extend(part.classes);
        }
        table
    }
}

/// The entries `slice` occupies in the full table of `grid`.
///
/// Use it to cut the matching part out of a full table's `classes` when handing
/// them to a sliced build as [`BuildClipParams::cached_classes`].
#[must_use]
pub const fn slice_entry_range(grid: &ShapedGrid, slice: &Range<usize>) -> Range<usize> {
    entries_before(grid, slice.start)..entries_before(grid, slice.end)
}

/// Entries of the full table that belong to the planes `a0' < a0`.
const fn entries_before(grid: &ShapedGrid, a0: usize) -> usize {
    let per_a = grid.range_count(1) * grid.range_count(2);
    (a0 * grid.cells[0] - a0 * a0.saturating_sub(1) / 2) * per_a
}

/// Classifies a box `[min_corner, max_corner]` against non-bounding-box planes.
///
/// Returns `(class, violated_plane_indices)`.
#[must_use]
pub fn classify_box(
    min_corner: [f64; 3],
    max_corner: [f64; 3],
    planes: &[(DVec3, f64)],
) -> (u8, Vec<usize>) {
    let mut violated = Vec::new();
    let class = classify_box_into(min_corner, max_corner, planes, &mut violated);
    (class, violated)
}

/// [`classify_box`] writing the violated plane indices into a reusable buffer
/// (cleared first; empty for an exterior box).
///
/// This is the one predicate every shaped stage classifies with. A plane `(n, m)`
/// excludes the box when even the corner nearest to it lies more than
/// [`PLANE_EPS_MM`] beyond it (`n . p > m + PLANE_EPS_MM`), and is listed as violated
/// when the farthest corner does.
pub fn classify_box_into(
    min_corner: [f64; 3],
    max_corner: [f64; 3],
    planes: &[(DVec3, f64)],
    violated: &mut Vec<usize>,
) -> u8 {
    violated.clear();
    for (i, &(n, m)) in planes.iter().enumerate() {
        let pick = |low: bool| {
            DVec3::new(
                if (n.x >= 0.0) == low {
                    min_corner[0]
                } else {
                    max_corner[0]
                },
                if (n.y >= 0.0) == low {
                    min_corner[1]
                } else {
                    max_corner[1]
                },
                if (n.z >= 0.0) == low {
                    min_corner[2]
                } else {
                    max_corner[2]
                },
            )
        };
        let limit = m + PLANE_EPS_MM;
        if n.dot(pick(true)) > limit {
            violated.clear();
            return CLASS_EXTERIOR;
        }
        if n.dot(pick(false)) > limit {
            violated.push(i);
        }
    }

    if violated.is_empty() {
        CLASS_INTERIOR
    } else {
        CLASS_PARTIAL
    }
}

/// Input parameters for building a clipped piece table.
#[derive(Debug, Clone)]
pub struct BuildClipParams<'a> {
    /// The unit grid over the rough.
    pub grid: &'a ShapedGrid,
    /// Candidate designs.
    pub front: &'a [CandidateDesign],
    /// Non-bounding-box cut/prism/pebble planes.
    pub non_box_planes: &'a [(DVec3, f64)],
    /// Size-only piece table for interior pieces. It must have been built from
    /// `front` (its design indices point into `front`).
    pub size_table: &'a PieceTable,
    /// Plan settings (allowance, min width).
    pub settings: &'a PlanSettings,
    /// The a0 slice of ranges to process.
    pub slice: Range<usize>,
    /// Optional cached classification flags (the `classes` of an earlier table
    /// over the same grid and planes, whatever its designs).
    ///
    /// The cache is indexed by the position of an entry WITHIN THIS SLICE:
    /// `cache[i]` is the class of the i-th entry the slice produces, so it holds
    /// exactly `slice_entry_range(grid, &slice).len()` flags. A build over a
    /// slice that starts past `0` must pass the matching part of a full table's
    /// classes, `&full.classes[slice_entry_range(grid, &slice)]`, never the whole
    /// cache.
    pub cached_classes: Option<&'a [u8]>,
}

/// One (design, orientation) pair of a piece size, ranked by its unclipped box value.
#[derive(Debug, Clone, Copy)]
struct Pair {
    /// Finished volume of the design in the unclipped stone box; an upper bound of
    /// any clipped value.
    bound: f64,
    /// Index into the front.
    design: u32,
    /// Assignment, an index into [`ASSIGNMENTS`].
    orient: u8,
}

impl Pair {
    /// A slot no pair has filled.
    const EMPTY: Self = Self {
        bound: f64::NEG_INFINITY,
        design: 0,
        orient: 0,
    };
}

/// Puts `pair` into the first `*len` slots of `slots`, which are ordered by descending
/// bound, keeping an earlier pair first on equal bounds. The last slot falls off when
/// all of them are full.
fn insert_pair(slots: &mut [Pair], len: &mut usize, pair: Pair) {
    let cap = slots.len();
    let at = slots[..*len]
        .iter()
        .position(|kept| kept.bound < pair.bound)
        .unwrap_or(*len);
    if at >= cap {
        return;
    }
    let end = (*len).min(cap - 1);
    slots.copy_within(at..end, at + 1);
    slots[at] = pair;
    *len = (*len + 1).min(cap);
}

/// Fills `slots` with the best pairs of `norms` in a stone box `dims`, by descending
/// bound and then ascending (design, assignment); returns how many it holds.
///
/// This is the prefix of the stable sort of all feasible pairs.
fn best_pairs(slots: &mut [Pair], norms: &[Norm], dims: [f64; 3], min_width: f64) -> usize {
    let mut len = 0;
    for (design, norm) in norms.iter().enumerate() {
        for orient in 0..ASSIGNMENTS.len() {
            let bound = stone_value(norm, orient, dims, min_width);
            if bound > f64::NEG_INFINITY {
                let pair = Pair {
                    bound,
                    design: design as u32,
                    orient: orient as u8,
                };
                insert_pair(slots, &mut len, pair);
            }
        }
    }
    len
}

/// The shortlist of (design, assignment) pairs for every piece size of a grid.
///
/// The unclipped box value of a pair depends on the piece's size alone, so the
/// ranking is done once per size instead of once per partial piece.
struct SizeShortlist {
    /// Grid cells per axis.
    cells: [usize; 3],
    /// [`CLIP_SHORTLIST_MAX`] slots per size.
    pairs: Vec<Pair>,
    /// Filled slots per size.
    counts: Vec<u8>,
}

impl SizeShortlist {
    /// The shortlists of every size `(gx, gy, gz)` in `1..=cells` for `norms`.
    fn build(params: &BuildClipParams<'_>, norms: &[Norm]) -> Self {
        let cells = params.grid.cells;
        let grid = Grid::with_cells(&params.grid.block(), params.settings, cells);
        let sizes = cells[0] * cells[1] * cells[2];
        let mut pairs = vec![Pair::EMPTY; sizes * CLIP_SHORTLIST_MAX];
        let mut counts = vec![0u8; sizes];
        let mut size = 0;
        for gx in 1..=cells[0] {
            for gy in 1..=cells[1] {
                for gz in 1..=cells[2] {
                    let dims = grid.stone_box([gx, gy, gz]);
                    let slots = &mut pairs[size * CLIP_SHORTLIST_MAX..][..CLIP_SHORTLIST_MAX];
                    let min_width = params.settings.min_width_mm;
                    counts[size] = best_pairs(slots, norms, dims, min_width) as u8;
                    size += 1;
                }
            }
        }
        Self {
            cells,
            pairs,
            counts,
        }
    }

    /// The pairs of a piece `lens` cells in size, best bound first.
    fn pairs_of(&self, lens: [usize; 3]) -> &[Pair] {
        let size = ((lens[0] - 1) * self.cells[1] + (lens[1] - 1)) * self.cells[2] + (lens[2] - 1);
        &self.pairs[size * CLIP_SHORTLIST_MAX..][..usize::from(self.counts[size])]
    }
}

/// Per-build workspace: the norms, the solver and the scratch buffers.
struct PieceWork<'a> {
    params: &'a BuildClipParams<'a>,
    norms: Vec<Norm>,
    solver: PartialSolver,
    violated: Vec<usize>,
    /// Built when the first partial piece asks for it.
    shortlist: Option<SizeShortlist>,
    /// Progress events this build reports, see [`grid_poll_events`].
    polls: usize,
}

impl PieceWork<'_> {
    /// Fills entry `entry` of `table` (an index into the slice's own table, not the
    /// whole grid's) for the piece with the given ranges.
    fn fill(
        &mut self,
        table: &mut ClippedTable,
        entry: usize,
        ranges: [(usize, usize); 3],
        cached: Option<u8>,
    ) {
        let params = self.params;
        let (origin_mm, size_mm) = params.grid.piece_box(ranges);
        let (b_min, b_max) =
            ShapedGrid::stone_box(origin_mm, size_mm, params.settings.allowance_mm);
        let class = match cached {
            Some(class) if class != CLASS_PARTIAL => class,
            _ => classify_box_into(b_min, b_max, params.non_box_planes, &mut self.violated),
        };
        table.classes[entry] = class;
        let lens = ranges.map(|(start, end)| end - start);
        match class {
            CLASS_INTERIOR => {
                let s_idx = params.size_table.index(lens);
                table.values[entry] = params.size_table.values[s_idx];
                table.design[entry] = params.size_table.design[s_idx];
                table.orient[entry] = params.size_table.orient[s_idx];
            }
            CLASS_PARTIAL => {
                let region = ClipRegion {
                    min: b_min,
                    max: b_max,
                    planes: params.non_box_planes,
                    violated: &self.violated,
                };
                let shortlist = self
                    .shortlist
                    .get_or_insert_with(|| SizeShortlist::build(params, &self.norms));
                let (val, des, ori) = solve_partial_piece(
                    &region,
                    &self.norms,
                    params.settings.min_width_mm,
                    &mut self.solver,
                    shortlist.pairs_of(lens),
                );
                table.values[entry] = val;
                table.design[entry] = des;
                table.orient[entry] = ori;
            }
            _ => {}
        }
    }

    /// Reports the progress event due before entry number `done` of this build, if any;
    /// `false` when `on_progress` cancels.
    fn poll(&self, done: usize, on_progress: &mut dyn FnMut(PlanProgress) -> bool) -> bool {
        if !done.is_multiple_of(GRID_POLL_PIECES) {
            return true;
        }
        on_progress(PlanProgress::Grid {
            done: done / GRID_POLL_PIECES + 1,
            total: self.polls,
        })
    }

    /// Fills the entries of the plane `[a0, a1)` starting at `entry`, which counts over
    /// the whole grid while `table` holds only the slice (its first entry is `base`);
    /// returns the next free entry, or `None` when `on_progress` cancels.
    fn fill_plane(
        &mut self,
        table: &mut ClippedTable,
        on_progress: &mut dyn FnMut(PlanProgress) -> bool,
        a_range: (usize, usize),
        (mut entry, base): (usize, usize),
    ) -> Option<usize> {
        let [_, gb, gc] = self.params.grid.cells;
        for b0 in 0..gb {
            for b1 in (b0 + 1)..=gb {
                for c0 in 0..gc {
                    for c1 in (c0 + 1)..=gc {
                        let local = entry - base;
                        if !self.poll(local, on_progress) {
                            return None;
                        }
                        let cached = self.params.cached_classes.map(|cache| cache[local]);
                        self.fill(table, local, [a_range, (b0, b1), (c0, c1)], cached);
                        entry += 1;
                    }
                }
            }
        }
        Some(entry)
    }
}

/// Builds the clipped piece table for an `a0` range slice.
///
/// If `cached_classes` is provided (see [`BuildClipParams::cached_classes`] for
/// its slice-local indexing), skips geometry re-classification of interior and
/// exterior pieces and recomputes values only.
///
/// Reports `PlanProgress::Grid` before every [`GRID_POLL_PIECES`]-th entry of the slice:
/// `grid_poll_events(entries)` events in all, each with that count as its `total`.
/// `on_progress` is also the cancellation point.
///
/// # Returns
///
/// `None` if cancelled via `on_progress`.
pub fn build_clipped_table(
    params: &BuildClipParams<'_>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<ClippedTable> {
    let ga = params.grid.cells[0];
    let entries = slice_entry_range(params.grid, &params.slice);
    let base = entries.start;
    let total_slice_entries = entries.len();
    let mut table = ClippedTable {
        cells: params.grid.cells,
        values: vec![f64::NEG_INFINITY; total_slice_entries],
        design: vec![0u32; total_slice_entries],
        orient: vec![0u8; total_slice_entries],
        classes: vec![CLASS_EXTERIOR; total_slice_entries],
    };
    let mut work = PieceWork {
        params,
        norms: params.front.iter().map(Norm::of).collect(),
        solver: PartialSolver::new(),
        violated: Vec::new(),
        shortlist: None,
        polls: grid_poll_events(total_slice_entries),
    };

    let mut entry = base;
    for a0 in params.slice.clone() {
        for a1 in (a0 + 1)..=ga {
            entry = work.fill_plane(&mut table, on_progress, (a0, a1), (entry, base))?;
        }
    }
    Some(table)
}

/// [`build_clipped_table`] of the whole grid (`params.slice` must be `0..cells[0]`) on
/// `lanes` scoped threads, one `a0` slice per thread.
///
/// The slices are balanced by their range count and concatenated in order, so the table
/// is bitwise the one a single build produces, whatever the lane count. Cached classes
/// are cut per slice. Progress events of the lanes arrive at `on_progress` on the
/// calling thread; per slice the events follow [`build_clipped_table`]. With one lane, or
/// a slice that is not the whole grid, it is [`build_clipped_table`] itself. `None` when
/// cancelled.
pub fn build_clipped_table_lanes(
    params: &BuildClipParams<'_>,
    lanes: usize,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<ClippedTable> {
    let grid = params.grid;
    if lanes <= 1 || params.slice != (0..grid.cells[0]) {
        return build_clipped_table(params, on_progress);
    }
    let parts = run_lanes(
        balanced_slices(grid.cells[0], lanes),
        |slice, report| {
            let cached = params
                .cached_classes
                .map(|classes| &classes[slice_entry_range(grid, &slice)]);
            let part = BuildClipParams {
                grid,
                front: params.front,
                non_box_planes: params.non_box_planes,
                size_table: params.size_table,
                settings: params.settings,
                slice,
                cached_classes: cached,
            };
            build_clipped_table(&part, report)
        },
        on_progress,
    )?;
    Some(ClippedTable::concat(grid.cells, parts))
}

/// Solves the LP for the best pairs of a partial piece's size, in descending order of
/// their unclipped box value.
///
/// A pair is skipped, and the search ends, once its bound cannot beat the best clipped
/// value found. Only the first [`CLIP_SHORTLIST`] pairs are solved when one of them
/// produced a stone; while none has, the search goes on down the list.
///
/// Returns `(value, design index, assignment)`; the value is `-inf` when no
/// pair fits.
fn solve_partial_piece(
    region: &ClipRegion<'_>,
    norms: &[Norm],
    min_width: f64,
    solver: &mut PartialSolver,
    pairs: &[Pair],
) -> (f64, u32, u8) {
    let mut best = (f64::NEG_INFINITY, 0u32, 0u8);
    for (attempt, pair) in pairs.iter().enumerate() {
        // The clipped value never exceeds the box bound, and the bounds only
        // fall from here on: nothing left can strictly beat the incumbent.
        if pair.bound <= best.0 || (attempt >= CLIP_SHORTLIST && best.0 > f64::NEG_INFINITY) {
            break;
        }
        let norm = &norms[pair.design as usize];
        let extents = caliper_extents(norm, usize::from(pair.orient));
        if let Some((k, _)) = solver.solve(region, extents)
            && k >= min_width
        {
            let val = norm.f * (k * k * k);
            if val > best.0 {
                best = (val, pair.design, pair.orient);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rough_plan::tests::{Lcg, random_designs};

    /// Every feasible pair in (design, assignment) order, stably sorted by descending
    /// bound: the list a size shortlist is a prefix of.
    fn all_pairs(norms: &[Norm], dims: [f64; 3], min_width: f64) -> Vec<(f64, usize, usize)> {
        let mut all = Vec::new();
        for (design, norm) in norms.iter().enumerate() {
            for orient in 0..ASSIGNMENTS.len() {
                let value = stone_value(norm, orient, dims, min_width);
                if value > f64::NEG_INFINITY {
                    all.push((value, design, orient));
                }
            }
        }
        all.sort_by(|a, b| b.0.total_cmp(&a.0));
        all
    }

    #[test]
    fn the_size_shortlist_is_the_prefix_of_the_stable_sort_of_all_pairs() {
        let mut rng = Lcg(5);
        let mut designs = random_designs(&mut rng, 10);
        // Repeated designs tie on every bound, across design indices.
        let repeats: Vec<_> = designs[..4].to_vec();
        designs.extend(repeats);
        let norms: Vec<Norm> = designs.iter().map(Norm::of).collect();
        for case in 0..40 {
            let dims = [
                rng.range(0.5, 8.0),
                rng.range(0.5, 8.0),
                rng.range(0.5, 8.0),
            ];
            let min_width = if case % 2 == 0 { 0.3 } else { 2.0 };
            let expected = all_pairs(&norms, dims, min_width);
            let mut slots = [Pair::EMPTY; CLIP_SHORTLIST_MAX];
            let len = best_pairs(&mut slots, &norms, dims, min_width);
            assert_eq!(len, expected.len().min(CLIP_SHORTLIST_MAX), "case {case}");
            for (slot, &(bound, design, orient)) in slots[..len].iter().zip(&expected) {
                assert_eq!(slot.bound.to_bits(), bound.to_bits(), "case {case}");
                assert_eq!(slot.design as usize, design, "case {case}");
                assert_eq!(usize::from(slot.orient), orient, "case {case}");
            }
        }
    }

    #[test]
    fn a_partial_piece_tries_past_the_fifth_pair_only_while_nothing_has_fit() {
        // In a 4 mm cube (minimum width 1 mm): the slim design scales to 4 / 10 = 0.4 mm
        // and fails; the good design reaches 4 mm, i.e. 4^3 = 64; the small one has the
        // same shape with a thousandth of the volume factor: 0.064.
        let norms = [
            Norm {
                l: 10.0,
                h: 1.0,
                f: 1.0,
            },
            Norm {
                l: 1.0,
                h: 1.0,
                f: 1.0,
            },
            Norm {
                l: 1.0,
                h: 1.0,
                f: 0.001,
            },
        ];
        let pair = |bound: f64, design: u32| Pair {
            bound,
            design,
            orient: 0,
        };
        let region = ClipRegion {
            min: [0.0; 3],
            max: [4.0; 3],
            planes: &[],
            violated: &[],
        };
        let mut solver = PartialSolver::new();
        let mut solve =
            |pairs: &[Pair]| solve_partial_piece(&region, &norms, 1.0, &mut solver, pairs);

        // Six failures come first: the search goes on to the seventh pair.
        let mut pairs: Vec<Pair> = (0..6).map(|i| pair(100.0 - f64::from(i), 0)).collect();
        pairs.push(pair(50.0, 1));
        let (value, design, orient) = solve(&pairs);
        assert!((value - 64.0).abs() < 1e-9, "value {value}");
        assert_eq!((design, orient), (1, 0));

        // A fit at once stops the search at the fifth pair: the good one at the sixth
        // place is never tried.
        let mut pairs = vec![pair(100.0, 2)];
        pairs.extend((0..4).map(|i| pair(99.0 - f64::from(i), 0)));
        pairs.push(pair(90.0, 1));
        let (value, design, _) = solve(&pairs);
        assert!((value - 0.064).abs() < 1e-9, "value {value}");
        assert_eq!(design, 2);

        // A pair whose bound is at most the best value found is never tried, and
        // neither is any after it.
        let (value, design, _) = solve(&[pair(100.0, 2), pair(0.01, 1)]);
        assert!((value - 0.064).abs() < 1e-9, "value {value}");
        assert_eq!(design, 2);

        // Nothing fits: no value.
        let (value, _, _) = solve(&[pair(100.0, 0)]);
        assert_eq!(value, f64::NEG_INFINITY);
        let (value, _, _) = solve(&[]);
        assert_eq!(value, f64::NEG_INFINITY);
    }

    #[test]
    fn the_poll_count_is_the_entries_over_the_poll_interval_rounded_up() {
        assert_eq!(grid_poll_events(0), 0);
        assert_eq!(grid_poll_events(1), 1);
        assert_eq!(grid_poll_events(GRID_POLL_PIECES), 1);
        assert_eq!(grid_poll_events(GRID_POLL_PIECES + 1), 2);
        assert_eq!(grid_poll_events(5 * GRID_POLL_PIECES), 5);
    }
}

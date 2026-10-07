//! Which geometry to hand the solid preview after an edit, chosen automatically among
//! three tiers of freshness.
//!
//! Pure and independent of `preview_state`'s worker
//! thread/`RedrawGate` machinery -- [`plan_preview`] is a plain function callers run
//! on the UI thread before ever touching `super::preview_state::SolidPreviewState`
//! (the desktop's own worker controller, which stays in `apps/indicatrix-cut`).
//!
//! # The three tiers
//!
//! 1. **Pinned.** Every tier is
//!    [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] -- each
//!    mast is read directly off its own constraint, no solver call, so the design
//!    previews instantly regardless of tier count. Checked first, unconditionally.
//! 2. **Fresh.** At least one tier is free, `last_solved` is aligned with `design`'s
//!    current tier count (the precondition `resolve_dirty` panics on otherwise), and
//!    [`DirtySolver::resolve_dirty`] finishes within `budget`.
//! 3. **Stale.** Same call as tier 2, but wall time exceeds `budget`. The solve can't
//!    be interrupted mid-flight, so "over budget" is only known once the result is in
//!    hand. That fresh result is still chained forward as [`PreviewPlan::solved`] (so
//!    the NEXT call doesn't repeat the same slow resolve), but this frame draws the
//!    OLD (`previous`) planes so the user sees the last solid actually finished.
//!    `pending` names every dirty tier, for the caller's overlay.
//!
//! `last_solved` missing or misaligned (an `AddTier`/`RemoveTier` edit, which can
//! never resolve as a subgraph) falls back to a full [`indicatrix_cut_core::Design::solve`];
//! a successful full solve is authoritative regardless of timing, reported
//! [`Freshness::Fresh`] too. Either path failing with [`indicatrix_cut_core::DesignSolveError`]
//! reports [`Freshness::Unsolvable`], falling back to the `last_solved` planes when they
//! still describe this design. A `last_solved` that does not (it predates an add or
//! remove) cannot be drawn, so `PreviewPlan::planes` is then empty and the renderer
//! keeps its last closed stone; it never panics, since the plan worker runs this.
//!
//! # The Cut slider
//!
//! [`CutLimit`] says how much of the cut to draw: the finished stone, the flat tiers up to
//! one index (the web slider), or the stone after k cutting steps with `0` the rough
//! ([`plan_preview_limited`]). [`display_geometry`] gives any redraw path the same planes
//! and tools for a limit, so they cannot disagree.
//!
//! # The injected [`Clock`]
//!
//! Tier 2/3's budget check needs a wall-clock reading, but this crate must stay
//! `Instant::now`-free (`std::time::Instant::now()` panics at runtime on
//! `wasm32-unknown-unknown` -- see the crate README), so [`plan_preview`] takes a
//! `&dyn Clock` rather than calling `std::time::Instant::now()` itself. The desktop
//! passes a small `std::time::Instant`-backed implementation of its own (see
//! `apps/indicatrix-cut/src/gui/solid_preview/preview_state/plan_worker.rs`); a web
//! caller passes one backed by `performance.now()`.

use glam::{DVec3, Vec3};
use indicatrix::geometry::{
    ToolPrimitive,
    cuts::StandardGemCuts,
    meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
};
use indicatrix_cut_core::{
    Design, DesignSolveError,
    design::{TierRef, ToolPlacements},
};
use std::{collections::BTreeSet, time::Duration};

/// Default over-budget threshold for [`plan_preview`]'s tier-2/3 decision.
///
/// `50` ms, a UI-responsiveness budget rather than a guarantee every design resolves
/// within it. Measured against the worst fixture available (CrackOtto-Step, 103
/// tiers, nearly all non-anchor): that resolve took **2.13 s**, confirming this is
/// exactly the shape tier 3 (`Freshness::Stale`) exists for -- a budget loose enough
/// to tolerate it would make ordinary ScaleReference-heavy edits feel just as
/// sluggish. See `tests::timing_resolve_dirty_on_crackotto_step_103_tier`.
pub const DEFAULT_PREVIEW_BUDGET: Duration = Duration::from_millis(50);

/// A monotonic clock injected into [`plan_preview`]'s tier-2/3 budget check --
/// see the module doc comment for why this crate never calls
/// `std::time::Instant::now()` directly.
///
/// Only ever read twice in a row (immediately before and after one
/// [`DirtySolver::resolve_dirty`] call) and the two readings' difference is all
/// [`plan_preview`] uses, so an implementation just needs a consistent,
/// monotonically non-decreasing "now" -- never wall-clock time, and never compared
/// across two different `Clock` values.
pub trait Clock {
    /// Milliseconds since an arbitrary, implementation-chosen origin.
    fn now_ms(&self) -> f64;
}

/// Abstracts [`indicatrix_cut_core::Design::resolve_dirty`] so [`plan_preview`] is testable:
/// a fake implementation can return canned results.
pub trait DirtySolver {
    /// # Errors
    ///
    /// See [`indicatrix_cut_core::Design::resolve_dirty`]'s own `# Errors` section.
    fn resolve_dirty(
        &self,
        design: &Design,
        previous: &[SolvedTier],
        dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError>;
}

/// The real [`DirtySolver`], delegating straight to [`indicatrix_cut_core::Design::resolve_dirty`].
/// Every other implementation in this crate exists solely for this module's tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealSolver;

impl DirtySolver for RealSolver {
    fn resolve_dirty(
        &self,
        design: &Design,
        previous: &[SolvedTier],
        dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        design.resolve_dirty(previous, dirty)
    }
}

/// Which of the module doc comment's three tiers [`plan_preview`] chose, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// No solver call was made.
    Pinned,
    /// Solved within budget (subgraph or full).
    Fresh,
    /// `pending` is the `dirty` set, for the caller's overlay/banner.
    Stale { pending: BTreeSet<usize> },
    /// See [`indicatrix_cut_core::DesignSolveError`].
    Unsolvable(DesignSolveError),
}

/// [`plan_preview`]'s result: what to draw, what to remember as `last_solved` next
/// call, and why.
#[derive(Debug, Clone)]
pub struct PreviewPlan {
    /// The plane arrangement to draw THIS frame, in the same `(normal, offset)` `f32`
    /// convention `mesh_cache::MeshCache::get_or_build` takes.
    pub planes: Vec<(Vec3, f32)>,
    /// The mast list to pass as `last_solved` next call -- can be newer than `planes`
    /// (see [`Freshness::Stale`]).
    pub solved: Option<Vec<SolvedTier>>,
    /// Whether `planes` matches the newest solve or is stale.
    pub freshness: Freshness,
}

/// Narrows a [`indicatrix_cut_core::Design::planes_from_solved`] result to the `f32`
/// convention every other `solid_preview` module uses.
fn narrow_planes(planes: Vec<(DVec3, f64)>) -> Vec<(Vec3, f32)> {
    planes
        .into_iter()
        .map(|(n, m)| (Vec3::new(n.x as f32, n.y as f32, n.z as f32), m as f32))
        .collect()
}

/// Which flat tiers the "show through tier N" slider shows, indexed like
/// `design.tiers`.
///
/// A planar design (no concave tiers) keeps its stored order: tiers `0..=cutoff` -- this is
/// the web slider's "show through tier N of the table"; the desktop Cut slider uses
/// [`CutLimit::Steps`], which follows [`Design::cutting_order`] for every design. With
/// concave tiers this slider walks [`Design::cutting_order`] too (plan §4.4): the shown flat
/// tiers are those cut up to and including flat tier `cutoff`, so the planes and the
/// concave tools of one frame always describe the same step of the cut. For a design
/// stored in cutting order the two rules agree.
pub(crate) fn visible_flat_tiers(design: &Design, cutoff: usize) -> Vec<bool> {
    let count = design.tiers.len();
    if design.concave_tiers.is_empty() {
        return (0..count).map(|i| i <= cutoff).collect();
    }
    let order = design.cutting_order();
    let Some(step) = order.iter().position(|t| *t == TierRef::Flat(cutoff)) else {
        // A cutoff past the last flat tier (or a stale one) means "every tier".
        return vec![true; count];
    };
    let mut visible = vec![false; count];
    for tier in &order[..=step] {
        if let TierRef::Flat(i) = tier {
            visible[*i] = true;
        }
    }
    visible
}

/// Which flat tiers (indexed like `design.tiers`) the planes of `limit` contain, `None`
/// for the finished stone (every tier).
///
/// The same rule [`planes_for_display`] draws with, so a [`crate::facet_map::FacetMap`]
/// built from it numbers the facets the way the drawn planes are numbered: the planes of
/// a partly cut stone are the finished stone's planes with the hidden tiers' slices
/// removed, in order.
#[must_use]
pub fn limit_visible_tiers(design: &Design, limit: CutLimit) -> Option<Vec<bool>> {
    match limit {
        CutLimit::Finished => None,
        CutLimit::ThroughTier(cutoff) => Some(visible_flat_tiers(design, cutoff)),
        CutLimit::Steps(steps) => {
            let order = design.preview_steps();
            if steps >= order.len() {
                return None;
            }
            let mut visible = vec![false; design.tiers.len()];
            for step in &order[..steps] {
                if let TierRef::Flat(index) = step
                    && let Some(slot) = visible.get_mut(*index)
                {
                    *slot = true;
                }
            }
            Some(visible)
        }
    }
}

/// `planes` of `design` restricted to the flat tiers `visible` marks (the preform's
/// planes are always kept): [`Design::planes_from_solved`] with each tier's own slice
/// of facet planes dropped when the tier is hidden. Tier slices are located exactly as
/// `Design::tier_for_plane_index` does, by the plane count of each schedule prefix.
fn planes_of_visible_tiers(
    design: &Design,
    solved: &[SolvedTier],
    visible: &[bool],
) -> Vec<(DVec3, f64)> {
    let planes = design.planes_from_solved(solved);
    let preform_len = design.preform.planes().len();
    let schedule = design.to_asc_schedule_from_solved(solved);
    let boundaries: Vec<usize> = (0..schedule.tiers.len())
        .map(|i| {
            let mut prefix = schedule.clone();
            prefix.tiers.truncate(i + 1);
            StandardGemCuts::from_asc_schedule(&prefix).len()
        })
        .collect();
    planes
        .into_iter()
        .enumerate()
        .filter(|(index, _)| {
            index
                .checked_sub(preform_len)
                .and_then(|local| boundaries.iter().position(|&end| local < end))
                .is_none_or(|tier| visible.get(tier).copied().unwrap_or(true))
        })
        .map(|(_, plane)| plane)
        .collect()
}

/// How much of the cut the preview draws: the Cut slider's position, in the one
/// vocabulary [`plan_preview_limited`] and [`display_geometry`] understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutLimit {
    /// Every tier: the finished stone.
    #[default]
    Finished,
    /// The flat tiers up to and including this index into `Design::tiers` -- the
    /// web app's slider, and what [`plan_preview`]'s `tier_cutoff` has always meant.
    ThroughTier(usize),
    /// The stone after this many cutting steps ([`Design::preview_steps`]); `0` is
    /// the rough, the preform alone. A count at or past the last step is the
    /// finished stone.
    Steps(usize),
}

impl CutLimit {
    /// The limit [`plan_preview`]'s `tier_cutoff` argument stands for.
    #[must_use]
    pub const fn from_tier_cutoff(tier_cutoff: Option<usize>) -> Self {
        match tier_cutoff {
            Some(through_tier) => Self::ThroughTier(through_tier),
            None => Self::Finished,
        }
    }
}

/// What a viewport draws for a design under a [`CutLimit`]: the planes, the concave
/// tools cut into them and where each tool came from.
#[derive(Debug, Clone, Default)]
pub struct DisplayGeometry {
    /// The plane arrangement, in `Design::planes_from_solved`'s `n . x <= m` form.
    pub planes: Vec<(DVec3, f64)>,
    /// The concave tools subtracted from `planes`; empty for a planar design.
    pub tools: Vec<ToolPrimitive>,
    /// `(concave tier, placement)` of each tool, parallel to `tools`.
    pub placements: ToolPlacements,
}

/// The geometry a viewport draws for `design` under `limit`, from a solved mast list.
///
/// The one entry every redraw path (the planner, the background-solve push, the full
/// refresh, the optimise ghost) goes through, so they cannot disagree about what a Cut
/// slider position shows.
///
/// Never panics. A `solved` that is not aligned with `design.tiers` (a stale list after
/// an add or remove) has nothing to draw, so the result is empty -- except the rough
/// ([`CutLimit::Steps`] of `0`), which needs no masts. Tools that do not resolve (an
/// invalid concave tier, too many placements) are left out: the flat stone is truthful,
/// a half-resolved set of tools is not.
#[must_use]
pub fn display_geometry(
    design: &Design,
    solved: &[SolvedTier],
    limit: CutLimit,
) -> DisplayGeometry {
    let planes = planes_for_display(design, solved, limit);
    let (tools, placements) = display_tools(design, solved, limit);
    DisplayGeometry {
        planes,
        tools,
        placements,
    }
}

/// [`plan_preview`]'s single choice of "the full arrangement" vs. "truncated" (the
/// "show through tier N" and Cut-step sliders) -- routing every one of
/// `plan_preview`'s five branches through here keeps that truncation decision in
/// exactly one place rather than five.
///
/// [`CutLimit::Finished`] reproduces [`Design::planes_from_solved`] exactly. A planar
/// [`CutLimit::ThroughTier`] truncates by stored position
/// ([`Design::planes_through_tier`]); a design with concave tiers follows
/// [`visible_flat_tiers`], the same rule that picks the concave tools.
/// [`CutLimit::Steps`] follows [`Design::try_planes_after_steps`].
///
/// A `solved` that is not aligned with `design.tiers` yields no planes instead of
/// panicking: this runs on the plan worker, which a panic would silently kill.
fn planes_for_display(
    design: &Design,
    solved: &[SolvedTier],
    limit: CutLimit,
) -> Vec<(DVec3, f64)> {
    if let CutLimit::Steps(steps) = limit {
        return design
            .try_planes_after_steps(solved, steps)
            .unwrap_or_default();
    }
    if solved.len() != design.tiers.len() {
        return Vec::new();
    }
    let CutLimit::ThroughTier(through_tier) = limit else {
        return design.planes_from_solved(solved);
    };
    if design.concave_tiers.is_empty() {
        return design.planes_through_tier(solved, through_tier);
    }
    let visible = visible_flat_tiers(design, through_tier);
    let is_stored_prefix = visible
        .iter()
        .enumerate()
        .all(|(i, &shown)| shown == (i <= through_tier));
    if is_stored_prefix {
        design.planes_through_tier(solved, through_tier)
    } else {
        planes_of_visible_tiers(design, solved, &visible)
    }
}

/// The concave tools to draw with [`display_geometry`]'s planes, and their placements.
///
/// Empty for a design without concave tiers (and so for every planar design, leaving the
/// frame byte-identical), when `solved` is not aligned with the flat tiers, and when the
/// concave tiers do not resolve.
///
/// For [`CutLimit::ThroughTier`] the tools are those that precede the first hidden flat
/// tier in cutting order, where "hidden" is [`visible_flat_tiers`]'s rule -- the one that
/// truncates the planes of the same frame, so a groove is never drawn into planes the
/// slider has removed. [`CutLimit::Steps`] takes the concave tiers among its first steps.
#[must_use]
pub fn display_tools(
    design: &Design,
    solved: &[SolvedTier],
    limit: CutLimit,
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    if design.concave_tiers.is_empty() || solved.len() != design.tiers.len() {
        return (Vec::new(), Vec::new());
    }
    let resolved = match limit {
        CutLimit::Finished => design.concave_tools_from_solved(solved),
        CutLimit::Steps(steps) => design.concave_tools_after_steps(solved, steps),
        CutLimit::ThroughTier(cutoff) => {
            // The first hidden flat tier in cutting order is the boundary; with no
            // hidden tier everything is shown.
            let visible = visible_flat_tiers(design, cutoff);
            let boundary = design
                .cutting_order()
                .into_iter()
                .find(|tier| matches!(tier, TierRef::Flat(i) if !visible[*i]));
            boundary.map_or_else(
                || design.concave_tools_from_solved(solved),
                |first_hidden| design.concave_tools_through_tier(solved, first_hidden),
            )
        }
    };
    resolved.unwrap_or_default()
}

/// Every tier's mast read directly off its own [`MeetConstraint::ScaleReference`] --
/// tier 1 of the module doc comment. Panics if any tier is not `ScaleReference`.
fn masts_from_pinned_tiers(design: &Design) -> Vec<SolvedTier> {
    design
        .tiers
        .iter()
        .map(|tier| {
            let MeetConstraint::ScaleReference(mast) = &tier.constraint else {
                unreachable!(
                    "masts_from_pinned_tiers: caller must confirm every tier is \
                     ScaleReference first"
                );
            };
            SolvedTier {
                mast: *mast,
                strategy: SolveStrategy::ScaleReference,
                detail: "pinned (ScaleReference)".to_string(),
            }
        })
        .collect()
}

/// Chooses which geometry to draw after an edit -- see the module doc comment for the
/// three tiers this implements.
///
/// `dirty` is the set of tier indices the triggering edit touched (see
/// `indicatrix_cut_core::resolve::affected_tiers`); `last_solved` is the previous call's
/// [`PreviewPlan::solved`], or `None` after an `AddTier`/`RemoveTier` edit.
///
/// `clock` is only read around the tier-2/3 [`DirtySolver::resolve_dirty`] call --
/// see the module doc comment for why this takes an injected [`Clock`] rather than
/// reading `std::time::Instant::now()` itself.
///
/// `tier_cutoff` is the "show through tier N" viewport slider: `Some(n)` truncates
/// every branch's drawn planes to `design.tiers[..=n]` (the preform's own planes
/// are always kept) via [`Design::planes_through_tier`] instead of the full
/// [`Design::planes_from_solved`] arrangement -- see [`planes_for_display`]. `None`
/// draws the full arrangement.
///
/// A thin wrapper over [`plan_preview_limited`] with [`CutLimit::from_tier_cutoff`].
#[must_use]
pub fn plan_preview(
    design: &Design,
    last_solved: Option<&[SolvedTier]>,
    dirty: &BTreeSet<usize>,
    budget: Duration,
    solver: &dyn DirtySolver,
    clock: &dyn Clock,
    tier_cutoff: Option<usize>,
) -> PreviewPlan {
    plan_preview_limited(
        design,
        last_solved,
        dirty,
        budget,
        solver,
        clock,
        CutLimit::from_tier_cutoff(tier_cutoff),
    )
}

/// [`plan_preview`] with the full [`CutLimit`] vocabulary: the desktop's Cut slider
/// asks for "the stone after k cutting steps" ([`CutLimit::Steps`], `0` being the
/// rough), which `tier_cutoff` cannot say.
///
/// Whatever the limit, `solved` in the result is the newest solve of the WHOLE design
/// (never truncated), so the next edit still has masts to diff against.
#[must_use]
pub fn plan_preview_limited(
    design: &Design,
    last_solved: Option<&[SolvedTier]>,
    dirty: &BTreeSet<usize>,
    budget: Duration,
    solver: &dyn DirtySolver,
    clock: &dyn Clock,
    limit: CutLimit,
) -> PreviewPlan {
    if design
        .tiers
        .iter()
        .all(|tier| matches!(tier.constraint, MeetConstraint::ScaleReference(_)))
    {
        let solved = masts_from_pinned_tiers(design);
        let planes = narrow_planes(planes_for_display(design, &solved, limit));
        return PreviewPlan {
            planes,
            solved: Some(solved),
            freshness: Freshness::Pinned,
        };
    }

    // Mismatch (or no previous solve) means an `AddTier`/`RemoveTier` edit happened;
    // a full solve is the only valid path.
    let aligned_previous = last_solved.filter(|previous| previous.len() == design.tiers.len());

    let Some(previous) = aligned_previous else {
        return match design.solve() {
            Ok(solved) => {
                let planes = narrow_planes(planes_for_display(design, &solved, limit));
                PreviewPlan {
                    planes,
                    solved: Some(solved),
                    freshness: Freshness::Fresh,
                }
            }
            Err(err) => {
                // `last_solved` is `None` or misaligned here (an aligned one took the
                // dirty-subgraph path above), so it cannot describe this design's tiers.
                // `planes_for_display` draws nothing for it instead of panicking, which
                // used to kill the plan worker and freeze the view until a restart; the
                // renderer then keeps its last closed stone, dimmed, under the status.
                // The rough needs no masts, so it is still drawn.
                let planes = narrow_planes(planes_for_display(
                    design,
                    last_solved.unwrap_or(&[]),
                    limit,
                ));
                PreviewPlan {
                    planes,
                    solved: None,
                    freshness: Freshness::Unsolvable(err),
                }
            }
        };
    };

    let start_ms = clock.now_ms();
    let result = solver.resolve_dirty(design, previous, dirty);
    let elapsed = Duration::from_secs_f64((clock.now_ms() - start_ms).max(0.0) / 1000.0);

    match result {
        Ok(new_solved) if elapsed <= budget => {
            let planes = narrow_planes(planes_for_display(design, &new_solved, limit));
            PreviewPlan {
                planes,
                solved: Some(new_solved),
                freshness: Freshness::Fresh,
            }
        }
        Ok(new_solved) => {
            // Over budget: show the OLD planes, but chain the fresh (late) result
            // forward as the next call's `last_solved`.
            let planes = narrow_planes(planes_for_display(design, previous, limit));
            PreviewPlan {
                planes,
                solved: Some(new_solved),
                freshness: Freshness::Stale {
                    pending: dirty.clone(),
                },
            }
        }
        Err(err) => {
            let planes = narrow_planes(planes_for_display(design, previous, limit));
            PreviewPlan {
                planes,
                solved: None,
                freshness: Freshness::Unsolvable(err),
            }
        }
    }
}

#[cfg(test)]
mod tests;

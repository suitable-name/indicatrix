//! Tests for [`super`].

use super::*;
// Only `crackotto_step_design` (below) needs these, and it is itself
// `#[cfg(not(target_arch = "wasm32"))]`.
#[cfg(not(target_arch = "wasm32"))]
use indicatrix::geometry::meet_solver::{Block, classify_blocks, meet_tier_inputs_from_asc};
use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
use std::cell::Cell;

/// Always reports `0.0` -- for every test whose branch never reaches the
/// [`DirtySolver`] call at all (so the clock value is irrelevant), plus the
/// "resolves within budget" case (`elapsed` is then always `0 <= budget`).
struct ZeroClock;

impl Clock for ZeroClock {
    fn now_ms(&self) -> f64 {
        0.0
    }
}

/// Reports `0.0` on its first call and `after_ms` on every call after --
/// simulates a slow [`DirtySolver::resolve_dirty`] without a real
/// `std::thread::sleep` (this crate must stay thread-free; see the crate
/// README). [`plan_preview`] reads the clock exactly once immediately before
/// and once immediately after the solver call, so two canned readings are
/// exactly what it needs.
struct JumpClock {
    calls: Cell<u32>,
    after_ms: f64,
}

impl JumpClock {
    const fn new(after_ms: f64) -> Self {
        Self {
            calls: Cell::new(0),
            after_ms,
        }
    }
}

impl Clock for JumpClock {
    fn now_ms(&self) -> f64 {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call == 0 { 0.0 } else { self.after_ms }
    }
}

/// Proves a branch of [`plan_preview`] never falls through to `resolve_dirty`.
struct PanicSolver;

impl DirtySolver for PanicSolver {
    fn resolve_dirty(
        &self,
        _design: &Design,
        _previous: &[SolvedTier],
        _dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        panic!("DirtySolver::resolve_dirty must not be called on this branch");
    }
}

/// Returns `result` immediately -- the "resolve finished" case, paired with
/// either [`ZeroClock`] (within budget) or [`JumpClock`] (over budget).
struct InstantSolver {
    result: Vec<SolvedTier>,
}

impl DirtySolver for InstantSolver {
    fn resolve_dirty(
        &self,
        _design: &Design,
        _previous: &[SolvedTier],
        _dirty: &BTreeSet<usize>,
    ) -> Result<Vec<SolvedTier>, DesignSolveError> {
        Ok(self.result.clone())
    }
}

fn tier(name: &str, angle_deg: f64, constraint: MeetConstraint) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![0.0],
        constraint,
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

fn pinned_design() -> Design {
    Design::new(
        PreformSpec::block(1.0, 1.0, 1.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![tier("Table", 0.0, MeetConstraint::ScaleReference(0.5))],
    )
}

/// One anchored (`ScaleReference`) tier plus one free (`MeetExisting`) tier in the
/// same block, just enough real structure for `Design::solve()` to succeed.
fn free_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![
            tier("C1", 30.0, MeetConstraint::ScaleReference(0.6)),
            tier("C2", 40.0, MeetConstraint::MeetExisting),
        ],
    )
}

#[test]
fn pinned_only_design_never_calls_the_solver() {
    let design = pinned_design();
    let plan = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    assert_eq!(plan.freshness, Freshness::Pinned);
    assert_ne!(plan.planes, [] as [(Vec3, f32); 0]);
    assert_eq!(plan.solved.map(|s| s.len()), Some(1));
}

#[test]
fn cheap_free_design_resolves_fresh_within_budget() {
    let design = free_design();
    let previous = design.solve().expect("fixture must solve");
    let solver = InstantSolver {
        result: previous.clone(),
    };
    let dirty = BTreeSet::from([1]);

    let plan = plan_preview(
        &design,
        Some(&previous),
        &dirty,
        Duration::from_millis(50),
        &solver,
        &ZeroClock,
        None,
    );
    assert_eq!(plan.freshness, Freshness::Fresh);
    assert!(plan.solved.is_some());
    assert_ne!(plan.planes, [] as [(Vec3, f32); 0]);
}

#[test]
fn over_budget_solver_reports_stale_with_the_edited_tier_pending() {
    let design = free_design();
    let previous = design.solve().expect("fixture must solve");
    let solver = InstantSolver {
        result: previous.clone(),
    };
    let dirty = BTreeSet::from([1]);
    let budget = Duration::from_millis(5);

    let plan = plan_preview(
        &design,
        Some(&previous),
        &dirty,
        budget,
        &solver,
        &JumpClock::new(40.0),
        None,
    );
    match plan.freshness {
        Freshness::Stale { pending } => assert_eq!(pending, dirty),
        other => panic!("expected Stale, got {other:?}"),
    }
    assert!(
        plan.solved.is_some(),
        "the fresh (late) result must still be chained forward as the next \
             call's last_solved, even though this frame reports Stale"
    );
}

#[test]
fn misaligned_previous_falls_back_to_a_full_solve() {
    let design = free_design();
    // Wrong length: stands in for a `last_solved` left over from before an
    // `AddTier`/`RemoveTier` edit.
    let stale_previous: Vec<SolvedTier> = Vec::new();
    let dirty = BTreeSet::from([1]);

    let plan = plan_preview(
        &design,
        Some(&stale_previous),
        &dirty,
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    assert_eq!(plan.freshness, Freshness::Fresh);
    assert_eq!(plan.solved.map(|s| s.len()), Some(design.tiers.len()));
}

#[test]
fn no_previous_at_all_also_falls_back_to_a_full_solve() {
    let design = free_design();
    let plan = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    assert_eq!(plan.freshness, Freshness::Fresh);
    assert!(plan.solved.is_some());
}

/// A `Some` tier cutoff must actually shrink the drawn arrangement (via
/// `Design::planes_through_tier`) relative to the same design's uncut
/// `plan_preview` result, not just pass through as a no-op.
#[test]
fn tier_cutoff_truncates_the_drawn_plane_arrangement() {
    let design = free_design();
    let full = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    let truncated = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        Some(0),
    );
    assert_eq!(full.freshness, Freshness::Fresh);
    assert_eq!(truncated.freshness, Freshness::Fresh);
    assert!(
        truncated.planes.len() < full.planes.len(),
        "cutting off after tier 0 must drop tier 1's own facet(s) from the drawn \
             arrangement: full={}, truncated={}",
        full.planes.len(),
        truncated.planes.len()
    );
}

/// `limit_visible_tiers` names the flat tiers whose slices the drawn planes keep, and
/// says nothing (`None`) for the finished stone.
#[test]
fn limit_visible_tiers_names_the_tiers_a_cut_keeps() {
    let design = free_design();
    assert_eq!(limit_visible_tiers(&design, CutLimit::Finished), None);
    assert_eq!(
        limit_visible_tiers(&design, CutLimit::ThroughTier(0)),
        Some(vec![true, false])
    );
    assert_eq!(
        limit_visible_tiers(&design, CutLimit::Steps(0)),
        Some(vec![false, false]),
        "the rough has no tier"
    );
    assert_eq!(
        limit_visible_tiers(&design, CutLimit::Steps(1)),
        Some(vec![true, false])
    );
    assert_eq!(
        limit_visible_tiers(&design, CutLimit::Steps(design.preview_step_count())),
        None,
        "the last step is the finished stone"
    );
    assert_eq!(limit_visible_tiers(&design, CutLimit::Steps(99)), None);
}

/// The restriction agrees with the planes actually drawn: the count of facet planes
/// a cut keeps is the preform's plus the visible tiers' own.
#[test]
fn the_visible_tiers_account_for_the_drawn_plane_count() {
    let design = free_design();
    let solved = design.solve().expect("fixture must solve");
    let preform = design.preform.planes().len();
    let full = planes_for_display(&design, &solved, CutLimit::Finished).len();
    let one_step = planes_for_display(&design, &solved, CutLimit::Steps(1)).len();
    let rough = planes_for_display(&design, &solved, CutLimit::Steps(0)).len();
    assert_eq!(rough, preform);
    let flags = limit_visible_tiers(&design, CutLimit::Steps(1)).expect("a partial cut");
    let map = crate::facet_map::FacetMap::from_design_cut(&design, &solved, &[], Some(&flags));
    assert_eq!(map.facet_count(), one_step);
    assert!(one_step > rough && one_step < full);
}

/// A cutoff at or past the last tier index must reproduce the full arrangement
/// exactly -- `Design::planes_through_tier`'s own documented "every tier"
/// equivalence for `through_tier >= tiers.len()`, but exercised here through
/// `plan_preview`'s own entry point rather than the core function directly.
/// Uses `design.tiers.len()` itself (not `usize::MAX`) -- `planes_through_tier`
/// computes `through_tier + 1` internally, which would overflow for `MAX`.
#[test]
fn tier_cutoff_past_the_last_tier_matches_the_full_arrangement() {
    let design = free_design();
    let full = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    let uncut = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        Some(design.tiers.len()),
    );
    assert_eq!(full.planes.len(), uncut.planes.len());
}

/// A design stored crown-first, with a pavilion groove: the slider must follow
/// cutting order for the planes as well as the tools. Stored-prefix truncation kept
/// only the crown tier's planes, so the pavilion groove was carved into a stone with
/// no pavilion.
#[test]
fn concave_tier_cutoff_follows_cutting_order_for_the_planes() {
    let mut design = Design::concave_fixture();
    // Stored: C40, C32, P-38, P-42, G90; cut: P-38, P-42, G90, (Groove), C40, C32, ...
    design.tiers.reverse();
    let uncut = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        None,
    );
    let cut = plan_preview(
        &design,
        None,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        &PanicSolver,
        &ZeroClock,
        Some(0),
    );
    assert!(cut.solved.is_some());
    assert_eq!(
        visible_flat_tiers(&design, 0),
        [true, false, true, true, true],
        "everything cut before the stored-first crown tier, and that tier"
    );
    // Only the second crown tier (stored 1, four facets) is hidden.
    assert_eq!(cut.planes.len() + 4, uncut.planes.len());
}

/// A design whose only tiers are free, so there is no anchor to solve from.
fn unsolvable_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        vec![
            tier("C1", 30.0, MeetConstraint::MeetExisting),
            tier("C2", 40.0, MeetConstraint::MeetExisting),
        ],
    )
}

/// One mast, standing in for a `last_solved` that predates an add or remove.
fn one_stale_mast() -> Vec<SolvedTier> {
    vec![SolvedTier {
        mast: 0.5,
        strategy: SolveStrategy::ScaleReference,
        detail: "stale".to_string(),
    }]
}

fn plan_with(
    design: &Design,
    last_solved: Option<&[SolvedTier]>,
    limit: CutLimit,
    solver: &dyn DirtySolver,
) -> PreviewPlan {
    plan_preview_limited(
        design,
        last_solved,
        &BTreeSet::new(),
        DEFAULT_PREVIEW_BUDGET,
        solver,
        &ZeroClock,
        limit,
    )
}

/// The freeze behind "the view stops following the Cut slider": a design that does not
/// solve, planned against a `last_solved` of the wrong length (an add or remove), fed
/// that list to `planes_through_tier`, which panics -- on the plan worker, whose death
/// stopped every later replan. It must draw nothing (the renderer keeps its last stone)
/// and report why, for every limit.
#[test]
fn an_unsolvable_design_with_misaligned_masts_does_not_panic() {
    let design = unsolvable_design();
    let stale = one_stale_mast();
    for limit in [
        CutLimit::Finished,
        CutLimit::ThroughTier(0),
        CutLimit::ThroughTier(5),
        CutLimit::Steps(1),
        CutLimit::Steps(2),
        CutLimit::Steps(99),
    ] {
        let plan = plan_with(&design, Some(&stale), limit, &PanicSolver);
        assert!(
            matches!(plan.freshness, Freshness::Unsolvable(_)),
            "{limit:?} must report the solve failure"
        );
        assert!(plan.planes.is_empty(), "{limit:?} has no masts to draw");
        assert!(plan.solved.is_none());
    }
    let none = plan_with(&design, None, CutLimit::Steps(1), &PanicSolver);
    assert!(none.planes.is_empty(), "no last solve: nothing to draw");
}

/// The rough needs no masts, so it is drawn even when the design does not solve.
#[test]
fn the_rough_is_drawn_even_when_the_design_does_not_solve() {
    let design = unsolvable_design();
    let plan = plan_with(&design, None, CutLimit::Steps(0), &PanicSolver);
    assert!(matches!(plan.freshness, Freshness::Unsolvable(_)));
    assert_eq!(plan.planes.len(), design.preform.planes().len());
}

#[test]
fn the_rough_draws_the_preform_alone_and_still_chains_every_mast() {
    let design = free_design();
    let plan = plan_with(&design, None, CutLimit::Steps(0), &PanicSolver);
    assert_eq!(plan.freshness, Freshness::Fresh);
    assert_eq!(plan.planes.len(), design.preform.planes().len());
    assert_eq!(
        plan.solved.map(|masts| masts.len()),
        Some(design.tiers.len()),
        "the next edit diffs against all the masts, not the shown ones"
    );
}

#[test]
fn steps_count_whole_tiers_and_the_last_step_is_the_finished_stone() {
    let design = free_design();
    let finished = plan_with(&design, None, CutLimit::Finished, &PanicSolver);
    let one = plan_with(&design, None, CutLimit::Steps(1), &PanicSolver);
    let all = plan_with(
        &design,
        None,
        CutLimit::Steps(design.preview_step_count()),
        &PanicSolver,
    );
    let rough = design.preform.planes().len();
    assert_eq!(one.planes.len(), rough + 1, "one tier, one facet plane");
    assert!(one.planes.len() < finished.planes.len());
    assert_eq!(all.planes, finished.planes);
    let through_first = plan_with(&design, None, CutLimit::ThroughTier(0), &PanicSolver);
    assert_eq!(
        one.planes, through_first.planes,
        "on a planar design one step is 'through tier 0'"
    );
}

/// The dirty-subgraph branch (aligned masts, a real `resolve_dirty`) must honour the
/// step limit too, not only the full-solve branch.
#[test]
fn a_step_limit_applies_on_the_dirty_subgraph_branch_too() {
    let design = free_design();
    let previous = design.solve().expect("fixture must solve");
    let solver = InstantSolver {
        result: previous.clone(),
    };
    let plan = plan_with(&design, Some(&previous), CutLimit::Steps(1), &solver);
    assert_eq!(plan.freshness, Freshness::Fresh);
    assert_eq!(plan.planes.len(), design.preform.planes().len() + 1);
    assert_eq!(plan.solved.map(|masts| masts.len()), Some(2));
}

#[test]
fn display_geometry_of_a_planar_design_has_no_tools_and_follows_the_limit() {
    let design = free_design();
    let solved = design.solve().expect("fixture must solve");
    let finished = display_geometry(&design, &solved, CutLimit::Finished);
    assert_eq!(finished.planes, design.planes_from_solved(&solved));
    assert!(finished.tools.is_empty() && finished.placements.is_empty());
    let rough = display_geometry(&design, &solved, CutLimit::Steps(0));
    assert_eq!(rough.planes.len(), design.preform.planes().len());
    let one = display_geometry(&design, &solved, CutLimit::Steps(1));
    assert_eq!(one.planes.len(), rough.planes.len() + 1);
}

#[test]
fn display_geometry_never_panics_on_misaligned_masts() {
    let design = free_design();
    let stale = one_stale_mast();
    for limit in [
        CutLimit::Finished,
        CutLimit::ThroughTier(0),
        CutLimit::Steps(1),
    ] {
        let geometry = display_geometry(&design, &stale, limit);
        assert!(geometry.planes.is_empty(), "{limit:?}");
        assert!(geometry.tools.is_empty(), "{limit:?}");
    }
    let rough = display_geometry(&design, &stale, CutLimit::Steps(0));
    assert_eq!(rough.planes.len(), design.preform.planes().len());
}

/// Planes and tools of one step come from the same cut: the groove (cut after the
/// pavilion and girdle tiers) appears at the step that cuts it, adds no plane, and the
/// finished stone carries both tool kinds.
#[test]
fn display_geometry_pairs_each_step_s_tools_with_its_planes() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture solves");
    let tools_after = |steps: usize| display_geometry(&design, &solved, CutLimit::Steps(steps));
    assert!(tools_after(0).tools.is_empty(), "the rough has no tools");
    assert!(tools_after(3).tools.is_empty(), "the groove is step 4");
    let grooved = tools_after(4);
    assert_eq!(grooved.tools.len(), 8);
    assert_eq!(grooved.planes, tools_after(3).planes);
    assert_eq!(grooved.placements.len(), grooved.tools.len());
    assert_eq!(tools_after(7).tools.len(), 12);
    let finished = display_geometry(&design, &solved, CutLimit::Finished);
    assert_eq!(finished.tools.len(), 12);
    assert_eq!(finished.planes, tools_after(7).planes);
}

/// `indicatrix-cut-core`'s "CrackOtto-Step" fixture (PC 05.115, 103 tiers), re-authored
/// as a full [`Design`] rather than raw planes: every tier is implicit
/// `MeetExisting`, so one `ScaleReference` anchor is bootstrapped per
/// crown/pavilion/girdle block from that tier's real recorded mast, leaving the
/// rest genuinely free.
///
/// `#[cfg(not(target_arch = "wasm32"))]`: its only caller
/// (`timing_resolve_dirty_on_crackotto_step_103_tier`) is gated the same way.
#[cfg(not(target_arch = "wasm32"))]
const CRACKOTTO_STEP_ASC: &str =
    include_str!("../../../indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc");

#[cfg(not(target_arch = "wasm32"))]
fn crackotto_step_design() -> Design {
    let schedule =
        indicatrix_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

/// `std::time::Instant`-based perf measurement, not a correctness check --
/// `#[cfg(not(target_arch = "wasm32"))]` (on top of `#[ignore]`) rather than an
/// injected [`Clock`], since this one measures `Design::resolve_dirty` directly
/// (bypassing `plan_preview`) and a canned `Clock` would defeat the point of a
/// real timing. See the crate README for why `wasm32-unknown-unknown` must
/// never see `Instant::now` even in a test that never runs there.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture"]
fn timing_resolve_dirty_on_crackotto_step_103_tier() {
    let design = crackotto_step_design();
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );
    let baseline = design.solve().expect("must solve to get a starting point");

    let mut edited = design;
    edited.tiers[20].angle_deg += 0.5;
    let dirty = BTreeSet::from([20]);

    let start = std::time::Instant::now();
    let result = edited
        .resolve_dirty(&baseline, &dirty)
        .expect("subgraph resolve");
    let elapsed = start.elapsed();
    assert_eq!(result.len(), 103);

    println!(
        "CrackOtto-Step (103 tiers): resolve_dirty for a single-tier edit: {elapsed:?} \
             (DEFAULT_PREVIEW_BUDGET = {DEFAULT_PREVIEW_BUDGET:?})"
    );
}

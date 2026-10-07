//! Tests for the finished stone: the pure decisions, the single-flight solve gate and the
//! export scene the stone is swapped into.

use super::*;
use crate::bridge::export_thread::SceneSnapshot;
use std::{
    sync::{Barrier, atomic::AtomicUsize},
    time::Duration,
};

fn round_brilliant() -> Design {
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// A render context holding `design` cut back to `steps`, the way the editor leaves it.
fn context_cut_back(design: &Design, steps: usize) -> Mutex<RenderContext> {
    let solved = design.solve().expect("every tier is pinned");
    let cut = cut_slider::cut_geometry(design, Some(&solved), Some(steps));
    Mutex::new(RenderContext {
        active_planes: Arc::new(cut.planes),
        active_tools: Arc::new(cut.tools),
        planes_owner: PlanesOwner::Editor { generation: 1 },
        ..Default::default()
    })
}

#[test]
fn only_the_editor_owning_the_viewport_shows_the_cut() {
    assert!(shows_editor_design(PlanesOwner::Editor { generation: 4 }));
    assert!(!shows_editor_design(PlanesOwner::Builtin));
    assert!(!shows_editor_design(PlanesOwner::Catalogue { entry_id: 9 }));
}

/// A one-tier design that cannot be solved: its only tier meets nothing.
fn unsolvable() -> Design {
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        vec![ConstraintTier {
            angle_deg: 30.0,
            name: "C1".to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }],
    )
}

/// The editor's shared solve cache holding `solved` as the masts of `generation`.
fn cache_holding(generation: u64, solved: Vec<SolvedTier>) -> SolidLastSolved {
    Arc::new(Mutex::new(Some((generation, solved))))
}

fn empty_cache() -> SolidLastSolved {
    Arc::new(Mutex::new(None))
}

/// The generation the cache is stored for, `None` when it is empty.
fn stored_for(cache: &SolidLastSolved) -> Option<u64> {
    cache
        .lock()
        .unwrap()
        .as_ref()
        .map(|(generation, _)| *generation)
}

/// The stone a job builds from the masts it already holds (the UI thread's path).
fn held_stone(job: &FinishedStoneJob) -> Option<StoneGeometryBuf> {
    assert!(job.holds_masts(), "the job holds masts");
    job.stone_from_held_masts()
}

/// The finished stone for `design` cut back to `steps`, with its masts in the cache.
fn finished_when_cut(
    design: &Design,
    solved: &[SolvedTier],
    steps: usize,
) -> Option<StoneGeometryBuf> {
    let job = FinishedStoneJob::when_cut(
        design,
        1,
        Some(steps),
        Some(cache_holding(1, solved.to_vec())),
    )
    .expect("the slider is cut back");
    held_stone(&job)
}

#[test]
fn nothing_is_substituted_while_the_slider_is_at_finished() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    assert!(FinishedStoneJob::when_cut(&design, 1, None, None).is_none());
    assert!(FinishedStoneJob::when_cut(&design, 1, None, Some(cache_holding(1, solved))).is_none());
}

#[test]
fn a_cut_back_stone_is_replaced_by_the_whole_design() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    for steps in [0, 1, 3, design.preview_step_count() - 1] {
        let finished =
            finished_when_cut(&design, &solved, steps).expect("a cut stone has a finished one");
        assert_eq!(finished, whole, "cut at {steps} steps");
        let cut = cut_slider::cut_geometry(&design, Some(&solved), Some(steps));
        assert!(
            finished.planes.len() > cut.planes.len(),
            "cut at {steps} steps is smaller than the finished stone"
        );
    }
}

/// F4-8: a design that does not solve is refused with the solver's sentence. It is never
/// the render context's half-cut stone in disguise (`Ok(None)`), and never an empty stone.
#[test]
fn a_design_that_does_not_solve_is_refused_not_exported_half_cut() {
    let job = FinishedStoneJob::when_cut(&unsolvable(), 1, Some(1), Some(empty_cache()))
        .expect("the slider is cut back");
    assert!(!job.holds_masts(), "nothing is cached");
    let refused = job
        .resolve()
        .expect_err("an unsolvable design has no stone");
    assert!(
        matches!(&refused, Withheld::Unsolvable(reason) if !reason.is_empty()),
        "{refused:?}"
    );
    let sentence = refused.export_message();
    assert!(sentence.starts_with("Nothing was exported"), "{sentence}");
    assert!(sentence.contains("does not solve"), "{sentence}");
}

/// The solve cache is trusted for exactly the generation it was stored for, and only
/// while it still lines up with the design's tiers.
#[test]
fn the_cached_masts_are_used_only_for_the_exact_generation() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let cached = (5, solved.clone());
    assert_eq!(
        masts_for_generation(Some(&cached), 5, &design).map(|masts| masts.len()),
        Some(design.tiers.len())
    );
    assert!(
        masts_for_generation(Some(&cached), 6, &design).is_none(),
        "one edit later"
    );
    assert!(
        masts_for_generation(Some(&cached), 4, &design).is_none(),
        "one edit earlier"
    );
    assert!(masts_for_generation(None, 5, &design).is_none());
    let shorter = (5, solved[1..].to_vec());
    assert!(
        masts_for_generation(Some(&shorter), 5, &design).is_none(),
        "a mast list of another length"
    );
}

/// The point of the whole module: with the design's current masts in the cache the job
/// never solves. The design here cannot be solved at all, so a solve would be refused;
/// getting a stone proves the cached masts were used.
#[test]
fn a_job_with_current_masts_does_not_solve() {
    let masts = round_brilliant().solve().expect("every tier is pinned");
    let design = unsolvable();
    assert_eq!(design.tiers.len(), 1);
    let cache = cache_holding(9, vec![masts[0].clone()]);
    let job = FinishedStoneJob::when_cut(&design, 9, Some(1), Some(cache))
        .expect("the slider is cut back");
    assert!(job.holds_masts());
    assert!(job.resolve().is_ok(), "built from the cached masts");
}

/// Without current masts the job solves (on its own thread, never the UI's) and leaves
/// the masts in the cache, so the next job for the same generation does not.
#[test]
fn a_job_without_current_masts_solves_once_and_remembers_the_masts() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);

    // Nothing cached, and a solve for the generation before: both are misses.
    for cache in [empty_cache(), cache_holding(6, solved)] {
        let job = FinishedStoneJob::when_cut(&design, 7, Some(3), Some(cache.clone()))
            .expect("the slider is cut back");
        assert!(!job.holds_masts());
        assert_eq!(job.resolve(), Ok(whole.clone()));
        assert_eq!(stored_for(&cache), Some(7), "the solve is remembered");

        let next = FinishedStoneJob::when_cut(&design, 7, Some(2), Some(cache))
            .expect("the slider is cut back");
        assert_eq!(held_stone(&next), Some(whole.clone()), "no second solve");
    }
}

/// A slow solve for an older generation must not overwrite what a later edit stored.
#[test]
fn a_remembered_solve_never_replaces_a_newer_one() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let cache = cache_holding(8, solved.clone());
    remember_masts(&cache, 7, solved.clone());
    assert_eq!(stored_for(&cache), Some(8));
    remember_masts(&cache, 8, solved.clone());
    assert_eq!(stored_for(&cache), Some(8));
    remember_masts(&cache, 9, solved);
    assert_eq!(stored_for(&cache), Some(9));
}

/// The cache can be filled after the job was made (a background solve landing while a
/// hover preview waits out its debounce): `resolve` reads it again before solving.
#[test]
fn a_job_reads_the_cache_again_before_it_solves() {
    let masts = round_brilliant().solve().expect("every tier is pinned");
    let design = unsolvable();
    let cache = empty_cache();
    let job = FinishedStoneJob::when_cut(&design, 3, Some(1), Some(cache.clone()))
        .expect("the slider is cut back");
    assert!(!job.holds_masts());
    // The unsolvable design would be refused if the job solved it itself.
    *cache.lock().unwrap() = Some((3, vec![masts[0].clone()]));
    assert!(job.resolve().is_ok());
}

/// F4-7: hovers that find the cache cold at the same moment run ONE solve between them;
/// the others wait for it and read its masts. Without the gate every one of them would
/// solve the whole design at once.
#[test]
fn concurrent_jobs_for_one_generation_solve_once() {
    const JOBS: usize = 6;
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    let cache = empty_cache();
    let solves = AtomicUsize::new(0);
    let barrier = Barrier::new(JOBS);
    let results = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        for _ in 0..JOBS {
            let job = FinishedStoneJob::when_cut(&design, 7, Some(3), Some(cache.clone()))
                .expect("the slider is cut back");
            let (solves, barrier, results) = (&solves, &barrier, &results);
            scope.spawn(move || {
                // All of them hold a cold-cache job before any starts to resolve.
                barrier.wait();
                let stone = job.resolve_with(|design| {
                    solves.fetch_add(1, Ordering::SeqCst);
                    // Long enough that every other job is waiting at the gate.
                    std::thread::sleep(Duration::from_millis(60));
                    solve_design(design)
                });
                results.lock().unwrap().push(stone);
            });
        }
    });

    assert_eq!(solves.load(Ordering::SeqCst), 1, "one solve in all");
    let results = results.into_inner().unwrap();
    assert_eq!(results.len(), JOBS);
    assert!(
        results.iter().all(|stone| stone.as_ref() == Ok(&whole)),
        "every job gets the finished stone"
    );
    assert_eq!(stored_for(&cache), Some(7));
}

#[test]
fn a_job_is_stale_once_the_editor_has_moved_past_its_generation() {
    assert!(!is_stale(7, Some(7)));
    assert!(is_stale(7, Some(8)), "one edit later");
    assert!(
        is_stale(7, Some(6)),
        "an editor that went back (a new design)"
    );
    assert!(!is_stale(7, None), "no live counter to compare with");

    let design = round_brilliant();
    let live = Arc::new(AtomicU64::new(7));
    let job = FinishedStoneJob::when_cut(&design, 7, Some(3), None)
        .expect("the slider is cut back")
        .following(Arc::clone(&live));
    assert!(!job.is_stale());
    live.fetch_add(1, Ordering::Relaxed);
    assert!(job.is_stale());
}

/// A job whose design the editor has already left never solves: its masts would warm the
/// cache for a generation nobody asks for, and its stone would be shown as current.
#[test]
fn a_stale_job_does_not_solve_and_is_withheld() {
    let design = round_brilliant();
    let live = Arc::new(AtomicU64::new(9));
    let job = FinishedStoneJob::when_cut(&design, 7, Some(3), Some(empty_cache()))
        .expect("the slider is cut back")
        .following(live);
    let result = job.resolve_with(|_| panic!("a stale job must not solve"));
    assert_eq!(result, Err(Withheld::Stale));
}

/// The editor moves on while the solve runs: what comes back is still the design the job
/// was made for, so it is withheld, and its masts are filed for the generation they describe.
#[test]
fn a_solve_that_lands_after_the_design_changed_is_withheld_but_remembered() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let cache = empty_cache();

    // Still the same design: delivered.
    let delivered = landed(&design, 7, Some(7), Ok(solved.clone()), |masts| {
        remember_masts(&cache, 7, masts);
    });
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    assert_eq!(delivered, Ok(Some(whole)));
    assert_eq!(stored_for(&cache), Some(7));

    // One edit later: withheld, whatever the result says.
    let later = empty_cache();
    let withheld = landed(&design, 7, Some(8), Ok(solved), |masts| {
        remember_masts(&later, 7, masts);
    });
    assert_eq!(withheld, Err(Withheld::Stale));
    assert_eq!(stored_for(&later), Some(7), "right for generation 7");
}

/// A failure is the design's own only while the editor still has that design; a displaced
/// solve says nothing about the geometry, and nothing about the design having changed.
#[test]
fn a_failed_solve_is_unsolvable_only_for_the_current_design() {
    let design = round_brilliant();
    let failed = || Err(SolveFailure::Failed("tier C1 meets nothing".to_string()));
    assert_eq!(
        landed(&design, 7, Some(7), failed(), |_| {}),
        Err(Withheld::Unsolvable("tier C1 meets nothing".to_string()))
    );
    assert_eq!(
        landed(&design, 7, Some(8), failed(), |_| {}),
        Err(Withheld::Stale),
        "the design that failed is not the design the cutter has now"
    );
    // No live counter to compare with: the failure stands.
    assert_eq!(
        landed(&design, 7, None, failed(), |_| {}),
        Err(Withheld::Unsolvable("tier C1 meets nothing".to_string()))
    );
}

/// F4-13: a solve the worker displaced for a newer request, for the design the editor still
/// has, is `Displaced`: it is not "the design changed", which would be false. Once the
/// design really has moved on it is `Stale`, as any other result for the old design is.
#[test]
fn a_displaced_solve_is_not_reported_as_the_design_changing() {
    let design = round_brilliant();
    assert_eq!(
        landed(&design, 7, Some(7), Err(SolveFailure::Superseded), |_| {}),
        Err(Withheld::Displaced)
    );
    assert_eq!(
        landed(&design, 7, None, Err(SolveFailure::Superseded), |_| {}),
        Err(Withheld::Displaced),
        "no live counter to compare with: nothing says the design moved"
    );
    assert_eq!(
        landed(&design, 7, Some(8), Err(SolveFailure::Superseded), |_| {}),
        Err(Withheld::Stale),
        "the design did change meanwhile"
    );
}

/// A stone with no facets is never delivered as "the finished stone".
#[test]
fn a_mast_list_that_draws_nothing_is_not_a_stone() {
    let design = round_brilliant();
    let mut masts = design.solve().expect("every tier is pinned");
    masts.truncate(1);
    assert_eq!(
        landed(&design, 1, Some(1), Ok(masts), |_| {}),
        Err(no_facets())
    );
}

#[test]
fn a_refused_export_says_why_in_plain_words() {
    let unsolvable = Withheld::Unsolvable("tier C1 meets nothing".to_string()).export_message();
    assert_eq!(
        unsolvable,
        "Nothing was exported: the design does not solve (tier C1 meets nothing)."
    );
    let stale = Withheld::Stale.export_message();
    assert!(stale.starts_with("Nothing was exported"), "{stale}");
    assert!(stale.ends_with("Start the export again."), "{stale}");
    // F4-13: the displaced solve's sentence names the interruption, not a change of design.
    let displaced = Withheld::Displaced.export_message();
    assert!(displaced.starts_with("Nothing was exported"), "{displaced}");
    assert!(displaced.contains("interrupted"), "{displaced}");
    assert!(!displaced.contains("design changed"), "{displaced}");
    assert!(
        displaced.ends_with("Start the export again."),
        "{displaced}"
    );
}

/// The export scene's geometry: a stone cut back to step 3 in the viewport is exported
/// whole, while the live capture (the remote dispatch) keeps following the slider.
#[test]
fn an_export_scene_holds_the_finished_stone_whatever_the_slider_says() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    let ctx = context_cut_back(&design, 3);
    let cut_planes = ctx.lock().unwrap().active_planes.to_vec();
    assert!(cut_planes.len() < whole.planes.len());

    let finished = finished_when_cut(&design, &solved, 3);
    let export = SceneSnapshot::capture_finished(&ctx, finished.as_ref()).expect("resolves");
    assert_eq!(export.active_planes, whole.planes);
    assert_eq!(export.tools, whole.tools);

    let live = SceneSnapshot::capture(&ctx).expect("resolves");
    assert_eq!(
        live.active_planes, cut_planes,
        "the live capture follows the cut"
    );

    // No substitute: the export is the context as it stands.
    let untouched = SceneSnapshot::capture_finished(&ctx, None).expect("resolves");
    assert_eq!(untouched.active_planes, cut_planes);
}

/// The same for a design with concave tiers: the groove's tools are in the export even
/// when the slider stops before the groove is cut.
#[test]
fn an_export_scene_carries_the_concave_tools_of_the_finished_stone() {
    let design = Design::concave_fixture();
    let solved = design.solve().expect("the fixture solves");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    assert_eq!(whole.tools.len(), 12);
    let ctx = context_cut_back(&design, 3);
    assert!(ctx.lock().unwrap().active_tools.is_empty());

    let finished = finished_when_cut(&design, &solved, 3);
    let export = SceneSnapshot::capture_finished(&ctx, finished.as_ref()).expect("resolves");
    assert_eq!(export.tools.len(), 12);
    assert_eq!(export.active_planes, whole.planes);
}

/// The frosted-girdle classification and the stone-width scale come from the planes the
/// export draws, not from the cut stone the viewport holds.
#[test]
fn an_export_classifies_the_girdle_of_the_finished_stone() {
    let design = round_brilliant();
    let solved = design.solve().expect("every tier is pinned");
    let whole = cut_slider::cut_geometry(&design, Some(&solved), None);
    let ctx = context_cut_back(&design, 3);
    ctx.lock().unwrap().girdle_frosted = true;

    let finished = finished_when_cut(&design, &solved, 3);
    let export = SceneSnapshot::capture_finished(&ctx, finished.as_ref()).expect("resolves");
    assert_eq!(export.facet_finishes.len(), whole.planes.len());
    assert_eq!(
        export.facet_finishes,
        indicatrix::geometry::girdle_facet_finishes(&whole.planes)
    );
}

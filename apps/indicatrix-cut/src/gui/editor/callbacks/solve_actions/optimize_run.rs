//! "Optimize"'s dispatch: builds the run from the Optimize tab (objective, what may
//! change, angle ranges, budget, seed, candidates), builds its material prelude, and
//! spawns the off-thread search.

use super::{OPTIMIZE_ACTIVITY_ID, RunProvenance, optimize_outcome::handle_optimize_outcome};
use crate::{
    EditorModel, MainWindow, OptimizeChangeRow, OptimizeModel, OptimizeResultRow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            optimize_panel::{self, WeightBoxes},
            optimize_solve::{self, OptimizeRunRequest},
            stall_guard::stall_guard,
            state::EditorState,
        },
        show_toast,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, MaterialSelection, OptimizeOutcome};
// The RI defaulting, the "only selected tiers" pinning and the start status line moved to
// `indicatrix_editor::optimize_view` (shared with the web Optimize tab); imported at
// their old names, which this module's tests exercise through `super::*`.
use indicatrix_editor::optimize_view::{
    default_optimize_material_ri, optimize_start_status, pin_non_selected_free_tiers,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::Instant,
};

/// "Optimize": the explicit, off-thread, cancellable search over the design's tier angles --
/// see `optimize_solve`'s module doc comment for the threading/cancellation/progress
/// machinery. Guarded by `editor_optimize_running` (not `EditorState::optimize` being
/// `Some`), the same non-`Send` completion-handler reasoning
/// [`super::deep_solve_run::setup_deep_solve_callback`] documents.
///
/// Unlike Deep Solve, a completed/cancelled run's result is not merely displayed: its
/// candidates are kept by `optimize_panel` and the picked one is stashed in
/// `EditorState::pending_optimize` (paired with the design generation it ran against) so
/// [`super::optimize_outcome::setup_optimize_apply_callback`] can commit it later -- this
/// callback itself never touches `design`/`history`.
///
/// `run_epoch` guards against the same superseded-run race
/// [`super::deep_solve_run::setup_deep_solve_callback`] documents -- narrower a window here (Optimize's own
/// cancellation is a real mid-search checkpoint, typically resolving within
/// [`optimize_solve`]'s own documented "milliseconds to a few seconds"), but a
/// cancel-then-immediately-restart click is still possible, and its stale `on_done`
/// would otherwise be free to overwrite a genuinely running new search's live status
/// or stash the WRONG result into `pending_optimize`.
///
/// The five arguments are the weight boxes, the yield slider and the signed tone slider; the
/// command bar's Optimize button and the tab's own pass them, and a preset other than
/// "Custom" ignores them.
///
/// When `design.material` names no preset and carries no RI override (a brand-new
/// design, or an untouched `.asc` import), this resolves a `refractive_index_override`
/// from [`indicatrix_cut_core::design::Design::effective_refractive_index`] before
/// handing the selection to the worker -- otherwise
/// `MaterialSelection::resolve`/`resolved_gem_material` silently fall back to
/// diamond (`n_D` 2.42), scoring the search against the wrong RI with nothing on
/// screen saying so (see the "silently scores against Diamond" finding).
pub(in crate::gui::editor) fn setup_optimize_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>().on_optimize(
        move |windowing: SharedString,
              extinction: SharedString,
              tilt_brilliance: SharedString,
              yield_weight: f32,
              tone: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            stall_guard("on_optimize", || {
                begin_tier_optimize_run(
                    &ui,
                    &state,
                    &render_ctx,
                    &run_epoch,
                    WeightBoxes {
                        windowing: &windowing,
                        extinction: &extinction,
                        tilt_brilliance: &tilt_brilliance,
                        yield_weight,
                        tone,
                    },
                );
            });
        },
    );
}

/// Everything [`begin_tier_optimize_run`] reads from the editor state before it
/// dispatches the worker, beyond the target `design` and the plan itself. It is its
/// own struct, built by [`prepare_optimize_run`], to keep that function under clippy's
/// function-length lint.
struct OptimizeRunPrep {
    /// The design's material, with an RI filled in when it names no preset.
    material_selection: MaterialSelection,
    /// The catalogue materials the worker resolves `material_selection` against: its
    /// own owned copy.
    custom_materials: Vec<GemMaterial>,
    /// The RI that was filled in, if any, for the start status line.
    defaulted_ri: Option<f64>,
    /// The design generation and epoch the run started against, so the completion
    /// handler can tell a stale or replaced design.
    provenance: RunProvenance,
    /// The slot the outcome waits in until the cutter picks a candidate and applies it.
    pending_optimize: Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    /// This run's number from `run_epoch`.
    this_run: u64,
    /// The shared epoch counter; a finishing run whose `this_run` is no longer the
    /// latest has been superseded.
    run_epoch_done: Arc<AtomicU64>,
}

/// [`begin_tier_optimize_run`]'s prelude -- see [`OptimizeRunPrep`]'s own doc
/// comment. `st` is the already-borrowed `EditorState` (read-only: nothing here
/// mutates it); `run_epoch` is bumped here, the one side effect this otherwise
/// pure capture has.
fn prepare_optimize_run(
    render_ctx: &Arc<Mutex<RenderContext>>,
    st: &EditorState,
    run_epoch: &Arc<AtomicU64>,
) -> OptimizeRunPrep {
    let mut material_selection = st.design.material.clone();
    // `OptimizeRunRequest::custom_materials` (see `optimize_solve::spawn_optimize_run`)
    // is a plain `Vec` -- a one-shot worker's own owned copy, not
    // `RenderContext`'s hot-path per-frame snapshot -- so this is the one actual
    // deep copy on this path, same as before `RenderContext::custom_materials`
    // became `Arc`-backed. Fetched before `default_optimize_material_ri` below
    // (moved up from its own original spot) so that default can resolve a CUSTOM
    // catalogue material's own RI too, not just a built-in's.
    let custom_materials = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let defaulted_ri =
        default_optimize_material_ri(&st.design, &mut material_selection, &custom_materials);
    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    OptimizeRunPrep {
        material_selection,
        custom_materials,
        defaulted_ri,
        provenance: RunProvenance::capture(st),
        pending_optimize: Arc::clone(&st.pending_optimize),
        this_run,
        run_epoch_done: Arc::clone(run_epoch),
    }
}

/// The share of the search's evaluation budget done, `0.0` to `1.0`, for the progress bar.
fn progress_fraction(progress: &optimize_solve::OptimizeSolveProgress) -> f32 {
    (progress.evaluations as f32 / progress.max_evaluations.max(1) as f32).clamp(0.0, 1.0)
}

/// The panel the moment a run starts: busy, the start status, and nothing left of the
/// previous run -- its candidates may not be applied or compared while the search is
/// going, and its pending outcome is dropped with them.
fn mark_run_started(ui: &MainWindow, st: &EditorState, defaulted_ri: Option<f64>) {
    let editor = ui.global::<EditorModel>();
    editor.set_optimize_running(true);
    editor.set_optimize_status(optimize_start_status(defaulted_ri).into());
    editor.set_optimize_status_is_problem(false);
    editor.set_optimize_can_apply(false);
    editor.set_optimize_result_rows(ModelRc::new(
        VecModel::from(Vec::<OptimizeResultRow>::new()),
    ));
    editor.set_optimize_change_rows(ModelRc::new(
        VecModel::from(Vec::<OptimizeChangeRow>::new()),
    ));
    optimize_panel::clear_results(ui);
    *st.pending_optimize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Starts one Optimize run from the Optimize tab: reads the tab, marks the panel busy,
/// registers the status-strip activity and spawns the off-thread search. The run's
/// outcome goes to [`handle_optimize_outcome`]; `run_epoch` is explained on
/// [`setup_optimize_callback`].
fn begin_tier_optimize_run(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    run_epoch: &Arc<AtomicU64>,
    boxes: WeightBoxes<'_>,
) {
    // Also guards against Solve or Deep Solve already running -- see
    // `super::deep_solve_run::setup_deep_solve_callback`'s doc comment for why.
    let model = ui.global::<EditorModel>();
    if model.get_optimize_running() || model.get_solve_running() || model.get_deep_solve_running() {
        return;
    }
    let mut st = state.borrow_mut();
    // The tab's whole state in one read: objective, what may change, the ranges, budget,
    // seed, candidates. A field that cannot be read is refused by name before anything runs.
    let plan = match optimize_panel::plan_from_ui(ui, &st.design, boxes) {
        Ok(plan) => plan,
        Err(message) => {
            show_toast(ui, &message, "error");
            return;
        }
    };
    if plan.movable_tiers == 0 {
        // `EditorView` only shows this button enabled when `optimize_available`
        // is true -- this only guards a race with a concurrent edit disabling
        // it out from under a stale click, not the common path.
        show_toast(
            ui,
            "Optimize has no tier it may change right now. Tick \"Vary anchored tiers\" in the \
             Optimize tab, or adopt a tier first.",
            "error",
        );
        return;
    }
    // A second snapshot for the completion closure: the per-tier change
    // table needs the ORIGINAL design's tiers/names to label its rows,
    // regardless of whichever design the search actually ran against below.
    let design_snapshot = st.design.clone();
    // When `EditorModel.optimize_only_selected` is on and at
    // least one tier is multi-selected, every OTHER free tier is pinned to
    // its own current mast before the search ever sees it, so the free tiers
    // are only the ones the cutter actually asked it to touch. `AngleChange::index`
    // stays valid against `design_snapshot`/the real `st.design` either
    // way -- see `pin_non_selected_free_tiers`'s own doc comment. The same selection
    // limits which anchored tiers may vary.
    let only_selected = ui.global::<EditorModel>().get_optimize_only_selected();
    let Some(design) = optimize_target_design(ui, &st, only_selected) else {
        return;
    };
    let only_tiers =
        (only_selected && !st.multi_selected.is_empty()).then(|| st.multi_selected.clone());
    let OptimizeRunPrep {
        material_selection,
        custom_materials,
        defaulted_ri,
        provenance,
        pending_optimize,
        this_run,
        run_epoch_done,
    } = prepare_optimize_run(render_ctx, &st, run_epoch);

    mark_run_started(ui, &st, defaulted_ri);

    // See `super::deep_solve_run::begin_deep_solve_run`'s matching comment for why this
    // reaches the shared registry through `auto_solve::activity()` rather than a new
    // parameter, and why `cancel` reaches back through `state` rather than `handle`
    // (which does not exist yet at this point).
    let activity = auto_solve::activity();
    let activity_id = activity.as_ref().map(|a| {
        a.start(
            "optimize",
            "Optimize",
            Some({
                let state = Rc::clone(state);
                Box::new(move || {
                    if let Some(handle) = state.borrow().optimize.as_ref() {
                        handle.cancel();
                    }
                })
            }),
        )
    });
    if activity_id.is_some() {
        OPTIMIZE_ACTIVITY_ID.with(|cell| *cell.borrow_mut() = activity_id);
    }

    let started = Instant::now();
    let ui_weak = ui.as_weak();
    let handle = optimize_solve::spawn_optimize_run(
        ui_weak,
        OptimizeRunRequest {
            design,
            material_selection,
            custom_materials,
            config: plan.config,
            options: plan.options,
            only_tiers,
        },
        |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
            ui.global::<EditorModel>()
                .set_optimize_status(optimize_progress_status(&progress).into());
            ui.global::<OptimizeModel>()
                .set_progress(progress_fraction(&progress));
        },
        move |ui: &MainWindow, outcome: optimize_solve::OptimizeRunOutcome| {
            // This run is finishing on its own -- see
            // `super::deep_solve_run::begin_deep_solve_run`'s matching comment for why an
            // already-finished id is a harmless no-op. Fetched on the UI thread, not
            // captured -- this closure must be `Send`.
            if let (Some(activity), Some(id)) = (auto_solve::activity(), activity_id) {
                activity.finish(id);
            }
            OPTIMIZE_ACTIVITY_ID.with(|cell| {
                if *cell.borrow() == activity_id {
                    *cell.borrow_mut() = None;
                }
            });
            // A superseded run -- see [`setup_optimize_callback`]'s doc comment on
            // `run_epoch`.
            if run_epoch_done.load(AtomicOrdering::Relaxed) != this_run {
                return;
            }
            handle_optimize_outcome(
                ui,
                outcome,
                &provenance,
                &pending_optimize,
                &design_snapshot,
                started.elapsed().as_secs_f32(),
            );
        },
    );
    st.optimize = Some(handle);
}

/// The design [`begin_tier_optimize_run`] actually hands to the search --
/// `st.design` unchanged, or (when multi-selection is active)
/// [`pin_non_selected_free_tiers`]'s restricted clone. Pulled out purely to keep
/// `begin_tier_optimize_run` under clippy's function-length lint.
///
/// Reuses the cached last solve, so the UI thread never solves synchronously here,
/// instead of calling [`Design::solve`] inline. Returns `None` (having already
/// toasted) rather than silently falling back to the unrestricted design, when no
/// cached solve matches this design's current tier count.
fn optimize_target_design(
    ui: &MainWindow,
    st: &EditorState,
    only_selected: bool,
) -> Option<Design> {
    if !only_selected || st.multi_selected.is_empty() {
        return Some(st.design.clone());
    }
    let Some(solved) = auto_solve::solid_last_solved()
        .and_then(|cache| {
            cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        })
        // the shared cache is now generation-tagged -- only the masts
        // themselves matter here.
        .filter(|(_, solved)| solved.len() == st.design.tiers.len())
        .map(|(_, solved)| solved)
    else {
        show_toast(
            ui,
            "Solve first, then Optimize with \"Only selected tiers\".",
            "error",
        );
        return None;
    };
    Some(pin_non_selected_free_tiers(
        &st.design,
        &solved,
        &st.multi_selected,
    ))
}

/// The running "Optimizing..." status line for each progress tick -- the wording lives in
/// `indicatrix_editor::optimize_view::optimize_progress_status` (shared with the web
/// Optimize tab); this only unpacks the desktop's progress struct.
fn optimize_progress_status(progress: &optimize_solve::OptimizeSolveProgress) -> String {
    indicatrix_editor::optimize_view::optimize_progress_status(
        progress.stage,
        progress.evaluations,
        progress.max_evaluations,
        progress.start,
        progress.elapsed.as_secs_f32(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::optimize::SearchStage;
    use std::{collections::BTreeSet, time::Duration};

    // --- progress_fraction ---

    fn progress(
        evaluations: usize,
        max_evaluations: usize,
    ) -> optimize_solve::OptimizeSolveProgress {
        optimize_solve::OptimizeSolveProgress {
            evaluations,
            max_evaluations,
            stage: SearchStage::Coordinate,
            start: None,
            elapsed: Duration::from_secs(1),
        }
    }

    #[test]
    fn the_progress_bar_is_the_share_of_the_budget_done() {
        assert!((progress_fraction(&progress(50, 200)) - 0.25).abs() < 1e-6);
        assert_eq!(progress_fraction(&progress(0, 200)), 0.0);
    }

    #[test]
    fn the_progress_bar_never_leaves_zero_to_one() {
        assert_eq!(progress_fraction(&progress(300, 200)), 1.0);
        assert_eq!(
            progress_fraction(&progress(5, 0)),
            1.0,
            "no budget known yet"
        );
        assert_eq!(progress_fraction(&progress(0, 0)), 0.0);
    }

    // --- default_optimize_material_ri (must resolve a CUSTOM catalogue
    // material's own RI, not just a built-in's) ---

    fn fixture_design() -> Design {
        let preform = indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
        Design::fresh(preform, 96, 8, 1.54)
    }

    #[test]
    fn default_optimize_material_ri_leaves_an_already_named_selection_alone() {
        let design = fixture_design();
        let mut selection = indicatrix_cut_core::MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        assert_eq!(
            default_optimize_material_ri(&design, &mut selection, &[]),
            None
        );
        assert_eq!(selection.refractive_index_override, None);
    }

    #[test]
    fn default_optimize_material_ri_defaults_to_the_designs_effective_ri() {
        let mut design = fixture_design();
        design.meta.refractive_index = 1.62;
        let mut selection = indicatrix_cut_core::MaterialSelection::none();
        let defaulted = default_optimize_material_ri(&design, &mut selection, &[]);
        assert_eq!(defaulted, Some(1.62));
        assert_eq!(selection.refractive_index_override, Some(1.62));
    }

    /// Regression guard: a design naming a CUSTOM catalogue
    /// material (not one of the built-ins `Design::effective_refractive_index`
    /// alone can resolve) must default to THAT material's own RI, not fall through
    /// to the legacy schedule value -- the optimizer would otherwise score against
    /// a material the design was never actually set to.
    #[test]
    fn default_optimize_material_ri_resolves_a_custom_materials_own_ri() {
        let mut design = fixture_design();
        design.material.name = Some("My Custom Garnet".to_string());
        // A legacy schedule RI that must NOT be the value picked, proving this
        // reads the custom material rather than falling through past it.
        design.meta.refractive_index = 1.54;
        let mut custom = GemMaterial::diamond();
        custom.name = "My Custom Garnet".to_string();
        custom.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
            a: 1.74,
            b: 0.0,
            c: 0.0,
        };
        let mut selection = indicatrix_cut_core::MaterialSelection::none();
        let defaulted = default_optimize_material_ri(&design, &mut selection, &[custom]);
        assert!(
            (defaulted.unwrap() - 1.74).abs() < 1e-6,
            "expected the custom material's own RI (1.74), got {defaulted:?}"
        );
        assert!((selection.refractive_index_override.unwrap() - 1.74).abs() < 1e-6);
    }

    // --- pin_non_selected_free_tiers ---

    fn tier(name: &str, constraint: MeetConstraint) -> indicatrix_cut_core::ConstraintTier {
        indicatrix_cut_core::ConstraintTier {
            angle_deg: -40.0,
            name: name.to_string(),
            indices: vec![0.0, 24.0],
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn two_free_tier_design() -> Design {
        Design::new(
            indicatrix_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::default(),
            vec![
                tier("Anchor", MeetConstraint::ScaleReference(0.5)),
                tier("Free1", MeetConstraint::MeetExisting),
                tier("Free2", MeetConstraint::MeetExisting),
            ],
        )
    }

    #[test]
    fn pin_non_selected_free_tiers_pins_every_free_tier_not_kept() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        // Tier 1 (kept) is untouched: still free.
        assert!(matches!(
            restricted.tiers[1].constraint,
            MeetConstraint::MeetExisting
        ));
        // Tier 2 (not kept) is now pinned to its own solved mast.
        assert!(matches!(
            restricted.tiers[2].constraint,
            MeetConstraint::ScaleReference(_)
        ));
        // The already-pinned anchor tier is unaffected either way.
        assert!(matches!(
            restricted.tiers[0].constraint,
            MeetConstraint::ScaleReference(_)
        ));
    }

    #[test]
    fn pin_non_selected_free_tiers_keeps_every_free_tier_when_keep_set_is_full() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1, 2]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        assert!(matches!(
            restricted.tiers[1].constraint,
            MeetConstraint::MeetExisting
        ));
        assert!(matches!(
            restricted.tiers[2].constraint,
            MeetConstraint::MeetExisting
        ));
    }

    #[test]
    fn pin_non_selected_free_tiers_never_reorders_or_removes_a_tier() {
        let design = two_free_tier_design();
        let solved = design.solve().expect("fixture must solve");
        let keep = BTreeSet::from([1]);
        let restricted = pin_non_selected_free_tiers(&design, &solved, &keep);
        assert_eq!(restricted.tiers.len(), design.tiers.len());
        for (a, b) in restricted.tiers.iter().zip(&design.tiers) {
            assert_eq!(a.name, b.name);
        }
    }
}

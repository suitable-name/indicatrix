//! Opening the dialog, and rebuilding the proposal when the mode/crown/target
//! controls change -- Shift mode's own synchronous rebuild with its validity check, and
//! Optimize mode's list of the Shift angles the search starts from. The search itself, its
//! options and the pick of one are [`super::optimize_run`]'s.

use super::{
    RETARGET_ASYNC, apply_ghost_preview_or_revert,
    check_run::{begin_check, reset_check},
    material::{initial_target_index, resolve_target_selection, resolved_material_from_selection},
    optimize_run::{
        cancel_optimize_run, prepare_for_plan, push_options, refresh_estimate, reset_search_ui,
        select_candidate, start_search,
    },
    proposal_view::{plan_view, push_retarget_view, push_target_error, push_target_readout},
    sync_embedded_comparison,
};
use crate::{
    MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            edit_intent::{EditIntent, EditIntentQueue},
            retarget::{CrownShift, RetargetPlan, RetargetProposal, build_plan},
            stale::{self, ResultKind},
            stall_guard::stall_guard,
            state::{EditorState, design_material_options},
            view,
        },
        render::render_visibility::recompute_tab_visible,
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::Design;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// Shown in place of the table when no tier has a slope the retarget could change.
const NOTHING_TO_RETARGET: &str =
    "Nothing to retarget: no crown or pavilion facet has a slope to shift.";

/// The first note of Optimize mode before a search has run: the table lists the Shift angles
/// the search would start from, not a result.
const SEED_NOTE: &str = "These are the Shift angles the search starts from. Press Search to look for better ones; the options it finds are listed below.";

/// Reads `render_ctx`'s current custom materials, resolves the target against them plus
/// `RetargetModel`'s own target picker fields, then builds the Shift plan. Shows the target
/// readout, or the error and `None` when the picker text cannot be used. The common first
/// half of the Shift and Optimize rebuilds below.
fn resolve_plan(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    crown: CrownShift,
) -> Option<(RetargetPlan, Vec<GemMaterial>)> {
    let combo_index = ui.global::<RetargetModel>().get_target_material_index();
    let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
    // Cloned out of the lock alongside the resolution below, rather than re-locking
    // for the proposal: `retarget_view` needs the same catalogue the target was
    // resolved against, and re-locking could observe a different
    // one if a custom material were saved in between.
    let (selection, target, custom_materials) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked with the Result-returning resolver FIRST, so an
        // unparseable override is reported instead of silently resolved against
        // `design.material` -- see `push_target_error`'s own doc comment.
        match resolve_target_selection(design, &ctx.custom_materials, combo_index, &ri_text) {
            Ok(selection) => {
                let target = resolved_material_from_selection(&selection, &ctx.custom_materials);
                let custom_materials = ctx.custom_materials.as_ref().clone();
                (selection, target, custom_materials)
            }
            Err(message) => {
                drop(ctx);
                push_target_error(ui, &message);
                return None;
            }
        }
    };
    push_target_readout(ui, &selection, &target);
    let plan = build_plan(design, &target, crown, &custom_materials);
    Some((plan, custom_materials))
}

/// Shift mode: builds the plan, pushes its rows and notes and starts the off-thread validity
/// check ([`begin_check`], `generation` is the design generation the plan is built against).
/// The common body [`setup_retarget_open_callback`] and Shift mode's branch of
/// [`setup_retarget_proposal_changed_callback`] both need.
///
/// The returned proposal holds only the rows that move; the table and culet are listed in
/// the dialog (greyed, "Not changed") but never travel with it. `None` when the target
/// text is unusable or nothing would move.
pub(super) fn rebuild_and_push(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    generation: u64,
    crown: CrownShift,
) -> Option<RetargetProposal> {
    let (plan, custom_materials) = resolve_plan(ui, render_ctx, design, crown)?;
    let mut view = plan_view(&plan).with_tier_names(design);
    if plan.is_empty() {
        view.rows.clear();
        view.notes.push(NOTHING_TO_RETARGET.to_string());
        push_retarget_view(ui, view);
        reset_check(ui);
        return None;
    }
    push_retarget_view(ui, view);
    let proposal = plan.proposal();
    begin_check(ui, design, plan, &custom_materials, generation);
    Some(proposal)
}

/// Optimize mode: lists the Shift angles the search starts from (no check, no proposal --
/// nothing can be applied until the cutter picks one of the options a search finds) and
/// prepares the time estimate.
fn rebuild_seed(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    generation: u64,
    crown: CrownShift,
) {
    let Some((plan, _)) = resolve_plan(ui, render_ctx, design, crown) else {
        return;
    };
    reset_check(ui);
    let mut view = plan_view(&plan).with_tier_names(design);
    if plan.is_empty() {
        view.rows.clear();
        view.notes.push(NOTHING_TO_RETARGET.to_string());
        push_retarget_view(ui, view);
        return;
    }
    view.notes.insert(0, SEED_NOTE.to_string());
    push_retarget_view(ui, view);
    prepare_for_plan(ui, design, &plan, generation);
}

/// "Retarget for material...": opens the dialog and builds the first proposal
/// (Shift mode, default crown settings -- reset here even if a previous session left
/// the dialog's `in-out` properties on Optimize/a nonzero crown fraction). Also
/// seeds the target picker (see this group's own `mod.rs` doc comment, "Where the
/// target material comes from") from `design`'s own current material, fills the Optimize
/// tab's option lists, and drops whatever a previous session's search left in
/// [`super::RETARGET_ASYNC`].
pub(in crate::gui::editor) fn setup_retarget_open_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    super::advanced_in_use::setup_advanced_in_use_callback(ui);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_open(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
        stale::clear(ResultKind::Retarget);

        let mut st = state.borrow_mut();
        ui.global::<RetargetModel>().set_mode_index(0);
        ui.global::<RetargetModel>().set_crown_fraction(0.0);
        ui.global::<RetargetModel>().set_scale_crown_by_ratio(false);
        ui.global::<RetargetModel>()
            .set_crown_follows_pavilion(true);
        ui.global::<RetargetModel>().set_keep_look(true);
        ui.global::<RetargetModel>().set_girdle_allowance(true);
        reset_search_ui(&ui);
        ui.global::<RetargetModel>().set_free_tier_count(0);
        push_options(&ui);
        // The viewport ghost preview is never used while the dialog is open (the
        // viewport is paused behind the modal and the embedded comparison pane shows
        // the proposal), so it is forced off: a previous session's value must not
        // put a stale ghost into the shared viewport.
        ui.global::<RetargetModel>().set_preview_enabled(false);

        let options = {
            let ctx = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            design_material_options(&ctx.custom_materials)
        };
        ui.global::<RetargetModel>()
            .set_target_material_index(initial_target_index(&st.design.material, &options));
        ui.global::<RetargetModel>().set_target_ri_override_text(
            st.design
                .material
                .refractive_index_override
                .map(|v| format!("{v}"))
                .unwrap_or_default()
                .into(),
        );
        ui.global::<RetargetModel>()
            .set_material_options(ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));

        let generation = st.generation.load(AtomicOrdering::Relaxed);
        let proposal = rebuild_and_push(
            &ui,
            &render_ctx,
            &st.design,
            generation,
            CrownShift::default(),
        );
        let solved = proposal.is_some();
        if let Some(proposal) = proposal {
            stale::stamp(ResultKind::Retarget, generation);
            st.pending_retarget = Some((proposal, generation));
        } else {
            st.pending_retarget = None;
        }
        // Released before the embedded comparison's own handler runs: it reads the
        // editor state to build the before/after pair.
        drop(st);
        ui.global::<RetargetModel>().set_is_open(true);
        // The modal hides the main viewport, so pause it for as long as the dialog
        // is open; the embedded comparison pane is the live view instead.
        recompute_tab_visible(&ui, &render_ctx);
        sync_embedded_comparison(&ui, solved);
    });
}

/// The mode/crown/target controls changed: rebuilds the proposal from their current
/// values. Shift mode rebuilds the plan and starts its validity check; Optimize mode lists
/// the Shift angles a search would start from and waits for the Search button.
///
/// Also registers the Optimize tab's callbacks -- `cancel_optimize`, `search`,
/// `select_candidate` and `settings_changed`: all are wired from this one `setup_*` function
/// (rather than separate ones) so this group's async run tracking stays entirely inside this
/// file's and [`super::optimize_run`]'s closures, with no new `setup_retarget_*` call site
/// needed in `gui::editor::mod` (owned elsewhere) to wire them up.
///
/// `preview_state`/`solid_last_solved` (added beyond this function's original
/// signature): every Shift rebuild here also shows or reverts the live ghost preview via
/// [`apply_ghost_preview_or_revert`], matching `RetargetModel.preview_enabled`'s
/// current value.
///
/// `retarget_dialog.slint`'s crown
/// slider fires its `changed(value)` interaction callback on every pixel of drag
/// (`ui/components/retarget_dialog.slint`'s own `crown_slider`), so calling
/// `RetargetModel.proposal_changed()` straight into a synchronous `rebuild_and_push`
/// plus ghost-preview/replan resubmit on every tick would rebuild the WHOLE
/// proposal once per pixel dragged. Both modes instead post an [`EditIntent::RetargetCrown`]
/// into a queue this function builds once, draining at most once per 16ms tick -- see
/// [`EditIntentQueue`]'s own doc comment.
pub(in crate::gui::editor) fn setup_retarget_proposal_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    {
        let intent_queue = {
            let state = Rc::clone(state);
            let render_ctx = Arc::clone(render_ctx);
            let preview_state = Arc::clone(preview_state);
            let solid_last_solved = Arc::clone(solid_last_solved);
            let ui_weak = ui.as_weak();
            EditIntentQueue::new(move |_intent| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                apply_retarget_intent(&ui, &state, &render_ctx, &preview_state, &solid_last_solved);
            })
        };
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_proposal_changed(move || {
            stall_guard("retarget_on_proposal_changed", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // A new request always supersedes whatever search was in flight, and the
                // options a finished one offered were computed from the old inputs.
                RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
                reset_search_ui(&ui);
                intent_queue.post(EditIntent::RetargetCrown);
            });
        });
    }

    {
        let state = Rc::clone(state);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_cancel_optimize(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Optimize mode never holds a Shift proposal, but cleared defensively anyway
            // since it costs nothing.
            state.borrow_mut().pending_retarget = None;
            cancel_optimize_run(&ui);
        });
    }

    {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_search(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            start_search(&ui, &state, &render_ctx);
        });
    }

    {
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>()
            .on_select_candidate(move |index| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                if let Ok(index) = usize::try_from(index) {
                    select_candidate(&ui, index);
                }
            });
    }

    {
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_settings_changed(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            refresh_estimate(&ui);
        });
    }
}

/// [`setup_retarget_proposal_changed_callback`]'s body, run once per drained
/// [`EditIntent::RetargetCrown`] instead of once per crown-slider tick -- see that function's
/// own doc comment. Reads `RetargetModel.crown_fraction`/`scale_crown_by_ratio`/
/// `crown_follows_pavilion` fresh, so a
/// coalesced burst always rebuilds against the LATEST slider position, not whatever it was
/// when the first tick of the burst posted.
///
/// Never solves: the ghost overlay's candidate is queued on the ghost preview's
/// background worker ([`view::submit_design_ghost_preview`]), so a slider drag stays
/// responsive however slow the candidate is to solve.
fn apply_retarget_intent(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let mut st = state.borrow_mut();
    let crown = CrownShift {
        fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
        scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
        follow_pavilion: ui.global::<RetargetModel>().get_crown_follows_pavilion(),
    };
    ui.global::<RetargetModel>().set_is_busy(false);
    let generation = st.generation.load(AtomicOrdering::Relaxed);
    if ui.global::<RetargetModel>().get_mode_index() == 1 {
        // Optimize mode holds no Shift proposal: Apply can only ever take an option the
        // search found, from `RETARGET_ASYNC::pending`. A stale Shift proposal left from
        // before the cutter switched modes must not be applied instead.
        st.pending_retarget = None;
        stale::clear(ResultKind::Retarget);
        rebuild_seed(ui, render_ctx, &st.design, generation, crown);
        // Released first: the embedded comparison's handler reads the editor state.
        drop(st);
        sync_embedded_comparison(ui, false);
        return;
    }
    let proposal = rebuild_and_push(ui, render_ctx, &st.design, generation, crown);
    // `true`: the ghost is queued and draws when its solve lands (a later tick
    // supersedes it), so the real design is not replanned over it. `false`: no ghost
    // is wanted, and the replan puts the real design back, which also drops a ghost
    // still being solved.
    if !apply_ghost_preview_or_revert(ui, render_ctx, preview_state, &st.design, proposal.as_ref())
    {
        view::submit_preview_replan(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    }
    // See `stale::ResultKind::Retarget`'s own doc comment.
    let solved = proposal.is_some();
    if let Some(proposal) = proposal {
        stale::stamp(ResultKind::Retarget, generation);
        st.pending_retarget = Some((proposal, generation));
    } else {
        stale::clear(ResultKind::Retarget);
        st.pending_retarget = None;
    }
    // Released first: the embedded comparison's handler reads the editor state.
    drop(st);
    sync_embedded_comparison(ui, solved);
}

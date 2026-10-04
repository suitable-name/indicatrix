//! The no-solve panel refresh: [`refresh_editor_panel_stale`]/[`push_stale_content`]
//! (the counterpart to [`super::panel::refresh_editor_panel_from_solve`] every edit
//! callback other than the explicit "Solve" action uses), and the Deep Solve/
//! Optimize button availability/hint pushes that ride along with both refresh paths.

use super::{
    panel::{push_tier_list_and_undo_redo, push_yield_material_scratch},
    solve_results::{deep_solve_hint, optimize_hint},
    state::{
        EditorState, ScratchDelta, design_label_text, manufacturability_warnings_tagged,
        preform_y_offset_mm_text, result_is_stale, tier_items_stale_with_last_solved,
    },
};
use crate::{
    AngleItem, EditorModel,
    bridge::render_thread::RenderContext,
    gui::editor::{auto_solve, stale},
};
use indicatrix_cut_core::PreformShape;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::Cell,
    collections::BTreeSet,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering},
};

/// The design state the Tier inspector form was last reconciled against: the edit
/// generation and the tier count. Two equal marks mean no edit landed in between,
/// so the form has nothing to re-seed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormSyncMark {
    /// `EditorState::generation` at the last refresh.
    generation: u64,
    /// `design.tiers.len()` at the last refresh.
    tier_count: usize,
}

thread_local! {
    /// The [`FormSyncMark`] the last refresh recorded (`None` before the first). UI
    /// thread only: every caller is a Slint callback, and keeping the mark here leaves
    /// `EditorState` untouched by a purely presentational bookkeeping value.
    static LAST_FORM_SYNC: Cell<Option<FormSyncMark>> = const { Cell::new(None) };
}

/// Whether the Tier form owes a re-seed after a refresh, and if so whether the tier
/// COUNT changed since the previous one (the payload of
/// `EditorModel.form_reseed_tier_count_changed`).
///
/// `None` when nothing changed since `last` (a refresh that follows no edit, e.g.
/// the availability pushes, must never disturb a form the cutter is typing into) or
/// when the selection does not survive (`selection_survives == false`: the form is
/// blanked through `form_reset_pulse` instead, and re-seeding a row that is gone
/// would resurrect it). A missing `last` re-seeds once, since nothing is known about
/// what the form was built from.
fn form_reseed_request(
    last: Option<FormSyncMark>,
    now: FormSyncMark,
    selection_survives: bool,
) -> Option<bool> {
    if !selection_survives {
        return None;
    }
    match last {
        Some(previous) if previous == now => None,
        Some(previous) => Some(previous.tier_count != now.tier_count),
        None => Some(false),
    }
}

/// Bumps `EditorModel.form_reseed_pulse` with `form_reseed_tier_count_changed`
/// written first, so `EditorView`'s watcher reads the matching flag.
///
/// A plain increment rather than a flag: a same-value property write raises no
/// `changed` (see `EditorModel.form_reset_pulse`'s doc comment for the rule).
fn push_form_reseed_pulse(ui: &crate::MainWindow, tier_count_changed: bool) {
    let model = ui.global::<EditorModel>();
    model.set_form_reseed_tier_count_changed(tier_count_changed);
    let pulse = model.get_form_reseed_pulse();
    model.set_form_reseed_pulse(pulse.wrapping_add(1));
}

/// Re-seeds the Tier inspector form from the design when an edit landed since the
/// previous refresh and the selected tier is still there.
///
/// The form is a snapshot (`EditorInspector`'s scratch fields) that only a selection
/// change loads, so every edit that leaves the selection where it was -- a handle
/// drag, nudge, pin, adopt, facet edit, gear remap, retarget or optimize apply,
/// symmetry change, Undo/Redo -- would otherwise leave it showing pre-edit values
/// that Save Tier writes straight back. `EditorInspector.reseed_after_external_change`
/// decides per form what the pulse does: a clean form follows the design, a dirty
/// one keeps the draft and says the tier moved underneath it.
///
/// `pub(super)`, not private: `panel.rs` (a sibling file) shares this call for the
/// refresh that follows a real solve.
pub(super) fn sync_tier_form_with_design(ui: &crate::MainWindow, state: &EditorState) {
    let now = FormSyncMark {
        generation: state.generation.load(AtomicOrdering::Relaxed),
        // Concave tiers count too: a concave row is a selectable table row, and adding or
        // removing one is as structural as it is for a flat tier.
        tier_count: state.design.tiers.len() + state.design.concave_tiers.len(),
    };
    let previous = LAST_FORM_SYNC.with(|cell| cell.replace(Some(now)));
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let selection_survives = usize::try_from(selected).is_ok_and(|index| index < now.tier_count);
    if let Some(tier_count_changed) = form_reseed_request(previous, now, selection_survives) {
        push_form_reseed_pulse(ui, tier_count_changed);
    }
}

/// Pushes Deep Solve's `available`/`hint` properties (see [`deep_solve_hint`]) --
/// shared by [`super::panel::refresh_editor_panel_from_solve`] and
/// [`refresh_editor_panel_stale`] since whether there is anything to repair
/// depends on the tier list, which both paths refresh.
///
/// `pub(super)`, not private: `panel.rs` (a sibling file) shares this call.
pub(super) fn refresh_deep_solve_availability(ui: &crate::MainWindow, state: &EditorState) {
    let (available, hint) = deep_solve_hint(state);
    ui.global::<EditorModel>()
        .set_deep_solve_available(available);
    ui.global::<EditorModel>().set_deep_solve_hint(hint.into());
}

/// Pushes Optimize's `available`/`hint` properties (see [`optimize_hint`]) -- shared
/// by [`super::panel::refresh_editor_panel_from_solve`] and
/// [`refresh_editor_panel_stale`] for the identical reason
/// [`refresh_deep_solve_availability`] is.
///
/// Also re-derives `editor_optimize_can_apply` from `EditorState::pending_optimize`
/// against the CURRENT `generation` -- so any edit that reaches this function
/// immediately greys the Apply button out the moment it would apply to a design the
/// search no longer describes, rather than waiting for a click to be refused.
/// `setup_optimize_callback`'s completion handler sets this property too, for the
/// one moment (a run finishing) that bypasses both refresh functions.
///
/// `pub(super)`, not private: `panel.rs` (a sibling file) shares this call.
pub(super) fn refresh_optimize_availability(ui: &crate::MainWindow, state: &EditorState) {
    let (available, hint) = optimize_hint(state, configured_optimize_max_evaluations(ui));
    ui.global::<EditorModel>().set_optimize_available(available);
    ui.global::<EditorModel>().set_optimize_hint(hint.into());

    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    let can_apply = state
        .pending_optimize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|(_, started_generation)| *started_generation == current_generation);
    ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
}

/// The coordinate-stage evaluation budget a fresh Optimize run
/// would actually use right now, read from `EditorModel.optimize_budget_text` (an
/// in-out text field the Optimize tab exposes). Falls back to
/// [`indicatrix_cut_core::OptimizeConfig::default`]'s own `200` for an empty,
/// unparseable, or non-positive value, so a field nobody has touched yet (or a
/// build with no such control at all) behaves as if this budget field did not
/// exist.
///
/// `callbacks::solve_actions::setup_optimize_callback` reads the SAME property the
/// same way when it actually launches a run, so the number quoted here can never
/// disagree with the number a click would use.
#[must_use]
pub(in crate::gui::editor) fn configured_optimize_max_evaluations(ui: &crate::MainWindow) -> usize {
    ui.global::<EditorModel>()
        .get_optimize_budget_text()
        .parse::<usize>()
        .ok()
        .filter(|&v| v > 0)
        .unwrap_or(200)
}

/// The no-solve counterpart to [`super::panel::refresh_editor_panel`] -- called
/// after every edit action that is not the explicit "Solve" button. Updates the
/// tier list via [`super::state::tier_items_stale`] (no mast/strategy data) and
/// undo/redo/preform fields exactly like [`super::panel::refresh_editor_panel`]
/// does, but overwrites the validation banner with a fixed, unmissable
/// "not solved" message instead of calling
/// [`super::state::status_text_and_is_problem`] -- this function must never touch
/// `Design::solve`/`status`/`measure` even indirectly.
///
/// Also re-seeds the Tier inspector form when an edit changed the design and the
/// selected tier survived -- see [`sync_tier_form_with_design`]; it runs inside
/// [`push_stale_content`], so this function's callers and `refresh_all`'s
/// large-design path both get it.
///
/// Ends by calling [`auto_solve::on_edit`]: every edit callback in this group calls
/// this function already, so that one call is this crate's single hook point for
/// "maybe schedule a debounced background solve" -- see that function's own doc
/// comment. [`push_stale_content`] is split out separately so
/// [`super::viewport::refresh_all`]'s large-design path can push the identical
/// stale content WITHOUT also scheduling a redundant debounced auto-solve on top
/// of the immediate background solve it dispatches itself.
///
/// `dirty` names the tier(s) the triggering edit is known to have touched -- see
/// [`super::state::tier_items_stale_with_last_solved`]'s own doc comment for
/// exactly how those rows (and every other row) are treated. Pass every tier
/// index (`0..state.design.tiers.len()`) for an edit whose blast radius isn't
/// tracked precisely, the same cases [`super::viewport::submit_preview_replan`]'s
/// own `force_full_solve: true` already names (Undo/Redo, a gear remap, a
/// symmetry/mirror change).
pub(in crate::gui::editor) fn refresh_editor_panel_stale(
    ui: &crate::MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    push_stale_content(ui, render_ctx, state, dirty);
    auto_solve::on_edit(ui, render_ctx, state);
    // Every ordinary edit ends here (tier form, quick add, inline edits,
    // undo/redo, material/yield applies) -- the worked-example guide's main
    // automatic-advance hook.
    crate::gui::editor::guide::check_progress(ui, state);
}

/// [`push_stale_content`]'s own
/// "which analysis results now describe an older generation" segment, split out
/// purely to keep that function under clippy's function-length lint. Recomputed
/// on EVERY edit (not only when a Deep Solve/Optimize run itself completes) so
/// the "Stale: design changed" badge appears the instant a FURTHER edit lands on
/// top of an already-displayed result -- see `EditorModel.deep_solve_stale`'s own
/// doc comment.
fn push_stale_generation_badges(
    ui: &crate::MainWindow,
    state: &EditorState,
    current_generation: u64,
) {
    ui.global::<EditorModel>()
        .set_deep_solve_stale(result_is_stale(
            state.deep_solve_result_generation,
            current_generation,
        ));
    let optimize_result_generation = state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, generation)| *generation);
    ui.global::<EditorModel>()
        .set_optimize_stale(result_is_stale(
            optimize_result_generation,
            current_generation,
        ));
    // `stale::refresh_badges` covers the Retarget proposal, the one analysis
    // result with no `EditorState` field of its own to stamp a generation on --
    // see that module's own doc comment for why Deep Solve/Optimize stay on the
    // two direct pushes just above instead.
    stale::refresh_badges(ui, current_generation);
}

/// [`push_stale_content`]'s manufacturability-warnings segment, split out purely
/// to keep that function under clippy's function-length lint.
///
/// A stale solve's MESH-based manufacturability findings would no longer
/// describe the current (edited, unsolved) design, so those are not shown -- but
/// the two mast-free checks (gear quantization, cut order) need no mast at all
/// and stay real and actionable even here, tagged "(pre-solve)" so they are
/// never mistaken for a completed pass.
fn push_stale_warnings(ui: &crate::MainWindow, state: &EditorState) {
    let tagged_warnings = manufacturability_warnings_tagged(&state.design, None);
    let warning_tiers: Vec<i32> = tagged_warnings
        .iter()
        .map(|(index, _)| i32::try_from(*index).unwrap_or(i32::MAX))
        .collect();
    let warnings: Vec<SharedString> = tagged_warnings
        .into_iter()
        .map(|(_, text)| SharedString::from(text))
        .collect();
    super::state::push_rows(
        &ui.global::<EditorModel>().get_manufacturability_warnings(),
        warnings,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warnings(model);
        },
    );
    super::state::push_rows(
        &ui.global::<EditorModel>()
            .get_manufacturability_warning_tiers(),
        warning_tiers,
        |model| {
            ui.global::<EditorModel>()
                .set_manufacturability_warning_tiers(model);
        },
    );
}

/// [`push_stale_content`]'s preform/yield-figure segment, split out purely to
/// keep that function under clippy's function-length lint. The preform SCRATCH
/// fields (shape/half-width/length-over-width/depth) only reseed when the
/// design's own preform actually changed (`delta.preform`); the mm readouts and
/// every yield figure below need a real solve this "no-solve" path deliberately
/// never does, so they are unconditionally cleared on every stale push
/// rather than left showing a superseded result.
fn push_stale_preform_and_yield(ui: &crate::MainWindow, state: &EditorState, delta: &ScratchDelta) {
    if delta.preform {
        let preform = &state.design.preform;
        ui.global::<EditorModel>()
            .set_preform_shape_index(match preform.shape {
                PreformShape::Block => 0,
                PreformShape::Cylinder { .. } => 1,
            });
        ui.global::<EditorModel>()
            .set_preform_half_width(format!("{:.4}", preform.half_width).into());
        ui.global::<EditorModel>()
            .set_preform_length_over_width(format!("{:.4}", preform.length_over_width).into());
        ui.global::<EditorModel>()
            .set_preform_depth(format!("{:.4}", preform.depth).into());
    }
    ui.global::<EditorModel>()
        .set_preform_half_width_mm_text("".into());
    ui.global::<EditorModel>()
        .set_preform_depth_mm_text("".into());
    // `mm_per_unit: None` always resolves to `""`.
    ui.global::<EditorModel>().set_preform_y_offset_mm(
        preform_y_offset_mm_text(state.design.preform_y_offset, None).into(),
    );

    // The form fields still get seeded from the design's current state (an edit may
    // have just changed the girdle diameter/material), but only when that value
    // actually changed (`delta`), and the read-only figures are cleared, not left
    // showing a superseded result: `yield_report` needs a solve this function
    // deliberately never does.
    push_yield_material_scratch(ui, state, delta);
    ui.global::<EditorModel>()
        .set_volumetric_yield_text("".into());
    ui.global::<EditorModel>().set_carat_weight_text("".into());
    ui.global::<EditorModel>()
        .set_specific_gravity_used_text("".into());
    ui.global::<EditorModel>()
        .set_preform_fit_warning("".into());
}

/// [`push_stale_content`]'s proportions/cutting-instructions/facet-count reset,
/// split out purely to keep that function under clippy's function-length lint.
/// All three need a real solve -- cleared here and repopulated once
/// [`super::panel::refresh_editor_panel`] next runs (the explicit "Solve"
/// action, or New/Load).
fn push_stale_proportions_reset(ui: &crate::MainWindow) {
    ui.global::<EditorModel>()
        .set_proportions_table_pct("-".into());
    ui.global::<EditorModel>()
        .set_proportions_crown_height("-".into());
    ui.global::<EditorModel>()
        .set_proportions_pavilion_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_total_depth("-".into());
    ui.global::<EditorModel>()
        .set_proportions_length_to_width("-".into());
    ui.global::<EditorModel>()
        .set_girdle_thickness_text("-".into());
    ui.global::<EditorModel>()
        .set_crown_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_pavilion_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_girdle_to_width_text("-".into());
    ui.global::<EditorModel>()
        .set_cutting_rows(ModelRc::new(VecModel::from(Vec::<AngleItem>::new())));
    // Facets are a solve-dependent count exactly
    // like the cutting instructions just above -- reset to 0 ("nothing solved yet")
    // rather than left showing a superseded design's count, matching
    // `refresh_editor_panel_from_solve`'s own "0 when the design does not
    // currently solve" reasoning for this same property.
    ui.global::<EditorModel>().set_facet_count(0);
    // the proportion-verdict chips are solve-dependent exactly like the
    // numbers above -- reset to "nothing to judge yet" (`-1`/`""`) rather than
    // left describing whichever design was last solved. `panel::
    // push_proportion_verdicts_from_solved`'s own `None` case pushes the
    // identical five values; this stays a direct push (not a shared call
    // through that function) since this call site has no `Design`/effective-RI
    // on hand to pass it, and none is needed to know the answer is always
    // "nothing yet" here.
    ui.global::<EditorModel>()
        .set_proportion_verdict_table_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_table_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_crown_angle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_crown_angle_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_pavilion_angle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_pavilion_angle_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_total_depth_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_total_depth_reason("".into());
    ui.global::<EditorModel>()
        .set_proportion_verdict_girdle_level(-1);
    ui.global::<EditorModel>()
        .set_proportion_verdict_girdle_reason("".into());
}

/// The actual "no-solve" content push -- see [`refresh_editor_panel_stale`]'s doc
/// comment for why this is split out, and for `dirty`'s own meaning. Split
/// further into [`push_stale_generation_badges`]/[`push_stale_warnings`]/
/// [`push_stale_preform_and_yield`]/[`push_stale_proportions_reset`] purely to
/// keep this function itself under clippy's function-length lint -- each of
/// those pushes one self-contained group of `EditorModel` properties and touches
/// nothing the others do.
///
/// `pub(super)`, not private: [`super::viewport::refresh_all`] (a sibling file)
/// shares this exact push for its own large-design path.
pub(super) fn push_stale_content(
    ui: &crate::MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    dirty: &BTreeSet<usize>,
) {
    super::viewport::push_trace_staleness(ui, render_ctx, state);
    let current_generation = state.generation.load(AtomicOrdering::Relaxed);
    push_stale_generation_badges(ui, state, current_generation);

    let delta = state.record_scratch_push();
    let n_d = super::inspector::refresh_design_settings(ui, render_ctx, state, &delta);
    // A cached solve from BEFORE this edit is still real
    // evidence for every row `dirty` does not name -- see `auto_solve::
    // solid_last_solved`'s own doc comment for why this is read from the shared
    // thread-local handle rather than threaded through as a parameter (this
    // function has no `SolidLastSolved` of its own to read).
    let last_solved = auto_solve::solid_last_solved().and_then(|cache| {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            // the shared cache is now generation-tagged
            // (`Arc<Mutex<Option<(u64, Vec<SolvedTier>)>>>`) -- this reader
            // predates that tag and only ever wanted the masts themselves, so
            // the generation half is simply dropped here rather than checked
            // (this function already has `dirty`'s own precise "which rows
            // changed" information from the caller, and only reads the cache
            // as evidence for the rows `dirty` does NOT name).
            .map(|(_, solved)| solved)
    });
    let tiers =
        tier_items_stale_with_last_solved(&state.design, n_d, last_solved.as_deref(), dirty);
    push_tier_list_and_undo_redo(ui, state, tiers);
    // After the tier rows: the form re-seeds from `EditorModel.tiers`.
    sync_tier_form_with_design(ui, state);

    ui.global::<EditorModel>().set_status_text(
        "Not solved -- click Solve to compute masts and validate this design.".into(),
    );
    ui.global::<EditorModel>().set_status_is_problem(true);
    ui.global::<EditorModel>().set_solve_state("stale".into());

    push_stale_warnings(ui, state);
    push_stale_preform_and_yield(ui, state, &delta);
    push_stale_proportions_reset(ui);

    // The design label is cheap and never solve-dependent -- always kept current.
    ui.global::<EditorModel>()
        .set_design_label(design_label_text(state.asc_filename.as_deref()).into());

    refresh_deep_solve_availability(ui, state);
    refresh_optimize_availability(ui, state);
}

#[cfg(test)]
mod tests {
    use super::{FormSyncMark, form_reseed_request};

    const fn mark(generation: u64, tier_count: usize) -> FormSyncMark {
        FormSyncMark {
            generation,
            tier_count,
        }
    }

    #[test]
    fn a_new_generation_with_a_surviving_selection_requests_a_reseed() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 6), true),
            Some(false),
            "an in-place edit re-seeds without claiming the tier count moved"
        );
    }

    #[test]
    fn a_tier_count_change_is_reported_in_the_request() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 7), true),
            Some(true)
        );
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 5), true),
            Some(true)
        );
    }

    #[test]
    fn an_unchanged_design_never_requests_a_reseed() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(4, 6), true),
            None,
            "a refresh with no edit must not disturb a form being typed into"
        );
    }

    #[test]
    fn a_vanished_selection_leaves_the_form_to_the_reset_pulse() {
        assert_eq!(
            form_reseed_request(Some(mark(4, 6)), mark(5, 5), false),
            None
        );
        assert_eq!(form_reseed_request(None, mark(1, 0), false), None);
    }

    #[test]
    fn the_first_refresh_reseeds_once() {
        assert_eq!(form_reseed_request(None, mark(1, 3), true), Some(false));
    }
}

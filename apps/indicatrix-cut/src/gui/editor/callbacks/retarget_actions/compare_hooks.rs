//! The narrow, read-only surface the visual before/after compare window
//! (`gui::editor::compare`) reaches this dialog's pending proposal through.
//!
//! Nothing here commits an edit. The compare window's "Keep after" invokes
//! `RetargetModel.apply()` -- i.e. [`super::apply::setup_retarget_apply_callback`],
//! the exact handler the dialog's own Apply button runs -- after checking with
//! [`current_retarget_guard`] that the proposal it compared is still the one that
//! handler would commit. "Discard" turns the dialog's viewport ghost off through
//! [`discard_retarget_preview`], sharing the close handler's own revert.

use super::{
    RETARGET_ASYNC,
    apply::revert_ghost_to_live_design,
    build_retarget_candidate,
    check_run::{candidate_anchors, is_invalid},
    material::{target_display_name, target_material_selection},
};
use crate::{
    MainWindow, RetargetModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            retarget::{AnchorChange, RetargetProposal},
            state::EditorState,
            view,
        },
        solid_preview::preview_state::SolidPreviewState,
    },
};
use indicatrix_cut_core::{Design, MaterialSelection};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering};

/// Everything that decides what `RetargetModel.apply()` would commit right now: the
/// held proposal, the design generation it was built against, the masts the validity
/// check re-anchored and the target material the dialog's picker names. The compare
/// window keeps the value it opened with and refuses "Keep after" unless
/// [`current_retarget_guard`] still returns an equal one -- so the cutter can never keep
/// something other than what they inspected (the dialog stays interactive while the
/// window is open).
#[derive(Debug, Clone, PartialEq)]
pub(in crate::gui::editor) struct RetargetKeepGuard {
    proposal: RetargetProposal,
    generation: u64,
    anchors: Vec<AnchorChange>,
    material: MaterialSelection,
}

/// What the compare window needs to open on the dialog's pending proposal.
pub(in crate::gui::editor) struct RetargetCompareInputs {
    /// The live design, cloned -- the "before" side.
    pub(in crate::gui::editor) current: Design,
    /// [`build_retarget_candidate`]'s candidate with the dialog's TARGET material
    /// attached -- the "after" side, exactly what Apply would leave behind (Apply
    /// commits the angles and the material as one `Edit::Batch`).
    pub(in crate::gui::editor) candidate: Design,
    /// The after side's header label, e.g. "Retarget to Ruby (Shift)".
    pub(in crate::gui::editor) after_label: String,
    /// The proposal this comparison describes -- see [`RetargetKeepGuard`].
    pub(in crate::gui::editor) guard: RetargetKeepGuard,
}

/// The held proposal, looked up in the same order
/// [`super::apply::setup_retarget_apply_callback`] consumes it: the Optimize option the cutter
/// picked ([`RETARGET_ASYNC`]) first, else a Shift-mode proposal on `EditorState`. Cloned,
/// never taken. Optimize mode never holds a Shift proposal, so the order only matters for a
/// stale leftover, which must not win over what the cutter picked.
fn pending_proposal(st: &EditorState) -> Option<(RetargetProposal, u64)> {
    RETARGET_ASYNC
        .with(|cell| cell.borrow().pending.clone())
        .or_else(|| st.pending_retarget.clone())
}

/// The dialog's target material, resolved exactly as the Apply handler resolves it.
fn target_selection(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
) -> MaterialSelection {
    let custom = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let model = ui.global::<RetargetModel>();
    target_material_selection(
        design,
        &custom,
        model.get_target_material_index(),
        &model.get_target_ri_override_text(),
    )
}

/// What `RetargetModel.apply()` would commit if pressed right now, or `None` when
/// no proposal is held at all.
#[must_use]
pub(in crate::gui::editor) fn current_retarget_guard(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Option<RetargetKeepGuard> {
    let (proposal, generation) = pending_proposal(st)?;
    let material = target_selection(ui, render_ctx, &st.design);
    // Still checking: no masts yet, which also differs from any finished guard, so a
    // window opened before the check finished can never be kept after it changed.
    let anchors = candidate_anchors(generation).unwrap_or_default();
    Some(RetargetKeepGuard {
        proposal,
        generation,
        anchors,
        material,
    })
}

/// The compare window's inputs for the held proposal.
///
/// # Errors
///
/// A cutter-facing sentence when there is nothing honest to compare: no proposal is
/// held, the design moved on since it was built (the same staleness Apply refuses), the
/// validity check is still running (the candidate needs its re-anchored masts), or its
/// retarget edit no longer applies to the live design. An INVALID change is compared:
/// that is how the cutter sees what is wrong with it.
pub(in crate::gui::editor) fn retarget_compare_inputs(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<RetargetCompareInputs, String> {
    let guard = current_retarget_guard(ui, st, render_ctx)
        .ok_or_else(|| "No retarget proposal to compare yet.".to_string())?;
    if guard.generation != st.generation.load(AtomicOrdering::Relaxed) {
        return Err(
            "The design changed since this proposal was built -- click Recompute first."
                .to_string(),
        );
    }
    let anchors = candidate_anchors(guard.generation)?;
    let mut candidate = build_retarget_candidate(&st.design, &guard.proposal, &anchors)
        .ok_or_else(|| {
            "This proposal no longer applies to the current design -- click Recompute first."
                .to_string()
        })?;
    candidate.material = guard.material.clone();
    let mode = if ui.global::<RetargetModel>().get_mode_index() == 1 {
        "Optimize"
    } else {
        "Shift"
    };
    let verdict = if is_invalid(guard.generation) {
        ", not valid"
    } else {
        ""
    };
    let after_label = format!(
        "Retarget to {} ({mode}{verdict})",
        target_display_name(&guard.material)
    );
    Ok(RetargetCompareInputs {
        current: st.design.clone(),
        candidate,
        after_label,
        guard,
    })
}

/// The compare window's "Discard" for a Retarget comparison: switches the legacy viewport
/// ghost preview (`RetargetModel.preview_enabled`, which the dialog no longer offers a
/// toggle for) off and, when it was on, puts the live design back in the
/// shared viewport through the close handler's own revert
/// ([`revert_ghost_to_live_design`]). The proposal itself stays pending and the
/// dialog stays open, so the cutter can adjust it and compare again.
pub(in crate::gui::editor) fn discard_retarget_preview(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    st: &EditorState,
) {
    let model = ui.global::<RetargetModel>();
    if !model.get_preview_enabled() {
        return;
    }
    model.set_preview_enabled(false);
    revert_ghost_to_live_design(ui, render_ctx, preview_state, solid_last_solved, st);
}

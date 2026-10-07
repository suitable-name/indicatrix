//! The verdict's Fix buttons: plan on a worker, apply as ONE undo step, say so in a toast.
//!
//! The click goes: [`fix_reason`] checks the verdict on screen still describes the design,
//! asks for a confirmation where the fix removes something, and [`start`] hands the planning
//! (`indicatrix_editor::verdict::plan_fix`, which solves candidates and so takes a moment) to
//! a thread. [`finish_fix`] runs back on the UI thread: it applies the planned edit through
//! the editor session (one history step, so one Undo takes it back), refreshes the panel the
//! way an Undo does, selects the tier the plan names and toasts the sentence.
//!
//! Nothing is applied when the design changed meanwhile (the plan was made for another
//! design) or when the planner refused: the cutter gets the planner's plain sentence instead.

use super::{SLOT, ensure_current, pending_fix, present, services};
use crate::{
    EditorModel, MainWindow,
    gui::{
        editor::{
            callbacks::{HistoryMove, refresh_after_renumbering_move},
            native_io::ask_write_confirm,
            relation_ui::take_cleared_relations_sentence,
        },
        show_toast,
    },
};
use indicatrix_cut_core::ConstraintTier;
use indicatrix_editor::verdict::{FixAction, FixPlan, plan_fix};
use slint::ComponentHandle;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The confirmation dialog's heading for a fix that removes facets.
const CONFIRM_HEADING: &str = "Remove what is cut away?";

/// The confirmation dialog's button for it.
const CONFIRM_BUTTON: &str = "Remove";

/// Said when the planner stopped unexpectedly.
const PLANNER_FAILED: &str = "The fix stopped unexpectedly. Nothing was changed.";

/// Said when the editor state was in use and the fix could not be applied.
const EDITOR_BUSY: &str = "The editor is busy. Try the fix again.";

/// `VerdictModel.fix_reason(index)`: the Fix button of reason `index` was pressed.
pub(super) fn fix_reason(ui: &MainWindow, index: i32) {
    let Some(services) = services() else {
        return;
    };
    let ready = usize::try_from(index)
        .map_err(|_| "There is no such reason.".to_string())
        .and_then(|index| pending_fix(index, &services.editor.borrow().design.tiers));
    match ready {
        Ok((action, Some(question))) => {
            ask_write_confirm(
                ui,
                CONFIRM_HEADING,
                question,
                CONFIRM_BUTTON,
                None,
                move |ui| {
                    start(ui, action);
                },
            );
        }
        Ok((action, None)) => start(ui, action),
        Err(message) => show_toast(ui, &message, "info"),
    }
}

/// Plans `action` against a copy of the design on a worker thread and applies the result.
fn start(ui: &MainWindow, action: FixAction) {
    let Some(services) = services() else {
        return;
    };
    // The confirmation dialog may have been open a while: check again.
    if let Err(message) = ensure_current(&services.editor.borrow().design.tiers) {
        show_toast(ui, &message, "info");
        return;
    }
    let design = services.editor.borrow().design.clone();
    let n_d = SLOT.with(|cell| cell.borrow().n_d);
    SLOT.with(|cell| cell.borrow_mut().busy = true);
    present::set_busy(ui, true);

    let reference = design.tiers.clone();
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let planned = catch_unwind(AssertUnwindSafe(|| plan_fix(&design, &action, n_d)))
            .unwrap_or_else(|_| Err(PLANNER_FAILED.to_string()));
        let _ = ui_weak.upgrade_in_event_loop(move |ui| finish_fix(&ui, &reference, planned));
    });
}

/// The planner's answer, back on the UI thread: applies it when it still fits the design.
fn finish_fix(ui: &MainWindow, reference: &[ConstraintTier], planned: Result<FixPlan, String>) {
    SLOT.with(|cell| cell.borrow_mut().busy = false);
    present::set_busy(ui, false);
    let plan = match planned {
        Ok(plan) => plan,
        Err(message) => {
            show_toast(ui, &message, "error");
            return;
        }
    };
    let Some(services) = services() else {
        return;
    };
    let Ok(mut st) = services.editor.try_borrow_mut() else {
        show_toast(ui, EDITOR_BUSY, "error");
        return;
    };
    // The plan was made for `reference`; any edit since (even a one-tier change) voids it.
    if st.design.tiers != reference {
        drop(st);
        show_toast(
            ui,
            "The design changed while the fix was being planned. Nothing was changed.",
            "info",
        );
        return;
    }
    let FixPlan {
        edit,
        message,
        select,
    } = plan;
    match st.try_apply_mapped(edit, None) {
        Ok((change, renumbering)) => {
            // The same refresh an Undo makes: the selected row first follows its tier (a fix
            // that removes tiers moves the rows below them), then a full re-solve, the form
            // re-seeded, the preview replanned. The solve's result reaches the verdict
            // through the usual hook, so the fix is re-validated by the next verdict.
            refresh_after_renumbering_move(
                ui,
                &services.render_ctx,
                &services.preview_state,
                &services.solid_last_solved,
                &st,
                &HistoryMove::new(change, renumbering),
            );
            // A fix that removes a tier other tiers' relations read frees those relations:
            // say so here, so the notice does not wait for an unrelated message.
            let freed = take_cleared_relations_sentence(&mut st);
            drop(st);
            if let Some(tier) = select.and_then(|tier| i32::try_from(tier).ok()) {
                ui.global::<EditorModel>().set_selected_tier_index(tier);
            }
            let mut text = format!("{message} Undo (Ctrl+Z) puts it back.");
            if let Some(freed) = freed {
                text.push(' ');
                text.push_str(&freed);
            }
            show_toast(ui, &text, "success");
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}

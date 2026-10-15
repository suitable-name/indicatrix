//! The History tab: the undo steps of the design as a list, with a jump to any of them
//! (the desktop's `editor_inspector/history_tab.slint` and `history_panel`, without the
//! pictures).
//!
//! The list is read from the shared `EditorSession` ([`history_entries`] and
//! [`history_position`]) and pushed by the inspector's refresh ([`push`], called from
//! `inspector::poll` while the tab shows; [`clear`] when there is no design). A tap calls
//! [`jump`], which walks the session with `EditorSession::jump_to_mapped` -- the same undos
//! and redos, so a jump is not itself a step and the steps it passes stay for jumping back --
//! makes the selected row follow its tier through the renumbering, and ends in the shared
//! edit path (`inspector::finish`, which is `edit::finish_edit` plus the panels), so the
//! views, the auto-solve and the persistence follow as after any edit.
//!
//! [`history_entries`]: indicatrix_editor::EditorSession::history_entries
//! [`history_position`]: indicatrix_editor::EditorSession::history_position

use super::{
    edit::{Dirty, with_app},
    inspector::{self, PushCtx},
};
use crate::{
    AppWindow, InspectorModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// `InspectorModel.tab` of the History tab.
pub const HISTORY_TAB: i32 = 4;

// The label order and the jump sentence are pure and native-tested in
// `indicatrix_web_core::rows`.
use indicatrix_web_core::rows::{jump_message, labels_newest_first};

/// Whether `shown` already lists exactly `labels`, in order.
fn same_labels(shown: &ModelRc<SharedString>, labels: &[String]) -> bool {
    shown.row_count() == labels.len()
        && labels
            .iter()
            .enumerate()
            .all(|(i, label)| shown.row_data(i).is_some_and(|s| s.as_str() == label))
}

/// Pushes the history into `InspectorModel` while the History tab is showing: the labels
/// (only when they changed, so the list keeps its scroll position) and the current step.
pub fn push(pcx: &PushCtx<'_>) {
    if pcx.tab != HISTORY_TAB || pcx.collapsed {
        return;
    }
    let session = &pcx.design_state.session;
    let labels = labels_newest_first(session.history_entries().into_iter().map(|e| e.label));
    let model = pcx.ui.global::<InspectorModel>();
    if !same_labels(&model.get_history_labels(), &labels) {
        model.set_history_labels(ModelRc::new(VecModel::from(
            labels
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    }
    model.set_history_position(i32::try_from(session.history_position()).unwrap_or(i32::MAX));
}

/// Empties the list (no design is open).
pub fn clear(ui: &AppWindow) {
    let model = ui.global::<InspectorModel>();
    model.set_history_labels(ModelRc::new(VecModel::<SharedString>::default()));
    model.set_history_position(0);
}

/// Goes to step `position` (0 is the start): the design is walked there, the selected row
/// follows its tier, and the shared edit path refreshes everything.
fn jump(ctx: &Ctx, position: i32) {
    let Ok(position) = usize::try_from(position) else {
        return;
    };
    let result = with_app(ctx, |app| {
        let selected = app.selected_tier;
        let design = app.design.as_mut()?;
        let (outcome, renumbering) = design.session.jump_to_mapped(position);
        let change = match &outcome {
            Ok(done) => done.change,
            Err(failure) => failure.change,
        };
        if let Some(change) = change {
            // The selected row stays on its tier, or is cleared when the tier is gone.
            app.selected_tier = selected.and_then(|row| {
                renumbering.map_table_row(
                    row,
                    change.tier_count_before,
                    change.tier_count_after,
                    change.concave_count_after,
                )
            });
        }
        Some(outcome)
    });
    match result {
        None => {}
        Some(Ok(done)) => {
            if done.change.is_some() {
                // A jump walks many steps; which tiers they touched is not tracked.
                inspector::finish(ctx, Dirty::All);
                show_message(ctx, MessageKind::Info, &jump_message(done.position));
            }
        }
        Some(Err(failure)) => {
            // Some steps may have been walked before the one that failed.
            if failure.change.is_some() {
                inspector::finish(ctx, Dirty::All);
            }
            show_message(
                ctx,
                MessageKind::Error,
                &format!("Cannot go to that step: {failure}"),
            );
        }
    }
}

/// Wires the History tab's jump callback.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let c = ctx.clone();
    ui.global::<InspectorModel>()
        .on_history_jump(move |position| jump(&c, position));
}

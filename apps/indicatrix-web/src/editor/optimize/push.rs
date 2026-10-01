//! Pushing the Optimize tab's availability (the desktop's
//! `panel_stale::refresh_optimize_availability`): whether any tier is free to move, the
//! hint that says why not, and whether a held result has gone stale.

use super::{RUNTIME, configured_budget};
use crate::{AppWindow, OptimizeModel, editor::inspector::PushCtx};
use indicatrix_editor::optimize_view::optimize_hint;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// One refresh: availability and hint, and the staleness of a held result.
pub fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<OptimizeModel>();
    let (available, hint) = optimize_hint(pcx.design, configured_budget(&model));
    model.set_available(available);
    model.set_hint(hint.into());

    // A held result goes stale the moment the design moves on: Apply is withdrawn and the
    // status line says why (once).
    let generation = pcx.design_state.session.current_generation();
    let (stale_note, result_stale) = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        let stale = rt
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation != generation);
        let note = if stale {
            rt.pending.take().map(|p| p.status)
        } else {
            None
        };
        // The badge covers any result on screen, appliable or not.
        (note, rt.shown.is_some_and(|shown| shown != generation))
    });
    model.set_result_stale(result_stale && !model.get_running());
    if let Some(status) = stale_note {
        model.set_can_apply(false);
        model.set_status(
            format!("{status} -- the design changed since; re-run Optimize to apply.").into(),
        );
    }
}

/// Blank: no design is loaded.
pub fn clear(ui: &AppWindow) {
    let model = ui.global::<OptimizeModel>();
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.pending = None;
        rt.shown = None;
    });
    model.set_result_stale(false);
    model.set_available(false);
    model.set_hint(SharedString::new());
    model.set_can_apply(false);
    if !model.get_running() {
        model.set_status(SharedString::new());
        model.set_result_rows(ModelRc::new(VecModel::default()));
        model.set_change_rows(ModelRc::new(VecModel::default()));
    }
}

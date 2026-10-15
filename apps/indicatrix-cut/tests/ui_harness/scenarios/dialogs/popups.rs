//! Scenario: the header popups (filters, import, remote worker, editor log, value-slider menu).

use super::{click_button, create_template_design};
use crate::support::{
    Control, Ctx, WindowView,
    app::{pump, wait_until},
    coverage,
};
use indicatrix_cut::EditorModel;
use slint::ComponentHandle;
use std::time::Duration;

/// The keys of the interactive controls currently in `view`.
fn control_keys(view: &WindowView) -> Vec<String> {
    view.controls().iter().map(Control::key).collect()
}

/// One user action on a popup.
enum Action {
    /// Click the last enabled button whose label starts with one of these.
    Click(&'static [&'static str]),
    /// Press Esc.
    Escape,
    /// Activate the first control labelled "... Values" (a value slider's menu).
    ValueMenu,
}

/// Performs `action`; `false` when its target is not on screen.
fn perform(ctx: &mut Ctx<'_>, action: &Action) -> bool {
    match action {
        Action::Click(prefixes) => click_button(&ctx.app.main_view(), prefixes).is_some(),
        Action::Escape => {
            ctx.press("Escape");
            true
        }
        Action::ValueMenu => {
            let handle = ctx
                .app
                .main_view()
                .controls()
                .into_iter()
                .find(|c| c.label.ends_with(" Values"))
                .map(|c| c.handle);
            handle.is_some_and(|h| {
                coverage::record_activation(&h);
                h.invoke_accessible_default_action();
                true
            })
        }
    }
}

/// `Role|id` of a control that has an element id: the same control under a new label (a status
/// line, an elapsed-time chip) keeps it.
fn control_slot(control: &Control) -> Option<String> {
    (!control.id.is_empty()).then(|| format!("{:?}|{}", control.role, control.id))
}

/// Opens a popup with `open` (when its opener is on screen), expects it to add controls to the
/// window's tree, and closes it with `close`; the new controls must be gone afterwards. A control
/// that was already on screen and only changed its label meanwhile (the status strip's message
/// while a solve finishes) is not one of the popup's.
fn popup_cycle(ctx: &mut Ctx<'_>, name: &str, open: &Action, close: &Action) {
    let before_view = ctx.app.main_view().controls();
    let before: Vec<String> = before_view.iter().map(Control::key).collect();
    let before_slots: Vec<String> = before_view.iter().filter_map(control_slot).collect();
    if !perform(ctx, open) {
        println!("ui-harness:   GAP {name}: its opener is not on screen in this state");
        return;
    }
    pump(300);
    let inside: Vec<String> = ctx
        .app
        .main_view()
        .controls()
        .iter()
        .filter(|c| {
            !before.contains(&c.key())
                && control_slot(c).is_none_or(|slot| !before_slots.contains(&slot))
        })
        .map(Control::key)
        .collect();
    ctx.expect(
        !inside.is_empty(),
        "popup-opens",
        format!("{name}: opening it added no interactive control to the window"),
    );
    if !perform(ctx, close) {
        ctx.expect(
            false,
            "popup-close-route",
            format!("{name}: no way to close it was found"),
        );
        return;
    }
    pump(300);
    let after = control_keys(&ctx.app.main_view());
    let left: Vec<&String> = inside.iter().filter(|key| after.contains(key)).collect();
    ctx.expect(
        left.is_empty(),
        "popup-closes",
        format!(
            "{name}: {} control(s) of the popup are still there after closing it",
            left.len()
        ),
    );
    ctx.check_health(&format!("popup {name}"));
}

/// `FilterPanel`, `ImportDialog`, `RemoteWorkerDialog` (header), `EditorLogPopup` and the value-slider
/// menu: each opens by its button, exposes controls, and closes (Esc or its close button).
pub(super) fn popups_open_and_close(ctx: &mut Ctx<'_>) {
    if !create_template_design(ctx) {
        return;
    }
    // The new design's first solve relabels the status strip and adds and drops activity chips;
    // let it finish so the popups are compared against a still window.
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let _ = wait_until(Duration::from_secs(15), || !editor.get_solve_running());
    pump(200);
    let esc = Action::Escape;
    popup_cycle(ctx, "FilterPanel", &Action::Click(&["Filters"]), &esc);
    popup_cycle(ctx, "ImportDialog", &Action::Click(&["Import"]), &esc);
    popup_cycle(ctx, "RemoteWorkerDialog", &Action::Click(&["Remote"]), &esc);
    popup_cycle(
        ctx,
        "EditorLogPopup",
        &Action::Click(&["Details"]),
        &Action::Click(&["Close Details"]),
    );
    popup_cycle(ctx, "value-slider menu", &Action::ValueMenu, &esc);
}

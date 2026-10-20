//! Scenarios: the main window's tabs and docks, the Simple/Advanced interface mode,
//! click-everything per top tab and toast dismissal.
//!
//! This file is also the home of the few driving helpers the other scenario files of this lane
//! share (`click`, `pointer_click`, `select_top_tab`, `select_render_view`): they never panic,
//! so one missing label ends a step with a finding instead of ending the whole scenario.

use super::{
    Piece, Scenario,
    editor::{Card, create_template, show_edit_tab},
    existing_install,
};
use crate::support::{
    Ctx, HarnessApp, WindowView,
    app::{pump, wait_until},
    query::is_clickable_role,
};
use indicatrix_cut::{ParametersModel, PreferencesModel, RawTextModel, SolidPreviewModel};
use slint::{
    ComponentHandle, LogicalPosition,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

/// The three catalogue tabs: the pill text at full window width, and a name for findings.
pub(super) const TOP_TABS: [(&str, &str); 3] = [
    ("3D Spectral Preview (1)", "3D Spectral Preview"),
    ("Cutting Instructions (2)", "Cutting Instructions"),
    ("Files & Downloads (3)", "Files and Downloads"),
];

/// How a tab was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// A pointer click on the tab pill selected it.
    Pointer,
    /// The pill carries no queryable label here; the model was driven instead.
    ModelOnly,
    /// The pill was clicked but the selected tab did not follow (the model was driven instead).
    Broken,
}

/// Activates the first clickable element (button, tab, radio button, list item ...) labelled
/// `label` in `view`. Plain `Text` elements with the same label are skipped. Returns whether one
/// was found.
pub(super) fn click_in(view: &WindowView, label: &str) -> bool {
    let target = view
        .by_label(label)
        .into_iter()
        .find(|e| e.accessible_role().is_some_and(is_clickable_role));
    target.is_some_and(|element| {
        element.invoke_accessible_default_action();
        true
    })
}

/// [`click_in`] over the main window, then lets the app react. Returns whether a control was
/// found and activated.
pub(super) fn click(app: &HarnessApp, label: &str) -> bool {
    let found = click_in(&app.main_view(), label);
    if found {
        pump(80);
    }
    found
}

/// Clicks with the pointer in the middle of the first visible element labelled `label` (a
/// real press and release through the window, so the touch area under it is exercised, not an
/// accessibility action). Returns whether such an element was found.
pub(super) fn pointer_click(app: &HarnessApp, label: &str) -> bool {
    let view = app.main_view();
    let target = view.by_label(label).into_iter().find(|e| {
        let size = e.size();
        size.width > 0.0 && size.height > 0.0
    });
    let Some(element) = target else {
        return false;
    };
    let origin = element.absolute_position();
    let size = element.size();
    let at = LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    let window = app.ui().window();
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    pump(80);
    true
}

/// Selects top tab `tab` (0 3D, 1 Cutting Instructions, 2 Files) through the model, exactly as
/// the tab pill does: sets the property and fires the changed callback.
pub(super) fn select_top_tab_model(app: &HarnessApp, tab: i32) {
    let ui = app.ui();
    ui.set_active_tab(tab);
    ui.invoke_active_tab_changed(tab);
    pump(100);
}

/// Selects top tab `tab` by clicking its pill; falls back to the model when the pill cannot be
/// found or the click did not take.
pub(super) fn select_top_tab(app: &HarnessApp, tab: i32) -> Reach {
    let index = usize::try_from(tab).unwrap_or(0).min(TOP_TABS.len() - 1);
    let clicked = pointer_click(app, TOP_TABS[index].0);
    if clicked && app.ui().get_active_tab() == tab {
        return Reach::Pointer;
    }
    select_top_tab_model(app, tab);
    if clicked {
        Reach::Broken
    } else {
        Reach::ModelOnly
    }
}

/// Shows the 3D tab's sub-view `view` (0 Live Render, 1 Edit) through the model, as the pills do.
pub(super) fn select_render_view(app: &HarnessApp, view: i32) {
    let ui = app.ui();
    ui.set_render_view_tab(view);
    ui.invoke_render_view_tab_changed(view);
    pump(150);
}

/// The pieces of this file; the click-everything pass is one piece per top tab.
pub fn pieces() -> Vec<Piece> {
    let mut all: Vec<Piece> = vec![
        Scenario {
            name: "tabs_and_views",
            seed: existing_install,
            run: tabs_and_views,
        }
        .into(),
        Scenario {
            name: "ui_mode_simple_and_advanced",
            seed: existing_install,
            run: ui_mode_simple_and_advanced,
        }
        .into(),
    ];
    for (index, (_, name)) in TOP_TABS.iter().enumerate() {
        all.push(Piece::new(
            format!("click_everything_per_tab:{name}"),
            existing_install,
            move |ctx| click_everything_on_tab(ctx, index),
        ));
    }
    all.push(
        Scenario {
            name: "toasts_dismiss",
            seed: existing_install,
            run: toasts_dismiss,
        }
        .into(),
    );
    all
}

/// Walks every top tab, every Parameters dock tab and the Solid view modes 1-5 on the Edit tab.
/// Proves each selection is followed by the matching model property and that every screen
/// reached sweeps clean.
fn tabs_and_views(ctx: &mut Ctx<'_>) {
    top_tabs(ctx);
    parameters_dock_tabs(ctx);
    solid_view_modes(ctx);
}

/// Each top tab by pill click; the selected-tab property must follow.
fn top_tabs(ctx: &mut Ctx<'_>) {
    for (index, (_, name)) in TOP_TABS.iter().enumerate() {
        let tab = i32::try_from(index).unwrap_or(0);
        match select_top_tab(ctx.app, tab) {
            Reach::Pointer => {}
            Reach::ModelOnly => ctx.report.note(format!(
                "top tab {name}: the pill has no queryable label here, the model was driven"
            )),
            Reach::Broken => {
                ctx.expect(
                    false,
                    "top-tab-click",
                    format!("clicking the {name} pill did not select tab {tab}"),
                );
            }
        }
        let active = ctx.app.ui().get_active_tab();
        ctx.expect(
            active == tab,
            "top-tab-selected",
            format!("{name}: active_tab is {active}, expected {tab}"),
        );
        ctx.check_health(&format!("top tab {name}"));
        ctx.sweep_main_at_sizes(&format!("top tab: {name}"));
    }
    select_top_tab(ctx.app, 0);
}

/// The Parameters dock's three tabs from the Live Render toolbar: each button opens its tab, a
/// second press on the showing tab folds the dock.
fn parameters_dock_tabs(ctx: &mut Ctx<'_>) {
    const BUTTONS: [(&str, i32); 3] = [
        ("Stone Properties", 0),
        ("Lighting and Environment", 1),
        ("Render and Output", 2),
    ];
    select_top_tab(ctx.app, 0);
    select_render_view(ctx.app, 0);
    ctx.app.ui().global::<ParametersModel>().set_open(false);
    pump(100);
    for (label, tab) in BUTTONS {
        if !click(ctx.app, label) {
            // GAP: the toolbar button is not reachable by label on this screen; the model's own
            // `show_tab` (what the rail buttons and the View menu call) stands in.
            ctx.app
                .ui()
                .global::<ParametersModel>()
                .invoke_show_tab(tab);
            pump(100);
        }
        let dock = ctx.app.ui().global::<ParametersModel>();
        let (open, active) = (dock.get_open(), dock.get_active_tab());
        ctx.expect(
            open && active == tab,
            "parameters-dock-tab",
            format!("{label}: open={open}, active_tab={active}, expected open and tab {tab}"),
        );
        ctx.check_health(&format!("Parameters dock {label}"));
        ctx.sweep_main_at_sizes(&format!("Parameters dock: {label}"));
    }
    // A second press on the tab that is showing folds the dock away.
    click(ctx.app, BUTTONS[2].0);
    let open = ctx.app.ui().global::<ParametersModel>().get_open();
    ctx.expect(
        !open,
        "parameters-dock-fold",
        "pressing the showing tab's button again did not fold the dock",
    );
}

/// Solid view modes 1-5 on the Edit tab, by the digit keys (the global shortcut); the view mode
/// property must follow. Flaky only if keyboard focus is not on the shortcut scope.
fn solid_view_modes(ctx: &mut Ctx<'_>) {
    select_top_tab(ctx.app, 0);
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template for the Solid view modes",
    ) {
        return;
    }
    for key in 1..=5_i32 {
        ctx.press(&key.to_string());
        // The mode is written by a handler that Slint runs on the next event-loop pass, so the
        // value is read only after the loop has had its turns (not straight after the key).
        let ui = ctx.app.ui().clone_strong();
        let _ = wait_until(Duration::from_secs(3), || {
            ui.global::<SolidPreviewModel>().get_view_mode() == key - 1
        });
        let mode = ui.global::<SolidPreviewModel>().get_view_mode();
        if !ctx.expect(
            mode == key - 1,
            "solid-view-mode-key",
            format!(
                "key {key}: view_mode is {mode}, expected {} (keyboard focus not on the shortcut scope?)",
                key - 1
            ),
        ) {
            ctx.app
                .ui()
                .global::<SolidPreviewModel>()
                .set_view_mode(key - 1);
            pump(100);
        }
        ctx.check_health(&format!("Solid view mode {key}"));
        ctx.sweep_main_at_sizes(&format!("Edit tab, Solid view mode {key}"));
    }
}

/// Switches Simple <-> Advanced by the header pill. Proves the advanced-only command bar
/// buttons (Deep Solve, Snapshot, Tilt Curves) disappear in Simple and return in Advanced.
fn ui_mode_simple_and_advanced(ctx: &mut Ctx<'_>) {
    const ADVANCED_ONLY: [&str; 3] = ["Deep Solve", "Snapshot", "Tilt Curves"];
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template for the interface-mode scenario",
    ) {
        return;
    }
    check_mode(ctx, false, &ADVANCED_ONLY);
    for (label, simple) in [("Simple Interface", true), ("Advanced Interface", false)] {
        let clicked = click(ctx.app, label);
        ctx.expect(
            clicked,
            "mode-pill",
            format!("the {label} pill was not found in the header"),
        );
        if !clicked {
            let ui = ctx.app.ui();
            let prefs = ui.global::<PreferencesModel>();
            prefs.set_simple_mode(simple);
            prefs.invoke_simple_mode_changed(simple);
            pump(150);
        }
        check_mode(ctx, simple, &ADVANCED_ONLY);
    }
}

/// Expects the interface to be in `simple` mode: the property says so and the advanced-only
/// controls in `advanced_only` are present exactly when it is not; sweeps the state.
fn check_mode(ctx: &mut Ctx<'_>, simple: bool, advanced_only: &[&str]) {
    let name = if simple { "Simple" } else { "Advanced" };
    let actual = ctx.app.ui().global::<PreferencesModel>().get_simple_mode();
    ctx.expect(
        actual == simple,
        "ui-mode-property",
        format!("{name}: simple_mode is {actual}"),
    );
    let view = ctx.app.main_view();
    for label in advanced_only {
        let present = view.has(label);
        ctx.expect(
            present != simple,
            "ui-mode-controls",
            format!(
                "{name} mode: control {label:?} present={present}, expected {}",
                !simple
            ),
        );
    }
    ctx.check_health(&format!("{name} mode"));
    ctx.sweep_main_at_sizes(&format!("Edit tab, {name} mode"));
}

/// The click-everything pass over top tab `index` (Live Render view for the 3D tab).
fn click_everything_on_tab(ctx: &mut Ctx<'_>, index: usize) {
    let name = TOP_TABS[index].1;
    let tab = i32::try_from(index).unwrap_or(0);
    let _stats = ctx.smoke_main(&format!("top tab {name}"), &mut |app: &mut HarnessApp| {
        select_top_tab_model(app, tab);
    });
    ctx.check_health(&format!("click-everything on {name}"));
}

/// Raises a harmless info toast (Edit as Text with no design) and proves it stays for the
/// 3.5 s display time and is gone right after (mock time).
fn toasts_dismiss(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ui.global::<RawTextModel>().invoke_open();
    pump(100);
    let shown = ui.get_toast_visible();
    ctx.expect(shown, "toast-shown", "the toast did not appear");
    let message = ui.get_toast_message().to_string();
    ctx.expect(
        message.contains("design"),
        "toast-text",
        format!("unexpected toast text {message:?}"),
    );
    ctx.check_health("toast raised");
    ctx.sweep_main_at_sizes("toast visible");
    pump(2800);
    ctx.expect(
        ui.get_toast_visible(),
        "toast-stays",
        "the toast vanished before its 3.5 s display time",
    );
    pump(1000);
    ctx.expect(
        !ui.get_toast_visible(),
        "toast-dismissed",
        "the toast is still showing 3.5 s after it appeared",
    );
    ctx.check_health("toast dismissed");
}

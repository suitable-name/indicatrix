//! Scenarios: the first-run questions and the confirm questions (Unsaved Changes, window-close
//! guard).

use super::{
    click_button, create_template_design, design_signature, edit_design, labels_like,
    request_create, request_main_close,
};
use crate::support::{
    Ctx,
    app::{pump, wait_until},
    coverage,
};
use indicatrix_cut::{EditorModel, MainWindow, PreferencesModel, UpdateModel};
use slint::ComponentHandle;
use std::time::Duration;

/// The Welcome dialog (first run) closes with "Not Now", and the new-version question that follows
/// closes with "Don't Check" (never "Check Daily").
pub(super) fn dialogs_first_run_buttons(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    let pending = ui.global::<PreferencesModel>().get_first_run_tour_pending();
    ctx.expect(
        pending,
        "welcome-pending",
        "no settings file, but no Welcome dialog is pending",
    );
    ctx.sweep_main_at_sizes("Welcome dialog");
    let view = ctx.app.main_view();
    if click_button(&view, &["Not Now"]).is_none() {
        let near = labels_like(&view, &["not now", "tour", "start"]);
        ctx.expect(
            false,
            "welcome-button",
            format!("no Not Now button in the Welcome dialog; similar labels: {near:?}"),
        );
        return;
    }
    pump(300);
    let still = ui.global::<PreferencesModel>().get_first_run_tour_pending();
    ctx.expect(
        !still,
        "welcome-closes",
        "the Welcome dialog is still pending after Not Now",
    );
    if !still {
        coverage::record_callback("TutorialsModel.tour_dismiss");
    }
    consent_question(ctx, &ui, true);
    ctx.check_health("first-run questions");
}

/// The Welcome dialog closes with Esc, and the new-version question closes with Esc too (asked
/// again next start, so it must disappear for this session).
pub(super) fn dialogs_first_run_escape(ctx: &mut Ctx<'_>) {
    let ui = ctx.app.ui().clone_strong();
    ctx.press("Escape");
    pump(300);
    let still = ui.global::<PreferencesModel>().get_first_run_tour_pending();
    ctx.expect(
        !still,
        "welcome-closes-esc",
        "the Welcome dialog is still pending after Esc",
    );
    consent_question(ctx, &ui, false);
    ctx.check_health("first-run questions with Esc");
}

/// The "Check for New Versions?" question: expected on screen, swept, then answered with
/// "Don't Check" (`by_button`) or dismissed with Esc; it must be gone afterwards.
fn consent_question(ctx: &mut Ctx<'_>, ui: &MainWindow, by_button: bool) {
    let update = ui.global::<UpdateModel>();
    ctx.expect(
        update.get_consent_pending(),
        "consent-pending",
        "the new-version question is not pending on a first run",
    );
    let view = ctx.app.main_view();
    let shown = view.has("Don't Check");
    ctx.expect(
        shown,
        "consent-shown",
        "the new-version question is not on screen after the Welcome dialog closed",
    );
    if !shown {
        return;
    }
    ctx.sweep_main_at_sizes("new-version question");
    if by_button {
        let _ = click_button(&ctx.app.main_view(), &["Don't Check"]);
    } else {
        ctx.press("Escape");
    }
    pump(300);
    let gone = !ctx.app.main_view().has("Don't Check");
    ctx.expect(
        gone,
        "consent-closes",
        format!(
            "the new-version question is still on screen after {}",
            if by_button { "Don't Check" } else { "Esc" }
        ),
    );
    if by_button {
        let kept = !update.get_consent_pending() && !update.get_check_enabled();
        ctx.expect(
            kept,
            "consent-answer-kept",
            "after Don't Check the question is still pending or the daily check is on",
        );
        if kept {
            coverage::record_callback("UpdateModel.consent_answered");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Confirm questions and popups
// ---------------------------------------------------------------------------------------------

/// The confirm questions reachable without special data. New Design over a dirty design asks
/// "Unsaved Changes": Cancel keeps the design, Discard replaces it. Closing the window with a
/// dirty design raises the close guard: Cancel keeps the window and the design, Discard closes.
pub(super) fn confirm_dialogs(ctx: &mut Ctx<'_>) {
    if !create_template_design(ctx) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    // What a Discard brings back: the same template, same options.
    let fresh = design_signature(&ui);
    if !edit_design(ctx) {
        return;
    }
    let edited = design_signature(&ui);
    ctx.expect(
        ui.global::<EditorModel>().get_is_dirty(),
        "dirty-after-edit",
        "an edited design is not marked dirty",
    );
    unsaved_changes_question(ctx, &ui, &edited, &fresh);
    window_close_guard(ctx, &ui);
}

/// New Design over a dirty design: the "Unsaved Changes" question, Cancel then Discard.
fn unsaved_changes_question(ctx: &mut Ctx<'_>, ui: &MainWindow, edited: &str, fresh: &str) {
    let editor = ui.global::<EditorModel>();
    editor.set_new_dialog_open(true);
    pump(250);
    request_create(ui);
    pump(300);
    let asked = editor.get_unsaved_dialog_open();
    ctx.expect(
        asked,
        "unsaved-question",
        "New Design over a dirty design did not ask the Unsaved Changes question",
    );
    if !asked {
        return;
    }
    ctx.sweep_main_at_sizes("Unsaved Changes question");
    let _ = click_button(&ctx.app.main_view(), &["Cancel"]);
    pump(300);
    ctx.expect(
        !editor.get_unsaved_dialog_open() && design_signature(ui) == edited,
        "unsaved-cancel-keeps",
        "Cancel on the Unsaved Changes question closed it but did not keep the edited design",
    );
    editor.set_new_dialog_open(true);
    pump(250);
    request_create(ui);
    pump(300);
    if !editor.get_unsaved_dialog_open() {
        ctx.expect(
            false,
            "unsaved-question-again",
            "the question did not come back",
        );
        return;
    }
    let _ = click_button(&ctx.app.main_view(), &["Discard"]);
    let replaced = wait_until(Duration::from_secs(10), || {
        !editor.get_unsaved_dialog_open() && design_signature(ui) == fresh
    });
    ctx.expect(
        replaced,
        "unsaved-discard-acts",
        "Discard did not replace the edited design with the new template",
    );
    if replaced {
        coverage::record_callback("EditorModel.unsaved_dialog_discard");
    }
    ctx.check_health("Unsaved Changes question");
}

/// The window-close guard: dirty design, Cancel then Discard.
fn window_close_guard(ctx: &mut Ctx<'_>, ui: &MainWindow) {
    if !edit_design(ctx) {
        return;
    }
    let edited = design_signature(ui);
    request_main_close(ui);
    let raised = ui.get_close_confirm_open() && ui.window().is_visible();
    ctx.expect(
        raised,
        "close-guard",
        "closing a dirty design did not raise the close guard (or the window closed anyway)",
    );
    if !raised {
        return;
    }
    ctx.sweep_main_at_sizes("window-close guard");
    let _ = click_button(&ctx.app.main_view(), &["Cancel"]);
    pump(250);
    ctx.expect(
        !ui.get_close_confirm_open() && ui.window().is_visible() && design_signature(ui) == edited,
        "close-guard-cancel",
        "Cancel on the close guard did not keep the window and the design",
    );
    request_main_close(ui);
    let _ = click_button(&ctx.app.main_view(), &["Discard"]);
    let closed = wait_until(Duration::from_secs(5), || !ui.window().is_visible());
    ctx.expect(
        closed,
        "close-guard-discard",
        "Discard on the close guard did not close the window",
    );
    ctx.check_health("window-close guard");
}

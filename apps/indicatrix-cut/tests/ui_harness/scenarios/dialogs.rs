//! Scenarios: the overlays and dialogs of the main window. Every overlay opens by its normal
//! route, sweeps clean at both sizes, and closes with Esc and (separate run) with its own close
//! button, handing the keyboard back to the main window; the confirm questions, the header
//! popups and the first-run questions are driven the way a user meets them.
//!
//! This file also holds the small helpers the other secondary-lane scenario files share
//! (`pub(super)`): creating a design from a template, a tier signature, clicking a button by
//! label prefix and clicking an element with the pointer.
//!
//! GAPs (kept here so the report can name them): the colour-conflict, fingerprint-mismatch,
//! write-confirm and start-up-restore questions need data the harness does not build (a file
//! saved by an older version, a leftover autosave); the command palette and the Edit-as-Text
//! "compare" sub-state have no separate close button.

mod confirm;
mod overlays;
mod popups;

use self::{
    confirm::{confirm_dialogs, dialogs_first_run_buttons, dialogs_first_run_escape},
    overlays::{dialogs_app_overlays, dialogs_editor_overlays, dialogs_library_overlays},
    popups::popups_open_and_close,
};
use super::{Scenario, existing_install, first_run};
use crate::support::{
    Control, Ctx, WindowView,
    app::{pump, wait_until},
    coverage,
};
use indicatrix_cut::{EditorModel, MainWindow, TemplateGalleryModel};
use slint::{
    ComponentHandle, Model,
    platform::{PointerEventButton, WindowEvent},
};
use slint_harness::{AccessibleRole, ElementHandle};
use std::time::Duration;

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "dialogs_editor_overlays",
            seed: existing_install,
            run: dialogs_editor_overlays,
        },
        Scenario {
            name: "dialogs_library_overlays",
            seed: existing_install,
            run: dialogs_library_overlays,
        },
        Scenario {
            name: "dialogs_app_overlays",
            seed: existing_install,
            run: dialogs_app_overlays,
        },
        Scenario {
            name: "dialogs_first_run_buttons",
            seed: first_run,
            run: dialogs_first_run_buttons,
        },
        Scenario {
            name: "dialogs_first_run_escape",
            seed: first_run,
            run: dialogs_first_run_escape,
        },
        Scenario {
            name: "confirm_dialogs",
            seed: existing_install,
            run: confirm_dialogs,
        },
        Scenario {
            name: "popups_open_and_close",
            seed: existing_install,
            run: popups_open_and_close,
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Helpers shared with shortcuts.rs, secondary_windows.rs and rough_planner.rs
// ---------------------------------------------------------------------------------------------

/// The number of tiers the editor shows.
pub(super) fn tier_count(ui: &MainWindow) -> usize {
    ui.global::<EditorModel>().get_tiers().row_count()
}

/// Every tier's angle and indices, unrounded and joined: changes with every edit that matters
/// and returns to the same text after an undo.
pub(super) fn design_signature(ui: &MainWindow) -> String {
    ui.global::<EditorModel>()
        .get_tiers()
        .iter()
        .map(|tier| format!("{}@{}", tier.angle_full, tier.indices_full))
        .collect::<Vec<_>>()
        .join("|")
}

/// Asks for the template design the New Design dialog has selected (same route as its Create
/// button: `TemplateGalleryModel.create`). `true` once the editor holds tiers; a timeout is a
/// finding.
pub(super) fn create_template_design(ctx: &mut Ctx<'_>) -> bool {
    let ui = ctx.app.ui().clone_strong();
    let made = make_template_design(&ui);
    ctx.expect(
        made,
        "timeout",
        "a template design is created: not reached within 15000 ms",
    )
}

/// [`create_template_design`] without a [`Ctx`] (for the `prepare` step of the click-everything
/// engines): `true` once the editor holds tiers.
pub(super) fn make_template_design(ui: &MainWindow) -> bool {
    let editor = ui.global::<EditorModel>();
    editor.set_new_template_index(1);
    editor.set_new_dialog_open(true);
    pump(200);
    ui.global::<TemplateGalleryModel>()
        .invoke_template_chosen(1);
    pump(100);
    request_create(ui);
    let made = wait_until(Duration::from_secs(15), || tier_count(ui) > 0);
    if made {
        coverage::record_callback("TemplateGalleryModel.create");
    }
    made
}

/// What the New Design dialog's Create button asks for.
fn request_create(ui: &MainWindow) {
    let gallery = ui.global::<TemplateGalleryModel>();
    gallery.invoke_create(
        ui.global::<EditorModel>().get_new_template_index(),
        gallery.get_gear_index(),
        gallery.get_material_index(),
        gallery.get_width_text(),
    );
}

/// Edits tier 0 (an undoable angle nudge) and waits until the design's signature changes. `true`
/// when it did; otherwise a finding.
pub(super) fn edit_design(ctx: &mut Ctx<'_>) -> bool {
    let ui = ctx.app.ui().clone_strong();
    let changed = nudge_first_tier(&ui);
    ctx.expect(
        changed,
        "edit-design",
        "nudging tier 0 did not change the design (setup of the scenario failed)",
    )
}

/// [`edit_design`] without a [`Ctx`].
pub(super) fn nudge_first_tier(ui: &MainWindow) -> bool {
    let before = design_signature(ui);
    ui.global::<EditorModel>().invoke_nudge_angle(0, 0.5);
    let changed = wait_until(Duration::from_secs(5), || design_signature(ui) != before);
    if changed {
        coverage::record_callback("EditorModel.nudge_angle");
    }
    changed
}

/// Activates the last enabled button whose label starts with one of `prefixes` (tried in order);
/// the last one because overlays come after the window's own content. Returns its label.
pub(super) fn click_button(view: &WindowView, prefixes: &[&str]) -> Option<String> {
    let controls: Vec<Control> = view.controls();
    for prefix in prefixes {
        let hit = controls
            .iter()
            .rev()
            .find(|c| c.role == AccessibleRole::Button && c.enabled && c.label.starts_with(prefix));
        if let Some(control) = hit {
            coverage::record_activation(&control.handle);
            control.handle.invoke_accessible_default_action();
            return Some(control.label.clone());
        }
    }
    None
}

/// The labels in `view` that contain one of `words` (case-insensitive): what a failed lookup
/// reports.
pub(super) fn labels_like(view: &WindowView, words: &[&str]) -> Vec<String> {
    view.labels()
        .into_iter()
        .filter(|label| {
            let lower = label.to_lowercase();
            words.iter().any(|w| lower.contains(&w.to_lowercase()))
        })
        .take(15)
        .collect()
}

/// Clicks the middle of `element` with the pointer (move, press, release), as a user would.
pub(super) fn click_center(ui: &MainWindow, element: &ElementHandle) {
    let position = element.absolute_position();
    let size = element.size();
    let at = slint::LogicalPosition::new(
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    );
    let window = ui.window();
    window.dispatch_event(WindowEvent::PointerMoved { position: at });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    pump(50);
}

/// Asks the main window to close (the title-bar button).
pub(super) fn request_main_close(ui: &MainWindow) {
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    pump(150);
}

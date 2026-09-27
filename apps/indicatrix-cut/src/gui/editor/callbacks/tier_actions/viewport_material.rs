//! The viewport's "Linked to design" checkbox and the printed-proportions panel.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{refresh_editor_panel_stale, sync_viewport_material_link},
        },
        show_toast,
    },
};

/// The viewport's "Linked to design" checkbox -- when switched ON, syncs the shared
/// viewport's render material to the design's own material IMMEDIATELY, so turning it
/// on feels responsive. `refresh_design_settings` keeps it in sync from then on.
///
/// Calls [`view::sync_viewport_material_link`] directly (`pub(super)`) rather than
/// the heavier `refresh_editor_panel_stale`, which would wipe the solved-state
/// banner, MAST/SOLVE figures, warnings and yield report and schedule a full
/// background re-solve for what is a display-only toggle that never touches
/// `Design`. Calling the real function directly, rather than a narrower copy,
/// also keeps the Render Material dropdown's own displayed index and
/// `stone_width_mm` in sync alongside `material_name` -- all three must move
/// together, or turning "Linked" on after picking a different material in the
/// dropdown would leave the dropdown and the absorption-path scaling both stale.
pub(in crate::gui::editor) fn setup_viewport_material_linked_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state_linked = Rc::clone(state);
    let render_ctx_linked = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_viewport_material_linked_changed(move |linked: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if linked {
                let st = state_linked.borrow();
                let selected_material_index = {
                    let mut ctx = render_ctx_linked
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    sync_viewport_material_link(&ui, &mut ctx, &st.design)
                };
                drop(st);
                // Set only after the `render_ctx` guard above is
                // dropped -- see `sync_viewport_material_link`'s own doc comment.
                if let Some(idx) = selected_material_index {
                    ui.global::<ViewportModel>()
                        .set_selected_material_index(idx);
                }
            }
        });

    // Registered here, alongside the viewport-link callback
    // above, rather than as its own `setup_*` function -- both need exactly
    // `(ui, state, render_ctx)`, and a new registration needs no new call site in
    // `gui::editor::mod`'s hub the way a new function would. Wired to
    // `EditorModel.apply_printed_proportions` (`ui/models/editor.slint`), called
    // from `editor_design_settings.slint`.
    let state_props = Rc::clone(state);
    let render_ctx_props = Arc::clone(render_ctx);
    let ui_weak_props = ui.as_weak();
    ui.global::<EditorModel>().on_apply_printed_proportions(
        move |vol_w3: SharedString,
              lw: SharedString,
              cw: SharedString,
              pw: SharedString,
              hw: SharedString| {
            let Some(ui) = ui_weak_props.upgrade() else {
                return;
            };
            let props = match parse_printed_proportions_form(&vol_w3, &lw, &cw, &pw, &hw) {
                Ok(props) => props,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let has_any = props.vol_w3.is_some()
                || props.lw.is_some()
                || props.cw.is_some()
                || props.pw.is_some()
                || props.hw.is_some();
            let mut st = state_props.borrow_mut();
            st.printed_proportions = has_any.then_some(props);
            // A printed-proportions edit never moves a tier's own mast.
            refresh_editor_panel_stale(&ui, &render_ctx_props, &st, &BTreeSet::new());
            drop(st);
            show_toast(
                &ui,
                if has_any {
                    "Printed proportions saved -- Deep Solve can now verify against them."
                } else {
                    "Printed proportions cleared."
                },
                "success",
            );
        },
    );
}

/// One printed-proportions field's text, parsed as `None` for blank text or
/// `Some(value)` for a finite positive number -- shared by
/// [`parse_printed_proportions_form`] across all five fields.
///
/// # Errors
///
/// A ready-to-toast message naming `label` when `text` is non-blank but does not
/// parse as a finite positive number.
///
/// `pub(super)` since [`super::tests`] exercises this directly.
pub(super) fn parse_printed_proportions_field(
    label: &str,
    text: &str,
) -> Result<Option<f64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: f64 = trimmed
        .parse()
        .map_err(|_| format!("{label} '{trimmed}' is not a number."))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be a positive number."));
    }
    Ok(Some(value))
}

/// Parses the printed-proportions panel's five text fields
/// into an [`ExternalProportions`], the exact shape
/// `loading::external_proportions_from_full_record` already
/// builds from a catalogue row's own measured columns -- see that function's own
/// doc comment for the target this feeds ([`EditorState::printed_proportions`],
/// Deep Solve's external verification).
///
/// # Errors
///
/// The first field (in `vol_w3, lw, cw, pw, hw` order) that fails to parse, via
/// [`parse_printed_proportions_field`].
///
/// `pub(super)` for the same reason as [`parse_printed_proportions_field`] above.
pub(super) fn parse_printed_proportions_form(
    vol_w3: &str,
    lw: &str,
    cw: &str,
    pw: &str,
    hw: &str,
) -> Result<indicatrix::geometry::stone_metrics::ExternalProportions, String> {
    Ok(indicatrix::geometry::stone_metrics::ExternalProportions {
        vol_w3: parse_printed_proportions_field("Vol/W\u{b3}", vol_w3)?,
        lw: parse_printed_proportions_field("L/W", lw)?,
        cw: parse_printed_proportions_field("C/W", cw)?,
        pw: parse_printed_proportions_field("P/W", pw)?,
        hw: parse_printed_proportions_field("H/W", hw)?,
    })
}

//! Small standalone callbacks (tier filter, angle live-preview, anchor-explainer
//! dismiss) and the selection-index bookkeeping helpers shared across this module
//! group.

use std::{cell::RefCell, rc::Rc};

use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    gui::editor::state::{
        EditorState, representative_crown_and_pavilion_angles_deg, tier_matches_filter,
    },
};

/// The tier list's filter box asks Rust whether one row matches, because Slint's
/// `string` type has no substring test. A `pure` callback, so the table can call it
/// straight from a per-row binding.
pub(in crate::gui::editor) fn setup_tier_filter_callback(ui: &MainWindow) {
    ui.global::<EditorModel>().on_tier_matches_filter(
        |haystack: SharedString, filter: SharedString| tier_matches_filter(&haystack, &filter),
    );
}

/// Live critical-angle guidance in the Tier form's Angle field (see
/// `EditorModel::angle_live_preview`'s own doc comment on `ui/models/editor.slint`).
/// Parses `text` (the field's own
/// live content) as a signed degree value: a pavilion angle (`< 0`) reads the
/// plain table-only critical-angle margin
/// (`state::tier_margin_and_risk`'s own pavilion branch, via
/// [`indicatrix_cut_core::tier_margin_deg`]/[`indicatrix_cut_core::windowing_risk`]);
/// a crown angle (`> 0`) reads the crown-window ESTIMATE
/// ([`indicatrix_cut_core::crown_window_margin_deg`]/
/// [`indicatrix_cut_core::crown_windowing_risk`]) against the design's own
/// representative pavilion angle
/// ([`representative_crown_and_pavilion_angles_deg`]) --
/// the exact same functions a saved row's MARGIN cell uses, so a value typed
/// but not yet saved reads the identical bar. Uses the design's plain
/// [`indicatrix_cut_core::Design::effective_refractive_index`] (built-in
/// materials only, no custom-catalogue lookup) rather than threading
/// `RenderContext` through for this preview-only path -- the authoritative
/// "Eff. RI"/critical-angle readouts elsewhere already use the full
/// custom-material-aware value; this is a live estimate while typing, not the
/// figure of record. An unparseable/blank/zero angle, or a crown angle with no
/// pavilion tier in the design to estimate against, clears the bar
/// (`level = -1`).
pub(in crate::gui::editor) fn setup_angle_live_preview_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_angle_live_preview(move |text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<EditorModel>();
            let clear = || {
                model.set_angle_live_margin_text(SharedString::new());
                model.set_angle_live_margin_level(-1);
                model.set_angle_live_margin_is_estimate(false);
            };
            let Ok(angle_deg) = text.trim().parse::<f64>() else {
                clear();
                return;
            };
            let st = state.borrow();
            let n_d = st.design.effective_refractive_index();
            if angle_deg < 0.0 {
                let margin = indicatrix_cut_core::tier_margin_deg(angle_deg, n_d);
                let level = match indicatrix_cut_core::windowing_risk(angle_deg, n_d) {
                    indicatrix_cut_core::Risk::Safe => 0,
                    indicatrix_cut_core::Risk::Marginal => 1,
                    indicatrix_cut_core::Risk::Windows => 2,
                };
                model.set_angle_live_margin_text(format!("{margin:+.1}\u{b0}").into());
                model.set_angle_live_margin_level(level);
                model.set_angle_live_margin_is_estimate(false);
            } else if angle_deg > 0.0 {
                let (_, pavilion_deg) = representative_crown_and_pavilion_angles_deg(&st.design);
                let Some(pavilion_deg) = pavilion_deg else {
                    clear();
                    return;
                };
                let margin =
                    indicatrix_cut_core::crown_window_margin_deg(pavilion_deg, angle_deg, n_d);
                let level =
                    match indicatrix_cut_core::crown_windowing_risk(pavilion_deg, angle_deg, n_d) {
                        indicatrix_cut_core::Risk::Safe => 0,
                        indicatrix_cut_core::Risk::Marginal => 1,
                        indicatrix_cut_core::Risk::Windows => 2,
                    };
                model.set_angle_live_margin_text(format!("{margin:+.1}\u{b0}").into());
                model.set_angle_live_margin_level(level);
                model.set_angle_live_margin_is_estimate(true);
            } else {
                clear();
            }
        });
}

/// The anchor explainer card's "Got it" / "Don't show again" dismiss buttons:
/// "Don't show again" additionally persists the suppression (`state::anchor_explainer_suppress_permanently`,
/// `AppSettings::suppressed_confirmations` key `"anchor_explainer"`) so
/// `state::should_open_anchor_explainer` never reopens it again, on top of the
/// session-scoped "already shown once" guard that function already applies.
pub(in crate::gui::editor) fn setup_anchor_explainer_dismiss_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_anchor_explainer_dismiss(move |dont_show_again: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_anchor_explainer_open(false);
            if dont_show_again {
                crate::gui::editor::state::anchor_explainer_suppress_permanently();
            }
        });
}

/// Bumps `EditorModel.form_reset_pulse` -- see that property's own doc comment
/// (`ui/models/editor.slint`) for why a plain increment, not a flag, is what makes
/// `EditorView`'s watcher fire even when `selected_tier_index` is already `-1` (so
/// setting it to `-1` again would raise no `changed` on its own).
///
/// `pub(super)` since the design-lifecycle and tier-CRUD callbacks share it.
pub(super) fn bump_form_reset_pulse(ui: &MainWindow) {
    let pulse = ui.global::<EditorModel>().get_form_reset_pulse();
    ui.global::<EditorModel>()
        .set_form_reset_pulse(pulse.wrapping_add(1));
}

/// Adjusts `EditorModel.selected_tier_index` after [`Edit::RemoveTier`] removes the
/// tier at `removed_index`, shifting every later tier down by one (see
/// `indicatrix_cut_core`'s own `edit::apply` -- read-only here): equal to the removed
/// index, the selected tier no longer exists, so the selection is cleared and
/// [`bump_form_reset_pulse`] guarantees `EditorView`'s form still blanks even if the
/// selection was already `-1`; greater than it, the same tier survived one position
/// lower, so the index is shifted down to keep pointing at it (this fires
/// `EditorView`'s `changed tracked_selected_tier_index` for real, re-seeding the
/// form from the right row). Anything else (less than, or already `-1`) is left
/// untouched.
///
/// `pub(super)` since [`super::tier_crud::setup_remove_tier_callback`] uses it.
pub(super) fn adjust_selection_after_remove(ui: &MainWindow, removed_index: i32) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    if selected == removed_index {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    } else if selected > removed_index {
        ui.global::<EditorModel>()
            .set_selected_tier_index(selected - 1);
    }
}

/// Clamps `EditorModel.selected_tier_index` after an undo/redo to `tier_count`
/// (`design.tiers.len()` post-replay). Unlike [`adjust_selection_after_remove`],
/// undo/redo can change the tier count by any amount in either direction (an
/// `AddTier`/`RemoveTier` reversed, or several tiers' worth of a coalesced
/// `RetargetAngles`), so there is no single shifted-by-one relationship to
/// preserve -- an index that no longer names a real tier is simply cleared, with
/// the same [`bump_form_reset_pulse`] guarantee.
///
/// `pub(super)` since [`super::history`]'s Undo/Redo callbacks use it.
pub(super) fn clamp_selection_to_tier_count(ui: &MainWindow, tier_count: usize) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let out_of_range = usize::try_from(selected).is_ok_and(|index| index >= tier_count);
    if out_of_range {
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        bump_form_reset_pulse(ui);
    }
}

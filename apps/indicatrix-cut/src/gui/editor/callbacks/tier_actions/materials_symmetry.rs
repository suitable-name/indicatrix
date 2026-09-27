//! The design settings panel's material combo/RI-override, the inferred-material
//! guess "Set material" action, and Symmetry/Mirror Apply (with its live preview).

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::Edit;
use slint::{ComponentHandle, Model, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            guide, loading,
            state::{
                EditorState, parse_design_material_form, tiers_incomplete_under_proposed_symmetry,
            },
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// Whether a plain material
/// pick (`picked_name`, with no typed RI override of its own) should pin
/// [`MaterialSelection::refractive_index_override`] to the design's legacy
/// exported RI (`legacy_ri`, i.e. `meta.refractive_index` -- the schedule's own
/// `I` line) so the pick does not silently rewrite it. Returns `None` (pick
/// stays unpinned, the newly resolved material's own `n_D` wins) whenever
/// `previous_material_name` is `Some`: once a design already has a NAMED
/// material, that material's own `n_D` (or an explicit typed override) is the
/// design's RI story from then on, and pinning the OUTGOING material's RI onto
/// the incoming one would be exactly the bug this guard exists to prevent --
/// see [`loading::ri_override_to_preserve`] for the tolerance check itself.
///
/// `pub(super)` since [`super::tests`] exercises this directly.
pub(super) fn ri_override_for_material_pick(
    picked_name: Option<&str>,
    previous_material_name: Option<&str>,
    legacy_ri: f64,
) -> Option<f64> {
    if previous_material_name.is_some() {
        return None;
    }
    loading::ri_override_to_preserve(picked_name?, legacy_ri)
}

/// The design settings panel's material combo + RI override field -- applies
/// [`Edit::SetMaterial`] via [`parse_design_material_form`], reading the combo's
/// current option list from `editor_material_combo_options` (pushed fresh every
/// refresh, so this always parses against the SAME list the user actually saw).
pub(in crate::gui::editor) fn setup_apply_design_material_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_design_material(
        move |combo_index: i32, ri_override_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let options: Vec<String> = ui
                .global::<EditorModel>()
                .get_material_combo_options()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut st = state.borrow_mut();
            match parse_design_material_form(
                combo_index,
                &ri_override_text,
                &options,
                &st.design.material,
            ) {
                Ok(mut material) => {
                    // A plain material
                    // pick (no typed RI override -- that path is left alone, it
                    // is an explicit choice) must not silently change what the
                    // exported I line reads, but must also not pin the OUTGOING
                    // material's RI onto the incoming one. See
                    // `ri_override_for_material_pick`'s own doc comment.
                    if material.refractive_index_override.is_none() {
                        material.refractive_index_override = ri_override_for_material_pick(
                            material.name.as_deref(),
                            st.design.material.name.as_deref(),
                            st.design.meta.refractive_index,
                        );
                    }
                    match st.apply(Edit::SetMaterial { material }) {
                        Ok(()) => {
                            // A material change never moves a tier's own mast.
                            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                            // The guide's "Pick a real material" step completes here.
                            guide::check_progress(&ui, &st);
                            submit_preview_replan(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                &st,
                                BTreeSet::new(),
                                false,
                            );
                        }
                        Err(e) => show_toast(&ui, &e.to_string(), "error"),
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Set material" on an inferred-material guess (the "Inferred material shown as
/// a guess, never as a fact" principle) -- writes `name` into the design's
/// [`indicatrix_cut_core::MaterialSelection`]
/// as an ordinary, undoable `Edit::SetMaterial`, keeping the design's own
/// current specific-gravity/RI-override fields untouched (only the NAME
/// changes -- this is "confirm the guess", not "reconfigure the material").
/// Once a name is set, `view::refresh_design_settings` stops computing a
/// guess at all (`design.material.name.is_some()`), so the guess label
/// disappears and the next native save carries the confirmed name through
/// `design.material`.
pub(in crate::gui::editor) fn setup_material_guess_set_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_set_material_from_guess(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut material = st.design.material.clone();
            material.name = Some(name.to_string());
            match st.apply(Edit::SetMaterial { material }) {
                Ok(()) => {
                    // A material name change never moves a tier's own mast.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                    show_toast(&ui, &format!("Material set to {name}."), "success");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The design settings panel's Symmetry/Mirror "Apply" -- wholesale
/// [`Edit::SetSchedule`], keeping the design's CURRENT gear (this control never
/// changes gear -- that's [`setup_gear_apply_callback`]'s job, since only a gear
/// change needs the remap confirmation). Also registers
/// [`EditorModel::on_request_symmetry_preview`]: a live
/// dry-run preview of the SAME proposed change, computed as the Symmetry Order
/// field is edited or Mirror is toggled, so switching (say) 8-fold to 6-fold no
/// longer turns rows amber with no warning and no chance to reconsider before
/// clicking Apply -- mirroring the gear-remap path's own dry-run preview
/// (`gear_remap_preview`/[`setup_gear_apply_callback`]). Registered here rather
/// than as its own `setup_*` function so it can share this function's own
/// `state` clone instead of this module's registration point (`mod.rs`, a
/// different lane's file right now) needing a new call site.
pub(in crate::gui::editor) fn setup_apply_symmetry_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    //    // `on_apply_symmetry` below moves the ORIGINAL `state` binding into its
    // own closure, not after (that closure is `move`, so `state` is gone once
    // it is constructed).
    let state_preview = Rc::clone(&state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_symmetry(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let symmetry_order: u32 = match symmetry_order_text.trim().parse() {
                Ok(v) if v >= 1 => v,
                _ => {
                    show_toast(
                        &ui,
                        "Symmetry order must be a positive whole number.",
                        "error",
                    );
                    return;
                }
            };
            let mut st = state.borrow_mut();
            let gear_teeth = st.design.meta.gear_teeth;
            match st.apply(Edit::SetSchedule {
                gear_teeth,
                symmetry_order,
                mirror,
            }) {
                Ok(()) => {
                    // Symmetry/mirror can move every tier's index-wheel position, not
                    // tracked precisely here, so force a full (non-blocking) solve --
                    // and trust no cached mast either.
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &(0..st.design.tiers.len()).collect(),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        true,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );

    //    // comment above.
    let ui_weak_preview = ui.as_weak();
    ui.global::<EditorModel>().on_request_symmetry_preview(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak_preview.upgrade() else {
                return;
            };
            // An unparsable/zero symmetry order can never be applied (see the
            // real `on_apply_symmetry` handler's own validation above) -- no
            // preview to show rather than a stale or misleading one.
            let Ok(symmetry_order) = symmetry_order_text.trim().parse::<u32>() else {
                ui.global::<EditorModel>()
                    .set_symmetry_preview_text(String::new().into());
                return;
            };
            if symmetry_order == 0 {
                ui.global::<EditorModel>()
                    .set_symmetry_preview_text(String::new().into());
                return;
            }
            let st = state_preview.borrow();
            let incomplete =
                tiers_incomplete_under_proposed_symmetry(&st.design, symmetry_order, mirror);
            let text = if incomplete == 0 {
                "No tiers would become incomplete orbits.".to_string()
            } else {
                let plural = if incomplete == 1 { "" } else { "s" };
                format!("{incomplete} tier{plural} would become incomplete orbits.")
            };
            ui.global::<EditorModel>()
                .set_symmetry_preview_text(text.into());
        },
    );
}

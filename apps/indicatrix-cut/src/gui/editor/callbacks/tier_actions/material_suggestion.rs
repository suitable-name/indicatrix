//! The catalogue material-suggestion banner's Accept/Dismiss callbacks.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::Edit;
use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            loading,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The material-suggestion banner's "Set Material" action -- see
/// `setup_load_selected_callback` for when this banner is populated. Applies
/// [`loading::material_selection_for_accepted_suggestion`]'s
/// [`indicatrix_cut_core::MaterialSelection`] (pinning `refractive_index_override` to
/// the schedule's own recorded RI whenever the suggested built-in's `n_D` would
/// otherwise silently move it by more than 0.01) as one undoable
/// [`Edit::SetMaterial`], then clears the banner.
pub(in crate::gui::editor) fn setup_material_suggestion_accept_callback(
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
        .on_material_suggestion_accept(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let schedule_ri = st.design.meta.refractive_index;
            let material = loading::material_selection_for_accepted_suggestion(
                &name,
                schedule_ri,
                &st.design.material,
            );
            match st.apply(Edit::SetMaterial { material }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    // Material-only: geometry is unchanged, only the critical-angle
                    // overlay (which depends on n_D) needs a fresh render.
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
            drop(st);
            ui.global::<EditorModel>()
                .set_material_suggestion_name("".into());
            ui.global::<EditorModel>()
                .set_material_suggestion_text("".into());
        });
}

/// The material-suggestion banner's dismiss ("No thanks") action -- just clears the
/// banner; the schedule's material/RI stay exactly as loaded.
///
/// Just clears the banner; mirrors the Accept path's own clearing above.
pub(in crate::gui::editor) fn setup_material_suggestion_dismiss_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_material_suggestion_dismiss(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<EditorModel>()
                .set_material_suggestion_name("".into());
            ui.global::<EditorModel>()
                .set_material_suggestion_text("".into());
        });
}

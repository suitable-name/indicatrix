//! "Create" on the New Design dialog, and the actual replacement work it (and the
//! unsaved-changes guard's resume) shares.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix_cut_core::FreshDesignSpec;
use slint::{ComponentHandle, SharedString};

use super::{FIXED_CYLINDER_PREFORM_SIDES, misc::bump_form_reset_pulse};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            callbacks::solve_actions::clear_analysis_results,
            guide, loading,
            stall_guard::stall_guard,
            state::{EditorState, PendingUnsavedAction, gear_choice_to_teeth},
            view::{SolidLastSolved, push_has_design, refresh_all_now},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// "Create" on the New Design dialog -- replaces the editor state with a brand-new
/// design built from the dialog's preform/gear/symmetry/mirror/material fields, via
/// `Design::fresh_from_spec`. Discards the previous design and its undo/redo history
/// entirely -- there is nothing to preserve across a deliberate "start over", so once
/// the fields parse, this checks [`EditorState::is_dirty`] before actually discarding
/// anything: a dirty design stashes [`PendingUnsavedAction::New`] and opens the
/// Save/Discard/Cancel guard instead of proceeding straight to [`do_new_design_create`]
/// -- see `setup_unsaved_guard_dispatch` for how "Save"/"Discard" resume it.
/// Validation runs BEFORE that check (matching `native_io::do_open_native`'s own
/// ordering) so a form error still surfaces immediately rather than behind a
/// confirmation dialog for a create that was never going to succeed anyway.
/// Opening/closing the dialog is pure Slint state; this only fires on "Create".
pub(in crate::gui::editor) fn setup_new_design_create_callback(
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
    ui.global::<EditorModel>().on_new_design_create(
        move |shape_index: i32,
              half_width: SharedString,
              length_over_width: SharedString,
              depth: SharedString,
              gear_preset_index: i32,
              gear_custom_text: SharedString,
              symmetry_order_text: SharedString,
              mirror: bool,
              material_index: i32,
              template_index: i32| {
            stall_guard("on_new_design_create", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let gear_teeth = match gear_choice_to_teeth(gear_preset_index, &gear_custom_text) {
                    Ok(t) => t,
                    Err(e) => {
                        show_toast(&ui, &e, "error");
                        return;
                    }
                };
                let preform = match loading::parse_preform_form(
                    shape_index,
                    &half_width,
                    &length_over_width,
                    &depth,
                    FIXED_CYLINDER_PREFORM_SIDES,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        show_toast(&ui, &e, "error");
                        return;
                    }
                };
                match loading::parse_new_design_form(
                    gear_teeth,
                    preform,
                    &symmetry_order_text,
                    mirror,
                    material_index,
                ) {
                    Ok(spec) => {
                        if state.borrow().is_dirty() {
                            state.borrow_mut().pending_unsaved_action =
                                Some(PendingUnsavedAction::New {
                                    spec,
                                    template_index,
                                });
                            ui.global::<EditorModel>().set_unsaved_dialog_message(
                                "Starting a new design will discard the current one's unsaved \
                                 changes."
                                    .into(),
                            );
                            ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                        } else {
                            do_new_design_create(
                                &ui,
                                &state,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                spec,
                                template_index,
                            );
                        }
                    }
                    Err(e) => show_toast(&ui, &e, "error"),
                }
            });
        },
    );
}

/// The actual "New" work, run either directly (a clean design) or as
/// [`PendingUnsavedAction::New`]'s resume once the Save/Discard/Cancel guard clears --
/// see [`setup_new_design_create_callback`]'s own doc comment for why the dirty check
/// runs before this is ever called, not inside it.
///
/// `pub(super)` since [`super::lifecycle::resume_pending_unsaved_action`] is the
/// other call site.
pub(super) fn do_new_design_create(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    spec: FreshDesignSpec,
    // The "Start from" choice -- see [`install_new_design`].
    template_index: i32,
) {
    install_new_design(&mut state.borrow_mut(), spec, template_index);
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
    // See `apply_loaded_design`'s matching reset -- "New" replaces the
    // whole `EditorState` exactly the same way.
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    ui.global::<EditorModel>().set_new_dialog_open(false);
    // Explicit, same reasoning as `native_io::commit_loaded_native`'s own matching
    // line: a freshly `fresh_from_spec` design starts clean by construction.
    ui.global::<EditorModel>().set_is_dirty(false);
    // A brand-new design has no file behind it yet, so the window title drops the
    // name entirely rather than keeping whatever was open before.
    ui.set_loaded_design_name(SharedString::new());
    // Right away, not only via `refresh_all`, which `refresh_all_now` may defer by
    // one event-loop tick: the empty-state card grid must give way to the new
    // design, even an Empty one with zero tiers.
    push_has_design(ui, &state.borrow());
    // The worked-example guide's first step completes on exactly this: a design
    // was created. Only this success path reaches here, never a rejected form.
    guide::notify(ui, guide::NEW_DESIGN_CREATED);
}

/// The state half of [`do_new_design_create`], with no window involved: replaces
/// `st` wholesale with a fresh design built from `spec`, marks it a real design
/// ([`EditorState::has_design`]) whatever the template -- template `0`, "Empty", is
/// a real design with zero tiers, not the startup placeholder -- and seeds
/// template `template_index`'s tiers when it names one.
///
/// `template_index`: 0 is "Empty"; 1..=N indexes
/// `indicatrix_cut_core::templates::TEMPLATES` at `template_index - 1` (index 1 is
/// "Standard Round Brilliant", `TEMPLATES[0]`, matching the gallery's own display
/// order -- see `gui/editor/templates.rs::setup_template_gallery`). Any index the
/// table has no entry for (0, a negative value, or one past the end) is treated as
/// empty rather than panicking -- the combo/gallery is the only producer, but a
/// stale index must not lose a design.
pub(super) fn install_new_design(st: &mut EditorState, spec: FreshDesignSpec, template_index: i32) {
    // `replace_wholesale`, not a plain `*st = ...`: carries this state's own
    // `generation` `Arc` across the replacement (and bumps it) instead of letting
    // `fresh_from_spec` hand back a brand-new one, so a background Deep
    // Solve/Optimize/auto-solve dispatched against the design being replaced still
    // observes that it changed (see that method's own doc comment).
    st.replace_wholesale(EditorState::fresh_from_spec(spec));
    // Seeded AFTER the replacement, directly on the fresh design, rather than
    // through `History`: this is the design's starting state, not an edit to it,
    // so it must not be undoable back to an empty schedule the cutter never saw.
    // `saved_generation` already matches, so the new design still reads as clean.
    // `usize::try_from` refuses a negative index (0 = "Empty", or a stale/corrupt
    // value) instead of panicking on the cast.
    if let Ok(table_index) = usize::try_from(template_index - 1)
        && let Some(template) = indicatrix_cut_core::templates::TEMPLATES.get(table_index)
    {
        st.design.tiers = template.tiers();
    }
    // `fresh_from_spec` already says so; restated here because this is the promise
    // the empty-state overlay depends on.
    st.has_design = true;
}

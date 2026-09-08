//! The tier/undo/redo/preform/yield edit callbacks ([`tier_actions`]), and the Deep
//! Solve/Optimize/Adopt start/cancel/apply callbacks ([`solve_actions`]) -- one
//! `setup_*` function per Slint callback, wired up by `super::setup_editor_callbacks`.
//! See this group's own `mod.rs` doc comment for the "`History` is the only thing that
//! mutates `Design`" rule every callback here that edits the design upholds via
//! `EditorState::apply`.
//!
//! Split purely to keep this file from growing further -- along the same seam the two
//! files' own doc comments already draw: the everyday tier/history editing callbacks
//! versus the two off-thread, cancellable, explicit actions (Deep Solve, Optimize) and
//! the "Adopt" action tied to Deep Solve's own suggestions.

mod retarget_actions;
mod solve_actions;
mod tier_actions;

pub(in crate::gui::editor) use retarget_actions::{
    setup_retarget_apply_callback, setup_retarget_close_callback, setup_retarget_open_callback,
    setup_retarget_proposal_changed_callback,
};
pub(in crate::gui::editor) use solve_actions::{
    setup_adopt_meet_callback, setup_deep_solve_callback, setup_deep_solve_cancel_callback,
    setup_optimize_apply_callback, setup_optimize_callback, setup_optimize_cancel_callback,
};
pub(in crate::gui::editor) use tier_actions::{
    setup_apply_design_material_callback, setup_apply_preform_callback,
    setup_apply_symmetry_callback, setup_apply_yield_inputs_callback,
    setup_duplicate_tier_callback, setup_gear_apply_callback, setup_gear_remap_cancel_callback,
    setup_gear_remap_confirm_callback, setup_inline_set_angle_callback,
    setup_load_selected_callback, setup_material_suggestion_accept_callback,
    setup_material_suggestion_dismiss_callback, setup_new_design_create_callback,
    setup_nudge_angle_callback, setup_redo_callback, setup_remove_tier_callback,
    setup_save_tier_callback, setup_solid_facet_click_callback, setup_solid_facet_hover_callback,
    setup_solid_selected_tier_changed_callback, setup_solve_callback, setup_toggle_detach_callback,
    setup_toggle_multi_select_callback, setup_undo_callback,
    setup_viewport_material_linked_changed_callback,
};

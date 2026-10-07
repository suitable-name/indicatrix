//! The Solve/New/Load-Selected/Undo/Redo/tier/preform/yield-input edit callbacks --
//! one `setup_*` function per Slint callback. See this group's `mod.rs` doc comment
//! for the "`History` is the only thing that mutates `Design`" rule every callback
//! here upholds via `EditorState::apply`.
//!
//! Split into one file per user-facing concern: design lifecycle
//! ([`lifecycle`]/[`new_design`]/[`material_suggestion`]), undo/redo ([`history`]), design-wide
//! forms ([`design_forms`]), the Save Tier form ([`tier_form`]), angle nudging
//! ([`nudge`]), tier CRUD ([`tier_crud`]), generated tier series
//! ([`tier_generation`]), multi-select ([`selection`]), Adopt/Pin ([`adopt`]),
//! concave tiers ([`concave_tier`]), per-facet index editing ([`facet_editing`]), material/symmetry settings
//! ([`materials_symmetry`]), gear remap ([`gear_remap`]), viewport material linking
//! ([`viewport_material`]), solid-view facet picking ([`solid_picking`]), and small
//! shared helpers ([`misc`]) -- plus the shared facet-overlay state
//! ([`facet_overlay`]) every selection/picking path resubmits through.

mod adopt;
mod concave_tier;
mod design_forms;
mod facet_editing;
mod facet_overlay;
mod gear_remap;
mod history;
mod lifecycle;
mod material_suggestion;
mod materials_symmetry;
mod misc;
mod new_design;
mod nudge;
mod selection;
mod solid_picking;
#[cfg(test)]
mod tests;
mod tier_crud;
mod tier_form;
mod tier_generation;
mod viewport_material;

// The remaining `pub(super)` items in this group's own files (e.g. every
// `setup_*_callback` piggybacked from `tier_crud::setup_toggle_detach_callback`
// or `selection::setup_toggle_multi_select_callback`) are reached directly via
// `super::<module>::<item>` from their one call site and never need to be named
// here -- only the callbacks `callbacks::mod`'s own hub imports are re-exported
// below.
pub(in crate::gui::editor) use design_forms::{
    setup_apply_cheater_offset_callback, setup_apply_design_meta_callback,
    setup_apply_preform_callback, setup_apply_preform_y_offset_callback,
    setup_apply_tier_note_callback, setup_apply_yield_inputs_callback,
};
pub(in crate::gui::editor) use gear_remap::{
    setup_gear_apply_callback, setup_gear_remap_cancel_callback, setup_gear_remap_confirm_callback,
};
pub(in crate::gui::editor) use history::{
    HistoryMove, refresh_after_history_move, refresh_after_renumbering_move, setup_redo_callback,
    setup_undo_callback,
};
pub(in crate::gui::editor) use lifecycle::{setup_load_selected_callback, setup_solve_callback};
pub(in crate::gui::editor) use material_suggestion::{
    setup_material_suggestion_accept_callback, setup_material_suggestion_dismiss_callback,
};
pub(in crate::gui::editor) use materials_symmetry::{
    setup_apply_design_material_callback, setup_apply_symmetry_callback,
    setup_material_guess_set_callback,
};
pub(in crate::gui::editor) use misc::{
    setup_anchor_explainer_dismiss_callback, setup_angle_live_preview_callback,
    setup_tier_filter_callback,
};
pub(in crate::gui::editor) use new_design::setup_new_design_create_callback;
pub(in crate::gui::editor) use nudge::{
    setup_inline_set_angle_callback, setup_nudge_angle_callback,
};
pub(in crate::gui::editor) use selection::setup_toggle_multi_select_callback;
// The facet overlay's merge-then-resubmit primitive, shared with the manipulation
// module (`gui::editor::manipulate`), which outlines the tiers a handle drag moves.
pub(in crate::gui::editor) use facet_overlay::resubmit_facet_overlay;
pub(in crate::gui::editor) use solid_picking::{
    map_to_pick_coordinates, pick_margin, pick_pixels_to_logical, selected_facet_id,
    setup_solid_facet_click_callback, setup_solid_facet_hover_callback,
    setup_solid_selected_tier_changed_callback,
};
// The pick-frame arithmetic the manipulation module's tests round-trip.
#[cfg(test)]
pub(in crate::gui::editor) use solid_picking::{letterbox_margin, logical_to_pick_pixels};
pub(in crate::gui::editor) use tier_crud::{
    setup_duplicate_tier_callback, setup_remove_tier_callback, setup_toggle_detach_callback,
};
pub(in crate::gui::editor) use tier_form::setup_save_tier_callback;
pub(in crate::gui::editor) use viewport_material::setup_viewport_material_linked_changed_callback;

/// A cylindrical preform's side count, fixed and independent of the design's
/// index gear. The preform is a piece of rough, not a machine setting. Switching
/// gear after Apply Preform leaves the original rough's side count behind if it
/// derived from the old gear; a preform applied under one gear stays shaped by it
/// if switched. 96 is high enough that the rough reads as smooth in the viewport
/// at every zoom level -- the same figure
/// `indicatrix_cut_core::PreformSpec::cylinder_for_schedule`'s own doc uses for
/// the "reconstructed once from an already-authored schedule" case in
/// `loading::default_preform_for_schedule`, which this constant deliberately does
/// not touch.
///
/// `pub(in crate::gui::editor)` (and re-exported by `callbacks` for tests) so the guide's
/// launch test builds the New Design form's design with THIS figure, not a copy of it.
pub(in crate::gui::editor) const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;

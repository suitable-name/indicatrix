//! [`EditorState`] -- the editor's live state (the shared
//! [`indicatrix_editor::EditorSession`] plus the desktop's own bookkeeping) -- and
//! the view-model helpers that read a `Design` into the strings/flags `EditorView`
//! (`types.slint`'s `EditorTierItem`) and the validation banner need. See this
//! group's `mod.rs` doc comment for the "`History` is the only thing that mutates
//! `Design`" rule [`EditorState::apply`]/[`apply_coalescing`](EditorState::apply_coalescing)/
//! [`undo`](EditorState::undo)/[`redo`](EditorState::redo)/`apply_optimize_outcome`
//! uphold, all routed through the session.
//!
//! The pure view-model logic (row builders, formatting, yield/proportion texts, the
//! banner, material/gear helpers) lives in `indicatrix_editor` and is shared with the
//! web app; this module re-exports it at the old paths and keeps only the
//! desktop's adapters to its Slint row types. Split into sibling files: [`core`]
//! ([`EditorState`] itself plus construction/replacement), [`history`] (the
//! session-routed edit entry points with the desktop's clock), [`material`] (the
//! combo cache accessor and the gear-remap row adapter), [`row_format`] (the Slint
//! push helpers and row/chip adapters), [`rows`] (the tier-row adapters) and
//! [`yield_report`] (the cut-order row adapter).

mod core;
mod file_extras;
mod history;
mod material;
mod row_format;
mod rows;
mod yield_report;

// `AfterSave` is consumed by `gui::window_close` (a sibling of `editor`, not a
// descendant), so it is re-exported one level wider than the rest of `core`.
pub(super) use core::{
    ANGLE_NUDGE_COALESCE_WINDOW, PendingGearRemap, anchor_explainer_suppress_permanently,
    should_open_anchor_explainer,
};
pub(in crate::gui) use core::{AfterSave, EditorState, PendingUnsavedAction};
pub(super) use file_extras::DesignFileExtras;
// The nudge coalescing key: the desktop's production nudge goes through
// `EditorState::nudge_angles`, so only the tests (and the identity pins) name it.
pub(super) use history::coalesce_timestamp;
#[cfg(test)]
pub(super) use indicatrix_editor::material::material_name_from_index;
#[cfg(test)]
pub(super) use indicatrix_editor::session::angle_nudge_coalesce_key;
pub(super) use indicatrix_editor::{
    material::{
        MaterialComboCache, body_colour_from_index, body_colour_index_for, body_colour_options,
        builtin_preset_names, design_material_index_from_name, design_material_options,
        gear_choice_to_teeth, gear_index_from_teeth, parse_design_material_form, parse_yield_form,
        ri_source_text,
    },
    scratch::{PushedScratch, ScratchDelta},
    session::result_is_stale,
    view_model::{
        row_format::{
            first_unresolved_meet_name, representative_crown_and_pavilion_angles_deg,
            tiers_incomplete_under_proposed_symmetry,
        },
        solid_status::{
            design_to_gpu_planes, status_text_and_is_problem,
            status_text_and_is_problem_from_solved, tier_matches_filter,
        },
    },
};
pub(super) use material::gear_remap_preview;
pub(super) use row_format::{
    apply_multi_selection, index_chip_items, push_multi_selected_count, push_rows, push_tiers,
};
pub(super) use rows::{
    apply_proposed_angles, manufacturability_warnings_tagged, tier_items, tier_items_from_solved,
    tier_items_stale_with_last_solved,
};
pub(super) use yield_report::{
    cutting_instructions_rows, design_label_text, girdle_and_ratio_texts, preform_mm_texts,
    preform_y_offset_mm_text, proportion_verdicts, proportions_texts, yield_report_texts,
    yield_report_texts_from_solved,
};

#[cfg(test)]
mod tests;

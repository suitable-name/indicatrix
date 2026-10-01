//! Resolving a [`Design`](indicatrix_cut_core::Design) from a catalogue entry's full
//! record (`super::callbacks::setup_load_selected_callback`), plus the form parsers.
//! See this group's own `mod.rs` doc comment.
//!
//! # Module split
//!
//! [`design`] resolves a design from a catalogue record (vault types, desktop only);
//! [`catalogue_planes`] resolves a record's facet planes for the detail view and
//! the preview/tilt batches from the same design file. The tier/index-shorthand,
//! preform, New Design and material-form parsers, the RI-preservation rule and the bare-`.asc` loader live in
//! `indicatrix_editor::loading` (shared with the web app) and are re-exported here
//! at their old paths.

mod catalogue_planes;
mod design;

pub use catalogue_planes::{CataloguePlanesSource, resolve_catalogue_planes};

pub(in crate::gui::editor) use design::{
    design_from_full_record, external_proportions_from_full_record,
};
pub(in crate::gui::editor) use indicatrix_editor::loading::{
    LoadedDesign, TierFormFields, design_from_asc_text, material_selection_for_accepted_suggestion,
    parse_new_design_form, parse_preform_form, parse_tier_form, parse_tier_target,
};
// Only the identity pins (`super::identity_pins`) still name the RI-preservation rule
// here; the editor's own callers reach it through `ri_override_for_material_pick`.
#[cfg(test)]
pub(in crate::gui::editor) use indicatrix_editor::loading::ri_override_to_preserve;
// Only the identity pins and the history tests call these two directly now: the inline
// angle commit and the step generator reach them through `EditorSession`
// (`set_tier_angle_from_text`, `generate_step_series`).
#[cfg(test)]
pub(in crate::gui::editor) use indicatrix_editor::loading::{parse_angle_only, parse_index_list};

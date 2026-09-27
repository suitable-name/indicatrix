//! Resolving a [`Design`](indicatrix_cut_core::Design) from external sources -- a catalogue entry's
//! full record (`super::callbacks::setup_load_selected_callback`) -- and parsing the
//! tier/preform edit forms into `indicatrix-cut-core` edit payloads. See this group's own `mod.rs`
//! doc comment.
//!
//! # Module split
//!
//! [`design`] resolves a [`indicatrix_cut_core::Design`] from a catalogue record or a
//! bare `.asc` text; [`tier_form`] parses the tier-edit form's text/enum fields;
//! [`preform_and_new_design`] parses the preform and New Design dialog forms; [`material`]
//! holds the RI-preservation rule shared by the catalogue-load material suggestion and
//! a plain in-editor material pick.

mod design;
mod material;
mod preform_and_new_design;
mod tier_form;

// A re-export can only match or narrow an item's own declared visibility, never
// widen it, so every one of these is declared `pub(in crate::gui::editor)`
// directly at its definition site.
pub(in crate::gui::editor) use design::{
    LoadedDesign, design_from_asc_text, design_from_full_record,
    external_proportions_from_full_record,
};
pub(in crate::gui::editor) use material::{
    material_selection_for_accepted_suggestion, ri_override_to_preserve,
};
pub(in crate::gui::editor) use preform_and_new_design::{
    parse_new_design_form, parse_preform_form,
};
pub(in crate::gui::editor) use tier_form::{
    TierFormFields, parse_angle_only, parse_index_list, parse_tier_form, parse_tier_target,
};

//! Parsing the editor's forms into `indicatrix-cut-core` edit payloads, and building
//! a design from a bare `.asc` text.
//!
//! [`tier_form`] parses the tier-edit form's text/enum fields (including the index
//! shorthand); [`preform_and_new_design`] parses the preform and New Design dialog
//! forms; [`material`] holds the RI-preservation rule shared by the catalogue-load
//! material suggestion and a plain in-editor material pick; [`design`] builds a
//! design from `.asc` text; [`naming`] picks the auto-name of a brand-new tier and
//! the collision-free names of duplicated, mirrored and generated tiers;
//! [`step_series`] parses the Generate steps form and reports [`StepSeriesError`];
//! [`number_expr`] lets every numeric field take arithmetic (`41 + 0.5`).

pub mod concave_form;
pub mod design;
pub mod material;
pub mod naming;
pub mod number_expr;
pub mod preform_and_new_design;
pub mod step_series;
pub mod tier_form;

pub use concave_form::{ConcaveTierFormFields, concave_tier_form_fields, parse_concave_tier_form};
pub use design::{LoadedDesign, default_preform_for_schedule, design_from_asc_text};
pub use material::{
    RI_PRESERVE_TOLERANCE, material_selection_for_accepted_suggestion,
    ri_override_for_material_pick, ri_override_to_preserve,
};
pub use naming::{
    name_collides, next_free_block_name, series_names, split_duplicate_suffix,
    unique_duplicate_name, unique_mirror_name,
};
pub use number_expr::{NumberExprError, eval_number};
pub use preform_and_new_design::{parse_new_design_form, parse_preform_form};
pub use step_series::{StepSeriesError, parse_step_series_form};
pub use tier_form::{
    TierFormFields, non_integral_index_warning, parse_angle_only, parse_angle_only_with_names,
    parse_index_list, parse_tier_form, parse_tier_form_with_relation, parse_tier_target,
    tier_form_error_field,
};

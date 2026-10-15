//! The template path of the New Design dialog, minus the window. The logic is pure and
//! native-tested, so it lives in `indicatrix_web_core::new_design`; this module re-exports
//! what the dialog code calls.

pub use indicatrix_web_core::new_design::{
    build_choice, default_material_index, material_labels, material_note_for,
    sanitized_template_index,
};

//! Resolves and displays the retarget dialog's own TARGET material -- see this
//! group's own `mod.rs` doc comment ("Where the target material comes from"). The
//! logic moved unchanged to `indicatrix_editor::retarget::view` (shared with the web
//! app); re-exported here at its old paths.

pub(super) use indicatrix_editor::retarget::view::{
    initial_target_index, resolve_target_selection, resolved_material_from_selection,
    target_display_name, target_material_selection,
};

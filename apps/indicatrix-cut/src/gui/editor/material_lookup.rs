//! The editor-side `indicatrix_cut_core::MaterialLookup` and the material helpers
//! built on it (RI-override-aware resolution, the traced-material rule, the
//! nearest-built-in search). Moved unchanged to `indicatrix_editor::material_lookup`
//! (shared with the web app); re-exported here at the old path.

pub(in crate::gui) use indicatrix_editor::material_lookup::{
    EditorMaterialLookup, MATERIAL_MATCH_TOLERANCE, material_for_refractive_index, material_guess,
    nearest_built_in_material, resolved_gem_material, traced_gem_material, traced_material_for,
};
// Only the identity pins still name the candidate list directly (the guess badge
// reaches it through `material_guess`).
#[cfg(test)]
pub(in crate::gui) use indicatrix_editor::material_lookup::material_guess_candidates;

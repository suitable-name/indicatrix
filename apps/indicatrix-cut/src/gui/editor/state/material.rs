//! Material/RI and gear-tooth view-model helpers. The logic lives in
//! `indicatrix_editor::material` (shared with the web app); this file keeps the
//! desktop's cache accessor on [`EditorState`] and the thin adapter from the plain
//! gear-remap preview rows to the Slint `GearRemapRow`.

use super::core::EditorState;
use crate::GearRemapRow;
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, RemapRounding};

impl EditorState {
    /// [`indicatrix_editor::material::design_material_options`], rebuilt only when
    /// `custom`'s own name list has changed since the last call -- see
    /// [`indicatrix_editor::material::MaterialComboCache`].
    /// `view::refresh_design_settings` calls this every refresh; only a save/delete/
    /// rename in the material editor dialog (which changes `custom`'s names) actually
    /// pays for a rebuild.
    pub(in crate::gui::editor) fn material_combo_options(
        &self,
        custom: &[GemMaterial],
    ) -> Vec<String> {
        self.material_combo_cache.borrow_mut().options(custom)
    }
}

/// [`indicatrix_editor::material::gear_remap_preview`], mapped to the gear-remap
/// confirmation panel's Slint rows.
pub(in crate::gui::editor) fn gear_remap_preview(
    design: &Design,
    from_gear: i32,
    to_gear: i32,
    rounding: RemapRounding,
) -> Vec<GearRemapRow> {
    indicatrix_editor::material::gear_remap_preview(design, from_gear, to_gear, rounding)
        .into_iter()
        .map(|row| GearRemapRow {
            name: row.name.into(),
            old_indices: row.old_indices.into(),
            new_indices: row.new_indices.into(),
            non_integral: row.non_integral,
        })
        .collect()
}

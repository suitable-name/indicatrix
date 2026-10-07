//! The body colour the material editor dialog is holding, beside [`super::physics_state`].
//!
//! The dialog's preset row is a legacy three-band triple; the path-aware L*C*h editor
//! (`indicatrix_editor::lch_color`) solves a seven-band colour next to the nearest triple. This
//! module remembers the band rows of that solve until "Save" writes them to the vault row
//! (`CustomMaterialRow::absorption_bands_json`): there is one dialog, so one slot. It is filled
//! from the selected material when the dialog's pre-fill is pushed, replaced by an editor
//! "Apply colour", and dropped when a preset swatch is chosen. Physics state is never touched.

use std::sync::{Mutex, PoisonError};

use indicatrix_cut_core::MaterialSelection;

/// The colour of the dialog's material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DialogColor {
    /// The seven-band rows `[centre_nm, width_nm, amplitude_per_mm]`, `None` for none.
    pub bands: Option<Vec<[f32; 3]>>,
    /// The legacy triple the preset row or the solve produced, `None` for a new material.
    pub triple: Option<[f32; 3]>,
}

static SLOT: Mutex<DialogColor> = Mutex::new(DialogColor {
    bands: None,
    triple: None,
});

/// The colour the dialog holds now.
#[must_use]
pub fn current() -> DialogColor {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Replaces the held colour (the dialog's pre-fill, or an editor "Apply colour").
pub fn set(color: DialogColor) {
    *SLOT.lock().unwrap_or_else(PoisonError::into_inner) = color;
}

/// A preset swatch was chosen: the bands go, the triple becomes the preset's (`None` for the
/// "Custom (keep)" swatch, which keeps the held triple).
pub fn preset_chosen(triple: Option<[f32; 3]>) {
    let mut slot = SLOT.lock().unwrap_or_else(PoisonError::into_inner);
    slot.bands = None;
    if triple.is_some() {
        slot.triple = triple;
    }
}

/// The held colour as a selection, which is what `lch_color::lch_from_material` seeds the
/// editor's sliders from.
#[must_use]
pub fn as_selection(color: &DialogColor) -> MaterialSelection {
    MaterialSelection::none().with_body_color_bands(color.triple, color.bands.clone(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_pick_drops_the_bands_and_keep_keeps_the_triple() {
        // One test owns the slot: the module is a single process-wide value.
        set(DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25]]),
            triple: Some([0.1, 0.2, 0.3]),
        });
        preset_chosen(None);
        assert_eq!(current().bands, None);
        assert_eq!(current().triple, Some([0.1, 0.2, 0.3]));
        preset_chosen(Some([0.4, 0.5, 0.6]));
        assert_eq!(current().triple, Some([0.4, 0.5, 0.6]));
        set(DialogColor::default());
        assert_eq!(current(), DialogColor::default());
    }

    #[test]
    fn the_selection_carries_triple_and_bands() {
        let color = DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25]]),
            triple: Some([0.1, 0.2, 0.3]),
        };
        let selection = as_selection(&color);
        assert_eq!(selection.body_color_override, Some([0.1, 0.2, 0.3]));
        assert_eq!(
            selection.body_color_bands_override,
            Some(vec![[460.0, 45.0, 0.25]])
        );
        assert_eq!(
            as_selection(&DialogColor::default()),
            MaterialSelection::none()
        );
    }
}

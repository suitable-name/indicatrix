//! Converting a startup `get_custom_materials()` row read into the render context's
//! custom-material and specific-gravity lists together.

use crate::gui::optics::crystal_optics::{
    custom_material_specific_gravity_from_rows, gem_material_from_row,
};
use indicatrix::optics::materials::GemMaterial;

/// Converts one startup `get_custom_materials()` row read into BOTH
/// `RenderContext::custom_materials` and `RenderContext::
/// custom_material_specific_gravity` together -- pulled out
/// of `super::main_window::build_main_window` purely so this pure conversion is
/// unit-testable without constructing a whole `MainWindow`/`Database`.
///
/// Both lists are derived from the same row read so they can never disagree:
/// without deriving `custom_material_specific_gravity` here too,
/// `RenderContext::custom_material_specific_gravity` would stay empty until this
/// session's own save/delete callback (`gui::optics::custom_materials`) ran at
/// least once -- a custom material's carat-weight estimate would read blank for
/// the whole first browse of a freshly launched app, even for a material that
/// already had a recorded specific gravity on disk.
#[must_use]
pub(super) fn initial_custom_materials_and_specific_gravity(
    rows: &[indicatrix_vault::model::material::CustomMaterialRow],
) -> (Vec<GemMaterial>, Vec<(String, f64)>) {
    let materials = rows.iter().map(gem_material_from_row).collect();
    let specific_gravity = custom_material_specific_gravity_from_rows(rows);
    (materials, specific_gravity)
}

#[cfg(test)]
mod tests {
    use super::initial_custom_materials_and_specific_gravity;
    use indicatrix_vault::model::material::CustomMaterialRow;

    fn custom_row(name: &str, specific_gravity: Option<f32>) -> CustomMaterialRow {
        CustomMaterialRow {
            name: name.to_string(),
            refractive_index: 1.7,
            dispersion: 0.02,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity,
        }
    }

    // --- initial_custom_materials_and_specific_gravity ---

    /// Every row becomes a `GemMaterial` regardless of whether it has a recorded
    /// SG, but only the rows that DO have one contribute an entry to the second
    /// tuple element -- the exact split `RenderContext::custom_materials`/
    /// `custom_material_specific_gravity` themselves keep, now populated at
    /// startup from the same read instead of staying empty until a save/delete
    /// callback has run this session.
    #[test]
    fn startup_helper_populates_both_lists_from_one_row_read() {
        let rows = [
            custom_row("My Garnet", Some(3.90)),
            custom_row("Custom Diamond", None),
        ];
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&rows);

        assert_eq!(materials.len(), 2, "every row becomes a GemMaterial");
        assert!(materials.iter().any(|m| m.name == "My Garnet"));
        assert!(materials.iter().any(|m| m.name == "Custom Diamond"));

        // Exact `f32` -> `f64` widening (no arithmetic in between), but not
        // literal-equal to a directly-typed `f64` -- see `crystal_optics::
        // custom_material_specific_gravity_from_rows`'s own tests for why this
        // compares by name and by tolerance rather than a brittle `assert_eq!`
        // against a `vec![...]` literal.
        assert_eq!(
            specific_gravity.len(),
            1,
            "only the row with a recorded SG contributes an entry, and a row \
             with none is skipped rather than showing up as a spurious Some(0.0)"
        );
        assert_eq!(specific_gravity[0].0, "My Garnet");
        assert!((specific_gravity[0].1 - 3.90_f64).abs() < 1e-6);
    }

    /// An empty catalogue (fresh install, or a session with no custom materials
    /// saved yet) must not panic and must produce two empty lists, never a
    /// default/placeholder entry.
    #[test]
    fn startup_helper_is_empty_for_no_custom_materials() {
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&[]);
        assert_eq!(materials.len(), 0);
        assert_eq!(specific_gravity.len(), 0);
    }
}

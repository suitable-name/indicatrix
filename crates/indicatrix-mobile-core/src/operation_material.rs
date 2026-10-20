//! Explicit optics for search; an unnamed imported index is not a material guess.
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, MaterialLookup, MaterialSelection};
use indicatrix_editor::material_lookup::EditorMaterialLookup;

pub fn source(design: &Design, custom: &[GemMaterial]) -> Result<GemMaterial, String> {
    resolve(&design.material, custom, Some(design.meta.refractive_index))
}

pub fn target(
    selection: &MaterialSelection,
    custom: &[GemMaterial],
) -> Result<GemMaterial, String> {
    resolve(selection, custom, None)
}

fn resolve(
    selection: &MaterialSelection,
    custom: &[GemMaterial],
    legacy_ri: Option<f64>,
) -> Result<GemMaterial, String> {
    if let Some(ri) = selection.refractive_index_override {
        trace_index(ri)?;
    }
    let base = if let Some(name) = selection.name.as_deref() {
        EditorMaterialLookup::new(custom)
            .lookup(name)
            .ok_or_else(|| {
                format!("Restore the material recipe for {name:?} before running this operation.")
            })?
    } else {
        let ri = selection
            .refractive_index_override
            .or(legacy_ri)
            .ok_or_else(|| {
                "Choose a target material or enter a valid refractive index.".to_owned()
            })?;
        let ri = trace_index(ri)?;
        GemMaterial::new_custom("Refractive index only", ri, 0.0, 0.0, [0.0; 3])
    };
    Ok(selection.apply_overrides(base))
}

fn trace_index(value: f64) -> Result<f32, String> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "validate the exact tracing precision after conversion"
    )]
    let value = value as f32;
    if value.is_finite() && value > 1.0 {
        Ok(value)
    } else {
        Err("The refractive index must be finite and greater than one at tracing precision.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_index_is_used_without_inventing_a_species() {
        let mut design = indicatrix_editor::EditorSession::fresh().design;
        design.material = MaterialSelection::none();
        design.meta.refractive_index = 1.72;
        let material = source(&design, &[]).expect("legacy index is usable");
        assert!((material.dispersion.n_d() - 1.72).abs() < 1e-6);
        assert_eq!(material.name, "Refractive index only");
        assert!(
            target(&design.material, &[]).is_err(),
            "target needs an explicit choice"
        );
    }

    #[test]
    fn unknown_recipes_and_unrepresentable_indices_are_refused() {
        let mut selection = MaterialSelection::none();
        selection.name = Some("Missing custom recipe".into());
        assert!(target(&selection, &[]).is_err());
        for name in [None, Some("Diamond".to_owned())] {
            selection.name = name;
            for ri in [1e100, 1.0 + 1e-12, f64::NAN] {
                selection.refractive_index_override = Some(ri);
                assert!(target(&selection, &[]).is_err());
            }
        }
    }
}

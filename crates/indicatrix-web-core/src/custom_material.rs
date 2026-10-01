//! The Design settings dialog's compact custom-material editor: the typed fields turned
//! into a [`GemMaterial`] and the [`CustomMaterialSnapshot`] a native file carries for it.
//!
//! The desktop keeps custom materials in its database; the browser has none, so a custom
//! material lives in the page's catalogue (the render and material combos offer it) and in
//! the design's own native file (`[material.custom]`), which is how it survives a reload of
//! the tab and a save. The fields are the ones that decide the optics -- refractive index,
//! dispersion (the F-C spread), birefringence, an optional specific gravity for the carat
//! estimate and a body colour -- and the crystal system and optical character are derived
//! from the birefringence exactly as `GemMaterial::new_custom` does. The desktop's fuller
//! editor (crystal system and optical character pickers, biaxial data, templates) is not
//! mirrored.

use indicatrix::optics::materials::GemMaterial;
use indicatrix_editor::material::body_colour_from_index;
use indicatrix_formats::native::CustomMaterialSnapshot;

/// The refractive-index range the desktop's editor allows.
pub const RI_RANGE: (f32, f32) = (1.30, 3.20);
/// The dispersion range (`n_F` - `n_C`) the desktop's editor allows.
pub const DISPERSION_RANGE: (f32, f32) = (0.0, 0.12);
/// The birefringence range the desktop's editor allows.
pub const BIREFRINGENCE_RANGE: (f32, f32) = (-0.05, 0.10);

/// The editor's fields, exactly as typed.
#[derive(Debug, Clone, Copy)]
pub struct CustomMaterialForm<'a> {
    /// The material's name.
    pub name: &'a str,
    /// Refractive index (sodium D).
    pub ri: &'a str,
    /// Dispersion, `n_F` - `n_C`.
    pub dispersion: &'a str,
    /// Birefringence (signed); blank reads as 0.
    pub birefringence: &'a str,
    /// Specific gravity for the carat estimate; blank or 0 means "not recorded".
    pub specific_gravity: &'a str,
    /// The Colour combo's index (`indicatrix_editor::material::body_colour_options`): 0
    /// and any unknown index are clear.
    pub colour_index: i32,
}

/// A material built from the fields.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltCustomMaterial {
    /// The material for the catalogue.
    pub material: GemMaterial,
    /// What a native file stores for it.
    pub snapshot: CustomMaterialSnapshot,
}

/// The built-in material whose name `name` equals (ignoring ASCII case), if any.
///
/// A custom material registered under such a name would shadow the built-in wherever its
/// name is selected, so neither the editor nor a restored file may add one.
#[must_use]
pub fn builtin_name_conflict(name: &str) -> Option<String> {
    GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(name.trim()))
        .map(|m| m.name)
}

/// The refusal text for a custom material named like a built-in one.
#[must_use]
pub fn builtin_name_message(name: &str, builtin: &str) -> String {
    format!(
        "'{}' is a built-in material name -- a custom material under it would silently \
         shadow '{builtin}' everywhere it is selected. Rename it first.",
        name.trim()
    )
}

/// A finite number from `text`, or `default` when the text is blank.
fn number(label: &str, text: &str, default: Option<f32>) -> Result<f32, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return default.ok_or_else(|| format!("{label} is required."));
    }
    trimmed
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{label} '{trimmed}' is not a number."))
}

/// `value` within `range`, or the message naming the field and the range.
fn within(label: &str, value: f32, range: (f32, f32)) -> Result<f32, String> {
    if (range.0..=range.1).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{label} must be between {} and {}.",
            range.0, range.1
        ))
    }
}

/// Builds the material the fields describe.
///
/// # Errors
///
/// A blank name, a name that is a built-in material's (saving one under it would shadow the
/// built-in wherever its name is selected), or a number that is missing, not a number or
/// outside its range -- as the message to show.
pub fn build_custom_material(form: &CustomMaterialForm<'_>) -> Result<BuiltCustomMaterial, String> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("A material name is required.".to_string());
    }
    if let Some(builtin) = builtin_name_conflict(name) {
        return Err(builtin_name_message(name, &builtin));
    }
    let ri = within(
        "Refractive index",
        number("Refractive index", form.ri, None)?,
        RI_RANGE,
    )?;
    let dispersion = within(
        "Dispersion",
        number("Dispersion", form.dispersion, Some(0.0))?,
        DISPERSION_RANGE,
    )?;
    let birefringence = within(
        "Birefringence",
        number("Birefringence", form.birefringence, Some(0.0))?,
        BIREFRINGENCE_RANGE,
    )?;
    let specific_gravity = number("Specific gravity", form.specific_gravity, Some(0.0))?;
    if specific_gravity < 0.0 {
        return Err("Specific gravity cannot be negative.".to_string());
    }
    // 0 is the desktop's "not recorded" sentinel, stored as `None` rather than a
    // zero-density material.
    let specific_gravity = (specific_gravity > 0.0001).then_some(f64::from(specific_gravity));
    let colour = body_colour_from_index(form.colour_index);
    let material = GemMaterial::new_custom(
        name,
        ri,
        dispersion,
        birefringence,
        colour.unwrap_or([0.0; 3]),
    );
    let snapshot = CustomMaterialSnapshot::new(
        f64::from(ri),
        f64::from(dispersion),
        f64::from(birefringence),
        specific_gravity,
        format!("{:?}", material.crystal_system),
        format!("{:?}", material.optical_character),
    )
    .with_body_colour(colour);
    Ok(BuiltCustomMaterial { material, snapshot })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form<'a>(name: &'a str, ri: &'a str) -> CustomMaterialForm<'a> {
        CustomMaterialForm {
            name,
            ri,
            dispersion: "",
            birefringence: "",
            specific_gravity: "",
            colour_index: 0,
        }
    }

    #[test]
    fn a_minimal_form_builds_an_isotropic_clear_material() {
        let built = build_custom_material(&form("  Glass X ", "1.62")).expect("builds");
        assert_eq!(built.material.name, "Glass X");
        assert_eq!(built.snapshot.mean_ri, f64::from(1.62_f32));
        assert_eq!(built.snapshot.dispersion_delta, 0.0);
        assert_eq!(built.snapshot.birefringence_delta, 0.0);
        assert_eq!(built.snapshot.specific_gravity, None);
        assert_eq!(built.snapshot.optical_character, "Isotropic");
    }

    #[test]
    fn birefringence_and_gravity_reach_the_snapshot_and_the_optics() {
        let built = build_custom_material(&CustomMaterialForm {
            name: "Mineral",
            ri: "1.76",
            dispersion: "0.018",
            birefringence: "-0.008",
            specific_gravity: "3.99",
            colour_index: 2,
        })
        .expect("builds");
        assert_eq!(built.snapshot.specific_gravity, Some(f64::from(3.99_f32)));
        assert_eq!(built.snapshot.birefringence_delta, f64::from(-0.008_f32));
        assert_eq!(built.snapshot.optical_character, "UniaxialNegative");
        // The colour is the preset the combo names (index 2 is the second preset), not clear.
        let clear = GemMaterial::new_custom("Mineral", 1.76, 0.018, -0.008, [0.0; 3]);
        assert_ne!(built.material.absorption, clear.absorption);
        // The snapshot rebuilds the same optics on the next load.
        let restored = indicatrix_cut_core::native::gem_material_from_custom_snapshot(
            "Mineral",
            &built.snapshot,
        );
        assert_eq!(restored.crystal_system, built.material.crystal_system);
        assert_eq!(restored.optical_character, built.material.optical_character);
        // ... and the body colour too: the restored material is not colourless.
        assert_eq!(
            built.snapshot.body_colour(),
            body_colour_from_index(2),
            "the preset the combo names"
        );
        assert_eq!(restored.absorption, built.material.absorption);
    }

    #[test]
    fn a_clear_colour_leaves_the_snapshot_without_one() {
        let built = build_custom_material(&form("Glass", "1.5")).expect("builds");
        assert_eq!(built.snapshot.absorption_rgb, None);
    }

    #[test]
    fn built_in_names_conflict_whatever_their_case() {
        assert_eq!(builtin_name_conflict("diamond").as_deref(), Some("Diamond"));
        assert_eq!(
            builtin_name_conflict(" Diamond ").as_deref(),
            Some("Diamond")
        );
        assert_eq!(builtin_name_conflict("Glass X"), None);
        assert!(builtin_name_message("diamond", "Diamond").contains("'diamond'"));
    }

    #[test]
    fn bad_fields_are_named() {
        let err = |f: CustomMaterialForm<'_>| build_custom_material(&f).expect_err("refused");
        assert!(err(form("", "1.5")).contains("name is required"));
        assert!(err(form("Diamond", "1.5")).contains("built-in"));
        assert!(
            err(form("diamond", "1.5")).contains("built-in"),
            "case-insensitive"
        );
        assert!(err(form("X", "")).contains("Refractive index is required"));
        assert!(err(form("X", "abc")).contains("is not a number"));
        assert!(err(form("X", "9")).contains("between"));
        assert!(err(form("X", "NaN")).contains("is not a number"));
        assert!(
            err(CustomMaterialForm {
                dispersion: "0.5",
                ..form("X", "1.5")
            })
            .contains("Dispersion")
        );
        assert!(
            err(CustomMaterialForm {
                specific_gravity: "-1",
                ..form("X", "1.5")
            })
            .contains("Specific gravity")
        );
    }
}

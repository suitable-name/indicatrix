//! The template path of the New Design dialog, minus the window.
//!
//! It covers the material and gear lists a template offers, the line under the material
//! combo, the stone-width field and the [`NewDesignChoice`] a Create asks
//! `create_from_template` for (the desktop's `gui/editor/templates.rs`, in the same words).
//!
//! Everything here is a plain function of its arguments, so the page's dialog code stays a
//! thin Slint shell around it and the logic is tested natively.

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{MaterialSelection, templates::TemplateSpec};
use indicatrix_editor::{
    material::{
        GEAR_PRESETS, design_material_index_from_name, design_material_name_from_index,
        design_material_options,
    },
    retarget::view::resolved_material_from_selection,
    templates::{NewDesignChoice, material_note, template_spec},
};

/// The widest stone the dialog accepts, in millimetres (the desktop's bound).
pub const MAX_STONE_WIDTH_MM: f64 = 200.0;

/// The template the dialog opens on: "Standard Round Brilliant".
pub const DEFAULT_TEMPLATE_INDEX: i32 = 1;

/// The material combo's entries: the built-in materials, then the custom ones.
///
/// This is the Design Settings list without its "(none)" first entry and its trailing
/// "Custom RI..." sentinel (a template is cut from a named material).
#[must_use]
pub fn material_labels(custom: &[GemMaterial]) -> Vec<String> {
    let mut options = design_material_options(custom);
    options.pop();
    if !options.is_empty() {
        options.remove(0);
    }
    options
}

/// The selection for the material combo's `ui_index`, or `None` for an index outside the
/// list. The combo omits the list's "(none)" first entry, so its index is one behind.
#[must_use]
pub fn material_selection(ui_index: i32, custom: &[GemMaterial]) -> Option<MaterialSelection> {
    let options = design_material_options(custom);
    let name = design_material_name_from_index(ui_index.checked_add(1)?, &options)?;
    Some(MaterialSelection {
        name: Some(name),
        ..MaterialSelection::none()
    })
}

/// The material combo's index of the template's own material (the first entry if the list
/// somehow lacks it).
#[must_use]
pub fn default_material_index(spec: &TemplateSpec, custom: &[GemMaterial]) -> i32 {
    let options = design_material_options(custom);
    (design_material_index_from_name(Some(spec.default_material), &options) - 1).max(0)
}

/// The line under the material combo: what the material at `ui_index` means for the
/// template's facet angles. Empty for an index outside the list.
#[must_use]
pub fn material_note_for(spec: &TemplateSpec, ui_index: i32, custom: &[GemMaterial]) -> String {
    let Some(selection) = material_selection(ui_index, custom) else {
        return String::new();
    };
    let label = selection
        .name
        .clone()
        .unwrap_or_else(|| "This material".to_string());
    let n_d = resolved_material_from_selection(&selection, custom).n_d;
    material_note(spec, &label, n_d)
}

/// `template_index` if it names a card of the dialog (a template, or `0` for the blank
/// design), else the round brilliant.
#[must_use]
pub fn sanitized_template_index(template_index: i32) -> i32 {
    if template_index == 0 || template_spec(template_index).is_some() {
        template_index
    } else {
        DEFAULT_TEMPLATE_INDEX
    }
}

/// Reads the stone width the dialog typed: empty is "not set" (`None`); otherwise a number
/// above 0 and at most [`MAX_STONE_WIDTH_MM`] millimetres.
///
/// # Errors
///
/// A sentence for the toast when the text is not such a number.
pub fn parse_stone_width(text: &str) -> Result<Option<f64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(mm) if mm.is_finite() && mm > 0.0 && mm <= MAX_STONE_WIDTH_MM => Ok(Some(mm)),
        _ => Err(format!(
            "The stone width must be a number above 0 and at most {MAX_STONE_WIDTH_MM} mm."
        )),
    }
}

/// What the dialog's Create asks for, validated: the template, the gear its combo offers at
/// `gear_index`, the material at `material_index` and the stone width typed.
///
/// # Errors
///
/// A sentence for the toast when the template, gear, material or width is not valid.
pub fn build_choice(
    template_index: i32,
    gear_index: i32,
    material_index: i32,
    width_text: &str,
    custom: &[GemMaterial],
) -> Result<NewDesignChoice, String> {
    let spec =
        template_spec(template_index).ok_or_else(|| "Pick one of the shapes first.".to_string())?;
    let gear_teeth = usize::try_from(gear_index)
        .ok()
        .and_then(|index| spec.allowed_gears_among(&GEAR_PRESETS).get(index).copied())
        .ok_or_else(|| "Pick an index gear from the list.".to_string())?;
    let material = material_selection(material_index, custom)
        .ok_or_else(|| "Pick a material from the list.".to_string())?;
    let girdle_diameter_mm = parse_stone_width(width_text)?;
    Ok(NewDesignChoice {
        template_index,
        gear_teeth,
        material,
        girdle_diameter_mm,
        custom_materials: custom.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_brilliant() -> &'static TemplateSpec {
        template_spec(DEFAULT_TEMPLATE_INDEX).expect("the default template exists")
    }

    #[test]
    fn material_labels_drop_the_none_entry_and_the_custom_ri_sentinel() {
        let labels = material_labels(&[]);
        assert_ne!(labels.len(), 0);
        assert!(!labels.iter().any(|l| l == "(none)"));
        assert!(!labels.iter().any(|l| l.starts_with("Custom RI")));
    }

    #[test]
    fn a_combo_index_names_the_label_at_that_position() {
        let labels = material_labels(&[]);
        for (i, label) in labels.iter().enumerate() {
            let index = i32::try_from(i).expect("small");
            let selection = material_selection(index, &[]).expect("in range");
            assert_eq!(selection.name.as_deref(), Some(label.as_str()));
        }
    }

    #[test]
    fn an_index_outside_the_list_selects_nothing() {
        assert!(material_selection(-1, &[]).is_none());
        assert!(material_selection(i32::MAX, &[]).is_none());
        assert!(material_selection(10_000, &[]).is_none());
    }

    #[test]
    fn the_default_material_index_is_in_range_and_names_the_templates_material() {
        let spec = round_brilliant();
        let labels = material_labels(&[]);
        let index = default_material_index(spec, &[]);
        let at = usize::try_from(index).expect("not negative");
        assert!(at < labels.len());
        if labels
            .iter()
            .any(|l| l.eq_ignore_ascii_case(spec.default_material))
        {
            assert!(labels[at].eq_ignore_ascii_case(spec.default_material));
        }
    }

    #[test]
    fn the_material_note_names_the_design_material_and_is_empty_off_the_list() {
        let spec = round_brilliant();
        let index = default_material_index(spec, &[]);
        let note = material_note_for(spec, index, &[]);
        assert!(note.starts_with("Designed for "), "{note}");
        assert_eq!(material_note_for(spec, -1, &[]), "");
    }

    #[test]
    fn a_template_index_off_the_gallery_falls_back_to_the_round_brilliant() {
        assert_eq!(sanitized_template_index(0), 0);
        assert_eq!(
            sanitized_template_index(DEFAULT_TEMPLATE_INDEX),
            DEFAULT_TEMPLATE_INDEX
        );
        assert_eq!(sanitized_template_index(-3), DEFAULT_TEMPLATE_INDEX);
        assert_eq!(sanitized_template_index(10_000), DEFAULT_TEMPLATE_INDEX);
    }

    #[test]
    fn the_stone_width_is_optional_and_bounded() {
        assert_eq!(parse_stone_width(""), Ok(None));
        assert_eq!(parse_stone_width("   "), Ok(None));
        assert_eq!(parse_stone_width(" 6.5 "), Ok(Some(6.5)));
        assert_eq!(parse_stone_width("200"), Ok(Some(200.0)));
        for bad in ["0", "-1", "200.5", "abc", "NaN", "inf", "1e999"] {
            assert!(parse_stone_width(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_valid_choice_carries_the_first_gear_the_material_and_the_width() {
        let spec = round_brilliant();
        let material = default_material_index(spec, &[]);
        let choice =
            build_choice(DEFAULT_TEMPLATE_INDEX, 0, material, "7.25", &[]).expect("a valid choice");
        assert_eq!(choice.template_index, DEFAULT_TEMPLATE_INDEX);
        assert_eq!(choice.gear_teeth, spec.gear_teeth);
        assert_eq!(choice.girdle_diameter_mm, Some(7.25));
        assert!(choice.material.name.is_some());
        assert_eq!(choice.custom_materials.len(), 0);
        let no_width =
            build_choice(DEFAULT_TEMPLATE_INDEX, 0, material, "", &[]).expect("width is optional");
        assert_eq!(no_width.girdle_diameter_mm, None);
    }

    #[test]
    fn an_invalid_choice_says_which_part_is_wrong() {
        assert_eq!(
            build_choice(0, 0, 0, "", &[]).err().as_deref(),
            Some("Pick one of the shapes first.")
        );
        assert_eq!(
            build_choice(DEFAULT_TEMPLATE_INDEX, 999, 0, "", &[])
                .err()
                .as_deref(),
            Some("Pick an index gear from the list.")
        );
        assert_eq!(
            build_choice(DEFAULT_TEMPLATE_INDEX, -1, 0, "", &[])
                .err()
                .as_deref(),
            Some("Pick an index gear from the list.")
        );
        assert_eq!(
            build_choice(DEFAULT_TEMPLATE_INDEX, 0, 10_000, "", &[])
                .err()
                .as_deref(),
            Some("Pick a material from the list.")
        );
        let width = build_choice(DEFAULT_TEMPLATE_INDEX, 0, 0, "wide", &[]).err();
        assert!(width.is_some_and(|m| m.starts_with("The stone width must be")));
    }
}

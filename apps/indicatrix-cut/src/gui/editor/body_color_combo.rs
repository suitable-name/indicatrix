//! The design settings panel's body-colour combo with its trailing "Custom..." entry.
//!
//! The combo lists the material's own colour, the nine presets and a last "Custom..."
//! entry ([`body_color_options`]). Choosing "Custom..." opens the hue/saturation/brightness
//! picker under the combo; the picked colour is matched to the closest reachable one and
//! applied as one undoable edit (`render_color::setup_render_color_callbacks`). A design can
//! also carry a body colour that matches no preset -- one picked from the Live Render
//! toolbar, for example -- and the combo then sits on "Custom...". Applying the panel must
//! KEEP such a colour rather than reset the design to the material's own colour (the combo
//! is authoritative on Apply), so "Custom..." means "leave the colour as it is".
//!
//! Everything here is a pure function of its inputs so the rule is unit-testable without a
//! window.

use indicatrix_cut_core::MaterialSelection;

use super::state::{
    body_color_custom_index, body_color_index_for, parse_design_material_form,
    with_body_color_choice,
};

/// Whether `color` is a body colour that matches no preset (so the combo shows its
/// "Custom..." entry).
pub(super) fn is_custom(color: Option<[f32; 3]>) -> bool {
    color.is_some() && body_color_index_for(color) == body_color_custom_index()
}

/// The material an Apply of the whole material row produces: the material/RI parse
/// ([`parse_design_material_form`]) with the colour combo applied on top through
/// [`with_body_color_choice`]. A custom colour (the triple and the L*C*h editor's bands)
/// therefore survives a change of material name, which the plain parse would drop; a preset
/// or "Material default" pick drops the bands.
pub(super) fn material_for_apply(
    combo_index: i32,
    ri_override_text: &str,
    options: &[String],
    current: &MaterialSelection,
    color_index: i32,
) -> Result<MaterialSelection, String> {
    parse_design_material_form(combo_index, ri_override_text, options, current)
        .map(|material| with_body_color_choice(material, color_index, current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::material::{
        body_color_for_apply, body_color_from_index, body_color_options,
    };

    /// A triple no preset has.
    const CUSTOM_RGB: [f32; 3] = [0.123, 0.456, 0.789];

    fn preset_rgb(index: i32) -> [f32; 3] {
        body_color_from_index(index).expect("a preset index")
    }

    #[test]
    fn no_color_and_preset_colors_are_not_custom() {
        assert!(!is_custom(None));
        for index in 1..body_color_custom_index() {
            assert!(!is_custom(Some(preset_rgb(index))), "preset {index}");
        }
    }

    #[test]
    fn a_non_preset_triple_is_custom() {
        assert!(is_custom(Some(CUSTOM_RGB)));
    }

    #[test]
    fn the_custom_entry_is_always_the_last_option() {
        let options = body_color_options();
        assert_eq!(
            usize::try_from(body_color_custom_index()).unwrap(),
            options.len() - 1
        );
        assert_eq!(
            body_color_index_for(Some(CUSTOM_RGB)),
            body_color_custom_index()
        );
    }

    #[test]
    fn apply_keeps_a_custom_triple_when_custom_is_selected() {
        assert_eq!(
            body_color_for_apply(body_color_custom_index(), Some(CUSTOM_RGB)),
            Some(CUSTOM_RGB)
        );
    }

    #[test]
    fn apply_resolves_presets_and_the_default_as_before() {
        assert_eq!(body_color_for_apply(0, Some(CUSTOM_RGB)), None);
        assert_eq!(
            body_color_for_apply(2, Some(CUSTOM_RGB)),
            Some(preset_rgb(2)),
            "choosing a preset replaces the custom triple"
        );
        assert_eq!(body_color_for_apply(2, None), Some(preset_rgb(2)));
    }

    #[test]
    fn apply_design_material_keeps_a_custom_triple() {
        let current = MaterialSelection::default().with_body_color(Some(CUSTOM_RGB));
        let options = vec!["(none)".to_owned(), "Diamond".to_owned()];
        // The material row is left on "(none)", no RI text, and the colour combo sits
        // on its Custom entry: the triple stays.
        let applied = material_for_apply(0, "", &options, &current, body_color_custom_index())
            .expect("the form parses");
        assert_eq!(applied.body_color_override, Some(CUSTOM_RGB));
        // The plain material parse drops the colour when the species changes ...
        let parsed = parse_design_material_form(1, "", &options, &current).expect("parses");
        assert_eq!(parsed.name.as_deref(), Some("Diamond"));
        assert_eq!(parsed.body_color_override, None);
        // ... but the combo is authoritative, so a Custom selection keeps the triple
        // through a species change too.
        let switched = material_for_apply(1, "", &options, &current, body_color_custom_index())
            .expect("the form parses");
        assert_eq!(switched.name.as_deref(), Some("Diamond"));
        assert_eq!(switched.body_color_override, Some(CUSTOM_RGB));
    }

    #[test]
    fn apply_keeps_the_lch_bands_for_custom_and_drops_them_for_a_preset() {
        let rows = vec![[460.0f32, 45.0, 0.25]];
        let current = MaterialSelection::default().with_body_color_bands(
            Some(CUSTOM_RGB),
            Some(rows.clone()),
            Some(2.0),
        );
        let options = vec!["(none)".to_owned()];
        let kept = material_for_apply(0, "", &options, &current, body_color_custom_index())
            .expect("parses");
        assert_eq!(kept.body_color_bands_override, Some(rows));
        assert_eq!(kept.absorption_path_scale_override, Some(2.0));
        let preset = material_for_apply(0, "", &options, &current, 4).expect("parses");
        assert_eq!(preset.body_color_bands_override, None);
        assert_eq!(preset.absorption_path_scale_override, None);
        let cleared = material_for_apply(0, "", &options, &current, 0).expect("parses");
        assert_eq!(cleared.body_color_bands_override, None);
    }

    #[test]
    fn apply_design_material_can_still_clear_or_replace_the_color() {
        let current = MaterialSelection::default().with_body_color(Some(CUSTOM_RGB));
        let options = vec!["(none)".to_owned()];
        let cleared = material_for_apply(0, "", &options, &current, 0).expect("parses");
        assert_eq!(cleared.body_color_override, None);
        let preset = material_for_apply(0, "", &options, &current, 4).expect("parses");
        assert_eq!(preset.body_color_override, Some(preset_rgb(4)));
    }
}

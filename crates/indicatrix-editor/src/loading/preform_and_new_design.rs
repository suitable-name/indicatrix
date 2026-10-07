//! Parses the preform-edit form and the New Design dialog's own fields into
//! `indicatrix-cut-core` spec types.

use super::number_expr::eval_number;
use crate::material::material_name_from_index;
use indicatrix_cut_core::{FreshDesignSpec, MaterialSelection, PreformSpec};

/// Parses the preform form's shape choice and three dimension fields into a
/// [`PreformSpec`].
///
/// Pure and unit tested directly, same reasoning as
/// [`super::tier_form::parse_tier_form`]. `cylinder_sides` is passed in (rather
/// than read from a `Design` inside this function) so it stays decoupled from
/// `indicatrix-cut-core` state and easy to test in isolation. Every caller
/// (`callbacks::tier_actions`) always passes a FIXED side count, independent of
/// the active design's index gear. Unlike
/// [`indicatrix_cut_core::PreformSpec::cylinder_for_schedule`]'s "fold count
/// matches the index gear" reasoning (correct only for a preform reconstructed
/// ONCE from an already-authored schedule), a reapply after gear change can
/// leave a stale side count behind if the design is queried for it.
///
/// # Errors
///
/// A message naming the field that is not a number, or saying the dimensions must
/// be positive and finite.
pub fn parse_preform_form(
    shape_index: i32,
    half_width: &str,
    length_over_width: &str,
    depth: &str,
    cylinder_sides: usize,
) -> Result<PreformSpec, String> {
    let half_width =
        eval_number(half_width, None).map_err(|error| error.message("Half-width", half_width))?;
    let length_over_width = eval_number(length_over_width, None)
        .map_err(|error| error.message("Length/width", length_over_width))?;
    let depth = eval_number(depth, None).map_err(|error| error.message("Depth", depth))?;
    if !(half_width.is_finite() && length_over_width.is_finite() && depth.is_finite())
        || half_width <= 0.0
        || length_over_width <= 0.0
        || depth <= 0.0
    {
        return Err("Preform dimensions must be positive, finite numbers.".to_string());
    }

    Ok(if shape_index == 0 {
        PreformSpec::block(half_width, length_over_width, depth)
    } else {
        PreformSpec::cylinder(cylinder_sides, half_width, length_over_width, depth)
    })
}

/// The New Design dialog's own symmetry-order/mirror/material fields, combined with an
/// already-resolved gear tooth count and [`PreformSpec`] (the caller.
///
/// `super::callbacks::setup_new_design_create_callback` -- resolves those two first, via
/// [`gear_choice_to_teeth`] then [`parse_preform_form`], since a cylinder preform's fold
/// count needs the gear already resolved; kept as separate calls rather than folded into
/// one wide function here to stay under clippy's `too_many_arguments`, matching
/// `gui::tilt_hover_preview`'s own documented preference for a handful of small calls
/// over one wide parameter list), into a [`FreshDesignSpec`] for
/// `indicatrix_cut_core::Design::fresh_from_spec`.
///
/// `symmetry_order_text` must be a
/// positive whole number; `material_index` reuses
/// [`material_name_from_index`] (the fixed built-in list -- see that
/// function's own doc comment; a brand-new design's starting material is
/// just that, a starting point the design settings panel can change to a
/// custom catalogue material afterward).
///
/// # Errors
///
/// A message when `symmetry_order_text` is not a positive whole number.
pub fn parse_new_design_form(
    gear_teeth: i32,
    preform: PreformSpec,
    symmetry_order_text: &str,
    mirror: bool,
    material_index: i32,
) -> Result<FreshDesignSpec, String> {
    let symmetry_order: u32 = symmetry_order_text.trim().parse().map_err(|_| {
        format!(
            "Symmetry order '{}' is not a whole number.",
            symmetry_order_text.trim()
        )
    })?;
    if symmetry_order < 1 {
        return Err("Symmetry order must be a positive whole number.".to_string());
    }
    Ok(FreshDesignSpec {
        gear_teeth,
        symmetry_order,
        mirror,
        material: MaterialSelection {
            name: material_name_from_index(material_index),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        preform,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::PreformShape;

    #[test]
    fn parse_preform_form_builds_a_block_at_index_zero() {
        let preform = parse_preform_form(0, "1.2", "1.5", "0.8", 96).unwrap();
        assert_eq!(preform.shape, PreformShape::Block);
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
    }

    #[test]
    fn parse_preform_form_takes_arithmetic_in_every_dimension() {
        let preform = parse_preform_form(0, "2.4 / 2", "1 + 0.5", "(0.4 + 0.4)", 96).unwrap();
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
        let message = parse_preform_form(0, "1", "1 / 0", "1", 96).unwrap_err();
        assert_eq!(
            message,
            "Length/width '1 / 0' cannot be calculated: it divides by zero."
        );
        let message = parse_preform_form(0, "wide", "1", "1", 96).unwrap_err();
        assert_eq!(message, "Half-width 'wide' is not a number.");
    }

    #[test]
    fn parse_preform_form_builds_a_cylinder_with_the_given_side_count() {
        let preform = parse_preform_form(1, "1.0", "1.0", "0.8", 64).unwrap();
        assert_eq!(preform.shape, PreformShape::Cylinder { sides: 64 });
    }

    #[test]
    fn parse_preform_form_rejects_zero_or_negative_dimensions() {
        assert!(parse_preform_form(0, "0.0", "1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "-1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "1.0", "0.0", 96).is_err());
    }

    #[test]
    fn parse_preform_form_rejects_unparseable_text() {
        assert!(parse_preform_form(0, "wide", "1.0", "0.8", 96).is_err());
    }

    // --- parse_new_design_form ---

    #[test]
    fn parse_new_design_form_round_trips_gear_symmetry_mirror_and_material() {
        let preform = PreformSpec::cylinder(80, 1.5, 1.0, 1.5);
        let spec = parse_new_design_form(80, preform, "6", false, 9).unwrap();
        assert_eq!(spec.gear_teeth, 80);
        assert_eq!(spec.symmetry_order, 6);
        assert!(!spec.mirror);
        assert_eq!(spec.material.name.as_deref(), Some("Quartz")); // builtin_preset_names()[9]
        assert_eq!(spec.preform.half_width, 1.5);
    }

    #[test]
    fn parse_new_design_form_none_material_index_means_no_material_selected() {
        let preform = PreformSpec::cylinder(50, 1.0, 1.0, 1.0);
        let spec = parse_new_design_form(50, preform, "8", true, 0).unwrap();
        assert_eq!(spec.gear_teeth, 50);
        assert_eq!(spec.material.name, None);
    }

    #[test]
    fn parse_new_design_form_rejects_a_non_positive_symmetry_order() {
        let preform = PreformSpec::cylinder(96, 1.0, 1.0, 1.0);
        assert!(parse_new_design_form(96, preform, "0", true, 0).is_err());
        assert!(parse_new_design_form(96, preform, "wide", true, 0).is_err());
    }
}

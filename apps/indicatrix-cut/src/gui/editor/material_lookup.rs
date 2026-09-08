//! The editor-side `indicatrix_cut_core::MaterialLookup` -- built-ins plus whatever
//! custom catalogue materials the render context currently has loaded -- and the small
//! pure helpers built on top of it: a `GemMaterial` that actually reflects an RI
//! override (for the optimizer and, later, the tilt curve), and the "nearest built-in
//! to this refractive index" search the catalogue-load suggestion and the
//! linked-viewport-material fallback both need.
//!
//! # Per-axis dispersion is not wired up here
//!
//! `indicatrix_vault::model::material::CustomMaterialRow::per_axis_dispersion_json`
//! exists in the schema, but nothing in this crate has ever written a real value into
//! it -- `gui::optics::crystal_optics::save_gem_material` always saves `None` for it
//! today, and `gem_material_from_row` (the one place a `CustomMaterialRow` becomes a
//! `GemMaterial`) does not parse it either. This lookup therefore has the exact same
//! "a custom material with only a flat RI/dispersion pair renders non-dispersive"
//! limitation the live-render viewport already has -- deliberately left as a documented
//! limitation (stated in the material combo's tooltip) rather than a per-axis
//! authoring surface.

use indicatrix::optics::{dispersion::DispersionModel, materials::GemMaterial};
use indicatrix_cut_core::material::{BuiltinMaterials, MaterialLookup, MaterialSelection};

/// Built-ins plus whatever custom catalogue materials the caller hands in --
/// exactly `bridge::render_thread::context::resolve_material`'s own "custom
/// materials take priority over the built-in presets" precedence, reused here so
/// a design's material resolves the SAME way in the editor as it already does in
/// the live-render viewport.
pub(super) struct EditorMaterialLookup<'a> {
    custom: &'a [GemMaterial],
}

impl<'a> EditorMaterialLookup<'a> {
    pub(super) const fn new(custom: &'a [GemMaterial]) -> Self {
        Self { custom }
    }
}

impl MaterialLookup for EditorMaterialLookup<'_> {
    fn lookup(&self, name: &str) -> Option<GemMaterial> {
        self.custom
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .cloned()
            .or_else(|| BuiltinMaterials.lookup(name))
    }
}

/// Resolves `selection` to the real [`GemMaterial`] a consumer that needs actual
/// optics (the optimizer's objective, the tilt curve) should score against -- unlike
/// [`MaterialSelection::resolve`] alone, this makes `refractive_index_override`
/// actually show up in the returned material's own dispersion, not just in the
/// caller-visible `n_d` scalar.
///
/// When no override is set, this is exactly `selection.resolve(lookup).gem` -- the
/// resolved material's own real dispersion curve, unchanged. When an override is set,
/// the resolved material's dispersion is replaced with a flat (non-dispersive)
/// `DispersionModel::Cauchy { a: n_d, b: 0.0, c: 0.0 }` fit at exactly the override
/// value, so `evaluate(_)` returns the override at every wavelength -- everything else
/// about the resolved material (crystal system, absorption, birefringence, c-axis) is
/// left untouched. Deliberate simplification, not a real dispersion curve: an override
/// is, by construction, a single typed RI with no accompanying dispersion data, so
/// "flat at the override" is the only honest thing to draw.
#[must_use]
pub(super) fn resolved_gem_material(
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
) -> GemMaterial {
    let resolved = selection.resolve(lookup);
    if selection.refractive_index_override.is_none() {
        return resolved.gem;
    }
    let mut gem = resolved.gem;
    gem.dispersion = DispersionModel::Cauchy {
        a: resolved.n_d as f32,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// The built-in preset (from [`super::state::MATERIAL_PRESET_NAMES`]'s thirteen
/// real materials, index `0` "(none)" excluded) whose own `n_D` sits closest to
/// `n_d`, when that distance is within `tolerance` -- `None` when nothing built
/// in is close enough. Ties keep the first (list-order) match, so this is
/// deterministic regardless of floating-point noise between two equally-close
/// presets, consistent with this crate's "no `HashMap`/`HashSet` in a decision
/// path" rule (a plain `Vec`/array scan here, not that this feeds the solver at
/// all).
///
/// Shared by the catalogue-load "suggest a material for this schedule's RI" toast
/// (`super::loading::suggest_material_for_schedule_ri`) and, in principle, any future
/// "best built-in name for a display string" need -- there is only one such search in
/// this crate today.
#[must_use]
pub(super) fn nearest_built_in_material(n_d: f64, tolerance: f64) -> Option<(&'static str, f64)> {
    super::state::MATERIAL_PRESET_NAMES
        .iter()
        .skip(1) // index 0 is "(none)", not a real preset.
        .filter_map(|&name| {
            indicatrix_cut_core::built_in_refractive_index(name).map(|ri| (name, ri))
        })
        .map(|(name, ri)| (name, ri, (ri - n_d).abs()))
        .filter(|&(_, _, diff)| diff <= tolerance)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(name, ri, _)| (name, ri))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_prefers_a_custom_material_over_a_same_named_built_in() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Diamond".to_string();
        custom.dispersion = DispersionModel::Cauchy {
            a: 9.0,
            b: 0.0,
            c: 0.0,
        };
        let custom_list = [custom];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let found = lookup.lookup("Diamond").unwrap();
        assert!((f64::from(found.dispersion.evaluate(589.3)) - 9.0).abs() < 1e-6);
    }

    #[test]
    fn lookup_falls_back_to_a_built_in_when_no_custom_material_matches() {
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        assert!(lookup.lookup("Quartz").is_some());
        assert!(lookup.lookup("Not A Real Material").is_none());
    }

    #[test]
    fn resolved_gem_material_uses_the_real_dispersion_curve_with_no_override() {
        let selection = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = resolved_gem_material(&selection, &lookup);
        let expected = GemMaterial::by_name("Quartz").unwrap();
        assert_eq!(gem.dispersion, expected.dispersion);
    }

    #[test]
    fn resolved_gem_material_flattens_dispersion_to_the_override_value() {
        let selection = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.70),
        };
        let custom_list: [GemMaterial; 0] = [];
        let lookup = EditorMaterialLookup::new(&custom_list);
        let gem = resolved_gem_material(&selection, &lookup);
        // Evaluated at several wavelengths -- a flat fit must agree everywhere,
        // not just at the sodium D line the override was authored against.
        for lambda in [450.0f32, 589.3, 650.0] {
            assert!((f64::from(gem.dispersion.evaluate(lambda)) - 1.70).abs() < 1e-4);
        }
        // Every other field (crystal system, absorption, c-axis) stays whatever
        // the resolved base material ("Diamond") already had -- only dispersion
        // is replaced.
        let base = GemMaterial::diamond();
        assert_eq!(gem.crystal_system, base.crystal_system);
        assert_eq!(gem.absorption, base.absorption);
    }

    #[test]
    fn nearest_built_in_material_finds_quartz_for_its_own_ri() {
        let quartz_n_d = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let (name, ri) = nearest_built_in_material(quartz_n_d, 0.01).unwrap();
        assert_eq!(name, "Quartz");
        assert!((ri - quartz_n_d).abs() < 1e-9);
    }

    #[test]
    fn nearest_built_in_material_returns_none_outside_tolerance() {
        // 1.9999 sits roughly halfway between Spinel (~1.727) and Zircon
        // (~1.923)/Tanzanite (~1.69) -- nothing in the table is genuinely close,
        // so a tight tolerance must find nothing rather than force a match.
        assert_eq!(nearest_built_in_material(1.9999, 0.001), None);
    }

    #[test]
    fn nearest_built_in_material_picks_the_closest_when_two_are_within_tolerance() {
        // Diamond (~2.417) and Cubic Zirconia (~2.15-2.2ish) may both fall within
        // a generous tolerance of some intermediate n_d -- the closer one must
        // win, not merely list order.
        let diamond_n_d = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        let (name, _) = nearest_built_in_material(diamond_n_d - 0.02, 0.5).unwrap();
        assert_eq!(name, "Diamond");
    }
}

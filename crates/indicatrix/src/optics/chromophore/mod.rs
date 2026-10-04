//! Physically based chromophore absorption modeling, recipe compilation, and inverse solving.

pub mod catalogue;
pub mod fluorescence;
pub mod recipe;
pub mod resolve;
pub mod solver;

pub use catalogue::{
    BandData, ChromophoreCatalogue, ChromophoreData, EDGE_DEFAULT_FWHM_CM1, EndMemberData,
    FluorescenceData, HostData, QuenchData, SidebandData, TreatmentData, TreatmentEffectData,
    garnet_optics, species_element, unit_is_valid,
};
pub use fluorescence::{
    EmitterReport, FluorescenceReport, GlowStrength, UV365_NM, UV395_NM, UvGlow,
    fluorescence_report, resolve_fluorescence, uv_glow,
};
pub use recipe::{RecipeEntry, ResolveError, ResolveWarning, ResolvedBands, colorRecipe};
pub use resolve::resolve;
pub use solver::{
    Cancelled, FantasySolution, MAX_EVALS, SolveRequest, SolveResult, TreatmentPolicy,
    solve_fantasy, solve_fantasy_lab, solve_physics, solve_physics_with,
};

#[cfg(test)]
pub mod acceptance_tests;
#[cfg(test)]
mod measured_color_tests;
#[cfg(test)]
mod primary_data_tests;
#[cfg(test)]
mod reference_validation;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::body_color::{Illuminant, body_colors},
        render_setup::materials::effective_stone_width_mm,
    };

    #[test]
    fn catalogue_loads_and_versions() {
        let cat = ChromophoreCatalogue::global();
        assert_eq!(cat.data_version, 4);
        assert!(!cat.hosts.is_empty());
        assert!(cat.host("corundum").is_some());
        assert!(cat.host("beryl").is_some());
        assert!(cat.host("chrysoberyl").is_some());
        assert!(cat.host("diamond").is_some());
    }

    #[test]
    fn offer_rule_by_id() {
        let cat = ChromophoreCatalogue::global();
        let corundum = cat.host("corundum").expect("corundum host exists");
        let corundum_ids: Vec<&str> = corundum
            .chromophores
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert!(corundum_ids.iter().any(|id| id.contains("Cr")));
        assert!(corundum_ids.iter().any(|id| id.contains("Fe")));
        assert!(corundum_ids.iter().any(|id| id.contains("Ti")));
        assert!(corundum_ids.iter().any(|id| id.contains('V')));

        let chrysoberyl = cat.host("chrysoberyl").expect("chrysoberyl host exists");
        let chrysoberyl_ids: Vec<&str> = chrysoberyl
            .chromophores
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert!(chrysoberyl_ids.iter().any(|id| id.contains("Cr")));

        let diamond = cat.host("diamond").expect("diamond host exists");
        let diamond_ids: Vec<&str> = diamond.chromophores.iter().map(|c| c.id.as_str()).collect();
        assert!(diamond_ids.iter().any(|id| id.contains('N')));
    }

    #[test]
    fn stone_size_defaults_acceptance_criterion() {
        assert_eq!(effective_stone_width_mm(0.0, true), 7.0);
        assert_eq!(effective_stone_width_mm(0.0, false), 0.0);
        assert_eq!(effective_stone_width_mm(5.0, true), 5.0);
    }

    #[test]
    fn corundum_cr_resolves_and_produces_red() {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = colorRecipe::new("corundum", cat.data_version);
        recipe.set_amount("Cr", 0.3); // 0.3 wt% Cr2O3 ruby
        let (tensor, _) = resolve(&recipe, cat).expect("resolves cleanly");
        assert!(tensor.o_ray.len() <= 8);
        assert!(tensor.e_ray.len() <= 8);
        assert!(!tensor.o_ray.is_empty());

        let colors = body_colors(&tensor, 5.0, Illuminant::D65);
        // Ruby should have positive a* (redness)
        assert!(
            colors.unpolarised.lab[1] > 10.0,
            "Expected red ruby, got a* = {}",
            colors.unpolarised.lab[1]
        );
    }

    #[test]
    fn blue_sapphire_recipe_resolves_with_pleochroism() {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = colorRecipe::new("corundum", cat.data_version);
        recipe.set_amount("Fe", 1000.0); // 1000 ppm
        recipe.set_amount("Ti", 100.0); // 100 ppm
        let (tensor, _) = resolve(&recipe, cat).expect("resolves cleanly");
        assert!(tensor.is_pleochroic);

        let colors = body_colors(&tensor, 5.0, Illuminant::D65);
        // Blue sapphire should have negative b* (blueness)
        assert!(
            colors.unpolarised.lab[2] < 0.0,
            "Expected blue sapphire, got b* = {}",
            colors.unpolarised.lab[2]
        );
    }

    #[test]
    fn solver_physics_ruby_roundtrip() {
        let cat = ChromophoreCatalogue::global();
        let mut initial_recipe = colorRecipe::new("corundum", cat.data_version);
        initial_recipe.set_amount("Cr", 0.3);
        let (tensor, _) = resolve(&initial_recipe, cat).expect("resolves cleanly");
        let target_col = body_colors(&tensor, 5.0, Illuminant::D65);

        let res = solve_physics(cat, "corundum", target_col.unpolarised.lab, None, 5.0, &[]);
        assert!(
            res.delta_e <= 1.0,
            "Expected roundtrip delta_e <= 1.0, got {}",
            res.delta_e
        );
        assert!(res.reachable);
        assert!(res.recipe.entries.iter().any(|e| e.id == "Cr"));
    }
}

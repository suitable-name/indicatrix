//! Tests for the specific-gravity table, [`MaterialSelection`] resolution, and
//! [`MaterialCatalogue`].

use super::*;
use crate::optics_hints::critical_angle_deg;
use indicatrix::optics::materials::GemMaterial;

#[test]
fn known_preset_names_resolve() {
    for name in [
        "Diamond",
        "Sapphire",
        "Ruby",
        "Emerald",
        "Zircon",
        "Alexandrite",
        "Topaz",
        "Spinel",
        "Quartz",
        "Tourmaline",
        "Tanzanite",
        "Synthetic Moissanite",
        "Cubic Zirconia",
    ] {
        let sg = built_in_specific_gravity(name)
            .unwrap_or_else(|| panic!("{name} must have a built-in SG"));
        assert!(sg.representative > 0.0, "{name}: {sg:?}");
        assert!(
            sg.range.0 <= sg.representative && sg.representative <= sg.range.1,
            "{name}: representative {} not inside its own range {:?}",
            sg.representative,
            sg.range
        );
    }
}

/// There is no "Garnet" preset (or any other name outside the thirteen above).
#[test]
fn unknown_names_including_garnet_resolve_to_none() {
    assert_eq!(built_in_specific_gravity("Garnet"), None);
    assert_eq!(built_in_specific_gravity("Not A Real Material"), None);
    assert_eq!(built_in_specific_gravity(""), None);
}

#[test]
fn none_selection_has_no_effective_sg() {
    assert_eq!(MaterialSelection::none().effective_specific_gravity(), None);
    assert_eq!(MaterialSelection::default(), MaterialSelection::none());
}

#[test]
fn preset_alone_uses_its_representative_figure() {
    let m = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    assert_eq!(m.effective_specific_gravity(), Some(3.52));
}

/// An override wins even when the preset name is also known -- the user's own
/// number is always authoritative over the table.
#[test]
fn override_wins_over_a_known_preset() {
    let m = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: Some(3.515),
        refractive_index_override: None,
    };
    assert_eq!(m.effective_specific_gravity(), Some(3.515));
}

/// An override is exactly what makes a species with no preset (garnet) usable.
#[test]
fn override_alone_works_for_a_species_with_no_preset() {
    let m = MaterialSelection {
        name: Some("Garnet".to_string()),
        specific_gravity_override: Some(3.90),
        refractive_index_override: None,
    };
    assert_eq!(m.effective_specific_gravity(), Some(3.90));
}

// --- MaterialSelection::with_specific_gravity_override ---

/// The one field named in the call must change; `name` and
/// `refractive_index_override` must carry through untouched -- the exact
/// property that makes this safe for a caller (the Yield form parser) that
/// owns only the SG field to use without reconstructing the whole struct.
#[test]
fn with_specific_gravity_override_replaces_only_that_field() {
    let original = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: Some(3.50),
        refractive_index_override: Some(2.42),
    };
    let updated = original.with_specific_gravity_override(Some(3.55));
    assert_eq!(updated.specific_gravity_override, Some(3.55));
    assert_eq!(updated.name, original.name);
    assert_eq!(
        updated.refractive_index_override,
        original.refractive_index_override
    );
}

/// Passing `None` clears the override (back to "use the preset's own figure")
/// while still leaving `name`/`refractive_index_override` alone.
#[test]
fn with_specific_gravity_override_can_clear_the_override() {
    let original = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: Some(2.70),
        refractive_index_override: Some(1.55),
    };
    let updated = original.with_specific_gravity_override(None);
    assert_eq!(updated.specific_gravity_override, None);
    assert_eq!(updated.name, Some("Quartz".to_string()));
    assert_eq!(updated.refractive_index_override, Some(1.55));
}

// --- MaterialSelection::resolve / built_in_refractive_index ---

#[test]
fn resolve_a_known_preset_returns_that_material_and_its_own_n_d() {
    let m = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let resolved = m.resolve(&BuiltinMaterials);
    assert_eq!(
        resolved.gem.name,
        GemMaterial::by_name("Quartz").unwrap().name
    );
    let expected_n_d = f64::from(
        GemMaterial::by_name("Quartz")
            .unwrap()
            .dispersion
            .evaluate(589.3),
    );
    assert!((resolved.n_d - expected_n_d).abs() < 1e-9);
    assert!((resolved.critical_angle_deg - critical_angle_deg(expected_n_d)).abs() < 1e-9);
}

/// No selection at all (a brand-new design) resolves to diamond.
#[test]
fn resolve_with_no_name_falls_back_to_diamond() {
    let resolved = MaterialSelection::none().resolve(&BuiltinMaterials);
    assert_eq!(resolved.gem.name, GemMaterial::diamond().name);
}

/// A name `BuiltinMaterials` does not recognize falls back to diamond too,
/// rather than panicking or silently doing nothing.
#[test]
fn resolve_with_an_unrecognized_name_falls_back_to_diamond() {
    let m = MaterialSelection {
        name: Some("Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let resolved = m.resolve(&BuiltinMaterials);
    assert_eq!(resolved.gem.name, GemMaterial::diamond().name);
}

/// `refractive_index_override` must win over the resolved material's own `n_D`.
#[test]
fn resolve_refractive_index_override_wins_over_the_resolved_material() {
    let m = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.70),
    };
    let resolved = m.resolve(&BuiltinMaterials);
    assert!((resolved.n_d - 1.70).abs() < 1e-12);
    assert!((resolved.critical_angle_deg - critical_angle_deg(1.70)).abs() < 1e-9);
}

#[test]
fn built_in_refractive_index_matches_the_material_own_dispersion_curve() {
    let expected = f64::from(GemMaterial::diamond().dispersion.evaluate(589.3));
    assert!((built_in_refractive_index("Diamond").unwrap() - expected).abs() < 1e-9);
    assert_eq!(built_in_refractive_index("Garnet"), None);
    assert_eq!(built_in_refractive_index("Not A Real Material"), None);
}

// --- MaterialLookup::specific_gravity / effective_specific_gravity_with ---

/// [`BuiltinMaterials::specific_gravity`] must agree with
/// [`built_in_specific_gravity`] for every known preset.
#[test]
fn builtin_materials_specific_gravity_matches_the_table() {
    assert_eq!(BuiltinMaterials.specific_gravity("Diamond"), Some(3.52));
    assert_eq!(BuiltinMaterials.specific_gravity("Garnet"), None);
}

/// A [`MaterialLookup`] that never overrides [`MaterialLookup::specific_gravity`]
/// keeps compiling and simply reports `None` -- the default-method contract
/// [`MaterialLookup::specific_gravity`]'s own doc comment promises.
#[test]
fn a_lookup_with_no_sg_override_reports_none() {
    struct LookupWithNoSg;
    impl MaterialLookup for LookupWithNoSg {
        fn lookup(&self, name: &str) -> Option<GemMaterial> {
            GemMaterial::by_name(name)
        }
    }
    assert_eq!(LookupWithNoSg.specific_gravity("Diamond"), None);
}

/// [`MaterialSelection::effective_specific_gravity_with`] must match the
/// built-ins-only [`MaterialSelection::effective_specific_gravity`] when the
/// catalogue is [`BuiltinMaterials`].
#[test]
fn effective_specific_gravity_with_matches_built_ins_only_path() {
    let m = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    assert_eq!(
        m.effective_specific_gravity_with(&BuiltinMaterials),
        m.effective_specific_gravity()
    );
}

/// The per-design override still wins over whatever the catalogue reports.
#[test]
fn effective_specific_gravity_with_override_wins_over_the_catalogue() {
    let m = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: Some(3.515),
        refractive_index_override: None,
    };
    assert_eq!(
        m.effective_specific_gravity_with(&BuiltinMaterials),
        Some(3.515)
    );
}

/// A catalogue that DOES know a custom material's SG (unlike the built-ins-only
/// path, which has no such entry) must have it reach the effective figure.
#[test]
fn effective_specific_gravity_with_resolves_a_custom_material_the_built_in_table_cannot() {
    struct CustomOnlyLookup;
    impl MaterialLookup for CustomOnlyLookup {
        fn lookup(&self, name: &str) -> Option<GemMaterial> {
            GemMaterial::by_name(name)
        }
        fn specific_gravity(&self, name: &str) -> Option<f64> {
            (name == "My Garnet").then_some(3.90)
        }
    }
    let m = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    assert_eq!(m.effective_specific_gravity(), None);
    assert_eq!(
        m.effective_specific_gravity_with(&CustomOnlyLookup),
        Some(3.90)
    );
}

// --- MaterialCatalogue ---

/// Every one of `GemMaterial::all_materials()`'s built-ins must appear, in that
/// function's own order -- the whole point of this type: a species available on
/// the render side can no longer be missing from a picker built off this
/// catalogue.
#[test]
fn catalogue_lists_every_builtin_in_all_materials_order() {
    let catalogue = MaterialCatalogue::build(&[]);
    let expected: Vec<String> = GemMaterial::all_materials()
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(catalogue.names(), expected);
    assert!(
        catalogue
            .entries()
            .iter()
            .all(|e| e.kind == MaterialKind::BuiltIn)
    );
}

/// Customs are appended after every built-in, sorted by name case-insensitively
/// -- not in `custom`'s own (arbitrary, load-order) sequence.
#[test]
fn catalogue_appends_customs_sorted_by_name_after_every_builtin() {
    let mut a = GemMaterial::diamond();
    a.name = "Zircon-ish Garnet".to_string();
    let mut b = GemMaterial::diamond();
    b.name = "amber quartz".to_string();
    let catalogue = MaterialCatalogue::build(&[a, b]);
    let names = catalogue.names();
    let builtin_count = GemMaterial::all_materials().len();
    assert_eq!(names.len(), builtin_count + 2);
    // Case-insensitive sort: "amber quartz" sorts before "Zircon-ish Garnet"
    // despite its lowercase leading letter.
    assert_eq!(names[builtin_count], "amber quartz");
    assert_eq!(names[builtin_count + 1], "Zircon-ish Garnet");
    assert_eq!(
        catalogue.entries()[builtin_count].kind,
        MaterialKind::Custom
    );
}

#[test]
fn catalogue_find_is_case_insensitive() {
    let catalogue = MaterialCatalogue::build(&[]);
    let found = catalogue.find("diamond").expect("Diamond must resolve");
    assert_eq!(found.name, "Diamond");
    assert!(catalogue.find("Not A Real Material").is_none());
}

/// Every built-in entry's `ri_d`/`birefringent` must agree with the resolved
/// `GemMaterial` itself -- the catalogue must describe the real material, not an
/// approximation of it.
#[test]
fn catalogue_entries_carry_the_real_material_ri_and_birefringence() {
    let catalogue = MaterialCatalogue::build(&[]);
    let diamond = catalogue.find("Diamond").unwrap();
    assert!((diamond.ri_d - 2.417).abs() < 0.01);
    assert!(!diamond.birefringent, "Diamond is isotropic");
    assert!(!diamond.has_absorption, "Diamond has no absorption bands");

    let sapphire = catalogue.find("Sapphire").unwrap();
    assert!(sapphire.birefringent, "Sapphire is uniaxial");
    assert!(
        sapphire.has_absorption,
        "Sapphire carries real absorption bands"
    );
}

/// A built-in the SG table covers reports it; a custom material with no SG side
/// channel reports `None` rather than a fabricated figure.
#[test]
fn catalogue_sg_is_populated_for_known_builtins_and_none_for_an_unrecorded_custom() {
    let mut custom = GemMaterial::diamond();
    custom.name = "My Garnet".to_string();
    let catalogue = MaterialCatalogue::build(&[custom]);
    assert_eq!(catalogue.find("Diamond").unwrap().sg, Some(3.52));
    assert_eq!(catalogue.find("My Garnet").unwrap().sg, None);
}

/// [`MaterialCatalogue::build_with_sg`] resolves a custom entry's SG from the
/// caller-supplied side channel, case-insensitively.
#[test]
fn catalogue_build_with_sg_resolves_a_custom_materials_specific_gravity() {
    let mut custom = GemMaterial::diamond();
    custom.name = "My Garnet".to_string();
    let sg_entries = [("my garnet".to_string(), 3.90)];
    let catalogue = MaterialCatalogue::build_with_sg(&[custom], &sg_entries);
    assert_eq!(catalogue.find("My Garnet").unwrap().sg, Some(3.90));
}

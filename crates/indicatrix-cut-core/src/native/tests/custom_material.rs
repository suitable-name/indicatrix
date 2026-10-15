//! A custom material's optics (and its own refractive index) surviving a
//! save/open round trip through [`crate::native::DesignExtras::custom_material`] and
//! [`crate::native::SaveExtras::custom_catalogue`], instead of silently
//! resolving to Diamond or the legacy schedule RI.

use super::fixtures::{FULLY_CANONICAL_ASC, fully_anchored_design, reopen, simple_design};
use indicatrix::optics::materials::GemMaterial;

use crate::{
    design::Design,
    material::MaterialSelection,
    native::{
        CustomMaterialSnapshot, DesignExtras, LoadedDesign, MaterialResolution, SaveExtras,
        catalogue_asc_extended, catalogue_asc_extended_from_solved, design_from_str,
        design_to_file, design_to_string, gem_material_from_custom_snapshot,
    },
};

/// `design` written as a design file with `snapshot` attached as its custom material,
/// and opened again: the file text and what the load found.
fn save_and_open(design: &Design, snapshot: &CustomMaterialSnapshot) -> (String, LoadedDesign) {
    let extras = DesignExtras {
        custom_material: Some(snapshot),
        ..DesignExtras::default()
    };
    let text = design_to_string(design, None, &extras).expect("must save");
    let loaded = design_from_str(&text).expect("must load back");
    (text, loaded)
}

/// A `[material.custom]` snapshot attached via [`DesignExtras::custom_material`] must
/// actually reach the written file's `material.custom` field.
#[test]
fn a_custom_material_snapshot_is_written_when_supplied_via_design_extras() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    let extras = DesignExtras {
        custom_material: Some(&snapshot),
        ..DesignExtras::default()
    };
    let file = design_to_file(&design, None, &extras);
    assert_eq!(file.material.custom, Some(snapshot));
}

/// A design naming an unrecognized material with no `[material.custom]` snapshot
/// (an older file, or one whose material was never actually custom) must report
/// [`MaterialResolution::Unresolved`] but nothing restorable -- there is nothing to
/// reconstruct from.
#[test]
fn an_unresolved_material_with_no_snapshot_has_nothing_restorable() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());

    let loaded = reopen(&design);
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, None);
}

/// The actual fix: a custom material's snapshot, once attached on save, comes back
/// out of `design_from_str` as `restorable_custom_material` -- a caller (the editor)
/// can then reconstruct the real material via `gem_material_from_custom_snapshot` and
/// register it, instead of `MaterialSelection::resolve` silently falling back to
/// Diamond.
#[test]
fn an_unresolved_material_with_a_snapshot_is_restorable_after_a_round_trip() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let snapshot = CustomMaterialSnapshot::new(
        1.62,
        0.017,
        -0.021,
        Some(3.06),
        "Trigonal",
        "UniaxialNegative",
    );
    let (_, loaded) = save_and_open(&design, &snapshot);
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, Some(snapshot.clone()));

    let rebuilt = gem_material_from_custom_snapshot("My Bespoke Gemstone", &snapshot);
    assert_eq!(rebuilt.name, "My Bespoke Gemstone");
    assert!((f64::from(rebuilt.birefringence_delta) - (-0.021)).abs() < 1e-6);
}

/// A custom material's body color travels in the snapshot: written to the file,
/// read back out of `design_from_str`, and rebuilt into the same absorption the original
/// `GemMaterial::new_custom` call produces -- instead of a restored material being
/// colorless.
#[test]
fn a_custom_materials_body_color_survives_a_save_and_open() {
    // Exactly representable, so the f64 snapshot fields cast back to the same f32s.
    let (ri, dispersion, birefringence) = (1.75_f32, 0.015_625_f32, -0.007_812_5_f32);
    let color = [0.2_f32, 1.4, 2.8];
    let mut design = simple_design();
    design.material.name = Some("My Ruby".to_string());
    let snapshot = CustomMaterialSnapshot::new(
        f64::from(ri),
        f64::from(dispersion),
        f64::from(birefringence),
        None,
        "Trigonal",
        "UniaxialNegative",
    )
    .with_body_color(Some(color));
    let (text, loaded) = save_and_open(&design, &snapshot);
    // The TOML writer spreads arrays over several lines: compare without whitespace.
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("absorption_rgb=[0.2,1.4,2.8"),
        "the file shows the color as typed, not as widened f32 digits:\n{text}"
    );

    let restored = loaded
        .restorable_custom_material
        .expect("the snapshot is restorable");
    assert_eq!(restored.body_color(), Some(color), "identical f32 bits");

    let rebuilt = gem_material_from_custom_snapshot("My Ruby", &restored);
    let original = GemMaterial::new_custom("My Ruby", ri, dispersion, birefringence, color);
    assert_eq!(rebuilt.absorption, original.absorption);
    let colorless = GemMaterial::new_custom("My Ruby", ri, dispersion, birefringence, [0.0; 3]);
    assert_ne!(rebuilt.absorption, colorless.absorption);
}

/// A snapshot without a color (a file written before the field existed, or a
/// colorless material) writes no `absorption_rgb` key and restores colorless.
#[test]
fn a_snapshot_without_a_color_writes_no_key_and_restores_colorless() {
    let mut design = simple_design();
    design.material.name = Some("Clear Glass".to_string());
    let snapshot = CustomMaterialSnapshot::new(1.5, 0.0, 0.0, None, "Cubic", "Isotropic");
    let (text, loaded) = save_and_open(&design, &snapshot);
    assert!(!text.contains("absorption_rgb"));

    let restored = loaded
        .restorable_custom_material
        .expect("the snapshot is restorable");
    assert_eq!(restored.absorption_rgb, None);
    let rebuilt = gem_material_from_custom_snapshot("Clear Glass", &restored);
    let clear = GemMaterial::new_custom("Clear Glass", 1.5, 0.0, 0.0, [0.0; 3]);
    assert_eq!(rebuilt.absorption, clear.absorption);
}

/// A material that DOES resolve against the built-in table must never be flagged as
/// restorable, even if a snapshot happens to be present (defensive -- this should
/// never occur in practice, since a caller only ever attaches a snapshot for a
/// material its own catalogue considers custom).
#[test]
fn a_known_material_is_never_flagged_as_restorable_even_with_a_snapshot_present() {
    let design = simple_design(); // material.name == Some("Diamond")
    let snapshot =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative");
    let (_, loaded) = save_and_open(&design, &snapshot);
    assert_eq!(loaded.material_resolution, MaterialResolution::Known);
    assert_eq!(loaded.restorable_custom_material, None);
}

/// For a solved design, `catalogue_asc_extended(design, ...)` and
/// `catalogue_asc_extended_from_solved(design, &design.solve().unwrap(), ...)` must
/// produce an identical `CatalogueAsc` -- same `asc_text`/`asc_preserved`/
/// `draft_reason`. `CatalogueAsc` derives no `PartialEq`, so each field is checked
/// individually.
#[test]
fn catalogue_asc_extended_and_from_solved_agree_for_a_solved_design() {
    let design = fully_anchored_design();
    let solved = design.solve().expect("fully_anchored_design always solves");

    let via_extended = catalogue_asc_extended(
        &design,
        Some(FULLY_CANONICAL_ASC),
        None,
        &SaveExtras::default(),
    )
    .expect("catalogue_asc_extended must write");
    let via_solved = catalogue_asc_extended_from_solved(
        &design,
        &solved,
        Some(FULLY_CANONICAL_ASC),
        None,
        &SaveExtras::default(),
    )
    .expect("catalogue_asc_extended_from_solved must write");

    assert_eq!(via_extended.asc_text, via_solved.asc_text);
    assert_eq!(via_extended.asc_preserved, via_solved.asc_preserved);
    assert!(via_extended.draft_reason.is_none());
    assert!(via_solved.draft_reason.is_none());
}

/// A [`indicatrix::optics::materials::GemMaterial`] named `"My Garnet"` with a
/// chosen `n_D`, mirroring
/// `crates/indicatrix-cut-core/src/design/export.rs`'s own test fixture of the same
/// name.
fn custom_garnet(n_d: f32) -> indicatrix::optics::materials::GemMaterial {
    let mut gem = indicatrix::optics::materials::GemMaterial::diamond();
    gem.name = "My Garnet".to_string();
    gem.dispersion = indicatrix::optics::dispersion::DispersionModel::Cauchy {
        a: n_d,
        b: 0.0,
        c: 0.0,
    };
    gem
}

/// A fully anchored design on the named material, no override of any kind.
fn anchored_design_on(name: &str) -> Design {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

/// `catalogue_asc_extended` must write an `I` line matching a CUSTOM catalogue
/// material's own `n_D` (here 1.9) once `extras.custom_catalogue` names it, while
/// the same call with an empty catalogue -- given the exact same design -- falls
/// back to the legacy schedule RI. No `original_asc_text` is supplied, so both
/// writes freshly regenerate `asc_text` rather than preserving stale text (which
/// would mask the very `I`-line difference this test checks).
#[test]
fn catalogue_asc_extended_resolves_a_custom_materials_own_refractive_index() {
    let design = anchored_design_on("My Garnet");
    let custom = [custom_garnet(1.9)];

    let with_catalogue = catalogue_asc_extended(
        &design,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("catalogue_asc_extended must write");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = catalogue_asc_extended(&design, None, None, &SaveExtras::default())
        .expect("catalogue_asc_extended must write");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// Same check via the already-solved entry point `catalogue_asc_extended_from_solved`.
#[test]
fn catalogue_asc_extended_from_solved_resolves_a_custom_materials_own_refractive_index() {
    let design = anchored_design_on("My Garnet");
    let custom = [custom_garnet(1.9)];
    let solved = design.solve().expect("fully_anchored_design always solves");

    let with_catalogue = catalogue_asc_extended_from_solved(
        &design,
        &solved,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("catalogue_asc_extended_from_solved must write");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only =
        catalogue_asc_extended_from_solved(&design, &solved, None, None, &SaveExtras::default())
            .expect("catalogue_asc_extended_from_solved must write");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// A design on a recognized built-in ("Diamond") must export byte-identical
/// `asc_text` whether or not an UNRELATED custom catalogue is supplied -- built-ins
/// take precedence over nothing here, since `custom` has no "Diamond" entry of its own.
#[test]
fn catalogue_asc_extended_matches_byte_for_byte_on_a_built_in_regardless_of_catalogue() {
    let design = anchored_design_on("Diamond");
    let custom = [custom_garnet(1.9)];

    let without_catalogue = catalogue_asc_extended(&design, None, None, &SaveExtras::default())
        .expect("catalogue_asc_extended must write");
    let with_catalogue = catalogue_asc_extended(
        &design,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("catalogue_asc_extended must write");

    assert_eq!(without_catalogue.asc_text, with_catalogue.asc_text);
    let schedule =
        indicatrix_formats::asc::parse_asc(&without_catalogue.asc_text).expect("must parse");
    assert!((schedule.refractive_index - 2.417).abs() < 1e-3);
}

// --- physics color recipe (spec 6 / 11.9) ---

mod physics_color {
    use super::*;
    use crate::{
        material::{ActiveColor, ColorMode, color::fantasy_lab},
        native::{
            SnapshotColor, color_recipe_dto, gem_material_from_custom_snapshot_keeping_recipe,
            snapshot_color,
        },
    };
    use indicatrix::{
        color::body_color::{Illuminant, body_colors, delta_e_2000},
        optics::chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
    };

    fn ruby_mode() -> ColorMode {
        let cat = ChromophoreCatalogue::global();
        let mut recipe = ColorRecipe::new("corundum", cat.data_version);
        recipe.set_amount("Cr", 0.3);
        let (tensor, _) = resolve(&recipe, cat).expect("ruby resolves");
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
        ColorMode::physics(recipe, [0.0; 3])
    }

    fn snapshot_for(mode: &ColorMode) -> CustomMaterialSnapshot {
        CustomMaterialSnapshot::new(1.768, 0.018, -0.008, None, "Trigonal", "UniaxialNegative")
            .with_body_color(Some(mode.fallback_rgb()))
            .with_color_recipe(Some(color_recipe_dto(mode)))
    }

    /// Save -> open: the file carries both the recipe DTO and the fallback color, and the
    /// restored material renders from the stored `resolved_bands`.
    #[test]
    fn a_physics_recipe_survives_a_save_and_open() {
        let mode = ruby_mode();
        let snapshot = snapshot_for(&mode);
        let mut design = simple_design();
        design.material.name = Some("Physics Ruby".to_string());
        let (_, loaded) = save_and_open(&design, &snapshot);
        let restored = loaded.restorable_custom_material.expect("restorable");
        assert!(matches!(
            snapshot_color(&restored),
            SnapshotColor::Physics(_)
        ));
        let rebuilt = gem_material_from_custom_snapshot("Physics Ruby", &restored);
        assert_eq!(
            rebuilt.absorption,
            mode.last_recipe
                .as_ref()
                .unwrap()
                .resolved_bands
                .to_tensor()
        );
    }

    /// An older build ignores the recipe and opens the fallback `absorption_rgb` only: its
    /// color must be near the physics color (`DeltaE00` <= 15).
    #[test]
    fn an_older_build_opens_the_fallback_color_within_delta_e_15() {
        let mode = ruby_mode();
        let snapshot = snapshot_for(&mode);
        // What an older build does: `new_custom` from the top-level color, no recipe.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "GemMaterial stores f32; the snapshot values are small material constants"
        )]
        let older = GemMaterial::new_custom(
            "Physics Ruby",
            snapshot.mean_ri as f32,
            snapshot.dispersion_delta as f32,
            snapshot.birefringence_delta as f32,
            snapshot.body_color().expect("fallback color written"),
        );
        let older_lab = body_colors(&older.absorption, 1.0, Illuminant::D65)
            .unpolarised
            .lab;
        let physics_lab = mode.recipe_lab().expect("has recipe");
        let de = delta_e_2000(physics_lab, older_lab);
        assert!(
            de <= 15.0,
            "older build shows DeltaE {de:.1} from the recipe"
        );
        assert!((delta_e_2000(fantasy_lab(snapshot.body_color().unwrap()), older_lab)) < 1e-6);
    }

    /// An older build that changed the top-level color: detected, defaults to the edited
    /// color, and keeps the recipe on request.
    #[test]
    fn a_color_edited_by_an_older_build_is_detected() {
        let mode = ruby_mode();
        let mut snapshot = snapshot_for(&mode);
        snapshot = snapshot.with_body_color(Some([0.1, 0.2, 2.5]));
        assert!(matches!(
            snapshot_color(&snapshot),
            SnapshotColor::EditedElsewhere(_)
        ));
        let edited = gem_material_from_custom_snapshot("Physics Ruby", &snapshot);
        let kept = gem_material_from_custom_snapshot_keeping_recipe("Physics Ruby", &snapshot);
        let fantasy = GemMaterial::new_custom("x", 1.768, 0.018, -0.008, [0.1, 0.2, 2.5]);
        assert_eq!(edited.absorption, fantasy.absorption);
        assert_eq!(
            kept.absorption,
            mode.last_recipe
                .as_ref()
                .unwrap()
                .resolved_bands
                .to_tensor()
        );
    }

    /// A material saved while fantasy is active is plain fantasy on restore.
    #[test]
    fn a_fantasy_active_material_restores_as_fantasy() {
        let mut mode = ruby_mode();
        mode.fantasy_rgb = [0.3, 0.6, 1.2];
        mode.switch_to_fantasy();
        assert_eq!(mode.active, ActiveColor::Fantasy);
        let snapshot = snapshot_for(&mode);
        assert_eq!(snapshot.body_color(), Some([0.3, 0.6, 1.2]));
        assert_eq!(snapshot_color(&snapshot), SnapshotColor::Fantasy);
    }
}

/// The N-band design colour (bands + path scale next to the triple) survives save/open, and a
/// design saved without bands opens with none.
#[test]
fn a_design_body_color_bands_override_round_trips_next_to_the_triple() {
    let mut design = simple_design();
    design.material = MaterialSelection::none().with_body_color_bands(
        Some([0.2, 0.4, 2.8]),
        Some(vec![[460.0, 45.0, 0.25], [620.0, 45.0, 0.5]]),
        Some(24.5),
    );
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must save");
    assert!(text.contains("body_color_bands_override"));
    let loaded = reopen(&design);
    assert_eq!(loaded.design.material, design.material);

    design.material = MaterialSelection::none().with_body_color(Some([0.2, 0.4, 2.8]));
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must save");
    assert!(!text.contains("body_color_bands_override"));
    let loaded = reopen(&design);
    assert_eq!(loaded.design.material.body_color_bands_override, None);
    assert_eq!(loaded.design.material.absorption_path_scale_override, None);
}

/// A snapshot's `absorption_bands_per_mm` wins over the triple next to it; an old snapshot
/// (triple only) rebuilds exactly as before.
#[test]
fn a_custom_snapshot_with_bands_rebuilds_from_the_bands() {
    let rows = [[460.0_f32, 45.0, 0.2], [620.0, 45.0, 0.6]];
    let base =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative")
            .with_body_color(Some([0.2, 0.6, 1.8]));
    let old = gem_material_from_custom_snapshot("Bespoke", &base);
    let banded = gem_material_from_custom_snapshot("Bespoke", &base.with_absorption_bands(&rows));
    assert_ne!(old.absorption, banded.absorption);
    assert_eq!(banded.absorption.o_ray.len(), 2);
    assert_eq!(old.absorption.o_ray.len(), 3);
}

// --- crystal optics read back from the snapshot's own names and biaxial delta ---

mod crystal_optics {
    use super::*;
    use crate::{
        material::{ActiveColor, ColorMode},
        native::color_recipe_dto,
    };
    use indicatrix::optics::{
        chromophore::{ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve},
        materials::{AbsorptionUnit, CrystalSystem, OpticalCharacter},
    };

    /// A physics recipe for the biaxial host tanzanite survives save -> open with the crystal
    /// system, optical character and biaxial delta the snapshot carries, instead of coming back
    /// uniaxial with no delta (which left the recipe's `beta_ray` unused by the renderer).
    #[test]
    fn a_biaxial_physics_materials_crystal_optics_survive_a_save_and_open() {
        let catalogue = ChromophoreCatalogue::global();
        let mut recipe = ColorRecipe::new("tanzanite", catalogue.data_version);
        recipe.set_amount("V", 0.1);
        let (tensor, _) = resolve(&recipe, catalogue).expect("tanzanite resolves");
        assert!(tensor.beta_ray.is_some());
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
        let mode = ColorMode {
            active: ActiveColor::Physics,
            fantasy_rgb: [0.0; 3],
            last_recipe: Some(recipe),
        };
        // Built the way `LibraryMaterial::snapshot` builds one.
        let snapshot = CustomMaterialSnapshot::new(
            1.70,
            0.02,
            0.009,
            Some(3.35),
            "Orthorhombic",
            "BiaxialPositive",
        )
        .with_biaxial_delta(Some(0.004))
        .with_body_color(Some(mode.fallback_rgb()));
        let mut dto = color_recipe_dto(&mode);
        if let Some(top_level) = snapshot.absorption_rgb {
            dto.fallback_rgb = top_level;
        }
        let snapshot = snapshot.with_color_recipe(Some(dto));

        let mut design = simple_design();
        design.material.name = Some("Biaxial Physics".to_string());
        let (text, loaded) = save_and_open(&design, &snapshot);
        assert!(text.contains("biaxial_delta_beta_alpha"), "{text}");
        let restored = loaded
            .restorable_custom_material
            .expect("the snapshot is restorable");

        let rebuilt = gem_material_from_custom_snapshot("Biaxial Physics", &restored);
        assert_eq!(rebuilt.crystal_system, CrystalSystem::Orthorhombic);
        assert_eq!(rebuilt.optical_character, OpticalCharacter::BiaxialPositive);
        let delta = rebuilt
            .biaxial_delta_beta_alpha
            .expect("the biaxial delta is restored");
        assert!((f64::from(delta) - 0.004).abs() < 1e-6, "{delta}");
        assert!(rebuilt.absorption.is_pleochroic);
        assert!(rebuilt.absorption.beta_ray.is_some());
        assert_eq!(rebuilt.absorption_unit, AbsorptionUnit::PerMm);
    }

    /// An empty or unrecognised name keeps what `GemMaterial::new_custom` infers from the sign of
    /// the birefringence, and a delta on a non-biaxial character is dropped.
    #[test]
    fn an_empty_or_unknown_name_keeps_the_inferred_crystal_optics() {
        for (system, character) in [("", ""), ("Amorphous", "Isotropy")] {
            let snapshot =
                CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, system, character);
            let material = gem_material_from_custom_snapshot("Inferred", &snapshot);
            assert_eq!(
                material.crystal_system,
                CrystalSystem::Trigonal,
                "{system:?}"
            );
            assert_eq!(
                material.optical_character,
                OpticalCharacter::UniaxialNegative,
                "{character:?}"
            );
            assert_eq!(material.biaxial_delta_beta_alpha, None);
        }

        let stray = CustomMaterialSnapshot::new(1.5, 0.0, 0.0, None, "Cubic", "Isotropic")
            .with_biaxial_delta(Some(0.004));
        let material = gem_material_from_custom_snapshot("Stray delta", &stray);
        assert_eq!(material.crystal_system, CrystalSystem::Cubic);
        assert_eq!(material.optical_character, OpticalCharacter::Isotropic);
        assert_eq!(material.biaxial_delta_beta_alpha, None);
    }

    /// A biaxial snapshot with no delta (a file written before the key existed) keeps its
    /// biaxial character and restores no delta: the documented limitation.
    #[test]
    fn a_biaxial_snapshot_without_the_delta_keeps_its_character() {
        let snapshot =
            CustomMaterialSnapshot::new(1.70, 0.02, 0.009, None, "Orthorhombic", "BiaxialNegative");
        let material = gem_material_from_custom_snapshot("Old biaxial", &snapshot);
        assert_eq!(material.crystal_system, CrystalSystem::Orthorhombic);
        assert_eq!(
            material.optical_character,
            OpticalCharacter::BiaxialNegative
        );
        assert_eq!(material.biaxial_delta_beta_alpha, None);
    }

    /// A snapshot that carries a biaxial delta but no readable optical-character name (a library
    /// row written before the names were stored, a hand-edited file, the web protocol's
    /// custom-material spec) still restores a biaxial material: the delta is only ever written
    /// for a biaxial one, and the optic-sign rule picks the sign (positive when
    /// `2 (beta - alpha) < gamma - alpha`). The crystal system is not guessed.
    #[test]
    fn a_delta_without_names_restores_a_biaxial_material_by_the_optic_sign_rule() {
        // Topaz: alpha 1.606, beta 1.609, gamma 1.617, so beta sits nearer alpha: positive.
        // Andalusite: 1.632 / 1.638 / 1.643, so beta sits nearer gamma: negative.
        let cases = [
            ("Topaz", 0.003, OpticalCharacter::BiaxialPositive),
            ("Andalusite", 0.006, OpticalCharacter::BiaxialNegative),
        ];
        for (name, delta, expected) in cases {
            // Empty names and names this build does not know both leave the character to the rule.
            for (system, character) in [("", ""), ("Amorphous", "Mystery")] {
                let snapshot =
                    CustomMaterialSnapshot::new(1.609, 0.014, 0.011, None, system, character)
                        .with_biaxial_delta(Some(delta));
                let material = gem_material_from_custom_snapshot(name, &snapshot);
                assert_eq!(
                    material.optical_character, expected,
                    "{name} {system:?} {character:?}"
                );
                let kept = material
                    .biaxial_delta_beta_alpha
                    .expect("the delta is kept");
                assert!((f64::from(kept) - delta).abs() < 1e-6, "{name}: {kept}");
                // Not guessed: what `new_custom` inferred from the birefringence.
                assert_eq!(material.crystal_system, CrystalSystem::Trigonal, "{name}");
            }

            // A crystal-system name that parses is kept; the character still comes from the rule.
            let system_only =
                CustomMaterialSnapshot::new(1.609, 0.014, 0.011, None, "Orthorhombic", "")
                    .with_biaxial_delta(Some(delta));
            let material = gem_material_from_custom_snapshot(name, &system_only);
            assert_eq!(material.crystal_system, CrystalSystem::Orthorhombic);
            assert_eq!(material.optical_character, expected, "{name}, system named");
            assert!(material.biaxial_delta_beta_alpha.is_some(), "{name}");

            // A biaxial spread is a magnitude: the sign of the stored birefringence is ignored.
            let signed = CustomMaterialSnapshot::new(1.609, 0.014, -0.011, None, "", "")
                .with_biaxial_delta(Some(delta));
            let material = gem_material_from_custom_snapshot(name, &signed);
            assert_eq!(
                material.optical_character, expected,
                "{name}, negative birefringence"
            );
        }
    }

    /// A name that parses always wins over a delta: a uniaxial or isotropic name keeps the
    /// material uniaxial or isotropic and the stray delta is dropped (today's behaviour,
    /// pinned), whatever the delta would have implied.
    #[test]
    fn a_parsed_non_biaxial_name_wins_over_a_delta_and_drops_it() {
        let cases = [
            (
                "Trigonal",
                "UniaxialPositive",
                OpticalCharacter::UniaxialPositive,
            ),
            ("", "UniaxialNegative", OpticalCharacter::UniaxialNegative),
            ("Cubic", "Isotropic", OpticalCharacter::Isotropic),
        ];
        for (system, character, expected) in cases {
            for delta in [0.003, 0.006] {
                let snapshot =
                    CustomMaterialSnapshot::new(1.609, 0.014, 0.011, None, system, character)
                        .with_biaxial_delta(Some(delta));
                let material = gem_material_from_custom_snapshot("Named", &snapshot);
                assert_eq!(
                    material.optical_character, expected,
                    "{system:?} {character:?} {delta}"
                );
                assert_eq!(material.biaxial_delta_beta_alpha, None);
            }
        }
    }

    /// A snapshot with no delta, or a delta the optic-sign rule rejects (zero, negative, not
    /// finite, larger than the birefringence), and no names is exactly what `new_custom` infers.
    #[test]
    fn a_missing_or_unusable_delta_without_names_keeps_the_inference() {
        let inferred = GemMaterial::new_custom("Inferred", 1.609, 0.014, 0.011, [0.0; 3]);
        assert_eq!(
            inferred.optical_character,
            OpticalCharacter::UniaxialPositive
        );
        let deltas = [
            None,
            Some(0.0),
            Some(-0.003),
            Some(0.012),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ];
        for delta in deltas {
            let snapshot = CustomMaterialSnapshot::new(1.609, 0.014, 0.011, None, "", "")
                .with_biaxial_delta(delta);
            let material = gem_material_from_custom_snapshot("Inferred", &snapshot);
            assert_eq!(
                material.crystal_system, inferred.crystal_system,
                "{delta:?}"
            );
            assert_eq!(
                material.optical_character, inferred.optical_character,
                "{delta:?}"
            );
            assert_eq!(material.biaxial_delta_beta_alpha, None, "{delta:?}");
        }
    }
}

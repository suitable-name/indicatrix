//! A custom material's optics (and its own refractive index) surviving a
//! save/open round trip through [`crate::native::SaveExtras::custom_material`]/
//! [`crate::native::SaveExtras::custom_catalogue`], instead of silently
//! resolving to Diamond or the legacy schedule RI.

use super::fixtures::{FULLY_CANONICAL_ASC, fully_anchored_design, simple_design};
use crate::{
    material::MaterialSelection,
    native::{
        CustomMaterialSnapshot, MaterialResolution, SaveExtras, gem_material_from_custom_snapshot,
        load_paired, save_paired, save_paired_extended, save_paired_extended_from_solved,
    },
};

/// A `[material.custom]` snapshot attached via [`SaveExtras::custom_material`] must
/// actually reach the written sidecar's `material.custom` field.
#[test]
fn a_custom_material_snapshot_is_written_when_supplied_via_save_extras() {
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
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");
    assert_eq!(saved.native.material.custom, Some(snapshot));
}

/// A design naming an unrecognized material with no `[material.custom]` snapshot
/// (an older sidecar, or one whose material was never actually custom) must report
/// [`MaterialResolution::Unresolved`] but nothing restorable -- there is nothing to
/// reconstruct from.
#[test]
fn an_unresolved_material_with_no_snapshot_has_nothing_restorable() {
    let mut design = simple_design();
    design.material.name = Some("My Bespoke Gemstone".to_string());
    let saved = save_paired(&design, "design.asc", None, None, None).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, None);
}

/// The actual fix: a custom material's snapshot, once attached on save, comes back
/// out of `load_paired` as `restorable_custom_material` -- a caller (the editor) can
/// then reconstruct the real material via `gem_material_from_custom_snapshot` and
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
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Unresolved);
    assert_eq!(loaded.restorable_custom_material, Some(snapshot.clone()));

    let rebuilt = gem_material_from_custom_snapshot("My Bespoke Gemstone", &snapshot);
    assert_eq!(rebuilt.name, "My Bespoke Gemstone");
    assert!((f64::from(rebuilt.birefringence_delta) - (-0.021)).abs() < 1e-6);
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
    let extras = SaveExtras {
        custom_material: Some(&snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");

    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    assert_eq!(loaded.material_resolution, MaterialResolution::Known);
    assert_eq!(loaded.restorable_custom_material, None);
}

/// For a solved design, `save_paired_extended(design, ...)` and
/// `save_paired_extended_from_solved(design, &design.solve().unwrap(), ...)` must
/// produce an identical `PairedSave` -- same `asc_text`/`asc_preserved`/
/// `native_toml`/`draft_reason`. `PairedSave`/`NativeDesignFile` derive no
/// `PartialEq`, so each field is checked individually.
#[test]
fn save_paired_extended_and_from_solved_agree_for_a_solved_design() {
    let design = fully_anchored_design();
    let solved = design.solve().expect("fully_anchored_design always solves");

    let via_extended = save_paired_extended(
        &design,
        "design.asc",
        Some(FULLY_CANONICAL_ASC),
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let via_solved = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        Some(FULLY_CANONICAL_ASC),
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended_from_solved must save");

    assert_eq!(via_extended.asc_text, via_solved.asc_text);
    assert_eq!(via_extended.asc_preserved, via_solved.asc_preserved);
    assert_eq!(via_extended.native_toml, via_solved.native_toml);
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

/// `save_paired_extended` must write an `I` line matching a CUSTOM catalogue
/// material's own `n_D` (here 1.9) once `extras.custom_catalogue` names it, while
/// the same call with an empty catalogue -- given the exact same design -- falls
/// back to the legacy schedule RI. No `original_asc_text` is supplied, so both
/// saves freshly regenerate `asc_text` rather than preserving stale text (which
/// would mask the very `I`-line difference this test checks).
#[test]
fn save_paired_extended_resolves_a_custom_materials_own_refractive_index() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];

    let with_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended must save");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// Same check via the already-solved entry point `save_paired_extended_from_solved`.
#[test]
fn save_paired_extended_from_solved_resolves_a_custom_materials_own_refractive_index() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];
    let solved = design.solve().expect("fully_anchored_design always solves");

    let with_catalogue = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended_from_solved must save");
    let with_catalogue_schedule = indicatrix_formats::asc::parse_asc(&with_catalogue.asc_text)
        .expect("freshly exported .asc must parse");
    assert!((with_catalogue_schedule.refractive_index - 1.9).abs() < 1e-6);

    let built_ins_only = save_paired_extended_from_solved(
        &design,
        &solved,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended_from_solved must save");
    let built_ins_only_schedule = indicatrix_formats::asc::parse_asc(&built_ins_only.asc_text)
        .expect("freshly exported .asc must parse");
    assert_eq!(
        built_ins_only_schedule.refractive_index,
        design.meta.refractive_index
    );
}

/// A design on a recognized built-in ("Diamond") must export byte-identical
/// `asc_text`/`native_toml` whether or not an UNRELATED custom catalogue is
/// supplied -- built-ins take precedence over nothing here, since `custom` has no
/// "Diamond" entry of its own.
#[test]
fn save_paired_extended_matches_byte_for_byte_on_a_built_in_regardless_of_catalogue() {
    let mut design = fully_anchored_design();
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
    };
    let custom = [custom_garnet(1.9)];

    let without_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras::default(),
    )
    .expect("save_paired_extended must save");
    let with_catalogue = save_paired_extended(
        &design,
        "design.asc",
        None,
        None,
        None,
        &SaveExtras {
            custom_material: None,
            history_entries: &[],
            custom_catalogue: &custom,
        },
    )
    .expect("save_paired_extended must save");

    assert_eq!(without_catalogue.asc_text, with_catalogue.asc_text);
    assert_eq!(without_catalogue.native_toml, with_catalogue.native_toml);
    let schedule =
        indicatrix_formats::asc::parse_asc(&without_catalogue.asc_text).expect("must parse");
    assert!((schedule.refractive_index - 2.417).abs() < 1e-3);
}

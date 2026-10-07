//! A custom material's coefficient-based dispersion curve through the design file: written
//! as `[material.custom.dispersion_model]`, read back, and rebuilt into the same curve (not
//! flattened to the Cauchy fit the plain refractive-index path builds); a snapshot without a
//! model, or with a model that cannot render, still takes the plain path.

use super::fixtures::simple_design;
use indicatrix::optics::{dispersion::DispersionModel, materials::GemMaterial};

use crate::native::{
    CustomMaterialSnapshot, DispersionModelDto, SaveExtras, dispersion_model_dto,
    gem_material_from_custom_snapshot, load_paired, save_paired_extended,
};

const NAME: &str = "BK7 Copy";

/// N-BK7's three-term Sellmeier curve: a model the plain path cannot reproduce.
fn bk7() -> DispersionModel {
    GemMaterial::by_name("Glass (N-BK7)")
        .expect("N-BK7 is a built-in")
        .dispersion
}

/// The snapshot the desktop writes for `model`: the plain numbers are the model's own
/// `n_d` and `n_F - n_C`, and the table rides along.
fn snapshot_for(model: &DispersionModel) -> CustomMaterialSnapshot {
    CustomMaterialSnapshot::new(
        f64::from(model.n_d()),
        f64::from(model.delta_f_c()),
        0.0,
        None,
        "Cubic",
        "Isotropic",
    )
    .with_dispersion_model(Some(dispersion_model_dto(model)))
}

/// Saves a design named `NAME` with `snapshot` attached and reads it back.
fn round_trip(snapshot: &CustomMaterialSnapshot) -> (String, CustomMaterialSnapshot) {
    let mut design = simple_design();
    design.material.name = Some(NAME.to_string());
    let extras = SaveExtras {
        custom_material: Some(snapshot),
        history_entries: &[],
        custom_catalogue: &[],
    };
    let saved =
        save_paired_extended(&design, "design.asc", None, None, None, &extras).expect("must save");
    let loaded = load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");
    let restored = loaded
        .restorable_custom_material
        .expect("the snapshot is restorable");
    (saved.native_toml, restored)
}

/// The model reaches the file as its own table and comes back as the identical curve.
#[test]
fn a_dispersion_model_survives_a_save_and_open() {
    let model = bk7();
    let snapshot = snapshot_for(&model);
    let (toml, restored) = round_trip(&snapshot);
    assert!(
        toml.contains("[material.custom.dispersion_model]"),
        "{toml}"
    );
    assert!(toml.contains("kind = \"sellmeier3\""), "{toml}");
    assert_eq!(restored, snapshot);

    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    assert_eq!(rebuilt.dispersion, model, "the curve is not flattened");
    assert_eq!(rebuilt.dispersion.n_d().to_bits(), model.n_d().to_bits());
    // The plain numbers alone would have built a different curve.
    let plain = GemMaterial::new_custom(
        NAME,
        restored.mean_ri as f32,
        restored.dispersion_delta as f32,
        0.0,
        [0.0; 3],
    );
    assert_ne!(rebuilt.dispersion, plain.dispersion);
}

/// A file written before the field existed (no table) rebuilds exactly as it always did.
#[test]
fn a_snapshot_without_a_model_builds_the_cauchy_fit_as_before() {
    let snapshot =
        CustomMaterialSnapshot::new(1.62, 0.017, -0.021, None, "Trigonal", "UniaxialNegative");
    let (toml, restored) = round_trip(&snapshot);
    assert!(!toml.contains("dispersion_model"), "{toml}");
    assert_eq!(restored.dispersion_model, None);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    let expected = GemMaterial::new_custom(NAME, 1.62, 0.017, -0.021, [0.0; 3]);
    assert_eq!(rebuilt, expected);
}

/// A table that cannot render (a resonance at 600 nm) is ignored: the material takes the
/// plain numbers instead of an index of 1 everywhere.
#[test]
fn an_unusable_model_in_a_file_falls_back_to_the_plain_numbers() {
    let snapshot = CustomMaterialSnapshot::new(1.62, 0.017, 0.0, None, "Cubic", "Isotropic")
        .with_dispersion_model(Some(DispersionModelDto::Sellmeier1 { b1: 1.0, c1: 0.36 }));
    let (_, restored) = round_trip(&snapshot);
    assert!(restored.dispersion_model.is_some(), "the file keeps it");
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    let expected = GemMaterial::new_custom(NAME, 1.62, 0.017, 0.0, [0.0; 3]);
    assert_eq!(rebuilt, expected);
}

/// The birefringence and colour of a model-based material come from the same rules as a
/// plain one's.
#[test]
fn a_model_based_snapshot_keeps_birefringence_and_colour() {
    let model = DispersionModel::Cauchy {
        a: 1.76,
        b: 0.004,
        c: 0.0,
    };
    let snapshot = CustomMaterialSnapshot::new(
        f64::from(model.n_d()),
        f64::from(model.delta_f_c()),
        -0.008_f64,
        None,
        "Trigonal",
        "UniaxialNegative",
    )
    .with_body_color(Some([0.2, 1.4, 2.8]))
    .with_dispersion_model(Some(dispersion_model_dto(&model)));
    let (_, restored) = round_trip(&snapshot);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    assert_eq!(rebuilt.dispersion, model);
    let expected = GemMaterial::new_custom_with_dispersion(NAME, model, -0.008, [0.2, 1.4, 2.8]);
    assert_eq!(rebuilt, expected);
}

//! The built-in "Color-Change Garnet (Pyrope-Spessartine)": identity, optics and the
//! daylight / incandescent colour swing that is the reason the entry exists.

use crate::{
    color::{
        Illuminant,
        body_color::{body_colors, srgb_to_lab},
    },
    optics::materials::{CrystalSystem, GemMaterial, OpticalCharacter},
};

const NAME: &str = "Color-Change Garnet (Pyrope-Spessartine)";

/// Reference path in model units (the garnets' band weights are tuned per model unit for a
/// ~7 mm stone, see the entry's comments), the same scale the colour-change acceptance tests
/// feed `body_colors`.
const REFERENCE_PATH: f64 = 2.0;

fn material() -> GemMaterial {
    GemMaterial::by_name(NAME).expect("the colour-change garnet must be a built-in material")
}

/// Display (sRGB-adapted) CIELAB of the transmitted body colour under `ill`.
fn display_lab(material: &GemMaterial, ill: Illuminant) -> [f64; 3] {
    let colors = body_colors(&material.absorption, REFERENCE_PATH, ill);
    srgb_to_lab(colors.unpolarised.srgb.map(|v| f64::from(v) / 255.0))
}

#[test]
fn color_change_garnet_resolves_by_name_and_is_isotropic() {
    let m = material();
    assert_eq!(m.name, NAME);
    assert_eq!(m.crystal_system, CrystalSystem::Cubic);
    assert_eq!(m.optical_character, OpticalCharacter::Isotropic);
    assert_eq!(m.birefringence_delta, 0.0);
    let n_d = m.dispersion.evaluate(589.3);
    assert!(
        (n_d - 1.760).abs() <= 0.001,
        "n_d = {n_d:.5} must be 1.760 within 0.001"
    );
    // Dispersion sits between pyrope (B-G 0.022) and spessartine (0.027): n(F) - n(C)
    // = 0.025 * 0.579 within the Cauchy fit's rounding.
    let delta_fc = m.dispersion.evaluate(486.1) - m.dispersion.evaluate(656.3);
    assert!(
        (delta_fc.abs() - 0.014_475).abs() < 5e-5,
        "n(F)-n(C) = {delta_fc:.6} must be ~0.014475"
    );
}

/// The two transmission windows (blue-green and red) either side of the strong ~573 nm
/// Cr3+/V3+ band, and the Mn2+ violet cut-off.
#[test]
fn color_change_garnet_has_two_open_windows_around_the_573_band() {
    let m = material();
    let transmittance = |lambda_nm: f32| -> f64 {
        let alpha: f32 = m
            .absorption
            .o_ray
            .iter()
            .map(|b| b.evaluate(lambda_nm))
            .sum();
        (-f64::from(alpha) * REFERENCE_PATH).exp()
    };
    assert!(transmittance(485.0) > 0.5, "blue-green window must be open");
    assert!(transmittance(660.0) > 0.5, "red window must be open");
    assert!(
        transmittance(573.0) < 0.15,
        "the 573 nm band must be strong"
    );
    assert!(
        transmittance(421.0) < 0.15,
        "the Mn2+ group must block violet"
    );
}

/// Hue swing between daylight and an incandescent lamp, same illuminant helper as the
/// alexandrite / ruby colour-change tests (`body_colors` with `Illuminant::D65` and
/// `Illuminant::Planckian(3200.0)`).
#[test]
fn color_change_garnet_shifts_clearly_and_redder_under_incandescent() {
    let m = material();
    let d65 = display_lab(&m, Illuminant::D65);
    let inc = display_lab(&m, Illuminant::Planckian(3200.0));

    let (dl, da, db) = (d65[0] - inc[0], d65[1] - inc[1], d65[2] - inc[2]);
    let de76 = db.mul_add(db, da.mul_add(da, dl * dl)).sqrt();
    println!("[colour-change garnet] D65 Lab={d65:.1?}  3200K Lab={inc:.1?}  dE76={de76:.1}");

    assert!(
        de76 > 8.0,
        "colour change D65 vs 3200 K must exceed dE76 8, got {de76:.2} (D65 {d65:?}, A {inc:?})"
    );
    assert!(
        inc[1] > d65[1],
        "the incandescent colour must be redder (higher a*): D65 a*={:.2}, 3200 K a*={:.2}",
        d65[1],
        inc[1]
    );
}

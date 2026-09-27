//! GPU-routing predicate tests: every built-in must report
//! `gpu_supported() == true`, independent of `biaxial_delta_beta_alpha`.

use crate::optics::materials::GemMaterial;
use glam::Vec3;

/// GPU routing test: `gpu_supported` must report `true` for EVERY built-in
/// material, biaxial ones (Alexandrite, Topaz, Tanzanite -- the same set
/// `only_biaxial_materials_expose_a_biaxial_indicatrix` above pins for
/// `biaxial_indicatrix`) included -- see `gpu_supported`'s own doc comment for the
/// eigenvector-conditioning fix and full re-verification that made this safe.
/// Keeping a single test that tracks the routing predicate's CURRENT contract,
/// rather than layering an exception list on top, means a future regression
/// (e.g. reintroducing a biaxial-only gate) shows up here directly.
#[test]
fn gpu_supported_is_true_for_every_built_in_material() {
    let biaxial_names = [
        "Alexandrite",
        "Topaz",
        "Tanzanite",
        "Chrysoberyl (Yellow)",
        "Peridot",
        "Andalusite",
    ];
    for material in GemMaterial::all_materials() {
        assert!(
            material.gpu_supported(),
            "{} must be reported gpu_supported() == true",
            material.name
        );
        if biaxial_names.contains(&material.name.as_str()) {
            assert!(
                material.biaxial_delta_beta_alpha.is_some(),
                "{} must actually carry biaxial data for this test to be meaningful",
                material.name
            );
        }
    }
}
/// `gpu_supported` does not depend on `biaxial_delta_beta_alpha` at all (nor on
/// anything else) -- it is `true` unconditionally: the biaxial eigenvector
/// conditioning lets Alexandrite/Topaz/Tanzanite pass the same GPU-equivalence
/// bar every other material already has to. Kept as a dedicated test (rather than
/// folded into the one above) because this predicate is itself a documented,
/// load-bearing contract point -- pinning it here is the useful signal a future
/// regression (e.g. reintroducing a biaxial-only gate) should trip.
#[test]
fn gpu_supported_no_longer_depends_on_biaxial_delta_beta_alpha() {
    let mut zircon = GemMaterial::by_name("Zircon").expect("Zircon must be a built-in");
    assert!(
        zircon.gpu_supported(),
        "Zircon (uniaxial) must be GPU-supported"
    );

    zircon.birefringence_delta *= -3.0;
    zircon.c_axis = Vec3::X;
    assert!(
        zircon.gpu_supported(),
        "changing unrelated uniaxial fields must not affect gpu_supported()"
    );

    zircon.biaxial_delta_beta_alpha = Some(0.0);
    assert!(
        zircon.gpu_supported(),
        "biaxial_delta_beta_alpha no longer gates GPU support"
    );

    zircon.biaxial_delta_beta_alpha = None;
    assert!(zircon.gpu_supported(), "and stays supported once cleared");
}

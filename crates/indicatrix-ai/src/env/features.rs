//! Compact measured material and orbit features used by the observation.

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::Design;

pub(super) fn absorption_summary(gem: &GemMaterial, wavelength_nm: f32) -> f32 {
    let sum = |bands: &[indicatrix::optics::absorption::AbsorptionBand]| {
        bands
            .iter()
            .map(|band| band.evaluate(wavelength_nm))
            .sum::<f32>()
    };
    let mut principal_sum = sum(&gem.absorption.o_ray) + sum(&gem.absorption.e_ray);
    let mut principal_count = 2.0;
    if let Some(beta) = &gem.absorption.beta_ray {
        principal_sum += sum(beta);
        principal_count = 3.0;
    }
    principal_sum / principal_count
}

pub(super) fn orbit_harmonics(design: &Design, tier_index: usize) -> (f32, f32, f32, f32) {
    let Some(tier) = design.tiers.get(tier_index) else {
        return (0.0, 1.0, 0.0, 1.0);
    };
    let gear = f64::from(design.meta.gear_teeth.unsigned_abs());
    if gear <= 0.0 || tier.indices.is_empty() {
        return (0.0, 1.0, 0.0, 1.0);
    }
    let reference = design.meta.gear_reference_angle.to_radians();
    let cheater = design
        .cheater_offset_deg(tier_index)
        .unwrap_or(0.0)
        .to_radians();
    let (mut s2, mut c2, mut s4, mut c4) = (0.0, 0.0, 0.0, 0.0);
    for index in &tier.indices {
        let theta = reference + std::f64::consts::TAU * *index / gear + cheater;
        s2 += (2.0 * theta).sin();
        c2 += (2.0 * theta).cos();
        s4 += (4.0 * theta).sin();
        c4 += (4.0 * theta).cos();
    }
    let n = tier.indices.len() as f64;
    (
        (s2 / n) as f32,
        (c2 / n) as f32,
        (s4 / n) as f32,
        (c4 / n) as f32,
    )
}

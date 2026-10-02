//! The surface-glare scale: the first-surface specular reflection of a camera path is
//! scaled, everything that entered the stone is not, and the default is bit-identical to
//! a trace without the control.

use super::super::{
    super::{
        camera::Ray, environment::LightingPreset, intersect::build_plane_soa, sampling::hash_u32,
    },
    trace_spectral_ray_with_finish_soa,
};
use crate::{geometry::cuts::StandardGemCuts, optics::materials::GemMaterial};
use glam::Vec3;

const SAMPLES: u32 = 512;

/// Traces `SAMPLES` seeded paths of one fixed oblique ray at the table of a round
/// brilliant Diamond under the classic studio rig with `glare` applied, returning each
/// sample's XYZ. The same seeds are used for every `glare`, so the stochastic decisions
/// (reflect or refract, scatter, roulette) are identical across calls.
fn samples_with_glare(glare: f32) -> Vec<Vec3> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let env = LightingPreset::RingLights
        .studio(1.0, 0.85, 0.95)
        .with_surface_glare(glare);
    (0..SAMPLES)
        .map(|i| {
            let seed = 4000 + i;
            let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
            trace_spectral_ray_with_finish_soa(
                ray,
                &planes,
                &plane_soa,
                &[],
                &diamond,
                12,
                env,
                seed,
                hero_rand,
                None,
            )
        })
        .collect()
}

fn total(samples: &[Vec3]) -> Vec3 {
    samples.iter().copied().sum()
}

/// Glare `0.0` removes the first-surface reflection: no sample gets brighter, the total
/// is strictly lower, and `0.5` lands between the two within float tolerance.
#[test]
fn surface_glare_removes_only_the_first_surface_reflection() {
    let full = samples_with_glare(1.0);
    let none = samples_with_glare(0.0);
    let half = samples_with_glare(0.5);

    for ((f, n), h) in full.iter().zip(&none).zip(&half) {
        assert!(
            n.y <= f.y + 1e-5,
            "glare 0.0 must never add radiance: {n:?} vs {f:?}"
        );
        assert!(
            h.y >= n.y - 1e-5 && h.y <= f.y + 1e-5,
            "glare 0.5 must lie between 0.0 and 1.0: {n:?} {h:?} {f:?}"
        );
    }
    let (t_full, t_none, t_half) = (total(&full).y, total(&none).y, total(&half).y);
    assert!(
        t_none < t_full,
        "the table reflection of the lit environment must be removed: {t_none} vs {t_full}"
    );
    let midpoint = f32::midpoint(t_full, t_none);
    assert!(
        (t_half - midpoint).abs() <= 1e-3 * t_full.max(1e-6),
        "a linear scale of one branch puts 0.5 at the midpoint: {t_half} vs {midpoint}"
    );
}

/// The default scale, an explicit `1.0` and out-of-range or NaN inputs (which clamp to
/// off) all reproduce the same bits.
#[test]
fn surface_glare_default_is_bit_identical() {
    let default_bits: Vec<[u32; 3]> = samples_with_glare(1.0)
        .iter()
        .map(|v| v.to_array().map(f32::to_bits))
        .collect();

    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    // The environment exactly as it was built before the control existed.
    let plain = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    for (i, expected) in (0..SAMPLES).zip(&default_bits) {
        let seed = 4000 + i;
        let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
        let v = trace_spectral_ray_with_finish_soa(
            ray,
            &planes,
            &plane_soa,
            &[],
            &diamond,
            12,
            plain,
            seed,
            hero_rand,
            None,
        );
        assert_eq!(&v.to_array().map(f32::to_bits), expected, "sample {i}");
    }
    for odd in [2.0, f32::NAN, f32::INFINITY] {
        let bits: Vec<[u32; 3]> = samples_with_glare(odd)
            .iter()
            .map(|v| v.to_array().map(f32::to_bits))
            .collect();
        assert_eq!(bits, default_bits, "glare {odd} must behave as 1.0");
    }
}

/// The setter clamps and reads back; an HDR map has no glare control.
#[test]
fn surface_glare_setter_clamps_and_reads_back() {
    let env = LightingPreset::Daylight.studio(1.0, 0.0, 0.5);
    assert_eq!(env.surface_glare().to_bits(), 1.0f32.to_bits());
    assert_eq!(
        env.with_surface_glare(0.25).surface_glare().to_bits(),
        0.25f32.to_bits()
    );
    assert_eq!(
        env.with_surface_glare(-3.0).surface_glare().to_bits(),
        0.0f32.to_bits()
    );
    assert_eq!(
        env.with_surface_glare(7.0).surface_glare().to_bits(),
        1.0f32.to_bits()
    );
    assert_eq!(
        env.with_surface_glare(f32::NAN).surface_glare().to_bits(),
        1.0f32.to_bits()
    );
}

//! Illuminant-dependent colour shift tests: ruby and alexandrite must render
//! measurably redder under warm incandescent light than under D65 daylight, driven
//! by their narrow chromophore transmission windows.

use glam::Vec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

/// Renders `material` under `lighting_preset`, averaged over `samples` independent
/// spectral ray samples (each with its own hashed seed) through the same fixed ray, and
/// returns the CIE xy chromaticity of the averaged XYZ. Averaging suppresses per-sample
/// Monte Carlo noise so the comparison below isolates the illuminant-driven colour
/// shift rather than sampling variance, following the same pattern used by
/// `render_pixel_grid_chromaticities` and the birefringence-split test above.
fn render_chromaticity_under_preset(
    material: &GemMaterial,
    planes: &[GpuFacetPlane],
    ray: Ray,
    lighting_preset: LightingPreset,
    samples: u32,
    seed_salt: u32,
) -> (f32, f32) {
    let mut xyz_sum = Vec3::ZERO;
    for i in 0..samples {
        let seed = hash_u32(seed_salt ^ hash_u32(i ^ 0x9E37_79B9));
        xyz_sum += trace_spectral_ray(
            ray,
            planes,
            material,
            12,
            lighting_preset.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let xyz_avg = xyz_sum / samples as f32;
    let sum = xyz_avg.x + xyz_avg.y + xyz_avg.z;
    (xyz_avg.x / sum, xyz_avg.y / sum)
}

/// The decisive test for Task B: real ruby shifts noticeably REDDER under warm
/// incandescent (3200K) light than under daylight (D65), because tungsten's blackbody
/// spectrum emits little energy in the blue where ruby's SMALLER of its two
/// transmission windows sits (see the Ruby entry's doc comment in
/// `GemMaterial::all_materials` for the cited Cr3+ band positions, 410nm/550nm, that
/// produce this narrow-window structure) -- daylight's relatively stronger blue content
/// lets more of that blue window through, pulling the daylight-rendered colour slightly
/// toward blue/away from red relative to incandescent. A three-broad-fixed-lobe
/// absorption model has no such narrow window to begin
/// with, so it cannot reproduce this shift; this test is what actually discriminates
/// the new banded model from the old one, rather than merely checking R > B in
/// isolation (which the old model already passed).
#[test]
fn ruby_shifts_redder_under_incandescent_than_d65() {
    const SAMPLES: u32 = 192;

    let planes = StandardGemCuts::emerald_cut();
    let ruby = GemMaterial::ruby();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let (x_d65, y_d65) = render_chromaticity_under_preset(
        &ruby,
        &planes,
        ray,
        LightingPreset::Daylight,
        SAMPLES,
        0xA5A5_0001,
    );
    let (x_inc, y_inc) = render_chromaticity_under_preset(
        &ruby,
        &planes,
        ray,
        LightingPreset::Incandescent,
        SAMPLES,
        0xA5A5_0002,
    );

    println!(
        "[ruby illuminant shift] D65 chroma=({x_d65:.4}, {y_d65:.4})  Incandescent chroma=({x_inc:.4}, {y_inc:.4})  dx={:+.4}",
        x_inc - x_d65
    );

    assert!(
        x_inc > x_d65 + 0.003,
        "Ruby's CIE x-chromaticity under Incandescent (3200K) ({x_inc:.4}) must be measurably \
         higher (redder) than under D65 Daylight ({x_d65:.4}) -- the narrow-window Cr3+ band \
         model should reproduce this well-known illuminant-dependent colour shift"
    );
}

/// Alexandrite counterpart of the ruby illuminant-shift test above -- the colour
/// change (daylight green / incandescent red) is alexandrite's DEFINING trait, driven
/// by the same narrow-transmission-window mechanism as ruby's shift (two Cr3+ bands
/// straddling the ~580/415nm Neuhaus critical values -- see the Alexandrite entry's
/// comment in `GemMaterial::all_materials`). Added alongside the trichroic
/// (three-band-set) absorption upgrade for Alexandrite specifically so that upgrade,
/// and any future per-axis amplitude retune, cannot silently weaken the colour change:
/// trichroism ADDS direction-dependence on top of the illuminant-dependence, and this
/// test pins that the illuminant-dependence survives at the face-up view (propagation
/// down `c_axis` = the n_gamma/crystal-b axis, so the ray mixes the alpha/red and
/// beta/yellow principal spectra).
///
/// Margin note (measured at this test's sample count during the trichroic upgrade):
/// the previous cited-isotropic entry measured dx = +0.0125; the trichroic entry
/// measures dx = +0.0104 at the same rays/seeds -- a mild (~17%) softening from
/// averaging direction-dependent band positions, same order of magnitude, comfortably
/// above the +0.003 assertion floor shared with the ruby test above.
#[test]
fn alexandrite_shifts_redder_under_incandescent_than_d65() {
    const SAMPLES: u32 = 192;

    let planes = StandardGemCuts::emerald_cut();
    let alexandrite =
        GemMaterial::by_name("Alexandrite").expect("Alexandrite must be a built-in material");
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let (x_d65, y_d65) = render_chromaticity_under_preset(
        &alexandrite,
        &planes,
        ray,
        LightingPreset::Daylight,
        SAMPLES,
        0xA5A5_0003,
    );
    let (x_inc, y_inc) = render_chromaticity_under_preset(
        &alexandrite,
        &planes,
        ray,
        LightingPreset::Incandescent,
        SAMPLES,
        0xA5A5_0004,
    );

    println!(
        "[alexandrite illuminant shift] D65 chroma=({x_d65:.4}, {y_d65:.4})  Incandescent chroma=({x_inc:.4}, {y_inc:.4})  dx={:+.4}",
        x_inc - x_d65
    );

    assert!(
        x_inc > x_d65 + 0.003,
        "Alexandrite's CIE x-chromaticity under Incandescent (3200K) ({x_inc:.4}) must be \
         measurably higher (redder) than under D65 Daylight ({x_d65:.4}) -- the defining \
         daylight-green / incandescent-red colour change must survive the trichroic \
         absorption data"
    );
}

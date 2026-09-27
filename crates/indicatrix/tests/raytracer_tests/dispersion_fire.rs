//! Image-level dispersion "fire" test: chromaticity variance across a rendered
//! pixel grid must be measurably higher for a dispersive material than for an
//! otherwise-identical flat-index one.

use glam::Vec3;
use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, hash_u32, trace_spectral_ray},
    },
};

/// Renders a `grid` x `grid` pixel image of `material` through `planes`, averaging
/// `samples_per_pixel` independent spectral samples per pixel (each with its own
/// hashed seed, so per-pixel noise is genuinely averaged down rather than repeating
/// the same random branch decisions), and returns the CIE xy chromaticity of every
/// pixel whose averaged luminance clears a minimum brightness (skipping background /
/// near-black pixels, which contribute illuminant-only chromaticity unrelated to the
/// gem itself and would otherwise dilute the measurement).
fn render_pixel_grid_chromaticities(
    camera: &Camera,
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    grid: usize,
    samples_per_pixel: u32,
    seed_salt: u32,
) -> Vec<(f32, f32)> {
    let dim = grid as f32;
    let mut chromaticities = Vec::new();

    for iy in 0..grid {
        for ix in 0..grid {
            let ray = camera.generate_ray(ix as f32, iy as f32, dim, dim, 0.5, 0.5);
            let mut xyz_sum = Vec3::ZERO;
            for s in 0..samples_per_pixel {
                let pixel_id = (iy as u32) * (grid as u32) + (ix as u32);
                let seed = hash_u32(seed_salt ^ hash_u32(pixel_id ^ hash_u32(s ^ 0x51ED_270B)));
                xyz_sum += trace_spectral_ray(
                    ray,
                    planes,
                    material,
                    12,
                    LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
            }
            let xyz_avg = xyz_sum / samples_per_pixel as f32;
            let sum = xyz_avg.x + xyz_avg.y + xyz_avg.z;
            if sum > 1e-4 && xyz_avg.y > 0.015 {
                chromaticities.push((xyz_avg.x / sum, xyz_avg.y / sum));
            }
        }
    }
    chromaticities
}

fn variance(values: &[f32]) -> f32 {
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n
}

/// Dispersion "fire" must be demonstrable at IMAGE level, not on a single ray -- a lone
/// ray only traces ONE geometric path, so any single-ray "chromatic spread" measurement
/// is dominated by illuminant colour temperature and CMF lobe shape rather than by
/// dispersion. Instead: render a small grid of pixels, average many samples per pixel
/// down to a low-noise chromaticity, and compare the VARIANCE of that chromaticity
/// ACROSS PIXELS between a high-dispersion material (Cubic Zirconia) and an
/// otherwise-equivalent FLAT material built via `GemMaterial::new_custom(..,
/// dispersion_delta = 0.0, ..)` (Cauchy b=c=0, a genuinely flat index across the whole
/// visible spectrum) at the SAME mean refractive index, so brightness/Fresnel-magnitude
/// differences between the two materials are controlled for.
///
/// A physically flat (non-dispersive) gem should render with essentially UNIFORM hue
/// across the image (facet-to-facet brightness varies, but not colour): with an
/// identical index at every wavelength, every channel's Fresnel reflect/transmit
/// probabilities and refracted directions agree at every bounce
/// (`spectral_mis_weight` collapsing to exactly 1.0, pinned down separately by
/// `spectral_mis_weight_is_exactly_unity_when_all_channels_agree`), so the render's
/// relative spectral shape reduces to a fixed hue at every pixel. The dispersive
/// material has no such guarantee: `spectral_mis_weight` concentrates each sample's
/// contribution onto its own hero wavelength once a dispersive refraction diverges
/// from the shared path, widening the chromaticity spread across the image.
#[test]
fn test_image_level_chromatic_spread_reveals_dispersion_fire_vs_flat_material() {
    const GRID: usize = 20;
    const SAMPLES_PER_PIXEL: u32 = 64;

    let planes = StandardGemCuts::standard_round_brilliant();
    let dispersive = GemMaterial::by_name("Cubic Zirconia").unwrap();
    let nd_cz = dispersive.dispersion.evaluate(589.3);
    // Same mean index as Cubic Zirconia, zero birefringence (isotropic, matching CZ's
    // own Cubic crystal system) and zero absorption (matching CZ's own
    // AbsorptionTensor::isotropic([0,0,0])) -- dispersion_delta is the ONLY
    // physically-meaningful difference from `dispersive`.
    let flat = GemMaterial::new_custom("Flat CZ-index reference", nd_cz, 0.0, 0.0, [0.0, 0.0, 0.0]);

    // A moderately oblique view so pavilion facets and internal TIR bounces are
    // actually in frame (a straight-down face-up view sees mostly the flat table
    // facet, with far fewer dispersive refraction events per ray).
    let camera = Camera::new(0.35, 0.55, 3.1, 42.0);

    let chroma_dispersive = render_pixel_grid_chromaticities(
        &camera,
        &planes,
        &dispersive,
        GRID,
        SAMPLES_PER_PIXEL,
        0x1111_1111,
    );
    let chroma_flat = render_pixel_grid_chromaticities(
        &camera,
        &planes,
        &flat,
        GRID,
        SAMPLES_PER_PIXEL,
        0x2222_2222,
    );

    assert!(
        chroma_dispersive.len() > 20 && chroma_flat.len() > 20,
        "expected a reasonable number of gem-covered pixels in the {}x{} grid (dispersive={}, flat={})",
        GRID,
        GRID,
        chroma_dispersive.len(),
        chroma_flat.len()
    );

    let x_dispersive: Vec<f32> = chroma_dispersive.iter().map(|&(x, _)| x).collect();
    let y_dispersive: Vec<f32> = chroma_dispersive.iter().map(|&(_, y)| y).collect();
    let x_flat: Vec<f32> = chroma_flat.iter().map(|&(x, _)| x).collect();
    let y_flat: Vec<f32> = chroma_flat.iter().map(|&(_, y)| y).collect();

    let var_x_dispersive = variance(&x_dispersive);
    let var_y_dispersive = variance(&y_dispersive);
    let var_x_flat = variance(&x_flat);
    let var_y_flat = variance(&y_flat);

    println!(
        "[fire demonstration] pixels: dispersive={} flat={} | chromaticity variance: dispersive (x={:.3e}, y={:.3e}) flat (x={:.3e}, y={:.3e}) | ratio (x={:.2}x, y={:.2}x)",
        chroma_dispersive.len(),
        chroma_flat.len(),
        var_x_dispersive,
        var_y_dispersive,
        var_x_flat,
        var_y_flat,
        var_x_dispersive / var_x_flat.max(1e-12),
        var_y_dispersive / var_y_flat.max(1e-12)
    );

    assert!(
        var_x_dispersive > var_x_flat * 2.0,
        "Cubic Zirconia's across-image x-chromaticity variance ({var_x_dispersive:.3e}) should clearly exceed the flat reference material's ({var_x_flat:.3e}) -- dispersion should visibly widen the spread of hues across the rendered image"
    );
    assert!(
        var_y_dispersive > var_y_flat * 2.0,
        "Cubic Zirconia's across-image y-chromaticity variance ({var_y_dispersive:.3e}) should clearly exceed the flat reference material's ({var_y_flat:.3e}) -- dispersion should visibly widen the spread of hues across the rendered image"
    );

    // Requirement 2's image-level counterpart: the flat material's own spread should
    // be genuinely small in absolute terms (a tight cluster of near-identical hues),
    // not merely "smaller than the dispersive material's" -- CIE xy chromaticity
    // spans roughly a unit square, so a variance below 1e-4 corresponds to a standard
    // deviation under ~1%, i.e. an essentially uniform hue across the image.
    assert!(
        var_x_flat < 1e-4 && var_y_flat < 1e-4,
        "flat (non-dispersive) material should render with essentially uniform hue across the image (var_x={var_x_flat:.3e}, var_y={var_y_flat:.3e})"
    );
}

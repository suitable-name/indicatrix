//! The display and export pipelines against the desktop's own calls.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{Camera, intersect_polyhedron, xyz_to_srgb_gamma},
    renderer::tonemap::tonemap_to_rgba_with_threads,
};

/// A deterministic, uneven float sum with out-of-gamut and near-zero values.
fn fixed_sum(len: usize) -> Vec<Vec3> {
    (0..len)
        .map(|i| {
            Vec3::new(
                ((i * 37) % 101) as f32 * 0.31,
                ((i * 53) % 97) as f32 * 0.22,
                ((i * 71) % 89) as f32 * 0.275,
            )
        })
        .collect()
}

#[test]
fn the_live_picture_is_the_mean_through_the_srgb_transfer() {
    let sum = fixed_sum(31 * 17);
    // One opaque RGBA pixel per sum entry; black stays black, whatever the count.
    assert_eq!(live_rgba(&sum, 4).len(), sum.len() * 4);
    assert_eq!(live_rgba(&[Vec3::ZERO], 9), vec![0, 0, 0, 255]);
    // The picture depends on the mean only: doubling the sum and the count (both exact
    // power-of-two scalings) changes nothing.
    let doubled: Vec<Vec3> = sum.iter().map(|v| *v * 2.0).collect();
    assert_eq!(live_rgba(&sum, 4), live_rgba(&doubled, 8));
    // A sum of zero samples is read as one sample, not divided by zero.
    assert_eq!(live_rgba(&sum, 0), live_rgba(&sum, 1));
    // The first pixel is the sRGB transfer of its mean.
    let rgba = live_rgba(&sum, 4);
    assert_eq!(&rgba[..4], &xyz_to_srgb_gamma(sum[0] * 0.25));
    // Every pixel is opaque.
    assert!(rgba.iter().skip(3).step_by(4).all(|&alpha| alpha == 255));
}

/// The PNG bytes of a web export equal the desktop export's `tonemap_accumulation` +
/// `encode_png_with_icc` for the same float sum, in both offered colour spaces.
#[test]
fn web_export_bytes_equal_the_desktop_export_for_a_fixed_sum() {
    let (width, height, samples) = (23_u32, 11_u32, 37_u32);
    let sum = fixed_sum((width * height) as usize);
    for color_space in EXPORT_COLOR_SPACES {
        let desktop = encode_png_with_icc(
            &tonemap_accumulation(width, height, samples, &sum, color_space),
            width,
            height,
            color_space,
        )
        .expect("desktop encode");
        let web = export_png(width, height, samples, &sum, color_space, None).expect("web encode");
        assert_eq!(web, desktop, "{color_space:?}");
        // A real PNG of the asked-for size: the signature, then the IHDR chunk (length,
        // type, width, height).
        assert_eq!(&web[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&web[12..16], b"IHDR");
        assert_eq!(&web[16..20], &width.to_be_bytes());
        assert_eq!(&web[20..24], &height.to_be_bytes());
        let has_icc = web.windows(4).any(|w| w == b"iCCP");
        assert_eq!(
            has_icc,
            color_space != ColorSpace::Srgb,
            "{color_space:?} ICC chunk"
        );
    }
    assert!(export_png(width, height, samples, &sum[1..], ColorSpace::Srgb, None).is_err());
}

/// `renderer::frame_denoise`'s own pin fixture: a 24x16 round brilliant at a fixed
/// pose and a noisy 3-sample sum low enough that the denoiser really filters.
fn fixture_frame_parts() -> (u32, u32, Camera, Vec<GpuFacetPlane>, Vec<Vec3>) {
    let (width, height) = (24, 16);
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let sum = (0..(width * height) as usize)
        .map(|i| {
            Vec3::new(
                ((i * 37) % 101) as f32 * 0.03,
                ((i * 53) % 97) as f32 * 0.02,
                ((i * 71) % 89) as f32 * 0.025,
            )
        })
        .collect();
    (width, height, camera, planes, sum)
}

/// The denoised sRGB export equals the settled live view (both the desktop's
/// `denoise_and_tonemap_frame` filter), and the live denoise is exactly that call.
#[test]
fn a_denoised_srgb_export_matches_the_settled_live_view() {
    let (width, height, camera, planes, sum) = fixture_frame_parts();
    let frame = DenoiseFrame {
        guide_key: 1,
        width,
        height,
        camera: &camera,
        planes: &planes,
        sample_count: 3,
        sum: &sum,
    };
    let mut denoiser = Denoiser::new();
    let live = denoiser.denoised_rgba(&frame);
    assert!(denoiser.has_guides_for(1));

    // The desktop's call, with its own fresh scratch.
    let guides = generate_guide_buffers(width, height, &camera, &planes);
    let mut desktop_denoiser = AtrousDenoiser::new();
    let (mut avg, mut filtered) = (Vec::new(), Vec::new());
    let desktop = denoise_and_tonemap_frame(
        FirstHitSnapshot {
            width,
            height,
            current_sample_count: 3,
            accum_buffer: &sum,
            first_hit_depth: &guides.depth,
            first_hit_normal: &guides.normal,
            first_hit_facet_id: &guides.facet_id,
        },
        &mut DenoiseScratch {
            denoiser: &mut desktop_denoiser,
            avg_color_buf: &mut avg,
            filtered_buf: &mut filtered,
        },
    );
    assert_eq!(live, desktop, "live denoise");
    assert_ne!(live, live_rgba(&sum, 3), "the fixture really is filtered");

    let mean = denoiser.denoised_mean(&frame);
    let png = export_png(width, height, 3, &sum, ColorSpace::Srgb, Some(&mean)).expect("encodes");
    let expected = encode_png_with_icc(&live, width, height, ColorSpace::Srgb).expect("encodes");
    assert_eq!(png, expected, "denoised sRGB export vs settled live view");
}

#[test]
fn export_names_follow_the_desktop_default_template() {
    assert_eq!(
        export_file_name("Diamond", 1600, 1200, 256, 1_790_000_000),
        "gem_export_Diamond_1600x1200_256spp_1790000000.png"
    );
    assert_eq!(
        export_file_name("Garnet: 1.74/rh", 8, 6, 16, 5),
        "gem_export_Garnet_ 1.74_rh_8x6_16spp_5.png"
    );
}

#[test]
fn export_sizes_keep_the_view_aspect_and_the_cap() {
    assert_eq!(export_size(800, 600, 1600), (1600, 1200));
    assert_eq!(export_size(600, 800, 1600), (1200, 1600));
    assert_eq!(export_size(960, 540, 9000), (4096, 2304));
    assert_eq!(export_size(0, 0, 0), (1, 1));
    assert_eq!(export_color_space(1), ColorSpace::DisplayP3);
    assert_eq!(export_color_space(7), ColorSpace::Srgb);
}

#[test]
fn the_pill_rate_leaves_out_worker_start_up() {
    // 8 samples after 20 s of start-up, then 8 more in the next second: 8 spp/s, not 0.8.
    let started = 1_000.0;
    let anchor = Some((21_000.0, 8));
    let rate = samples_per_second(16, started, anchor, 22_000.0);
    assert!((rate - 8.0).abs() < 1e-9, "rate {rate}");
    // With no second report yet it falls back to samples over the whole time -- and a
    // slow rate still prints with a decimal, never as "0".
    let first = samples_per_second(8, started, Some((21_000.0, 8)), 21_000.0);
    assert!((first - 0.4).abs() < 1e-9, "rate {first}");
    assert_eq!(format_rate(first), "0.4");
    assert_eq!(format_rate(37.6), "38");
    assert_eq!(format_rate(9.94), "9.9");
    // No anchor and no elapsed time cannot divide by zero.
    assert!(samples_per_second(4, 5.0, None, 5.0).is_finite());
}

/// Timing probe for the settle-denoise decision (single thread, like the browser).
/// Run natively with an optimised build:
/// `cargo test -p indicatrix-web-core --profile indicatrix-web-compute denoise_timing -- --ignored --nocapture`.
#[test]
#[ignore = "timing probe, not a check"]
fn denoise_timing_probe() {
    let camera = Camera::new(0.60, 0.45, 2.4, 42.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    for (width, height) in [(480_u32, 360_u32), (800, 600), (960, 720), (1600, 1200)] {
        let pixels = (width * height) as usize;
        let sum = fixed_sum(pixels);

        let start = std::time::Instant::now();
        let mut depth = Vec::with_capacity(pixels);
        let mut normal = Vec::with_capacity(pixels);
        let mut facet_id = Vec::with_capacity(pixels);
        for y in 0..height {
            for x in 0..width {
                let ray =
                    camera.generate_ray(x as f32, y as f32, width as f32, height as f32, 0.0, 0.0);
                let hit = intersect_polyhedron(ray, &planes);
                depth.push(hit.map_or(1.0e6, |h| h.t));
                normal.push(hit.map_or(Vec3::ZERO, |h| h.normal));
                facet_id.push(hit.map_or(-1, |h| h.facet_idx as i32));
            }
        }
        let guide_ms = start.elapsed().as_secs_f64() * 1000.0;

        let start = std::time::Instant::now();
        let avg: Vec<Vec3> = sum.iter().map(|v| *v * (1.0 / 64.0)).collect();
        let mut filtered = Vec::new();
        AtrousDenoiser::new().denoise_into_with_threads(
            &GBuffers {
                color: &avg,
                depth: &depth,
                normal: &normal,
                facet_id: &facet_id,
                width: width as usize,
                height: height as usize,
                spp: 64,
            },
            &AtrousParams::default(),
            &mut filtered,
            1,
        );
        let denoise_ms = start.elapsed().as_secs_f64() * 1000.0;

        let start = std::time::Instant::now();
        let rgba = tonemap_to_rgba_with_threads(&filtered, 1.0, 1);
        let tonemap_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(rgba.len(), pixels * 4);
        println!(
            "{width}x{height}: guide {guide_ms:.1} ms, denoise {denoise_ms:.1} ms, \
             tonemap {tonemap_ms:.1} ms (one thread, native)"
        );
    }
}

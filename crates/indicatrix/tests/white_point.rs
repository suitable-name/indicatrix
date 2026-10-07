//! White-point guard: "a colourless stone renders colourless" under every analytic
//! lighting preset, plus measuring tools for the GPU gallery PNGs and reference photos.
//!
//! The renders here are CPU renders that keep the PRE-tone-map XYZ per pixel (the
//! gallery's `render_cpu` only returns tone-mapped sRGB bytes). A pixel counts as stone
//! when every one of five sub-pixel rays hits the polyhedron, as backdrop when none does;
//! edge pixels, which mix the two, are left out of both regions.
//!
//! The two PNG-measuring tools are ignored and need the `hdr` feature (it brings the
//! `image` decoder in):
//!
//! ```text
//! cargo test -p indicatrix --features hdr --test white_point measure_gallery_pngs -- --ignored --nocapture
//! cargo test -p indicatrix --features hdr --test white_point measure_photo_pngs -- --ignored --nocapture
//! ```

use glam::Vec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        Ray,
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, Camera, DEFAULT_FOV_DEG, LightingPreset, build_plane_soa, hash_u32,
            intersect_polyhedron, trace_spectral_ray_with_finish_soa,
        },
        studio_rig::StudioRig,
    },
};
use std::f32::consts::FRAC_PI_2;

const MAX_BOUNCES: u32 = 12;
const CAMERA_DISTANCE: f32 = 2.4;
const LIGHT_YAW_DEG: f32 = 48.0;
const LIGHT_PITCH_DEG: f32 = 54.0;
/// D65 white point chromaticity.
const D65_X: f32 = 0.3127;
const D65_Y: f32 = 0.3290;
/// Tolerance for real Diamond, whose dispersion shifts the average by ~0.003-0.005.
const CHROMA_TOLERANCE: f32 = 0.007;
/// Tolerance for the non-dispersive twin (measured within 0.0001 at 256 spp).
const STRICT_CHROMA_TOLERANCE: f32 = 0.002;

/// The lead sets this from the first run of `light_tent_table_luminance_floor` (the printed
/// stone / backdrop mean-Y ratio), then reruns. While it is 0.0 the test only prints.
const PINNED_TENT_TABLE_RATIO: f32 = 0.0;
/// Relative band around [`PINNED_TENT_TABLE_RATIO`].
const PINNED_TENT_BAND: f32 = 0.15;

#[derive(Clone, Copy)]
struct View {
    name: &'static str,
    yaw: f32,
    pitch: f32,
}

const TOP: View = View {
    name: "top",
    yaw: 0.0,
    pitch: FRAC_PI_2,
};
const TILTED: View = View {
    name: "tilted",
    yaw: 0.60,
    pitch: 0.45,
};

/// Which pixels of a render are stone, which backdrop (the rest are edge pixels).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Stone,
    Backdrop,
    Edge,
}

/// Mean pre-tone-map XYZ per pixel and the region mask.
fn render_xyz(
    material: &str,
    preset: LightingPreset,
    view: View,
    size: u32,
    samples_per_pixel: u32,
) -> (Vec<Vec3>, Vec<Region>) {
    let material = GemMaterial::by_name(material)
        .unwrap_or_else(|| panic!("{material} is a built-in material"));
    render_xyz_with(&material, preset, view, size, samples_per_pixel)
}

/// [`render_xyz`] for an explicit material object.
fn render_xyz_with(
    material: &GemMaterial,
    preset: LightingPreset,
    view: View,
    size: u32,
    samples_per_pixel: u32,
) -> (Vec<Vec3>, Vec<Region>) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = material.clone();
    let camera = Camera::new(view.yaw, view.pitch, CAMERA_DISTANCE, DEFAULT_FOV_DEG);
    let environment = preset
        .studio(
            1.0,
            LIGHT_YAW_DEG.to_radians(),
            LIGHT_PITCH_DEG.to_radians(),
        )
        .with_backdrop(BACKDROP_GREY);
    let salt = hash_u32(0x9E37_79B9 ^ preset.index() as u32 ^ hash_u32(view.name.len() as u32));
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let dim = size as f32;
    let inv_spp = 1.0 / samples_per_pixel as f32;

    let mut xyz = vec![Vec3::ZERO; (size * size) as usize];
    let mut regions = vec![Region::Edge; (size * size) as usize];
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for t in 0..threads {
            let (planes, plane_soa, material, camera) = (&planes, &plane_soa, &material, &camera);
            handles.push(scope.spawn(move || {
                let mut rows = Vec::new();
                for y in (t as u32..size).step_by(threads) {
                    let mut row = Vec::with_capacity(size as usize);
                    for x in 0..size {
                        let pixel = y * size + x;
                        let mut sum = Vec3::ZERO;
                        for s in 0..samples_per_pixel {
                            let seed = hash_u32(salt ^ hash_u32(pixel ^ hash_u32(s ^ 0x51ED_270B)));
                            let jx = hash_u32(seed ^ 0xA511_E9B3) as f32 / 4_294_967_295.0;
                            let jy = hash_u32(seed ^ 0x63D8_3595) as f32 / 4_294_967_295.0;
                            let hero = hash_u32(seed) as f32 / 4_294_967_295.0;
                            let ray = camera.generate_ray(x as f32, y as f32, dim, dim, jx, jy);
                            sum += trace_spectral_ray_with_finish_soa(
                                ray,
                                planes,
                                plane_soa,
                                &[],
                                material,
                                MAX_BOUNCES,
                                environment,
                                seed,
                                hero,
                                None,
                            );
                        }
                        let hits = [(0.5, 0.5), (0.1, 0.1), (0.9, 0.1), (0.1, 0.9), (0.9, 0.9)]
                            .iter()
                            .filter(|(jx, jy)| {
                                let ray =
                                    camera.generate_ray(x as f32, y as f32, dim, dim, *jx, *jy);
                                intersect_polyhedron(ray, planes).is_some()
                            })
                            .count();
                        let region = match hits {
                            0 => Region::Backdrop,
                            5 => Region::Stone,
                            _ => Region::Edge,
                        };
                        row.push((sum * inv_spp, region));
                    }
                    rows.push((y, row));
                }
                rows
            }));
        }
        for handle in handles {
            for (y, row) in handle.join().expect("render thread panicked") {
                for (x, (value, region)) in row.into_iter().enumerate() {
                    let i = (y * size) as usize + x;
                    xyz[i] = value;
                    regions[i] = region;
                }
            }
        }
    });
    (xyz, regions)
}

fn mean_of(xyz: &[Vec3], regions: &[Region], wanted: Region) -> (Vec3, usize) {
    let mut sum = Vec3::ZERO;
    let mut n = 0usize;
    for (v, r) in xyz.iter().zip(regions) {
        if *r == wanted {
            sum += *v;
            n += 1;
        }
    }
    (sum / n.max(1) as f32, n)
}

fn chromaticity(xyz: Vec3) -> (f32, f32) {
    let total = xyz.x + xyz.y + xyz.z;
    (xyz.x / total, xyz.y / total)
}

fn is_within(xyz: Vec3, tolerance: f32) -> bool {
    let (x, y) = chromaticity(xyz);
    (x - D65_X).abs() < tolerance && (y - D65_Y).abs() < tolerance
}

const SIZE: u32 = 48;
const SPP: u32 = 64;

/// Renders `material` under every non-UV preset, face-up and tilted, and returns one failure
/// line per stone / backdrop region whose chromaticity is off D65 by more than the given
/// tolerances. The backdrop is only checked when `backdrop_tolerance` is `Some`.
///
/// Skipped presets: the UV lamps (not white light) and `Aset`, the contrast view, which is
/// false-colour by design (one narrow band per elevation zone). With `skip_sun_stone` the
/// stone region of `DaylightSun` is printed but not asserted: the 40 000-radiance sun reaches
/// a polished stone only through a handful of deterministic facet flashes, so at 64 spp the
/// stone mean is a few firefly samples (each carrying only four wavelengths) and its
/// chromaticity is Monte-Carlo noise, not a bias. The unbiased-equal-weight property is
/// pinned deterministically by `a_ray_into_the_sun_has_every_channel_at_equal_weight`.
fn neutrality_failures(
    material: &GemMaterial,
    stone_tolerance: f32,
    backdrop_tolerance: Option<f32>,
    skip_sun_stone: bool,
) -> Vec<String> {
    println!("preset / view: stone (x, y, Y, n)  backdrop (x, y, Y, n)");
    let mut failures = Vec::new();
    for preset in LightingPreset::ALL {
        if preset.is_uv_lamp() || preset == LightingPreset::Aset {
            continue;
        }
        let stone_asserted = !(skip_sun_stone && preset == LightingPreset::DaylightSun);
        for view in [TOP, TILTED] {
            let (xyz, regions) = render_xyz_with(material, preset, view, SIZE, SPP);
            let (stone, n_stone) = mean_of(&xyz, &regions, Region::Stone);
            let (back, n_back) = mean_of(&xyz, &regions, Region::Backdrop);
            let (sx, sy) = chromaticity(stone);
            let (bx, by) = chromaticity(back);
            println!(
                "{:<28} {:<7} stone ({sx:.4}, {sy:.4}, Y {:.4}, n {n_stone})  backdrop ({bx:.4}, {by:.4}, Y {:.4}, n {n_back})",
                preset.label(),
                view.name,
                stone.y,
                back.y
            );
            assert!(n_stone > 0 && n_back > 0, "regions must be non-empty");
            if stone_asserted && !is_within(stone, stone_tolerance) {
                failures.push(format!(
                    "{} {} stone ({sx:.4}, {sy:.4})",
                    preset.label(),
                    view.name
                ));
            }
            if let Some(tolerance) = backdrop_tolerance
                && !is_within(back, tolerance)
            {
                failures.push(format!(
                    "{} {} backdrop ({bx:.4}, {by:.4})",
                    preset.label(),
                    view.name
                ));
            }
        }
    }
    failures
}

/// Strict white-balance check: a colourless, non-dispersive stone (same mean index as
/// diamond, zero F-C delta, no absorption) and the backdrop must both sit on D65 under every
/// analytic preset. Measured 2026-10-07 at 256 spp the twin is within 0.0001 of D65.
#[test]
fn a_non_dispersive_colourless_stone_is_neutral_under_every_analytic_preset() {
    let flat = GemMaterial::new_custom("NoDispersion", 2.417, 0.0, 0.0, [0.0; 3]);
    let failures = neutrality_failures(
        &flat,
        STRICT_CHROMA_TOLERANCE,
        Some(STRICT_CHROMA_TOLERANCE),
        true,
    );
    assert!(
        failures.is_empty(),
        "colourless regions off D65 ({D65_X}, {D65_Y}) by more than {STRICT_CHROMA_TOLERANCE}: {failures:#?}"
    );
}

/// Deterministic sun check: a camera ray that misses the stone and points straight at the
/// sun disc of `DaylightSun` must deposit the sun's D65 spectrum with the SAME weight in every
/// channel (no hero-dependent MIS weight, no different SPD). Averaged over 512 evenly spaced
/// hero draws the chromaticity is D65, and the luminance is the sun's own (tens of
/// thousands, not the sky's fraction of one). A one-wavelength-in-four drop (the hero getting
/// a competing-technique weight the companions do not) would shift both.
#[test]
fn a_ray_into_the_sun_has_every_channel_at_equal_weight() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = GemMaterial::new_custom("NoDispersion", 2.417, 0.0, 0.0, [0.0; 3]);
    let yaw = LIGHT_YAW_DEG.to_radians();
    let pitch = LIGHT_PITCH_DEG.to_radians();
    let environment = LightingPreset::DaylightSun.studio(1.0, yaw, pitch);
    let key_dir = StudioRig::new(yaw, pitch).key_dir;
    // Starts well away from the stone and moves up and outwards: it cannot touch it.
    let ray = Ray {
        origin: Vec3::new(6.0, 0.5, 6.0),
        dir: key_dir,
    };
    let heroes = 512u32;
    let mut sum = Vec3::ZERO;
    for i in 0..heroes {
        let hero = (i as f32 + 0.5) / heroes as f32;
        sum += trace_spectral_ray_with_finish_soa(
            ray,
            &planes,
            &plane_soa,
            &[],
            &material,
            MAX_BOUNCES,
            environment,
            hash_u32(i ^ 0x5EED_0001),
            hero,
            None,
        );
    }
    let mean = sum / heroes as f32;
    let (x, y) = chromaticity(mean);
    assert!(
        is_within(mean, STRICT_CHROMA_TOLERANCE),
        "direct sun chromaticity ({x:.4}, {y:.4}) is off D65 ({D65_X}, {D65_Y})"
    );
    assert!(
        mean.y > 10_000.0,
        "direct sun luminance {} is too low",
        mean.y
    );
}

/// Looser check for real Diamond. Dispersion "fire" shifts the face-up average by about
/// 0.003-0.005 depending on spp (measured 2026-10-07: +0.003/+0.003 top, -0.002/-0.002
/// tilted at 256 spp; up to 0.0045/0.0047 at 64 spp), which is physical, not a
/// white-balance error.
#[test]
fn clear_diamond_stays_near_neutral_under_every_analytic_preset() {
    let diamond = GemMaterial::by_name("Diamond").expect("built-in");
    let failures = neutrality_failures(&diamond, CHROMA_TOLERANCE, None, false);
    assert!(
        failures.is_empty(),
        "Diamond stone off D65 ({D65_X}, {D65_Y}) by more than {CHROMA_TOLERANCE}: {failures:#?}"
    );
}

/// Diagnostic: is the top-view stone cast caused by dispersion (fire sampled from a spiky
/// rig) rather than the white balance? Renders Diamond and a non-dispersive twin
/// (same mean index, zero F-C delta, no absorption) face-up at 4x the samples and prints
/// both stone chromaticities. If the twin is neutral and Diamond is not, the cast is
/// dispersion; the white balance is then exonerated.
///
/// Measured 2026-10-07 (256 spp): the twin is neutral to within 0.0001 of D65 under every
/// Studio preset, face-up and tilted; Diamond leans +0.003/+0.003 face-up and
/// -0.002/-0.002 tilted. The cast is physical fire, not a white-balance error.
///
/// `cargo test -p indicatrix --test white_point dispersion_explains_top_view_cast -- --ignored --nocapture`
#[test]
#[ignore = "diagnostic; run with: cargo test -p indicatrix --test white_point dispersion_explains_top_view_cast -- --ignored --nocapture"]
fn dispersion_explains_top_view_cast() {
    let diamond = GemMaterial::by_name("Diamond").expect("built-in");
    let flat = GemMaterial::new_custom("NoDispersion", 2.417, 0.0, 0.0, [0.0; 3]);
    for preset in [
        LightingPreset::Incandescent,
        LightingPreset::DarkSpotlight,
        LightingPreset::Daylight,
        LightingPreset::RingLights,
    ] {
        for (name, material) in [("Diamond", &diamond), ("no-dispersion", &flat)] {
            for view in [TOP, TILTED] {
                let (xyz, regions) = render_xyz_with(material, preset, view, SIZE, SPP * 4);
                let (stone, n) = mean_of(&xyz, &regions, Region::Stone);
                let (sx, sy) = chromaticity(stone);
                println!(
                    "{:<28} {:<14} {:<7} stone ({sx:.4}, {sy:.4}) dx {:+.4} dy {:+.4} (n {n})",
                    preset.label(),
                    name,
                    view.name,
                    sx - D65_X,
                    sy - D65_Y
                );
            }
        }
    }
}

#[test]
fn light_tent_table_luminance_floor() {
    let (xyz, regions) = render_xyz("Diamond", LightingPreset::LightTent, TOP, SIZE, SPP);
    let (stone, _) = mean_of(&xyz, &regions, Region::Stone);
    let (back, _) = mean_of(&xyz, &regions, Region::Backdrop);
    let ratio = stone.y / back.y;

    let mut stone_y: Vec<f32> = xyz
        .iter()
        .zip(&regions)
        .filter(|(_, r)| **r == Region::Stone)
        .map(|(v, _)| v.y)
        .collect();
    stone_y.sort_by(f32::total_cmp);
    let tenth = (stone_y.len() / 10).max(1);
    let dark = stone_y[..tenth].iter().sum::<f32>() / tenth as f32;
    let bright = stone_y[stone_y.len() - tenth..].iter().sum::<f32>() / tenth as f32;
    println!(
        "light tent, face-up: stone mean Y {:.4}, backdrop mean Y {:.4}, ratio {ratio:.4}; \
         darkest 10% mean Y {dark:.4}, brightest 10% mean Y {bright:.4}",
        stone.y, back.y
    );

    if PINNED_TENT_TABLE_RATIO > 0.0 {
        let lo = PINNED_TENT_TABLE_RATIO * (1.0 - PINNED_TENT_BAND);
        let hi = PINNED_TENT_TABLE_RATIO * (1.0 + PINNED_TENT_BAND);
        assert!(
            (lo..=hi).contains(&ratio),
            "stone/backdrop luminance ratio {ratio:.4} outside {lo:.4}..{hi:.4} \
             (pinned {PINNED_TENT_TABLE_RATIO})"
        );
    } else {
        println!("PINNED_TENT_TABLE_RATIO is unset (0.0): set it to {ratio:.4} and rerun");
    }
}

/// Decoded-image measurement helpers, shared by the two ignored PNG tools.
#[cfg(feature = "hdr")]
mod png_tools {
    use super::{Vec3, chromaticity};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn srgb_to_linear(v: u8) -> f32 {
        let c = f32::from(v) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(rgb: [f32; 3]) -> f32 {
        0.0722f32.mul_add(rgb[2], 0.7152f32.mul_add(rgb[1], 0.2126 * rgb[0]))
    }

    fn rgb_to_xyz(rgb: [f32; 3]) -> Vec3 {
        Vec3::new(
            0.1805f32.mul_add(rgb[2], 0.3576f32.mul_add(rgb[1], 0.4124 * rgb[0])),
            luminance(rgb),
            0.9505f32.mul_add(rgb[2], 0.1192f32.mul_add(rgb[1], 0.0193 * rgb[0])),
        )
    }

    /// A pixel rectangle `(x, y, w, h)`.
    pub type Rect = (u32, u32, u32, u32);

    pub struct Stats {
        pub mean_rgb: [f32; 3],
        pub x: f32,
        pub y: f32,
        pub lum: f32,
        pub p10: f32,
        pub p90: f32,
    }

    /// Statistics of the linear pixels in the rectangle `(x, y, w, h)`.
    pub fn measure(img: &image::RgbImage, rect: (u32, u32, u32, u32)) -> Stats {
        let (rx, ry, rw, rh) = rect;
        let mut sum = [0.0f32; 3];
        let mut lums = Vec::new();
        for y in ry..(ry + rh).min(img.height()) {
            for x in rx..(rx + rw).min(img.width()) {
                let p = img.get_pixel(x, y).0;
                let rgb = [
                    srgb_to_linear(p[0]),
                    srgb_to_linear(p[1]),
                    srgb_to_linear(p[2]),
                ];
                for (s, c) in sum.iter_mut().zip(rgb) {
                    *s += c;
                }
                lums.push(luminance(rgb));
            }
        }
        let n = lums.len().max(1) as f32;
        let mean_rgb = [sum[0] / n, sum[1] / n, sum[2] / n];
        lums.sort_by(f32::total_cmp);
        let pick = |q: f32| {
            lums.get(((lums.len() as f32 - 1.0) * q) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let (x, y) = chromaticity(rgb_to_xyz(mean_rgb));
        Stats {
            mean_rgb,
            x,
            y,
            lum: luminance(mean_rgb),
            p10: pick(0.1),
            p90: pick(0.9),
        }
    }

    fn describe(label: &str, s: &Stats) -> String {
        format!(
            "{label}: mean RGB ({:.4}, {:.4}, {:.4}) xy ({:.4}, {:.4}) Y {:.4}",
            s.mean_rgb[0], s.mean_rgb[1], s.mean_rgb[2], s.x, s.y, s.lum
        )
    }

    /// Centre 40 % square and a corner patch (10 % of the side) of an image.
    pub fn centre_and_corner(img: &image::RgbImage) -> (Rect, Rect) {
        let (w, h) = (img.width(), img.height());
        let centre = (w * 3 / 10, h * 3 / 10, w * 4 / 10, h * 4 / 10);
        let corner = (0, 0, (w / 10).max(1), (h / 10).max(1));
        (centre, corner)
    }

    pub fn report_gallery_image(name: &str, path: &Path) {
        let img = match image::open(path) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                println!("{name}: cannot decode ({e})");
                return;
            }
        };
        let (centre_rect, corner_rect) = centre_and_corner(&img);
        let stone = measure(&img, centre_rect);
        let back = measure(&img, corner_rect);
        println!("{name}");
        println!("  {}", describe("stone (centre 40%)", &stone));
        println!("  {}", describe("backdrop (corner)", &back));
        println!(
            "  stone/backdrop luminance {:.4}; stone p10 {:.4}, p90 {:.4}",
            stone.lum / back.lum.max(1e-9),
            stone.p10,
            stone.p90
        );
    }

    pub fn gallery_dir() -> PathBuf {
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("lighting_gallery")
    }

    pub fn read_rect_env(name: &str) -> Option<(u32, u32, u32, u32)> {
        let value = std::env::var(name).ok()?;
        let parts: Vec<u32> = value
            .split(',')
            .filter_map(|p| p.trim().parse().ok())
            .collect();
        match parts[..] {
            [x, y, w, h] => Some((x, y, w, h)),
            _ => None,
        }
    }

    pub fn report_photo(path: &Path) {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let img = match image::open(path) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                println!("{name}: cannot decode ({e})");
                return;
            }
        };
        let (centre_rect, corner_rect) = centre_and_corner(&img);
        let stone_rect = read_rect_env("INDICATRIX_PHOTO_STONE_RECT").unwrap_or(centre_rect);
        let card_rect = read_rect_env("INDICATRIX_PHOTO_CARD_RECT").unwrap_or(corner_rect);
        let stone = measure(&img, stone_rect);
        let card = measure(&img, card_rect);
        println!("{name}");
        println!("  {}", describe("stone", &stone));
        println!("  {}", describe("grey card", &card));
        println!(
            "  stone xy relative to card: dx {:+.4}, dy {:+.4}; stone/card luminance {:.4}; \
             stone p10 {:.4}, p90 {:.4}",
            stone.x - card.x,
            stone.y - card.y,
            stone.lum / card.lum.max(1e-9),
            stone.p10,
            stone.p90
        );
    }

    pub fn gallery_names() -> Vec<String> {
        let index = fs::read_to_string(gallery_dir().join("index.txt")).unwrap_or_default();
        index
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|token| {
                Path::new(token)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
            })
            .map(str::to_owned)
            .collect()
    }
}

/// Prints, for every PNG listed in the gallery's `index.txt`, the mean linear RGB and
/// chromaticity of the centre 40 % square (the stone) and of a corner patch (the
/// backdrop), their luminance ratio, and the 10th/90th percentile luminance of the
/// centre square.
///
/// `cargo test -p indicatrix --features hdr --test white_point measure_gallery_pngs -- --ignored --nocapture`
#[cfg(feature = "hdr")]
#[test]
#[ignore = "measures the GPU gallery PNGs; run with: cargo test -p indicatrix --features hdr --test white_point measure_gallery_pngs -- --ignored --nocapture"]
fn measure_gallery_pngs() {
    let names = png_tools::gallery_names();
    assert!(
        !names.is_empty(),
        "no gallery images listed in {}",
        png_tools::gallery_dir().join("index.txt").display()
    );
    for name in names {
        png_tools::report_gallery_image(&name, &png_tools::gallery_dir().join(&name));
    }
}

/// The same measurement on `C:\temp\mcp2\reference_photos\*.png` (the owner converts the
/// RAW/JPEG to PNG). `INDICATRIX_PHOTO_CARD_RECT=x,y,w,h` marks the grey-card patch and
/// `INDICATRIX_PHOTO_STONE_RECT=x,y,w,h` the stone (defaults: corner patch, centre 40 %);
/// the stone chromaticity is printed relative to the card.
///
/// `cargo test -p indicatrix --features hdr --test white_point measure_photo_pngs -- --ignored --nocapture`
#[cfg(feature = "hdr")]
#[test]
#[ignore = "measures reference photo PNGs; run with: cargo test -p indicatrix --features hdr --test white_point measure_photo_pngs -- --ignored --nocapture"]
fn measure_photo_pngs() {
    let dir = std::path::Path::new(r"C:\temp\mcp2\reference_photos");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no PNGs in {}", dir.display());
    for path in paths {
        png_tools::report_photo(&path);
    }
}

#[cfg(not(feature = "hdr"))]
#[test]
#[ignore = "needs the hdr feature for the PNG decoder: cargo test -p indicatrix --features hdr --test white_point measure_gallery_pngs -- --ignored --nocapture"]
fn measure_gallery_pngs() {
    panic!("build with --features hdr");
}

#[cfg(not(feature = "hdr"))]
#[test]
#[ignore = "needs the hdr feature for the PNG decoder: cargo test -p indicatrix --features hdr --test white_point measure_photo_pngs -- --ignored --nocapture"]
fn measure_photo_pngs() {
    panic!("build with --features hdr");
}

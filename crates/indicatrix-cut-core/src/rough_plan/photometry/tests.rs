//! Tests of the photometry module. Synthetic images only; no photo files.

use glam::{DVec2, DVec3};

use super::{
    CalibrationFrames, CaptureMeta, ConsistencyOptions, ConsistencyWarning, FrameRole, HdrInput,
    HdrOptions, InclusionMarker, LinearImage, MeshMaskOptions, NoiseModel, NoiseOptions, PixelMask,
    ResampleOptions, ResampleSource, SourceKind, TransmittanceOptions, ViewCalibration,
    WorkingGrid, check_view, demosaic_bilinear, estimate_noise, flag, from_rgb16, from_srgb8,
    icc_is_linear, linear_from_dynamic, linear_to_srgb, merge_hdr, mesh_masks, parse_jpeg_exif,
    resample, srgb_to_linear, stone_region,
};
use crate::rough_plan::locate::{
    Projection, RigProfile, Rigid, Scene, ViewPose, box_mesh, predict_ghosts,
};

/// Narrows a synthetic `f64` value to the `f32` the images store.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the image storage precision; the synthetic values are in the normal float range"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// A working-grid coordinate (non-negative, inside the image) as a pixel index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass non-negative working coordinates inside a small test grid"
)]
const fn to_index(value: f64) -> usize {
    value as usize
}

/// A linear-to-sRGB level in 0..=1 scaled by 255 and rounded, as an 8-bit code.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the encoded level is in 0..=1, so the rounded code is within 0..=255"
)]
const fn to_code(rounded: f32) -> u8 {
    rounded as u8
}

/// A tiny deterministic generator (a linear congruential generator, high bits).
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// A standard normal number (Box-Muller).
    fn gauss(&mut self) -> f64 {
        let first = self.unit().max(1e-12);
        let second = self.unit();
        (-2.0 * first.ln()).sqrt() * (std::f64::consts::TAU * second).cos()
    }
}

fn image_from(width: usize, height: usize, f: impl Fn(usize, usize, usize) -> f32) -> LinearImage {
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            pixels.push([f(x, y, 0), f(x, y, 1), f(x, y, 2)]);
        }
    }
    LinearImage::from_pixels(width, height, pixels, SourceKind::Raw).unwrap()
}

#[test]
fn srgb_round_trip_and_known_values() {
    for i in 0..=1000 {
        let encoded = i as f32 / 1000.0;
        let back = linear_to_srgb(srgb_to_linear(encoded));
        assert!((back - encoded).abs() < 1e-5, "{encoded}: {back}");
    }
    assert!((srgb_to_linear(0.5) - 0.214_04).abs() < 1e-4);
    assert_eq!(srgb_to_linear(0.0), 0.0);
    assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    let image = from_srgb8(2, 1, &[0, 128, 255, 255, 255, 255]).unwrap();
    assert_eq!(image.pixels[0][0], 0.0);
    assert!((image.pixels[0][1] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-6);
    assert!((image.pixels[1][2] - 1.0).abs() < 1e-6);
    assert!(image.non_linear_source);
    assert_eq!(image.source, SourceKind::Rgb8);
}

#[test]
fn sixteen_bit_linear_icc_and_flagging() {
    let samples = [32768_u16, 0, 65535];
    let linear = from_rgb16(1, 1, &samples, true).unwrap();
    assert!(!linear.non_linear_source);
    assert!((linear.pixels[0][0] - 32768.0 / 65535.0).abs() < 1e-6);
    let encoded = from_rgb16(1, 1, &samples, false).unwrap();
    assert!(encoded.non_linear_source);
    assert!(encoded.pixels[0][0] < linear.pixels[0][0]);

    // A profile with an identity `rTRC` is linear; a 2.2 gamma one is not.
    let profile = |count: u32, gamma: Option<u16>| {
        let mut bytes = vec![0_u8; 128];
        bytes.extend(1_u32.to_be_bytes());
        bytes.extend(*b"rTRC");
        bytes.extend(144_u32.to_be_bytes());
        bytes.extend(14_u32.to_be_bytes());
        bytes.extend(*b"curv");
        bytes.extend(0_u32.to_be_bytes());
        bytes.extend(count.to_be_bytes());
        if let Some(g) = gamma {
            bytes.extend(g.to_be_bytes());
        }
        bytes
    };
    assert!(icc_is_linear(&profile(0, None)));
    assert!(icc_is_linear(&profile(1, Some(256))));
    assert!(!icc_is_linear(&profile(1, Some(563))));

    // The same decision through a decoded picture.
    let buffer =
        image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(1, 1, samples.to_vec()).unwrap();
    let picture = image::DynamicImage::ImageRgb16(buffer);
    let with_icc =
        linear_from_dynamic(&picture, Some(&profile(0, None)), CaptureMeta::default()).unwrap();
    assert!(!with_icc.non_linear_source);
    assert_eq!(with_icc.source, SourceKind::Tiff16);
    let without = linear_from_dynamic(&picture, None, CaptureMeta::default()).unwrap();
    assert!(without.non_linear_source);
}

#[test]
fn bilinear_demosaic_recovers_a_flat_colour_everywhere() {
    let colour = [0.8_f32, 0.4, 0.2];
    let layout = [[0_usize, 1], [1, 2]]; // RGGB
    let (w, h) = (9, 7);
    let mut plane = Vec::new();
    for y in 0..h {
        for x in 0..w {
            plane.push(colour[layout[y & 1][x & 1]]);
        }
    }
    for pixel in demosaic_bilinear(&plane, w, h, &layout) {
        for c in 0..3 {
            assert!((pixel[c] - colour[c]).abs() < 1e-6);
        }
    }
}

#[test]
fn jpeg_exif_is_read() {
    let mut tiff = Vec::new();
    tiff.extend(*b"II");
    tiff.extend(42_u16.to_le_bytes());
    tiff.extend(8_u32.to_le_bytes());
    // IFD0 at 8: one entry (ExifIFD pointer to 26).
    tiff.extend(1_u16.to_le_bytes());
    for part in [
        0x8769_u16.to_le_bytes().to_vec(),
        4_u16.to_le_bytes().to_vec(),
        1_u32.to_le_bytes().to_vec(),
        26_u32.to_le_bytes().to_vec(),
    ] {
        tiff.extend(part);
    }
    tiff.extend(0_u32.to_le_bytes());
    assert_eq!(tiff.len(), 26);
    // Exif IFD at 26: exposure time (RATIONAL at 68), ISO 400, white balance 1.
    tiff.extend(3_u16.to_le_bytes());
    for (tag, kind, value) in [
        (0x829A_u16, 5_u16, 68_u32),
        (0x8827, 3, 400),
        (0xA403, 3, 1),
    ] {
        tiff.extend(tag.to_le_bytes());
        tiff.extend(kind.to_le_bytes());
        tiff.extend(1_u32.to_le_bytes());
        tiff.extend(value.to_le_bytes());
    }
    tiff.extend(0_u32.to_le_bytes());
    assert_eq!(tiff.len(), 68);
    tiff.extend(1_u32.to_le_bytes());
    tiff.extend(125_u32.to_le_bytes());

    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend(
        u16::try_from(tiff.len() + 8)
            .expect("the EXIF block is small")
            .to_be_bytes(),
    );
    jpeg.extend(*b"Exif\0\0");
    jpeg.extend(&tiff);
    jpeg.extend([0xFF, 0xD9]);
    let meta = parse_jpeg_exif(&jpeg);
    assert_eq!(meta.exposure_time_s, Some(0.008));
    assert_eq!(meta.iso, Some(400));
    assert_eq!(meta.white_balance_mode, Some(1));
    assert_eq!(parse_jpeg_exif(&[1, 2, 3]), CaptureMeta::default());
}

#[test]
fn dark_white_correction_recovers_a_known_transmittance() {
    let (w, h) = (32, 24);
    let truth =
        |x: usize, y: usize| (0.7 * (x as f32 / 31.0)).mul_add(0.5 + 0.5 * y as f32 / 23.0, 0.2);
    let backlight =
        |x: usize, y: usize| 0.1f32.mul_add(x as f32 / 31.0, 0.3f32.mul_add(y as f32 / 23.0, 0.4));
    let dark_level = 0.01_f32;
    let white = image_from(w, h, |x, y, _| backlight(x, y));
    let dark = image_from(w, h, |_, _, _| dark_level);
    let stone = image_from(w, h, |x, y, _| {
        f32::mul_add(truth(x, y), backlight(x, y) - dark_level, dark_level)
    });
    let calibration = ViewCalibration {
        frames: CalibrationFrames {
            white: vec![white],
            dark: vec![dark],
        },
        noise: NoiseModel {
            a: [1e-6; 3],
            b: [1e-5; 3],
        },
    };
    let t = calibration
        .transmittance(&stone, &TransmittanceOptions::default())
        .unwrap();
    for y in 0..h {
        for x in 0..w {
            let got = t.values[y * w + x];
            for c in 0..3 {
                assert!((got[c] - truth(x, y)).abs() < 1e-4, "({x},{y}) {got:?}");
            }
            assert!(t.variance[y * w + x][0].is_finite());
        }
    }
    assert_eq!(t.mask.count(flag::ALL), 0);

    // A pixel where the backlight does not rise above the dark level is masked.
    let mut dead = calibration.clone();
    dead.frames.white[0].pixels[5] = [dark_level; 3];
    let t = dead
        .transmittance(&stone, &TransmittanceOptions::default())
        .unwrap();
    assert!(t.mask.has(5, 0, flag::BELOW_NOISE));
    assert_eq!(t.values[5], [0.0; 3]);
    assert!(t.variance[5][0].is_infinite());
    assert_eq!(t.mask.count(flag::BELOW_NOISE), 1);

    // A clipped stone pixel is flagged saturated.
    let mut clipped = stone;
    clipped.pixels[7] = [1.0; 3];
    let t = calibration
        .transmittance(&clipped, &TransmittanceOptions::default())
        .unwrap();
    assert!(t.mask.has(7, 0, flag::SATURATED));
}

#[test]
fn hdr_merge_recovers_radiance_with_a_saturated_exposure() {
    let (w, h) = (16, 16);
    let radiance = |x: usize, y: usize| 1.7f32.mul_add((x + 16 * y) as f32 / 255.0, 0.1);
    let long = image_from(w, h, |x, y, _| (radiance(x, y) * 1.0).min(1.0));
    let short = image_from(w, h, |x, y, _| radiance(x, y) * 0.25);
    let noise = NoiseModel {
        a: [1e-6; 3],
        b: [1e-4; 3],
    };
    let merged = merge_hdr(
        &[
            HdrInput {
                image: &long,
                exposure: Some(1.0),
            },
            HdrInput {
                image: &short,
                exposure: Some(0.25),
            },
        ],
        &noise,
        &HdrOptions::default(),
    )
    .unwrap();
    for y in 0..h {
        for x in 0..w {
            let got = merged.image.get(x, y);
            for c in 0..3 {
                let want = radiance(x, y);
                assert!(
                    (got[c] - want).abs() < 1e-4 * want.max(1.0),
                    "({x},{y}) {got:?}"
                );
            }
        }
    }
    assert_eq!(merged.mask.count(flag::SATURATED), 0);
    assert!(merged.image.variance.is_some());
    assert!(merged.image.full_scale > 1.0);

    // Without any exposure information the merge is refused.
    let err = merge_hdr(
        &[
            HdrInput {
                image: &long,
                exposure: None,
            },
            HdrInput {
                image: &short,
                exposure: None,
            },
        ],
        &noise,
        &HdrOptions::default(),
    );
    assert!(err.is_err());

    // From EXIF: the exposure time (both have an ISO of 100).
    let mut long_meta = long;
    long_meta.meta.exposure_time_s = Some(0.04);
    long_meta.meta.iso = Some(100);
    let mut short_meta = short;
    short_meta.meta.exposure_time_s = Some(0.01);
    short_meta.meta.iso = Some(100);
    let merged = merge_hdr(
        &[
            HdrInput {
                image: &long_meta,
                exposure: None,
            },
            HdrInput {
                image: &short_meta,
                exposure: None,
            },
        ],
        &noise,
        &HdrOptions::default(),
    )
    .unwrap();
    assert!((merged.image.get(15, 15)[0] - radiance(15, 15)).abs() < 1e-3);
}

#[test]
fn noise_model_recovers_read_and_shot_noise() {
    // variance = a + b * signal, drawn as a Gaussian of that variance (the Gaussian limit of
    // Poisson plus read noise, valid for the 50+ electrons per pixel of these signals).
    let (a_true, b_true) = (1.0e-4_f64, 1.0e-3_f64);
    let mut rng = Lcg(0x5EED_1234);
    let size = 128;
    let mut frame = |mean: &dyn Fn(usize, usize) -> f64| {
        let mut pixels = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let m = mean(x, y);
                let sigma = f64::mul_add(b_true, m.max(0.0), a_true).sqrt();
                let mut px = [0.0_f32; 3];
                for slot in &mut px {
                    *slot = to_f32(sigma.mul_add(rng.gauss(), m));
                }
                pixels.push(px);
            }
        }
        LinearImage::from_pixels(size, size, pixels, SourceKind::Raw).unwrap()
    };
    let white_x = frame(&|x, _| 0.05 + 0.85 * x as f64 / 127.0);
    let white_y = frame(&|_, y| 0.05 + 0.85 * y as f64 / 127.0);
    let dark = frame(&|_, _| 0.0);
    let model = estimate_noise(&[&white_x, &white_y, &dark], &NoiseOptions::default()).unwrap();
    for c in 0..3 {
        let (a, b) = (f64::from(model.a[c]), f64::from(model.b[c]));
        assert!((a - a_true).abs() < 0.2 * a_true, "a[{c}] = {a}");
        assert!((b - b_true).abs() < 0.1 * b_true, "b[{c}] = {b}");
    }
    assert!(estimate_noise(&[], &NoiseOptions::default()).is_err());
}

#[test]
fn resample_conserves_the_mean_and_propagates_variance() {
    let (w, h) = (16, 16);
    let mut rng = Lcg(7);
    let values: Vec<[f32; 3]> = (0..w * h)
        .map(|_| [to_f32(rng.unit()), to_f32(rng.unit()), to_f32(rng.unit())])
        .collect();
    let variance = vec![[0.04_f32; 3]; w * h];
    let grid = WorkingGrid::fit([0, 0, 16, 16], 4).unwrap();
    assert_eq!((grid.width, grid.height), (4, 4));
    assert_eq!(grid.scale, 4.0);
    assert_eq!(grid.footprint(1, 2), [4.0, 8.0, 8.0, 12.0]);
    let out = resample(
        &ResampleSource {
            width: w,
            height: h,
            values: &values,
            variance: Some(&variance),
            mask: None,
        },
        &grid,
        &ResampleOptions::default(),
    )
    .unwrap();
    let source_mean: f64 = values.iter().map(|p| f64::from(p[0])).sum::<f64>() / (w * h) as f64;
    let grid_mean: f64 = out.values.iter().map(|p| f64::from(p[0])).sum::<f64>() / 16.0;
    assert!((source_mean - grid_mean).abs() < 1e-6);
    for var in &out.variance {
        assert!((var[1] - 0.04 / 16.0).abs() < 1e-7);
    }
    assert!(out.coverage.iter().all(|&c| (c - 1.0).abs() < 1e-6));
    assert_eq!(out.mask.count(flag::ALL), 0);

    // A saturated source pixel leaves the average of its block.
    let mut mask = PixelMask::new(w, h);
    mask.set(0, 0, flag::SATURATED);
    let masked = resample(
        &ResampleSource {
            width: w,
            height: h,
            values: &values,
            variance: Some(&variance),
            mask: Some(&mask),
        },
        &grid,
        &ResampleOptions::default(),
    )
    .unwrap();
    let block: Vec<f64> = (0..4)
        .flat_map(|j| (0..4).map(move |i| (i, j)))
        .filter(|&(i, j)| (i, j) != (0, 0))
        .map(|(i, j)| f64::from(values[j * w + i][0]))
        .collect();
    let want = block.iter().sum::<f64>() / 15.0;
    assert!((f64::from(masked.values[0][0]) - want).abs() < 1e-6);
    assert!((masked.coverage[0] - 15.0 / 16.0).abs() < 1e-6);
    assert!((masked.variance[0][0] - 0.04 / 15.0).abs() < 1e-7);

    // A fractional scale still covers the whole region.
    let odd = WorkingGrid::fit([0, 0, 10, 10], 4).unwrap();
    assert_eq!((odd.width, odd.height), (4, 4));
    assert!((odd.scale - 2.5).abs() < 1e-12);
    assert!(WorkingGrid::fit([3, 3, 3, 9], 4).is_err());
}

#[test]
fn consistency_report_flags_mismatched_exif() {
    let with_meta = |exposure: f64, iso: u32, wb: u16, size: usize| {
        let mut image = LinearImage::filled(size, size, [0.5; 3], SourceKind::Raw).unwrap();
        image.meta = CaptureMeta {
            exposure_time_s: Some(exposure),
            iso: Some(iso),
            white_balance_mode: Some(wb),
            ..CaptureMeta::default()
        };
        image
    };
    let stone = with_meta(0.01, 100, 1, 8);
    let frames = CalibrationFrames {
        white: vec![with_meta(0.02, 200, 0, 8)],
        dark: vec![with_meta(0.01, 100, 1, 8), with_meta(0.01, 100, 1, 6)],
    };
    let report = check_view(&stone, &frames, &ConsistencyOptions::default());
    let white = FrameRole::White(0);
    assert!(
        report
            .warnings
            .contains(&ConsistencyWarning::ExposureMismatch {
                frame: white,
                stone: 0.01,
                other: 0.02,
            })
    );
    assert!(report.warnings.contains(&ConsistencyWarning::IsoMismatch {
        frame: white,
        stone: 100,
        other: 200,
    }));
    assert!(
        report
            .warnings
            .contains(&ConsistencyWarning::WhiteBalanceMismatch {
                frame: white,
                stone: 1,
                other: 0,
            })
    );
    assert!(report.warnings.contains(&ConsistencyWarning::SizeMismatch {
        frame: FrameRole::Dark(1),
        stone: [8, 8],
        other: [6, 6],
    }));
    // The matching dark frame draws no warning.
    assert!(!report.warnings.iter().any(|w| matches!(
        w,
        ConsistencyWarning::ExposureMismatch {
            frame: FrameRole::Dark(0),
            ..
        } | ConsistencyWarning::IsoMismatch {
            frame: FrameRole::Dark(0),
            ..
        }
    )));
    assert_eq!(report.saturated_fraction, 0.0);

    let mut clipped = stone.clone();
    for pixel in clipped.pixels.iter_mut().take(32) {
        *pixel = [1.0; 3];
    }
    let report = check_view(&clipped, &frames, &ConsistencyOptions::default());
    assert!((report.saturated_fraction - 0.5).abs() < 1e-6);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| matches!(w, ConsistencyWarning::Saturation { .. }))
    );
    let none = check_view(
        &stone,
        &CalibrationFrames::default(),
        &ConsistencyOptions::default(),
    );
    assert!(none.warnings.contains(&ConsistencyWarning::NoWhiteFrame));
}

/// One camera at -Y looking at a 10 mm cube at the origin.
fn cube_scene_parts() -> (crate::rough_plan::RoughMesh, RigProfile) {
    let mesh = box_mesh(DVec3::splat(5.0)).expect("a cube is a valid mesh");
    let view = ViewPose::look_at(
        "front",
        DVec3::new(0.0, -150.0, 0.0),
        DVec3::ZERO,
        DVec3::Z,
        Projection::Pinhole { focal_px: 4000.0 },
        [2048, 1536],
    );
    let rig = RigProfile::new("test", vec![view], 1.5168);
    (mesh, rig)
}

#[test]
fn mesh_masks_mark_outline_edge_inclusion_and_ghosts() {
    let (mesh, rig) = cube_scene_parts();
    let scene = Scene::new(&mesh, &rig, Rigid::IDENTITY);
    let region = stone_region(&scene, 0, 0.1).expect("the cube is in front of the camera");
    assert!(region[0] < 1024 && 1024 < region[2] && region[1] < 768 && 768 < region[3]);
    let grid = WorkingGrid::fit(region, 96).unwrap();
    let (cx, cy) = grid
        .working_coordinates(DVec2::new(1024.0, 768.0))
        .expect("the image centre is in the region");
    let (cx, cy) = (to_index(cx), to_index(cy));

    let options = MeshMaskOptions::default();
    let plain = mesh_masks(&scene, 0, &grid, &[], &options);
    // The region's corner lies outside the cube's outline; its centre inside, clear of the band.
    assert!(plain.has(0, 0, flag::OUTSIDE_OUTLINE));
    assert!(plain.is_clear(cx, cy));
    assert_eq!(plain.count(flag::INCLUSION), 0);
    // Along the middle row, the last inside pixel is in the edge band and a pixel ten further
    // in is not.
    let last_inside = (0..grid.width)
        .rev()
        .find(|&x| !plain.has(x, cy, flag::OUTSIDE_OUTLINE))
        .expect("some pixel of the row is inside");
    assert!(plain.has(last_inside, cy, flag::EDGE_BAND));
    assert!(!plain.has(last_inside - 10, cy, flag::EDGE_BAND));

    // An inclusion at the cube's centre masks the pixels whose rays pass within its radius.
    let marker = InclusionMarker {
        centre: DVec3::ZERO,
        radius_mm: 1.0,
    };
    let masked = mesh_masks(&scene, 0, &grid, &[marker], &options);
    assert!(masked.has(cx, cy, flag::INCLUSION));
    assert!(masked.count(flag::INCLUSION) > 0);
    assert!(masked.count(flag::INCLUSION) < plain.clear_count());
    assert!(!masked.has(last_inside - 10, cy, flag::INCLUSION));

    // Ghosts: every ghost `predict_ghosts` lists inside the grid is flagged.
    let ghosts = predict_ghosts(
        &scene,
        0,
        marker.centre,
        options.max_bounces,
        options.ghost_max_miss_mm,
    );
    for ghost in &ghosts {
        if let Some((gx, gy)) = grid.working_coordinates(DVec2::from_array(ghost.pixel)) {
            assert!(masked.has(to_index(gx), to_index(gy), flag::GHOST));
        }
    }
}

#[test]
fn inclusion_marker_from_shell_reaches_the_shell() {
    let (points, triangles) =
        crate::rough_plan::locate::sphere_shell(DVec3::new(1.0, 2.0, 3.0), 0.5);
    let shell = crate::rough_plan::locate::InclusionShell {
        points,
        triangles,
        margin_mm: 0.3,
    };
    let marker = InclusionMarker::from_shell(&shell).unwrap();
    assert!((marker.centre - DVec3::new(1.0, 2.0, 3.0)).length() < 1e-9);
    assert!(marker.radius_mm >= 0.5 + 0.3);
}

#[test]
fn an_encoded_source_carries_quantisation_and_compression_variance() {
    use super::{COMPRESSION_SIGMA_ENCODED, encoding_variance};
    // RAW is measured as linear: no encoding term.
    assert_eq!(encoding_variance(SourceKind::Raw, false, 0.5), 0.0);
    assert_eq!(encoding_variance(SourceKind::Rgb8, false, 0.5), 0.0);
    // 8 bits at the mid-tone: the derivative of the inverse EOTF at linear 0.2 (encoded 0.484)
    // is 2.4 / 1.055 * ((0.484 + 0.055) / 1.055)^1.4, about 0.90.
    let v = encoding_variance(SourceKind::Rgb8, true, 0.2);
    let sd_encoded = COMPRESSION_SIGMA_ENCODED
        .mul_add(COMPRESSION_SIGMA_ENCODED, (1.0_f32 / 255.0).powi(2) / 12.0);
    let slope = 2.4_f32 / 1.055 * ((linear_to_srgb(0.2) + 0.055) / 1.055).powf(1.4);
    assert!((slope * slope).mul_add(-sd_encoded, v).abs() < 1e-9, "{v}");
    // A 16-bit non-linear file has no compression term, so far less variance.
    assert!(encoding_variance(SourceKind::Tiff16, true, 0.2) < 1e-3 * v);
    // The encoding term reaches the transmittance variance of a JPEG-like pair of frames.
    let (w, h) = (16, 16);
    let grey = |level: f32, source: SourceKind| {
        LinearImage::from_pixels(w, h, vec![[level; 3]; w * h], source).unwrap()
    };
    let noise = NoiseModel {
        a: [1e-6; 3],
        b: [1e-5; 3],
    };
    let variance_of = |source: SourceKind| {
        let calibration = ViewCalibration {
            frames: CalibrationFrames {
                white: vec![grey(0.7, source)],
                dark: Vec::new(),
            },
            noise,
        };
        calibration
            .transmittance(&grey(0.35, source), &TransmittanceOptions::default())
            .unwrap()
            .variance[0][0]
    };
    // A perfectly flat white frame shows no compression error (round 3, D4.2 estimates it from
    // the frame), so only the 8-bit quantisation is added: 1.2 times, not the five times of the
    // assumed constant.
    assert!(variance_of(SourceKind::Rgb8) > 1.2 * variance_of(SourceKind::Raw));
}

/// Round 3, D4.2: the compression sigma comes from the local variance of the white frame.
#[test]
fn the_compression_sigma_is_estimated_from_the_white_frame() {
    use super::{COMPRESSION_SIGMA_MAX, estimate_compression_sigma, srgb_to_linear};
    let (w, h) = (32, 32);
    let noise = NoiseModel {
        a: [1e-12; 3],
        b: [0.0; 3],
    };
    // Encoded 0.8 with a pseudo-random +-2 codes: sigma = 2 / 255 in the encoded domain.
    let sigma_codes = 2.0_f32;
    let jittered: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let mut pixel = [0.0_f32; 3];
            for (c, slot) in pixel.iter_mut().enumerate() {
                let hash = (i as u64 * 3 + c as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let sign = if (hash >> 40) & 1 == 0 { -1.0 } else { 1.0 };
                *slot = srgb_to_linear(0.8 + sign * sigma_codes / 255.0);
            }
            pixel
        })
        .collect();
    let image = LinearImage::from_pixels(w, h, jittered, SourceKind::Rgb8).unwrap();
    let estimate = estimate_compression_sigma(&image, &noise, 0.98).expect("an 8-bit frame");
    let expected = sigma_codes / 255.0;
    for (c, value) in estimate.iter().enumerate() {
        assert!(
            (value - expected).abs() < 0.15 * expected,
            "channel {c}: estimated {value}, expected {expected}"
        );
    }
    // A flat frame has none, and the estimate never exceeds the cap.
    let flat = LinearImage::from_pixels(w, h, vec![[0.5; 3]; w * h], SourceKind::Rgb8).unwrap();
    let flat_estimate = estimate_compression_sigma(&flat, &noise, 0.98).unwrap();
    assert!(flat_estimate.iter().all(|v| *v < 1e-3), "{flat_estimate:?}");
    assert!(estimate.iter().all(|v| *v <= COMPRESSION_SIGMA_MAX));
    // A RAW frame has no compression term to estimate.
    let raw = LinearImage::from_pixels(w, h, vec![[0.5; 3]; w * h], SourceKind::Raw).unwrap();
    assert!(estimate_compression_sigma(&raw, &noise, 0.98).is_none());
}

/// ZC-3: the stone's compression error is estimated from the stone frame by a robust Laplacian,
/// and it is what the stone's transmittance variance carries.
#[test]
#[allow(
    clippy::too_many_lines,
    clippy::suboptimal_flops,
    reason = "one end-to-end oracle test; the arithmetic mirrors the estimator's unfused form"
)]
fn the_stone_compression_sigma_is_estimated_from_the_stone_frame() {
    use super::{
        COMPRESSION_SIGMA_MAX, decode_standard_bytes, estimate_compression_sigma,
        estimate_stone_compression_sigma,
    };
    use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
    let (w, h) = (96_usize, 96_usize);
    // A smooth gradient (0.2 -> 0.6 linear) has no Laplacian, so what the decoded frame shows
    // there is the codec's own error plus the 8-bit rounding. The sensor noise is nil (a tiny
    // noise model): a codec that smooths real sensor noise away reads low, see the estimator's
    // doc. Quality 70 so the error is clearly above the rounding.
    let truth_linear = |x: usize, y: usize| 0.2 + 0.4 * ((x + y) as f32 / (w + h - 2) as f32);
    let noise = NoiseModel {
        a: [1e-12; 3],
        b: [0.0; 3],
    };
    let mut raw_codes = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                // Channels differ by a fixed offset so the three Laplacians are independent.
                let level = truth_linear(x, y) * (1.0 - 0.1 * c as f32);
                raw_codes.push(to_code((linear_to_srgb(level) * 255.0).round()));
            }
        }
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 70)
        .encode(
            &raw_codes,
            u32::try_from(w).expect("small test frame"),
            u32::try_from(h).expect("small test frame"),
            ExtendedColorType::Rgb8,
        )
        .expect("the JPEG encodes");
    let decoded = decode_standard_bytes(&bytes).expect("the JPEG decodes");
    let estimate = estimate_stone_compression_sigma(&decoded, &noise, None, 0.98)
        .expect("an 8-bit stone frame");
    assert!(estimate.iter().all(|v| *v <= COMPRESSION_SIGMA_MAX));

    // The oracle: the same robust Laplacian spread, taken on the ERROR image (decoded minus the
    // smooth truth, encoded domain), with the rounding added back to the estimate.
    let quantisation = 1.0_f64 / (255.0 * 255.0 * 12.0);
    for (c, &sigma) in estimate.iter().enumerate() {
        let error = |x: usize, y: usize| {
            let truth = linear_to_srgb(truth_linear(x, y) * (1.0 - 0.1 * c as f32));
            f64::from(linear_to_srgb(decoded.pixels[y * w + x][c]) - truth)
        };
        let mut laplacians = Vec::new();
        let mut squares = 0.0_f64;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let l = 4.0 * error(x, y)
                    - error(x - 1, y)
                    - error(x + 1, y)
                    - error(x, y - 1)
                    - error(x, y + 1);
                squares += l * l;
                laplacians.push(l);
            }
        }
        let n = laplacians.len();
        laplacians.sort_by(f64::total_cmp);
        let centre = laplacians[n / 2];
        let mut deviations: Vec<f64> = laplacians.iter().map(|l| (l - centre).abs()).collect();
        deviations.sort_by(f64::total_cmp);
        let robust = deviations[n / 2] / 0.6745 / 20.0_f64.sqrt();
        let rms = (squares / n as f64 / 20.0).sqrt();
        let reported = f64::from(sigma).hypot(quantisation.sqrt());
        println!(
            "channel {c}: estimate {:.3} codes (with rounding {:.3}), robust oracle {:.3}, \
             rms oracle {:.3}",
            f64::from(sigma) * 255.0,
            reported * 255.0,
            robust * 255.0,
            rms * 255.0
        );
        assert!(
            robust > 0.3 / 255.0,
            "channel {c}: the test error is below the rounding, so it proves nothing"
        );
        assert!(
            (reported - robust).abs() < 0.25 * robust,
            "channel {c}: estimated {reported}, actual {robust}"
        );
    }

    // Without texture the stone estimate is the white-frame estimate: both are zero.
    let flat = LinearImage::from_pixels(w, h, vec![[0.5; 3]; w * h], SourceKind::Rgb8).unwrap();
    let from_stone = estimate_stone_compression_sigma(&flat, &noise, None, 0.98).unwrap();
    let from_white = estimate_compression_sigma(&flat, &noise, 0.98).unwrap();
    for c in 0..3 {
        assert!(
            from_stone[c] < 1e-3 && from_white[c] < 1e-3,
            "{from_stone:?} {from_white:?}"
        );
    }

    // A RAW source, a mask of another size and a fully masked frame give no estimate.
    let raw = LinearImage::from_pixels(w, h, vec![[0.5; 3]; w * h], SourceKind::Raw).unwrap();
    assert!(estimate_stone_compression_sigma(&raw, &noise, None, 0.98).is_none());
    let small = PixelMask::new(w - 1, h);
    assert!(estimate_stone_compression_sigma(&decoded, &noise, Some(&small), 0.98).is_none());
    let mut blocked = PixelMask::new(w, h);
    for y in 0..h {
        for x in 0..w {
            blocked.set(x, y, flag::OUTSIDE_OUTLINE);
        }
    }
    assert!(estimate_stone_compression_sigma(&decoded, &noise, Some(&blocked), 0.98).is_none());

    // The stone's variance in the transmittance carries the stone's estimate, not the flat white
    // frame's zero: a textured JPEG stone against a flat white frame.
    let calibration = ViewCalibration {
        frames: CalibrationFrames {
            white: vec![
                LinearImage::from_pixels(w, h, vec![[0.8; 3]; w * h], SourceKind::Rgb8).unwrap(),
            ],
            dark: Vec::new(),
        },
        noise,
    };
    let textured = calibration
        .transmittance(&decoded, &TransmittanceOptions::default())
        .unwrap();
    let mut as_raw = decoded.clone();
    as_raw.source = SourceKind::Raw;
    let raw_stone = calibration
        .transmittance(&as_raw, &TransmittanceOptions::default())
        .unwrap();
    let probe = (h / 2) * w + w / 2;
    assert!(
        textured.variance[probe][0] > raw_stone.variance[probe][0],
        "the encoding term reaches the stone's variance"
    );
    let masked = calibration
        .transmittance_masked(
            &decoded,
            &TransmittanceOptions::default(),
            Some(&PixelMask::new(w, h)),
        )
        .unwrap();
    assert_eq!(masked.variance, textured.variance);
    assert!(
        calibration
            .transmittance_masked(&decoded, &TransmittanceOptions::default(), Some(&small))
            .is_err()
    );
}

/// S2 (root-cause analysis R4): the per-8x8-block DC offset of a stone frame is estimated from the
/// block means.
///
/// The oracle frame is built, not encoded: a smooth encoded-domain gradient plus a known Gaussian
/// offset per 8 x 8 block (sigma 0.9 code) plus independent pixel noise (sigma 0.9 code), rounded
/// to 8 bits. A real JPEG cannot serve as the 30 % oracle here: at quality 90 the DC step is only
/// 3 coefficient units (0.1 code of block mean, below the pixel noise floor), and on a noise-free
/// gradient the DC error is a deterministic sawtooth that is locally smooth, so no Laplacian can
/// see it. The codec is covered by the smoke checks at the end.
fn block_dc_oracle_frame() -> (LinearImage, NoiseModel) {
    use super::JPEG_BLOCK;
    let (w, h) = (160_usize, 160_usize);
    let blocks_across = w / JPEG_BLOCK;
    let sigma_dc_codes = 0.9_f64;
    let sigma_pixel_codes = 0.9_f64;
    let mut rng = Lcg(0x5EED_0B10);
    let offsets: Vec<[f64; 3]> = (0..blocks_across * (h / JPEG_BLOCK))
        .map(|_| [0; 3].map(|_| rng.gauss() * sigma_dc_codes))
        .collect();
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let block = (y / JPEG_BLOCK) * blocks_across + x / JPEG_BLOCK;
            let level = 0.45 + 0.1 * (x + y) as f64 / (w + h - 2) as f64;
            let mut pixel = [0.0_f32; 3];
            for (c, slot) in pixel.iter_mut().enumerate() {
                #[allow(
                    clippy::suboptimal_flops,
                    reason = "pinned-seed synthetic pixel codes: fusing would change the rounded 8-bit data"
                )]
                let code = (level * 255.0 + offsets[block][c] + rng.gauss() * sigma_pixel_codes)
                    .round()
                    .clamp(0.0, 255.0);
                *slot = srgb_to_linear(to_f32(code) / 255.0);
            }
            pixels.push(pixel);
        }
    }
    let noise = NoiseModel {
        a: [1e-12; 3],
        b: [0.0; 3],
    };
    let frame = LinearImage::from_pixels(w, h, pixels, SourceKind::Rgb8).unwrap();
    (frame, noise)
}

#[test]
fn the_block_dc_error_matches_the_known_block_offsets_within_thirty_percent() {
    use super::{estimate_stone_compression, estimate_stone_compression_sigma};
    let sigma_dc_codes = 0.9_f64;
    let (frame, noise) = block_dc_oracle_frame();
    let estimate = estimate_stone_compression(&frame, &noise, None, 0.98).expect("an 8-bit frame");
    let ac_only = estimate_stone_compression_sigma(&frame, &noise, None, 0.98).unwrap();
    assert_eq!(estimate.ac, ac_only, "the AC part is the old estimate");
    for (c, block_dc) in estimate.block_dc.iter().enumerate() {
        let variance_codes = (f64::from(*block_dc) * 255.0).powi(2);
        let expected = sigma_dc_codes * sigma_dc_codes;
        assert!(
            (variance_codes - expected).abs() < 0.3 * expected,
            "channel {c}: block-DC variance {variance_codes} codes^2, expected {expected}"
        );
    }
}

/// A frame with fewer than nine usable interior blocks reports no block-DC term, but keeps its
/// AC estimate.
#[test]
fn a_small_frame_reports_no_block_dc_term() {
    use super::estimate_stone_compression;
    let (w, _h) = (160_usize, 160_usize);
    let (frame, noise) = block_dc_oracle_frame();
    let small = LinearImage::from_pixels(
        24,
        24,
        (0..24 * 24)
            .map(|i| frame.pixels[(i / 24) * w + i % 24])
            .collect(),
        SourceKind::Rgb8,
    )
    .unwrap();
    let small_estimate = estimate_stone_compression(&small, &noise, None, 0.98).unwrap();
    assert_eq!(small_estimate.block_dc, [0.0; 3]);
}

/// A mask flag inside every block removes every block.
#[test]
fn a_mask_flag_in_every_block_removes_the_block_dc_term() {
    use super::{JPEG_BLOCK, estimate_stone_compression};
    let (w, h) = (160_usize, 160_usize);
    let blocks_across = w / JPEG_BLOCK;
    let (frame, noise) = block_dc_oracle_frame();
    let mut speckled = PixelMask::new(w, h);
    for by in 0..h / JPEG_BLOCK {
        for bx in 0..blocks_across {
            speckled.set(
                bx * JPEG_BLOCK + 3,
                by * JPEG_BLOCK + 3,
                flag::OUTSIDE_OUTLINE,
            );
        }
    }
    let masked = estimate_stone_compression(&frame, &noise, Some(&speckled), 0.98).unwrap();
    assert_eq!(masked.block_dc, [0.0; 3]);
}

/// Smoke check on a real quality 90 JPEG of a smooth gradient: both parts exist and respect
/// the cap, and the AC part is the old estimate.
#[test]
fn a_quality_90_jpeg_gives_a_capped_block_dc_term_and_the_old_ac_estimate() {
    use super::{
        COMPRESSION_SIGMA_MAX, decode_standard_bytes, estimate_stone_compression,
        estimate_stone_compression_sigma,
    };
    use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
    let noise = NoiseModel {
        a: [1e-12; 3],
        b: [0.0; 3],
    };
    let (jw, jh) = (96_usize, 96_usize);
    let mut codes = Vec::with_capacity(jw * jh * 3);
    for y in 0..jh {
        for x in 0..jw {
            for c in 0..3 {
                #[allow(
                    clippy::suboptimal_flops,
                    reason = "synthetic gradient quantised to 8-bit codes: fusing would change the encoded data"
                )]
                let level =
                    (0.2 + 0.4 * ((x + y) as f32 / (jw + jh - 2) as f32)) * (1.0 - 0.1 * c as f32);
                codes.push(to_code((linear_to_srgb(level) * 255.0).round()));
            }
        }
    }
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode(
            &codes,
            u32::try_from(jw).expect("small test frame"),
            u32::try_from(jh).expect("small test frame"),
            ExtendedColorType::Rgb8,
        )
        .expect("the JPEG encodes");
    let decoded = decode_standard_bytes(&bytes).expect("the JPEG decodes");
    let jpeg = estimate_stone_compression(&decoded, &noise, None, 0.98).expect("an 8-bit frame");
    assert_eq!(
        jpeg.ac,
        estimate_stone_compression_sigma(&decoded, &noise, None, 0.98).unwrap()
    );
    assert!(
        jpeg.block_dc
            .iter()
            .all(|v| (0.0..=COMPRESSION_SIGMA_MAX).contains(v))
    );
}

/// S2: the per-pixel variance averages down onto the working grid, the block-DC variance only by
/// the block count (`var_dc * min(1, 64 / N)`), and a transmittance without a block term resamples
/// exactly as before.
#[test]
fn the_block_dc_variance_is_propagated_as_correlated_within_a_block() {
    use super::TransmittanceImage;
    let (w, h) = (16_usize, 16_usize);
    let (per_pixel, block) = (1e-4_f32, 4e-4_f32);
    let make = |block_variance: f32| TransmittanceImage {
        width: w,
        height: h,
        values: vec![[0.5; 3]; w * h],
        variance: vec![[per_pixel + block_variance; 3]; w * h],
        block_variance: vec![[block_variance; 3]; w * h],
        mask: PixelMask::new(w, h),
    };
    let close = |found: f32, expected: f32| {
        assert!(
            (found - expected).abs() <= 1e-3 * expected,
            "found {found}, expected {expected}"
        );
    };
    let options = ResampleOptions::default();
    let with_block = make(block);
    for (scale, side, expected) in [
        // One photo pixel per working pixel: nothing averages.
        (1.0_f64, 16, per_pixel + block),
        // One block per working pixel: the pixel part averages by 64, the block part not at all.
        (8.0, 2, per_pixel / 64.0 + block),
        // Four blocks per working pixel: the block part averages by 4 (= 64 / 256 * 64 / 64).
        (16.0, 1, per_pixel / 256.0 + block / 4.0),
    ] {
        let grid = WorkingGrid {
            width: side,
            height: side,
            origin: [0.0, 0.0],
            scale,
        };
        let working = with_block.to_working(&grid, &options).unwrap();
        for v in &working.variance {
            for &channel in v {
                close(channel, expected);
            }
        }
    }
    // Without a block term the old resampling is unchanged, bit for bit.
    let plain = make(0.0);
    let grid = WorkingGrid {
        width: 2,
        height: 2,
        origin: [0.0, 0.0],
        scale: 8.0,
    };
    let direct = resample(
        &ResampleSource {
            width: w,
            height: h,
            values: &plain.values,
            variance: Some(&plain.variance),
            mask: Some(&plain.mask),
        },
        &grid,
        &options,
    )
    .unwrap();
    assert_eq!(plain.to_working(&grid, &options).unwrap(), direct);
}

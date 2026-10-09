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
    jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
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
                    *slot = sigma.mul_add(rng.gauss(), m) as f32;
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
        .map(|_| [rng.unit() as f32, rng.unit() as f32, rng.unit() as f32])
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
    let (cx, cy) = (cx as usize, cy as usize);

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
            assert!(masked.has(gx as usize, gy as usize, flag::GHOST));
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
    let noisy: Vec<[f32; 3]> = (0..w * h)
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
    let image = LinearImage::from_pixels(w, h, noisy, SourceKind::Rgb8).unwrap();
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
